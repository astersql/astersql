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

//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/snap_client/client_test.rs`对应的SnapClient 建表/集群新鲜度/限速等测试，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go `br/pkg/restore/snap_client/client_test.go` 的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 本任务要求至少113行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `RecordingImporter`：记录 SetDownloadSpeedLimit 等调用，验证限速与 Close 回调。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `decode_table_id/mk_table`：测试辅助：构造带 ID/名称的假 TableInfo。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `generate_metautil_table`：生成 metautil::Table，驱动 CreateTables/AllocTableIDs。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `create_tables_test`：封装 CreateTables 场景公共准备（domain、session、表列表）。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_create_tables`：建表成功：RewriteRules 与新表 ID 映射正确。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_need_check_target_cluster_fresh`：判定是否需要检查目标集群“干净”的条件组合。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_check_target_cluster_fresh`：空集群通过；已有用户对象时失败。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_create_duplicate_database_for_one_session`：单 session 重复建库应可幂等或返回预期错误。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_create_duplicate_database_for_session_pool`：session 池路径下重复建库行为与单 session 对齐。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_check_target_cluster_fresh_with_table`：已有用户表时 freshness 检查失败。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_init_full_cluster_restore`：全量集群恢复标志与系统库处理开关。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_set_speed_limit`：SetSpeedLimit 回调对每 store 下发限速值。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_set_speed_limit_close_callback_idempotent`：Close 回调多次调用仍安全，限速清零只生效一次语义。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_sort_tables_by_schema_id`：按 schema ID 排序保证建表依赖顺序。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_alloc_table_ids`：预分配区间连续且覆盖所有物理表/分区。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_get_min_user_table_id`：最小用户表 ID 边界，避开系统 ID 段。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `CreateTablesTestExt`：扩展 trait：测试侧暴露内部建表钩子。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 中文注释索引结束

//! Go-equivalent tests for `client_test.go`.
//! Domain/PD/TiKV boundaries: MemDomain/MemPdClient/MemImporterClient (no kv/domain).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::client::{
    IGNORE_PLACEMENT_POLICY_MODE, NewRestoreClient, NewRestoreClientForTest,
    SetSpeedLimitCallbacks, SortTablesBySchemaID, getMinUserTableID, makeDBPool, needLoadSchemas,
};
use crate::export_test::MockCallSetSpeedLimit;
use crate::import::{KvMode, NewSnapFileImporter, NewSnapFileImporterOptions, RewriteMode};
use crate::pitr_collector::PiTRCollDep;
use crate::stubs::{
    BatchBackupFileSet, CheckpointMetadata, CheckpointRunner, ChecksumClient, ChecksumItem,
    ClusterConfig, CollectTableIDs, ComputeSortedIDsHash, Context, CreatedTable, DbSession, Error,
    ExternalStorage, ImporterClient, MemDb, MemDomain, MemImporterClient, MemPdClient,
    MemSplitClient, MemStorage, MergeOptionRule, PdClient, PdController, PreallocIDs, Result,
    SnapshotCheckpointManager, SstRestorer, StoreMeta, TemporaryDBName, TlsConfig, UniqueTableName,
    backuppb, import_sstpb, metapb, metautil, model, tablecodec,
};

fn decode_table_id(key: &[u8]) -> i64 {
    assert!(key.len() >= 9 && key[0] == b't');
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&key[1..9]);
    (u64::from_be_bytes(buf) ^ (1u64 << 63)) as i64
}

fn mk_table(db: &str, db_id: i64, name: &str, id: i64) -> metautil::Table {
    metautil::Table {
        DB: model::DBInfo {
            ID: db_id,
            Name: model::CIStr::new(db),
        },
        Info: model::TableInfo {
            ID: id,
            Name: model::CIStr::new(name),
            Columns: vec![model::ColumnInfo {
                Name: model::CIStr::new("id"),
                ..Default::default()
            }],
            ..Default::default()
        },
        FilesOfPhysicals: HashMap::new(),
        ..Default::default()
    }
}

fn generate_metautil_table(db_name: &str, table_id: i64, partition_ids: &[i64]) -> metautil::Table {
    generate_metautil_table_with_name(db_name, "", table_id, partition_ids)
}

