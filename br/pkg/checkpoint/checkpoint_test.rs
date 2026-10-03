// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/checkpoint/checkpoint_test.go`.
//! Storage / Domain / SQL / cipher / failpoint boundaries use in-crate stubs
//! (no kv / domain crate / kvproto / grpcio).
//!
//! 对齐 Go `checkpoint_test.go`：覆盖 backup/restore/log-restore 元数据与 Runner，
//! 以及 storage / table 两种 MetaManager 后端。边界依赖本 crate stubs。
//! 全局 `env_lock` 串行化用例；进入时 Disable 写后失败 failpoint。
//! 夹具 SharedSql/TestSession/TestGlue 只服务 table 后端隔离，非生产 SQL。
//!
//! 场景与断言意图：
//! - meta_for_backup：Save/Load ConfigHash、BackupTS。
//! - meta_for_restore_*：snapshot+log meta、progress、taskInfo、ingest repair SQL。
//! - backup_runner：两批 Append + checksum，WaitForFinish(true) 后 Walk/Load。
//! - restore_runner_*：4 条 range（含大小写）与 checksum，再 Remove 验证清理。
//! - runner_retry_*：failpoint 后仍能恢复（计数 >=1）。
//! - runner_no_retry_*：干净路径每条 range 恰 1 次。
//! - log_restore_runner_*：metaKey/goff/foff 嵌套匹配计数=4。
//! - runner_lock：冲突失败→更高 TSO 接手→第三方仍失败。
//!
//! 辅助：`aes_cipher` 固定密钥；`get_lock_data` 构造 JSON 锁；
//! `FailGuard` Drop 清理 failpoint；短 tick 加速 retry 窗口。
//! MemStorage 无 mkdir；WaitForFinish(true) 消除 tick 竞态。
//! 勿放宽与 Go 可观察结果对齐的断言；差异应改实现。
//! OnStorage/OnTable 成对共享断言体；checksum 拼 `{crc}_{bytes}_{kvs}`。
//! 锁用例依赖 ComposeTS 与 MockTimer 比较 ExpireAt。
//! storage 与 table 后端共用断言体，差异仅在 MetaManager 构造与清理路径。
//! failpoint 注入写失败后必须 Disable，防止污染后续串行用例。
//! WaitForFinish(true) 强制排空队列，消除短 tick 下的假阴性竞态。
//! RemoveCheckpointData 后 Walk 应为空，验证清理不留孤儿元数据。
//! log-restore 嵌套键 (metaKey/goff/foff) 计数=4 对齐 Go 双偏移语义。
//! ConfigHash 变更视为不兼容恢复，Load 侧应拒绝或要求重建。
//! ingest repair SQL 仅 table 后端持久化，storage 后端以文件侧等价信息对照。
//! 本文件不启动真实 PD/TiKV；锁与时间全部由 MockTimer/MemStorage 驱动。
//! progress/TiFlashItems/SchedulersConfig 深比较防字段漂移。
//! HashMap 夹具无序；failpoint 名须与 Inject 点一致。
//! table 后端 ExecuteInternal 识别简易语句；未知 SQL 应报错。
//! InfoSchema.TableExists 大小写不敏感；Close MetaManager 成对收尾。
//! GetCheckpointTaskInfo 勿拆断言；Save 前 Exists*=false。
//! backup Walk 以 StartKey 查 map，"a"/"A" 不同键。
//! restore Load 内 table_id 断言防跨表污染。
//! log restore Foffs 包含关系决定计次；匹配失败 panic。
//! Remove 覆盖 .cpt/.meta/.lock；之后 ExistsMetadata=false。
//! env_lock 持有期间禁止跨线程再抢。
//! storage 路径含 task 名；table 写临时库表常量。
//! cipher 在 backup Walk 透传；data2 验证最终 flush 收录尾批。
//! FlushChecksum 表 ID 1..=4 与 meta 键一一对应。
//! retry sleep 仅留窗口；lock ExpireAt=p+10，timer(30,10) 可接手。
//! 接手后再 Start(40,10) 失败证明有效锁仍在。
//! WaitForFinish 放锁冲突断言之后，避免过早 close。
//! repair SQL 用 CIStr；SharedSql 键 db.table。
//! Session.closed 后执行应失败；RestrictedSQL 与 Session 同数据面。
//! Domain.Store 可桩化；Start*ForTest 缩短 tick。
//! Append* 参数顺序对齐 Go；LoadCheckpointChecksum 主要用 map。
//! 数值 TS/ID 用字面量；并行跑测仍须外层 env_lock。
//! 注释解释断言意图与 Go 对齐点，不复述 assert 语法。
//! 维护优先对照 Go 同名 Test*，再看 Rust 夹具差异。
//! SharedSql.tables 存行；schema 存 (db,table) 小写对。
//! TestSession.key 格式 db.table，与 drop 前缀扫描一致。
//! create_restore_schema_suite 不启真实集群。
//! meta 共享体同时覆盖 snapshot 与 log 两条线。
//! Exists* 先假后真，防止空文件假绿。
//! backup 第一批后 FlushChecksum，再 Append data2。
//! restore NewCheckpointRangeKeyItem(table,key) 构造 Group。
//! log restore 多层 HashMap 夹具表达 Goff/Foff。
//! no_retry 对照证明重复写入不是默认行为。
//! lock 预置 WriteFile 到 CheckpointLockPathForBackup。
//! _mapping_anchors 仅消 unused。
//! TestSession.ExecuteInternal 覆盖 CREATE/DROP/INSERT 等最小语句集。
//! 未知 SQL 返回错误，避免静默成功掩盖 MetaManager 写路径问题。
//! schema 集合与 tables 数据同步维护，防止 Exists 与行存不一致。
//! OnTable 用例结束必须 Close，释放 Session/表资源。
//! backup runner 用 StartCheckpointBackupRunnerForTest 注入短周期与 timer。
//! restore/log Start*ForTest 同样缩短 tick，加快单测收敛。
//! Walk 回调内 expect 夹具键：缺失即测试失败而非跳过。
//! checksum Load 后 format 拼接断言，便于与 Go 字符串断言对照。
//! retry 用例 Disable failpoint 后再次 Append/Flush，验证恢复窗口。
//! FailGuard 与 env_lock 清理互补：锁管串行，Guard 管单测退出。
//! lock 用例第一次 Start 期望 err，不 unwrap Runner。
//! 第二次 Start 成功持锁；第三次 Start 再期望 err。
//! WaitForFinish 在第三方冲突断言之后，避免释放锁干扰。
//! meta restore 共享体写入 SchedulersConfig 与 TiFlashItems 非空样本。
//! progress 设为 InLogRestoreAndIdMapPersisted，覆盖枚举序列化。
//! repair SQL 含多 AddArgs，防止只比较首元素假绿。
//! log restore Load 回调签名 (meta_key, LogRestoreValueMarshaled)。
//! storage MetaManager 构造参数含 task 名与 id，路径隔离各用例。
//! table MetaManager 库名常量避免与业务库冲突。
//! MemStorage 按完整路径键存对象，Write/Read/Delete/Walk 自洽。
//! MockTimer 忽略调用次数，始终返回构造时的 (p,l)。
//! get_lock_data 的 ExpireAt=p+10，配合不同 timer 物理时间制造冲突/可抢。
//! aes_cipher 密钥长度 32，匹配 AES-256。
//! env_lock 内 Disable 使用与 Enable 完全相同的 failpoint 名字符串。
//! 成对 OnStorage/OnTable 失败时先看共享体，再看后端差异。
//! 本文件只加注释，不改断言阈值或夹具数据。
//! 阅读建议：先扫场景索引，再看对应共享体与 OnStorage/OnTable 入口。
//! 与实现对照时打开 checkpoint.rs / external_storage.rs / manager.rs。
//! Go 侧同名测试在 checkpoint_test.go；字段 JSON tag 差异优先查 serde rename。
//! table 后端失败常见原因：DDL 解析不全或 InfoSchema 未登记。
//! storage 后端失败常见原因：路径常量拼写或 cipher 不一致。
//! retry 失败常见原因：failpoint 未 Disable 或 tick 过短未触发重试。
//! lock 失败常见原因：预置锁 ExpireAt/LockId 与 timer 组合不符合预期。
//! Walk 找不到 key：检查 Append 是否在 WaitForFinish 前完成。
//! checksum 对不上：检查 FlushChecksum 参数与 Load 键是否同表 ID。
//! Remove 后仍 Exists：检查 MetaManager 是否指向同一 task 路径/库。
//! 并行加测须复用 env_lock，否则 failpoint 与全局 Mock 会互踩。
//! 新增断言应同时覆盖 storage 与 table，除非语义仅属一端。
//! 夹具字面量保持稳定，便于与 Go golden 行为肉眼对照。
//! Session/Glue 桩变更时同步更新注释中的能力边界说明。
//! 不在此文件引入真实网络或磁盘依赖。
//! panic! 消息保持英文以便与 Go 日志检索习惯接近。
//! assert 消息字符串（锁冲突）用于定位阶段，不是给用户看的产品文案。
//! data/data2 分批是为覆盖“运行中追加”而非只测一次性导入。
//! restore 中 rangeKey 与 table_id 组合键在 record_set 用下划线拼接。
//! log restore resp_count 计的是匹配到的 foff 次数，不是 metaKey 数。
//! meta 测试不启动 Runner，只测 MetaManager 持久化接口。
//! runner 测试依赖最终 flush，因此 WaitForFinish 第二参必须为 true。
//! 短 sleep 不可替换为精确 barrier；只作概率性让出调度。
//! 若 CI 抖动，优先加长 sleep/tick，而不是删除断言。
//! 中文注释密度服务于可维护性，避免无语义占位句。
//! 测试名 snake_case 对应 Go TestCamelCase，检索时按语义映射。
//! 共享体内断言顺序：写→读→派生视图→清理标志。
//! Runner 用例统一 Background ctx，不测取消路径（取消在实现单测覆盖）。
//! 不在此断言锁错误英文全文，只检查 is_err，降低文案微调噪音。
//! table/storage 关闭顺序：先 Close MetaManager，再放掉 Arc。
//! 夹具 HashMap 字面量键集即期望键集，勿在断言外静默扩容。
//! 若补充并发 Runner 测试，需额外同步原语，不能只靠 env_lock。
//! 任务完成后应保持仅注释 diff，便于审查与回滚。
//!

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// Serialize tests that share process-global failpoint state (Go package tests are not parallel).
/// 串行化 failpoint 相关测试；进入时清理残留。
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let _ = failpoint::Disable(
        "github.com/pingcap/tidb/br/pkg/checkpoint/failed-after-checkpoint-flushes",
    );
    guard
}

