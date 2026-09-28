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

//! 表后端检查点存储：SQL 模板、分片写入、合并读取与 meta 生命周期。
//!
//! 对应 Go `br/pkg/checkpoint/storage.go`。Runner 通过 `tableCheckpointStorage`
//! 将 data/checksum 分块 REPLACE 进临时库；加载时 `mergeSelectCheckpoint`
//! 按 uuid+连续 segment_id 拼回完整 payload，空洞则丢弃整组。
//! meta/progress/ingest 使用无 uuid 的 segment 表。
//!
//! 约束：`initialLock`/`updateLock` 在表后端未实现（`panic`），与 Go 侧
//! 表存储不走外部锁文件的语义一致；真实锁仅外部对象存储路径需要。
//! `MemSession` 仅供对等测试，不是生产 Session。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

// SQL 模板中的 %n/%? 由 Session 实现解释；此处保持与 Go 字符串一致。

use crate::checkpoint::{
    ChecksumItem, KeyType, ValueType, checkpointStorage, parseCheckpointChecksum,
    parseCheckpointData,
};
use crate::stubs::{
    Context, Domain, Error, RestrictedSQLExecutor, Result, Session, SqlRow, SqlValue,
};

/// 日志恢复临时检查点库名前缀；`IsCheckpointDB` 用 starts_with 匹配带 restoreID 的后缀。
pub const LogRestoreCheckpointDatabaseName: &str = "__TiDB_BR_Temporary_Log_Restore_Checkpoint";
/// 快照恢复临时检查点库名前缀。
pub const SnapshotRestoreCheckpointDatabaseName: &str =
    "__TiDB_BR_Temporary_Snapshot_Restore_Checkpoint";
/// 自定义 SST 恢复临时检查点库名（通常无后缀变体）。
pub const CustomSSTRestoreCheckpointDatabaseName: &str =
    "__TiDB_BR_Temporary_Custom_SST_Restore_Checkpoint";

/// data 表：uuid + segment_id 主键，承载压缩后的 range 批次。
pub const checkpointDataTableName: &str = "cpt_data";
/// checksum 表：结构与 data 相同，内容为 ChecksumInfo JSON。
pub const checkpointChecksumTableName: &str = "cpt_checksum";
/// 任务元数据表名（无 uuid，按 segment_id 分片）。
pub const checkpointMetaTableName: &str = "cpt_metadata";
/// 进度相位表名。
pub const checkpointProgressTableName: &str = "cpt_progress";
/// 摄入索引修复 SQL 表名。
pub const checkpointIngestTableName: &str = "cpt_ingest";

/// 创建 data/checksum 类表：uuid(binary32)+segment_id+BLOB≤524288。
pub const createCheckpointTable: &str = r#"
		CREATE TABLE %n.%n (
			uuid binary(32) NOT NULL,
			segment_id BIGINT NOT NULL,
			data BLOB(524288) NOT NULL,
			update_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
			PRIMARY KEY(uuid, segment_id));"#;

/// REPLACE 写入 data/checksum；由 `chunkInsertCheckpointSQLs` 格式化库表名。
pub const insertCheckpointSQLTemplate: &str = r#"
		REPLACE INTO %s.%s
			(uuid, segment_id, data) VALUES (%?, %?, %?);"#;

/// 按 uuid,segment_id 有序扫描，供合并算法消费。
pub const selectCheckpointSQLTemplate: &str = r#"
		SELECT uuid, segment_id, data FROM %n.%n ORDER BY uuid, segment_id;"#;

/// meta 类表：仅 segment_id 主键，无 uuid 分组。
pub const createCheckpointMetaTable: &str = r#"
		CREATE TABLE %n.%n (
			segment_id BIGINT NOT NULL,
			data BLOB(524288) NOT NULL,
			update_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
			PRIMARY KEY(segment_id));"#;

/// meta 分片 REPLACE。
pub const insertCheckpointMetaSQLTemplate: &str = r#"
		REPLACE INTO %n.%n (segment_id, data) VALUES (%?, %?);"#;

/// 读取全部 meta 分片（调用方校验 segment 连续性）。
pub const selectCheckpointMetaSQLTemplate: &str = r#"SELECT segment_id, data FROM %n.%n;"#;

/// 判断库名是否属于三类 BR 临时检查点库（前缀匹配）。
pub fn IsCheckpointDB(dbname: &str) -> bool {
    dbname.starts_with(LogRestoreCheckpointDatabaseName)
        || dbname.starts_with(SnapshotRestoreCheckpointDatabaseName)
        || dbname.starts_with(CustomSSTRestoreCheckpointDatabaseName)
}