fn generate_metautil_table_with_name(
    db_name: &str,
    table_name: &str,
    table_id: i64,
    partition_ids: &[i64],
) -> metautil::Table {
    let partition = if partition_ids.is_empty() {
        None
    } else {
        Some(model::PartitionInfo {
            Definitions: partition_ids
                .iter()
                .map(|id| model::PartitionDefinition {
                    ID: *id,
                    Name: model::CIStr::new(format!("p{id}")),
                })
                .collect(),
        })
    };
    metautil::Table {
        DB: model::DBInfo {
            Name: model::CIStr::new(db_name),
            ..Default::default()
        },
        Info: model::TableInfo {
            ID: table_id,
            Name: model::CIStr::new(table_name),
            Partition: partition,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Extension trait so sibling modules can call CreateTablesTest from export_test impl.
trait CreateTablesTestExt {
    fn create_tables_test(
        &mut self,
        dom: Arc<dyn crate::stubs::DomainLike>,
        tables: &[metautil::Table],
        new_ts: u64,
    ) -> Result<(crate::stubs::RewriteRules, Vec<model::TableInfo>)>;
}

impl CreateTablesTestExt for crate::client::SnapClient {
    fn create_tables_test(
        &mut self,
        dom: Arc<dyn crate::stubs::DomainLike>,
        tables: &[metautil::Table],
        new_ts: u64,
    ) -> Result<(crate::stubs::RewriteRules, Vec<model::TableInfo>)> {
        self.CreateTablesTest(dom, tables, new_ts)
    }
}

/// TestCreateTables — Go `TestCreateTables`.
#[test]
// 断言新建 TableInfo.ID 来自预分配，且 RewriteRules.Data 非空。
fn test_create_tables() {
    let mut client = NewRestoreClientForTest();
    let dom = Arc::new(MemDomain {
        empty: true,
        tables: Mutex::new(HashMap::new()),
    });
    // Downstream allocates new IDs = old + 100 for visible rewrite differences.
    {
        let mut t = dom.tables.lock().unwrap();
        for i in 0..4 {
            t.insert(
                ("test".into(), format!("test{i}")),
                model::TableInfo {
                    ID: 100 + i,
                    Name: model::CIStr::new(format!("test{i}")),
                    ..Default::default()
                },
            );
        }
    }
    client
        .InitConnections(dom.clone(), Box::new(MemDb::default()))
        .unwrap();
    client.SetBatchDdlSize(1);

    // Go: for i := len-1; i >= 0; i-- { tables[i] = test{i} }
    let mut tables = vec![None; 4];
    for i in (0..4).rev() {
        tables[i] = Some(mk_table("test", 1, &format!("test{i}"), i as i64));
    }
    let tables: Vec<_> = tables.into_iter().map(|t| t.unwrap()).collect();

    let (rules, new_tables) = client
        .create_tables_test(dom, &tables, 0)
        .expect("CreateTablesTest");
    for (i, tbl) in tables.iter().enumerate() {
        assert_eq!(tbl.Info.Name.O, new_tables[i].Name.O);
    }
    let mut old_exist = HashMap::new();
    let mut new_exist = HashMap::new();
    for tr in &rules.Data {
        let old_id = decode_table_id(tr.GetOldKeyPrefix());
        assert!(!old_exist.contains_key(&old_id), "duplicate old id");
        old_exist.insert(old_id, true);
        let new_id = decode_table_id(tr.GetNewKeyPrefix());
        assert!(!new_exist.contains_key(&new_id), "duplicate new id");
        new_exist.insert(new_id, true);
    }
    for i in 0..tables.len() {
        assert!(old_exist.contains_key(&(i as i64)), "missing rule for {i}");
    }
}

struct RecordingCreateDb {
    batch_calls: Arc<AtomicUsize>,
    single_calls: Arc<AtomicUsize>,
    fail_batch: bool,
    delay: Duration,
}

impl DbSession for RecordingCreateDb {
    fn Execute(&mut self, _ctx: &Context, _sql: &str) -> Result<()> {
        Ok(())
    }

    fn ExecDDL(&mut self, _ctx: &Context, _job: &model::Job) -> Result<()> {
        Ok(())
    }

    fn RegisterPreallocatedIDs(&mut self, _ids: &PreallocIDs) {}

    fn CreateTable(
        &mut self,
        _ctx: &Context,
        _table: &metautil::Table,
        _rebased: &HashMap<UniqueTableName, bool>,
        _support_policy: bool,
    ) -> Result<()> {
        self.single_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn CreateTables(
        &mut self,
        _ctx: &Context,
        _tables: &[metautil::Table],
        _rebased: &HashMap<UniqueTableName, bool>,
        _support_policy: bool,
    ) -> Result<()> {
        self.batch_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_batch {
            return Err(Error::with_code(
                "BR:Restore:ErrUnsupportedBatchDDL",
                "unsupported batch create table",
            ));
        }
        std::thread::sleep(self.delay);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingMergeDomain {
    tables: Mutex<HashMap<(String, String), model::TableInfo>>,
    rules: Mutex<Vec<MergeOptionRule>>,
}

impl crate::stubs::DomainLike for RecordingMergeDomain {
    fn AssertUserDBsEmpty(&self) -> Result<()> {
        Ok(())
    }

    fn TableInfoByName(&self, schema: &str, table: &str) -> Result<model::TableInfo> {
        self.tables
            .lock()
            .unwrap()
            .get(&(schema.to_string(), table.to_string()))
            .cloned()
            .ok_or_else(|| Error::new("table not found"))
    }

    fn UpdateMergeOptionRules(&self, _ctx: &Context, rules: &[MergeOptionRule]) -> Result<()> {
        self.rules.lock().unwrap().extend_from_slice(rules);
        Ok(())
    }
}

#[test]
fn test_create_tables_batch_pool_fallback_and_merge_options_match_go() {
    let domain = Arc::new(RecordingMergeDomain::default());
    let mut tables = Vec::new();
    for id in 1..=4 {
        let mut table = mk_table("test", 1, &format!("t{id}"), id);
        table.IsMergeOptionAllowed = id == 1;
        if id == 1 {
            table.Info.Partition = Some(model::PartitionInfo {
                Definitions: vec![model::PartitionDefinition {
                    ID: 11,
                    Name: model::CIStr::new("P0"),
                }],
            });
            table.PartitionMergeOptionAllowed.insert("P0".into(), true);
        }
        let mut downstream = table.Info.clone();
        downstream.ID += 100;
        if let Some(partition) = &mut downstream.Partition {
            partition.Definitions[0].ID += 100;
        }
        domain
            .tables
            .lock()
            .unwrap()
            .insert(("test".into(), format!("t{id}")), downstream);
        tables.push(table);
    }

    let batch_calls = Arc::new(AtomicUsize::new(0));
    let single_calls = Arc::new(AtomicUsize::new(0));
    let mut client = NewRestoreClientForTest();
    client
        .InitConnections(domain.clone(), Box::new(MemDb::default()))
        .unwrap();
    client.SetBatchDdlSize(2);
    for _ in 0..2 {
        client.dbPool.push(Box::new(RecordingCreateDb {
            batch_calls: batch_calls.clone(),
            single_calls: single_calls.clone(),
            fail_batch: false,
            delay: Duration::from_millis(150),
        }));
    }
    let start = Instant::now();
    let created = client
        .CreateTables(&Context::Background(), &tables, 7)
        .unwrap();
    assert_eq!(created.len(), 4);
    assert!(start.elapsed() < Duration::from_millis(270));
    assert_eq!(batch_calls.load(Ordering::SeqCst), 2);
    assert_eq!(single_calls.load(Ordering::SeqCst), 0);
    let rules = domain.rules.lock().unwrap();
    assert_eq!(rules.len(), 2);
    assert!(rules.iter().any(|rule| rule.PhysicalID == 101));
    assert!(rules.iter().any(|rule| rule.PhysicalID == 111));
    drop(rules);

    let fallback_batch_calls = Arc::new(AtomicUsize::new(0));
    let fallback_single_calls = Arc::new(AtomicUsize::new(0));
    client.dbPool.clear();
    for _ in 0..2 {
        client.dbPool.push(Box::new(RecordingCreateDb {
            batch_calls: fallback_batch_calls.clone(),
            single_calls: fallback_single_calls.clone(),
            fail_batch: true,
            delay: Duration::ZERO,
        }));
    }
    client
        .CreateTables(&Context::Background(), &tables, 7)
        .unwrap();
    assert!(fallback_batch_calls.load(Ordering::SeqCst) >= 1);
    assert_eq!(fallback_single_calls.load(Ordering::SeqCst), 4);
}

/// TestNeedCheckTargetClusterFresh — Go same (incremental via backupMeta, not failpoint).
#[test]
// 全量恢复/过滤模式组合决定是否跳过 freshness 检查。
fn test_need_check_target_cluster_fresh() {
    let client = NewRestoreClientForTest();
    assert!(client.NeedCheckFreshCluster(false, false));
    assert!(!client.NeedCheckFreshCluster(false, true));
    assert!(!client.NeedCheckFreshCluster(true, true));
    assert!(!client.NeedCheckFreshCluster(true, false));

    let mut incr = NewRestoreClientForTest();
    incr.backupMeta = Some(backuppb::BackupMeta {
        StartVersion: 1,
        EndVersion: 2,
        ..Default::default()
    });
    assert!(!incr.NeedCheckFreshCluster(false, false));
}

/// TestCheckTargetClusterFresh — Go same.
#[test]
// 目标已有非系统库时必须拒绝，防止误覆盖。
fn test_check_target_cluster_fresh() {
    let mut client = NewRestoreClientForTest();
    let empty = Arc::new(MemDomain {
        empty: true,
        ..Default::default()
    });
    client
        .InitConnections(empty, Box::new(MemDb::default()))
        .unwrap();
    client.EnsureNoUserTables().unwrap();

    let mut client2 = NewRestoreClientForTest();
    let nonempty = Arc::new(MemDomain {
        empty: false,
        ..Default::default()
    });
    client2
        .InitConnections(nonempty, Box::new(MemDb::default()))
        .unwrap();
    let err = client2.EnsureNoUserTables().unwrap_err();
    assert_eq!(err.code, Some("BR:Restore:ErrRestoreNotFreshCluster"));
}

/// TestCreateDuplicateDatabaseForOneSession — Go session-factory failure + reuse tracking.
#[test]
// 重复 CREATE DATABASE 的错误码/幂等语义需与 Go glue 一致。
fn test_create_duplicate_database_for_one_session() {
    let counter = Arc::new(Mutex::new(0));
    let c = counter.clone();
    let pooled = makeDBPool(2, || {
        let mut n = c.lock().unwrap();
        if *n == 0 {
            *n = 1;
            Ok(Box::new(MemDb::default()) as Box<dyn DbSession>)
        } else {
            Err(Error::new("session already created"))
        }
    });
    let (partial_pool, err) = pooled;
    assert_eq!(partial_pool.len(), 1);
    let err = err.expect("expected session already created");
    assert_eq!(err.msg, "session already created");

    let mut client = NewRestoreClientForTest();
    let ctx = Context::Background();
    let db = metautil::Database {
        Info: model::DBInfo {
            Name: model::CIStr::new("user_db"),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(!db.IsReusedByPITR());
    client.CreateDatabases(&ctx, &[db.clone()]).unwrap();
    assert!(!db.IsReusedByPITR());
    client.CreateDatabases(&ctx, &[db.clone()]).unwrap();
    assert!(db.IsReusedByPITR());
}

/// TestCreateDuplicateDatabaseForSessionPool — Go session-pool path.
#[test]
fn test_create_duplicate_database_for_session_pool() {
    let mut client = NewRestoreClientForTest();
    client.dbPool = vec![Box::new(MemDb::default()), Box::new(MemDb::default())];
    let ctx = Context::Background();
    let mk = |n: &str| metautil::Database {
        Info: model::DBInfo {
            Name: model::CIStr::new(n),
            ..Default::default()
        },
        ..Default::default()
    };
    let db1 = mk("user_db_1");
    let db2 = mk("user_db_2");
    let db3 = mk("user_db_3");
    client
        .CreateDatabases(&ctx, &[db1.clone(), db2.clone(), db3.clone()])
        .unwrap();
    assert!(!db1.IsReusedByPITR());
    assert!(!db2.IsReusedByPITR());
    assert!(!db3.IsReusedByPITR());
    let db4 = mk("user_db_4");
    client
        .CreateDatabases(&ctx, &[db1.clone(), db2.clone(), db3.clone(), db4.clone()])
        .unwrap();
    assert!(db1.IsReusedByPITR());
    assert!(db2.IsReusedByPITR());
    assert!(db3.IsReusedByPITR());
    assert!(!db4.IsReusedByPITR());
}

/// TestCheckTargetClusterFreshWithTable — Go same.
#[test]
fn test_check_target_cluster_fresh_with_table() {
    let mut client = NewRestoreClientForTest();
    let dom = Arc::new(MemDomain {
        empty: true,
        tables: Mutex::new(HashMap::from([(
            ("test".into(), "t".into()),
            model::TableInfo {
                ID: 50,
                Name: model::CIStr::new("t"),
                ..Default::default()
            },
        )])),
    });
    client
        .InitConnections(dom.clone(), Box::new(MemDb::default()))
        .unwrap();
    let table = mk_table("test", 1, "t", 1);
    client.create_tables_test(dom, &[table], 0).unwrap();
    // After create, domain still empty flag unless flipped — Go EnsureNoUserTables fails after table.
    let mut client2 = NewRestoreClientForTest();
    let dom2 = Arc::new(MemDomain {
        empty: false,
        ..Default::default()
    });
    client2
        .InitConnections(dom2, Box::new(MemDb::default()))
        .unwrap();
    assert_eq!(
        client2.EnsureNoUserTables().unwrap_err().code,
        Some("BR:Restore:ErrRestoreNotFreshCluster")
    );
}

/// TestInitFullClusterRestore — Go same (incremental via backupMeta).
#[test]
// fullClusterRestore=true 时系统库与放置策略路径被启用。
fn test_init_full_cluster_restore() {
    let mut client = NewRestoreClientForTest();
    client.InitFullClusterRestore(true, true, true);
    assert!(!client.IsFullClusterRestore());
    client.InitFullClusterRestore(false, true, true);
    assert!(client.IsFullClusterRestore());
    client.InitFullClusterRestore(false, true, false);
    assert!(!client.IsFullClusterRestore());

    client.backupMeta = Some(backuppb::BackupMeta {
        StartVersion: 1,
        EndVersion: 2,
        ..Default::default()
    });
    client.InitFullClusterRestore(false, true, true);
    assert!(!client.IsFullClusterRestore());
}

// 捕获限速与 Close 调用顺序，替代真实 TiKV importer。
struct RecordingImporter {
    inner: MemImporterClient,
    error_store: u64,
    delay_ms: u64,
    recorded: Mutex<Vec<u64>>,
    reset_failures: Mutex<usize>,
}

impl ImporterClient for RecordingImporter {
    fn SetDownloadSpeedLimit(
        &self,
        ctx: &Context,
        store_id: u64,
        req: &import_sstpb::SetDownloadSpeedLimitRequest,
    ) -> Result<()> {
        if req.SpeedLimit == 0 {
            let mut failures = self.reset_failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                return Err(Error::new("transient reset failure"));
            }
        }
        if store_id == self.error_store {
            return Err(Error::new(format!("storeID:{store_id} ERROR")));
        }
        if self.delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(self.delay_ms));
        }
        self.recorded.lock().unwrap().push(store_id);
        self.inner.SetDownloadSpeedLimit(ctx, store_id, req)
    }
    fn CheckMultiIngestSupport(&self, c: &Context, s: &[u64]) -> Result<()> {
        self.inner.CheckMultiIngestSupport(c, s)
    }
    fn CheckBatchDownloadSupport(&self, c: &Context, s: &[u64]) -> Result<bool> {
        self.inner.CheckBatchDownloadSupport(c, s)
    }
    fn CheckBatchDownloadLatestMVCCSupport(&self, c: &Context, s: &[u64]) -> Result<()> {
        self.inner.CheckBatchDownloadLatestMVCCSupport(c, s)
    }
    fn AddForcePartitionRange(
        &self,
        c: &Context,
        id: u64,
        r: &import_sstpb::AddPartitionRangeRequest,
    ) -> Result<()> {
        self.inner.AddForcePartitionRange(c, id, r)
    }
    fn RemoveForcePartitionRange(
        &self,
        c: &Context,
        id: u64,
        r: &import_sstpb::RemovePartitionRangeRequest,
    ) -> Result<()> {
        self.inner.RemoveForcePartitionRange(c, id, r)
    }
    fn CloseGrpcClient(&self) -> Result<()> {
        self.inner.CloseGrpcClient()
    }
}

/// TestSetSpeedLimit — Go concurrent worker-pool behavior.
#[test]
// 每个 TiKV store 都应收到相同 rateLimit；PD 不可达时错误上抛。
fn test_set_speed_limit() {
    let stores: Vec<_> = (1..=10)
        .map(|id| metapb::Store {
            Id: id,
            State: metapb::StoreState::Up,
            ..Default::default()
        })
        .collect();
    let pd = Arc::new(MemPdClient {
        cluster_id: 1,
        stores: stores.clone(),
    });
    let mut client = NewRestoreClient(pd.clone(), pd);
    client.meta_client = Some(Arc::new(MemSplitClient::default()));
    let ctx = Context::Background();
    let importer = Arc::new(RecordingImporter {
        inner: MemImporterClient::default(),
        error_store: 0,
        delay_ms: 100,
        recorded: Mutex::new(Vec::new()),
        reset_failures: Mutex::new(0),
    });
    let start = Instant::now();
    MockCallSetSpeedLimit(&ctx, importer.clone(), &mut client, 10).unwrap();
    let cost = start.elapsed();
    assert!(
        cost < Duration::from_millis(500),
        "speed-limit RPCs ran serially: {cost:?}"
    );
    let mut recorded = importer.recorded.lock().unwrap().clone();
    recorded.sort();
    assert_eq!(recorded.len(), stores.len());
    for (i, s) in stores.iter().enumerate() {
        assert_eq!(recorded[i], s.Id);
    }
    client.Close();

    // Error store aborts subsequent, not-yet-started calls.
    let mut stores2 = stores.clone();
    stores2[5].Id = 999999;
    let pd2 = Arc::new(MemPdClient {
        cluster_id: 1,
        stores: stores2,
    });
    let mut client2 = NewRestoreClient(pd2.clone(), pd2);
    client2.meta_client = Some(Arc::new(MemSplitClient::default()));
    let importer2 = Arc::new(RecordingImporter {
        inner: MemImporterClient::default(),
        error_store: 999999,
        delay_ms: 0,
        recorded: Mutex::new(Vec::new()),
        reset_failures: Mutex::new(0),
    });
    let err = MockCallSetSpeedLimit(&ctx, importer2.clone(), &mut client2, 2).unwrap_err();
    assert!(err.msg.contains("999999") || err.msg.contains("ERROR"));
    assert!(importer2.recorded.lock().unwrap().len() < 10);
}

/// TestSetSpeedLimitCloseCallbackIdempotent — Go same.
#[test]
// closed 标志保证清零回调只执行一次，避免重复 RPC。
fn test_set_speed_limit_close_callback_idempotent() {
    let stores: Vec<_> = (1..=4)
        .map(|id| metapb::Store {
            Id: id,
            State: metapb::StoreState::Up,
            ..Default::default()
        })
        .collect();
    let ctx = Context::Background();
    let pd = Arc::new(MemPdClient {
        cluster_id: 1,
        stores: stores.clone(),
    });
    let recording = Arc::new(RecordingImporter {
        inner: MemImporterClient::default(),
        error_store: 0,
        delay_ms: 0,
        recorded: Mutex::new(Vec::new()),
        reset_failures: Mutex::new(2),
    });
    let import_client: Arc<dyn ImporterClient> = recording.clone();
    let closed = Arc::new(Mutex::new(false));
    let (create_cbs, mut close_cbs) =
        SetSpeedLimitCallbacks(&ctx, pd.clone(), import_client.clone(), 42, 4, closed).unwrap();
    let close_fn = close_cbs.pop().unwrap();
    let opt = NewSnapFileImporterOptions(
        None,
        Arc::new(MemSplitClient::default()),
        import_client,
        None,
        RewriteMode::RewriteModeLegacy,
        stores,
        1,
        0,
        false,
        create_cbs,
        Vec::new(),
    );
    let mut importer = NewSnapFileImporter(&ctx, 0, crate::import::KvMode::TiDBFull, opt).unwrap();
    close_fn(&mut importer).unwrap();
    close_fn(&mut importer).unwrap(); // idempotent via closed flag
    assert_eq!(*recording.reset_failures.lock().unwrap(), 0);
}

/// TestSortTablesBySchemaID — Go same.
#[test]
// 排序稳定性影响 DDL 并发批次的依赖满足顺序。
fn test_sort_tables_by_schema_id() {
    let tables = vec![
        mk_table("db", 2, "t", 3),
        mk_table("db", 1, "t", 2),
        mk_table("db", 3, "t", 5),
        mk_table("db", 1, "t", 1),
        mk_table("db", 2, "t", 4),
        mk_table("db", 3, "t", 6),
        mk_table("db", 6, "t", 7),
    ];
    let sorted = SortTablesBySchemaID(tables);
    assert_eq!(sorted.len(), 7);
    let schema_ids: Vec<_> = sorted.iter().map(|t| t.DB.ID).collect();
    let table_ids: Vec<_> = sorted.iter().map(|t| t.Info.ID).collect();
    assert_eq!(schema_ids, vec![1, 1, 2, 2, 3, 3, 6]);
    assert_eq!(table_ids, vec![1, 2, 3, 4, 5, 6, 7]);
}

fn reuse_checkpoint(
    start: i64,
    reusable_border: i64,
    end: i64,
    tables: &[metautil::Table],
) -> PreallocIDs {
    let (_, ids) = CollectTableIDs(tables).unwrap();
    PreallocIDs {
        Start: start,
        ReusableBorder: reusable_border,
        End: end,
        Hash: ComputeSortedIDsHash(&ids),
        AllocRule: HashMap::new(),
    }
}

/// TestAllocTableIDs — Go same allocation and checkpoint-reuse cases.
#[test]
// 预分配数量 >= 物理表数；分区表需额外 ID。
fn test_alloc_table_ids() {
    let mut client = NewRestoreClientForTest();
    let dom = Arc::new(MemDomain {
        empty: true,
        tables: Mutex::new(HashMap::from([
            (
                ("mysql".into(), "user".into()),
                model::TableInfo {
                    ID: 50,
                    Name: model::CIStr::new("user"),
                    ..Default::default()
                },
            ),
            (
                ("mysql".into(), "stats_meta".into()),
                model::TableInfo {
                    ID: 60,
                    Name: model::CIStr::new("stats_meta"),
                    ..Default::default()
                },
            ),
        ])),
    });
    client
        .InitConnections(dom, Box::new(MemDb::default()))
        .unwrap();

    client.AllocTableIDs(&[], false, false, None).unwrap();
    assert_eq!(client.GetPreAllocedTableIDRange().unwrap(), [0, 0]);
    assert!(client.CreatePreallocIDCheckpoint().is_none());

    let not_reused = client
        .AllocTableIDs(
            &[
                generate_metautil_table("mysql", 100, &[]),
                generate_metautil_table(&TemporaryDBName("mysql"), 99, &[]),
            ],
            true,
            false,
            None,
        )
        .unwrap();
    assert!(!not_reused);

    // min user id < prealloc start when reuse range starts above user ids
    let low_user_tables = vec![
        generate_metautil_table("mysql", 5, &[6]),
        generate_metautil_table("test", 3, &[4]),
    ];
    let not_reused = client
        .AllocTableIDs(
            &low_user_tables,
            true,
            false,
            Some(reuse_checkpoint(10, 10, 20, &low_user_tables)),
        )
        .unwrap();
    assert!(not_reused);

    let mut invalid_checkpoint = reuse_checkpoint(10, 10, 20, &low_user_tables);
    invalid_checkpoint.Hash[0] ^= 0xff;
    let err = client
        .AllocTableIDs(&low_user_tables, false, false, Some(invalid_checkpoint))
        .unwrap_err();
    assert!(err.msg.contains("hash mismatch"));

    let tables = vec![
        generate_metautil_table_with_name(&TemporaryDBName("mysql"), "user", 50, &[]),
        generate_metautil_table_with_name("test", "user", 100, &[]),
        generate_metautil_table_with_name(&TemporaryDBName("mysql"), "test", 200, &[]),
        generate_metautil_table_with_name("mysql", "test", 300, &[]),
    ];
    let not_reused = client
        .AllocTableIDs(
            &tables,
            false,
            true,
            Some(reuse_checkpoint(1, 301, 20000, &tables)),
        )
        .unwrap();
    assert!(!not_reused);
    assert!(client.temporarySystemTablesRenamed);
    let new_tables = client.CleanTablesIfTemporarySystemTablesRenamed(false, true, tables.clone());
    assert_eq!(new_tables.len(), 3);
    assert!(
        !new_tables
            .iter()
            .any(|t| t.DB.Name.O == "mysql" && t.Info.Name.O == "user")
    );

    let tables = vec![
        generate_metautil_table_with_name("test", "stats_meta", 100, &[]),
        generate_metautil_table_with_name(&TemporaryDBName("mysql"), "stats_meta", 60, &[]),
        generate_metautil_table_with_name(&TemporaryDBName("mysql"), "test", 200, &[]),
        generate_metautil_table_with_name("mysql", "test", 300, &[]),
    ];
    client.temporarySystemTablesRenamed = false;
    let not_reused = client
        .AllocTableIDs(
            &tables,
            true,
            false,
            Some(reuse_checkpoint(1, 301, 20000, &tables)),
        )
        .unwrap();
    assert!(!not_reused);
    let new_tables = client.CleanTablesIfTemporarySystemTablesRenamed(true, false, tables);
    assert_eq!(new_tables.len(), 3);
}

/// TestGetMinUserTableID — Go same (via export GetMinUserTableID / local fn).
#[test]
// 小于该阈值的 ID 视为系统保留，用户表映射不得落入。
fn test_get_min_user_table_id() {
    let min = getMinUserTableID(&[
        generate_metautil_table("mysql", 1, &[]),
        generate_metautil_table(&TemporaryDBName("mysql"), 2, &[]),
        generate_metautil_table("mysql", 4, &[]),
        generate_metautil_table(&TemporaryDBName("mysql"), 3, &[]),
    ]);
    assert_eq!(min, i64::MAX);
    let min = getMinUserTableID(&[
        generate_metautil_table("mysql", 3, &[1]),
        generate_metautil_table("test", 4, &[6]),
    ]);
    assert_eq!(min, 4);
    let min = getMinUserTableID(&[
        generate_metautil_table("mysql", 4, &[1]),
        generate_metautil_table("test", 3, &[2]),
        generate_metautil_table("test2", 5, &[6]),
    ]);
    assert_eq!(min, 2);
}

#[test]
fn test_new_restore_client_uses_go_zero_value_defaults() {
    let pd = Arc::new(MemPdClient {
        cluster_id: 7,
        stores: Vec::new(),
    });
    let client = NewRestoreClient(pd.clone(), pd);
    assert_eq!(client.concurrencyPerStore, 0);
    assert_eq!(client.regionScanConcurrency, 0);
    assert_eq!(client.batchDdlSize, 0);
    assert!(client.policyMode.is_empty());
    assert!(client.restoreUUID.is_empty());
}

#[test]
fn test_client_configuration_matches_go_edge_cases() {
    let mut client = NewRestoreClientForTest();
    client.SetPlacementPolicyMode("ignore");
    assert_eq!(client.policyMode, IGNORE_PLACEMENT_POLICY_MODE);

    client.SetBatchDdlSize(0);
    assert_eq!(client.GetBatchDdlSize(), 0);

    assert!(!needLoadSchemas(&backuppb::BackupMeta {
        IsTxnKv: true,
        ..Default::default()
    }));

    client.SetCrypter(Some(backuppb::CipherInfo::default()));
    assert!(client.cipher.is_some());
    client.SetCrypter(None);
    assert!(client.cipher.is_none());

    let pd = Arc::new(MemPdClient {
        cluster_id: 1,
        stores: Vec::new(),
    });
    let mut uninitialized = NewRestoreClient(pd.clone(), pd);
    assert!(uninitialized.EnsureNoUserTables().is_err());
    assert!(
        uninitialized
            .ExecDDLs(&Context::Background(), vec![model::Job::default()])
            .is_err()
    );
}

struct KeyspacePd;

impl PdClient for KeyspacePd {
    fn SupportsKeyspaceBR(&self, _ctx: &Context) -> Result<bool> {
        Ok(true)
    }
}

impl StoreMeta for KeyspacePd {
    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
        Ok(Vec::new())
    }
}

#[test]
fn test_set_rewrite_mode_uses_cluster_capability_probe() {
    let pd = Arc::new(KeyspacePd);
    let mut client = NewRestoreClient(pd.clone(), pd);
    client.SetRewriteMode(&Context::Background());
    assert_eq!(client.GetRewriteMode(), RewriteMode::RewriteModeKeyspace);
}

struct CloseRecordingRestorer(Arc<AtomicUsize>);

impl SstRestorer for CloseRecordingRestorer {
    fn Close(&mut self) -> Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn GoRestore(
        &mut self,
        _on_progress: &dyn Fn(i64),
        _groups: &[BatchBackupFileSet],
    ) -> Result<()> {
        Ok(())
    }

    fn WaitUntilFinish(&mut self) -> Result<()> {
        Ok(())
    }
}

#[test]
fn test_get_restorer_keeps_cached_instance_until_client_close() {
    let closes = Arc::new(AtomicUsize::new(0));
    let mut client = NewRestoreClientForTest();
    client.restorer = Some(Box::new(CloseRecordingRestorer(closes.clone())));
    let _ = client.GetRestorer();
    client.Close();
    assert_eq!(closes.load(Ordering::SeqCst), 1);
}

#[test]
fn test_load_schema_initializes_raw_and_txn_clients() {
    let stores = vec![metapb::Store {
        Id: 1,
        State: metapb::StoreState::Up,
        ..Default::default()
    }];
    let pd = Arc::new(MemPdClient {
        cluster_id: 9,
        stores,
    });
    let mut raw_client = NewRestoreClient(pd.clone(), pd.clone());
    raw_client.SetConcurrencyPerStore(1);
    raw_client.meta_client = Some(Arc::new(MemSplitClient::default()));
    raw_client.import_client = Some(Arc::new(MemImporterClient::default()));
    raw_client
        .LoadSchemaIfNeededAndInitClient(
            &Context::Background(),
            backuppb::BackupMeta {
                IsRawKv: true,
                ApiVersion: 2,
                ..Default::default()
            },
            HashMap::from([("must-not-load".into(), metautil::Database::default())]),
            vec![model::Job::default()],
            None,
            b"a".to_vec(),
            b"z".to_vec(),
            false,
            true,
            true,
        )
        .unwrap();
    assert!(raw_client.databases.is_empty());
    let importer = raw_client.importer.as_ref().unwrap();
    assert_eq!(importer.kvMode, KvMode::Raw);
    assert_eq!(importer.apiVersion, 2);
    assert_eq!(importer.rawStartKey, b"a");
    assert_eq!(importer.rawEndKey, b"z");
    assert_eq!(raw_client.workerPoolSize, 7186);
    assert!(raw_client.IsFullClusterRestore());

    let mut txn_client = NewRestoreClient(pd.clone(), pd);
    txn_client.SetConcurrencyPerStore(1);
    txn_client.meta_client = Some(Arc::new(MemSplitClient::default()));
    txn_client.import_client = Some(Arc::new(MemImporterClient::default()));
    txn_client
        .LoadSchemaIfNeededAndInitClient(
            &Context::Background(),
            backuppb::BackupMeta {
                IsTxnKv: true,
                ..Default::default()
            },
            HashMap::new(),
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            false,
            true,
            true,
        )
        .unwrap();
    assert_eq!(txn_client.importer.as_ref().unwrap().kvMode, KvMode::Txn);
}

#[derive(Default)]
struct PolicyDb {
    created: Arc<Mutex<Vec<String>>>,
}

impl DbSession for PolicyDb {
    fn Execute(&mut self, _ctx: &Context, _sql: &str) -> Result<()> {
        Ok(())
    }

    fn ExecDDL(&mut self, _ctx: &Context, _job: &model::Job) -> Result<()> {
        Ok(())
    }

    fn RegisterPreallocatedIDs(&mut self, _ids: &PreallocIDs) {}

    fn CreateTable(
        &mut self,
        _ctx: &Context,
        _table: &metautil::Table,
        _rebased: &HashMap<UniqueTableName, bool>,
        _support_policy: bool,
    ) -> Result<()> {
        Ok(())
    }

    fn CreatePlacementPolicy(&mut self, _ctx: &Context, policy: &model::PolicyInfo) -> Result<()> {
        self.created.lock().unwrap().push(policy.Name.O.clone());
        Ok(())
    }
}

struct RetryResetController {
    failures: AtomicUsize,
    seen: Mutex<Vec<u64>>,
}

impl PdController for RetryResetController {
    fn ResetTS(&self, _ctx: &Context, ts: u64) -> Result<()> {
        self.seen.lock().unwrap().push(ts);
        if self.failures.fetch_sub(1, Ordering::SeqCst) > 0 {
            Err(Error::new("transient reset error"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn test_partition_policy_tls_and_reset_helpers_match_go() {
    let mut client = NewRestoreClientForTest();
    client.tlsConf = Some(TlsConfig {
        CaPath: "ca".into(),
        CertPath: "cert".into(),
        KeyPath: "key".into(),
    });
    assert_eq!(client.GetTLSConfig().unwrap().CaPath, "ca");

    let table = generate_metautil_table_with_name("db", "pt", 10, &[11, 12]);
    client.databases.insert(
        "db".into(),
        metautil::Database {
            Info: model::DBInfo {
                ID: 7,
                Name: model::CIStr::new("db"),
            },
            Tables: vec![table],
            ..Default::default()
        },
    );
    let partitions = client.GetPartitionMap();
    assert_eq!(partitions[&11].ParentTableID, 10);
    assert_eq!(partitions[&11].TableName, "pt");
    assert_eq!(partitions[&11].DbID, 7);
    assert!(partitions[&11].IsPartition);

    client.backupMeta = Some(backuppb::BackupMeta {
        EndVersion: 88,
        Policies: vec![backuppb::Policy {
            Info: br#"{"name":{"O":"Primary","L":"primary"}}"#.to_vec(),
        }],
        ..Default::default()
    });
    let policies = client.GetPlacementPolicies().unwrap();
    assert_eq!(policies["primary"].Name.O, "Primary");
    client.SetPolicyMap(policies.clone());
    let created = Arc::new(Mutex::new(Vec::new()));
    client.db = Some(Box::new(PolicyDb {
        created: created.clone(),
    }));
    client
        .CreatePolicies(&Context::Background(), &policies)
        .unwrap();
    assert_eq!(*created.lock().unwrap(), vec!["Primary"]);

    let controller = RetryResetController {
        failures: AtomicUsize::new(2),
        seen: Mutex::new(Vec::new()),
    };
    client.ResetTS(&Context::Background(), &controller).unwrap();
    assert_eq!(*controller.seen.lock().unwrap(), vec![88, 88, 88]);
}

#[derive(Default)]
struct CheckpointState {
    waits: Mutex<Vec<bool>>,
    flushed: Mutex<Vec<ChecksumItem>>,
}

struct TestCheckpointRunner(Arc<CheckpointState>);

impl CheckpointRunner for TestCheckpointRunner {
    fn WaitForFinish(&mut self, _ctx: &Context, flush: bool) {
        self.0.waits.lock().unwrap().push(flush);
    }

    fn FlushChecksumItem(&mut self, _ctx: &Context, item: &ChecksumItem) -> Result<()> {
        self.0.flushed.lock().unwrap().push(item.clone());
        Ok(())
    }
}

#[derive(Default)]
struct TestCheckpointManager {
    metadata: Mutex<Option<CheckpointMetadata>>,
    data: Mutex<Vec<(i64, String)>>,
    checksums: Mutex<HashMap<i64, ChecksumItem>>,
    state: Arc<CheckpointState>,
}

impl SnapshotCheckpointManager for TestCheckpointManager {
    fn LoadCheckpointMetadata(&self, _ctx: &Context) -> Result<CheckpointMetadata> {
        self.metadata
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::new("checkpoint metadata not found"))
    }

    fn SaveCheckpointMetadata(&self, _ctx: &Context, metadata: &CheckpointMetadata) -> Result<()> {
        *self.metadata.lock().unwrap() = Some(metadata.clone());
        Ok(())
    }

    fn LoadCheckpointData(&self, _ctx: &Context) -> Result<Vec<(i64, String)>> {
        Ok(self.data.lock().unwrap().clone())
    }

    fn LoadCheckpointChecksum(&self, _ctx: &Context) -> Result<HashMap<i64, ChecksumItem>> {
        Ok(self.checksums.lock().unwrap().clone())
    }

    fn StartCheckpointRunner(&self, _ctx: &Context) -> Result<Box<dyn CheckpointRunner>> {
        Ok(Box::new(TestCheckpointRunner(self.state.clone())))
    }
}

struct FixedChecksumClient {
    calls: AtomicUsize,
    item: ChecksumItem,
}

impl ChecksumClient for FixedChecksumClient {
    fn CalculateChecksum(
        &self,
        _ctx: &Context,
        _table: &CreatedTable,
        _concurrency: u32,
    ) -> Result<ChecksumItem> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.item.clone())
    }
}

#[test]
fn test_checkpoint_resume_and_checksum_validation_match_go() {
    let ctx = Context::Background();
    let manager = TestCheckpointManager::default();
    let mut client = NewRestoreClientForTest();
    client.backupMeta = Some(backuppb::BackupMeta {
        ClusterId: 42,
        EndVersion: 99,
        ..Default::default()
    });
    client.preallocedIDs = Some(PreallocIDs {
        Start: 10,
        ReusableBorder: 15,
        End: 20,
        Hash: vec![7],
        AllocRule: HashMap::new(),
    });
    let config = ClusterConfig {
        Schedulers: vec!["balance-region".into()],
        ScheduleCfg: "cfg".into(),
        RuleID: "rule".into(),
    };
    let (ranges, loaded_config, start_ts) = client
        .InitCheckpoint(
            &ctx,
            &manager,
            Some(config.clone()),
            11,
            77,
            vec![1, 2, 3],
            false,
        )
        .unwrap();
    assert!(ranges.is_empty());
    assert!(loaded_config.is_none());
    assert_eq!(start_ts, 11);
    let saved = manager.metadata.lock().unwrap().clone().unwrap();
    assert_eq!(saved.UpstreamClusterID, 42);
    assert_eq!(saved.RestoredTS, 99);
    assert_eq!(saved.PreallocIDs, client.preallocedIDs);
    assert_eq!(saved.SchedulersConfig, Some(config.clone()));
    assert!(!client.restoreUUID.is_empty());

    manager
        .data
        .lock()
        .unwrap()
        .extend([(10, "a".into()), (10, "b".into())]);
    manager.checksums.lock().unwrap().insert(
        20,
        ChecksumItem {
            TableID: 20,
            Crc64xor: 9,
            TotalKvs: 3,
            TotalBytes: 30,
        },
    );
    let mut resumed = NewRestoreClientForTest();
    resumed.backupMeta = client.backupMeta.clone();
    let (ranges, loaded_config, start_ts) = resumed
        .InitCheckpoint(&ctx, &manager, None, 0, 77, vec![1, 2, 3], true)
        .unwrap();
    assert_eq!(ranges[&10].len(), 2);
    assert_eq!(loaded_config, Some(config));
    assert_eq!(start_ts, 11);
    assert_eq!(resumed.restoreUUID, saved.RestoreUUID);
    resumed.WaitForFinishCheckpoint(&ctx, true);
    assert_eq!(*manager.state.waits.lock().unwrap(), vec![true]);

    let mut old_table = mk_table("db", 1, "t", 10);
    old_table.FilesOfPhysicals.insert(
        10,
        vec![backuppb::File {
            Crc64Xor: 9,
            TotalKvs: 3,
            TotalBytes: 30,
            ..Default::default()
        }],
    );
    let created_table = CreatedTable {
        Table: model::TableInfo {
            ID: 20,
            Name: model::CIStr::new("t"),
            ..Default::default()
        },
        OldTable: old_table,
        ..Default::default()
    };
    let checksum_client = FixedChecksumClient {
        calls: AtomicUsize::new(0),
        item: ChecksumItem {
            TableID: 20,
            Crc64xor: 9,
            TotalKvs: 3,
            TotalBytes: 30,
        },
    };
    resumed.checkpointChecksum.clear();
    resumed
        .execAndValidateChecksum(&ctx, &created_table, &checksum_client, 4)
        .unwrap();
    assert_eq!(checksum_client.calls.load(Ordering::SeqCst), 1);
    assert_eq!(manager.state.flushed.lock().unwrap().len(), 1);

    resumed.checkpointChecksum.insert(
        20,
        ChecksumItem {
            TableID: 20,
            Crc64xor: 8,
            TotalKvs: 3,
            TotalBytes: 30,
        },
    );
    let err = resumed
        .execAndValidateChecksum(&ctx, &created_table, &checksum_client, 4)
        .unwrap_err();
    assert_eq!(err.code, Some("BR:Restore:ErrRestoreChecksumMismatch"));

    let mut mismatch = NewRestoreClientForTest();
    mismatch.backupMeta = resumed.backupMeta.clone();
    assert!(
        mismatch
            .InitCheckpoint(&ctx, &manager, None, 0, 77, vec![9], true)
            .is_err()
    );
}

#[test]
fn test_install_pitr_support_registers_lifecycle_and_rejects_incremental() {
    let ctx = Context::Background();
    let pd = Arc::new(MemPdClient {
        cluster_id: 1,
        stores: vec![metapb::Store {
            Id: 1,
            State: metapb::StoreState::Up,
            ..Default::default()
        }],
    });
    let restore_storage = Arc::new(MemStorage::default());
    let task_storage = Arc::new(MemStorage::default());

    let mut client = NewRestoreClient(pd.clone(), pd.clone());
    client.SetConcurrencyPerStore(2);
    client.meta_client = Some(Arc::new(MemSplitClient::default()));
    client.import_client = Some(Arc::new(MemImporterClient::default()));
    client
        .LoadSchemaIfNeededAndInitClient(
            &ctx,
            backuppb::BackupMeta {
                StartVersion: 9,
                EndVersion: 9,
                ..Default::default()
            },
            HashMap::new(),
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            false,
            true,
            true,
        )
        .unwrap();
    client
        .InstallPiTRSupport(
            &ctx,
            PiTRCollDep {
                PDCli: Some(pd.clone()),
                Storage: Some(restore_storage.clone()),
                TaskStorage: Some(task_storage.clone()),
                enabled: true,
                name: "task-51".into(),
                restoreSuccess: Some(Box::new(|| true)),
                tso: Some(Box::new(|_| Ok(123))),
                ..Default::default()
            },
        )
        .unwrap();
    client.Close();
    assert!(
        task_storage
            .FileExists(&ctx, "v1/ext_backups/task-51/extbackupmeta")
            .unwrap()
    );

    let mut incremental = NewRestoreClient(pd.clone(), pd.clone());
    incremental.SetConcurrencyPerStore(1);
    incremental.meta_client = Some(Arc::new(MemSplitClient::default()));
    incremental.import_client = Some(Arc::new(MemImporterClient::default()));
    incremental
        .LoadSchemaIfNeededAndInitClient(
            &ctx,
            backuppb::BackupMeta {
                StartVersion: 1,
                EndVersion: 2,
                ..Default::default()
            },
            HashMap::new(),
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            false,
            true,
            true,
        )
        .unwrap();
    let err = incremental
        .InstallPiTRSupport(
            &ctx,
            PiTRCollDep {
                PDCli: Some(pd),
                Storage: Some(restore_storage),
                TaskStorage: Some(task_storage),
                enabled: true,
                name: "task-51-incremental".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.code, Some("BR:Stream:ErrStreamLogTaskExist"));
}

#[test]
fn test_skip_create_database_preserves_metadata_and_session_state() {
    let mut client = NewRestoreClientForTest();
    client.EnableSkipCreateSQL();
    let db = metautil::Database {
        Info: model::DBInfo {
            Name: model::CIStr::new("skipped"),
            ..Default::default()
        },
        ..Default::default()
    };
    client
        .CreateDatabases(&Context::Background(), &[db])
        .unwrap();
    assert!(!client.databases.contains_key("skipped"));
}

#[test]
fn test_full_restore_clears_rebased_tables_and_sys_db_requires_temporary_name() {
    let mut client = NewRestoreClientForTest();
    let tables = vec![mk_table("db", 1, "t", 2)];
    client.generateRebasedTables(&tables);
    assert!(client.rebasedTablesMap.is_empty());

    client.databases.insert(
        "mysql".into(),
        metautil::Database {
            Info: model::DBInfo {
                Name: model::CIStr::new("mysql"),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    assert!(!client.HasBackedUpSysDB());
    client.databases.clear();
    client
        .databases
        .insert(TemporaryDBName("mysql"), metautil::Database::default());
    assert!(client.HasBackedUpSysDB());
}

#[test]
fn test_get_files_in_raw_range_matches_go_coverage_and_boundaries() {
    let mut client = NewRestoreClientForTest();
    let err = client
        .GetFilesInRawRange(b"b", b"d", "default")
        .unwrap_err();
    assert_eq!(err.code, Some("BR:Restore:ErrRestoreModeMismatch"));

    client.backupMeta = Some(backuppb::BackupMeta {
        IsRawKv: true,
        RawRanges: vec![backuppb::RawRange {
            StartKey: b"a".to_vec(),
            EndKey: b"z".to_vec(),
            Cf: "default".into(),
        }],
        Files: vec![
            backuppb::File {
                Name: "left".into(),
                StartKey: b"a".to_vec(),
                EndKey: b"c".to_vec(),
                Cf: "default".into(),
                ..Default::default()
            },
            backuppb::File {
                Name: "middle".into(),
                StartKey: b"c".to_vec(),
                EndKey: b"e".to_vec(),
                Cf: "default".into(),
                ..Default::default()
            },
            backuppb::File {
                Name: "other-cf".into(),
                StartKey: b"b".to_vec(),
                EndKey: b"d".to_vec(),
                Cf: "write".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    });

    let files = client.GetFilesInRawRange(b"c", b"d", "default").unwrap();
    assert_eq!(
        files
            .iter()
            .map(|file| file.Name.as_str())
            .collect::<Vec<_>>(),
        vec!["left", "middle"]
    );

    let err = client
        .GetFilesInRawRange(b"0", b"d", "default")
        .unwrap_err();
    assert_eq!(err.code, Some("BR:Restore:ErrRestoreRangeMismatch"));
}

#[test]
fn test_download_worker_pool_scales_with_stores_independently_of_import_concurrency() {
    for store_count in [0, 1, 3] {
        for concurrency in [1, 36, 128] {
            let mut client = NewRestoreClientForTest();
            client.SetConcurrencyPerStore(concurrency);
            let stores = (0..store_count)
                .map(|id| metapb::Store {
                    Id: id as u64 + 1,
                    State: metapb::StoreState::Up,
                    ..Default::default()
                })
                .collect();
            client
                .initClients(
                    &Context::Background(),
                    None,
                    true,
                    false,
                    Arc::new(MemSplitClient::default()),
                    Arc::new(MemImporterClient::default()),
                    stores,
                    Vec::new(),
                    Vec::new(),
                )
                .unwrap();
            assert_eq!(client.workerPoolSize, store_count * 7186);
            assert_eq!(client.concurrencyPerStore, concurrency);
        }
    }
}
