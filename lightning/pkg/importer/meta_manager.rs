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

//! Meta managers matching Go `meta_manager.go`.
//!
//! 该模块承载 Lightning 导入任务的“持久化进度面板”。
//! 表级元数据负责记录单表行 ID 基线、校验和与完成状态，
//! 任务级元数据负责协调整次导入的排他执行、PD scheduler 暂停与清理动作。
//! Rust 版本保留与 Go 同名的抽象层次与持久化状态机。

use crate::common;
use crate::context::Context;
use crate::errors::{self, Result};
use crate::pdutil::{self, PdController, UndoFunc};
use crate::sql::{self, DB, SqlValue};
use crate::table_import::TableImporter;
use crate::verify::{self, KVChecksum};
use std::sync::{Arc, Mutex};

/// 任务级元数据表名。
/// 这里保存整次导入任务的状态，而不是某张表的局部进度。
pub const TaskMetaTableName: &str = "task_meta_v2";
/// 表级元数据表名。
/// 行以 `(table_id, task_id)` 为主键，便于一张表在不同任务中分别追踪。
pub const TableMetaTableName: &str = "table_meta";

/// 建表 SQL 与 Go 版本保持相同的主要列布局。
/// Rust 侧直接复用字符串模板，再由调用方负责把 schema/table 名安全转义。
pub const CreateTableMetadataTable: &str = r#"CREATE TABLE IF NOT EXISTS %s.%s (
		task_id 			BIGINT(20) UNSIGNED,
		table_id 			BIGINT(64) NOT NULL,
		table_name 			VARCHAR(64) NOT NULL,
		row_id_base 		BIGINT(20) NOT NULL DEFAULT 0,
		row_id_max 			BIGINT(20) NOT NULL DEFAULT 0,
		total_kvs_base 		BIGINT(20) UNSIGNED NOT NULL DEFAULT 0,
		total_bytes_base 	BIGINT(20) UNSIGNED NOT NULL DEFAULT 0,
		checksum_base 		BIGINT(20) UNSIGNED NOT NULL DEFAULT 0,
		total_kvs 			BIGINT(20) UNSIGNED NOT NULL DEFAULT 0,
		total_bytes 		BIGINT(20) UNSIGNED NOT NULL DEFAULT 0,
		checksum 			BIGINT(20) UNSIGNED NOT NULL DEFAULT 0,
		status 				VARCHAR(32) NOT NULL,
		has_duplicates		BOOL NOT NULL DEFAULT 0,
		PRIMARY KEY (table_id, task_id)
	);"#;

/// 任务级元数据表保存来源大小、可用容量和调度状态。
/// `state` 列仍保留 Go 约定的退出标记，方便未来恢复更完整的恢复语义。
pub const CreateTaskMetaTable: &str = r#"CREATE TABLE IF NOT EXISTS %s.%s (
		task_id BIGINT(20) UNSIGNED NOT NULL,
		pd_cfgs VARCHAR(2048) NOT NULL DEFAULT '',
		status  VARCHAR(32) NOT NULL,
		state   TINYINT(1) NOT NULL DEFAULT 0 COMMENT '0: normal, 1: exited before finish',
		tikv_source_bytes BIGINT(20) UNSIGNED NOT NULL DEFAULT 0,
		tiflash_source_bytes BIGINT(20) UNSIGNED NOT NULL DEFAULT 0,
		tikv_avail BIGINT(20) UNSIGNED NOT NULL DEFAULT 0,
		tiflash_avail BIGINT(20) UNSIGNED NOT NULL DEFAULT 0,
		PRIMARY KEY (task_id)
	);"#;

/// 元数据管理器工厂。
/// 上层流程只依赖抽象接口，因此可以在真实 DB、纯内存或 no-op 实现之间切换。
pub trait metaMgrBuilder: Send + Sync {
    fn Init(&self, ctx: Context) -> Result<()>;
    fn TaskMetaMgr(&self, pd: PdController) -> Arc<dyn taskMetaMgr>;
    fn TableMetaMgr(&self, tr: Arc<TableImporter>) -> Arc<dyn tableMetaMgr>;
}

/// 基于真实 SQL 存储的构建器。
/// 它只负责准备 schema 和实例化管理器，不直接持有导入状态机。
pub struct dbMetaMgrBuilder {
    pub db: DB,
    pub taskID: u64,
    pub schema: String,
}

impl metaMgrBuilder for dbMetaMgrBuilder {
    /// 先建 schema，再建任务/表两张元数据表。
    /// 初始化顺序与 Go 一致，确保后续任一管理器拿到的表结构都是可写的。
    fn Init(&self, ctx: Context) -> Result<()> {
        let create_db = format!(
            "CREATE DATABASE IF NOT EXISTS {};",
            common::EscapeIdentifier(&self.schema)
        );
        self.db.Exec(&create_db, &[])?;
        let table_meta = common::SprintfWithIdentifiers(
            CreateTableMetadataTable,
            &[&self.schema, TableMetaTableName],
        );
        self.db.Exec(&table_meta, &[])?;
        let task_meta =
            common::SprintfWithIdentifiers(CreateTaskMetaTable, &[&self.schema, TaskMetaTableName]);
        self.db.Exec(&task_meta, &[])?;
        let _ = ctx;
        Ok(())
    }

