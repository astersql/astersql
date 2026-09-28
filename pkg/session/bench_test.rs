// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// Session 执行路径的可执行性能测试。
//
// 对应 Go `bench_test.go`：准备 mock store/session/测试数据，覆盖 TableScan、
// PointGet、IndexLookUp、Sort、Join、分区裁剪与 Pipelined DML 等热路径；
// helper 接线 canonical mock-store session 与 benchdaily runner。

// 这段逻辑只描述 session benchmark 如何准备 mock store、session、测试数据和热路径 SQL，不连接真实 TiDB 集群。
// 所有 benchmark 入口都保留 Go 名称；资源关闭、failpoint、prepared stmt、chunk allocator、BulkDML 标记等迁移点用中文标注。
#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::cell::RefCell;
use std::sync::Arc;

use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
use crate::testutil::{TestRecordSet, TestSession};

thread_local! {
    static ACTIVE_BENCH_SESSION: RefCell<Option<Arc<ConcreteSession>>> = const { RefCell::new(None) };
}

/// 小规模数据集行数（默认 100）。
pub static mut smallCount: i32 = 100;
/// 大规模数据集行数（默认 10000）。
pub static mut bigCount: i32 = 10000;
/// 批量准备数据时的批次数。
pub static mut batchNum: i32 = 100;
/// 每批插入行数。
pub static mut batchSize: i32 = 100;

/// 准备 benchmark 用的 session/domain/storage 草稿。
pub fn prepareBenchSession() -> BenchSession {
    let (domain, session) = CreateAnalyzeSession()
        .unwrap_or_else(|error| panic!("create benchmark session failed: {error}"));
    let session = Arc::new(session);
    ACTIVE_BENCH_SESSION.with(|active| active.replace(Some(Arc::clone(&session))));
    let bench = BenchSession {
        session,
        _domain: domain,
    };
    must_execute("use test");
    bench
}

/// 准备聚簇主键表及二级索引上的 benchmark 数据。
pub fn prepareBenchData(col_type: &str, value_format: &str, value_count: i32) {
    // 对应聚簇主键表：drop/create/begin/insert/commit，索引列为 col。
    prepare_values(
        "create table t (pk int primary key auto_increment, col {col_type}, index idx (col))",
        col_type,
        value_format,
        value_count,
    );
}

/// 准备非聚簇主键表数据，用于 IndexLookUp benchmark。
pub fn prepareNonclusteredBenchData(col_type: &str, value_format: &str, value_count: i32) {
    // 非聚簇主键版本保留 /*T![clustered_index] NONCLUSTERED */ hint，用于 IndexLookUp benchmark。
    prepare_values(
        "create table t (pk int primary key /*T![clustered_index] NONCLUSTERED */ auto_increment, col {col_type}, index idx (col))",
        col_type,
        value_format,
        value_count,
    );
}

/// 准备排序 benchmark 数据（随机值、批量提交）。
pub fn prepareSortBenchData(col_type: &str, value_format: &str, value_count: i32) {
    // Go 每 1000 行提交一次并用 rand.Intn(valueCount) 打散排序列；这里保留批量提交和随机值来源。
    prepare_values_with_order(
        "create table t (pk int primary key auto_increment, col {col_type})",
        col_type,
        value_format,
        value_count,
        true,
    );
}

/// 准备 Join benchmark 数据（无二级索引）。
pub fn prepareJoinBenchData(col_type: &str, value_format: &str, value_count: i32) {
    // join benchmark 不建二级索引，只准备 pk 和 col。
    prepare_values(
        "create table t (pk int primary key auto_increment, col {col_type})",
        col_type,
        value_format,
        value_count,
    );
}

/// 从 RecordSet 读取指定行数并关闭结果集。
pub fn readResult(mut rs: BenchmarkRecordSet, mut count: i32) {
    while count > 0 {
        match rs
            .Next()
            .unwrap_or_else(|error| panic!("read result failed: {error}"))
        {
            Some(_) => count -= 1,
            None => panic!("record set ended with {count} row(s) remaining"),
        }
    }
    rs.Close()
        .unwrap_or_else(|error| panic!("close record set failed: {error}"));
}

/// 通过 explain 结果断言物理计划中包含指定算子名。
pub fn hasPlan(plan: &str) {
    let rows = execute_and_drain("explain select * from t where col = 'hello 64'");
    if !rows.iter().flatten().any(|column| column.contains(plan)) {
        panic!("plan does not contain `{plan}`: {rows:?}");
    }
}

/// 基准：常量查询 `select 1`。
pub fn BenchmarkBasic(iterations: usize) {
    let _bench = prepareBenchSession();
    run_query_benchmark("select 1", 1, iterations);
}

/// 基准：全表扫描。
pub fn BenchmarkTableScan(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareBenchData("int", "%v", unsafe { smallCount });
    run_query_benchmark("select * from t", unsafe { smallCount }, iterations);
}