/// 单段 BLOB 上限，与建表定义及 Go 常量一致。
pub const CheckpointIdMapBlockSize: usize = 524288;

/// 将字节流按 BlockSize 切片，回调 `(segmentId, chunk)`；空数据不回调。
pub fn chunkInsertCheckpointData<F>(data: &[u8], mut fn_: F) -> Result<()>
where
    F: FnMut(u64, &[u8]) -> Result<()>,
{
    // 左闭右开切片；最后一段可短于 BlockSize
    let mut startIdx = 0usize;
    let mut segmentId = 0u64;
    while startIdx < data.len() {
        let endIdx = (startIdx + CheckpointIdMapBlockSize).min(data.len());
        fn_(segmentId, &data[startIdx..endIdx])?;
        startIdx = endIdx;
        segmentId += 1;
    }
    Ok(())
}

/// 为同一 uuid 生成多条 REPLACE SQL/参数，供 `flushCheckpoint*` 批量执行。
pub fn chunkInsertCheckpointSQLs(
    dbName: &str,
    tableName: &str,
    data: &[u8],
) -> (Vec<String>, Vec<Vec<SqlValue>>) {
    let mut sqls = Vec::new();
    let mut argss = Vec::new();
    // 同一批 flush 共享一个 uuid，加载时据此拼回
    let uuid = Uuid::new_v4();
    let _ = chunkInsertCheckpointData(data, |segmentId, chunk| {
        sqls.push(format!(
            "\n\t\tREPLACE INTO {dbName}.{tableName}\n\t\t\t(uuid, segment_id, data) VALUES (%?, %?, %?);"
        ));
        argss.push(vec![
            SqlValue::Bytes(uuid.as_bytes().to_vec()),
            SqlValue::U64(segmentId),
            SqlValue::Bytes(chunk.to_vec()),
        ]);
        Ok(())
    });
    (sqls, argss)
}

/// 实现 `checkpointStorage`：持有 Session，将 flush 落到指定库的 data/checksum 表。
///
/// `se` 用 Mutex+Option：`close` take 后后续 flush 返回 "session closed"。
/// lock 相关方法故意未实现——表后端不使用外部锁文件。
pub struct tableCheckpointStorage {
    /// 刷盘用 Session；close 后为 None
    pub se: Mutex<Option<Box<dyn Session>>>,
    /// 目标临时库名（含 restoreID）
    pub checkpointDBName: String,
}

// 构造时接管 session 所有权，生命周期与 Runner 绑定。
impl tableCheckpointStorage {
    pub fn new(se: Box<dyn Session>, checkpointDBName: String) -> Self {
        Self {
            se: Mutex::new(Some(se)),
            checkpointDBName,
        }
    }
}

// —— checkpointStorage：data/checksum 刷盘；lock 路径 panic ——
impl checkpointStorage for tableCheckpointStorage {
    /// 分片 REPLACE 进 cpt_data。
    fn flushCheckpointData(&self, ctx: &Context, data: &[u8]) -> Result<()> {
        let (sqls, argss) =
            chunkInsertCheckpointSQLs(&self.checkpointDBName, checkpointDataTableName, data);
        let mut guard = self.se.lock().unwrap();
        // session 已关闭则无法刷盘
        let se = guard.as_mut().ok_or_else(|| Error::new("session closed"))?;
        for (i, sql) in sqls.iter().enumerate() {
            se.ExecuteInternal(ctx, sql, &argss[i])?;
        }
        Ok(())
    }

    /// 分片 REPLACE 进 cpt_checksum。
    fn flushCheckpointChecksum(&self, ctx: &Context, data: &[u8]) -> Result<()> {
        let (sqls, argss) =
            chunkInsertCheckpointSQLs(&self.checkpointDBName, checkpointChecksumTableName, data);
        let mut guard = self.se.lock().unwrap();
        let se = guard.as_mut().ok_or_else(|| Error::new("session closed"))?;
        for (i, sql) in sqls.iter().enumerate() {
            se.ExecuteInternal(ctx, sql, &argss[i])?;
        }
        Ok(())
    }

    /// 表后端无锁文件；调用即 panic，防止误用外部存储锁语义。
    fn initialLock(&self, _ctx: &Context) -> Result<()> {
        // 有意 panic：调用方应使用外部存储实现的锁
        panic!("unimplement!");
    }

    /// 同 `initialLock`：未实现。
    fn updateLock(&self, _ctx: &Context) -> Result<()> {
        panic!("unimplement!");
    }

    /// take 并 Close session。
    fn close(&self) {
        if let Some(mut se) = self.se.lock().unwrap().take() {
            se.Close();
        }
    }
}