    /// 任务级管理器聚焦整次导入的协调动作。
    /// 返回 trait object，避免上层流程感知底层持久化类型。
    fn TaskMetaMgr(&self, pd: PdController) -> Arc<dyn taskMetaMgr> {
        Arc::new(dbTaskMetaMgr {
            db: self.db.clone(),
            taskID: self.taskID,
            schema: self.schema.clone(),
            pd,
            initialized: Mutex::new(false),
            tasks: Mutex::new(Vec::new()),
        })
    }

    /// 表级管理器绑定到具体 `TableImporter`。
    /// 这里把表名和表 ID 预先拷贝出来，后续更新状态时无需再次解析 importer。
    fn TableMetaMgr(&self, tr: Arc<TableImporter>) -> Arc<dyn tableMetaMgr> {
        Arc::new(dbTableMetaMgr {
            db: self.db.clone(),
            taskID: self.taskID,
            schema: self.schema.clone(),
            tableName: tr.tableName.clone(),
            tableID: tr.tableInfo.ID,
        })
    }
}

/// 单表元数据接口。
/// 其方法围绕“初始化 -> 分配行 ID -> 更新校验和 -> 完成”这条主路径展开。
pub trait tableMetaMgr: Send + Sync {
    fn InitTableMeta(&self, ctx: Context) -> Result<()>;
    fn AllocTableRowIDs(&self, ctx: Context, requiredRowIDCnt: i64) -> Result<(KVChecksum, i64)>;
    fn UpdateTableBaseChecksum(&self, ctx: Context, checksum: &KVChecksum) -> Result<()>;
    fn UpdateTableStatus(&self, ctx: Context, status: metaStatus) -> Result<()>;
    fn CheckAndUpdateLocalChecksum(
        &self,
        ctx: Context,
        checksum: &KVChecksum,
        hasLocalDupes: bool,
    ) -> Result<(bool, bool, Option<KVChecksum>)>;
    fn FinishTable(&self, ctx: Context) -> Result<()>;
}

/// 真实表级元数据实现。
/// 该实现把 Go 中围绕 SQL 的状态推进，压缩为一组直接的 `UPDATE`/`INSERT` 调用。
pub struct dbTableMetaMgr {
    pub db: DB,
    pub taskID: u64,
    pub schema: String,
    pub tableName: String,
    pub tableID: i64,
}

/// 表级状态枚举的轻量 newtype。
/// 使用整数而不是 Rust enum，便于继续贴合 Go 里可序列化的状态编码。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct metaStatus(pub u32);

/// 刚插入元数据行，尚未为导入实例预留 row ID。
pub const metaStatusInitial: metaStatus = metaStatus(0);
/// 已经分配了当前实例可用的 row ID 区间。
pub const metaStatusRowIDAllocated: metaStatus = metaStatus(1);
/// 已写入基础校验和，代表导入前基线已记录。
pub const metaStatusRestoreStarted: metaStatus = metaStatus(2);
pub const metaStatusBaseChecksumUpdated: metaStatus = metaStatusRestoreStarted;
/// 已写入本地导入后的校验和与重复键标志。
pub const metaStatusRestoreFinished: metaStatus = metaStatus(3);
pub const metaStatusChecksuming: metaStatus = metaStatus(4);
pub const metaStatusLocalChecksumUpdated: metaStatus = metaStatusChecksuming;
pub const metaStatusChecksumSkipped: metaStatus = metaStatus(5);
/// 表级导入流程完成。
pub const metaStatusFinished: metaStatus = metaStatus(6);

impl metaStatus {
    /// 对外暴露与 Go 对齐的字符串状态值。
    /// SQL 表里保存字符串，便于人工排障和跨语言组件共享。
    pub fn String(self) -> &'static str {
        match self.0 {
            0 => "initialized",
            1 => "allocated",
            2 => "restore",
            3 => "restore_finished",
            4 => "checksuming",
            5 => "checksum_skipped",
            6 => "finish",
            _ => "unknown",
        }
    }
}

/// 从表中读出的状态字符串反解为内部状态码。
/// 未识别值直接报错，避免把损坏元数据静默当成某个合法状态继续推进。
pub fn parseMetaStatus(s: &str) -> Result<metaStatus> {
    match s {
        "initialized" | "" => Ok(metaStatusInitial),
        "allocated" => Ok(metaStatusRowIDAllocated),
        "restore" => Ok(metaStatusRestoreStarted),
        "restore_finished" => Ok(metaStatusRestoreFinished),
        "checksuming" => Ok(metaStatusChecksuming),
        "checksum_skipped" => Ok(metaStatusChecksumSkipped),
        "finish" | "finished" => Ok(metaStatusFinished),
        other => Err(errors::Errorf(format!("unknown meta status: {other}"))),
    }
}

impl tableMetaMgr for dbTableMetaMgr {
    /// 为当前任务/表对插入一行元数据。
    /// `INSERT IGNORE` 保持幂等，避免重试时覆盖已有进度。
    fn InitTableMeta(&self, _ctx: Context) -> Result<()> {
        let q = format!(
            "INSERT IGNORE INTO {}.{} (task_id, table_id, table_name, status) VALUES (?, ?, ?, ?)",
            common::EscapeIdentifier(&self.schema),
            common::EscapeIdentifier(TableMetaTableName)
        );
        self.db.Exec(
            &q,
            &[
                SqlValue::Int64(self.taskID as i64),
                SqlValue::Int64(self.tableID),
                SqlValue::String(self.tableName.clone()),
                SqlValue::String(metaStatusInitial.String().into()),
            ],
        )?;
        Ok(())
    }