/// 基准：explain 全表扫描。
pub fn BenchmarkExplainTableScan(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareBenchData("int", "%v", 0);
    run_query_benchmark("explain select * from t", 1, iterations);
}

/// 基准：主键点查。
pub fn BenchmarkTableLookup(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareBenchData("int", "%d", unsafe { smallCount });
    run_query_benchmark("select * from t where pk = 64", 1, iterations);
}

/// 基准：explain 主键点查。
pub fn BenchmarkExplainTableLookup(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareBenchData("int", "%d", 0);
    run_query_benchmark("explain select * from t where pk = 64", 1, iterations);
}

/// 基准：字符串索引范围扫描。
pub fn BenchmarkStringIndexScan(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareBenchData("varchar(255)", "'hello %d'", unsafe { smallCount });
    run_query_benchmark(
        "select * from t where col > 'hello'",
        unsafe { smallCount },
        iterations,
    );
}

/// 基准：explain 字符串索引扫描。
pub fn BenchmarkExplainStringIndexScan(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareBenchData("varchar(255)", "'hello %d'", 0);
    run_query_benchmark("explain select * from t where col > 'hello'", 1, iterations);
}

/// 基准：PointGet（主键等值，走 drain 路径）。
pub fn BenchmarkPointGet(iterations: usize) {
    let _bench = prepareBenchSession();
    // PointGet 使用 drainRecordSet 和 chunk allocator，循环后 Reset allocator，避免 readResult 的额外路径影响。
    must_execute_many(&[
        "create table t (pk int primary key)",
        "insert t values (61),(62),(63),(64)",
    ]);
    run_drain_benchmark("select * from t where pk = 64", iterations);
}

/// 基准：BatchPointGet（IN 列表）。
pub fn BenchmarkBatchPointGet(iterations: usize) {
    let _bench = prepareBenchSession();
    must_execute_many(&[
        "create table t (pk int primary key)",
        "insert t values (61),(62),(63),(64)",
    ]);
    run_drain_benchmark("select * from t where pk in (61, 64, 67)", iterations);
}

/// 基准：预处理语句 PointGet。
pub fn BenchmarkPreparedPointGet(iterations: usize) {
    let _bench = prepareBenchSession();
    // Go PrepareStmt("select * from t where pk = ?") 后复用 stmtID 和 Args2Expressions4Test(64)。
    must_execute_many(&[
        "create table t (pk int primary key)",
        "insert t values (61),(62),(63),(64)",
    ]);
    run_prepared_drain_benchmark("select * from t where pk = ?", "64", iterations);
}

/// 基准：字符串索引回表（IndexLookUp）。
pub fn BenchmarkStringIndexLookup(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareNonclusteredBenchData("varchar(255)", "'hello %d'", unsafe { smallCount });
    hasPlan("IndexLookUp");
    run_query_benchmark("select * from t where col = 'hello 64'", 1, iterations);
}

/// 基准：整型索引范围扫描。
pub fn BenchmarkIntegerIndexScan(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareBenchData("int", "%v", unsafe { smallCount });
    run_query_benchmark(
        "select * from t where col >= 0",
        unsafe { smallCount },
        iterations,
    );
}

/// 基准：整型索引回表。
pub fn BenchmarkIntegerIndexLookup(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareNonclusteredBenchData("int", "%v", unsafe { smallCount });
    hasPlan("IndexLookUp");
    run_query_benchmark("select * from t where col = 64", 1, iterations);
}

/// 基准：Decimal 索引范围扫描。
pub fn BenchmarkDecimalIndexScan(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareBenchData("decimal(32,6)", "%v.1234", unsafe { smallCount });
    run_query_benchmark(
        "select * from t where col >= 0",
        unsafe { smallCount },
        iterations,
    );
}

/// 基准：Decimal 索引回表。
pub fn BenchmarkDecimalIndexLookup(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareNonclusteredBenchData("decimal(32,6)", "%v.1234", unsafe { smallCount });
    hasPlan("IndexLookUp");
    run_query_benchmark("select * from t where col = 64.1234", 1, iterations);
}

/// 基准：带二级索引的插入。
pub fn BenchmarkInsertWithIndex(iterations: usize) {
    let _bench = prepareBenchSession();
    must_execute_many(&[
        "set @@tidb_enable_mutation_checker = 0",
        "drop table if exists t",
        "create table t (pk int primary key, col int, index idx (col))",
    ]);
    run_insert_loop("insert t values ({i}, {i})", iterations);
}

/// 基准：无二级索引的插入。
pub fn BenchmarkInsertNoIndex(iterations: usize) {
    let _bench = prepareBenchSession();
    must_execute_many(&[
        "drop table if exists t",
        "create table t (pk int primary key, col int)",
    ]);
    run_insert_loop("insert t values ({i}, {i})", iterations);
}