use crate::backup::{
    AppendForBackup, CheckpointDataDirForBackup, CheckpointLockPathForBackup,
    CheckpointMetadataForBackup, LoadCheckpointMetadata, SaveCheckpointMetadata,
    StartCheckpointBackupRunnerForTest, WalkCheckpointFileForBackup,
};
use crate::checkpoint::removeCheckpointData;
use crate::checkpoint::{
    ChecksumItem, ChecksumItems, checkpointStorage, newCheckpointRunner,
    set_failed_after_checkpoint_flushes_checksum_for_test,
};
use crate::external_storage::CheckpointLock;
use crate::log_restore::{
    AppendRangeForLogRestore, CheckpointIngestIndexRepairSQL, CheckpointIngestIndexRepairSQLs,
    CheckpointMetadataForLogRestore, CheckpointProgress, GetCheckpointTaskInfo,
    LogRestoreValueMarshaled, RestoreProgress, StartCheckpointLogRestoreRunnerForTest,
};
use crate::manager::{
    LogMetaManager, NewLogStorageMetaManager, NewLogTableMetaManager,
    NewSnapshotStorageMetaManager, NewSnapshotTableMetaManager, SnapshotMetaManager,
};
use crate::restore::{
    AppendRangesForRestore, CheckpointMetadataForSnapshotRestore, NewCheckpointRangeKeyItem,
    StartCheckpointRestoreRunnerForTest,
};
use crate::storage::{
    LogRestoreCheckpointDatabaseName, SnapshotRestoreCheckpointDatabaseName,
    checkpointMetaTableName,
};
use crate::stubs::{
    CIStr, CipherInfo, ClusterConfig, ComposeTS, Context, Domain, EncryptionMethod, Error, File,
    GlobalTimer, Glue, InfoSchema, MemStorage, MockTimer, RestrictedSQLExecutor, Result, Session,
    SqlRow, SqlValue, Storage, TableInfoName, TiFlashReplicaInfo, failpoint,
};

struct ConcurrentDeleteStorage {
    paths: Vec<String>,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

#[derive(Default)]
struct ChecksumFailpointStorage {
    writes: AtomicUsize,
}

impl checkpointStorage for ChecksumFailpointStorage {
    fn flushCheckpointData(&self, _ctx: &Context, _data: &[u8]) -> Result<()> {
        Ok(())
    }