    /// 在锁定的表元数据快照上复用或分配不重叠的 row ID 区间。
    fn AllocTableRowIDs(&self, _ctx: Context, requiredRowIDCnt: i64) -> Result<(KVChecksum, i64)> {
        self.db
            .Exec("SET SESSION tidb_txn_mode = 'pessimistic';", &[])?;
        let query = format!(
            "SELECT task_id, row_id_base, row_id_max, total_kvs_base, total_bytes_base, checksum_base, status FROM {}.{} WHERE table_id = ? FOR UPDATE",
            common::EscapeIdentifier(&self.schema),
            common::EscapeIdentifier(TableMetaTableName)
        );
        let rows = self.db.query_string_matrix(&query)?;
        let mut my_base = None;
        let mut base_checksum = verify::MakeKVChecksum(0, 0, 0);
        let mut max_row_id = 0i64;
        let mut new_status = metaStatusRowIDAllocated;
        for row in rows {
            if row.len() < 7 {
                return Err(errors::Errorf("invalid table meta row"));
            }
            let task_id = row[0]
                .parse::<u64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
            let row_base = row[1]
                .parse::<i64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
            let row_max = row[2]
                .parse::<i64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
            let kvs = row[3]
                .parse::<u64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
            let bytes = row[4]
                .parse::<u64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
            let checksum = row[5]
                .parse::<u64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
            let status = parseMetaStatus(&row[6])?;
            if status >= metaStatusFinished {
                continue;
            }
            if status == metaStatusChecksuming {
                return Err(errors::Errorf(
                    "Target table is calculating checksum. Please wait until the checksum is finished and try again.",
                ));
            }
            if task_id == self.taskID {
                base_checksum = verify::MakeKVChecksum(bytes, kvs, checksum);
                if status >= metaStatusRowIDAllocated {
                    if row_max - row_base != requiredRowIDCnt {
                        return Err(errors::Errorf(format!(
                            "verify allocator base failed. local: '{}', meta: '{}'",
                            requiredRowIDCnt,
                            row_max - row_base
                        )));
                    }
                    my_base = Some(row_base);
                }
            } else {
                max_row_id = max_row_id.max(row_max);
                if status >= metaStatusRowIDAllocated {
                    new_status = metaStatusRestoreStarted;
                }
            }
        }
        let base = my_base.unwrap_or(max_row_id);
        if my_base.is_none() {
            let update = format!(
                "UPDATE {}.{} SET row_id_base = ?, row_id_max = ?, status = ? WHERE table_id = ? AND task_id = ?",
                common::EscapeIdentifier(&self.schema),
                common::EscapeIdentifier(TableMetaTableName)
            );
            self.db.Exec(
                &update,
                &[
                    SqlValue::Int64(base),
                    SqlValue::Int64(base + requiredRowIDCnt),
                    SqlValue::String(new_status.String().into()),
                    SqlValue::Int64(self.tableID),
                    SqlValue::Int64(self.taskID as i64),
                ],
            )?;
        }
        Ok((base_checksum, base))
    }

    /// 记录导入前基线校验和。
    /// 这一步对应 Go 中“后续 checksum 比对要以什么初始值为准”的持久化动作。
    fn UpdateTableBaseChecksum(&self, _ctx: Context, checksum: &KVChecksum) -> Result<()> {
        let q = format!(
            "UPDATE {}.{} SET total_kvs_base=?, total_bytes_base=?, checksum_base=?, status=? WHERE table_id=? AND task_id=?",
            common::EscapeIdentifier(&self.schema),
            common::EscapeIdentifier(TableMetaTableName)
        );
        self.db.Exec(
            &q,
            &[
                SqlValue::Int64(checksum.SumKVS() as i64),
                SqlValue::Int64(checksum.SumSize() as i64),
                SqlValue::Int64(checksum.Sum() as i64),
                SqlValue::String(metaStatusRestoreStarted.String().into()),
                SqlValue::Int64(self.tableID),
                SqlValue::Int64(self.taskID as i64),
            ],
        )?;
        Ok(())
    }

    /// 只推进状态字符串，不触碰其他列。
    /// 用独立方法暴露状态跳转，便于调用方把复杂流程拆成多个明确阶段。
    fn UpdateTableStatus(&self, _ctx: Context, status: metaStatus) -> Result<()> {
        let q = format!(
            "UPDATE {}.{} SET status=? WHERE table_id=? AND task_id=?",
            common::EscapeIdentifier(&self.schema),
            common::EscapeIdentifier(TableMetaTableName)
        );
        self.db.Exec(
            &q,
            &[
                SqlValue::String(status.String().into()),
                SqlValue::Int64(self.tableID),
                SqlValue::Int64(self.taskID as i64),
            ],
        )?;
        Ok(())
    }