/// 基准：ORDER BY + LIMIT。
pub fn BenchmarkSort(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareSortBenchData("int", "%v", unsafe { bigCount });
    run_query_benchmark("select * from t order by col limit 50", 50, iterations);
}

/// 基准：百万行全量排序。
pub fn BenchmarkSort2(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareSortBenchData("int", "%v", 1_000_000);
    run_query_benchmark("select * from t order by col", 1_000_000, iterations);
}

/// 基准：自连接。
pub fn BenchmarkJoin(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareJoinBenchData("int", "%v", unsafe { smallCount });
    run_query_benchmark(
        "select * from t a join t b on a.col = b.col",
        unsafe { smallCount },
        iterations,
    );
}

/// 基准：自连接 + LIMIT 1。
pub fn BenchmarkJoinLimit(iterations: usize) {
    let _bench = prepareBenchSession();
    prepareJoinBenchData("int", "%v", unsafe { smallCount });
    run_query_benchmark(
        "select * from t a join t b on a.col = b.col limit 1",
        1,
        iterations,
    );
}

/// 基准：RANGE 分区裁剪（partition pruning）。
pub fn BenchmarkPartitionPruning(iterations: usize) {
    let _bench = prepareBenchSession();
    // Go 源文件直接列出 p0..p1023 的 range(to_days(dt)) 分区；这里用同样边界生成，避免在这里重复 1024 行 DDL。
    let ddl = build_range_partition_ddl(
        "create table t (id int, dt datetime)\npartition by range (to_days(dt))",
        0,
        1023,
        737515,
    );
    must_execute(&ddl);
    must_execute("analyze table t");
    run_drain_benchmark(
        "select * from t where dt > to_days('2019-04-01 21:00:00') and dt < to_days('2019-04-07 23:59:59')",
        iterations,
    );
}

/// 基准：RANGE COLUMNS 分区裁剪。
pub fn BenchmarkRangeColumnPartitionPruning(iterations: usize) {
    let _bench = prepareBenchSession();
    // Go 用 strings.Builder 从 2020-05-15 起逐日生成 1023 个 range columns 分区，最后 maxvalue。
    let ddl = build_range_columns_partition_ddl("2020-05-15", 1023);
    must_execute(&ddl);
    must_execute("analyze table t");
    run_drain_benchmark(
        "select * from t where dt > '2020-05-01' and dt < '2020-06-07'",
        iterations,
    );
}

/// 基准：HASH 分区等值裁剪。
pub fn BenchmarkHashPartitionPruningPointSelect(iterations: usize) {
    let _bench = prepareBenchSession();
    must_execute("create table t (id int, dt datetime) partition by hash(id) partitions 1024;");
    run_drain_benchmark("select * from t where id = 2330", iterations);
}

/// 基准：HASH 分区多谓词裁剪。
pub fn BenchmarkHashPartitionPruningMultiSelect(iterations: usize) {
    let _bench = prepareBenchSession();
    must_execute("create table t (id int, dt datetime) partition by hash(id) partitions 1024;");
    for _ in 0..iterations {
        // 同一次 benchmark 迭代依次覆盖等值、OR、多值 IN 三种 hash partition pruning 选择。
        drain_sql("select * from t where id = 2330");
        drain_sql("select * from t where id = 1233 or id = 1512");
        drain_sql("select * from t where id in (117, 1233, 15678)");
    }
}

/// 基准：INSERT INTO ... SELECT。
pub fn BenchmarkInsertIntoSelect(iterations: usize) {
    let _bench = prepareBenchSession();
    must_execute_many(&[
        "set @@tidb_enable_mutation_checker = 0",
        "set @@tmp_table_size = 1000000000",
        "create global temporary table tmp (id int, dt varchar(512)) on commit delete rows",
        "create table src (id int, dt varchar(512))",
    ]);
    prepare_batched_source(
        100,
        100,
        "insert into src values (42, repeat('x', 512)), (66, repeat('x', 512))",
    );
    run_statement_loop("insert into tmp select * from src", iterations);
}

/// 基准：宽表 insert-select 的 Compile 耗时。
pub fn BenchmarkCompileStmt(iterations: usize) {
    let _bench = prepareBenchSession();
    // See issue https://github.com/pingcap/tidb/issues/27633
    // Go 创建含 a..z2 大量列的 item1/item2，Prepare insert-select，再反复 executor.Compiler.Compile。
    must_execute(create_item1_ddl());
    must_execute("CREATE TABLE item2 like item1");
    run_compile_prepared_stmt(
        "insert into item2 select * from item1 where a1 = ?",
        "3401544",
        iterations,
    );
}

/// 基准：自增列插入。
pub fn BenchmarkAutoIncrement(iterations: usize) {
    let _bench = prepareBenchSession();
    must_execute_many(&[
        "create table auto_inc (id int unsigned key nonclustered auto_increment) shard_row_id_bits=4 auto_id_cache 1;",
        "set @@tidb_enable_mutation_checker = false",
    ]);
    run_statement_loop("insert into auto_inc values ()", iterations);
}

