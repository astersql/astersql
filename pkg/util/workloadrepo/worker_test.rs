// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 工作负载仓库 Worker 相关单元测试。
//
// 对应 Go `pkg/util/workloadrepo/worker_test.go`。用内存后端模拟建表、分区、
// etcd KV 与 SQL 执行记录，覆盖多 Worker 采样、全局仓库启停、快照 SNAP_ID
// 恢复、分区增删与配置变量等场景。

#![allow(dead_code)]

use chrono::{DateTime, Duration, Local};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::worker::{repositoryTable, worker};
use crate::*;

/// 串行化依赖全局 Worker 的测试，避免互相干扰。
static GLOBAL_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 测试用内存后端：记录 SQL、维护表/分区集合与简易 etcd KV。
struct MemoryBackend {
    tables: Mutex<HashSet<String>>,
    partitions: Mutex<HashMap<String, Vec<String>>>,
    kv: Mutex<HashMap<String, String>>,
    executed: Mutex<Vec<(String, Vec<Value>)>>,
    owner: AtomicBool,
    etcd: AtomicBool,
    /// 剩余可注入的失败次数。
    failures: AtomicUsize,
    maxSnapID: Mutex<u64>,
}

impl Default for MemoryBackend {
    fn default() -> Self {
        Self {
            tables: Mutex::new(HashSet::new()),
            partitions: Mutex::new(HashMap::new()),
            kv: Mutex::new(HashMap::new()),
            executed: Mutex::new(Vec::new()),
            owner: AtomicBool::new(true),
            etcd: AtomicBool::new(true),
            failures: AtomicUsize::new(0),
            maxSnapID: Mutex::new(0),
        }
    }
}

/// 从 SQL 文本中提取形如 `pYYYYMMDD` 的分区名。
fn partitionNames(sql: &str) -> Vec<String> {
    sql.split(|character: char| character.is_whitespace() || matches!(character, ',' | '(' | ')'))
        .map(|value| value.trim_matches('`'))
        .filter(|value| {
            value.len() == 9
                && value.starts_with('p')
                && value[1..]
                    .chars()
                    .all(|character| character.is_ascii_digit())
        })
        .map(str::to_string)
        .collect()
}

/// 从带反引号的 SQL 中取目标表名（第 4 个反引号片段）。
fn destinationTable(sql: &str) -> Option<String> {
    let parts = sql.split('`').collect::<Vec<_>>();
    (parts.len() >= 4).then(|| parts[3].to_string())
}