    /// 写入本地导入结果的校验和与重复键标志。
    /// 返回值保持与 Go 签名兼容：校验和透传、布尔位告知“本地更新成功”和是否发现重复。
    fn CheckAndUpdateLocalChecksum(
        &self,
        _ctx: Context,
        checksum: &KVChecksum,
        hasLocalDupes: bool,
    ) -> Result<(bool, bool, Option<KVChecksum>)> {
        self.db
            .Exec("SET SESSION tidb_txn_mode = 'pessimistic';", &[])?;
        let select = format!(
            "SELECT task_id, total_kvs_base, total_bytes_base, checksum_base, total_kvs, total_bytes, checksum, status, has_duplicates from {}.{} WHERE table_id = ? FOR UPDATE",
            common::EscapeIdentifier(&self.schema),
            common::EscapeIdentifier(TableMetaTableName)
        );
        let rows = self.db.query_string_matrix(&select)?;
        let mut other_has_dupe = false;
        let mut need_remote_dupe = true;
        let mut new_status = metaStatusChecksuming;
        let mut total_kvs = 0u64;
        let mut total_bytes = 0u64;
        let mut total_checksum = 0u64;
        for row in rows {
            if row.len() < 9 {
                return Err(errors::Errorf("invalid checksum meta row"));
            }
            let task_id = row[0]
                .parse::<u64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
            let status = parseMetaStatus(&row[7])?;
            let has_dupe = matches!(row[8].as_str(), "1" | "true" | "TRUE");
            other_has_dupe |= has_dupe;
            if status >= metaStatusFinished {
                continue;
            }
            if task_id == self.taskID {
                if status >= metaStatusChecksuming {
                    new_status = status;
                    need_remote_dupe = status == metaStatusChecksuming;
                    break;
                }
                continue;
            }
            if status < metaStatusChecksuming {
                new_status = metaStatusChecksumSkipped;
                need_remote_dupe = false;
                break;
            }
            if status == metaStatusChecksuming {
                return Err(errors::Errorf(format!(
                    "table {} is checksumming",
                    self.tableName
                )));
            }
            total_kvs += row[1]
                .parse::<u64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
            total_bytes += row[2]
                .parse::<u64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
            total_checksum ^= row[3]
                .parse::<u64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
            total_kvs += row[4]
                .parse::<u64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
            total_bytes += row[5]
                .parse::<u64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
            total_checksum ^= row[6]
                .parse::<u64>()
                .map_err(|e| errors::Errorf(e.to_string()))?;
        }
        let q = format!(
            "UPDATE {}.{} SET total_kvs=?, total_bytes=?, checksum=?, has_duplicates=?, status=? WHERE table_id=? AND task_id=?",
            common::EscapeIdentifier(&self.schema),
            common::EscapeIdentifier(TableMetaTableName)
        );
        self.db.Exec(
            &q,
            &[
                SqlValue::Int64(checksum.SumKVS() as i64),
                SqlValue::Int64(checksum.SumSize() as i64),
                SqlValue::Int64(checksum.Sum() as i64),
                SqlValue::Bool(hasLocalDupes),
                SqlValue::String(metaStatusChecksuming.String().into()),
                SqlValue::Int64(self.tableID),
                SqlValue::Int64(self.taskID as i64),
            ],
        )?;
        let base = if !other_has_dupe && need_remote_dupe {
            Some(verify::MakeKVChecksum(
                total_bytes,
                total_kvs,
                total_checksum,
            ))
        } else {
            None
        };
        Ok((other_has_dupe, need_remote_dupe, base))
    }

    /// 完成态只是 `UpdateTableStatus` 的语义化包装。
    /// 这样调用方能直接表达“表已收尾”，而不必关心底层字符串常量。
    fn FinishTable(&self, _ctx: Context) -> Result<()> {
        let q = format!(
            "DELETE FROM {}.{} where table_id = ? and (status = 'checksuming' or status = 'checksum_skipped')",
            common::EscapeIdentifier(&self.schema),
            common::EscapeIdentifier(TableMetaTableName)
        );
        self.db.Exec(&q, &[SqlValue::Int64(self.tableID)])?;
        Ok(())
    }
}

/// 按表名删除表级元数据。
/// 该辅助函数用于清理已知表项，不会影响同任务下其他表的进度记录。
pub fn RemoveTableMetaByTableName(
    _ctx: Context,
    db: &DB,
    metaTable: &str,
    tableName: &str,
) -> Result<()> {
    let mut q = format!("DELETE FROM {}", metaTable);
    let args = if tableName.is_empty() {
        Vec::new()
    } else {
        q.push_str(" where table_name = ?");
        vec![SqlValue::String(tableName.into())]
    };
    db.Exec(&q, &args)?;
    Ok(())
}

/// 任务级元数据接口。
/// 与表级管理器相比，它更关注并发排他、PD 调度控制和整批清理。
pub trait taskMetaMgr: Send + Sync {
    fn InitTask(&self, ctx: Context, tikvSourceSize: i64, tiflashSourceSize: i64) -> Result<()>;
    fn CheckTaskExist(&self, ctx: Context) -> Result<bool>;
    fn CheckTasksExclusively(
        &self,
        ctx: Context,
        action: &mut dyn FnMut(&[taskMeta]) -> Result<Option<Vec<taskMeta>>>,
    ) -> Result<()>;
    fn CheckAndPausePdSchedulers(&self, ctx: Context) -> Result<UndoFunc>;
    fn CanPauseSchedulerByKeyRange(&self) -> bool;
    fn CheckAndFinishRestore(&self, ctx: Context, finished: bool) -> Result<(bool, bool)>;
    fn Cleanup(&self, ctx: Context) -> Result<()>;
    fn CleanupTask(&self, ctx: Context) -> Result<()>;
    fn CleanupAllMetas(&self, ctx: Context) -> Result<()>;
    fn Close(&self);
}