// TestBenchDaily collects the daily benchmark test result and generates a json output file.
// The format of the json output is described by the BenchOutput.
// Used by this command in the Makefile
//	make bench-daily TO=xxx.json
/// 注册每日 benchmark 列表（对应 make bench-daily）。
#[test]
pub fn TestBenchDaily() {
    // benchdaily.Run 按 Go 源文件顺序注册 daily benchmark；只保留注册列表，不产生 json。
    benchdaily_run(&[
        "BenchmarkPreparedPointGet",
        "BenchmarkPointGet",
        "BenchmarkBatchPointGet",
        "BenchmarkBasic",
        "BenchmarkTableScan",
        "BenchmarkTableLookup",
        "BenchmarkExplainTableLookup",
        "BenchmarkStringIndexScan",
        "BenchmarkExplainStringIndexScan",
        "BenchmarkStringIndexLookup",
        "BenchmarkIntegerIndexScan",
        "BenchmarkIntegerIndexLookup",
        "BenchmarkDecimalIndexScan",
        "BenchmarkDecimalIndexLookup",
        "BenchmarkInsertWithIndex",
        "BenchmarkInsertNoIndex",
        "BenchmarkSort",
        "BenchmarkJoin",
        "BenchmarkJoinLimit",
        "BenchmarkPartitionPruning",
        "BenchmarkRangeColumnPartitionPruning",
        "BenchmarkHashPartitionPruningPointSelect",
        "BenchmarkHashPartitionPruningMultiSelect",
        "BenchmarkInsertIntoSelect",
        "BenchmarkCompileStmt",
        "BenchmarkAutoIncrement",
    ]);
}

/// 基准：Pipelined 简单 INSERT SELECT。
pub fn BenchmarkPipelinedSimpleInsert(iterations: usize) {
    let _bench = prepareBenchSession();
    let _fp =
        astersql_testkit_testfailpoint::enable("tikvclient/pipelinedSkipResolveLock", "return");
    setup_pipelined_insert_source("tmp", "src", false);
    enable_bulk_dml("insert");
    run_pipelined_loop("insert into tmp select * from src", iterations);
}

/// 基准：Pipelined INSERT IGNORE。
pub fn BenchmarkPipelinedInsertIgnoreNoDuplicates(iterations: usize) {
    let _bench = prepareBenchSession();
    let _fp =
        astersql_testkit_testfailpoint::enable("tikvclient/pipelinedSkipResolveLock", "return");
    setup_pipelined_insert_source("tmp", "src", false);
    enable_bulk_dml("insert");
    run_pipelined_loop("insert ignore into tmp select * from src", iterations);
}

/// 基准：Pipelined ON DUPLICATE KEY UPDATE。
pub fn BenchmarkPipelinedInsertOnDuplicate(iterations: usize) {
    let _bench = prepareBenchSession();
    setup_pipelined_insert_source("tmp", "src", true);
    enable_bulk_dml("insert");
    run_pipelined_loop(
        "insert into tmp select * from src on duplicate key update dt = values(dt)",
        iterations,
    );
}

/// 基准：Pipelined DELETE。
pub fn BenchmarkPipelinedDelete(iterations: usize) {
    let _bench = prepareBenchSession();
    setup_pipelined_insert_source("tmp", "src", false);
    enable_bulk_dml("delete");
    let mut elapsed = std::time::Duration::ZERO;
    for _ in 0..iterations {
        // Go 在计时外 truncate/insert 回填 tmp，在计时内 delete from tmp。
        execute_outside_timer("truncate tmp");
        execute_outside_timer("insert into tmp select * from src");
        elapsed += execute_inside_timer("delete from tmp");
    }
    report_ns_per_row(elapsed, iterations);
}

/// 基准：Pipelined REPLACE。
pub fn BenchmarkPipelinedReplaceNoDuplicates(iterations: usize) {
    let _bench = prepareBenchSession();
    let _fp =
        astersql_testkit_testfailpoint::enable("tikvclient/pipelinedSkipResolveLock", "return");
    setup_pipelined_insert_source("tmp", "src", false);
    enable_bulk_dml("insert");
    run_pipelined_loop("replace into tmp select * from src", iterations);
}

/// 基准：Pipelined UPDATE。
pub fn BenchmarkPipelinedUpdate(iterations: usize) {
    let _bench = prepareBenchSession();
    must_execute("create table src (id int, dt varchar(128))");
    prepare_batched_source(
        unsafe { batchNum },
        unsafe { batchSize },
        "insert into src values (42, repeat('x', 128))",
    );
    enable_bulk_dml("update");
    let mut elapsed = std::time::Duration::ZERO;
    for i in 0..iterations {
        // Go 按奇偶交替 concat y/z，避免每轮更新成完全相同的值。
        if i % 2 == 0 {
            elapsed += execute_inside_timer("update src set dt = left(concat('y', dt), 128)");
        } else {
            elapsed += execute_inside_timer("update src set dt = left(concat('z', dt), 128)");
        }
    }
    report_ns_per_row(elapsed, iterations);
}