impl RepositoryBackend for MemoryBackend {
    fn execute(&self, sql: &str, args: &[Value]) -> Result<Vec<Row>, String> {
        // 按 failures 计数注入瞬时失败，模拟 execRetry 场景。
        if self
            .failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_sub(1)
            })
            .is_ok()
        {
            return Err("injected failure".into());
        }
        self.executed
            .lock()
            .unwrap()
            .push((sql.into(), args.to_vec()));
        if sql.starts_with("SELECT MAX") {
            return Ok(vec![vec![Value::UInt(*self.maxSnapID.lock().unwrap())]]);
        }
        // 根据 DDL 关键字同步内存中的表与分区状态。
        if sql.starts_with("CREATE TABLE") {
            if let Some(table) = destinationTable(sql) {
                self.tables.lock().unwrap().insert(table.clone());
                self.partitions
                    .lock()
                    .unwrap()
                    .entry(table)
                    .or_default()
                    .extend(partitionNames(sql));
            }
        } else if sql.contains(" ADD PARTITION ") {
            if let Some(table) = destinationTable(sql) {
                self.partitions
                    .lock()
                    .unwrap()
                    .entry(table)
                    .or_default()
                    .extend(partitionNames(sql));
            }
        } else if sql.contains(" DROP PARTITION ") {
            if let Some(table) = destinationTable(sql) {
                if let Some(name) = sql
                    .split("DROP PARTITION ")
                    .nth(1)
                    .map(str::trim)
                    .map(|name| name.trim_matches('`'))
                {
                    self.partitions
                        .lock()
                        .unwrap()
                        .entry(table)
                        .or_default()
                        .retain(|partition| partition != name);
                }
            }
        }
        Ok(Vec::new())
    }
    fn source_columns(&self, _: &str, _: &str) -> Result<Vec<ColumnDefinition>, String> {
        Ok(vec![ColumnDefinition {
            name: "VALUE".into(),
            type_description: "BIGINT".into(),
            comment: "sample".into(),
        }])
    }
    fn table_exists(&self, table: &str) -> bool {
        self.tables.lock().unwrap().contains(table)
    }
    fn partitions(&self, table: &str) -> Result<Vec<String>, String> {
        Ok(self
            .partitions
            .lock()
            .unwrap()
            .get(table)
            .cloned()
            .unwrap_or_default())
    }
    fn instance_id(&self) -> Result<String, String> {
        Ok("node-1".into())
    }
    fn is_owner(&self) -> bool {
        self.owner.load(Ordering::SeqCst)
    }
    fn etcd_available(&self) -> bool {
        self.etcd.load(Ordering::SeqCst)
    }
    fn kv_create(&self, key: &str, value: &str) -> Result<bool, String> {
        let mut kv = self.kv.lock().unwrap();
        if kv.contains_key(key) {
            Ok(false)
        } else {
            kv.insert(key.into(), value.into());
            Ok(true)
        }
    }
    fn kv_get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.kv.lock().unwrap().get(key).cloned())
    }
    fn kv_cas(&self, key: &str, old: &str, new: &str) -> Result<bool, String> {
        let mut kv = self.kv.lock().unwrap();
        if kv.get(key).is_some_and(|value| value == old) {
            kv.insert(key.into(), new.into());
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// 构造测试 Worker；`testWorker=true` 时仅挂一张 PROCESSLIST 采样表。
fn setupWorkerForTest(backend: Arc<MemoryBackend>, testWorker: bool) -> Arc<worker> {
    let backend_trait: Arc<dyn RepositoryBackend> = backend;
    initializeWorker(
        backend_trait,
        if testWorker {
            vec![repositoryTable {
                schema: "INFORMATION_SCHEMA".into(),
                table: "PROCESSLIST".into(),
                tableType: samplingTable,
                ..Default::default()
            }]
        } else {
            defaultWorkloadTables()
        },
    )
}
/// 占位：返回内存 etcd 地址字符串。
fn setupEtcd() -> String {
    "memory://etcd".into()
}
/// 构造默认后端、当前时间与实例 ID 三元组。
fn setupDomainAndContext() -> (Arc<MemoryBackend>, DateTime<Local>, String) {
    (
        Arc::new(MemoryBackend::default()),
        Local::now(),
        "node-1".into(),
    )
}
/// `setupWorkerForTest` 的薄封装。
fn setupWorker(backend: Arc<MemoryBackend>, testWorker: bool) -> Arc<worker> {
    setupWorkerForTest(backend, testWorker)
}
/// 立即求值谓词（对应 Go eventually 在测试中的简化）。
fn eventuallyWithLock(worker: &worker, predicate: impl Fn(&worker) -> bool) -> bool {
    predicate(worker)
}
/// 立即求值谓词。
fn trueWithLock(worker: &worker, predicate: impl Fn(&worker) -> bool) -> bool {
    predicate(worker)
}
/// 等待/检查目标表与分区是否就绪。
fn waitForTables(worker: &worker, now: DateTime<Local>) -> bool {
    worker.checkTablesExists(now)
}

/// 两个 Worker 并发 start，验证目标表被创建。
#[test]
fn TestRaceToCreateTablesWorker() {
    let backend = Arc::new(MemoryBackend::default());
    let first = setupWorker(backend.clone(), true);
    let second = setupWorker(backend, true);
    first.start().unwrap();
    second.start().unwrap();
    assert!(first.backend.table_exists("HIST_PROCESSLIST"));
}

/// 统计针对指定历史表的 INSERT 次数。
fn getMultipleWorkerCount(backend: &MemoryBackend, worker: &str) -> usize {
    backend
        .executed
        .lock()
        .unwrap()
        .iter()
        .filter(|(sql, _)| sql.starts_with("INSERT ") && sql.contains(worker))
        .count()
}

/// 两个 Worker 各自采样一次，应产生两次 INSERT。
#[test]
fn TestMultipleWorker() {
    let backend = Arc::new(MemoryBackend::default());
    let first = setupWorker(backend.clone(), true);
    let second = setupWorker(backend.clone(), true);
    first.start().unwrap();
    second.start().unwrap();
    first.startSample()().unwrap();
    second.startSample()().unwrap();
    assert_eq!(getMultipleWorkerCount(&backend, "HIST_PROCESSLIST"), 2);
}

/// 全局 Setup/Stop 与 setRepositoryDest 启停行为。
#[test]
fn TestGlobalWorker() {
    let _guard = GLOBAL_TEST_LOCK.lock().unwrap();
    StopRepository();
    let backend = Arc::new(MemoryBackend::default());
    let worker = setupWorker(backend, true);
    SetupRepository(worker.clone()).unwrap();
    worker.setRepositoryDest("table").unwrap();
    assert!(worker.enabled());
    StopRepository();
    assert!(!worker.enabled());
}

/// 未启动时 takeSnapshot 失败；启用后应得到 SNAP_ID=1。
#[test]
fn TestAdminWorkloadRepo() {
    let _guard = GLOBAL_TEST_LOCK.lock().unwrap();
    StopRepository();
    let backend = Arc::new(MemoryBackend::default());
    let worker = setupWorker(backend, true);
    assert!(takeSnapshot().is_err());
    SetupRepository(worker.clone()).unwrap();
    worker.setRepositoryDest("table").unwrap();
    assert_eq!(takeSnapshot().unwrap(), 1);
    StopRepository();
}

/// 过滤出包含指定子串的已执行 SQL 记录。
fn getRows(backend: &MemoryBackend, query: &str) -> Vec<(String, Vec<Value>)> {
    backend
        .executed
        .lock()
        .unwrap()
        .iter()
        .filter(|(sql, _)| sql.contains(query))
        .cloned()
        .collect()
}
/// 断言时间差在 maxSecs 内并返回该时间。
fn validateDate(value: DateTime<Local>, last: DateTime<Local>, maxSecs: i64) -> DateTime<Local> {
    assert!((value - last).num_seconds().abs() <= maxSecs);
    value
}
/// 触发一次采样闭包。
fn SamplingTimingWorker(worker: &worker) -> Result<(), String> {
    worker.startSample()()
}

/// 采样后应写入一条 HIST_PROCESSLIST INSERT。
#[test]
fn TestSamplingTimingWorker() {
    let backend = Arc::new(MemoryBackend::default());
    let worker = setupWorker(backend.clone(), true);
    worker.start().unwrap();
    SamplingTimingWorker(&worker).unwrap();
    assert_eq!(
        getRows(&backend, "INSERT `WORKLOAD_SCHEMA`.`HIST_PROCESSLIST`").len(),
        1
    );
}

/// 判断执行记录参数中是否包含指定 snapID。
fn findMatchingRowForSnapshot(rows: &[(String, Vec<Value>)], snapID: u64) -> bool {
    rows.iter()
        .any(|(_, args)| args.contains(&Value::UInt(snapID)))
}
/// 触发指定 snapID 的快照闭包。
fn SnapshotTimingWorker(worker: &worker, snapID: u64) -> Result<(), String> {
    worker.startSnapshot(snapID)()
}

/// 快照执行应把 SNAP_ID 绑定进 INSERT 参数。
#[test]
fn TestSnapshotTimingWorker() {
    let backend = Arc::new(MemoryBackend::default());
    let mut table = repositoryTable {
        schema: "INFORMATION_SCHEMA".into(),
        table: "TIDB_INDEX_USAGE".into(),
        tableType: snapshotTable,
        ..Default::default()
    };
    table.destTable = "HIST_TIDB_INDEX_USAGE".into();
    backend
        .tables
        .lock()
        .unwrap()
        .insert(table.destTable.clone());
    backend.partitions.lock().unwrap().insert(
        table.destTable.clone(),
        vec![generatePartitionName(Local::now() + Duration::days(2))],
    );
    let worker = initializeWorker(backend.clone(), vec![table]);
    worker.readInstanceID().unwrap();
    SnapshotTimingWorker(&worker, 7).unwrap();
    assert!(findMatchingRowForSnapshot(
        &backend.executed.lock().unwrap(),
        7
    ));
}

/// stop 后再 start 应重新进入 started 状态。
#[test]
fn TestStoppingAndRestartingWorker() {
    let backend = Arc::new(MemoryBackend::default());
    let worker = setupWorker(backend, true);
    worker.start().unwrap();
    assert!(worker.started());
    worker.stop();
    assert!(!worker.started());
    worker.start().unwrap();
    assert!(worker.started());
}

/// 采样/快照间隔与保留天数配置读写及 dest 校验。
#[test]
fn TestSettingSQLVariables() {
    let worker = setupWorker(Arc::new(MemoryBackend::default()), true);
    worker.start().unwrap();
    worker.changeSamplingInterval("10").unwrap();
    worker.changeSnapshotInterval("900").unwrap();
    worker.setRetentionDays("30").unwrap();
    assert_eq!(worker.intervals(), (10, 900, 30));
    assert!(validateDest("TABLE").is_ok());
    assert!(validateDest("s3").is_err());
}

/// 默认快照元数据表 DDL 必须保留 Go 版本的完整列契约。
#[test]
fn default_snapshot_metadata_ddl_matches_go_columns() {
    let tables = defaultWorkloadTables();
    let metadata = tables
        .iter()
        .find(|table| table.tableType == metadataTable)
        .unwrap();

    for column in [
        "`SNAP_ID`",
        "`BEGIN_TIME`",
        "`END_TIME`",
        "`DB_VER`",
        "`WR_VER`",
        "`SOURCE`",
        "`ERROR`",
    ] {
        assert!(
            metadata.createStmt.contains(column),
            "missing {column} in {}",
            metadata.createStmt
        );
    }
    assert!(
        metadata
            .createStmt
            .contains("Global unique identifier of the snapshot")
    );
    assert!(
        metadata
            .createStmt
            .contains("Versions of TiDB, TiKV, PD at the moment")
    );
}

/// Go errors.Join 用换行连接五次执行失败。
#[test]
fn exec_retry_joins_all_five_errors_like_go() {
    let backend = MemoryBackend::default();
    backend.failures.store(5, Ordering::SeqCst);

    assert_eq!(
        execRetry(&backend, "SELECT 1", &[]).unwrap_err(),
        ["injected failure"; 5].join("\n")
    );
}

/// 按 destTable 名查找仓库表描述。
fn getTable(tableName: &str, worker: &worker) -> repositoryTable {
    worker
        .workloadTables
        .lock()
        .unwrap()
        .iter()
        .find(|table| table.destTable == tableName)
        .unwrap()
        .clone()
}
/// 断言后端分区列表等于期望值。
fn validatePartitionsMatchExpected(backend: &MemoryBackend, table: &str, expected: &[String]) {
    assert_eq!(backend.partitions(table).unwrap(), expected);
}
/// 生成单个分区名。
fn buildPartitionRow(now: DateTime<Local>) -> String {
    generatePartitionName(now)
}
/// 将多个时间点格式化为逗号分隔的分区名串。
fn buildPartitionString(partitions: &[DateTime<Local>]) -> String {
    partitions
        .iter()
        .map(|time| generatePartitionName(*time))
        .collect::<Vec<_>>()
        .join(",")
}
/// 在内存后端预置表及其分区列表。
fn createTableWithParts(backend: &MemoryBackend, table: &repositoryTable, partitions: Vec<String>) {
    backend
        .tables
        .lock()
        .unwrap()
        .insert(table.destTable.clone());
    backend
        .partitions
        .lock()
        .unwrap()
        .insert(table.destTable.clone(), partitions);
}
/// 调用 createAllPartitions。
fn validatePartitionCreation(worker: &worker, now: DateTime<Local>) {
    worker.createAllPartitions(now).unwrap();
}

/// 空分区表应被补齐未来分区（期望 2 个）。
#[test]
fn TestCreatePartition() {
    let backend = Arc::new(MemoryBackend::default());
    let table = repositoryTable {
        destTable: "HIST_T".into(),
        ..Default::default()
    };
    createTableWithParts(&backend, &table, Vec::new());
    let worker = initializeWorker(backend.clone(), vec![table]);
    validatePartitionCreation(&worker, Local::now());
    assert_eq!(backend.partitions("HIST_T").unwrap().len(), 2);
}

/// 调用 dropOldPartitions。
fn validatePartitionDrop(worker: &worker, now: DateTime<Local>, retention: i32) {
    worker.dropOldPartitions(now, retention).unwrap();
}

/// 超过保留期的旧分区应被删除，近期分区保留。
#[test]
fn TestDropOldPartitions() {
    let backend = Arc::new(MemoryBackend::default());
    let table = repositoryTable {
        destTable: "HIST_T".into(),
        ..Default::default()
    };
    let old = generatePartitionName(Local::now() - Duration::days(10));
    let recent = generatePartitionName(Local::now() + Duration::days(2));
    createTableWithParts(&backend, &table, vec![old, recent.clone()]);
    let worker = initializeWorker(backend.clone(), vec![table]);
    validatePartitionDrop(&worker, Local::now(), 7);
    validatePartitionsMatchExpected(&backend, "HIST_T", &[recent]);
}

/// start 后表与分区应就绪。
#[test]
fn TestAddNewPartitionsOnStart() {
    let backend = Arc::new(MemoryBackend::default());
    let worker = setupWorker(backend.clone(), true);
    worker.start().unwrap();
    assert!(waitForTables(&worker, Local::now()));
}

/// 计算下一次管家任务触发间隔。
fn getNextTick(now: DateTime<Local>) -> Duration {
    calcNextTick(now)
}

/// 启动后执行一次 housekeeper 闭包应成功。
#[test]
fn TestHouseKeeperThread() {
    let backend = Arc::new(MemoryBackend::default());
    let worker = setupWorker(backend, true);
    worker.start().unwrap();
    worker.startHouseKeeper(Local::now())().unwrap();
}

/// calcNextTick 应落在 (0, 1 天] 区间。
#[test]
fn TestCalcNextTick() {
    let duration = getNextTick(Local::now());
    assert!(duration > Duration::zero());
    assert!(duration <= Duration::days(1));
}

/// 非 owner 节点 start 应失败（无法建表就绪）。
#[test]
fn TestOwnerRandomDown() {
    let backend = Arc::new(MemoryBackend::default());
    backend.owner.store(false, Ordering::SeqCst);
    let worker = setupWorker(backend, true);
    assert!(worker.start().is_err());
}

/// etcd 无 snapID 时从 SQL MAX 恢复并递增写入 KV。
#[test]
fn TestRecoverSnapID() {
    let backend = Arc::new(MemoryBackend::default());
    *backend.maxSnapID.lock().unwrap() = 41;
    let worker = setupWorker(backend.clone(), true);
    assert_eq!(worker.takeSnapshot().unwrap(), 42);
    assert_eq!(backend.kv_get(snapIDKey).unwrap(), Some("42".into()));
}