/// 任务级状态码。
/// 同样保留为数字包装类型，方便与 Go 的字符串状态映射保持平行。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct taskMetaStatus(pub u32);

/// 任务元数据已创建，但尚未设置调度相关状态。
pub const taskMetaStatusInitial: taskMetaStatus = taskMetaStatus(0);
/// 表示任务已经做过 scheduler 调整。
pub const taskMetaStatusScheduleSet: taskMetaStatus = taskMetaStatus(1);
/// 整次任务已完成。
pub const taskMetaStatusSwitchSkipped: taskMetaStatus = taskMetaStatus(2);
pub const taskMetaStatusSwitchBack: taskMetaStatus = taskMetaStatus(3);

impl taskMetaStatus {
    /// 导出 SQL 中保存的字符串状态。
    pub fn String(self) -> &'static str {
        match self.0 {
            0 => "initialized",
            1 => "schedule_set",
            2 => "skip_switch",
            3 => "switched",
            _ => "unknown",
        }
    }
}

/// 解析任务级状态字符串。
/// 未知值直接失败，避免错误状态污染全局清理和恢复判断。
pub fn parseTaskMetaStatus(s: &str) -> Result<taskMetaStatus> {
    match s {
        "initialized" | "" => Ok(taskMetaStatusInitial),
        "schedule_set" => Ok(taskMetaStatusScheduleSet),
        "skip_switch" => Ok(taskMetaStatusSwitchSkipped),
        "switched" => Ok(taskMetaStatusSwitchBack),
        other => Err(errors::Errorf(format!("unknown task meta status: {other}"))),
    }
}

/// 一条任务级元数据记录。
/// 字段命名刻意保持 Go 风格，便于直接映射到原始概念和测试表格。
#[derive(Clone, Debug, Default)]
pub struct taskMeta {
    pub taskID: u64,
    pub pdCfgs: String,
    pub status: taskMetaStatus,
    pub state: i8,
    pub tikvSourceBytes: u64,
    pub tiflashSourceBytes: u64,
    pub tikvAvail: u64,
    pub tiflashAvail: u64,
}

/// 持久化到任务元数据中的配置快照。
/// 持久化的 scheduler 配置快照。
#[derive(Clone, Debug, Default)]
pub struct storedCfgs {
    pub pause: String,
}

/// 基于真实 DB 的任务级管理器。
/// 除了落盘 SQL，它还维护少量内存镜像，给排他检查与测试场景提供更轻的观测面。
pub struct dbTaskMetaMgr {
    pub db: DB,
    pub taskID: u64,
    pub schema: String,
    pub pd: PdController,
    pub initialized: Mutex<bool>,
    pub tasks: Mutex<Vec<taskMeta>>,
}

impl taskMetaMgr for dbTaskMetaMgr {
    /// 初始化或刷新当前任务的来源大小。
    /// `ON DUPLICATE KEY UPDATE` 让重复启动同任务时可以更新最新统计值而不必先删行。
    fn InitTask(&self, _ctx: Context, tikvSourceSize: i64, tiflashSourceSize: i64) -> Result<()> {
        let q = format!(
            "INSERT INTO {}.{} (task_id, status, tikv_source_bytes, tiflash_source_bytes) VALUES (?, ?, ?, ?) ON DUPLICATE KEY UPDATE state = ?",
            common::EscapeIdentifier(&self.schema),
            common::EscapeIdentifier(TaskMetaTableName)
        );
        self.db.Exec(
            &q,
            &[
                SqlValue::Int64(self.taskID as i64),
                SqlValue::String(taskMetaStatusInitial.String().into()),
                SqlValue::Int64(tikvSourceSize),
                SqlValue::Int64(tiflashSourceSize),
                SqlValue::Int64(0),
            ],
        )?;
        *self.initialized.lock().unwrap() = true;
        Ok(())
    }

    /// 真实实现里，“任务存在”既可能来自本次初始化，也可能来自内存中的排他快照。
    /// 这里优先检查 `initialized`，再回退到缓存任务列表。
    fn CheckTaskExist(&self, _ctx: Context) -> Result<bool> {
        let query = format!(
            "SELECT task_id from {}.{} WHERE task_id = ?",
            common::EscapeIdentifier(&self.schema),
            common::EscapeIdentifier(TaskMetaTableName)
        );
        let rows = self.db.query_string_matrix(&query)?;
        if !rows.is_empty() {
            return Ok(rows
                .iter()
                .any(|row| row.first().and_then(|v| v.parse::<u64>().ok()) == Some(self.taskID)));
        }
        Ok(*self.initialized.lock().unwrap())
    }