/// Benchmark 会话：持有真实 mock-store domain 和 canonical session。
struct BenchSession {
    session: Arc<ConcreteSession>,
    _domain: Arc<astersql_domain::Domain>,
}
/// 真实 canonical session 结果集。
type BenchmarkRecordSet = Box<dyn TestRecordSet>;
impl Drop for BenchSession {
    fn drop(&mut self) {
        ACTIVE_BENCH_SESSION.with(|active| {
            let should_clear = active
                .borrow()
                .as_ref()
                .is_some_and(|session| Arc::ptr_eq(session, &self.session));
            if should_clear {
                active.replace(None);
            }
        });
    }
}

fn with_session<T>(operation: impl FnOnce(&ConcreteSession) -> T) -> T {
    ACTIVE_BENCH_SESSION.with(|active| {
        let active = active.borrow();
        let session = active
            .as_ref()
            .expect("prepareBenchSession must be called before benchmark work");
        operation(session)
    })
}

fn format_bench_value(pattern: &str, value: i32) -> String {
    pattern
        .replace("%v", &value.to_string())
        .replace("%d", &value.to_string())
}

fn prepare_values(ddl_template: &str, col_type: &str, value_format: &str, value_count: i32) {
    prepare_values_with_order(ddl_template, col_type, value_format, value_count, false);
}

fn prepare_values_with_order(
    ddl_template: &str,
    col_type: &str,
    value_format: &str,
    value_count: i32,
    randomized: bool,
) {
    must_execute("drop table if exists t");
    must_execute(&ddl_template.replace("{col_type}", col_type));
    must_execute("begin");
    let mut random_state = 0x9e37_79b9_u64;
    for i in 0..value_count {
        if randomized && i % 1000 == 0 {
            must_execute("commit");
            must_execute("begin");
        }
        let value = if randomized && value_count > 0 {
            random_state = random_state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            (random_state % value_count as u64) as i32
        } else {
            i
        };
        must_execute(&format!(
            "insert t (col) values ({})",
            format_bench_value(value_format, value)
        ));
    }
    must_execute("commit");
}

fn execute_and_drain(sql: &str) -> Vec<Vec<String>> {
    with_session(|session| {
        let mut record_sets = session
            .Execute(sql)
            .unwrap_or_else(|error| panic!("execute {sql}: {error}"));
        let mut rows = Vec::new();
        for mut record_set in record_sets.drain(..) {
            while let Some(row) = record_set
                .Next()
                .unwrap_or_else(|error| panic!("read {sql}: {error}"))
            {
                rows.push(row);
            }
            record_set
                .Close()
                .unwrap_or_else(|error| panic!("close {sql}: {error}"));
        }
        rows
    })
}

fn run_query_benchmark(sql: &str, rows: i32, iterations: usize) {
    for _ in 0..iterations {
        let record_set = with_session(|session| {
            session
                .Execute(sql)
                .unwrap_or_else(|error| panic!("execute {sql}: {error}"))
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("execute {sql} returned no record set"))
        });
        readResult(record_set, rows);
    }
}

fn run_drain_benchmark(sql: &str, iterations: usize) {
    for _ in 0..iterations {
        let _ = execute_and_drain(sql);
    }
}

fn run_prepared_drain_benchmark(sql: &str, arg: &str, iterations: usize) {
    let statement_id = with_session(|session| {
        session
            .PrepareStmt(sql)
            .unwrap_or_else(|error| panic!("prepare {sql}: {error}"))
    });
    for _ in 0..iterations {
        with_session(|session| {
            if let Some(mut record_set) = session
                .ExecutePreparedStmt(statement_id, &[arg.to_owned()])
                .unwrap_or_else(|error| panic!("execute prepared {sql}: {error}"))
            {
                while record_set
                    .Next()
                    .unwrap_or_else(|error| panic!("read prepared {sql}: {error}"))
                    .is_some()
                {}
                record_set
                    .Close()
                    .unwrap_or_else(|error| panic!("close prepared {sql}: {error}"));
            }
        });
    }
}

fn run_insert_loop(sql_template: &str, iterations: usize) {
    for i in 0..iterations {
        must_execute(&sql_template.replace("{i}", &i.to_string()));
    }
}

fn run_statement_loop(sql: &str, iterations: usize) {
    for _ in 0..iterations {
        must_execute(sql);
    }
}