/// 扫描表并按 uuid 合并连续 segment；空洞或空 uuid 行跳过。
///
/// 算法：uuid 变化时提交上一组（若有效）；同 uuid 内要求 segment_id 从 0 递增，
/// 一旦断裂标记 invalid，丢弃该 uuid 后续分片。对齐 Go `mergeSelectCheckpoint`。
pub fn mergeSelectCheckpoint(
    ctx: &Context,
    execCtx: &dyn RestrictedSQLExecutor,
    dbName: &str,
    tableName: &str,
) -> Result<Vec<Vec<u8>>> {
    // 失败时 Annotate 带上库表名，便于排障
    let rows = execCtx
        .ExecRestrictedSQL(
            ctx,
            selectCheckpointSQLTemplate,
            &[
                SqlValue::Str(dbName.to_string()),
                SqlValue::Str(tableName.to_string()),
            ],
        )
        .map_err(|e| {
            e.Annotatef(format!(
                "failed to get checkpoint data from table {dbName}.{tableName}"
            ))
        })?;

    // 状态机：lastUUID / nextSegmentID / invalid 标志
    let mut retData: Vec<Vec<u8>> = Vec::with_capacity(rows.len());
    let mut rowData: Vec<u8> = Vec::new();
    let mut lastUUID: Option<Vec<u8>> = None;
    let mut lastUUIDInvalid = false;
    let mut nextSegmentID: u64 = 0;

    for row in rows {
        let uuid = row.GetBytes(0).to_vec();
        let segment_id = row.GetUint64(1);
        let data = row.GetBytes(2);
        // 空 uuid 视为脏行，忽略
        if uuid.is_empty() {
            continue;
        }
        // 新 uuid：先刷出上一组有效数据
        if lastUUID.as_ref() != Some(&uuid) {
            if !lastUUIDInvalid && !rowData.is_empty() {
                retData.push(std::mem::take(&mut rowData));
            }
            rowData.clear();
            lastUUIDInvalid = false;
            nextSegmentID = 0;
            lastUUID = Some(uuid);
        }
        // 已判定无效的 uuid 跳过剩余行
        if lastUUIDInvalid {
            continue;
        }
        // 空洞：整组作废
        if nextSegmentID != segment_id {
            lastUUIDInvalid = true;
            continue;
        }
        // 追加分片并期望下一段 id+1
        rowData.extend_from_slice(data);
        nextSegmentID += 1;
    }
    // 循环结束后提交最后一组
    if !lastUUIDInvalid && !rowData.is_empty() {
        retData.push(rowData);
    }
    Ok(retData)
}

/// 合并 data 表后逐包 `parseCheckpointData`，累加历史耗时并回调键值。
pub fn selectCheckpointData<K, V, F>(
    ctx: &Context,
    execCtx: &dyn RestrictedSQLExecutor,
    dbName: &str,
    mut fn_: F,
) -> Result<Duration>
where
    K: KeyType + Default + PartialEq + for<'de> serde::Deserialize<'de>,
    V: ValueType + for<'de> serde::Deserialize<'de>,
    F: FnMut(K, V) -> Result<()>,
{
    // 多包内容各自 parse，耗时累加
    let mut pastDureTime = Duration::ZERO;
    let checkpointDatas = mergeSelectCheckpoint(ctx, execCtx, dbName, checkpointDataTableName)?;
    for content in checkpointDatas {
        parseCheckpointData::<K, V, _>(&content, &mut pastDureTime, None, &mut fn_)?;
    }
    Ok(pastDureTime)
}

/// 合并 checksum 表后解析进 map，返回 (map, 累计耗时)。
pub fn selectCheckpointChecksum(
    ctx: &Context,
    execCtx: &dyn RestrictedSQLExecutor,
    dbName: &str,
) -> Result<(HashMap<i64, ChecksumItem>, Duration)> {
    // checksum 合并进同一 map，后写覆盖同 TableID
    let mut pastDureTime = Duration::ZERO;
    let mut checkpointChecksum = HashMap::new();
    let checkpointChecksums =
        mergeSelectCheckpoint(ctx, execCtx, dbName, checkpointChecksumTableName)?;
    for content in checkpointChecksums {
        parseCheckpointChecksum(&content, &mut checkpointChecksum, &mut pastDureTime)?;
    }
    Ok((checkpointChecksum, pastDureTime))
}

