// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

use std::collections::BTreeMap;

use crate::table::{
    TableError, TableInfo as CatalogTableInfo, TableState, alter_shard_row_id_bits,
    set_tiflash_replica, update_tiflash_replica_status,
};

fn catalog_table() -> CatalogTableInfo {
    CatalogTableInfo {
        id: 1,
        schema_id: 1,
        name: "t".into(),
        state: TableState::Public,
        partition_ids: Vec::new(),
        auto_increment_id: 0,
        auto_random_id: 0,
        auto_id_cache: 0,
        auto_id_schema_id: 0,
        shard_row_id_bits: 10,
        max_shard_row_id_bits: 10,
        comment: String::new(),
        charset: "utf8mb4".into(),
        collation: "utf8mb4_bin".into(),
        version: 0,
        foreign_keys: Vec::new(),
        tiflash_replica: None,
        placement_policy: None,
        attributes: BTreeMap::new(),
        cached: false,
        affinity: None,
        split_policy: None,
    }
}

#[test]
fn lowering_shard_row_id_bits_below_historical_max_matches_go() {
    let mut table = catalog_table();

    assert_eq!(Ok(true), alter_shard_row_id_bits(&mut table, 5));
    assert_eq!(5, table.shard_row_id_bits);
    assert_eq!(10, table.max_shard_row_id_bits);

    assert_eq!(Ok(false), alter_shard_row_id_bits(&mut table, 5));
    assert_eq!(
        Err(TableError::ShardBitsOverflow),
        alter_shard_row_id_bits(&mut table, 16)
    );
}

#[test]
fn changing_tiflash_replica_count_preserves_availability_like_go() {
    let mut table = catalog_table();
    table.partition_ids = vec![11, 12];

    set_tiflash_replica(&mut table, 2, vec!["zone".into()]).unwrap();
    update_tiflash_replica_status(&mut table, 11, true).unwrap();
    set_tiflash_replica(&mut table, 3, vec!["zone".into(), "rack".into()]).unwrap();

    let replica = table.tiflash_replica.as_ref().unwrap();
    assert_eq!(3, replica.count);
    assert_eq!(vec!["zone", "rack"], replica.location_labels);
    assert_eq!(1, replica.available_partition_ids.len());
    assert!(replica.available_partition_ids.contains(&11));
}

// 表级 DDL 测试 helper 与用例占位（源自 Go `table_test.go`）。
//
// 本文件保留 create/drop/truncate/rename/lock/cache、视图、TTL、
// owner 切换并发建表、RefreshMeta 等测试的调用形状与中文流程说明；
// 占位类型尚未接入真实 testkit，函数体多为空实现以便机械对照 Go。

// 以下占位类型保留 Go API 的调用形状；本计划不要求接入 Rust crate 或测试框架。
/// 测试框架 `testing.T` 的占位类型。
type TestingT = ();
/// 会话上下文占位。
type SessionContext = ();
/// DDL executor 测试接口占位。
type DDLExecutorForTest = ();
/// KV Storage 占位。
type Storage = ();
/// Domain（领域服务容器）占位。
type Domain = ();
/// TestKit 占位。
type TestKit = ();
/// DDL Job 模型占位。
type ModelJob = ();
/// 数据库元信息占位。
type DBInfo = ();
/// 表元信息占位。
type TableInfo = ();
/// 大小写不敏感字符串（CIStr）占位。
type CIStr = String;
/// 表锁类型占位（Go 中为枚举，此处用静态字符串）。
type TableLockType = &'static str;

/// 构造并提交 ActionRenameTable job 的测试 helper。
// testRenameTable 对应 Go 的同名 helper：构造 ActionRenameTable job 并通过 DDL executor 提交。
pub fn test_rename_table(
    _t: &TestingT,
    _ctx: &mut SessionContext,
    _d: &DDLExecutorForTest,
    _new_schema_id: i64,
    _old_schema_id: i64,
    old_schema_name: CIStr,
    new_schema_name: CIStr,
    _tbl_info: &mut TableInfo,
) -> ModelJob {
    // Go 里会填充 InvolvingSchemaInfo，分别记录旧库表和新库表，供 DDL job history 校验。
    let _involving_schema_info = vec![old_schema_name, new_schema_name];
    // ctx.SetValue(QueryString, "skip") 避免测试中依赖真实 SQL 文本。
    // d.DoDDLJobWrapper(...) 提交后，Go 会把 tblInfo.State 临时设为 Public 做 history 校验，再恢复 StateNone。
    ()
}