fn run_compile_prepared_stmt(sql: &str, arg: &str, iterations: usize) {
    for _ in 0..iterations {
        let statement_id = with_session(|session| {
            session
                .PrepareStmt(sql)
                .unwrap_or_else(|error| panic!("prepare {sql}: {error}"))
        });
        with_session(|session| {
            if let Some(mut record_set) = session
                .ExecutePreparedStmt(statement_id, &[arg.to_owned()])
                .unwrap_or_else(|error| panic!("compile/execute {sql}: {error}"))
            {
                record_set
                    .Close()
                    .unwrap_or_else(|error| panic!("close compiled {sql}: {error}"));
            }
        });
    }
}

fn run_pipelined_loop(sql: &str, iterations: usize) {
    let started = std::time::Instant::now();
    run_statement_loop(sql, iterations);
    report_ns_per_row(started.elapsed(), iterations);
}

fn drain_sql(sql: &str) {
    let _ = execute_and_drain(sql);
}

fn must_execute(sql: &str) {
    let _ = execute_and_drain(sql);
}
/// 依次 must_execute 多条 SQL。
fn must_execute_many(sqls: &[&str]) {
    for sql in sqls {
        must_execute(sql);
    }
}
/// 在 benchmark 计时区间外执行 SQL。
fn execute_outside_timer(sql: &str) {
    must_execute(sql);
}
/// 在 benchmark 计时区间内执行 SQL。
fn execute_inside_timer(sql: &str) -> std::time::Duration {
    let started = std::time::Instant::now();
    must_execute(sql);
    started.elapsed()
}

fn report_ns_per_row(elapsed: std::time::Duration, iterations: usize) {
    let rows = iterations
        .saturating_mul(unsafe { batchSize as usize })
        .saturating_mul(unsafe { batchNum as usize });
    if rows > 0 {
        std::hint::black_box(elapsed.as_nanos() / rows as u128);
    }
}

macro_rules! daily_adapter {
    ($adapter:ident, $benchmark:ident) => {
        fn $adapter(benchmark: &mut astersql_util_benchdaily::Benchmark) {
            $benchmark(benchmark.iterations() as usize);
        }
    };
}

daily_adapter!(daily_prepared_point_get, BenchmarkPreparedPointGet);
daily_adapter!(daily_point_get, BenchmarkPointGet);
daily_adapter!(daily_batch_point_get, BenchmarkBatchPointGet);
daily_adapter!(daily_basic, BenchmarkBasic);
daily_adapter!(daily_table_scan, BenchmarkTableScan);
daily_adapter!(daily_table_lookup, BenchmarkTableLookup);
daily_adapter!(daily_explain_table_lookup, BenchmarkExplainTableLookup);
daily_adapter!(daily_string_index_scan, BenchmarkStringIndexScan);
daily_adapter!(
    daily_explain_string_index_scan,
    BenchmarkExplainStringIndexScan
);
daily_adapter!(daily_string_index_lookup, BenchmarkStringIndexLookup);
daily_adapter!(daily_integer_index_scan, BenchmarkIntegerIndexScan);
daily_adapter!(daily_integer_index_lookup, BenchmarkIntegerIndexLookup);
daily_adapter!(daily_decimal_index_scan, BenchmarkDecimalIndexScan);
daily_adapter!(daily_decimal_index_lookup, BenchmarkDecimalIndexLookup);
daily_adapter!(daily_insert_with_index, BenchmarkInsertWithIndex);
daily_adapter!(daily_insert_no_index, BenchmarkInsertNoIndex);
daily_adapter!(daily_sort, BenchmarkSort);
daily_adapter!(daily_join, BenchmarkJoin);
daily_adapter!(daily_join_limit, BenchmarkJoinLimit);
daily_adapter!(daily_partition_pruning, BenchmarkPartitionPruning);
daily_adapter!(daily_range_columns, BenchmarkRangeColumnPartitionPruning);
daily_adapter!(daily_hash_point, BenchmarkHashPartitionPruningPointSelect);
daily_adapter!(daily_hash_multi, BenchmarkHashPartitionPruningMultiSelect);
daily_adapter!(daily_insert_select, BenchmarkInsertIntoSelect);
daily_adapter!(daily_compile_stmt, BenchmarkCompileStmt);
daily_adapter!(daily_auto_increment, BenchmarkAutoIncrement);

fn benchdaily_run(_names: &[&str]) {
    astersql_util_benchdaily::Run(vec![
        daily_prepared_point_get,
        daily_point_get,
        daily_batch_point_get,
        daily_basic,
        daily_table_scan,
        daily_table_lookup,
        daily_explain_table_lookup,
        daily_string_index_scan,
        daily_explain_string_index_scan,
        daily_string_index_lookup,
        daily_integer_index_scan,
        daily_integer_index_lookup,
        daily_decimal_index_scan,
        daily_decimal_index_lookup,
        daily_insert_with_index,
        daily_insert_no_index,
        daily_sort,
        daily_join,
        daily_join_limit,
        daily_partition_pruning,
        daily_range_columns,
        daily_hash_point,
        daily_hash_multi,
        daily_insert_select,
        daily_compile_stmt,
        daily_auto_increment,
    ]);
}