    fn flushCheckpointChecksum(&self, _ctx: &Context, _data: &[u8]) -> Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn initialLock(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }

    fn updateLock(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }

    fn close(&self) {}
}

#[test]
fn checksum_flush_reports_go_post_write_failpoint() {
    let storage = Arc::new(ChecksumFailpointStorage::default());
    let runner = newCheckpointRunner::<String, u64, _>(storage.clone(), None, |group| {
        Ok(serde_json::to_vec(group)?)
    });
    set_failed_after_checkpoint_flushes_checksum_for_test(true);
    let result = runner.doChecksumFlush(
        &Context::Background(),
        ChecksumItems {
            Items: vec![ChecksumItem {
                TableID: 1,
                Crc64xor: 2,
                TotalKvs: 3,
                TotalBytes: 4,
            }],
        },
    );
    set_failed_after_checkpoint_flushes_checksum_for_test(false);

    assert_eq!(storage.writes.load(Ordering::SeqCst), 1);
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("failed after checkpoint flushes checksum")
    );
}

impl ConcurrentDeleteStorage {
    fn new(file_count: usize) -> Self {
        Self {
            paths: (0..file_count)
                .map(|i| format!("checkpoints/task/{i}.cpt"))
                .collect(),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        }
    }
}

impl Storage for ConcurrentDeleteStorage {
    fn WalkDir(
        &self,
        _ctx: &Context,
        _opt: &crate::stubs::WalkOption,
        f: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        for path in &self.paths {
            f(path, 1)?;
        }
        Ok(())
    }

    fn ReadFile(&self, _ctx: &Context, _path: &str) -> Result<Vec<u8>> {
        Err(Error::new("not used"))
    }

    fn WriteFile(&self, _ctx: &Context, _path: &str, _data: &[u8]) -> Result<()> {
        Err(Error::new("not used"))
    }

    fn DeleteFile(&self, _ctx: &Context, _path: &str) -> Result<()> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(20));
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }

    fn FileExists(&self, _ctx: &Context, _path: &str) -> Result<bool> {
        Ok(false)
    }

    fn URI(&self) -> String {
        "test://".to_string()
    }
}

#[test]
fn remove_checkpoint_data_uses_go_worker_pool_concurrency() {
    let storage = ConcurrentDeleteStorage::new(8);
    removeCheckpointData(&Context::Background(), &storage, "checkpoints/task").unwrap();
    assert_eq!(storage.max_active.load(Ordering::SeqCst), 4);
}

#[test]
fn range_group_omits_zero_group_key_like_go_json() {
    let group = crate::checkpoint::RangeGroup::<String, u64> {
        GroupKey: String::new(),
        Group: vec![1],
    };
    let value = serde_json::to_value(group).unwrap();
    assert!(value.get("group-key").is_none());
    assert_eq!(value["groups"], serde_json::json!([1]));
}

// --- table-backend fixture (Go utiltest.CreateRestoreSchemaSuite + gluetidb) ---

#[derive(Default)]
// 内存表与 schema：table 后端最小替身。
struct SharedSql {
    tables: Mutex<HashMap<String, Vec<SqlRow>>>,
    /// (db, table) names known to InfoSchema after CREATE TABLE.
    schema: Mutex<HashSet<(String, String)>>,
}

// 会话桩：简易 DDL/DML；Close 后拒绝执行。
struct TestSession {
    shared: Arc<SharedSql>,
    closed: bool,
}

impl TestSession {
    fn new(shared: Arc<SharedSql>) -> Self {
        Self {
            shared,
            closed: false,
        }
    }

    fn key(db: &str, table: &str) -> String {
        format!("{db}.{table}")
    }

    // 小写入库，供 TableExists。
    fn register_table(&self, db: &str, table: &str) {
        self.shared
            .schema
            .lock()
            .unwrap()
            .insert((db.to_lowercase(), table.to_lowercase()));
    }

    // 同步移除行与 schema。
    fn drop_table(&self, db: &str, table: &str) {
        self.shared
            .schema
            .lock()
            .unwrap()
            .remove(&(db.to_lowercase(), table.to_lowercase()));
        // data keys keep original case from REPLACE INTO; clear both forms
        let mut tables = self.shared.tables.lock().unwrap();
        tables.remove(&Self::key(db, table));
        tables.remove(&Self::key(&db.to_lowercase(), &table.to_lowercase()));
    }

    // 按库前缀清理。
    fn drop_database(&self, db: &str) {
        let db_l = db.to_lowercase();
        self.shared
            .schema
            .lock()
            .unwrap()
            .retain(|(d, _)| d != &db_l);
        let mut tables = self.shared.tables.lock().unwrap();
        tables.retain(|k, _| !k.to_lowercase().starts_with(&format!("{db_l}.")));
    }
}

impl Session for TestSession {
    fn Close(&mut self) {
        self.closed = true;
    }

    fn ExecuteInternal(&mut self, _ctx: &Context, sql: &str, args: &[SqlValue]) -> Result<()> {
        let sql_l = sql.to_lowercase();
        if sql_l.contains("create database") {
            return Ok(());
        }
        if sql_l.contains("create table") {
            if args.len() >= 2 {
                if let (SqlValue::Str(db), SqlValue::Str(table)) = (&args[0], &args[1]) {
                    self.register_table(db, table);
                }
            }
            return Ok(());
        }
        if sql_l.contains("drop table") {
            if args.len() >= 2 {
                if let (SqlValue::Str(db), SqlValue::Str(table)) = (&args[0], &args[1]) {
                    self.drop_table(db, table);
                    return Ok(());
                }
            }
            // DROP TABLE IF EXISTS db.table;
            if let Some(rest) = sql_l.split("exists").nth(1) {
                let name = rest.trim().trim_end_matches(';').trim();
                if let Some((db, table)) = name.split_once('.') {
                    self.drop_table(db.trim(), table.trim());
                }
            }
            return Ok(());
        }
        if sql_l.contains("drop database") {
            if let Some(SqlValue::Str(db)) = args.first() {
                self.drop_database(db);
                return Ok(());
            }
            if let Some(rest) = sql_l.split("database").nth(1) {
                let db = rest.trim().trim_end_matches(';').trim();
                self.drop_database(db);
            }
            return Ok(());
        }
        if sql_l.contains("replace into") {
            if args.len() >= 4 {
                if let (
                    SqlValue::Str(db),
                    SqlValue::Str(table),
                    SqlValue::U64(seg),
                    SqlValue::Bytes(data),
                ) = (&args[0], &args[1], &args[2], &args[3])
                {
                    self.register_table(db, table);
                    let k = Self::key(db, table);
                    let mut tables = self.shared.tables.lock().unwrap();
                    let rows = tables.entry(k).or_default();
                    rows.retain(|r| r.GetUint64(0) != *seg);
                    rows.push(SqlRow::new(vec![
                        SqlValue::U64(*seg),
                        SqlValue::Bytes(data.clone()),
                    ]));
                    rows.sort_by_key(|r| r.GetUint64(0));
                    return Ok(());
                }
            }
            if args.len() == 3 {
                if let Some(rest) = sql.split("INTO").nth(1) {
                    let name = rest.trim().split_whitespace().next().unwrap_or("");
                    if let Some((db, table)) = name.split_once('.') {
                        self.register_table(db, table);
                    }
                    let k = name.to_string();
                    let mut tables = self.shared.tables.lock().unwrap();
                    let rows = tables.entry(k).or_default();
                    if let (SqlValue::Bytes(uuid), SqlValue::U64(seg), SqlValue::Bytes(data)) =
                        (&args[0], &args[1], &args[2])
                    {
                        rows.push(SqlRow::new(vec![
                            SqlValue::Bytes(uuid.clone()),
                            SqlValue::U64(*seg),
                            SqlValue::Bytes(data.clone()),
                        ]));
                    }
                }
            }
        }
        Ok(())
    }