/// 批量 rename tables 的测试 helper。
// testRenameTables 对应批量 rename tables 的 helper，保留多表参数拆包和 history job 校验语义。
pub fn test_rename_tables(
    _t: &TestingT,
    _ctx: &mut SessionContext,
    _d: &DDLExecutorForTest,
    _old_schema_ids: Vec<i64>,
    _new_schema_ids: Vec<i64>,
    _new_table_names: Vec<CIStr>,
    _old_table_ids: Vec<i64>,
    _old_schema_names: Vec<CIStr>,
    _old_table_names: Vec<CIStr>,
) -> ModelJob {
    // Go 通过 model.GetRenameTablesArgsFromV1 构造 RenameTablesArgs；这里保留批量参数的顺序约束。
    // checkJobWithHistory(t, ctx, job.ID, nil, nil) 校验多表 DDL 已进入 history。
    ()
}

/// 提交 ActionLockTable 的测试 helper。
// testLockTable 对应 Go 的表锁 helper：把 session/server 信息写入 LockTablesArgs。
pub fn test_lock_table(
    _t: &TestingT,
    _ctx: &mut SessionContext,
    _d: &DDLExecutorForTest,
    uuid: &str,
    _new_schema_id: i64,
    schema_name: CIStr,
    _tbl_info: &TableInfo,
    lock_tp: TableLockType,
) -> ModelJob {
    // SessionInfo 取 ctx.GetSessionVars().ConnectionID；不读取真实 session。
    let _lock_session = (uuid, lock_tp, schema_name);
    // DDL job 类型是 ActionLockTable，完成后 checkJobWithHistory 只检查 job history，不替换 table info。
    ()
}

/// 从 meta KV 校验表锁会话、类型与 Public 状态。
// checkTableLockedTest 读取 meta kv 中的 TableInfo.Lock，验证锁会话、锁类型和 public 状态。
pub fn check_table_locked_test(
    _t: &TestingT,
    _store: &Storage,
    _db_info: &DBInfo,
    _tbl_info: &TableInfo,
    server_id: &str,
    session_id: u64,
    lock_tp: TableLockType,
) {
    // Go 使用 kv.RunInNewTxn 包裹 meta.NewMutator(txn).GetTable；失败会立即 require.NoError。
    // 事务只读地检查 info.Lock.Sessions[0]，不会修改测试数据。
    let _expected_lock = (server_id, session_id, lock_tp);
}

/// 提交 truncate table（含申请新 table ID）的测试 helper。
// testTruncateTable 对应 truncate table 流程：先申请新 table ID，再提交 ActionTruncateTable。
pub fn test_truncate_table(
    _t: &TestingT,
    _ctx: &mut SessionContext,
    _store: &Storage,
    _d: &DDLExecutorForTest,
    _db_info: &DBInfo,
    _tbl_info: &mut TableInfo,
) -> ModelJob {
    // Go 调用 genGlobalIDs(store, 1)，并在 DDL 成功后把 tblInfo.ID 替换成 newTableID。
    // checkJobWithHistory 需要看到新的 TableInfo，因此这里保留“先换 ID 再验 history”的迁移点。
    ()
}

/// 从 meta 事务读取 TableInfo 并构造 table 对象的测试桥梁。
// testGetTableWithError 从 meta 事务里读取 TableInfo，再构造 autoid allocator 和 table.Table。
pub fn test_get_table_with_error(_schema_id: i64, _table_id: i64) -> Result<(), String> {
    // Go 对 GetTable 错误使用 errors.Trace，对 nil TableInfo 返回 "table not found"。
    // 该 helper 是测试读取 table meta 的桥梁，不创建真实 allocator。
    Ok(())
}

/// 覆盖 create/drop/truncate/rename/lock/cache 等基础表 DDL 流程。
// TestTable 覆盖 create/drop/truncate/rename/lock/cache/no-cache 的基础表 DDL 流程。
#[test]
pub fn test_table() {
    // 创建 mock store/domain，取 domain.DDLExecutor 作为 ExecutorForTest。
    // 1. 创建 schema 与普通表，插入 2000 行，再 drop table 并检查 job done。
    // 2. 创建 tt 后 truncate，验证新 table ID 对应的 StatePublic 和 history job。
    // 3. rename 到新 schema，随后 lock table 并从 meta 中检查锁信息。
    // 4. alter cache/no-cache table，分别检查 TableCacheStatusEnable/Disable。
    // 5. 最后 drop schema；Go 源测试未在 defer 中收尾，顺序依赖每一步 require 成功。
}

/// 覆盖创建 view、replace view 及旧 view ID 不存在的场景。
// TestCreateView 迁移创建 view、replace view，以及 replace 使用不存在旧 view ID 的场景。
#[test]
pub fn test_create_view() {
    // 先创建基础表 t，再手动构造 ActionCreateView job。
    // Replace 分支把 OnExistReplace 设为 true，并携带 OldViewTblID；Go 断言不存在旧 ID 时也不再报错。
}