/// 生成 RANGE(to_days) 分区 DDL。
fn build_range_partition_ddl(prefix: &str, first: i32, last: i32, first_less_than: i32) -> String {
    let mut ddl = format!("{prefix} (\n");
    for p in first..=last {
        // 与 Go 源文件的 pN values less than (737515+N) 保持同一边界。
        ddl.push_str(&format!(
            "partition p{} values less than ({}){}\n",
            p,
            first_less_than + (p - first),
            if p == last { "" } else { "," },
        ));
    }
    ddl.push(')');
    ddl
}

/// 生成 RANGE COLUMNS 分区 DDL。
fn build_range_columns_partition_ddl(start_date: &str, partitions: i32) -> String {
    let mut ddl =
        String::from("create table t (id int, dt date) partition by range columns (dt) (");
    let mut boundary = chrono::NaiveDate::parse_from_str(start_date, "%Y-%m-%d")
        .expect("benchmark partition start date must use YYYY-MM-DD");
    for i in 0..partitions {
        boundary = boundary
            .checked_add_days(chrono::Days::new(1))
            .expect("benchmark partition date must remain representable");
        ddl.push_str(&format!(
            "partition p{} values less than ('{}'),\n",
            i,
            boundary.format("%Y-%m-%d"),
        ));
    }
    ddl.push_str(&format!(
        "partition p{partitions} values less than maxvalue)"
    ));
    ddl
}

/// 返回宽表 item1 的建表 DDL（复现 issue 27633）。
fn create_item1_ddl() -> &'static str {
    // 原 Go DDL 覆盖 a..z2 多个 varchar/bigint/decimal/datetime 列，用于复现 issue 27633 的编译耗时。
    "CREATE TABLE item1 (a varchar(200) DEFAULT NULL, b varchar(480) DEFAULT NULL, c varchar(200) DEFAULT NULL, d varchar(200) DEFAULT NULL, e varchar(200) DEFAULT NULL, f varchar(200) DEFAULT NULL, g varchar(3999) DEFAULT NULL, h bigint(38) DEFAULT NULL, i varchar(80) DEFAULT NULL, j bigint(38) DEFAULT NULL, k varchar(480) DEFAULT NULL, l varchar(480) DEFAULT NULL, m decimal(18,4) DEFAULT NULL, n decimal(18,4) DEFAULT NULL, o decimal(22,8) DEFAULT NULL, p varchar(8) DEFAULT NULL, q decimal(18,4) DEFAULT NULL, r decimal(18,4) DEFAULT NULL, s varchar(40) DEFAULT NULL, t decimal(18,4) DEFAULT NULL, u decimal(18,4) DEFAULT NULL, v decimal(18,4) DEFAULT NULL, w decimal(18,5) DEFAULT NULL, x decimal(12,8) DEFAULT NULL, y varchar(40) DEFAULT NULL, z decimal(12,8) DEFAULT NULL, a1 decimal(18,4) DEFAULT NULL, b1 decimal(18,4) DEFAULT NULL, c1 decimal(18,4) DEFAULT NULL, d1 decimal(18,4) DEFAULT NULL, e1 decimal(12,8) DEFAULT NULL, f1 varchar(40) DEFAULT NULL, g1 decimal(18,4) DEFAULT NULL, h1 decimal(18,4) DEFAULT NULL, i1 decimal(18,4) DEFAULT NULL, j1 decimal(18,4) DEFAULT NULL, k1 varchar(40) DEFAULT NULL, l1 decimal(14,8) DEFAULT NULL, m1 bigint(38) DEFAULT NULL, n1 varchar(8) DEFAULT NULL, o1 varchar(40) DEFAULT NULL, p1 decimal(12,8) DEFAULT NULL, q1 varchar(480) DEFAULT NULL, r1 varchar(480) DEFAULT NULL, s1 decimal(12,8) DEFAULT NULL, t1 decimal(14,10) DEFAULT NULL, u1 decimal(18,4) DEFAULT NULL, v1 decimal(18,4) DEFAULT NULL, w1 varchar(8) DEFAULT NULL, x1 decimal(18,4) DEFAULT NULL, y1 datetime DEFAULT NULL, z1 datetime DEFAULT NULL, a2 decimal(18,4) DEFAULT NULL, b2 decimal(18,4) DEFAULT NULL, c2 decimal(18,4) DEFAULT NULL, d2 decimal(18,4) DEFAULT NULL, e2 decimal(12,8) DEFAULT NULL, f2 varchar(40) DEFAULT NULL, g2 decimal(18,4) DEFAULT NULL, h2 decimal(18,4) DEFAULT NULL, i2 decimal(18,4) DEFAULT NULL, j2 decimal(18,4) DEFAULT NULL, k2 varchar(40) DEFAULT NULL, l2 decimal(14,8) DEFAULT NULL, m2 bigint(38) DEFAULT NULL, n2 varchar(8) DEFAULT NULL, o2 varchar(40) DEFAULT NULL, p2 decimal(12,8) DEFAULT NULL, q2 varchar(480) DEFAULT NULL, r2 varchar(480) DEFAULT NULL, s2 decimal(12,8) DEFAULT NULL, t2 decimal(14,10) DEFAULT NULL, u2 decimal(18,4) DEFAULT NULL, v2 decimal(18,4) DEFAULT NULL, w2 varchar(8) DEFAULT NULL, x2 decimal(18,4) DEFAULT NULL, y2 datetime DEFAULT NULL, z2 datetime DEFAULT NULL)"
}