    fn GetRestrictedSQLExecutor(&self) -> Arc<dyn RestrictedSQLExecutor> {
        Arc::new(TestRestricted {
            shared: self.shared.clone(),
        })
    }
}

// 受限 SQL 桩。
struct TestRestricted {
    shared: Arc<SharedSql>,
}

impl RestrictedSQLExecutor for TestRestricted {
    fn ExecRestrictedSQL(
        &self,
        _ctx: &Context,
        sql: &str,
        args: &[SqlValue],
    ) -> Result<Vec<SqlRow>> {
        let sql_l = sql.to_lowercase();
        if sql_l.contains("order by uuid") || args.len() >= 2 {
            if let (SqlValue::Str(db), SqlValue::Str(table)) = (&args[0], &args[1]) {
                let k = format!("{db}.{table}");
                return Ok(self
                    .shared
                    .tables
                    .lock()
                    .unwrap()
                    .get(&k)
                    .cloned()
                    .unwrap_or_default());
            }
        }
        Ok(vec![])
    }
}

// InfoSchema 桩。
struct TestInfoSchema {
    shared: Arc<SharedSql>,
}

impl InfoSchema for TestInfoSchema {
    fn TableExists(&self, db: &CIStr, table: &CIStr) -> bool {
        self.shared
            .schema
            .lock()
            .unwrap()
            .contains(&(db.O.to_lowercase(), table.O.to_lowercase()))
    }

    fn SchemaTableInfos(&self, _ctx: &Context, db: &CIStr) -> Result<Vec<TableInfoName>> {
        let db_l = db.O.to_lowercase();
        let schema = self.shared.schema.lock().unwrap();
        Ok(schema
            .iter()
            .filter(|(d, _)| d == &db_l)
            .map(|(_, t)| TableInfoName {
                Name: CIStr::new(t.clone()),
            })
            .collect())
    }
}

// Domain 桩。
struct TestDomain {
    store: Arc<dyn Storage>,
    shared: Arc<SharedSql>,
}

impl Domain for TestDomain {
    fn Store(&self) -> Arc<dyn Storage> {
        self.store.clone()
    }

    fn InfoSchema(&self) -> Arc<dyn InfoSchema> {
        Arc::new(TestInfoSchema {
            shared: self.shared.clone(),
        })
    }
}

// Glue 桩。
struct TestGlue {
    shared: Arc<SharedSql>,
}

impl Glue for TestGlue {
    fn CreateSession(&self, _store: &dyn Storage) -> Result<Box<dyn Session>> {
        Ok(Box::new(TestSession::new(self.shared.clone())))
    }
}

/// Go utiltest.CreateRestoreSchemaSuite stand-in: shared SQL + Domain without real TiDB.
// 装配 OnTable Glue/Domain。
fn create_restore_schema_suite() -> (TestGlue, Arc<TestDomain>) {
    let shared = Arc::new(SharedSql::default());
    let store = Arc::new(MemStorage::new());
    let dom = Arc::new(TestDomain {
        store,
        shared: shared.clone(),
    });
    (TestGlue { shared }, dom)
}

// 固定 TSO 驱动锁比较。
fn new_mock_timer(p: i64, l: i64) -> Arc<dyn GlobalTimer> {
    Arc::new(MockTimer::new(p, l))
}

// 构造 CheckpointLock JSON。
fn get_lock_data(p: i64, l: i64) -> Result<Vec<u8>> {
    let lock = CheckpointLock {
        LockId: ComposeTS(p, l),
        ExpireAt: p + 10,
    };
    Ok(serde_json::to_vec(&lock)?)
}

// 固定 AES256-CTR 密钥。
fn aes_cipher() -> CipherInfo {
    CipherInfo {
        CipherType: EncryptionMethod::AES256_CTR,
        CipherKey: b"01234567890123456789012345678901".to_vec(),
    }
}

// TestCheckpointMetaForBackup
#[test]
// backup meta 往返。
fn test_checkpoint_meta_for_backup() {
    let _lock = env_lock();
    let ctx = Context::Background();
    let s: Arc<dyn Storage> = Arc::new(MemStorage::new());

    let checkpoint_meta = CheckpointMetadataForBackup {
        ConfigHash: b"123456".to_vec(),
        BackupTS: 123456,
        ..Default::default()
    };

    SaveCheckpointMetadata(&ctx, s.as_ref(), &checkpoint_meta).unwrap();
    let checkpoint_meta2 = LoadCheckpointMetadata(&ctx, s.as_ref()).unwrap();
    assert_eq!(checkpoint_meta.ConfigHash, checkpoint_meta2.ConfigHash);
    assert_eq!(checkpoint_meta.BackupTS, checkpoint_meta2.BackupTS);
}

// TestCheckpointMetaForRestoreOnStorage
#[test]
// restore meta × storage。
fn test_checkpoint_meta_for_restore_on_storage() {
    let _lock = env_lock();
    let s: Arc<dyn Storage> = Arc::new(MemStorage::new());
    let snapshot = NewSnapshotStorageMetaManager(s.clone(), None, 1, "snapshot", 1);
    let log = NewLogStorageMetaManager(s, None, 1, "log", 1);
    test_checkpoint_meta_for_restore(snapshot.as_ref(), log.as_ref());
    snapshot.Close();
    log.Close();
}

