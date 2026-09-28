// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 分区表 PointGet / BatchPointGet / IndexLookup / TableScan 的 plan cache 性能基准。
//
// 对应 Go `bench_test.go`：在非分区与 HASH/KEY/LIST/RANGE（含表达式与 columns）
// 分区定义下，分别测量普通 SQL 与 prepared statement 开启/关闭计划缓存（plan cache）
// 的吞吐。PointGet 为等值主键点查；BatchPointGet 为多点/IN；IndexLookup 经二级索引回表；
// TableScan 为全表/范围扫描。会话与执行路径使用真实的 mock-store 测试运行时。

// 分区表 PointGet/BatchPointGet/IndexLookup/TableScan 的普通与 prepared plan cache benchmark。
#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::sync::Arc;

use astersql_session::runtime::{ConcreteSession, CreateAnalyzeSession};
use astersql_session::testutil::{TestRecordSet, TestSession};

// 全局查询和分区定义对应 Go var 块；字符串内容保持原 SQL/DDL 形状。
/// 主键等值点查 SQL（非 prepared）。
pub const pointQuery: &str = "select * from t where id = 1";
/// 主键等值点查 SQL（prepared，占位符 `?`）。
pub const pointQueryPrepared: &str = "select * from t where id = ?";
/// 期望计划算子名：Point_Get。
pub const expectedPointPlan: &str = "Point_Get";
/// 多点 OR 主键查询 SQL（非 prepared）。
pub const batchPointQuery: &str =
    "select * from t where id = 1 or id = 5000 or id = 2 or id = 100000";
/// 多点 IN 主键查询 SQL（prepared）。
pub const batchPointQueryPrepared: &str = "select * from t where id IN (?,?,?)";
/// 期望计划算子名：Batch_Point_Get。
pub const expectedBatchPointPlan: &str = "Batch_Point_Get";
/// 期望计划算子名：IndexLookUp（二级索引回表）。
pub const expectedIndexPlan: &str = "IndexLookUp";
/// 期望计划算子名：TableReader（表扫描）。
pub const expectedTableScanPlan: &str = "TableReader";
/// HASH(id) 七分区定义片段。
pub const partitionByHash: &str = "partition by hash(id) partitions 7";
/// HASH(表达式) 七分区定义片段。
pub const partitionByHashExpr: &str = "partition by hash(floor(id*0.5)) partitions 7";
/// KEY(id) 七分区定义片段。
pub const partitionByKey: &str = "partition by key(id) partitions 7";
/// RANGE(id) 分区定义（含 maxvalue）。
pub const partitionByRange: &str = "partition by range(id) (partition p0 values less than (10), partition p1 values less than (1000), partition p3 values less than (100000), partition pMax values less than (maxvalue))";
/// RANGE(表达式) 分区定义。
pub const partitionByRangeExpr: &str = "partition by range(floor(id*0.5)) (partition p0 values less than (10), partition p1 values less than (1000), partition p3 values less than (100000), partition pMax values less than (maxvalue))";
/// RANGE COLUMNS(id) 分区定义。
pub const partitionByRangeColumns: &str = "partition by range columns (id) (partition p0 values less than (10), partition p1 values less than (1000), partition p3 values less than (100000), partition pMax values less than (maxvalue))";
/// prepared 点查绑定参数。
pub const pointArgs: i32 = 1;
/// prepared 批量点查绑定参数列表。
pub const batchArgs: &[i32] = &[2, 10000, 1];
/// prepared 场景下更细粒度的 RANGE(id) 分区定义。
pub const partitionByRangePrep: &str = "partition by range (id) (partition p0 values less than (10), partition p1 values less than (63), partition p3 values less than (100), partition pMax values less than (maxvalue))";
/// prepared 场景下 RANGE(表达式) 分区定义。
pub const partitionByRangeExprPrep: &str = "partition by range (floor(id*0.5)*2) (partition p0 values less than (10), partition p1 values less than (63), partition p3 values less than (100), partition pMax values less than (maxvalue))";
/// prepared 场景下 RANGE COLUMNS(id) 分区定义。
pub const partitionByRangeColumnsPrep: &str = "partition by range columns (id) (partition p0 values less than (10), partition p1 values less than (63), partition p3 values less than (100), partition pMax values less than (maxvalue))";

// accessType 对应 Go 的 pointGet/indexLookup/tableScan 枚举。
/// 访问路径枚举：点查 / 索引回表 / 表扫描，对应 Go pointGet/indexLookup/tableScan。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum accessType {
    pointGet,
    indexLookup,
    tableScan,
}

// BenchSessionDraft 承载 Go prepareBenchSession 返回的 session/domain/storage 三件套。
/// benchmark 会话：绑定真实 mock store、domain 和可执行测试会话。
#[derive(Clone)]
pub struct BenchSessionDraft {
    session: Arc<ConcreteSession>,
    _domain: Arc<astersql_domain::Domain>,
}

// prepareBenchSession 对应 Go 同名函数。
// Go 会关闭 slow log、创建 mockstore、BootstrapSession、CreateSession4Test，并执行 use test。
/// 对应 Go 同名函数：关闭 slow log、建 mockstore、BootstrapSession、CreateSession4Test，
/// 并执行 `use test`。
pub fn prepareBenchSession() -> BenchSessionDraft {
    let (domain, session) = CreateAnalyzeSession()
        .unwrap_or_else(|error| panic!("create benchmark session failed: {error}"));
    let bench = BenchSessionDraft {
        session: Arc::new(session),
        _domain: domain,
    };
    mustExecute(&bench, "use test", &[]);
    bench
}

// mustExecute 对应 Go 的 ExecuteInternal 包装。
// 它设置 10 秒 timeout 和 InternalTxnBootstrap source type；错误时 Fatal 并带 SQL 与 stack。
/// 对应 Go ExecuteInternal 包装：设超时与 InternalTxnBootstrap，错误时 Fatal。
pub fn mustExecute(session: &BenchSessionDraft, sql: &str, args: &[i32]) {
    let result = if args.is_empty() {
        session.session.Execute(sql).map(|mut sets| {
            for mut set in sets.drain(..) {
                set.Close()
                    .unwrap_or_else(|error| panic!("close {sql}: {error}"));
            }
        })
    } else {
        let stmt_id = session
            .session
            .PrepareStmt(sql)
            .unwrap_or_else(|error| panic!("prepare {sql}: {error}"));
        let params = args.iter().map(ToString::to_string).collect::<Vec<_>>();
        session
            .session
            .ExecutePreparedStmt(stmt_id, &params)
            .map(|record_set| {
                if let Some(mut record_set) = record_set {
                    record_set
                        .Close()
                        .unwrap_or_else(|error| panic!("close {sql}: {error}"));
                }
            })
    };
    result.unwrap_or_else(|error| panic!("mustExecute failed for {sql}: {error}"));
}

// drainRecordSet 对应 Go 的 chunk 循环读取。
// Go 不断调用 RecordSet.Next，遇到空 chunk 返回，循环中 Renew chunk 并收集 Row。
/// 对应 Go chunk 循环读取 RecordSet，直到空 chunk。
pub fn drainRecordSet(
    _ctx: ContextDraft,
    mut record_set: RecordSetDraft,
    _alloc: ChunkAllocatorDraft,
) -> Result<Vec<RowDraft>, String> {
    let _ = (_ctx, _alloc);
    if record_set.closed {
        return Err("record set is closed".to_owned());
    }
    let rows = record_set.rows.split_off(record_set.next_row);
    record_set.next_row += rows.len();
    record_set.closed = true;
    Ok(rows)
}

// runPointSelect 对应普通 SQL benchmark 的主循环。
// enablePlanCache=true 时先打开 non-prepared plan cache，并用 tTemp 两条查询冲刷旧缓存；Point_Get/Batch_Point_Get 因 FastPlan 不期望命中。
/// 普通 SQL benchmark 主循环；可选开启非 prepared plan cache 并校验命中。
pub fn runPointSelect(
    session: &BenchSessionDraft,
    iterations: usize,
    query: &str,
    expected_plan: &str,
    enable_plan_cache: bool,
) {
    if enable_plan_cache {
        mustExecute(session, "set tidb_enable_non_prepared_plan_cache = 1", &[]);
        mustExecute(session, "set tidb_session_plan_cache_size = 0", &[]);
        mustExecute(session, "create table tTemp (id int)", &[]);
        mustExecute(session, "insert into tTemp values (1)", &[]);
        execute_and_drain(session, "select * from tTemp where id = 1 or id = 9");
        execute_and_drain(session, "select * from tTemp where id = 1 or id IN (2,5)");
        mustExecute(session, "drop table tTemp", &[]);
        mustExecute(session, "set tidb_session_plan_cache_size = default", &[]);
    } else {
        mustExecute(session, "set tidb_enable_non_prepared_plan_cache = 0", &[]);
    }
    let check_hits = explain_and_check_plan(session, query, expected_plan);
    let expect_hits = enable_plan_cache
        && expected_plan != expectedPointPlan
        && expected_plan != expectedBatchPointPlan;
    let mut hits = 0;
    for _ in 0..iterations {
        execute_and_drain(session, query);
        if check_hits && session.session.LastPlanFromCache() {
            hits += 1;
        }
    }
    if !expect_hits && check_hits && hits > 0 {
        eprintln!(
            "not expected plan cache to be used with PointGet: hits={hits}, iterations={iterations}"
        );
    }
    if expect_hits && check_hits && hits == 0 && iterations > 5 {
        eprintln!(
            "expected plan cache to be used with PointSelect: hits={hits}, iterations={iterations}"
        );
    }
}

// preparePointGet 对应 Go 的主键表准备逻辑。
/// 准备主键表 fixture（可附加分区定义）并 analyze。
pub fn preparePointGet(session: &BenchSessionDraft, partition_by: &str) {
    mustExecute(session, "drop table if exists t", &[]);
    mustExecute(
        session,
        &format!("CREATE TABLE t (id int primary key, d varchar(255), key (d)) {partition_by}"),
        &[],
    );
    mustExecute(
        session,
        "insert into t (id) values (1), (8), (5000), (10000), (100000)",
        &[],
    );
    mustExecute(session, "analyze table t", &[]);
}

// insert1kRows 对应 Go 通过 insert-select 倍增到 1024 行的 fixture。
/// 通过 insert-select 倍增插入约 1024 行，便于 IndexLookup 优于 TableScan。
pub fn insert1kRows(session: &BenchSessionDraft) {
    // 每次 insert-select 让行数翻倍：8、16、32、64、128、256、512、1024。
    mustExecute(
        session,
        "insert into t values (1,1), (5000,5000), (10000,10000), (99900,99900)",
        &[],
    );
    for offset in [1, 2, 4, 8, 16, 32, 64, 128] {
        mustExecute(
            session,
            &format!("insert into t select id + {offset}, d from t"),
            &[],
        );
    }
    mustExecute(session, "analyze table t", &[]);
}

// prepareIndexLookup 对应 Go 的二级索引表准备逻辑。
/// 准备带 idx_id 二级索引的表并灌数。
pub fn prepareIndexLookup(session: &BenchSessionDraft, partition_by: &str) {
    mustExecute(session, "drop table if exists t", &[]);
    mustExecute(
        session,
        &format!("CREATE TABLE t (id int, d varchar(255), key idx_id (id), key(d)) {partition_by}"),
        &[],
    );
    insert1kRows(session);
}

// prepareTableScan 对应 Go 的无 idx_id 表准备逻辑。
/// 准备无 idx_id 的表并灌数，迫使 TableScan。
pub fn prepareTableScan(session: &BenchSessionDraft, partition_by: &str) {
    mustExecute(session, "drop table if exists t", &[]);
    mustExecute(
        session,
        &format!("CREATE TABLE t (id int, d varchar(255), key(d)) {partition_by}"),
        &[],
    );
    insert1kRows(session);
}

// benchmarkPointGetPlanCache 对应 Go 的组合 benchmark：同一 session 内依次跑 PointGet、BatchPointGet、IndexLookup 和 TableScan 子项。
/// 组合 benchmark：同一会话内依次跑 PointGet/Batch/IndexLookup/TableScan。
pub fn benchmarkPointGetPlanCache(iterations: usize, partition_by: &str) {
    let session = prepareBenchSession();
    preparePointGet(&session, partition_by);
    runPointSelect(&session, iterations, pointQuery, expectedPointPlan, true);
    runPointSelect(&session, iterations, pointQuery, expectedPointPlan, false);
    runPointSelect(
        &session,
        iterations,
        batchPointQuery,
        if partition_by.is_empty() {
            expectedBatchPointPlan
        } else {
            expectedTableScanPlan
        },
        true,
    );
    runPointSelect(
        &session,
        iterations,
        batchPointQuery,
        if partition_by.is_empty() {
            expectedBatchPointPlan
        } else {
            expectedTableScanPlan
        },
        false,
    );
    prepareIndexLookup(&session, partition_by);
    runPointSelect(&session, iterations, pointQuery, expectedIndexPlan, true);
    runPointSelect(&session, iterations, pointQuery, expectedIndexPlan, false);
    runPointSelect(
        &session,
        iterations,
        batchPointQuery,
        expectedIndexPlan,
        true,
    );
    runPointSelect(
        &session,
        iterations,
        batchPointQuery,
        expectedIndexPlan,
        false,
    );
    mustExecute(&session, "alter table t drop index idx_id", &[]);
    mustExecute(&session, "analyze table t", &[]);
    runPointSelect(
        &session,
        iterations,
        pointQuery,
        expectedTableScanPlan,
        true,
    );
    runPointSelect(
        &session,
        iterations,
        pointQuery,
        expectedTableScanPlan,
        false,
    );
    runPointSelect(
        &session,
        iterations,
        batchPointQuery,
        expectedTableScanPlan,
        true,
    );
    runPointSelect(
        &session,
        iterations,
        batchPointQuery,
        expectedTableScanPlan,
        false,
    );
}