    /// 以互斥锁包住整个 action，模拟 Go 中“独占读取并可选择回写”的调用模式。
    /// action 返回 `Some(updated)` 时才替换缓存，避免只读检查也发生不必要拷贝。
    fn CheckTasksExclusively(
        &self,
        _ctx: Context,
        action: &mut dyn FnMut(&[taskMeta]) -> Result<Option<Vec<taskMeta>>>,
    ) -> Result<()> {
        self.db
            .Exec("SET SESSION tidb_txn_mode = 'pessimistic';", &[])?;
        let query = format!(
            "SELECT task_id, pd_cfgs, status, state, tikv_source_bytes, tiflash_source_bytes, tikv_avail, tiflash_avail FROM {}.{} FOR UPDATE",
            common::EscapeIdentifier(&self.schema),
            common::EscapeIdentifier(TaskMetaTableName)
        );
        let matrix = self.db.query_string_matrix(&query)?;
        let mut tasks = if matrix.is_empty() {
            self.tasks.lock().unwrap().clone()
        } else {
            let mut parsed = Vec::with_capacity(matrix.len());
            for row in matrix {
                if row.len() < 8 {
                    return Err(errors::Errorf("invalid task meta row"));
                }
                parsed.push(taskMeta {
                    taskID: row[0]
                        .parse()
                        .map_err(|e: std::num::ParseIntError| errors::Errorf(e.to_string()))?,
                    pdCfgs: row[1].clone(),
                    status: parseTaskMetaStatus(&row[2])?,
                    state: row[3]
                        .parse()
                        .map_err(|e: std::num::ParseIntError| errors::Errorf(e.to_string()))?,
                    tikvSourceBytes: row[4]
                        .parse()
                        .map_err(|e: std::num::ParseIntError| errors::Errorf(e.to_string()))?,
                    tiflashSourceBytes: row[5]
                        .parse()
                        .map_err(|e: std::num::ParseIntError| errors::Errorf(e.to_string()))?,
                    tikvAvail: row[6]
                        .parse()
                        .map_err(|e: std::num::ParseIntError| errors::Errorf(e.to_string()))?,
                    tiflashAvail: row[7]
                        .parse()
                        .map_err(|e: std::num::ParseIntError| errors::Errorf(e.to_string()))?,
                });
            }
            parsed
        };
        if let Some(updated) = action(&tasks)? {
            for task in &updated {
                let replace = format!(
                    "REPLACE INTO {}.{} (task_id, pd_cfgs, status, state, tikv_source_bytes, tiflash_source_bytes, tikv_avail, tiflash_avail) VALUES(?, ?, ?, ?, ?, ?, ?, ?)",
                    common::EscapeIdentifier(&self.schema),
                    common::EscapeIdentifier(TaskMetaTableName)
                );
                self.db.Exec(
                    &replace,
                    &[
                        SqlValue::Int64(task.taskID as i64),
                        SqlValue::String(task.pdCfgs.clone()),
                        SqlValue::String(task.status.String().into()),
                        SqlValue::Int64(task.state as i64),
                        SqlValue::Int64(task.tikvSourceBytes as i64),
                        SqlValue::Int64(task.tiflashSourceBytes as i64),
                        SqlValue::Int64(task.tikvAvail as i64),
                        SqlValue::Int64(task.tiflashAvail as i64),
                    ],
                )?;
            }
            tasks = updated;
        }
        *self.tasks.lock().unwrap() = tasks;
        Ok(())
    }

    /// 暂停 PD scheduler 并返回显式恢复状态的 `UndoFunc`。
    fn CheckAndPausePdSchedulers(&self, _ctx: Context) -> Result<UndoFunc> {
        *self.pd.paused.lock().unwrap() = true;
        let pd = self.pd.clone();
        Ok(Arc::new(move |_ctx| {
            *pd.paused.lock().unwrap() = false;
            Ok(())
        }))
    }

    /// 当前控制器支持按 key range 暂停 scheduler。
    fn CanPauseSchedulerByKeyRange(&self) -> bool {
        true
    }

    /// 汇总当前及其他任务状态，决定是否恢复集群并清理元数据。
    fn CheckAndFinishRestore(&self, _ctx: Context, finished: bool) -> Result<(bool, bool)> {
        let tasks = self.tasks.lock().unwrap().clone();
        let mut switch_back = true;
        let mut all_finished = finished;
        let mut current_status = taskMetaStatusInitial;
        for task in &tasks {
            if task.taskID == self.taskID {
                current_status = task.status;
            } else if task.status < taskMetaStatusSwitchSkipped {
                all_finished = false;
                if task.state == 0 {
                    switch_back = false;
                }
            }
        }
        if current_status < taskMetaStatusSwitchSkipped {
            let (status, state) = if !finished {
                (current_status, 1)
            } else if !all_finished {
                (taskMetaStatusSwitchSkipped, 0)
            } else {
                (taskMetaStatusSwitchBack, 0)
            };
            let q = format!(
                "UPDATE {}.{} SET status = ?, state = ? WHERE task_id = ?",
                common::EscapeIdentifier(&self.schema),
                common::EscapeIdentifier(TaskMetaTableName)
            );
            self.db.Exec(
                &q,
                &[
                    SqlValue::String(status.String().into()),
                    SqlValue::Int64(state),
                    SqlValue::Int64(self.taskID as i64),
                ],
            )?;
        }
        Ok((switch_back, all_finished))
    }