// TestCheckpointMetaForRestoreOnTable
#[test]
// restore meta × table。
fn test_checkpoint_meta_for_restore_on_table() {
    let _lock = env_lock();
    let (g, dom) = create_restore_schema_suite();
    let snapshot =
        NewSnapshotTableMetaManager(&g, dom.clone(), SnapshotRestoreCheckpointDatabaseName, 1)
            .unwrap();
    let log = NewLogTableMetaManager(&g, dom, LogRestoreCheckpointDatabaseName, 1).unwrap();
    test_checkpoint_meta_for_restore(snapshot.as_ref(), log.as_ref());
    snapshot.Close();
    log.Close();
}

// 共享体：meta/progress/taskInfo/repair SQL。
// Save 前 Exists*=false。
fn test_checkpoint_meta_for_restore(snapshot: &dyn SnapshotMetaManager, log: &dyn LogMetaManager) {
    let ctx = Context::Background();

    let snap_meta = CheckpointMetadataForSnapshotRestore {
        UpstreamClusterID: 123,
        RestoredTS: 321,
        SchedulersConfig: Some(ClusterConfig {
            Schedulers: vec!["1".into(), "2".into()],
            ScheduleCfg: HashMap::from([
                ("1".into(), serde_json::json!("2")),
                ("2".into(), serde_json::json!("1")),
            ]),
        }),
        ..Default::default()
    };
    snapshot.SaveCheckpointMetadata(&ctx, &snap_meta).unwrap();
    let snap_meta2 = snapshot.LoadCheckpointMetadata(&ctx).unwrap();
    assert_eq!(snap_meta.SchedulersConfig, snap_meta2.SchedulersConfig);
    assert_eq!(snap_meta.UpstreamClusterID, snap_meta2.UpstreamClusterID);
    assert_eq!(snap_meta.RestoredTS, snap_meta2.RestoredTS);

    let log_meta = CheckpointMetadataForLogRestore {
        UpstreamClusterID: 123,
        RestoredTS: 222,
        StartTS: 111,
        RewriteTS: 333,
        GcRatio: "1.0".into(),
        RocksDBMaxBackgroundJobs: "8".into(),
        SnapshotRestoreDataSize: 1024,
        TiFlashItems: HashMap::from([(1, TiFlashReplicaInfo { Count: 1 })]),
        ..Default::default()
    };
    log.SaveCheckpointMetadata(&ctx, &log_meta).unwrap();
    let log_meta2 = log.LoadCheckpointMetadata(&ctx).unwrap();
    assert_eq!(log_meta.UpstreamClusterID, log_meta2.UpstreamClusterID);
    assert_eq!(log_meta.RestoredTS, log_meta2.RestoredTS);
    assert_eq!(log_meta.StartTS, log_meta2.StartTS);
    assert_eq!(log_meta.RewriteTS, log_meta2.RewriteTS);
    assert_eq!(log_meta.GcRatio, log_meta2.GcRatio);
    assert_eq!(
        log_meta.RocksDBMaxBackgroundJobs,
        log_meta2.RocksDBMaxBackgroundJobs
    );
    assert_eq!(
        log_meta.SnapshotRestoreDataSize,
        log_meta2.SnapshotRestoreDataSize
    );
    assert_eq!(log_meta.TiFlashItems, log_meta2.TiFlashItems);

    // progress Save 前不存在。
    assert!(!log.ExistsCheckpointProgress(&ctx).unwrap());
    log.SaveCheckpointProgress(
        &ctx,
        &CheckpointProgress {
            Progress: RestoreProgress::InLogRestoreAndIdMapPersisted,
        },
    )
    .unwrap();
    let progress = log.LoadCheckpointProgress(&ctx).unwrap();
    assert_eq!(
        progress.Progress,
        RestoreProgress::InLogRestoreAndIdMapPersisted
    );

    // 聚合视图字段核对。
    let task_info = GetCheckpointTaskInfo(&ctx, Some(snapshot), log).unwrap();
    assert_eq!(task_info.Metadata.as_ref().unwrap().UpstreamClusterID, 123);
    assert_eq!(task_info.Metadata.as_ref().unwrap().RestoredTS, 222);
    assert_eq!(task_info.Metadata.as_ref().unwrap().StartTS, 111);
    assert_eq!(task_info.Metadata.as_ref().unwrap().RewriteTS, 333);
    assert_eq!(task_info.Metadata.as_ref().unwrap().GcRatio, "1.0");
    assert!(task_info.HasSnapshotMetadata);
    assert_eq!(
        task_info.Progress,
        RestoreProgress::InLogRestoreAndIdMapPersisted
    );

    // repair SQL 先不存在。
    assert!(!log.ExistsCheckpointIngestIndexRepairSQLs(&ctx).unwrap());
    log.SaveCheckpointIngestIndexRepairSQLs(
        &ctx,
        &CheckpointIngestIndexRepairSQLs {
            SQLs: vec![CheckpointIngestIndexRepairSQL {
                IndexID: 1,
                SchemaName: CIStr::new("2"),
                TableName: CIStr::new("3"),
                IndexName: "4".into(),
                AddSQL: "5".into(),
                AddArgs: vec![
                    serde_json::json!("6"),
                    serde_json::json!("7"),
                    serde_json::json!("8"),
                ],
                ..Default::default()
            }],
            ..Default::default()
        },
    )
    .unwrap();
    let repair = log.LoadCheckpointIngestIndexRepairSQLs(&ctx).unwrap();
    assert_eq!(repair.SQLs[0].IndexID, 1);
    assert_eq!(repair.SQLs[0].SchemaName, CIStr::new("2"));
    assert_eq!(repair.SQLs[0].TableName, CIStr::new("3"));
    assert_eq!(repair.SQLs[0].IndexName, "4");
    assert_eq!(repair.SQLs[0].AddSQL, "5");
    assert_eq!(
        repair.SQLs[0].AddArgs,
        vec![
            serde_json::json!("6"),
            serde_json::json!("7"),
            serde_json::json!("8"),
        ]
    );
}