// runBenchmark 对应 Go 的单入口普通 SQL benchmark。
/// 单入口普通 SQL benchmark：按 accessType 准备表后跑 runPointSelect。
pub fn runBenchmark(
    iterations: usize,
    partition_by: &str,
    query: &str,
    expected_plan: &str,
    access: accessType,
    enable_plan_cache: bool,
) {
    let session = prepareBenchSession();
    match access {
        accessType::pointGet => preparePointGet(&session, partition_by),
        accessType::indexLookup => prepareIndexLookup(&session, partition_by),
        accessType::tableScan => prepareTableScan(&session, partition_by),
    }
    runPointSelect(
        &session,
        iterations,
        query,
        expected_plan,
        enable_plan_cache,
    );
}

// getListPartitionDef 对应 Go 动态拼接 LIST/LIST COLUMNS 分区定义。
// use_columns=true 时保留 `partition by list columns(expr)` 形状，否则使用表达式 list 分区。
/// 动态拼接 LIST / LIST COLUMNS 分区定义字符串。
pub fn getListPartitionDef(expr: &str, use_columns: bool) -> String {
    let mut definition = format!(
        "partition by list{}({}) (",
        if use_columns { " columns" } else { "" },
        expr
    );
    for (part_id, start) in [1, 5000, 10000, 99900].into_iter().enumerate() {
        if part_id > 0 {
            definition.push(',');
        }
        let mut values = (0..256)
            .map(|offset| (start + offset).to_string())
            .collect::<Vec<_>>();
        if !expr.is_empty() && start == 1 {
            values.push("0".to_owned());
        }
        definition.push_str(&format!(
            "partition p{part_id} values in ({})",
            values.join(",")
        ));
    }
    definition.push(')');
    definition
}

// runPreparedPointSelect 对应 prepared statement benchmark 主循环。
// Go 先 PrepareStmt，再把 args 转 expression.Args2Expressions4Test，循环 ExecutePreparedStmt 并统计 FoundInPlanCache。
/// prepared statement benchmark 主循环：Prepare 后重复 Execute 并观察缓存命中。
pub fn runPreparedPointSelect(
    session: &BenchSessionDraft,
    iterations: usize,
    query: &str,
    enable_plan_cache: bool,
    args: &[i32],
) {
    set_prepared_plan_cache(session, enable_plan_cache);
    prepare_stmt(session, query);
    let using_clause = bind_prepared_args(session, args);
    let mut hits = 0;
    for i in 0..iterations {
        execute_prepared_and_drain(session, &using_clause);
        let hit = session.session.LastPlanFromCache();
        if hit {
            hits += 1;
        }
        if enable_plan_cache && i > 0 {
            if hit {
                hits += 1;
            } else {
                eprintln!("no prepared plan cache hit at iteration {i}");
            }
        }
    }
    if enable_plan_cache && hits < iterations / 2 {
        eprintln!("prepared plan cache was not used enough: hits={hits}, iterations={iterations}");
    }
}

// benchPreparedPointGet 对应 Go 的 prepared 组合 benchmark。
/// prepared 组合 benchmark：点查/批量/索引/拆索引后的表扫描路径。
pub fn benchPreparedPointGet(iterations: usize, partition_by: &str) {
    let session = prepareBenchSession();
    preparePointGet(&session, partition_by);
    runPreparedPointSelect(
        &session,
        iterations,
        pointQueryPrepared,
        false,
        &[pointArgs],
    );
    runPreparedPointSelect(&session, iterations, pointQueryPrepared, true, &[pointArgs]);
    runPreparedPointSelect(
        &session,
        iterations,
        batchPointQueryPrepared,
        false,
        batchArgs,
    );
    runPreparedPointSelect(
        &session,
        iterations,
        batchPointQueryPrepared,
        true,
        batchArgs,
    );
    prepareIndexLookup(&session, partition_by);
    runPreparedPointSelect(
        &session,
        iterations,
        pointQueryPrepared,
        false,
        &[pointArgs],
    );
    runPreparedPointSelect(&session, iterations, pointQueryPrepared, true, &[pointArgs]);
    runPreparedPointSelect(
        &session,
        iterations,
        batchPointQueryPrepared,
        false,
        batchArgs,
    );
    runPreparedPointSelect(
        &session,
        iterations,
        batchPointQueryPrepared,
        true,
        batchArgs,
    );
    mustExecute(&session, "alter table t drop index idx_id", &[]);
    mustExecute(&session, "analyze table t", &[]);
    runPreparedPointSelect(
        &session,
        iterations,
        pointQueryPrepared,
        false,
        &[pointArgs],
    );
    runPreparedPointSelect(&session, iterations, pointQueryPrepared, true, &[pointArgs]);
    runPreparedPointSelect(
        &session,
        iterations,
        batchPointQueryPrepared,
        false,
        batchArgs,
    );
    runPreparedPointSelect(
        &session,
        iterations,
        batchPointQueryPrepared,
        true,
        batchArgs,
    );
}

// runBenchmarkPrepared 对应 Go 的单入口 prepared benchmark。
/// 将标量或切片参数统一转为 `Vec<i32>`，供 prepared benchmark 绑定。
pub trait PreparedQueryArgs {
    fn to_query_args(self) -> Vec<i32>;
}

impl PreparedQueryArgs for i32 {
    fn to_query_args(self) -> Vec<i32> {
        vec![self]
    }
}

impl PreparedQueryArgs for &[i32] {
    fn to_query_args(self) -> Vec<i32> {
        self.to_vec()
    }
}

/// 单入口 prepared benchmark：按 accessType 准备表后跑 runPreparedPointSelect。
pub fn runBenchmarkPrepared(
    iterations: usize,
    partition_by: &str,
    query: &str,
    access: accessType,
    enable_plan_cache: bool,
    q_args: impl PreparedQueryArgs,
) {
    let session = prepareBenchSession();
    match access {
        accessType::pointGet => preparePointGet(&session, partition_by),
        accessType::indexLookup => prepareIndexLookup(&session, partition_by),
        accessType::tableScan => prepareTableScan(&session, partition_by),
    }
    let q_args = q_args.to_query_args();
    runPreparedPointSelect(&session, iterations, query, enable_plan_cache, &q_args);
}

// BenchmarkNonPartitionPointGetPlanCacheOn 对应 Go benchmark：非分区表、普通 SQL PointGet、plan cache 开启。
/// BenchmarkNonPartitionPointGetPlanCacheOn 对应 Go benchmark：非分区表、普通 SQL PointGet、plan cache 开启。
pub fn BenchmarkNonPartitionPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        "",
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkNonPartitionPointGetPlanCacheOff 对应 Go benchmark：非分区表、普通 SQL PointGet、plan cache 关闭。
/// BenchmarkNonPartitionPointGetPlanCacheOff 对应 Go benchmark：非分区表、普通 SQL PointGet、plan cache 关闭。
pub fn BenchmarkNonPartitionPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        "",
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkNonPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：非分区表、普通 SQL BatchPointGet、plan cache 开启。
/// BenchmarkNonPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：非分区表、普通 SQL BatchPointGet、plan cache 开启。
pub fn BenchmarkNonPartitionBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        "",
        batchPointQuery,
        expectedBatchPointPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkNonPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：非分区表、普通 SQL BatchPointGet、plan cache 关闭。
/// BenchmarkNonPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：非分区表、普通 SQL BatchPointGet、plan cache 关闭。
pub fn BenchmarkNonPartitionBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        "",
        batchPointQuery,
        expectedBatchPointPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkNonPartitionIndexLookupPlanCacheOn 对应 Go benchmark：非分区表、普通 SQL IndexLookup、plan cache 开启。
/// BenchmarkNonPartitionIndexLookupPlanCacheOn 对应 Go benchmark：非分区表、普通 SQL IndexLookup、plan cache 开启。
pub fn BenchmarkNonPartitionIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        "",
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkNonPartitionIndexLookupPlanCacheOff 对应 Go benchmark：非分区表、普通 SQL IndexLookup、plan cache 关闭。
/// BenchmarkNonPartitionIndexLookupPlanCacheOff 对应 Go benchmark：非分区表、普通 SQL IndexLookup、plan cache 关闭。
pub fn BenchmarkNonPartitionIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        "",
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkNonPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：非分区表、普通 SQL BatchIndexLookup、plan cache 开启。
/// BenchmarkNonPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：非分区表、普通 SQL BatchIndexLookup、plan cache 开启。
pub fn BenchmarkNonPartitionBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        "",
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkNonPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：非分区表、普通 SQL BatchIndexLookup、plan cache 关闭。
/// BenchmarkNonPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：非分区表、普通 SQL BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkNonPartitionBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        "",
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkNonPartitionTableScanPlanCacheOn 对应 Go benchmark：非分区表、普通 SQL TableScan、plan cache 开启。
/// BenchmarkNonPartitionTableScanPlanCacheOn 对应 Go benchmark：非分区表、普通 SQL TableScan、plan cache 开启。
pub fn BenchmarkNonPartitionTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        "",
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkNonPartitionTableScanPlanCacheOff 对应 Go benchmark：非分区表、普通 SQL TableScan、plan cache 关闭。
/// BenchmarkNonPartitionTableScanPlanCacheOff 对应 Go benchmark：非分区表、普通 SQL TableScan、plan cache 关闭。
pub fn BenchmarkNonPartitionTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        "",
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkNonPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：非分区表、普通 SQL BatchTableScan、plan cache 开启。
/// BenchmarkNonPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：非分区表、普通 SQL BatchTableScan、plan cache 开启。
pub fn BenchmarkNonPartitionBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        "",
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkNonPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：非分区表、普通 SQL BatchTableScan、plan cache 关闭。
/// BenchmarkNonPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：非分区表、普通 SQL BatchTableScan、plan cache 关闭。
pub fn BenchmarkNonPartitionBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        "",
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkNonPartition 对应 Go 的组合普通 SQL benchmark，覆盖非分区表下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkNonPartition 对应 Go 的组合普通 SQL benchmark，覆盖非分区表下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkNonPartition(iterations: usize) {
    benchmarkPointGetPlanCache(iterations, "");
}

// BenchmarkHashPartitionPointGetPlanCacheOn 对应 Go benchmark：hash 分区、普通 SQL PointGet、plan cache 开启。
/// BenchmarkHashPartitionPointGetPlanCacheOn 对应 Go benchmark：hash 分区、普通 SQL PointGet、plan cache 开启。
pub fn BenchmarkHashPartitionPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHash,
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkHashPartitionPointGetPlanCacheOff 对应 Go benchmark：hash 分区、普通 SQL PointGet、plan cache 关闭。
/// BenchmarkHashPartitionPointGetPlanCacheOff 对应 Go benchmark：hash 分区、普通 SQL PointGet、plan cache 关闭。
pub fn BenchmarkHashPartitionPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHash,
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkHashPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：hash 分区、普通 SQL BatchPointGet、plan cache 开启。
/// BenchmarkHashPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：hash 分区、普通 SQL BatchPointGet、plan cache 开启。
pub fn BenchmarkHashPartitionBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHash,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkHashPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：hash 分区、普通 SQL BatchPointGet、plan cache 关闭。
/// BenchmarkHashPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：hash 分区、普通 SQL BatchPointGet、plan cache 关闭。
pub fn BenchmarkHashPartitionBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHash,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkHashPartitionIndexLookupPlanCacheOn 对应 Go benchmark：hash 分区、普通 SQL IndexLookup、plan cache 开启。
/// BenchmarkHashPartitionIndexLookupPlanCacheOn 对应 Go benchmark：hash 分区、普通 SQL IndexLookup、plan cache 开启。
pub fn BenchmarkHashPartitionIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHash,
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkHashPartitionIndexLookupPlanCacheOff 对应 Go benchmark：hash 分区、普通 SQL IndexLookup、plan cache 关闭。
/// BenchmarkHashPartitionIndexLookupPlanCacheOff 对应 Go benchmark：hash 分区、普通 SQL IndexLookup、plan cache 关闭。
pub fn BenchmarkHashPartitionIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHash,
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkHashPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：hash 分区、普通 SQL BatchIndexLookup、plan cache 开启。
/// BenchmarkHashPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：hash 分区、普通 SQL BatchIndexLookup、plan cache 开启。
pub fn BenchmarkHashPartitionBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHash,
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkHashPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：hash 分区、普通 SQL BatchIndexLookup、plan cache 关闭。
/// BenchmarkHashPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：hash 分区、普通 SQL BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkHashPartitionBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHash,
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkHashPartitionTableScanPlanCacheOn 对应 Go benchmark：hash 分区、普通 SQL TableScan、plan cache 开启。
/// BenchmarkHashPartitionTableScanPlanCacheOn 对应 Go benchmark：hash 分区、普通 SQL TableScan、plan cache 开启。
pub fn BenchmarkHashPartitionTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHash,
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkHashPartitionTableScanPlanCacheOff 对应 Go benchmark：hash 分区、普通 SQL TableScan、plan cache 关闭。
/// BenchmarkHashPartitionTableScanPlanCacheOff 对应 Go benchmark：hash 分区、普通 SQL TableScan、plan cache 关闭。
pub fn BenchmarkHashPartitionTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHash,
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkHashPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：hash 分区、普通 SQL BatchTableScan、plan cache 开启。
/// BenchmarkHashPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：hash 分区、普通 SQL BatchTableScan、plan cache 开启。
pub fn BenchmarkHashPartitionBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHash,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkHashPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：hash 分区、普通 SQL BatchTableScan、plan cache 关闭。
/// BenchmarkHashPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：hash 分区、普通 SQL BatchTableScan、plan cache 关闭。
pub fn BenchmarkHashPartitionBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHash,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkHashPartition 对应 Go 的组合普通 SQL benchmark，覆盖hash 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkHashPartition 对应 Go 的组合普通 SQL benchmark，覆盖hash 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkHashPartition(iterations: usize) {
    benchmarkPointGetPlanCache(iterations, partitionByHash);
}