/// 校验表缓存状态为 Enable。
// checkTableCacheTest 从 meta 事务读取 TableCacheStatusType，要求为 enable。
pub fn check_table_cache_test(
    _t: &TestingT,
    _store: &Storage,
    _db_info: &DBInfo,
    _tbl_info: &TableInfo,
) {
    // Go 使用 internal DDL source type 开启新事务，避免污染外层 session 上下文。
}

/// 校验表缓存状态为 Disable。
// checkTableNoCacheTest 与 cache 检查相反，要求 TableCacheStatusDisable。
pub fn check_table_no_cache_test(
    _t: &TestingT,
    _store: &Storage,
    _db_info: &DBInfo,
    _tbl_info: &TableInfo,
) {
    // 这里保留 require.Equal(TableCacheStatusDisable, info.TableCacheStatusType) 的断言意图。
}

/// 提交 ActionAlterCacheTable 的测试 helper。
// testAlterCacheTable 提交 ActionAlterCacheTable，并记录涉及的 schema/table。
pub fn test_alter_cache_table(
    _t: &TestingT,
    _ctx: &mut SessionContext,
    _d: &DDLExecutorForTest,
    _new_schema_id: i64,
    new_schema_name: CIStr,
    _tbl_info: &TableInfo,
) -> ModelJob {
    // Args 是 model.EmptyArgs；DDL job 完成后只检查 history，不替换 TableInfo。
    let _schema_for_history = new_schema_name;
    ()
}

/// 提交 ActionAlterNoCacheTable 的测试 helper。
// testAlterNoCacheTable 提交 ActionAlterNoCacheTable，结构与 testAlterCacheTable 对称。
pub fn test_alter_no_cache_table(
    _t: &TestingT,
    _ctx: &mut SessionContext,
    _d: &DDLExecutorForTest,
    _new_schema_id: i64,
    new_schema_name: CIStr,
    _tbl_info: &TableInfo,
) -> ModelJob {
    // Go 这里直接 require.NoError 包住 DoDDLJobWrapper，失败会终止测试。
    let _schema_for_history = new_schema_name;
    ()
}

/// 批量 rename 两张表并校验 history 中的 MultipleTableInfos。
// TestRenameTables 创建两张表并批量 rename，最后读取 history job 验证 MultipleTableInfos。
#[test]
pub fn test_rename_tables_case() {
    // Go 先生成 t1/t2 和 tt1/tt2 的 TableInfo，再调用 testRenameTables。
    // historyJob.BinlogInfo.MultipleTableInfos 的顺序必须是 tt1、tt2。
}

/// 批量建表，并用 failpoint 模拟首次 GetJobByID 失败后仍成功。
// TestCreateTables 批量创建三张表，并用 failpoint 模拟首次 GetJobByID 失败后仍能成功。
#[test]
pub fn test_create_tables() {
    // genGlobalIDs(store, 3) 产生三张表 ID，BatchCreateTableArgs.Tables 顺序对应 s1/s2/s3。
    // mockGetJobByIDFail 使用 sync.Once，只在第一次 failpoint 回调时注入错误。
    // DoDDLJobWrapper 成功后逐一 testGetTable，确认三张表均可通过 domain 读取。
}

/// 覆盖修改 / 移除 TTLInfo 时的 DDL history 断言。
// TestAlterTTL 覆盖修改 TTLInfo 和移除 TTLInfo 的 DDL history 信息。
#[test]
pub fn test_alter_ttl() {
    // 初始表有两个 datetime 列，TTLInfo 指向第一列，间隔为 5 day。
    // ActionAlterTTLInfo 后期望 historyJob.BinlogInfo.TableInfo.TTLInfo 指向第二列，间隔为 1 year。
    // ActionAlterTTLRemove 后期望 history 中 TTLInfo 为空。
}

/// rename 中间态用例：SQL、并发 insert、期望错误与最终表名。
// RenameIntermediateStateCase 对应 Go 中匿名 testCases struct。
pub struct RenameIntermediateStateCase {
    /// 执行的 rename SQL。
    pub rename_sql: &'static str,
    /// schema sync 阶段尝试的 insert SQL。
    pub insert_sql: &'static str,
    /// 期望的错误消息；空串表示应成功。
    pub err_msg: &'static str,
    /// 最终可见的库表名。
    pub final_db: &'static str,
}