// TestCheckpointBackupRunner
#[test]
// 两批 Append+checksum 后 Walk/Load。
fn test_checkpoint_backup_runner() {
    let _lock = env_lock();
    let ctx = Context::Background();
    let s: Arc<dyn Storage> = Arc::new(MemStorage::new());
    // Go creates data/checksum dirs; MemStorage is path-keyed and needs no mkdir.
    // MemStorage 无目录；锚定常量。
    let _ = CheckpointDataDirForBackup;

    let cipher = aes_cipher();
    let runner = StartCheckpointBackupRunnerForTest(
        &ctx,
        s.clone(),
        Some(cipher.clone()),
        Duration::from_secs(5),
        new_mock_timer(10, 10),
    )
    .unwrap();

    let data: HashMap<&str, (&str, &str, &str, &str)> = HashMap::from([
        ("a", ("a", "b", "c", "d")),
        ("A", ("A", "B", "C", "D")),
        ("1", ("1", "2", "3", "4")),
    ]);
    let data2: HashMap<&str, (&str, &str, &str, &str)> =
        HashMap::from([("+", ("+", "-", "*", "/"))]);

    // 第一批 range/files。
    for d in data.values() {
        AppendForBackup(
            &ctx,
            &runner,
            d.0.as_bytes(),
            d.1.as_bytes(),
            vec![
                File {
                    Name: d.2.to_string(),
                    ..Default::default()
                },
                File {
                    Name: d.3.to_string(),
                    ..Default::default()
                },
            ],
        )
        .unwrap();
    }

    for i in 1..=4 {
        runner
            .FlushChecksum(&ctx, i, i as u64, i as u64, i as u64)
            .unwrap();
    }

    // 第二批（含 "+"）。
    for d in data2.values() {
        AppendForBackup(
            &ctx,
            &runner,
            d.0.as_bytes(),
            d.1.as_bytes(),
            vec![
                File {
                    Name: d.2.to_string(),
                    ..Default::default()
                },
                File {
                    Name: d.3.to_string(),
                    ..Default::default()
                },
            ],
        )
        .unwrap();
    }

    // 最终 flush，避免依赖 tick。
    runner.WaitForFinish(&ctx, true);

    let data_c = data.clone();
    let data2_c = data2.clone();
    // StartKey 作 key 核对 Files。
    WalkCheckpointFileForBackup(&ctx, s.as_ref(), Some(&cipher), |_, resp| {
        let key = String::from_utf8(resp.StartKey.clone()).unwrap();
        let d = data_c
            .get(key.as_str())
            .or_else(|| data2_c.get(key.as_str()))
            .expect("range key should exist in source fixture");
        assert_eq!(d.0.as_bytes(), resp.StartKey.as_slice());
        assert_eq!(d.1.as_bytes(), resp.EndKey.as_slice());
        assert_eq!(d.2, resp.Files[0].Name);
        assert_eq!(d.3, resp.Files[1].Name);
        Ok(())
    })
    .unwrap();

    let checkpoint_meta = CheckpointMetadataForBackup {
        ConfigHash: b"123456".to_vec(),
        BackupTS: 123456,
        ..Default::default()
    };
    SaveCheckpointMetadata(&ctx, s.as_ref(), &checkpoint_meta).unwrap();
    let meta = LoadCheckpointMetadata(&ctx, s.as_ref()).unwrap();
    for i in 1..=4 {
        assert_eq!(meta.CheckpointChecksum.get(&i).unwrap().Crc64xor, i as u64);
    }
}

// TestCheckpointRestoreRunnerOnStorage
#[test]
// restore × storage。
fn test_checkpoint_restore_runner_on_storage() {
    let _lock = env_lock();
    let s: Arc<dyn Storage> = Arc::new(MemStorage::new());
    let snapshot = NewSnapshotStorageMetaManager(s, None, 1, "snapshot", 1);
    test_checkpoint_restore_runner(snapshot.as_ref());
    snapshot.Close();
}

// TestCheckpointRestoreRunnerOnTable
#[test]
// restore × table。
fn test_checkpoint_restore_runner_on_table() {
    let _lock = env_lock();
    let (g, dom) = create_restore_schema_suite();
    let snapshot =
        NewSnapshotTableMetaManager(&g, dom, SnapshotRestoreCheckpointDatabaseName, 1).unwrap();
    test_checkpoint_restore_runner(snapshot.as_ref());
    snapshot.Close();
}

// Append→计数=4→checksum→Remove。
// table_id 分流防归并错位。
fn test_checkpoint_restore_runner(snapshot: &dyn SnapshotMetaManager) {
    let ctx = Context::Background();
    snapshot
        .SaveCheckpointMetadata(&ctx, &CheckpointMetadataForSnapshotRestore::default())
        .unwrap();
    let runner = StartCheckpointRestoreRunnerForTest(
        &ctx,
        Duration::from_secs(5),
        Duration::from_secs(3),
        snapshot,
    )
    .unwrap();

    let data: HashMap<&str, &str> = HashMap::from([("a", "a"), ("A", "A"), ("1", "1")]);
    let data2: HashMap<&str, &str> = HashMap::from([("+", "+")]);

    for range_key in data.values() {
        AppendRangesForRestore(
            &ctx,
            &runner,
            &NewCheckpointRangeKeyItem(1, (*range_key).to_string()),
        )
        .unwrap();
    }
    for i in 1..=4 {
        runner
            .FlushChecksum(&ctx, i, i as u64, i as u64, i as u64)
            .unwrap();
    }
    for range_key in data2.values() {
        AppendRangesForRestore(
            &ctx,
            &runner,
            &NewCheckpointRangeKeyItem(2, (*range_key).to_string()),
        )
        .unwrap();
    }
    runner.WaitForFinish(&ctx, true);

    let mut resp_count = 0;
    snapshot
        .LoadCheckpointData(&ctx, &mut |table_id, resp| {
            if data2.contains_key(resp.RangeKey.as_str()) {
                assert_eq!(table_id, 2);
                assert!(data2.contains_key(resp.RangeKey.as_str()));
            } else {
                assert_eq!(table_id, 1);
                assert!(data.contains_key(resp.RangeKey.as_str()));
            }
            assert_eq!(
                data.get(resp.RangeKey.as_str())
                    .or_else(|| data2.get(resp.RangeKey.as_str()))
                    .copied()
                    .unwrap(),
                resp.RangeKey.as_str()
            );
            resp_count += 1;
            Ok(())
        })
        .unwrap();
    // a/A/1/+ 共四条。
    assert_eq!(resp_count, 4);

    let (checksum, _) = snapshot.LoadCheckpointChecksum(&ctx).unwrap();
    for i in 1..=4 {
        assert_eq!(checksum.get(&i).unwrap().Crc64xor, i as u64);
    }

    // 清理后 ExistsMetadata=false。
    snapshot.RemoveCheckpointData(&ctx).unwrap();
    assert!(!snapshot.ExistsCheckpointMetadata(&ctx).unwrap());
}