// BenchmarkHashExprPartitionPointGetPlanCacheOn 对应 Go benchmark：hash 表达式分区、普通 SQL PointGet、plan cache 开启。
/// BenchmarkHashExprPartitionPointGetPlanCacheOn 对应 Go benchmark：hash 表达式分区、普通 SQL PointGet、plan cache 开启。
pub fn BenchmarkHashExprPartitionPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHashExpr,
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkHashExprPartitionPointGetPlanCacheOff 对应 Go benchmark：hash 表达式分区、普通 SQL PointGet、plan cache 关闭。
/// BenchmarkHashExprPartitionPointGetPlanCacheOff 对应 Go benchmark：hash 表达式分区、普通 SQL PointGet、plan cache 关闭。
pub fn BenchmarkHashExprPartitionPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHashExpr,
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkHashExprPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：hash 表达式分区、普通 SQL BatchPointGet、plan cache 开启。
/// BenchmarkHashExprPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：hash 表达式分区、普通 SQL BatchPointGet、plan cache 开启。
pub fn BenchmarkHashExprPartitionBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHashExpr,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkHashExprPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：hash 表达式分区、普通 SQL BatchPointGet、plan cache 关闭。
/// BenchmarkHashExprPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：hash 表达式分区、普通 SQL BatchPointGet、plan cache 关闭。
pub fn BenchmarkHashExprPartitionBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHashExpr,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkHashExprPartitionIndexLookupPlanCacheOn 对应 Go benchmark：hash 表达式分区、普通 SQL IndexLookup、plan cache 开启。
/// BenchmarkHashExprPartitionIndexLookupPlanCacheOn 对应 Go benchmark：hash 表达式分区、普通 SQL IndexLookup、plan cache 开启。
pub fn BenchmarkHashExprPartitionIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHashExpr,
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkHashExprPartitionIndexLookupPlanCacheOff 对应 Go benchmark：hash 表达式分区、普通 SQL IndexLookup、plan cache 关闭。
/// BenchmarkHashExprPartitionIndexLookupPlanCacheOff 对应 Go benchmark：hash 表达式分区、普通 SQL IndexLookup、plan cache 关闭。
pub fn BenchmarkHashExprPartitionIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHashExpr,
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkHashExprPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：hash 表达式分区、普通 SQL BatchIndexLookup、plan cache 开启。
/// BenchmarkHashExprPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：hash 表达式分区、普通 SQL BatchIndexLookup、plan cache 开启。
pub fn BenchmarkHashExprPartitionBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHashExpr,
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkHashExprPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：hash 表达式分区、普通 SQL BatchIndexLookup、plan cache 关闭。
/// BenchmarkHashExprPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：hash 表达式分区、普通 SQL BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkHashExprPartitionBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHashExpr,
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkHashExprPartitionTableScanPlanCacheOn 对应 Go benchmark：hash 表达式分区、普通 SQL TableScan、plan cache 开启。
/// BenchmarkHashExprPartitionTableScanPlanCacheOn 对应 Go benchmark：hash 表达式分区、普通 SQL TableScan、plan cache 开启。
pub fn BenchmarkHashExprPartitionTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHashExpr,
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkHashExprPartitionTableScanPlanCacheOff 对应 Go benchmark：hash 表达式分区、普通 SQL TableScan、plan cache 关闭。
/// BenchmarkHashExprPartitionTableScanPlanCacheOff 对应 Go benchmark：hash 表达式分区、普通 SQL TableScan、plan cache 关闭。
pub fn BenchmarkHashExprPartitionTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHashExpr,
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkHashExprPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：hash 表达式分区、普通 SQL BatchTableScan、plan cache 开启。
/// BenchmarkHashExprPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：hash 表达式分区、普通 SQL BatchTableScan、plan cache 开启。
pub fn BenchmarkHashExprPartitionBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHashExpr,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkHashExprPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：hash 表达式分区、普通 SQL BatchTableScan、plan cache 关闭。
/// BenchmarkHashExprPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：hash 表达式分区、普通 SQL BatchTableScan、plan cache 关闭。
pub fn BenchmarkHashExprPartitionBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByHashExpr,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkHashExprPartition 对应 Go 的组合普通 SQL benchmark，覆盖hash 表达式分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkHashExprPartition 对应 Go 的组合普通 SQL benchmark，覆盖hash 表达式分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkHashExprPartition(iterations: usize) {
    benchmarkPointGetPlanCache(iterations, partitionByHashExpr);
}

// BenchmarkKeyPartitionPointGetPlanCacheOn 对应 Go benchmark：key 分区、普通 SQL PointGet、plan cache 开启。
/// BenchmarkKeyPartitionPointGetPlanCacheOn 对应 Go benchmark：key 分区、普通 SQL PointGet、plan cache 开启。
pub fn BenchmarkKeyPartitionPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByKey,
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkKeyPartitionPointGetPlanCacheOff 对应 Go benchmark：key 分区、普通 SQL PointGet、plan cache 关闭。
/// BenchmarkKeyPartitionPointGetPlanCacheOff 对应 Go benchmark：key 分区、普通 SQL PointGet、plan cache 关闭。
pub fn BenchmarkKeyPartitionPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByKey,
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkKeyPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：key 分区、普通 SQL BatchPointGet、plan cache 开启。
/// BenchmarkKeyPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：key 分区、普通 SQL BatchPointGet、plan cache 开启。
pub fn BenchmarkKeyPartitionBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByKey,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkKeyPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：key 分区、普通 SQL BatchPointGet、plan cache 关闭。
/// BenchmarkKeyPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：key 分区、普通 SQL BatchPointGet、plan cache 关闭。
pub fn BenchmarkKeyPartitionBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByKey,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkKeyPartitionIndexLookupPlanCacheOn 对应 Go benchmark：key 分区、普通 SQL IndexLookup、plan cache 开启。
/// BenchmarkKeyPartitionIndexLookupPlanCacheOn 对应 Go benchmark：key 分区、普通 SQL IndexLookup、plan cache 开启。
pub fn BenchmarkKeyPartitionIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByKey,
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkKeyPartitionIndexLookupPlanCacheOff 对应 Go benchmark：key 分区、普通 SQL IndexLookup、plan cache 关闭。
/// BenchmarkKeyPartitionIndexLookupPlanCacheOff 对应 Go benchmark：key 分区、普通 SQL IndexLookup、plan cache 关闭。
pub fn BenchmarkKeyPartitionIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByKey,
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkKeyPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：key 分区、普通 SQL BatchIndexLookup、plan cache 开启。
/// BenchmarkKeyPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：key 分区、普通 SQL BatchIndexLookup、plan cache 开启。
pub fn BenchmarkKeyPartitionBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByKey,
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkKeyPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：key 分区、普通 SQL BatchIndexLookup、plan cache 关闭。
/// BenchmarkKeyPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：key 分区、普通 SQL BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkKeyPartitionBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByKey,
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkKeyPartitionTableScanPlanCacheOn 对应 Go benchmark：key 分区、普通 SQL TableScan、plan cache 开启。
/// BenchmarkKeyPartitionTableScanPlanCacheOn 对应 Go benchmark：key 分区、普通 SQL TableScan、plan cache 开启。
pub fn BenchmarkKeyPartitionTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByKey,
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkKeyPartitionTableScanPlanCacheOff 对应 Go benchmark：key 分区、普通 SQL TableScan、plan cache 关闭。
/// BenchmarkKeyPartitionTableScanPlanCacheOff 对应 Go benchmark：key 分区、普通 SQL TableScan、plan cache 关闭。
pub fn BenchmarkKeyPartitionTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByKey,
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkKeyPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：key 分区、普通 SQL BatchTableScan、plan cache 开启。
/// BenchmarkKeyPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：key 分区、普通 SQL BatchTableScan、plan cache 开启。
pub fn BenchmarkKeyPartitionBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByKey,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkKeyPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：key 分区、普通 SQL BatchTableScan、plan cache 关闭。
/// BenchmarkKeyPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：key 分区、普通 SQL BatchTableScan、plan cache 关闭。
pub fn BenchmarkKeyPartitionBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByKey,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkKeyPartition 对应 Go 的组合普通 SQL benchmark，覆盖key 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkKeyPartition 对应 Go 的组合普通 SQL benchmark，覆盖key 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkKeyPartition(iterations: usize) {
    benchmarkPointGetPlanCache(iterations, partitionByKey);
}

// BenchmarkListPartitionPointGetPlanCacheOn 对应 Go benchmark：list 分区、普通 SQL PointGet、plan cache 开启。
/// BenchmarkListPartitionPointGetPlanCacheOn 对应 Go benchmark：list 分区、普通 SQL PointGet、plan cache 开启。
pub fn BenchmarkListPartitionPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", false),
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkListPartitionPointGetPlanCacheOff 对应 Go benchmark：list 分区、普通 SQL PointGet、plan cache 关闭。
/// BenchmarkListPartitionPointGetPlanCacheOff 对应 Go benchmark：list 分区、普通 SQL PointGet、plan cache 关闭。
pub fn BenchmarkListPartitionPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", false),
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkListPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：list 分区、普通 SQL BatchPointGet、plan cache 开启。
/// BenchmarkListPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：list 分区、普通 SQL BatchPointGet、plan cache 开启。
pub fn BenchmarkListPartitionBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", false),
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkListPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：list 分区、普通 SQL BatchPointGet、plan cache 关闭。
/// BenchmarkListPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：list 分区、普通 SQL BatchPointGet、plan cache 关闭。
pub fn BenchmarkListPartitionBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", false),
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkListPartitionIndexLookupPlanCacheOn 对应 Go benchmark：list 分区、普通 SQL IndexLookup、plan cache 开启。
/// BenchmarkListPartitionIndexLookupPlanCacheOn 对应 Go benchmark：list 分区、普通 SQL IndexLookup、plan cache 开启。
pub fn BenchmarkListPartitionIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", false),
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkListPartitionIndexLookupPlanCacheOff 对应 Go benchmark：list 分区、普通 SQL IndexLookup、plan cache 关闭。
/// BenchmarkListPartitionIndexLookupPlanCacheOff 对应 Go benchmark：list 分区、普通 SQL IndexLookup、plan cache 关闭。
pub fn BenchmarkListPartitionIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", false),
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkListPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：list 分区、普通 SQL BatchIndexLookup、plan cache 开启。
/// BenchmarkListPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：list 分区、普通 SQL BatchIndexLookup、plan cache 开启。
pub fn BenchmarkListPartitionBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", false),
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkListPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：list 分区、普通 SQL BatchIndexLookup、plan cache 关闭。
/// BenchmarkListPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：list 分区、普通 SQL BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkListPartitionBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", false),
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkListPartitionTableScanPlanCacheOn 对应 Go benchmark：list 分区、普通 SQL TableScan、plan cache 开启。
/// BenchmarkListPartitionTableScanPlanCacheOn 对应 Go benchmark：list 分区、普通 SQL TableScan、plan cache 开启。
pub fn BenchmarkListPartitionTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", false),
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkListPartitionTableScanPlanCacheOff 对应 Go benchmark：list 分区、普通 SQL TableScan、plan cache 关闭。
/// BenchmarkListPartitionTableScanPlanCacheOff 对应 Go benchmark：list 分区、普通 SQL TableScan、plan cache 关闭。
pub fn BenchmarkListPartitionTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", false),
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkListPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：list 分区、普通 SQL BatchTableScan、plan cache 开启。
/// BenchmarkListPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：list 分区、普通 SQL BatchTableScan、plan cache 开启。
pub fn BenchmarkListPartitionBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", false),
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkListPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：list 分区、普通 SQL BatchTableScan、plan cache 关闭。
/// BenchmarkListPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：list 分区、普通 SQL BatchTableScan、plan cache 关闭。
pub fn BenchmarkListPartitionBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", false),
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkListPartition 对应 Go 的组合普通 SQL benchmark，覆盖list 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkListPartition 对应 Go 的组合普通 SQL benchmark，覆盖list 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkListPartition(iterations: usize) {
    benchmarkPointGetPlanCache(iterations, &getListPartitionDef("id", false));
}

// BenchmarkListExprPartitionPointGetPlanCacheOn 对应 Go benchmark：list 表达式分区、普通 SQL PointGet、plan cache 开启。
/// BenchmarkListExprPartitionPointGetPlanCacheOn 对应 Go benchmark：list 表达式分区、普通 SQL PointGet、plan cache 开启。
pub fn BenchmarkListExprPartitionPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkListExprPartitionPointGetPlanCacheOff 对应 Go benchmark：list 表达式分区、普通 SQL PointGet、plan cache 关闭。
/// BenchmarkListExprPartitionPointGetPlanCacheOff 对应 Go benchmark：list 表达式分区、普通 SQL PointGet、plan cache 关闭。
pub fn BenchmarkListExprPartitionPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkListExprPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：list 表达式分区、普通 SQL BatchPointGet、plan cache 开启。
/// BenchmarkListExprPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：list 表达式分区、普通 SQL BatchPointGet、plan cache 开启。
pub fn BenchmarkListExprPartitionBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkListExprPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：list 表达式分区、普通 SQL BatchPointGet、plan cache 关闭。
/// BenchmarkListExprPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：list 表达式分区、普通 SQL BatchPointGet、plan cache 关闭。
pub fn BenchmarkListExprPartitionBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkListExprPartitionIndexLookupPlanCacheOn 对应 Go benchmark：list 表达式分区、普通 SQL IndexLookup、plan cache 开启。
/// BenchmarkListExprPartitionIndexLookupPlanCacheOn 对应 Go benchmark：list 表达式分区、普通 SQL IndexLookup、plan cache 开启。
pub fn BenchmarkListExprPartitionIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkListExprPartitionIndexLookupPlanCacheOff 对应 Go benchmark：list 表达式分区、普通 SQL IndexLookup、plan cache 关闭。
/// BenchmarkListExprPartitionIndexLookupPlanCacheOff 对应 Go benchmark：list 表达式分区、普通 SQL IndexLookup、plan cache 关闭。
pub fn BenchmarkListExprPartitionIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkListExprPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：list 表达式分区、普通 SQL BatchIndexLookup、plan cache 开启。
/// BenchmarkListExprPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：list 表达式分区、普通 SQL BatchIndexLookup、plan cache 开启。
pub fn BenchmarkListExprPartitionBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkListExprPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：list 表达式分区、普通 SQL BatchIndexLookup、plan cache 关闭。
/// BenchmarkListExprPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：list 表达式分区、普通 SQL BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkListExprPartitionBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkListExprPartitionTableScanPlanCacheOn 对应 Go benchmark：list 表达式分区、普通 SQL TableScan、plan cache 开启。
/// BenchmarkListExprPartitionTableScanPlanCacheOn 对应 Go benchmark：list 表达式分区、普通 SQL TableScan、plan cache 开启。
pub fn BenchmarkListExprPartitionTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkListExprPartitionTableScanPlanCacheOff 对应 Go benchmark：list 表达式分区、普通 SQL TableScan、plan cache 关闭。
/// BenchmarkListExprPartitionTableScanPlanCacheOff 对应 Go benchmark：list 表达式分区、普通 SQL TableScan、plan cache 关闭。
pub fn BenchmarkListExprPartitionTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkListExprPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：list 表达式分区、普通 SQL BatchTableScan、plan cache 开启。
/// BenchmarkListExprPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：list 表达式分区、普通 SQL BatchTableScan、plan cache 开启。
pub fn BenchmarkListExprPartitionBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkListExprPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：list 表达式分区、普通 SQL BatchTableScan、plan cache 关闭。
/// BenchmarkListExprPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：list 表达式分区、普通 SQL BatchTableScan、plan cache 关闭。
pub fn BenchmarkListExprPartitionBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkListExprPartition 对应 Go 的组合普通 SQL benchmark，覆盖list 表达式分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkListExprPartition 对应 Go 的组合普通 SQL benchmark，覆盖list 表达式分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkListExprPartition(iterations: usize) {
    benchmarkPointGetPlanCache(iterations, &getListPartitionDef("floor(id*0.5)*2", false));
}