/// 确保库存在，并为给定表名执行 createCheckpointTable。
pub fn initCheckpointTable(
    ctx: &Context,
    se: &mut dyn Session,
    dbName: &str,
    checkpointTableNames: &[&str],
) -> Result<()> {
    // 先建库再建表，幂等 IF NOT EXISTS
    se.ExecuteInternal(
        ctx,
        "CREATE DATABASE IF NOT EXISTS %n;",
        &[SqlValue::Str(dbName.to_string())],
    )?;
    for tableName in checkpointTableNames {
        se.ExecuteInternal(
            ctx,
            createCheckpointTable,
            &[
                SqlValue::Str(dbName.to_string()),
                SqlValue::Str(tableName.to_string()),
            ],
        )?;
    }
    Ok(())
}

/// 序列化 meta → 建 meta 表 → 按段 REPLACE；对齐 Go insertCheckpointMeta。
pub fn insertCheckpointMeta<T: Serialize>(
    ctx: &Context,
    se: &mut dyn Session,
    dbName: &str,
    tableName: &str,
    meta: &T,
) -> Result<()> {
    // 序列化失败直接返回；建表后再分片写入
    let data = serde_json::to_vec(meta)?;
    se.ExecuteInternal(
        ctx,
        createCheckpointMetaTable,
        &[
            SqlValue::Str(dbName.to_string()),
            SqlValue::Str(tableName.to_string()),
        ],
    )?;
    chunkInsertCheckpointData(&data, |segmentId, chunk| {
        se.ExecuteInternal(
            ctx,
            insertCheckpointMetaSQLTemplate,
            &[
                SqlValue::Str(dbName.to_string()),
                SqlValue::Str(tableName.to_string()),
                SqlValue::U64(segmentId),
                SqlValue::Bytes(chunk.to_vec()),
            ],
        )
    })
}

/// 读取 meta 全部分片；空结果或 segment 不连续视为不完整检查点错误。
pub fn selectCheckpointMeta<T: DeserializeOwned>(
    ctx: &Context,
    execCtx: &dyn RestrictedSQLExecutor,
    dbName: &str,
    tableName: &str,
    meta: &mut T,
) -> Result<()> {
    let rows = execCtx
        .ExecRestrictedSQL(
            ctx,
            selectCheckpointMetaSQLTemplate,
            &[
                SqlValue::Str(dbName.to_string()),
                SqlValue::Str(tableName.to_string()),
            ],
        )
        .map_err(|e| {
            e.Annotatef(format!(
                "failed to get checkpoint metadata from table {dbName}.{tableName}"
            ))
        })?;
    // 空表：检查点不完整
    if rows.is_empty() {
        return Err(Error::new(format!(
            "get the empty checkpoint meta, the checkpoint is incomplete from table {dbName}.{tableName}"
        )));
    }
    // 预分配按满段估算；实际最后一段可能更短
    let mut data = Vec::with_capacity(rows.len() * CheckpointIdMapBlockSize);
    for (i, row) in rows.iter().enumerate() {
        let segmentId = row.GetUint64(0);
        let chunk = row.GetBytes(1);
        // meta 要求从 0 起连续，索引即期望 segmentId
        if i as u64 != segmentId {
            return Err(Error::new(format!(
                "the checkpoint metadata is incomplete from table {dbName}.{tableName} at segment {segmentId}"
            )));
        }
        data.extend_from_slice(chunk);
    }
    // JSON 反序列化到调用方提供的 meta 槽位
    *meta = serde_json::from_slice(&data)?;
    Ok(())
}

/// DROP 指定表；若库已空则再 DROP DATABASE。
/// 用 InfoSchema.SchemaTableInfos 判断库是否仍有表，避免误删非空库。
pub fn dropCheckpointTables(
    ctx: &Context,
    dom: &dyn Domain,
    se: &mut dyn Session,
    dbName: &str,
    tableNames: &[&str],
) -> Result<()> {
    // 先逐表 DROP，再决定是否删库
    for tableName in tableNames {
        se.ExecuteInternal(
            ctx,
            "DROP TABLE IF EXISTS %n.%n;",
            &[
                SqlValue::Str(dbName.to_string()),
                SqlValue::Str(tableName.to_string()),
            ],
        )?;
    }
    let tables = dom
        .InfoSchema()
        .SchemaTableInfos(ctx, &crate::stubs::CIStr::new(dbName))?;
    // 库内仍有其他表则保留数据库
    if !tables.is_empty() {
        return Ok(());
    }
    // 空库删除，避免残留临时库名
    se.ExecuteInternal(
        ctx,
        "DROP DATABASE %n;",
        &[SqlValue::Str(dbName.to_string())],
    )
}