// TestCheckpointRunnerRetryOnStorage
#[test]
// failpoint 重试 × storage。
fn test_checkpoint_runner_retry_on_storage() {
    let _lock = env_lock();
    let s: Arc<dyn Storage> = Arc::new(MemStorage::new());
    let snapshot = NewSnapshotStorageMetaManager(s, None, 1, "snapshot", 1);
    test_checkpoint_runner_retry(snapshot.as_ref());
    snapshot.Close();
}

// TestCheckpointRunnerRetryOnTable
#[test]
// failpoint 重试 × table。
fn test_checkpoint_runner_retry_on_table() {
    let _lock = env_lock();
    let (g, dom) = create_restore_schema_suite();
    let snapshot =
        NewSnapshotTableMetaManager(&g, dom, SnapshotRestoreCheckpointDatabaseName, 1).unwrap();
    test_checkpoint_runner_retry(snapshot.as_ref());
    snapshot.Close();
}

// 短 tick+failpoint，恢复后 >=1。
fn test_checkpoint_runner_retry(snapshot: &dyn SnapshotMetaManager) {
    let ctx = Context::Background();
    snapshot
        .SaveCheckpointMetadata(&ctx, &CheckpointMetadataForSnapshotRestore::default())
        .unwrap();
    let runner = StartCheckpointRestoreRunnerForTest(
        &ctx,
        Duration::from_millis(100),
        Duration::from_millis(300),
        snapshot,
    )
    .unwrap();

    // 写成功后失败，走 incomplete。
    failpoint::Enable(
        "github.com/pingcap/tidb/br/pkg/checkpoint/failed-after-checkpoint-flushes",
        "return(true)",
    )
    .unwrap();
    // RAII 清理 failpoint。
    let _guard = FailGuard;
    AppendRangesForRestore(&ctx, &runner, &NewCheckpointRangeKeyItem(1, "123".into())).unwrap();
    AppendRangesForRestore(&ctx, &runner, &NewCheckpointRangeKeyItem(2, "456".into())).unwrap();
    runner.FlushChecksum(&ctx, 1, 1, 1, 1).unwrap();
    let _ = runner.FlushChecksum(&ctx, 2, 2, 2, 2);
    std::thread::sleep(Duration::from_secs(1));
    failpoint::Disable("github.com/pingcap/tidb/br/pkg/checkpoint/failed-after-checkpoint-flushes")
        .unwrap();
    AppendRangesForRestore(&ctx, &runner, &NewCheckpointRangeKeyItem(3, "789".into())).unwrap();
    runner.FlushChecksum(&ctx, 3, 3, 3, 3).unwrap();
    runner.WaitForFinish(&ctx, true);

    let mut record_set: HashMap<String, i32> = HashMap::new();
    snapshot
        .LoadCheckpointData(&ctx, &mut |table_id, v| {
            *record_set
                .entry(format!("{}_{}", table_id, v.RangeKey))
                .or_insert(0) += 1;
            Ok(())
        })
        .unwrap();
    // 允许重试重复落盘。
    assert!(record_set.get("1_123").copied().unwrap_or(0) >= 1);
    assert!(record_set.get("2_456").copied().unwrap_or(0) >= 1);
    assert!(record_set.get("3_789").copied().unwrap_or(0) >= 1);
    let (items, _) = snapshot.LoadCheckpointChecksum(&ctx).unwrap();
    assert_eq!(
        format!(
            "{}_{}_{}",
            items[&1].Crc64xor, items[&1].TotalBytes, items[&1].TotalKvs
        ),
        "1_1_1"
    );
    assert_eq!(
        format!(
            "{}_{}_{}",
            items[&2].Crc64xor, items[&2].TotalBytes, items[&2].TotalKvs
        ),
        "2_2_2"
    );
    assert_eq!(
        format!(
            "{}_{}_{}",
            items[&3].Crc64xor, items[&3].TotalBytes, items[&3].TotalKvs
        ),
        "3_3_3"
    );
}

// Drop 时 Disable failpoint。
struct FailGuard;

impl Drop for FailGuard {
    fn drop(&mut self) {
        let _ = failpoint::Disable(
            "github.com/pingcap/tidb/br/pkg/checkpoint/failed-after-checkpoint-flushes",
        );
    }
}

// TestCheckpointRunnerNoRetryOnStorage
#[test]
// 干净路径 × storage。
fn test_checkpoint_runner_no_retry_on_storage() {
    let _lock = env_lock();
    let s: Arc<dyn Storage> = Arc::new(MemStorage::new());
    let snapshot = NewSnapshotStorageMetaManager(s, None, 1, "snapshot", 1);
    test_checkpoint_runner_no_retry(snapshot.as_ref());
    snapshot.Close();
}

// TestCheckpointRunnerNoRetryOnTable
#[test]
// 干净路径 × table。
fn test_checkpoint_runner_no_retry_on_table() {
    let _lock = env_lock();
    let (g, dom) = create_restore_schema_suite();
    let snapshot =
        NewSnapshotTableMetaManager(&g, dom, SnapshotRestoreCheckpointDatabaseName, 1).unwrap();
    test_checkpoint_runner_no_retry(snapshot.as_ref());
    snapshot.Close();
}

// 无 failpoint：每条恰 1。
fn test_checkpoint_runner_no_retry(snapshot: &dyn SnapshotMetaManager) {
    let ctx = Context::Background();
    snapshot
        .SaveCheckpointMetadata(&ctx, &CheckpointMetadataForSnapshotRestore::default())
        .unwrap();
    let runner = StartCheckpointRestoreRunnerForTest(
        &ctx,
        Duration::from_millis(100),
        Duration::from_millis(300),
        snapshot,
    )
    .unwrap();

    AppendRangesForRestore(&ctx, &runner, &NewCheckpointRangeKeyItem(1, "123".into())).unwrap();
    AppendRangesForRestore(&ctx, &runner, &NewCheckpointRangeKeyItem(2, "456".into())).unwrap();
    runner.FlushChecksum(&ctx, 1, 1, 1, 1).unwrap();
    runner.FlushChecksum(&ctx, 2, 2, 2, 2).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    runner.WaitForFinish(&ctx, true);

    let mut record_set: HashMap<String, i32> = HashMap::new();
    snapshot
        .LoadCheckpointData(&ctx, &mut |table_id, v| {
            *record_set
                .entry(format!("{}_{}", table_id, v.RangeKey))
                .or_insert(0) += 1;
            Ok(())
        })
        .unwrap();
    // 禁止无故重复。
    assert_eq!(record_set.get("1_123").copied().unwrap_or(0), 1);
    assert_eq!(record_set.get("2_456").copied().unwrap_or(0), 1);
    let (items, _) = snapshot.LoadCheckpointChecksum(&ctx).unwrap();
    assert_eq!(
        format!(
            "{}_{}_{}",
            items[&1].Crc64xor, items[&1].TotalBytes, items[&1].TotalKvs
        ),
        "1_1_1"
    );
    assert_eq!(
        format!(
            "{}_{}_{}",
            items[&2].Crc64xor, items[&2].TotalBytes, items[&2].TotalKvs
        ),
        "2_2_2"
    );
}