// BenchmarkListColumnsPartitionPointGetPlanCacheOn 对应 Go benchmark：list columns 分区、普通 SQL PointGet、plan cache 开启。
/// BenchmarkListColumnsPartitionPointGetPlanCacheOn 对应 Go benchmark：list columns 分区、普通 SQL PointGet、plan cache 开启。
pub fn BenchmarkListColumnsPartitionPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", true),
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkListColumnsPartitionPointGetPlanCacheOff 对应 Go benchmark：list columns 分区、普通 SQL PointGet、plan cache 关闭。
/// BenchmarkListColumnsPartitionPointGetPlanCacheOff 对应 Go benchmark：list columns 分区、普通 SQL PointGet、plan cache 关闭。
pub fn BenchmarkListColumnsPartitionPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", true),
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkListColumnsPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：list columns 分区、普通 SQL BatchPointGet、plan cache 开启。
/// BenchmarkListColumnsPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：list columns 分区、普通 SQL BatchPointGet、plan cache 开启。
pub fn BenchmarkListColumnsPartitionBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", true),
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkListColumnsPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：list columns 分区、普通 SQL BatchPointGet、plan cache 关闭。
/// BenchmarkListColumnsPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：list columns 分区、普通 SQL BatchPointGet、plan cache 关闭。
pub fn BenchmarkListColumnsPartitionBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", true),
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkListColumnsPartitionIndexLookupPlanCacheOn 对应 Go benchmark：list columns 分区、普通 SQL IndexLookup、plan cache 开启。
/// BenchmarkListColumnsPartitionIndexLookupPlanCacheOn 对应 Go benchmark：list columns 分区、普通 SQL IndexLookup、plan cache 开启。
pub fn BenchmarkListColumnsPartitionIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", true),
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkListColumnsPartitionIndexLookupPlanCacheOff 对应 Go benchmark：list columns 分区、普通 SQL IndexLookup、plan cache 关闭。
/// BenchmarkListColumnsPartitionIndexLookupPlanCacheOff 对应 Go benchmark：list columns 分区、普通 SQL IndexLookup、plan cache 关闭。
pub fn BenchmarkListColumnsPartitionIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", true),
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkListColumnsPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：list columns 分区、普通 SQL BatchIndexLookup、plan cache 开启。
/// BenchmarkListColumnsPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：list columns 分区、普通 SQL BatchIndexLookup、plan cache 开启。
pub fn BenchmarkListColumnsPartitionBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", true),
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkListColumnsPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：list columns 分区、普通 SQL BatchIndexLookup、plan cache 关闭。
/// BenchmarkListColumnsPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：list columns 分区、普通 SQL BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkListColumnsPartitionBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", true),
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkListColumnsPartitionTableScanPlanCacheOn 对应 Go benchmark：list columns 分区、普通 SQL TableScan、plan cache 开启。
/// BenchmarkListColumnsPartitionTableScanPlanCacheOn 对应 Go benchmark：list columns 分区、普通 SQL TableScan、plan cache 开启。
pub fn BenchmarkListColumnsPartitionTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", true),
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkListColumnsPartitionTableScanPlanCacheOff 对应 Go benchmark：list columns 分区、普通 SQL TableScan、plan cache 关闭。
/// BenchmarkListColumnsPartitionTableScanPlanCacheOff 对应 Go benchmark：list columns 分区、普通 SQL TableScan、plan cache 关闭。
pub fn BenchmarkListColumnsPartitionTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", true),
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkListColumnsPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：list columns 分区、普通 SQL BatchTableScan、plan cache 开启。
/// BenchmarkListColumnsPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：list columns 分区、普通 SQL BatchTableScan、plan cache 开启。
pub fn BenchmarkListColumnsPartitionBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", true),
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkListColumnsPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：list columns 分区、普通 SQL BatchTableScan、plan cache 关闭。
/// BenchmarkListColumnsPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：list columns 分区、普通 SQL BatchTableScan、plan cache 关闭。
pub fn BenchmarkListColumnsPartitionBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        &getListPartitionDef("id", true),
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkListColumnsPartition 对应 Go 的组合普通 SQL benchmark，覆盖list columns 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkListColumnsPartition 对应 Go 的组合普通 SQL benchmark，覆盖list columns 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkListColumnsPartition(iterations: usize) {
    benchmarkPointGetPlanCache(iterations, &getListPartitionDef("id", true));
}

// BenchmarkRangePartitionPointGetPlanCacheOn 对应 Go benchmark：range 分区、普通 SQL PointGet、plan cache 开启。
/// BenchmarkRangePartitionPointGetPlanCacheOn 对应 Go benchmark：range 分区、普通 SQL PointGet、plan cache 开启。
pub fn BenchmarkRangePartitionPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRange,
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkRangePartitionPointGetPlanCacheOff 对应 Go benchmark：range 分区、普通 SQL PointGet、plan cache 关闭。
/// BenchmarkRangePartitionPointGetPlanCacheOff 对应 Go benchmark：range 分区、普通 SQL PointGet、plan cache 关闭。
pub fn BenchmarkRangePartitionPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRange,
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkRangePartitionBatchPointGetPlanCacheOn 对应 Go benchmark：range 分区、普通 SQL BatchPointGet、plan cache 开启。
/// BenchmarkRangePartitionBatchPointGetPlanCacheOn 对应 Go benchmark：range 分区、普通 SQL BatchPointGet、plan cache 开启。
pub fn BenchmarkRangePartitionBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRange,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkRangePartitionBatchPointGetPlanCacheOff 对应 Go benchmark：range 分区、普通 SQL BatchPointGet、plan cache 关闭。
/// BenchmarkRangePartitionBatchPointGetPlanCacheOff 对应 Go benchmark：range 分区、普通 SQL BatchPointGet、plan cache 关闭。
pub fn BenchmarkRangePartitionBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRange,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkRangePartitionIndexLookupPlanCacheOn 对应 Go benchmark：range 分区、普通 SQL IndexLookup、plan cache 开启。
/// BenchmarkRangePartitionIndexLookupPlanCacheOn 对应 Go benchmark：range 分区、普通 SQL IndexLookup、plan cache 开启。
pub fn BenchmarkRangePartitionIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRange,
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkRangePartitionIndexLookupPlanCacheOff 对应 Go benchmark：range 分区、普通 SQL IndexLookup、plan cache 关闭。
/// BenchmarkRangePartitionIndexLookupPlanCacheOff 对应 Go benchmark：range 分区、普通 SQL IndexLookup、plan cache 关闭。
pub fn BenchmarkRangePartitionIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRange,
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkRangePartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：range 分区、普通 SQL BatchIndexLookup、plan cache 开启。
/// BenchmarkRangePartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：range 分区、普通 SQL BatchIndexLookup、plan cache 开启。
pub fn BenchmarkRangePartitionBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRange,
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkRangePartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：range 分区、普通 SQL BatchIndexLookup、plan cache 关闭。
/// BenchmarkRangePartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：range 分区、普通 SQL BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkRangePartitionBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRange,
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkRangePartitionTableScanPlanCacheOn 对应 Go benchmark：range 分区、普通 SQL TableScan、plan cache 开启。
/// BenchmarkRangePartitionTableScanPlanCacheOn 对应 Go benchmark：range 分区、普通 SQL TableScan、plan cache 开启。
pub fn BenchmarkRangePartitionTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRange,
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkRangePartitionTableScanPlanCacheOff 对应 Go benchmark：range 分区、普通 SQL TableScan、plan cache 关闭。
/// BenchmarkRangePartitionTableScanPlanCacheOff 对应 Go benchmark：range 分区、普通 SQL TableScan、plan cache 关闭。
pub fn BenchmarkRangePartitionTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRange,
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkRangePartitionBatchTableScanPlanCacheOn 对应 Go benchmark：range 分区、普通 SQL BatchTableScan、plan cache 开启。
/// BenchmarkRangePartitionBatchTableScanPlanCacheOn 对应 Go benchmark：range 分区、普通 SQL BatchTableScan、plan cache 开启。
pub fn BenchmarkRangePartitionBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRange,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkRangePartitionBatchTableScanPlanCacheOff 对应 Go benchmark：range 分区、普通 SQL BatchTableScan、plan cache 关闭。
/// BenchmarkRangePartitionBatchTableScanPlanCacheOff 对应 Go benchmark：range 分区、普通 SQL BatchTableScan、plan cache 关闭。
pub fn BenchmarkRangePartitionBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRange,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkRangePartition 对应 Go 的组合普通 SQL benchmark，覆盖range 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkRangePartition 对应 Go 的组合普通 SQL benchmark，覆盖range 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkRangePartition(iterations: usize) {
    benchmarkPointGetPlanCache(iterations, partitionByRange);
}

// BenchmarkRangeExprPartitionPointGetPlanCacheOn 对应 Go benchmark：range 表达式分区、普通 SQL PointGet、plan cache 开启。
/// BenchmarkRangeExprPartitionPointGetPlanCacheOn 对应 Go benchmark：range 表达式分区、普通 SQL PointGet、plan cache 开启。
pub fn BenchmarkRangeExprPartitionPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeExpr,
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkRangeExprPartitionPointGetPlanCacheOff 对应 Go benchmark：range 表达式分区、普通 SQL PointGet、plan cache 关闭。
/// BenchmarkRangeExprPartitionPointGetPlanCacheOff 对应 Go benchmark：range 表达式分区、普通 SQL PointGet、plan cache 关闭。
pub fn BenchmarkRangeExprPartitionPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeExpr,
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkRangeExprPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：range 表达式分区、普通 SQL BatchPointGet、plan cache 开启。
/// BenchmarkRangeExprPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：range 表达式分区、普通 SQL BatchPointGet、plan cache 开启。
pub fn BenchmarkRangeExprPartitionBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeExpr,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkRangeExprPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：range 表达式分区、普通 SQL BatchPointGet、plan cache 关闭。
/// BenchmarkRangeExprPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：range 表达式分区、普通 SQL BatchPointGet、plan cache 关闭。
pub fn BenchmarkRangeExprPartitionBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeExpr,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkRangeExprPartitionIndexLookupPlanCacheOn 对应 Go benchmark：range 表达式分区、普通 SQL IndexLookup、plan cache 开启。
/// BenchmarkRangeExprPartitionIndexLookupPlanCacheOn 对应 Go benchmark：range 表达式分区、普通 SQL IndexLookup、plan cache 开启。
pub fn BenchmarkRangeExprPartitionIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeExpr,
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkRangeExprPartitionIndexLookupPlanCacheOff 对应 Go benchmark：range 表达式分区、普通 SQL IndexLookup、plan cache 关闭。
/// BenchmarkRangeExprPartitionIndexLookupPlanCacheOff 对应 Go benchmark：range 表达式分区、普通 SQL IndexLookup、plan cache 关闭。
pub fn BenchmarkRangeExprPartitionIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeExpr,
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkRangeExprPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：range 表达式分区、普通 SQL BatchIndexLookup、plan cache 开启。
/// BenchmarkRangeExprPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：range 表达式分区、普通 SQL BatchIndexLookup、plan cache 开启。
pub fn BenchmarkRangeExprPartitionBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeExpr,
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkRangeExprPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：range 表达式分区、普通 SQL BatchIndexLookup、plan cache 关闭。
/// BenchmarkRangeExprPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：range 表达式分区、普通 SQL BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkRangeExprPartitionBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeExpr,
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkRangeExprPartitionTableScanPlanCacheOn 对应 Go benchmark：range 表达式分区、普通 SQL TableScan、plan cache 开启。
/// BenchmarkRangeExprPartitionTableScanPlanCacheOn 对应 Go benchmark：range 表达式分区、普通 SQL TableScan、plan cache 开启。
pub fn BenchmarkRangeExprPartitionTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeExpr,
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkRangeExprPartitionTableScanPlanCacheOff 对应 Go benchmark：range 表达式分区、普通 SQL TableScan、plan cache 关闭。
/// BenchmarkRangeExprPartitionTableScanPlanCacheOff 对应 Go benchmark：range 表达式分区、普通 SQL TableScan、plan cache 关闭。
pub fn BenchmarkRangeExprPartitionTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeExpr,
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkRangeExprPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：range 表达式分区、普通 SQL BatchTableScan、plan cache 开启。
/// BenchmarkRangeExprPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：range 表达式分区、普通 SQL BatchTableScan、plan cache 开启。
pub fn BenchmarkRangeExprPartitionBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeExpr,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkRangeExprPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：range 表达式分区、普通 SQL BatchTableScan、plan cache 关闭。
/// BenchmarkRangeExprPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：range 表达式分区、普通 SQL BatchTableScan、plan cache 关闭。
pub fn BenchmarkRangeExprPartitionBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeExpr,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkRangeExprPartition 对应 Go 的组合普通 SQL benchmark，覆盖range 表达式分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkRangeExprPartition 对应 Go 的组合普通 SQL benchmark，覆盖range 表达式分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkRangeExprPartition(iterations: usize) {
    benchmarkPointGetPlanCache(iterations, partitionByRangeExpr);
}