/// In-memory session used by parity tests for table checkpoint paths.
#[derive(Default)]
/// 对等测试用内存 Session：解析 REPLACE/SELECT 模板，不连真实 TiDB。
/// DDL 语句（create/drop）直接成功；非 REPLACE 写入忽略。
pub struct MemSession {
    /// 内存表：key=`db.table`，value=行列表
    pub tables: Arc<Mutex<HashMap<String, Vec<SqlRow>>>>,
    /// Close 后标记；当前 Execute 未强制检查
    closed: bool,
}

// 表键为 `db.table` 字符串。
impl MemSession {
    /// 创建空内存会话。
    pub fn new() -> Self {
        Self::default()
    }

    /// 拼装内存表键。
    fn key(db: &str, table: &str) -> String {
        format!("{db}.{table}")
    }
}

// ExecuteInternal：识别 meta(4 参) 与 data(3 参) 两种 REPLACE 形态。
impl Session for MemSession {
    /// 仅置位；不清理 tables，便于测试在 Close 后仍读 executor
    fn Close(&mut self) {
        self.closed = true;
    }

    fn ExecuteInternal(&mut self, _ctx: &Context, sql: &str, args: &[SqlValue]) -> Result<()> {
        let sql_l = sql.to_lowercase();
        // DDL 在内存实现中空成功，避免测试依赖真实 schema
        if sql_l.contains("create database")
            || sql_l.contains("create table")
            || sql_l.contains("drop ")
        {
            return Ok(());
        }
        if sql_l.contains("replace into") {
            // meta 路径：参数为 db,table,segment_id,data
            // meta: (db, table, segment_id, data) via %n args OR data table via formatted sql
            if args.len() >= 4 {
                if let (
                    SqlValue::Str(db),
                    SqlValue::Str(table),
                    SqlValue::U64(seg),
                    SqlValue::Bytes(data),
                ) = (&args[0], &args[1], &args[2], &args[3])
                {
                    let k = Self::key(db, table);
                    let mut tables = self.tables.lock().unwrap();
                    let rows = tables.entry(k).or_default();
                    // 按 segment_id 覆盖旧行后排序
                    // replace by segment
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
                // data/checksum：从 SQL 文本解析库表名，参数为 uuid,segment,data
                // data/checksum table: uuid, segment_id, data — db.table in SQL text
                // parse "REPLACE INTO db.table"
                if let Some(rest) = sql.split("INTO").nth(1) {
                    let name = rest.trim().split_whitespace().next().unwrap_or("");
                    let k = name.to_string();
                    let mut tables = self.tables.lock().unwrap();
                    let rows = tables.entry(k).or_default();
                    if let (SqlValue::Bytes(uuid), SqlValue::U64(seg), SqlValue::Bytes(data)) =
                        (&args[0], &args[1], &args[2])
                    {
                        // data 行三列：uuid, segment_id, data
                        rows.push(SqlRow::new(vec![
                            SqlValue::Bytes(uuid.clone()),
                            SqlValue::U64(*seg),
                            SqlValue::Bytes(data.clone()),
                        ]));
                    }
                }
            }
        }
        // 未识别的 SQL 静默成功，降低测试脆性
        Ok(())
    }

    fn GetRestrictedSQLExecutor(&self) -> Arc<dyn RestrictedSQLExecutor> {
        Arc::new(MemRestricted {
            tables: self.tables.clone(),
        })
    }
}

/// 与 MemSession 共享 tables map 的受限 SQL 执行器。
struct MemRestricted {
    tables: Arc<Mutex<HashMap<String, Vec<SqlRow>>>>,
}

// 按 SQL 是否含 order by uuid 区分 data 与 meta 查询，返回内存行副本。
impl RestrictedSQLExecutor for MemRestricted {
    fn ExecRestrictedSQL(
        &self,
        _ctx: &Context,
        sql: &str,
        args: &[SqlValue],
    ) -> Result<Vec<SqlRow>> {
        let sql_l = sql.to_lowercase();
        // data 表有序扫描；缺表返回空 vec
        if sql_l.contains("order by uuid") {
            // data table select: args db, table
            if args.len() >= 2 {
                if let (SqlValue::Str(db), SqlValue::Str(table)) = (&args[0], &args[1]) {
                    let k = format!("{db}.{table}");
                    return Ok(self
                        .tables
                        .lock()
                        .unwrap()
                        .get(&k)
                        .cloned()
                        .unwrap_or_default());
                }
            }
        }
        // meta 查询：同样按 db/table 取行
        if args.len() >= 2 {
            if let (SqlValue::Str(db), SqlValue::Str(table)) = (&args[0], &args[1]) {
                let k = format!("{db}.{table}");
                return Ok(self
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