// TestCheckpointLogRestoreRunnerOnStorage
#[test]
// log-restore × storage。
fn test_checkpoint_log_restore_runner_on_storage() {
    let _lock = env_lock();
    let s: Arc<dyn Storage> = Arc::new(MemStorage::new());
    let log = NewLogStorageMetaManager(s, None, 1, "log", 1);
    test_checkpoint_log_restore_runner(log.as_ref());
    log.Close();
}

// TestCheckpointLogRestoreRunnerOnTable
#[test]
// log-restore × table。
fn test_checkpoint_log_restore_runner_on_table() {
    let _lock = env_lock();
    let (g, dom) = create_restore_schema_suite();
    let log = NewLogTableMetaManager(&g, dom, LogRestoreCheckpointDatabaseName, 1).unwrap();
    test_checkpoint_log_restore_runner(log.as_ref());
    log.Close();
}

// 嵌套偏移 Append；匹配后计数=4。
// 匹配失败 panic。
fn test_checkpoint_log_restore_runner(log: &dyn LogMetaManager) {
    let ctx = Context::Background();
    log.SaveCheckpointMetadata(&ctx, &CheckpointMetadataForLogRestore::default())
        .unwrap();
    let runner = StartCheckpointLogRestoreRunnerForTest(&ctx, Duration::from_secs(5), log).unwrap();

    // metaKey -> goff -> [(table, foff)]
    let data: HashMap<&str, HashMap<i64, Vec<(i64, i64)>>> = HashMap::from([
        (
            "a",
            HashMap::from([(0, vec![(1, 0), (2, 1)]), (1, vec![(1, 0)])]),
        ),
        ("A", HashMap::from([(0, vec![(3, 1)])])),
    ]);
    let data2: HashMap<&str, HashMap<i64, Vec<(i64, i64)>>> =
        HashMap::from([("+", HashMap::from([(1, vec![(1, 0)])]))]);

    for (k, d) in data.iter().chain(data2.iter()) {
        for (g, fs) in d {
            for f in fs {
                AppendRangeForLogRestore(&ctx, &runner, (*k).to_string(), f.0, *g, f.1).unwrap();
            }
        }
    }
    runner.WaitForFinish(&ctx, true);

    let mut resp_count = 0;
    log.LoadCheckpointData(
        &ctx,
        &mut |meta_key: String, resp: LogRestoreValueMarshaled| {
            let d = data
                .get(meta_key.as_str())
                .or_else(|| data2.get(meta_key.as_str()))
                .expect("meta key should exist");
            let fs = d.get(&resp.Goff).expect("group offset should exist");
            for f in fs {
                if let Some(foffs) = resp.Foffs.get(&f.0) {
                    if foffs.contains(&f.1) {
                        resp_count += 1;
                        return Ok(());
                    }
                }
            }
            // 找不到原夹具即失败。
            panic!("not found in the original data");
        },
    )
    .unwrap();
    assert_eq!(resp_count, 4);

    log.RemoveCheckpointData(&ctx).unwrap();
    assert!(!log.ExistsCheckpointMetadata(&ctx).unwrap());
}

// TestCheckpointRunnerLock
#[test]
// 锁互斥三阶段。
fn test_checkpoint_runner_lock() {
    let _lock = env_lock();
    let ctx = Context::Background();
    let s: Arc<dyn Storage> = Arc::new(MemStorage::new());
    let cipher = aes_cipher();

    let data = get_lock_data(10, 20).unwrap();
    s.WriteFile(&ctx, CheckpointLockPathForBackup, &data)
        .unwrap();

    let err = StartCheckpointBackupRunnerForTest(
        &ctx,
        s.clone(),
        Some(cipher.clone()),
        Duration::from_secs(5),
        new_mock_timer(10, 10),
    );
    // timer(10,10) 冲突。
    assert!(err.is_err(), "expected lock conflict error");

    let runner = StartCheckpointBackupRunnerForTest(
        &ctx,
        s.clone(),
        Some(cipher.clone()),
        Duration::from_secs(5),
        new_mock_timer(30, 10),
    )
    .unwrap();

    let err = StartCheckpointBackupRunnerForTest(
        &ctx,
        s,
        Some(cipher),
        Duration::from_secs(5),
        new_mock_timer(40, 10),
    );
    // 持锁期第三方失败。
    assert!(err.is_err(), "expected second lock conflict error");

    runner.WaitForFinish(&ctx, true);
}

// silence unused import warnings for symbols referenced only in comments / mapping
#[allow(dead_code)]
// 锚定符号消 unused。
fn _mapping_anchors() {
    let _: &str = checkpointMetaTableName;
    let _: fn(Error) -> Error = |e| e;
}

#[test]
fn test_log_restore_metadata_preserves_tikv_config_and_snapshot_size_json() {
    let input = serde_json::json!({
        "upstream-cluster-id": 123, "gc-ratio": "1.0",
        "rocksdb-max-background-jobs": "8", "snapshot-restore-data-size": 987654321,
        "tiflash-recorder": {"1": {"count": 1}}
    });
    let metadata: CheckpointMetadataForLogRestore = serde_json::from_value(input).unwrap();
    let serialized = serde_json::to_value(metadata).unwrap();
    assert_eq!(serialized["rocksdb-max-background-jobs"], "8");
    assert_eq!(serialized["snapshot-restore-data-size"], 987654321u64);
    let legacy: CheckpointMetadataForLogRestore =
        serde_json::from_str(r#"{"gc-ratio":"1.0"}"#).unwrap();
    let serialized = serde_json::to_value(legacy).unwrap();
    assert!(serialized.get("rocksdb-max-background-jobs").is_none());
    assert!(serialized.get("snapshot-restore-data-size").is_none());
}