// BenchmarkRangeColumnsPartitionPointGetPlanCacheOn 对应 Go benchmark：range columns 分区、普通 SQL PointGet、plan cache 开启。
/// BenchmarkRangeColumnsPartitionPointGetPlanCacheOn 对应 Go benchmark：range columns 分区、普通 SQL PointGet、plan cache 开启。
pub fn BenchmarkRangeColumnsPartitionPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeColumns,
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkRangeColumnsPartitionPointGetPlanCacheOff 对应 Go benchmark：range columns 分区、普通 SQL PointGet、plan cache 关闭。
/// BenchmarkRangeColumnsPartitionPointGetPlanCacheOff 对应 Go benchmark：range columns 分区、普通 SQL PointGet、plan cache 关闭。
pub fn BenchmarkRangeColumnsPartitionPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeColumns,
        pointQuery,
        expectedPointPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkRangeColumnsPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：range columns 分区、普通 SQL BatchPointGet、plan cache 开启。
/// BenchmarkRangeColumnsPartitionBatchPointGetPlanCacheOn 对应 Go benchmark：range columns 分区、普通 SQL BatchPointGet、plan cache 开启。
pub fn BenchmarkRangeColumnsPartitionBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeColumns,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        true,
    );
}

// BenchmarkRangeColumnsPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：range columns 分区、普通 SQL BatchPointGet、plan cache 关闭。
/// BenchmarkRangeColumnsPartitionBatchPointGetPlanCacheOff 对应 Go benchmark：range columns 分区、普通 SQL BatchPointGet、plan cache 关闭。
pub fn BenchmarkRangeColumnsPartitionBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeColumns,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::pointGet,
        false,
    );
}

// BenchmarkRangeColumnsPartitionIndexLookupPlanCacheOn 对应 Go benchmark：range columns 分区、普通 SQL IndexLookup、plan cache 开启。
/// BenchmarkRangeColumnsPartitionIndexLookupPlanCacheOn 对应 Go benchmark：range columns 分区、普通 SQL IndexLookup、plan cache 开启。
pub fn BenchmarkRangeColumnsPartitionIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeColumns,
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkRangeColumnsPartitionIndexLookupPlanCacheOff 对应 Go benchmark：range columns 分区、普通 SQL IndexLookup、plan cache 关闭。
/// BenchmarkRangeColumnsPartitionIndexLookupPlanCacheOff 对应 Go benchmark：range columns 分区、普通 SQL IndexLookup、plan cache 关闭。
pub fn BenchmarkRangeColumnsPartitionIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeColumns,
        pointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkRangeColumnsPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：range columns 分区、普通 SQL BatchIndexLookup、plan cache 开启。
/// BenchmarkRangeColumnsPartitionBatchIndexLookupPlanCacheOn 对应 Go benchmark：range columns 分区、普通 SQL BatchIndexLookup、plan cache 开启。
pub fn BenchmarkRangeColumnsPartitionBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeColumns,
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        true,
    );
}

// BenchmarkRangeColumnsPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：range columns 分区、普通 SQL BatchIndexLookup、plan cache 关闭。
/// BenchmarkRangeColumnsPartitionBatchIndexLookupPlanCacheOff 对应 Go benchmark：range columns 分区、普通 SQL BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkRangeColumnsPartitionBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeColumns,
        batchPointQuery,
        expectedIndexPlan,
        accessType::indexLookup,
        false,
    );
}

// BenchmarkRangeColumnsPartitionTableScanPlanCacheOn 对应 Go benchmark：range columns 分区、普通 SQL TableScan、plan cache 开启。
/// BenchmarkRangeColumnsPartitionTableScanPlanCacheOn 对应 Go benchmark：range columns 分区、普通 SQL TableScan、plan cache 开启。
pub fn BenchmarkRangeColumnsPartitionTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeColumns,
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkRangeColumnsPartitionTableScanPlanCacheOff 对应 Go benchmark：range columns 分区、普通 SQL TableScan、plan cache 关闭。
/// BenchmarkRangeColumnsPartitionTableScanPlanCacheOff 对应 Go benchmark：range columns 分区、普通 SQL TableScan、plan cache 关闭。
pub fn BenchmarkRangeColumnsPartitionTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeColumns,
        pointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkRangeColumnsPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：range columns 分区、普通 SQL BatchTableScan、plan cache 开启。
/// BenchmarkRangeColumnsPartitionBatchTableScanPlanCacheOn 对应 Go benchmark：range columns 分区、普通 SQL BatchTableScan、plan cache 开启。
pub fn BenchmarkRangeColumnsPartitionBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeColumns,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        true,
    );
}

// BenchmarkRangeColumnsPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：range columns 分区、普通 SQL BatchTableScan、plan cache 关闭。
/// BenchmarkRangeColumnsPartitionBatchTableScanPlanCacheOff 对应 Go benchmark：range columns 分区、普通 SQL BatchTableScan、plan cache 关闭。
pub fn BenchmarkRangeColumnsPartitionBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmark(
        iterations,
        partitionByRangeColumns,
        batchPointQuery,
        expectedTableScanPlan,
        accessType::tableScan,
        false,
    );
}

// BenchmarkRangeColumnsPartition 对应 Go 的组合普通 SQL benchmark，覆盖range columns 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkRangeColumnsPartition 对应 Go 的组合普通 SQL benchmark，覆盖range columns 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkRangeColumnsPartition(iterations: usize) {
    benchmarkPointGetPlanCache(iterations, partitionByRangeColumns);
}

// BenchmarkNonPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：非分区表、prepared PointGet、plan cache 开启。
/// BenchmarkNonPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：非分区表、prepared PointGet、plan cache 开启。
pub fn BenchmarkNonPartitionPreparedPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        "",
        pointQueryPrepared,
        accessType::pointGet,
        true,
        pointArgs,
    );
}

// BenchmarkNonPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：非分区表、prepared PointGet、plan cache 关闭。
/// BenchmarkNonPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：非分区表、prepared PointGet、plan cache 关闭。
pub fn BenchmarkNonPartitionPreparedPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        "",
        pointQueryPrepared,
        accessType::pointGet,
        false,
        pointArgs,
    );
}

// BenchmarkNonPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：非分区表、prepared BatchPointGet、plan cache 开启。
/// BenchmarkNonPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：非分区表、prepared BatchPointGet、plan cache 开启。
pub fn BenchmarkNonPartitionPreparedBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        "",
        batchPointQueryPrepared,
        accessType::pointGet,
        true,
        batchArgs,
    );
}

// BenchmarkNonPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：非分区表、prepared BatchPointGet、plan cache 关闭。
/// BenchmarkNonPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：非分区表、prepared BatchPointGet、plan cache 关闭。
pub fn BenchmarkNonPartitionPreparedBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        "",
        batchPointQueryPrepared,
        accessType::pointGet,
        false,
        batchArgs,
    );
}

// BenchmarkNonPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：非分区表、prepared IndexLookup、plan cache 开启。
/// BenchmarkNonPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：非分区表、prepared IndexLookup、plan cache 开启。
pub fn BenchmarkNonPartitionPreparedIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        "",
        pointQueryPrepared,
        accessType::indexLookup,
        true,
        pointArgs,
    );
}

// BenchmarkNonPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：非分区表、prepared IndexLookup、plan cache 关闭。
/// BenchmarkNonPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：非分区表、prepared IndexLookup、plan cache 关闭。
pub fn BenchmarkNonPartitionPreparedIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        "",
        pointQueryPrepared,
        accessType::indexLookup,
        false,
        pointArgs,
    );
}

// BenchmarkNonPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：非分区表、prepared BatchIndexLookup、plan cache 开启。
/// BenchmarkNonPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：非分区表、prepared BatchIndexLookup、plan cache 开启。
pub fn BenchmarkNonPartitionPreparedBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        "",
        batchPointQueryPrepared,
        accessType::indexLookup,
        true,
        batchArgs,
    );
}

// BenchmarkNonPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：非分区表、prepared BatchIndexLookup、plan cache 关闭。
/// BenchmarkNonPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：非分区表、prepared BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkNonPartitionPreparedBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        "",
        batchPointQueryPrepared,
        accessType::indexLookup,
        false,
        batchArgs,
    );
}

// BenchmarkNonPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：非分区表、prepared TableScan、plan cache 开启。
/// BenchmarkNonPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：非分区表、prepared TableScan、plan cache 开启。
pub fn BenchmarkNonPartitionPreparedTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        "",
        pointQueryPrepared,
        accessType::tableScan,
        true,
        pointArgs,
    );
}

// BenchmarkNonPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：非分区表、prepared TableScan、plan cache 关闭。
/// BenchmarkNonPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：非分区表、prepared TableScan、plan cache 关闭。
pub fn BenchmarkNonPartitionPreparedTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        "",
        pointQueryPrepared,
        accessType::tableScan,
        false,
        pointArgs,
    );
}

// BenchmarkNonPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：非分区表、prepared BatchTableScan、plan cache 开启。
/// BenchmarkNonPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：非分区表、prepared BatchTableScan、plan cache 开启。
pub fn BenchmarkNonPartitionPreparedBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        "",
        batchPointQueryPrepared,
        accessType::tableScan,
        true,
        batchArgs,
    );
}

// BenchmarkNonPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：非分区表、prepared BatchTableScan、plan cache 关闭。
/// BenchmarkNonPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：非分区表、prepared BatchTableScan、plan cache 关闭。
pub fn BenchmarkNonPartitionPreparedBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        "",
        batchPointQueryPrepared,
        accessType::tableScan,
        false,
        batchArgs,
    );
}

// BenchmarkNonPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖非分区表下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkNonPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖非分区表下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkNonPartitionPrepared(iterations: usize) {
    benchPreparedPointGet(iterations, "");
}

// BenchmarkHashPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：hash 分区、prepared PointGet、plan cache 开启。
/// BenchmarkHashPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：hash 分区、prepared PointGet、plan cache 开启。
pub fn BenchmarkHashPartitionPreparedPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHash,
        pointQueryPrepared,
        accessType::pointGet,
        true,
        pointArgs,
    );
}

// BenchmarkHashPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：hash 分区、prepared PointGet、plan cache 关闭。
/// BenchmarkHashPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：hash 分区、prepared PointGet、plan cache 关闭。
pub fn BenchmarkHashPartitionPreparedPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHash,
        pointQueryPrepared,
        accessType::pointGet,
        false,
        pointArgs,
    );
}

// BenchmarkHashPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：hash 分区、prepared BatchPointGet、plan cache 开启。
/// BenchmarkHashPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：hash 分区、prepared BatchPointGet、plan cache 开启。
pub fn BenchmarkHashPartitionPreparedBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHash,
        batchPointQueryPrepared,
        accessType::pointGet,
        true,
        batchArgs,
    );
}

// BenchmarkHashPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：hash 分区、prepared BatchPointGet、plan cache 关闭。
/// BenchmarkHashPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：hash 分区、prepared BatchPointGet、plan cache 关闭。
pub fn BenchmarkHashPartitionPreparedBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHash,
        batchPointQueryPrepared,
        accessType::pointGet,
        false,
        batchArgs,
    );
}

// BenchmarkHashPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：hash 分区、prepared IndexLookup、plan cache 开启。
/// BenchmarkHashPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：hash 分区、prepared IndexLookup、plan cache 开启。
pub fn BenchmarkHashPartitionPreparedIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHash,
        pointQueryPrepared,
        accessType::indexLookup,
        true,
        pointArgs,
    );
}

// BenchmarkHashPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：hash 分区、prepared IndexLookup、plan cache 关闭。
/// BenchmarkHashPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：hash 分区、prepared IndexLookup、plan cache 关闭。
pub fn BenchmarkHashPartitionPreparedIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHash,
        pointQueryPrepared,
        accessType::indexLookup,
        false,
        pointArgs,
    );
}

// BenchmarkHashPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：hash 分区、prepared BatchIndexLookup、plan cache 开启。
/// BenchmarkHashPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：hash 分区、prepared BatchIndexLookup、plan cache 开启。
pub fn BenchmarkHashPartitionPreparedBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHash,
        batchPointQueryPrepared,
        accessType::indexLookup,
        true,
        batchArgs,
    );
}

// BenchmarkHashPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：hash 分区、prepared BatchIndexLookup、plan cache 关闭。
/// BenchmarkHashPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：hash 分区、prepared BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkHashPartitionPreparedBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHash,
        batchPointQueryPrepared,
        accessType::indexLookup,
        false,
        batchArgs,
    );
}

// BenchmarkHashPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：hash 分区、prepared TableScan、plan cache 开启。
/// BenchmarkHashPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：hash 分区、prepared TableScan、plan cache 开启。
pub fn BenchmarkHashPartitionPreparedTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHash,
        pointQueryPrepared,
        accessType::tableScan,
        true,
        pointArgs,
    );
}

// BenchmarkHashPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：hash 分区、prepared TableScan、plan cache 关闭。
/// BenchmarkHashPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：hash 分区、prepared TableScan、plan cache 关闭。
pub fn BenchmarkHashPartitionPreparedTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHash,
        pointQueryPrepared,
        accessType::tableScan,
        false,
        pointArgs,
    );
}

// BenchmarkHashPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：hash 分区、prepared BatchTableScan、plan cache 开启。
/// BenchmarkHashPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：hash 分区、prepared BatchTableScan、plan cache 开启。
pub fn BenchmarkHashPartitionPreparedBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHash,
        batchPointQueryPrepared,
        accessType::tableScan,
        true,
        batchArgs,
    );
}

// BenchmarkHashPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：hash 分区、prepared BatchTableScan、plan cache 关闭。
/// BenchmarkHashPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：hash 分区、prepared BatchTableScan、plan cache 关闭。
pub fn BenchmarkHashPartitionPreparedBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHash,
        batchPointQueryPrepared,
        accessType::tableScan,
        false,
        batchArgs,
    );
}

// BenchmarkHashPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖hash 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkHashPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖hash 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkHashPartitionPrepared(iterations: usize) {
    benchPreparedPointGet(iterations, partitionByHash);
}

// BenchmarkHashExprPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：hash 表达式分区、prepared PointGet、plan cache 开启。
/// BenchmarkHashExprPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：hash 表达式分区、prepared PointGet、plan cache 开启。
pub fn BenchmarkHashExprPartitionPreparedPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHashExpr,
        pointQueryPrepared,
        accessType::pointGet,
        true,
        pointArgs,
    );
}

// BenchmarkHashExprPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：hash 表达式分区、prepared PointGet、plan cache 关闭。
/// BenchmarkHashExprPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：hash 表达式分区、prepared PointGet、plan cache 关闭。
pub fn BenchmarkHashExprPartitionPreparedPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHashExpr,
        pointQueryPrepared,
        accessType::pointGet,
        false,
        pointArgs,
    );
}

// BenchmarkHashExprPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：hash 表达式分区、prepared BatchPointGet、plan cache 开启。
/// BenchmarkHashExprPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：hash 表达式分区、prepared BatchPointGet、plan cache 开启。
pub fn BenchmarkHashExprPartitionPreparedBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHashExpr,
        batchPointQueryPrepared,
        accessType::pointGet,
        true,
        batchArgs,
    );
}

// BenchmarkHashExprPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：hash 表达式分区、prepared BatchPointGet、plan cache 关闭。
/// BenchmarkHashExprPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：hash 表达式分区、prepared BatchPointGet、plan cache 关闭。
pub fn BenchmarkHashExprPartitionPreparedBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHashExpr,
        batchPointQueryPrepared,
        accessType::pointGet,
        false,
        batchArgs,
    );
}

// BenchmarkHashExprPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：hash 表达式分区、prepared IndexLookup、plan cache 开启。
/// BenchmarkHashExprPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：hash 表达式分区、prepared IndexLookup、plan cache 开启。
pub fn BenchmarkHashExprPartitionPreparedIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHashExpr,
        pointQueryPrepared,
        accessType::indexLookup,
        true,
        pointArgs,
    );
}

// BenchmarkHashExprPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：hash 表达式分区、prepared IndexLookup、plan cache 关闭。
/// BenchmarkHashExprPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：hash 表达式分区、prepared IndexLookup、plan cache 关闭。
pub fn BenchmarkHashExprPartitionPreparedIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHashExpr,
        pointQueryPrepared,
        accessType::indexLookup,
        false,
        pointArgs,
    );
}

// BenchmarkHashExprPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：hash 表达式分区、prepared BatchIndexLookup、plan cache 开启。
/// BenchmarkHashExprPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：hash 表达式分区、prepared BatchIndexLookup、plan cache 开启。
pub fn BenchmarkHashExprPartitionPreparedBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHashExpr,
        batchPointQueryPrepared,
        accessType::indexLookup,
        true,
        batchArgs,
    );
}

// BenchmarkHashExprPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：hash 表达式分区、prepared BatchIndexLookup、plan cache 关闭。
/// BenchmarkHashExprPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：hash 表达式分区、prepared BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkHashExprPartitionPreparedBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHashExpr,
        batchPointQueryPrepared,
        accessType::indexLookup,
        false,
        batchArgs,
    );
}

// BenchmarkHashExprPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：hash 表达式分区、prepared TableScan、plan cache 开启。
/// BenchmarkHashExprPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：hash 表达式分区、prepared TableScan、plan cache 开启。
pub fn BenchmarkHashExprPartitionPreparedTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHashExpr,
        pointQueryPrepared,
        accessType::tableScan,
        true,
        pointArgs,
    );
}

// BenchmarkHashExprPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：hash 表达式分区、prepared TableScan、plan cache 关闭。
/// BenchmarkHashExprPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：hash 表达式分区、prepared TableScan、plan cache 关闭。
pub fn BenchmarkHashExprPartitionPreparedTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHashExpr,
        pointQueryPrepared,
        accessType::tableScan,
        false,
        pointArgs,
    );
}

// BenchmarkHashExprPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：hash 表达式分区、prepared BatchTableScan、plan cache 开启。
/// BenchmarkHashExprPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：hash 表达式分区、prepared BatchTableScan、plan cache 开启。
pub fn BenchmarkHashExprPartitionPreparedBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHashExpr,
        batchPointQueryPrepared,
        accessType::tableScan,
        true,
        batchArgs,
    );
}

// BenchmarkHashExprPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：hash 表达式分区、prepared BatchTableScan、plan cache 关闭。
/// BenchmarkHashExprPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：hash 表达式分区、prepared BatchTableScan、plan cache 关闭。
pub fn BenchmarkHashExprPartitionPreparedBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByHashExpr,
        batchPointQueryPrepared,
        accessType::tableScan,
        false,
        batchArgs,
    );
}

// BenchmarkHashExprPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖hash 表达式分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkHashExprPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖hash 表达式分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkHashExprPartitionPrepared(iterations: usize) {
    benchPreparedPointGet(iterations, partitionByHashExpr);
}

// BenchmarkListPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：list 分区、prepared PointGet、plan cache 开启。
/// BenchmarkListPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：list 分区、prepared PointGet、plan cache 开启。
pub fn BenchmarkListPartitionPreparedPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", false),
        pointQueryPrepared,
        accessType::pointGet,
        true,
        pointArgs,
    );
}

// BenchmarkListPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：list 分区、prepared PointGet、plan cache 关闭。
/// BenchmarkListPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：list 分区、prepared PointGet、plan cache 关闭。
pub fn BenchmarkListPartitionPreparedPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", false),
        pointQueryPrepared,
        accessType::pointGet,
        false,
        pointArgs,
    );
}

// BenchmarkListPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：list 分区、prepared BatchPointGet、plan cache 开启。
/// BenchmarkListPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：list 分区、prepared BatchPointGet、plan cache 开启。
pub fn BenchmarkListPartitionPreparedBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", false),
        batchPointQueryPrepared,
        accessType::pointGet,
        true,
        batchArgs,
    );
}

// BenchmarkListPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：list 分区、prepared BatchPointGet、plan cache 关闭。
/// BenchmarkListPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：list 分区、prepared BatchPointGet、plan cache 关闭。
pub fn BenchmarkListPartitionPreparedBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", false),
        batchPointQueryPrepared,
        accessType::pointGet,
        false,
        batchArgs,
    );
}

// BenchmarkListPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：list 分区、prepared IndexLookup、plan cache 开启。
/// BenchmarkListPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：list 分区、prepared IndexLookup、plan cache 开启。
pub fn BenchmarkListPartitionPreparedIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", false),
        pointQueryPrepared,
        accessType::indexLookup,
        true,
        pointArgs,
    );
}

// BenchmarkListPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：list 分区、prepared IndexLookup、plan cache 关闭。
/// BenchmarkListPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：list 分区、prepared IndexLookup、plan cache 关闭。
pub fn BenchmarkListPartitionPreparedIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", false),
        pointQueryPrepared,
        accessType::indexLookup,
        false,
        pointArgs,
    );
}

// BenchmarkListPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：list 分区、prepared BatchIndexLookup、plan cache 开启。
/// BenchmarkListPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：list 分区、prepared BatchIndexLookup、plan cache 开启。
pub fn BenchmarkListPartitionPreparedBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", false),
        batchPointQueryPrepared,
        accessType::indexLookup,
        true,
        batchArgs,
    );
}

// BenchmarkListPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：list 分区、prepared BatchIndexLookup、plan cache 关闭。
/// BenchmarkListPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：list 分区、prepared BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkListPartitionPreparedBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", false),
        batchPointQueryPrepared,
        accessType::indexLookup,
        false,
        batchArgs,
    );
}

// BenchmarkListPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：list 分区、prepared TableScan、plan cache 开启。
/// BenchmarkListPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：list 分区、prepared TableScan、plan cache 开启。
pub fn BenchmarkListPartitionPreparedTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", false),
        pointQueryPrepared,
        accessType::tableScan,
        true,
        pointArgs,
    );
}

// BenchmarkListPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：list 分区、prepared TableScan、plan cache 关闭。
/// BenchmarkListPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：list 分区、prepared TableScan、plan cache 关闭。
pub fn BenchmarkListPartitionPreparedTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", false),
        pointQueryPrepared,
        accessType::tableScan,
        false,
        pointArgs,
    );
}

// BenchmarkListPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：list 分区、prepared BatchTableScan、plan cache 开启。
/// BenchmarkListPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：list 分区、prepared BatchTableScan、plan cache 开启。
pub fn BenchmarkListPartitionPreparedBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", false),
        batchPointQueryPrepared,
        accessType::tableScan,
        true,
        batchArgs,
    );
}

// BenchmarkListPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：list 分区、prepared BatchTableScan、plan cache 关闭。
/// BenchmarkListPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：list 分区、prepared BatchTableScan、plan cache 关闭。
pub fn BenchmarkListPartitionPreparedBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", false),
        batchPointQueryPrepared,
        accessType::tableScan,
        false,
        batchArgs,
    );
}

// BenchmarkListPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖list 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkListPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖list 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkListPartitionPrepared(iterations: usize) {
    benchPreparedPointGet(iterations, &getListPartitionDef("id", false));
}

// BenchmarkListExprPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：list 表达式分区、prepared PointGet、plan cache 开启。
/// BenchmarkListExprPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：list 表达式分区、prepared PointGet、plan cache 开启。
pub fn BenchmarkListExprPartitionPreparedPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        pointQueryPrepared,
        accessType::pointGet,
        true,
        pointArgs,
    );
}

// BenchmarkListExprPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：list 表达式分区、prepared PointGet、plan cache 关闭。
/// BenchmarkListExprPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：list 表达式分区、prepared PointGet、plan cache 关闭。
pub fn BenchmarkListExprPartitionPreparedPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        pointQueryPrepared,
        accessType::pointGet,
        false,
        pointArgs,
    );
}

// BenchmarkListExprPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：list 表达式分区、prepared BatchPointGet、plan cache 开启。
/// BenchmarkListExprPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：list 表达式分区、prepared BatchPointGet、plan cache 开启。
pub fn BenchmarkListExprPartitionPreparedBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        batchPointQueryPrepared,
        accessType::pointGet,
        true,
        batchArgs,
    );
}

// BenchmarkListExprPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：list 表达式分区、prepared BatchPointGet、plan cache 关闭。
/// BenchmarkListExprPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：list 表达式分区、prepared BatchPointGet、plan cache 关闭。
pub fn BenchmarkListExprPartitionPreparedBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        batchPointQueryPrepared,
        accessType::pointGet,
        false,
        batchArgs,
    );
}

// BenchmarkListExprPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：list 表达式分区、prepared IndexLookup、plan cache 开启。
/// BenchmarkListExprPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：list 表达式分区、prepared IndexLookup、plan cache 开启。
pub fn BenchmarkListExprPartitionPreparedIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        pointQueryPrepared,
        accessType::indexLookup,
        true,
        pointArgs,
    );
}

// BenchmarkListExprPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：list 表达式分区、prepared IndexLookup、plan cache 关闭。
/// BenchmarkListExprPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：list 表达式分区、prepared IndexLookup、plan cache 关闭。
pub fn BenchmarkListExprPartitionPreparedIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        pointQueryPrepared,
        accessType::indexLookup,
        false,
        pointArgs,
    );
}

// BenchmarkListExprPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：list 表达式分区、prepared BatchIndexLookup、plan cache 开启。
/// BenchmarkListExprPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：list 表达式分区、prepared BatchIndexLookup、plan cache 开启。
pub fn BenchmarkListExprPartitionPreparedBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        batchPointQueryPrepared,
        accessType::indexLookup,
        true,
        batchArgs,
    );
}

// BenchmarkListExprPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：list 表达式分区、prepared BatchIndexLookup、plan cache 关闭。
/// BenchmarkListExprPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：list 表达式分区、prepared BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkListExprPartitionPreparedBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        batchPointQueryPrepared,
        accessType::indexLookup,
        false,
        batchArgs,
    );
}

// BenchmarkListExprPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：list 表达式分区、prepared TableScan、plan cache 开启。
/// BenchmarkListExprPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：list 表达式分区、prepared TableScan、plan cache 开启。
pub fn BenchmarkListExprPartitionPreparedTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        pointQueryPrepared,
        accessType::tableScan,
        true,
        pointArgs,
    );
}

// BenchmarkListExprPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：list 表达式分区、prepared TableScan、plan cache 关闭。
/// BenchmarkListExprPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：list 表达式分区、prepared TableScan、plan cache 关闭。
pub fn BenchmarkListExprPartitionPreparedTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        pointQueryPrepared,
        accessType::tableScan,
        false,
        pointArgs,
    );
}

// BenchmarkListExprPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：list 表达式分区、prepared BatchTableScan、plan cache 开启。
/// BenchmarkListExprPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：list 表达式分区、prepared BatchTableScan、plan cache 开启。
pub fn BenchmarkListExprPartitionPreparedBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        batchPointQueryPrepared,
        accessType::tableScan,
        true,
        batchArgs,
    );
}

// BenchmarkListExprPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：list 表达式分区、prepared BatchTableScan、plan cache 关闭。
/// BenchmarkListExprPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：list 表达式分区、prepared BatchTableScan、plan cache 关闭。
pub fn BenchmarkListExprPartitionPreparedBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("floor(id*0.5)*2", false),
        batchPointQueryPrepared,
        accessType::tableScan,
        false,
        batchArgs,
    );
}

// BenchmarkListExprPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖list 表达式分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkListExprPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖list 表达式分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkListExprPartitionPrepared(iterations: usize) {
    benchPreparedPointGet(iterations, &getListPartitionDef("floor(id*0.5)*2", false));
}

// BenchmarkListColumnsPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：list columns 分区、prepared PointGet、plan cache 开启。
/// BenchmarkListColumnsPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：list columns 分区、prepared PointGet、plan cache 开启。
pub fn BenchmarkListColumnsPartitionPreparedPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", true),
        pointQueryPrepared,
        accessType::pointGet,
        true,
        pointArgs,
    );
}

// BenchmarkListColumnsPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：list columns 分区、prepared PointGet、plan cache 关闭。
/// BenchmarkListColumnsPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：list columns 分区、prepared PointGet、plan cache 关闭。
pub fn BenchmarkListColumnsPartitionPreparedPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", true),
        pointQueryPrepared,
        accessType::pointGet,
        false,
        pointArgs,
    );
}

// BenchmarkListColumnsPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：list columns 分区、prepared BatchPointGet、plan cache 开启。
/// BenchmarkListColumnsPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：list columns 分区、prepared BatchPointGet、plan cache 开启。
pub fn BenchmarkListColumnsPartitionPreparedBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", true),
        batchPointQueryPrepared,
        accessType::pointGet,
        true,
        batchArgs,
    );
}

// BenchmarkListColumnsPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：list columns 分区、prepared BatchPointGet、plan cache 关闭。
/// BenchmarkListColumnsPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：list columns 分区、prepared BatchPointGet、plan cache 关闭。
pub fn BenchmarkListColumnsPartitionPreparedBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", true),
        batchPointQueryPrepared,
        accessType::pointGet,
        false,
        batchArgs,
    );
}

// BenchmarkListColumnsPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：list columns 分区、prepared IndexLookup、plan cache 开启。
/// BenchmarkListColumnsPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：list columns 分区、prepared IndexLookup、plan cache 开启。
pub fn BenchmarkListColumnsPartitionPreparedIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", true),
        pointQueryPrepared,
        accessType::indexLookup,
        true,
        pointArgs,
    );
}

// BenchmarkListColumnsPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：list columns 分区、prepared IndexLookup、plan cache 关闭。
/// BenchmarkListColumnsPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：list columns 分区、prepared IndexLookup、plan cache 关闭。
pub fn BenchmarkListColumnsPartitionPreparedIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", true),
        pointQueryPrepared,
        accessType::indexLookup,
        false,
        pointArgs,
    );
}

// BenchmarkListColumnsPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：list columns 分区、prepared BatchIndexLookup、plan cache 开启。
/// BenchmarkListColumnsPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：list columns 分区、prepared BatchIndexLookup、plan cache 开启。
pub fn BenchmarkListColumnsPartitionPreparedBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", true),
        batchPointQueryPrepared,
        accessType::indexLookup,
        true,
        batchArgs,
    );
}

// BenchmarkListColumnsPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：list columns 分区、prepared BatchIndexLookup、plan cache 关闭。
/// BenchmarkListColumnsPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：list columns 分区、prepared BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkListColumnsPartitionPreparedBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", true),
        batchPointQueryPrepared,
        accessType::indexLookup,
        false,
        batchArgs,
    );
}

// BenchmarkListColumnsPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：list columns 分区、prepared TableScan、plan cache 开启。
/// BenchmarkListColumnsPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：list columns 分区、prepared TableScan、plan cache 开启。
pub fn BenchmarkListColumnsPartitionPreparedTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", true),
        pointQueryPrepared,
        accessType::tableScan,
        true,
        pointArgs,
    );
}

// BenchmarkListColumnsPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：list columns 分区、prepared TableScan、plan cache 关闭。
/// BenchmarkListColumnsPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：list columns 分区、prepared TableScan、plan cache 关闭。
pub fn BenchmarkListColumnsPartitionPreparedTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", true),
        pointQueryPrepared,
        accessType::tableScan,
        false,
        pointArgs,
    );
}

// BenchmarkListColumnsPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：list columns 分区、prepared BatchTableScan、plan cache 开启。
/// BenchmarkListColumnsPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：list columns 分区、prepared BatchTableScan、plan cache 开启。
pub fn BenchmarkListColumnsPartitionPreparedBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", true),
        batchPointQueryPrepared,
        accessType::tableScan,
        true,
        batchArgs,
    );
}

// BenchmarkListColumnsPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：list columns 分区、prepared BatchTableScan、plan cache 关闭。
/// BenchmarkListColumnsPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：list columns 分区、prepared BatchTableScan、plan cache 关闭。
pub fn BenchmarkListColumnsPartitionPreparedBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        &getListPartitionDef("id", true),
        batchPointQueryPrepared,
        accessType::tableScan,
        false,
        batchArgs,
    );
}

// BenchmarkListColumnPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖list columns 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkListColumnPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖list columns 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkListColumnPartitionPrepared(iterations: usize) {
    benchPreparedPointGet(iterations, &getListPartitionDef("id", true));
}

// BenchmarkRangePartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：range 分区、prepared PointGet、plan cache 开启。
/// BenchmarkRangePartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：range 分区、prepared PointGet、plan cache 开启。
pub fn BenchmarkRangePartitionPreparedPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangePrep,
        pointQueryPrepared,
        accessType::pointGet,
        true,
        pointArgs,
    );
}

// BenchmarkRangePartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：range 分区、prepared PointGet、plan cache 关闭。
/// BenchmarkRangePartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：range 分区、prepared PointGet、plan cache 关闭。
pub fn BenchmarkRangePartitionPreparedPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangePrep,
        pointQueryPrepared,
        accessType::pointGet,
        false,
        pointArgs,
    );
}

// BenchmarkRangePartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：range 分区、prepared BatchPointGet、plan cache 开启。
/// BenchmarkRangePartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：range 分区、prepared BatchPointGet、plan cache 开启。
pub fn BenchmarkRangePartitionPreparedBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangePrep,
        batchPointQueryPrepared,
        accessType::pointGet,
        true,
        batchArgs,
    );
}

// BenchmarkRangePartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：range 分区、prepared BatchPointGet、plan cache 关闭。
/// BenchmarkRangePartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：range 分区、prepared BatchPointGet、plan cache 关闭。
pub fn BenchmarkRangePartitionPreparedBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangePrep,
        batchPointQueryPrepared,
        accessType::pointGet,
        false,
        batchArgs,
    );
}

// BenchmarkRangePartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：range 分区、prepared IndexLookup、plan cache 开启。
/// BenchmarkRangePartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：range 分区、prepared IndexLookup、plan cache 开启。
pub fn BenchmarkRangePartitionPreparedIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangePrep,
        pointQueryPrepared,
        accessType::indexLookup,
        true,
        pointArgs,
    );
}

// BenchmarkRangePartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：range 分区、prepared IndexLookup、plan cache 关闭。
/// BenchmarkRangePartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：range 分区、prepared IndexLookup、plan cache 关闭。
pub fn BenchmarkRangePartitionPreparedIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangePrep,
        pointQueryPrepared,
        accessType::indexLookup,
        false,
        pointArgs,
    );
}

// BenchmarkRangePartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：range 分区、prepared BatchIndexLookup、plan cache 开启。
/// BenchmarkRangePartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：range 分区、prepared BatchIndexLookup、plan cache 开启。
pub fn BenchmarkRangePartitionPreparedBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangePrep,
        batchPointQueryPrepared,
        accessType::indexLookup,
        true,
        batchArgs,
    );
}

// BenchmarkRangePartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：range 分区、prepared BatchIndexLookup、plan cache 关闭。
/// BenchmarkRangePartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：range 分区、prepared BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkRangePartitionPreparedBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangePrep,
        batchPointQueryPrepared,
        accessType::indexLookup,
        false,
        batchArgs,
    );
}

// BenchmarkRangePartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：range 分区、prepared TableScan、plan cache 开启。
/// BenchmarkRangePartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：range 分区、prepared TableScan、plan cache 开启。
pub fn BenchmarkRangePartitionPreparedTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangePrep,
        pointQueryPrepared,
        accessType::tableScan,
        true,
        pointArgs,
    );
}

// BenchmarkRangePartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：range 分区、prepared TableScan、plan cache 关闭。
/// BenchmarkRangePartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：range 分区、prepared TableScan、plan cache 关闭。
pub fn BenchmarkRangePartitionPreparedTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangePrep,
        pointQueryPrepared,
        accessType::tableScan,
        false,
        pointArgs,
    );
}

// BenchmarkRangePartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：range 分区、prepared BatchTableScan、plan cache 开启。
/// BenchmarkRangePartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：range 分区、prepared BatchTableScan、plan cache 开启。
pub fn BenchmarkRangePartitionPreparedBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangePrep,
        batchPointQueryPrepared,
        accessType::tableScan,
        true,
        batchArgs,
    );
}

// BenchmarkRangePartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：range 分区、prepared BatchTableScan、plan cache 关闭。
/// BenchmarkRangePartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：range 分区、prepared BatchTableScan、plan cache 关闭。
pub fn BenchmarkRangePartitionPreparedBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangePrep,
        batchPointQueryPrepared,
        accessType::tableScan,
        false,
        batchArgs,
    );
}

// BenchmarkRangePartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖range 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkRangePartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖range 分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkRangePartitionPrepared(iterations: usize) {
    benchPreparedPointGet(iterations, partitionByRangePrep);
}

// BenchmarkRangeExprPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：range 表达式分区、prepared PointGet、plan cache 开启。
/// BenchmarkRangeExprPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：range 表达式分区、prepared PointGet、plan cache 开启。
pub fn BenchmarkRangeExprPartitionPreparedPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeExprPrep,
        pointQueryPrepared,
        accessType::pointGet,
        true,
        pointArgs,
    );
}

// BenchmarkRangeExprPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：range 表达式分区、prepared PointGet、plan cache 关闭。
/// BenchmarkRangeExprPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：range 表达式分区、prepared PointGet、plan cache 关闭。
pub fn BenchmarkRangeExprPartitionPreparedPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeExprPrep,
        pointQueryPrepared,
        accessType::pointGet,
        false,
        pointArgs,
    );
}

// BenchmarkRangeExprPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：range 表达式分区、prepared BatchPointGet、plan cache 开启。
/// BenchmarkRangeExprPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：range 表达式分区、prepared BatchPointGet、plan cache 开启。
pub fn BenchmarkRangeExprPartitionPreparedBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeExprPrep,
        batchPointQueryPrepared,
        accessType::pointGet,
        true,
        batchArgs,
    );
}

// BenchmarkRangeExprPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：range 表达式分区、prepared BatchPointGet、plan cache 关闭。
/// BenchmarkRangeExprPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：range 表达式分区、prepared BatchPointGet、plan cache 关闭。
pub fn BenchmarkRangeExprPartitionPreparedBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeExprPrep,
        batchPointQueryPrepared,
        accessType::pointGet,
        false,
        batchArgs,
    );
}

// BenchmarkRangeExprPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：range 表达式分区、prepared IndexLookup、plan cache 开启。
/// BenchmarkRangeExprPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：range 表达式分区、prepared IndexLookup、plan cache 开启。
pub fn BenchmarkRangeExprPartitionPreparedIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeExprPrep,
        pointQueryPrepared,
        accessType::indexLookup,
        true,
        pointArgs,
    );
}

// BenchmarkRangeExprPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：range 表达式分区、prepared IndexLookup、plan cache 关闭。
/// BenchmarkRangeExprPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：range 表达式分区、prepared IndexLookup、plan cache 关闭。
pub fn BenchmarkRangeExprPartitionPreparedIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeExprPrep,
        pointQueryPrepared,
        accessType::indexLookup,
        false,
        pointArgs,
    );
}

// BenchmarkRangeExprPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：range 表达式分区、prepared BatchIndexLookup、plan cache 开启。
/// BenchmarkRangeExprPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：range 表达式分区、prepared BatchIndexLookup、plan cache 开启。
pub fn BenchmarkRangeExprPartitionPreparedBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeExprPrep,
        batchPointQueryPrepared,
        accessType::indexLookup,
        true,
        batchArgs,
    );
}

// BenchmarkRangeExprPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：range 表达式分区、prepared BatchIndexLookup、plan cache 关闭。
/// BenchmarkRangeExprPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：range 表达式分区、prepared BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkRangeExprPartitionPreparedBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeExprPrep,
        batchPointQueryPrepared,
        accessType::indexLookup,
        false,
        batchArgs,
    );
}

// BenchmarkRangeExprPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：range 表达式分区、prepared TableScan、plan cache 开启。
/// BenchmarkRangeExprPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：range 表达式分区、prepared TableScan、plan cache 开启。
pub fn BenchmarkRangeExprPartitionPreparedTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeExprPrep,
        pointQueryPrepared,
        accessType::tableScan,
        true,
        pointArgs,
    );
}

// BenchmarkRangeExprPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：range 表达式分区、prepared TableScan、plan cache 关闭。
/// BenchmarkRangeExprPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：range 表达式分区、prepared TableScan、plan cache 关闭。
pub fn BenchmarkRangeExprPartitionPreparedTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeExprPrep,
        pointQueryPrepared,
        accessType::tableScan,
        false,
        pointArgs,
    );
}

// BenchmarkRangeExprPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：range 表达式分区、prepared BatchTableScan、plan cache 开启。
/// BenchmarkRangeExprPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：range 表达式分区、prepared BatchTableScan、plan cache 开启。
pub fn BenchmarkRangeExprPartitionPreparedBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeExprPrep,
        batchPointQueryPrepared,
        accessType::tableScan,
        true,
        batchArgs,
    );
}

// BenchmarkRangeExprPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：range 表达式分区、prepared BatchTableScan、plan cache 关闭。
/// BenchmarkRangeExprPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：range 表达式分区、prepared BatchTableScan、plan cache 关闭。
pub fn BenchmarkRangeExprPartitionPreparedBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeExprPrep,
        batchPointQueryPrepared,
        accessType::tableScan,
        false,
        batchArgs,
    );
}

// BenchmarkRangeExprPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖range 表达式分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkRangeExprPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖range 表达式分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkRangeExprPartitionPrepared(iterations: usize) {
    benchPreparedPointGet(iterations, partitionByRangeExprPrep);
}

// BenchmarkRangeColumnsPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：range columns 分区、prepared PointGet、plan cache 开启。
/// BenchmarkRangeColumnsPartitionPreparedPointGetPlanCacheOn 对应 Go benchmark：range columns 分区、prepared PointGet、plan cache 开启。
pub fn BenchmarkRangeColumnsPartitionPreparedPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeColumnsPrep,
        pointQueryPrepared,
        accessType::pointGet,
        true,
        pointArgs,
    );
}

// BenchmarkRangeColumnsPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：range columns 分区、prepared PointGet、plan cache 关闭。
/// BenchmarkRangeColumnsPartitionPreparedPointGetPlanCacheOff 对应 Go benchmark：range columns 分区、prepared PointGet、plan cache 关闭。
pub fn BenchmarkRangeColumnsPartitionPreparedPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeColumnsPrep,
        pointQueryPrepared,
        accessType::pointGet,
        false,
        pointArgs,
    );
}

// BenchmarkRangeColumnsPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：range columns 分区、prepared BatchPointGet、plan cache 开启。
/// BenchmarkRangeColumnsPartitionPreparedBatchPointGetPlanCacheOn 对应 Go benchmark：range columns 分区、prepared BatchPointGet、plan cache 开启。
pub fn BenchmarkRangeColumnsPartitionPreparedBatchPointGetPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeColumnsPrep,
        batchPointQueryPrepared,
        accessType::pointGet,
        true,
        batchArgs,
    );
}

// BenchmarkRangeColumnsPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：range columns 分区、prepared BatchPointGet、plan cache 关闭。
/// BenchmarkRangeColumnsPartitionPreparedBatchPointGetPlanCacheOff 对应 Go benchmark：range columns 分区、prepared BatchPointGet、plan cache 关闭。
pub fn BenchmarkRangeColumnsPartitionPreparedBatchPointGetPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeColumnsPrep,
        batchPointQueryPrepared,
        accessType::pointGet,
        false,
        batchArgs,
    );
}

// BenchmarkRangeColumnsPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：range columns 分区、prepared IndexLookup、plan cache 开启。
/// BenchmarkRangeColumnsPartitionPreparedIndexLookupPlanCacheOn 对应 Go benchmark：range columns 分区、prepared IndexLookup、plan cache 开启。
pub fn BenchmarkRangeColumnsPartitionPreparedIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeColumnsPrep,
        pointQueryPrepared,
        accessType::indexLookup,
        true,
        pointArgs,
    );
}

// BenchmarkRangeColumnsPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：range columns 分区、prepared IndexLookup、plan cache 关闭。
/// BenchmarkRangeColumnsPartitionPreparedIndexLookupPlanCacheOff 对应 Go benchmark：range columns 分区、prepared IndexLookup、plan cache 关闭。
pub fn BenchmarkRangeColumnsPartitionPreparedIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeColumnsPrep,
        pointQueryPrepared,
        accessType::indexLookup,
        false,
        pointArgs,
    );
}

// BenchmarkRangeColumnsPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：range columns 分区、prepared BatchIndexLookup、plan cache 开启。
/// BenchmarkRangeColumnsPartitionPreparedBatchIndexLookupPlanCacheOn 对应 Go benchmark：range columns 分区、prepared BatchIndexLookup、plan cache 开启。
pub fn BenchmarkRangeColumnsPartitionPreparedBatchIndexLookupPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeColumnsPrep,
        batchPointQueryPrepared,
        accessType::indexLookup,
        true,
        batchArgs,
    );
}

// BenchmarkRangeColumnsPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：range columns 分区、prepared BatchIndexLookup、plan cache 关闭。
/// BenchmarkRangeColumnsPartitionPreparedBatchIndexLookupPlanCacheOff 对应 Go benchmark：range columns 分区、prepared BatchIndexLookup、plan cache 关闭。
pub fn BenchmarkRangeColumnsPartitionPreparedBatchIndexLookupPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeColumnsPrep,
        batchPointQueryPrepared,
        accessType::indexLookup,
        false,
        batchArgs,
    );
}

// BenchmarkRangeColumnsPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：range columns 分区、prepared TableScan、plan cache 开启。
/// BenchmarkRangeColumnsPartitionPreparedTableScanPlanCacheOn 对应 Go benchmark：range columns 分区、prepared TableScan、plan cache 开启。
pub fn BenchmarkRangeColumnsPartitionPreparedTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeColumnsPrep,
        pointQueryPrepared,
        accessType::tableScan,
        true,
        pointArgs,
    );
}

// BenchmarkRangeColumnsPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：range columns 分区、prepared TableScan、plan cache 关闭。
/// BenchmarkRangeColumnsPartitionPreparedTableScanPlanCacheOff 对应 Go benchmark：range columns 分区、prepared TableScan、plan cache 关闭。
pub fn BenchmarkRangeColumnsPartitionPreparedTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeColumnsPrep,
        pointQueryPrepared,
        accessType::tableScan,
        false,
        pointArgs,
    );
}

// BenchmarkRangeColumnsPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：range columns 分区、prepared BatchTableScan、plan cache 开启。
/// BenchmarkRangeColumnsPartitionPreparedBatchTableScanPlanCacheOn 对应 Go benchmark：range columns 分区、prepared BatchTableScan、plan cache 开启。
pub fn BenchmarkRangeColumnsPartitionPreparedBatchTableScanPlanCacheOn(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeColumnsPrep,
        batchPointQueryPrepared,
        accessType::tableScan,
        true,
        batchArgs,
    );
}

// BenchmarkRangeColumnsPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：range columns 分区、prepared BatchTableScan、plan cache 关闭。
/// BenchmarkRangeColumnsPartitionPreparedBatchTableScanPlanCacheOff 对应 Go benchmark：range columns 分区、prepared BatchTableScan、plan cache 关闭。
pub fn BenchmarkRangeColumnsPartitionPreparedBatchTableScanPlanCacheOff(iterations: usize) {
    runBenchmarkPrepared(
        iterations,
        partitionByRangeColumnsPrep,
        batchPointQueryPrepared,
        accessType::tableScan,
        false,
        batchArgs,
    );
}

// BenchmarkRangeColumnPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖range columns 预处理分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
/// BenchmarkRangeColumnPartitionPrepared 对应 Go 的组合 prepared benchmark，覆盖range columns 预处理分区下 PointGet、BatchPointGet、IndexLookup 与 TableScan 子项。
pub fn BenchmarkRangeColumnPartitionPrepared(iterations: usize) {
    benchPreparedPointGet(iterations, partitionByRangeColumnsPrep);
}

// BenchmarkHashPartitionMultiPointSelect 对应 Go 的多点 hash 分区选择 benchmark。
// 每次迭代依次执行等值、OR 和 IN 三条 SQL，并复用同一个 chunk allocator 后 Reset。
/// HASH 分区多点选择：等值、OR、IN 三条 SQL 轮询执行。
pub fn BenchmarkHashPartitionMultiPointSelect(iterations: usize) {
    let session = prepareBenchSession();
    mustExecute(
        &session,
        "create table t (id int primary key, dt datetime) partition by hash(id) partitions 64",
        &[],
    );
    for _ in 0..iterations {
        execute_and_drain(&session, "select * from t where id = 2330");
        execute_and_drain(&session, "select * from t where id = 1233 or id = 1512");
        execute_and_drain(&session, "select * from t where id in (117, 1233, 15678)");
    }
}

// TestBenchDaily 对应 Go 的 benchdaily.Run 清单。
// Go 文件中大量候选 benchmark 以注释保留；这里记录当前启用的日常 benchmark 入口。
/// 对应 Go benchdaily.Run：登记日常启用的 benchmark 入口清单。
pub fn TestBenchDaily() {
    benchdaily_run();
}

/// 执行上下文兼容类型；真实 benchmark 使用 `TestSession` 的执行上下文。
#[derive(Debug, Clone, Copy)]
pub struct ContextDraft;
/// 结果集兼容类型，用于保留迁移后的公开 `drainRecordSet` 形状。
#[derive(Debug, Clone)]
pub struct RecordSetDraft {
    rows: Vec<RowDraft>,
    next_row: usize,
    closed: bool,
}

impl RecordSetDraft {
    pub fn new(rows: Vec<RowDraft>) -> Self {
        Self {
            rows,
            next_row: 0,
            closed: false,
        }
    }
}
/// chunk 分配器兼容类型。
#[derive(Debug, Clone, Copy)]
pub struct ChunkAllocatorDraft;
/// 行兼容类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowDraft {
    pub values: Vec<String>,
}

fn drain_test_record_set(mut record_set: Box<dyn TestRecordSet>) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    loop {
        match record_set
            .Next()
            .unwrap_or_else(|error| panic!("read record set failed: {error}"))
        {
            Some(row) => rows.push(row),
            None => break,
        }
    }
    record_set
        .Close()
        .unwrap_or_else(|error| panic!("close record set failed: {error}"));
    rows
}

fn execute_and_drain(session: &BenchSessionDraft, query: &str) -> Vec<Vec<String>> {
    let mut record_sets = session
        .session
        .Execute(query)
        .unwrap_or_else(|error| panic!("execute {query}: {error}"));
    let record_set = record_sets
        .drain(..)
        .next()
        .unwrap_or_else(|| panic!("execute {query} returned no record set"));
    drain_test_record_set(record_set)
}

fn explain_and_check_plan(session: &BenchSessionDraft, query: &str, expected_plan: &str) -> bool {
    let rows = execute_and_drain(session, &format!("explain {query}"));
    let actual = rows
        .first()
        .and_then(|row| row.first())
        .map(String::as_str)
        .unwrap_or("");
    let matches = actual.starts_with(expected_plan);
    if !matches {
        eprintln!("expected plan {expected_plan} for {query}, got {actual:?}");
    }
    matches
}

fn set_prepared_plan_cache(session: &BenchSessionDraft, enable: bool) {
    mustExecute(
        session,
        if enable {
            "set tidb_enable_prepared_plan_cache = 1"
        } else {
            "set tidb_enable_prepared_plan_cache = 0"
        },
        &[],
    );
}

fn prepare_stmt(session: &BenchSessionDraft, query: &str) {
    let query = query.replace('\\', "\\\\").replace('\'', "''");
    mustExecute(
        session,
        &format!("prepare aster_partition_bench_stmt from '{query}'"),
        &[],
    );
}

fn bind_prepared_args(session: &BenchSessionDraft, args: &[i32]) -> String {
    for (index, argument) in args.iter().enumerate() {
        mustExecute(
            session,
            &format!("set @aster_bench_arg_{index} = {argument}"),
            &[],
        );
    }
    if args.is_empty() {
        String::new()
    } else {
        format!(
            " using {}",
            (0..args.len())
                .map(|index| format!("@aster_bench_arg_{index}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

fn execute_prepared_and_drain(session: &BenchSessionDraft, using_clause: &str) {
    let _ = execute_and_drain(
        session,
        &format!("execute aster_partition_bench_stmt{using_clause}"),
    );
}

macro_rules! daily_adapter {
    ($name:ident, $benchmark:ident) => {
        fn $name(benchmark: &mut astersql_util_benchdaily::Benchmark) {
            $benchmark(benchmark.iterations() as usize);
        }
    };
}

daily_adapter!(daily_non_point_on, BenchmarkNonPartitionPointGetPlanCacheOn);
daily_adapter!(
    daily_non_point_off,
    BenchmarkNonPartitionPointGetPlanCacheOff
);
daily_adapter!(
    daily_non_batch_point_on,
    BenchmarkNonPartitionBatchPointGetPlanCacheOn
);
daily_adapter!(
    daily_non_batch_point_off,
    BenchmarkNonPartitionBatchPointGetPlanCacheOff
);
daily_adapter!(
    daily_non_index_on,
    BenchmarkNonPartitionIndexLookupPlanCacheOn
);
daily_adapter!(
    daily_non_index_off,
    BenchmarkNonPartitionIndexLookupPlanCacheOff
);
daily_adapter!(
    daily_non_batch_index_on,
    BenchmarkNonPartitionBatchIndexLookupPlanCacheOn
);
daily_adapter!(
    daily_non_batch_index_off,
    BenchmarkNonPartitionBatchIndexLookupPlanCacheOff
);
daily_adapter!(
    daily_hash_point_on,
    BenchmarkHashPartitionPointGetPlanCacheOn
);
daily_adapter!(
    daily_hash_point_off,
    BenchmarkHashPartitionPointGetPlanCacheOff
);
daily_adapter!(
    daily_hash_batch_point_on,
    BenchmarkHashPartitionBatchPointGetPlanCacheOn
);
daily_adapter!(
    daily_hash_batch_point_off,
    BenchmarkHashPartitionBatchPointGetPlanCacheOff
);
daily_adapter!(
    daily_hash_index_on,
    BenchmarkHashPartitionIndexLookupPlanCacheOn
);
daily_adapter!(
    daily_hash_index_off,
    BenchmarkHashPartitionIndexLookupPlanCacheOff
);
daily_adapter!(
    daily_hash_batch_index_on,
    BenchmarkHashPartitionBatchIndexLookupPlanCacheOn
);
daily_adapter!(
    daily_hash_batch_index_off,
    BenchmarkHashPartitionBatchIndexLookupPlanCacheOff
);
daily_adapter!(
    daily_range_point_on,
    BenchmarkRangePartitionPointGetPlanCacheOn
);
daily_adapter!(
    daily_range_batch_point_on,
    BenchmarkRangePartitionBatchPointGetPlanCacheOn
);
daily_adapter!(
    daily_range_index_on,
    BenchmarkRangePartitionIndexLookupPlanCacheOn
);
daily_adapter!(
    daily_range_batch_index_on,
    BenchmarkRangePartitionBatchIndexLookupPlanCacheOn
);

fn benchdaily_run() {
    astersql_util_benchdaily::Run(vec![
        daily_non_point_on,
        daily_non_point_off,
        daily_non_batch_point_on,
        daily_non_batch_point_off,
        daily_non_index_on,
        daily_non_index_off,
        daily_non_batch_index_on,
        daily_non_batch_index_off,
        daily_hash_point_on,
        daily_hash_point_off,
        daily_hash_batch_point_on,
        daily_hash_batch_point_off,
        daily_hash_index_on,
        daily_hash_index_off,
        daily_hash_batch_index_on,
        daily_hash_batch_index_off,
        daily_range_point_on,
        daily_range_batch_point_on,
        daily_range_index_on,
        daily_range_batch_index_on,
    ]);
}

#[test]
fn partitioned_batch_point_get_wrapper_uses_table_reader_plan() {
    let session = prepareBenchSession();
    preparePointGet(&session, partitionByHash);

    assert!(explain_and_check_plan(
        &session,
        batchPointQuery,
        expectedTableScanPlan
    ));

    preparePointGet(&session, "");
    assert!(explain_and_check_plan(
        &session,
        batchPointQuery,
        expectedBatchPointPlan
    ));
}

#[test]
fn prepared_benchmark_hits_plan_cache_after_first_execution() {
    let session = prepareBenchSession();
    preparePointGet(&session, "");
    runPreparedPointSelect(&session, 2, pointQueryPrepared, true, &[pointArgs]);

    assert_eq!(
        execute_and_drain(&session, "select @@last_plan_from_cache"),
        vec![vec!["1".to_owned()]]
    );
}