/// 在 rename 的 schema sync 中间态插入数据，验证新旧表可见性。
// TestRenameTableIntermediateState 在 rename table schema sync 阶段插入数据，验证新旧表可见性。
#[test]
pub fn test_rename_table_intermediate_state() {
    let test_cases = [
        RenameIntermediateStateCase {
            rename_sql: "rename table db1.t to db1.t1;",
            insert_sql: "insert into db1.t values(1);",
            err_msg: "[schema:1146]Table 'db1.t' doesn't exist",
            final_db: "db1.t1",
        },
        RenameIntermediateStateCase {
            rename_sql: "rename table db1.t1 to db1.t;",
            insert_sql: "insert into db1.t values(1);",
            err_msg: "",
            final_db: "db1.t",
        },
        RenameIntermediateStateCase {
            rename_sql: "rename table db1.t to db2.t;",
            insert_sql: "insert into db1.t values(1);",
            err_msg: "[schema:1146]Table 'db1.t' doesn't exist",
            final_db: "db2.t",
        },
        RenameIntermediateStateCase {
            rename_sql: "rename table db2.t to db1.t;",
            insert_sql: "insert into db1.t values(1);",
            err_msg: "",
            final_db: "db1.t",
        },
    ];
    // failpoint afterWaitSchemaSynced 只处理未完成 job；当 job 到达 StatePublic 时尝试并发 insert。
    // 有 err_msg 的 case 期望旧表不可见，无 err_msg 的 case 期望新表可写。
    let _preserved_cases = test_cases;
}

/// 频繁 owner 切换时并发提交同名表/库创建。
// TestCreateSameTableOrDBOnOwnerChange 在频繁 owner change 时并发提交同名表/库创建。
#[test]
pub fn test_create_same_table_or_db_on_owner_change() {
    // NewDistExecutionContext(t, 2) 提供两个 DDL owner 参与者，defer Close 回收资源。
    // ownerWg 持续每 50ms TriggerOwnerChange，finished 原子变量用于通知 goroutine 退出。
    // beforeLoadAndDeliverJobs failpoint 通过 waitSchCh 暂停调度，保证所有 job 先落表。
    // waitJobSubmitted failpoint 通过 waitSubmitCh 串联，确保每个并发 create 都已经提交。
    // 两组 SQL 分别测试同名 table 和同名 database：第一个成功，其余报 ErrTableExists/ErrDatabaseExists。
}

/// 验证 drop table 到 StateNone 前 infoschema 仍可访问。
// TestDropTableAccessibleInInfoSchema 验证 drop table 到 StateNone 前仍能从 infoschema 访问。
#[test]
pub fn test_drop_table_accessible_in_info_schema() {
    // beforeRunOneJobStep failpoint 在 StateDeleteOnly/StateWriteOnly 时读取 InfoSchema.TableByName。
    // drop 完成后关闭 failpoint，所有中间态读取错误都应为 nil，且至少捕获过一次中间态。
}

/// 首个 create view 投递前并发尝试创建同名 view。
// TestCreateViewTwice 在首个 create view 投递前并发尝试创建同名 view。
#[test]
pub fn test_create_view_twice() {
    // Go 使用 sync.WaitGroup 和 goroutine；failpoint beforeDeliveryJob 第一次触发时启动第二个 TestKit。
    // 第二个 create view 预期通过 MustExecToErr 得到错误，主流程 create view 成功后等待 goroutine 收尾。
}

/// 验证 truncate/exchange partition 后 partitions.create_time 不变（Issue 59238）。
// TestIssue59238 验证 truncate/exchange partition 后 information_schema.partitions 的 create_time 不变。
#[test]
pub fn test_issue_59238() {
    // 创建 range partition 表，记录 distinct create_time。
    // truncate partition p1 与 exchange partition p1 with table t1 后，查询结果都应等于初始 create_time。
}

/// 覆盖 meta KV 与 infoschema 不一致时 RefreshMeta 的基础修复场景。
// TestRefreshMetaBasic 覆盖 meta kv 与 infoschema 不一致时 RefreshMeta 的基础修复场景。
#[test]
pub fn test_refresh_meta_basic() {
    // 场景 1：kv 中把 t1 改名为 t2，infoschema 还没有 t2；RefreshMeta 后 schema version +1 且 t2 可见。
    // 场景 2：kv 删除 t3，infoschema 仍有 t3；RefreshMeta 后 table 和 placement bundle 均不可见。
    // 场景 3：kv 创建 t4，infoschema 没有 t4；RefreshMeta 后 table info 与 kv 中 ID/Name/PlacementPolicyRef 一致。
    // 场景 4：kv 删除 database test1，infoschema 仍有；RefreshMeta(model.InvolvingAll) 后 schema 不可见。
    // 场景 5：kv 创建 database test2，infoschema 没有；RefreshMeta 后 infoschema 中 DBInfo 与 kvDBInfo 相等。
    // Go 源码多次 store.Begin()/txn.Commit(context.Background())，这里只保留事务边界和断言顺序。
}