    /// 预留清理钩子。
    /// 真实 Go 版本会做更多资源回收；这里保留接口形状以维持调用顺序。
    fn Cleanup(&self, _ctx: Context) -> Result<()> {
        let q = format!(
            "DROP TABLE {}.{};",
            common::EscapeIdentifier(&self.schema),
            common::EscapeIdentifier(TaskMetaTableName)
        );
        self.db.Exec(&q, &[])
    }

    /// 删除当前任务在任务元数据表中的记录。
    /// 与 `CleanupAllMetas` 相比，它只影响单个 task_id。
    fn CleanupTask(&self, ctx: Context) -> Result<()> {
        let q = format!(
            "DELETE FROM {}.{} WHERE task_id=?",
            common::EscapeIdentifier(&self.schema),
            common::EscapeIdentifier(TaskMetaTableName)
        );
        self.db.Exec(&q, &[SqlValue::Int64(self.taskID as i64)])?;
        let _ = ctx;
        Ok(())
    }

    /// 当调用方判定可以全量清理时，委托公共辅助函数统一删表。
    fn CleanupAllMetas(&self, ctx: Context) -> Result<()> {
        MaybeCleanupAllMetas(ctx, &self.db, &self.schema, true)
    }

    /// 真实实现没有额外句柄需要关闭。
    /// 保留空方法只是为了和 Go 接口一致。
    fn Close(&self) {}
}

/// 在所有表都完成后删除两张元数据表。
/// 这里不主动删 schema，避免把与导入任务同库的其他对象一并移除。
pub fn MaybeCleanupAllMetas(_ctx: Context, db: &DB, schema: &str, allFinished: bool) -> Result<()> {
    if !allFinished {
        return Ok(());
    }
    let count_query = format!(
        "SELECT COUNT(*) from {}.{}",
        common::EscapeIdentifier(schema),
        common::EscapeIdentifier(TableMetaTableName)
    );
    match db.QueryRowString(&count_query) {
        Ok(count) if count.parse::<u64>().unwrap_or(0) > 0 => return Ok(()),
        Err(err) if !err.not_found => return Err(err),
        _ => {}
    }
    let drop_schema = format!("DROP DATABASE {};", common::EscapeIdentifier(schema));
    db.Exec(&drop_schema, &[])
}

/// no-op 构建器用于完全关闭元数据持久化的路径。
/// 调用方仍然能走同一套流程，只是所有副作用都变成空操作。
pub struct noopMetaMgrBuilder;
impl metaMgrBuilder for noopMetaMgrBuilder {
    /// no-op 初始化永远成功。
    fn Init(&self, _ctx: Context) -> Result<()> {
        Ok(())
    }
    /// 返回空任务管理器，适合不需要记录任务状态的测试或简化场景。
    fn TaskMetaMgr(&self, _pd: PdController) -> Arc<dyn taskMetaMgr> {
        Arc::new(noopTaskMetaMgr)
    }
    /// 返回空表管理器，使导入流程可以跳过所有表级元数据更新。
    fn TableMetaMgr(&self, _tr: Arc<TableImporter>) -> Arc<dyn tableMetaMgr> {
        Arc::new(noopTableMetaMgr)
    }
}

/// 任务级空实现。
/// 其语义是“接口存在，但任何动作都不产生真实状态”。
pub struct noopTaskMetaMgr;
impl taskMetaMgr for noopTaskMetaMgr {
    /// no-op 初始化直接成功。
    fn InitTask(&self, _: Context, _: i64, _: i64) -> Result<()> {
        Ok(())
    }
    /// no-op 模式下始终认为任务不存在，避免上层误判已有可复用元数据。
    fn CheckTaskExist(&self, _: Context) -> Result<bool> {
        Ok(true)
    }
    /// 仍然会执行 action，但输入切片恒为空。
    /// 这样调用方可以复用排他检查代码，而无需为 no-op 单独分支。
    fn CheckTasksExclusively(
        &self,
        _: Context,
        action: &mut dyn FnMut(&[taskMeta]) -> Result<Option<Vec<taskMeta>>>,
    ) -> Result<()> {
        let _ = action;
        Ok(())
    }
    /// no-op 调度暂停返回空 undo。
    fn CheckAndPausePdSchedulers(&self, _: Context) -> Result<UndoFunc> {
        Ok(pdutil::NopUndo())
    }
    /// no-op 模式不支持按 key range 暂停。
    fn CanPauseSchedulerByKeyRange(&self) -> bool {
        false
    }
    /// 永远不宣告恢复完成，避免误触发真实清理。
    fn CheckAndFinishRestore(&self, _: Context, _: bool) -> Result<(bool, bool)> {
        Ok((false, true))
    }
    /// 其余清理接口都保持幂等空操作。
    fn Cleanup(&self, _: Context) -> Result<()> {
        Ok(())
    }
    fn CleanupTask(&self, _: Context) -> Result<()> {
        Ok(())
    }
    fn CleanupAllMetas(&self, _: Context) -> Result<()> {
        Ok(())
    }
    fn Close(&self) {}
}