/// 为 pipelined insert 基准准备 tmp/src 表与批量数据。
fn setup_pipelined_insert_source(tmp: &str, src: &str, unique_tmp: bool) {
    let tmp_ddl = if unique_tmp {
        format!("create table {tmp} (id int, dt varchar(512), unique key k1(id))")
    } else {
        format!("create table {tmp} (id int, dt varchar(512))")
    };
    must_execute(&tmp_ddl);
    must_execute(&format!("create table {} (id int, dt varchar(512))", src));
    let insert_sql = if unique_tmp {
        "insert into src values ({generated_id}, repeat('x', 512))"
    } else {
        "insert into src values (42, repeat('x', 512))"
    };
    prepare_batched_source(unsafe { batchNum }, unsafe { batchSize }, insert_sql);
}

/// 按 batch 显式事务插入源表数据。
fn prepare_batched_source(batch_num: i32, batch_size: i32, insert_sql: &str) {
    for batch in 0..batch_num {
        must_execute("begin");
        for line in 0..batch_size {
            let generated_id = batch.saturating_mul(batch_size).saturating_add(line);
            must_execute(&insert_sql.replace("{generated_id}", &generated_id.to_string()));
        }
        must_execute("commit");
    }
}

/// 启用 Bulk DML；语句类型由真实 parser/dispatch 路径写入 StmtCtx。
fn enable_bulk_dml(kind: &str) {
    assert!(matches!(kind, "insert" | "delete" | "update"));
    must_execute("set @@tidb_dml_type = 'bulk'");
}

/// 校验自增分配器在显式 ID 后能 rebase 并继续递增。
#[test]
fn canonical_auto_id_allocator_rebases_explicit_ids_and_advances() {
    let mut table =
        crate::dml_runtime::RelationalTableState::New(astersql_meta_model::TableInfo::default());
    assert_eq!(table.AllocateAutoID(None).unwrap(), 1);
    assert_eq!(table.AllocateAutoID(Some(20)).unwrap(), 20);
    assert_eq!(table.AllocateAutoID(None).unwrap(), 21);
    assert_eq!(table.Info.AutoIncID, 22);
}

#[test]
fn range_partition_ddl_matches_go_partition_boundaries() {
    let ddl = build_range_partition_ddl(
        "create table t (id int, dt datetime)\npartition by range (to_days(dt))",
        0,
        1023,
        737515,
    );

    assert!(ddl.starts_with(
        "create table t (id int, dt datetime)\npartition by range (to_days(dt)) (\n\
         partition p0 values less than (737515),\n"
    ));
    assert!(ddl.ends_with("partition p1023 values less than (738538)\n)"));
    assert!(!ddl.ends_with(",\n)"));
}

#[test]
fn range_columns_partition_ddl_matches_go_daily_boundaries() {
    let ddl = build_range_columns_partition_ddl("2020-05-15", 1023);

    assert!(ddl.contains("partition p0 values less than ('2020-05-16'),\n"));
    assert!(ddl.contains("partition p1 values less than ('2020-05-17'),\n"));
    assert!(ddl.ends_with("partition p1023 values less than maxvalue)"));
    assert!(!ddl.contains("generated-date-"));
}

#[test]
fn canonical_benchmark_helpers_execute_real_session_sql() {
    let _bench = prepareBenchSession();
    prepareBenchData("int", "%v", 3);
    assert_eq!(execute_and_drain("select col from t order by col").len(), 3);

    run_prepared_drain_benchmark("select * from t where col = ?", "1", 1);
    must_execute("create table src (id int, dt varchar(8))");
    prepare_batched_source(2, 2, "insert into src values ({generated_id}, 'x')");
    assert_eq!(execute_and_drain("select * from src").len(), 4);

    enable_bulk_dml("insert");
    must_execute("create table bulk_target (id int primary key)");
    must_execute("insert into bulk_target values (1)");
    assert_eq!(
        execute_and_drain("select * from bulk_target"),
        vec![vec!["1".to_owned()]]
    );
}

#[test]
fn benchmark_entrypoints_execute_canonical_paths() {
    BenchmarkBasic(1);
    BenchmarkPointGet(1);
}