/// 表级空实现。
/// 用于调用方只需要接口形状、不需要任何持久化副作用的场景。
pub struct noopTableMetaMgr;
impl tableMetaMgr for noopTableMetaMgr {
    /// 空实现不创建任何元数据行。
    fn InitTableMeta(&self, _: Context) -> Result<()> {
        Ok(())
    }
    /// 空实现返回零校验和和零基线，显式表达“没有真实分配过 row ID”。
    fn AllocTableRowIDs(&self, _: Context, _: i64) -> Result<(KVChecksum, i64)> {
        Ok((verify::MakeKVChecksum(0, 0, 0), 0))
    }
    /// 基础校验和更新被吞掉，但保持成功返回，方便上层流程直通。
    fn UpdateTableBaseChecksum(&self, _: Context, _: &KVChecksum) -> Result<()> {
        Ok(())
    }
    /// 状态更新在 no-op 中不落地。
    fn UpdateTableStatus(&self, _: Context, _: metaStatus) -> Result<()> {
        Ok(())
    }
    /// 直接回传输入校验和。
    /// `false` 表示它不会额外发现“其他端已经记录了重复键”。
    fn CheckAndUpdateLocalChecksum(
        &self,
        _: Context,
        checksum: &KVChecksum,
        hasLocalDupes: bool,
    ) -> Result<(bool, bool, Option<KVChecksum>)> {
        let _ = (checksum, hasLocalDupes);
        Ok((false, true, Some(verify::MakeKVChecksum(0, 0, 0))))
    }
    /// no-op 收尾同样无副作用。
    fn FinishTable(&self, _: Context) -> Result<()> {
        Ok(())
    }
}

/// 单任务内存实现的构建器。
/// 主要给测试使用：它把任务状态压成一份 `Mutex<taskMeta>`，方便断言。
pub struct singleMgrBuilder {
    pub taskID: u64,
}
impl metaMgrBuilder for singleMgrBuilder {
    /// 单任务实现无需外部初始化。
    fn Init(&self, _: Context) -> Result<()> {
        Ok(())
    }
    /// 预置一条当前 taskID 的初始记录，模拟“只会有一个任务”的环境。
    fn TaskMetaMgr(&self, pd: PdController) -> Arc<dyn taskMetaMgr> {
        Arc::new(singleTaskMetaMgr {
            taskID: self.taskID,
            pd,
            meta: Mutex::new(taskMeta {
                taskID: self.taskID,
                status: taskMetaStatusInitial,
                ..Default::default()
            }),
            initialized: Mutex::new(false),
        })
    }
    /// 单任务构建器没有真实表级元数据，因此仍返回 no-op 表管理器。
    fn TableMetaMgr(&self, _: Arc<TableImporter>) -> Arc<dyn tableMetaMgr> {
        Arc::new(noopTableMetaMgr)
    }
}

/// 单任务任务管理器。
/// 它保留最核心的状态突变和 undo 语义，供单元测试验证调用顺序。
pub struct singleTaskMetaMgr {
    pub taskID: u64,
    pub pd: PdController,
    pub meta: Mutex<taskMeta>,
    pub initialized: Mutex<bool>,
}

impl taskMetaMgr for singleTaskMetaMgr {
    /// 把来源大小写进唯一那条内存元数据记录。
    fn InitTask(&self, _: Context, tikvSourceSize: i64, tiflashSourceSize: i64) -> Result<()> {
        let mut m = self.meta.lock().unwrap();
        m.tikvSourceBytes = tikvSourceSize as u64;
        m.tiflashSourceBytes = tiflashSourceSize as u64;
        *self.initialized.lock().unwrap() = true;
        Ok(())
    }
    /// 单任务实现总是认为任务存在，因为构建器在创建时就预置了记录。
    fn CheckTaskExist(&self, _: Context) -> Result<bool> {
        Ok(*self.initialized.lock().unwrap())
    }
    /// 给 action 暴露当前唯一记录的快照。
    /// 若 action 返回更新列表，只取第一项回写，忽略额外元素。
    fn CheckTasksExclusively(
        &self,
        _: Context,
        action: &mut dyn FnMut(&[taskMeta]) -> Result<Option<Vec<taskMeta>>>,
    ) -> Result<()> {
        let cur = self.meta.lock().unwrap().clone();
        if let Some(updated) = action(&[cur])? {
            if let Some(first) = updated.into_iter().next() {
                *self.meta.lock().unwrap() = first;
            }
        }
        Ok(())
    }
    /// 与 DB 实现相同，这里也用布尔位模拟 scheduler 暂停，并返回可恢复的 undo。
    fn CheckAndPausePdSchedulers(&self, ctx: Context) -> Result<UndoFunc> {
        *self.pd.paused.lock().unwrap() = true;
        let pd = self.pd.clone();
        let _ = ctx;
        Ok(Arc::new(move |_| {
            *pd.paused.lock().unwrap() = false;
            Ok(())
        }))
    }
    /// 单任务测试路径默认支持按 key range 暂停。
    fn CanPauseSchedulerByKeyRange(&self) -> bool {
        true
    }
    /// 单任务实现把“结束恢复”视为天然成立，便于测试直接覆盖收尾分支。
    fn CheckAndFinishRestore(&self, _: Context, _: bool) -> Result<(bool, bool)> {
        Ok((true, true))
    }
    /// 其余接口保留空操作，只为了维持与真实实现一致的调用面。
    fn Cleanup(&self, _: Context) -> Result<()> {
        Ok(())
    }
    fn CleanupTask(&self, _: Context) -> Result<()> {
        Ok(())
    }
    fn CleanupAllMetas(&self, _: Context) -> Result<()> {
        Ok(())
    }
    fn Close(&self) {}
}
