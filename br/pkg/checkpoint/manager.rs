// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! 恢复检查点元数据管理器：表存储与外部对象存储两套实现。
//!
//! 对应 Go `br/pkg/checkpoint/manager.go`。Go 侧用泛型 `MetaManager`/`LogMetaManager`
//! 统一 snapshot/log；Rust 侧展开为独立 trait，避免跨阶段类型耦合。
//!
//! 职责分层：
//! - `SnapshotMetaManager` / `LogMetaManager`：加载/保存/探测/清理检查点，并启动 Runner；
//! - `TableMetaManager`：检查点落在 TiDB 系统表（库名带 restoreID 后缀）；
//! - `StorageMetaManager`：检查点落在外部 Storage 路径（按 cluster/task 拼前缀）。
//!
//! 约束：`StartCheckpointRunner` 会 `take` 走 runner 专用 session（表实现），
//! 同一 manager 不可重复启动；`Close` 负责释放剩余 session。外部存储实现无 session，
//! `Close` 为空操作；`TryGetStorage` 仅 Storage 实现返回底层句柄。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::checkpoint::{
    CheckpointRunner, ChecksumItem, RangeGroup, defaultRetryDuration,
    defaultTickDurationForChecksum, defaultTickDurationForFlush, loadCheckpointChecksum,
    loadCheckpointMeta, newCheckpointRunner, removeCheckpointData, saveCheckpointMetadata,
    walkCheckpointFile,
};
use crate::external_storage::{
    CheckpointRestoreDirFormat, flushPathForRestore, getCheckpointChecksumDirByName,
    getCheckpointDataDirByName, getCheckpointIngestIndexPathByName, getCheckpointMetaPathByName,
    getCheckpointProgressPathByName, newExternalCheckpointStorage,
};
use crate::log_restore::{
    CheckpointIngestIndexRepairSQLs, CheckpointMetadataForLogRestore, CheckpointProgress,
    LogRestoreKeyType, LogRestoreValueMarshaled, LogRestoreValueType,
};
use crate::restore::{CheckpointMetadataForSnapshotRestore, RestoreKeyType, RestoreValueType};
use crate::storage::{
    checkpointChecksumTableName, checkpointDataTableName, checkpointIngestTableName,
    checkpointMetaTableName, checkpointProgressTableName, dropCheckpointTables,
    initCheckpointTable, insertCheckpointMeta, selectCheckpointChecksum, selectCheckpointData,
    selectCheckpointMeta, tableCheckpointStorage,
};
use crate::stubs::{
    CIStr, CipherInfo, Context, Domain, Glue, Result, Session, Storage, TableInfoName,
};
/// Go 泛型别名占位：指向 `SnapshotMetaManager` trait 对象。
/// 对应 Go `SnapshotMetaManagerT = MetaManager[...]`。
pub type SnapshotMetaManagerT = dyn SnapshotMetaManager;
/// 日志恢复侧 MetaManager trait 对象别名；对应 Go `LogMetaManagerT`。
pub type LogMetaManagerT = dyn LogMetaManager;

#[derive(Clone, Debug)]
/// Runner 主循环的 tick / 重试间隔配置，对齐 Go `tickDurationConfig`。
/// flush 与 checksum 可独立调速；测试路径常整体缩短。
pub struct tickDurationConfig {
    /// 将内存中的 range 数据刷到存储的周期
    pub tickDurationForFlush: Duration,
    /// checksum 汇总写入周期
    pub tickDurationForChecksum: Duration,
    /// 刷盘失败后的重试间隔
    pub retryDuration: Duration,
}

/// 使用包级默认常量填充三档周期，供正式恢复路径使用。
pub fn DefaultTickDurationConfig() -> tickDurationConfig {
    tickDurationConfig {
        tickDurationForFlush: defaultTickDurationForFlush,
        tickDurationForChecksum: defaultTickDurationForChecksum,
        retryDuration: defaultRetryDuration,
    }
}

/// 快照恢复检查点管理接口。
///
/// 对应 Go `MetaManager` 在 snapshot 类型参数下的方法集。
/// 键值类型固定为 `RestoreKeyType`/`RestoreValueType`，元数据为
/// `CheckpointMetadataForSnapshotRestore`。实现须线程安全（`Send+Sync`）。
pub trait SnapshotMetaManager: Send + Sync {
    /// 人类可读位置描述（库名或外部路径），用于日志。
    fn String(&self) -> String;
    /// 遍历全部已落盘 range，经回调吐出键值；返回历史耗时总和。
    fn LoadCheckpointData(
        &self,
        ctx: &Context,
        fn_: &mut dyn FnMut(RestoreKeyType, RestoreValueType) -> Result<()>,
    ) -> Result<Duration>;
    /// 加载各表 checksum 项及累计耗时。
    fn LoadCheckpointChecksum(
        &self,
        ctx: &Context,
    ) -> Result<(HashMap<i64, ChecksumItem>, Duration)>;
    /// 读取快照恢复任务元数据。
    fn LoadCheckpointMetadata(&self, ctx: &Context)
    -> Result<CheckpointMetadataForSnapshotRestore>;
    /// 初始化必要表后写入元数据（表实现还会建 data/checksum 表）。
    fn SaveCheckpointMetadata(
        &self,
        ctx: &Context,
        meta: &CheckpointMetadataForSnapshotRestore,
    ) -> Result<()>;
    /// 探测元数据是否存在；表实现查 InfoSchema，存储实现查 FileExists。
    fn ExistsCheckpointMetadata(&self, ctx: &Context) -> Result<bool>;
    /// 删除本任务相关的全部检查点数据。
    fn RemoveCheckpointData(&self, ctx: &Context) -> Result<()>;
    /// 以给定 marshaler 启动 `CheckpointRunner`；lock tick 传 `Duration::ZERO`。
    fn StartCheckpointRunner(
        &self,
        ctx: &Context,
        cfg: tickDurationConfig,
        valueMarshaler: fn(&RangeGroup<RestoreKeyType, RestoreValueType>) -> Result<Vec<u8>>,
    ) -> Result<CheckpointRunner<RestoreKeyType, RestoreValueType>>;
    /// 释放持有的 session 等资源。
    fn Close(&self);
}

/// 日志恢复检查点管理接口。
///
/// 在 snapshot 能力之上增加进度文件、摄入索引修复 SQL，以及可选的底层 Storage。
/// 对应 Go `LogMetaManager`；加载 data 时回调值类型为压缩后的 `LogRestoreValueMarshaled`。
pub trait LogMetaManager: Send + Sync {
    /// 人类可读位置描述。
    fn String(&self) -> String;
    /// 加载已压缩的日志 range（`LogRestoreValueMarshaled`）并回调。
    fn LoadCheckpointData(
        &self,
        ctx: &Context,
        fn_: &mut dyn FnMut(LogRestoreKeyType, LogRestoreValueMarshaled) -> Result<()>,
    ) -> Result<Duration>;
    /// 加载 checksum 汇总。
    fn LoadCheckpointChecksum(
        &self,
        ctx: &Context,
    ) -> Result<(HashMap<i64, ChecksumItem>, Duration)>;
    /// 读取日志恢复任务元数据。
    fn LoadCheckpointMetadata(&self, ctx: &Context) -> Result<CheckpointMetadataForLogRestore>;
    /// 保存日志恢复元数据（表实现会先 init data/checksum 表）。
    fn SaveCheckpointMetadata(
        &self,
        ctx: &Context,
        meta: &CheckpointMetadataForLogRestore,
    ) -> Result<()>;
    /// 探测日志元数据是否存在。
    fn ExistsCheckpointMetadata(&self, ctx: &Context) -> Result<bool>;
    /// 清理 data/checksum/meta/progress/ingest 全部相关表或文件。
    fn RemoveCheckpointData(&self, ctx: &Context) -> Result<()>;
    /// 启动日志恢复 Runner；value 侧仍用稀疏 `LogRestoreValueType`，落盘时再压缩。
    fn StartCheckpointRunner(
        &self,
        ctx: &Context,
        cfg: tickDurationConfig,
        valueMarshaler: fn(&RangeGroup<LogRestoreKeyType, LogRestoreValueType>) -> Result<Vec<u8>>,
    ) -> Result<CheckpointRunner<LogRestoreKeyType, LogRestoreValueType>>;
    /// 关闭 session（表）或空操作（存储）。
    fn Close(&self);

    // —— 以下为日志恢复独有：进度相位与摄入索引修复 SQL ——
    /// 读取 snapshot+log 联合进度（`RestoreProgress`）。
    fn LoadCheckpointProgress(&self, ctx: &Context) -> Result<CheckpointProgress>;
    /// 持久化进度；进入 `InLogRestoreAndIdMapPersisted` 后不可再回退跑快照。
    fn SaveCheckpointProgress(&self, ctx: &Context, meta: &CheckpointProgress) -> Result<()>;
    /// 进度文件/表是否存在。
    fn ExistsCheckpointProgress(&self, ctx: &Context) -> Result<bool>;

    /// 加载待重放的 ADD INDEX / 外键更新 SQL 包。
    fn LoadCheckpointIngestIndexRepairSQLs(
        &self,
        ctx: &Context,
    ) -> Result<CheckpointIngestIndexRepairSQLs>;
    /// 保存摄入索引修复 SQL，供中断后续跑。
    fn SaveCheckpointIngestIndexRepairSQLs(
        &self,
        ctx: &Context,
        meta: &CheckpointIngestIndexRepairSQLs,
    ) -> Result<()>;
    /// 探测摄入索引修复记录是否存在。
    fn ExistsCheckpointIngestIndexRepairSQLs(&self, ctx: &Context) -> Result<bool>;

    /// 表实现恒返回 `None`；外部存储实现返回底层 `Storage`，供直接访问对象路径。
    fn TryGetStorage(&self) -> Option<Arc<dyn Storage>>;
}

/// 基于 TiDB 会话与系统表的检查点管理器。
///
/// 持有两份 session：`se` 供元数据读写，`runnerSe` 专供 Runner 刷盘，避免互相阻塞。
/// `dbName` = `{base}_{restoreID}`，与 Go `fmt.Sprintf("%s_%d", dbName, restoreID)` 对齐。
/// `kind` 区分对外暴露为 Snapshot 还是 Log trait（内部存储结构相同）。
pub struct TableMetaManager {
    /// 元数据/查询用 session；`Close` 或失败路径会 take 走
    se: Mutex<Option<Box<dyn Session>>>,
    /// Runner 专用 session；`StartCheckpointRunner` 一旦 take 即不可再次启动
    runnerSe: Mutex<Option<Box<dyn Session>>>,
    /// Domain：用于 InfoSchema 探测表是否存在、以及 drop 时清理
    dom: Arc<dyn Domain>,
    /// 检查点库名（含 restoreID 后缀）
    dbName: String,
    /// 标记本实例对外是 Snapshot 还是 Log 管理器
    kind: TableManagerKind,
}

#[derive(Clone, Copy)]
/// 表管理器角色；构造函数写入，当前实现主要靠分别 impl 两个 trait 区分 API。
enum TableManagerKind {
    Snapshot,
    Log,
}

/// 创建日志恢复用表管理器：新建两个 session，库名附加 restoreID。
/// 对应 Go `NewLogTableMetaManager`。
pub fn NewLogTableMetaManager(
    g: &dyn Glue,
    dom: Arc<dyn Domain>,
    dbName: &str,
    restoreID: u64,
) -> Result<Box<dyn LogMetaManager>> {
    let se = g.CreateSession(dom.Store().as_ref())?;
    let runnerSe = g.CreateSession(dom.Store().as_ref())?;
    Ok(Box::new(TableMetaManager {
        se: Mutex::new(Some(se)),
        runnerSe: Mutex::new(Some(runnerSe)),
        dom,
        dbName: format!("{dbName}_{restoreID}"),
        kind: TableManagerKind::Log,
    }))
}

/// 创建快照恢复用表管理器；session/库名规则与日志侧相同，仅 trait 绑定不同。
/// 对应 Go `NewSnapshotTableMetaManager`。
pub fn NewSnapshotTableMetaManager(
    g: &dyn Glue,
    dom: Arc<dyn Domain>,
    dbName: &str,
    restoreID: u64,
) -> Result<Box<dyn SnapshotMetaManager>> {
    let se = g.CreateSession(dom.Store().as_ref())?;
    let runnerSe = g.CreateSession(dom.Store().as_ref())?;
    Ok(Box::new(TableMetaManager {
        se: Mutex::new(Some(se)),
        runnerSe: Mutex::new(Some(runnerSe)),
        dom,
        dbName: format!("{dbName}_{restoreID}"),
        kind: TableManagerKind::Snapshot,
    }))
}

impl TableMetaManager {
    /// 在持锁下借用 `se`；若已 Close 则返回 "session closed"。
    fn with_session<R>(&self, f: impl FnOnce(&mut dyn Session) -> Result<R>) -> Result<R> {
        let mut guard = self.se.lock().unwrap();
        let se = guard
            .as_mut()
            .ok_or_else(|| crate::stubs::Error::new("session closed"))?;
        f(se.as_mut())
    }
}

// —— TableMetaManager：快照恢复路径（SQL 表后端） ——
// 数据面走 `selectCheckpointData`/`selectCheckpointChecksum`；
// 元数据走 `checkpointMetaTableName`；Runner 使用 `tableCheckpointStorage`。
impl SnapshotMetaManager for TableMetaManager {
    /// 描述检查点所在库名，便于运维日志定位。
    fn String(&self) -> String {
        format!("databases[such as {}]", self.dbName)
    }

    /// 经受限 SQL 执行器扫描 data 表，回调每条 range；返回累计耗时。
    fn LoadCheckpointData(
        &self,
        ctx: &Context,
        fn_: &mut dyn FnMut(RestoreKeyType, RestoreValueType) -> Result<()>,
    ) -> Result<Duration> {
        self.with_session(|se| {
            let exec = se.GetRestrictedSQLExecutor();
            selectCheckpointData(ctx, exec.as_ref(), &self.dbName, fn_)
        })
    }

    /// 从 checksum 表聚合各 table id 的校验项。
    fn LoadCheckpointChecksum(
        &self,
        ctx: &Context,
    ) -> Result<(HashMap<i64, ChecksumItem>, Duration)> {
        self.with_session(|se| {
            let exec = se.GetRestrictedSQLExecutor();
            selectCheckpointChecksum(ctx, exec.as_ref(), &self.dbName)
        })
    }

    /// 反序列化 meta 表中的快照恢复元数据。
    fn LoadCheckpointMetadata(
        &self,
        ctx: &Context,
    ) -> Result<CheckpointMetadataForSnapshotRestore> {
        self.with_session(|se| {
            let exec = se.GetRestrictedSQLExecutor();
            let mut m = CheckpointMetadataForSnapshotRestore::default();
            selectCheckpointMeta(
                ctx,
                exec.as_ref(),
                &self.dbName,
                checkpointMetaTableName,
                &mut m,
            )?;
            Ok(m)
        })
    }

    /// 确保 data/checksum 表存在后写入 meta；对齐 Go 先 init 再 insert。
    fn SaveCheckpointMetadata(
        &self,
        ctx: &Context,
        meta: &CheckpointMetadataForSnapshotRestore,
    ) -> Result<()> {
        self.with_session(|se| {
            initCheckpointTable(
                ctx,
                se,
                &self.dbName,
                &[checkpointDataTableName, checkpointChecksumTableName],
            )?;
            insertCheckpointMeta(ctx, se, &self.dbName, checkpointMetaTableName, meta)
        })
    }

    /// 用 InfoSchema 判断 meta 表是否存在（不访问外部存储）。
    fn ExistsCheckpointMetadata(&self, _ctx: &Context) -> Result<bool> {
        Ok(self.dom.InfoSchema().TableExists(
            &CIStr::new(&self.dbName),
            &CIStr::new(checkpointMetaTableName),
        ))
    }

    /// 一次性 drop data/checksum/meta/progress/ingest 五张表。
    fn RemoveCheckpointData(&self, ctx: &Context) -> Result<()> {
        self.with_session(|se| {
            dropCheckpointTables(
                ctx,
                self.dom.as_ref(),
                se,
                &self.dbName,
                &[
                    checkpointDataTableName,
                    checkpointChecksumTableName,
                    checkpointMetaTableName,
                    checkpointProgressTableName,
                    checkpointIngestTableName,
                ],
            )
        })
    }

    /// take `runnerSe` 构造 `tableCheckpointStorage` 并启动主循环；lock tick 为 0。
    /// 若 runner session 已被取走则报 "runner session missing"。
    fn StartCheckpointRunner(
        &self,
        ctx: &Context,
        cfg: tickDurationConfig,
        valueMarshaler: fn(&RangeGroup<RestoreKeyType, RestoreValueType>) -> Result<Vec<u8>>,
    ) -> Result<CheckpointRunner<RestoreKeyType, RestoreValueType>> {
        let runnerSe = self
            .runnerSe
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| crate::stubs::Error::new("runner session missing"))?;
        let storage = Arc::new(tableCheckpointStorage::new(runnerSe, self.dbName.clone()));
        let runner = newCheckpointRunner(storage, None, valueMarshaler);
        runner.startCheckpointMainLoop(
            ctx.clone(),
            cfg.tickDurationForFlush,
            cfg.tickDurationForChecksum,
            Duration::ZERO,
            cfg.retryDuration,
        );
        Ok(runner)
    }

    /// 关闭元数据与 Runner 两侧 session（若仍持有）。
    fn Close(&self) {
        if let Some(mut se) = self.se.lock().unwrap().take() {
            se.Close();
        }
        if let Some(mut se) = self.runnerSe.lock().unwrap().take() {
            se.Close();
        }
    }
}

// —— TableMetaManager：日志恢复路径（SQL 表后端） ——
// 额外读写 progress / ingest 表；`TryGetStorage` 固定返回 None。
impl LogMetaManager for TableMetaManager {
    /// 与快照侧相同的库名描述。
    fn String(&self) -> String {
        format!("databases[such as {}]", self.dbName)
    }

    /// 扫描 data 表；值类型为落盘压缩格式。
    fn LoadCheckpointData(
        &self,
        ctx: &Context,
        fn_: &mut dyn FnMut(LogRestoreKeyType, LogRestoreValueMarshaled) -> Result<()>,
    ) -> Result<Duration> {
        self.with_session(|se| {
            let exec = se.GetRestrictedSQLExecutor();
            selectCheckpointData(ctx, exec.as_ref(), &self.dbName, fn_)
        })
    }

    /// 读取 checksum 表。
    fn LoadCheckpointChecksum(
        &self,
        ctx: &Context,
    ) -> Result<(HashMap<i64, ChecksumItem>, Duration)> {
        self.with_session(|se| {
            let exec = se.GetRestrictedSQLExecutor();
            selectCheckpointChecksum(ctx, exec.as_ref(), &self.dbName)
        })
    }

    /// 读取日志恢复 meta。
    fn LoadCheckpointMetadata(&self, ctx: &Context) -> Result<CheckpointMetadataForLogRestore> {
        self.with_session(|se| {
            let exec = se.GetRestrictedSQLExecutor();
            let mut m = CheckpointMetadataForLogRestore::default();
            selectCheckpointMeta(
                ctx,
                exec.as_ref(),
                &self.dbName,
                checkpointMetaTableName,
                &mut m,
            )?;
            Ok(m)
        })
    }

    /// init data/checksum 后写入日志 meta。
    fn SaveCheckpointMetadata(
        &self,
        ctx: &Context,
        meta: &CheckpointMetadataForLogRestore,
    ) -> Result<()> {
        self.with_session(|se| {
            initCheckpointTable(
                ctx,
                se,
                &self.dbName,
                &[checkpointDataTableName, checkpointChecksumTableName],
            )?;
            insertCheckpointMeta(ctx, se, &self.dbName, checkpointMetaTableName, meta)
        })
    }

    /// InfoSchema 探测 meta 表。
    fn ExistsCheckpointMetadata(&self, _ctx: &Context) -> Result<bool> {
        Ok(self.dom.InfoSchema().TableExists(
            &CIStr::new(&self.dbName),
            &CIStr::new(checkpointMetaTableName),
        ))
    }

    /// 清理五张检查点相关表。
    fn RemoveCheckpointData(&self, ctx: &Context) -> Result<()> {
        self.with_session(|se| {
            dropCheckpointTables(
                ctx,
                self.dom.as_ref(),
                se,
                &self.dbName,
                &[
                    checkpointDataTableName,
                    checkpointChecksumTableName,
                    checkpointMetaTableName,
                    checkpointProgressTableName,
                    checkpointIngestTableName,
                ],
            )
        })
    }

    /// 与快照侧相同：take runnerSe → tableCheckpointStorage → 主循环。
    fn StartCheckpointRunner(
        &self,
        ctx: &Context,
        cfg: tickDurationConfig,
        valueMarshaler: fn(&RangeGroup<LogRestoreKeyType, LogRestoreValueType>) -> Result<Vec<u8>>,
    ) -> Result<CheckpointRunner<LogRestoreKeyType, LogRestoreValueType>> {
        let runnerSe = self
            .runnerSe
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| crate::stubs::Error::new("runner session missing"))?;
        let storage = Arc::new(tableCheckpointStorage::new(runnerSe, self.dbName.clone()));
        let runner = newCheckpointRunner(storage, None, valueMarshaler);
        runner.startCheckpointMainLoop(
            ctx.clone(),
            cfg.tickDurationForFlush,
            cfg.tickDurationForChecksum,
            Duration::ZERO,
            cfg.retryDuration,
        );
        Ok(runner)
    }

    /// 关闭两侧 session。
    fn Close(&self) {
        if let Some(mut se) = self.se.lock().unwrap().take() {
            se.Close();
        }
        if let Some(mut se) = self.runnerSe.lock().unwrap().take() {
            se.Close();
        }
    }

    /// 从 progress 表反序列化 `CheckpointProgress`。
    fn LoadCheckpointProgress(&self, ctx: &Context) -> Result<CheckpointProgress> {
        self.with_session(|se| {
            let exec = se.GetRestrictedSQLExecutor();
            let mut m = CheckpointProgress::default();
            selectCheckpointMeta(
                ctx,
                exec.as_ref(),
                &self.dbName,
                checkpointProgressTableName,
                &mut m,
            )?;
            Ok(m)
        })
    }

    /// 写入 progress 表（不强制 init data 表）。
    fn SaveCheckpointProgress(&self, ctx: &Context, meta: &CheckpointProgress) -> Result<()> {
        self.with_session(|se| {
            insertCheckpointMeta(ctx, se, &self.dbName, checkpointProgressTableName, meta)
        })
    }

    /// InfoSchema 探测 progress 表。
    fn ExistsCheckpointProgress(&self, _ctx: &Context) -> Result<bool> {
        Ok(self.dom.InfoSchema().TableExists(
            &CIStr::new(&self.dbName),
            &CIStr::new(checkpointProgressTableName),
        ))
    }

    /// 从 ingest 表加载索引/外键修复 SQL。
    fn LoadCheckpointIngestIndexRepairSQLs(
        &self,
        ctx: &Context,
    ) -> Result<CheckpointIngestIndexRepairSQLs> {
        self.with_session(|se| {
            let exec = se.GetRestrictedSQLExecutor();
            let mut m = CheckpointIngestIndexRepairSQLs::default();
            selectCheckpointMeta(
                ctx,
                exec.as_ref(),
                &self.dbName,
                checkpointIngestTableName,
                &mut m,
            )?;
            Ok(m)
        })
    }

    /// 将修复 SQL 包写入 ingest 表。
    fn SaveCheckpointIngestIndexRepairSQLs(
        &self,
        ctx: &Context,
        meta: &CheckpointIngestIndexRepairSQLs,
    ) -> Result<()> {
        self.with_session(|se| {
            insertCheckpointMeta(ctx, se, &self.dbName, checkpointIngestTableName, meta)
        })
    }

    /// InfoSchema 探测 ingest 表。
    fn ExistsCheckpointIngestIndexRepairSQLs(&self, _ctx: &Context) -> Result<bool> {
        Ok(self.dom.InfoSchema().TableExists(
            &CIStr::new(&self.dbName),
            &CIStr::new(checkpointIngestTableName),
        ))
    }

    /// 表后端无外部 Storage，恒返回 None。
    fn TryGetStorage(&self) -> Option<Arc<dyn Storage>> {
        None
    }
}

/// 基于外部对象存储的检查点管理器。
///
/// `taskName` = `{clusterID}/{prefix}_{restoreID}`，用于拼接 data/checksum/meta/progress
/// 等路径；加密可选（`cipher`）。无 session，`Close` 为空。
/// 对应 Go `StorageMetaManager`。
pub struct StorageMetaManager {
    /// 外部对象存储句柄
    storage: Arc<dyn Storage>,
    /// 可选加密；传给 walk/load 与 Runner
    cipher: Option<CipherInfo>,
    /// 集群 ID 字符串，用于 `String()` 展示路径模板
    clusterID: String,
    /// 任务级路径前缀片段
    taskName: String,
    /// Snapshot / Log 角色标记
    kind: StorageManagerKind,
}

#[derive(Clone, Copy)]
/// 外部存储管理器角色，与 `TableManagerKind` 对称。
enum StorageManagerKind {
    Snapshot,
    Log,
}

/// 构造快照恢复的外部存储管理器。
/// 对应 Go `NewSnapshotStorageMetaManager`。
pub fn NewSnapshotStorageMetaManager(
    storage: Arc<dyn Storage>,
    cipher: Option<CipherInfo>,
    clusterID: u64,
    prefix: &str,
    restoreID: u64,
) -> Box<dyn SnapshotMetaManager> {
    Box::new(StorageMetaManager {
        storage,
        cipher,
        clusterID: format!("{clusterID}"),
        taskName: format!("{clusterID}/{prefix}_{restoreID}"),
        kind: StorageManagerKind::Snapshot,
    })
}

/// 构造日志恢复的外部存储管理器。
/// 对应 Go `NewLogStorageMetaManager`。
pub fn NewLogStorageMetaManager(
    storage: Arc<dyn Storage>,
    cipher: Option<CipherInfo>,
    clusterID: u64,
    prefix: &str,
    restoreID: u64,
) -> Box<dyn LogMetaManager> {
    Box::new(StorageMetaManager {
        storage,
        cipher,
        clusterID: format!("{clusterID}"),
        taskName: format!("{clusterID}/{prefix}_{restoreID}"),
        kind: StorageManagerKind::Log,
    })
}

// —— StorageMetaManager：快照恢复路径（外部对象存储） ——
// 通过 `walkCheckpointFile`/`loadCheckpointMeta` 等读写路径文件；
// `RemoveCheckpointData` 前缀为 `checkpoints/restore-{taskName}`。
impl SnapshotMetaManager for StorageMetaManager {
    /// 用集群 ID 填充恢复目录模板，展示外部路径。
    fn String(&self) -> String {
        format!(
            "path[{}]",
            CheckpointRestoreDirFormat.replace("%s", &self.clusterID)
        )
    }

    /// 遍历外部 data 目录下的检查点文件并回调。
    fn LoadCheckpointData(
        &self,
        ctx: &Context,
        fn_: &mut dyn FnMut(RestoreKeyType, RestoreValueType) -> Result<()>,
    ) -> Result<Duration> {
        walkCheckpointFile(
            ctx,
            self.storage.as_ref(),
            self.cipher.as_ref(),
            &getCheckpointDataDirByName(&self.taskName),
            fn_,
        )
    }

    /// 从 checksum 目录加载汇总。
    fn LoadCheckpointChecksum(
        &self,
        ctx: &Context,
    ) -> Result<(HashMap<i64, ChecksumItem>, Duration)> {
        loadCheckpointChecksum(
            ctx,
            self.storage.as_ref(),
            &getCheckpointChecksumDirByName(&self.taskName),
        )
    }

    /// 读取 task 级 meta 文件。
    fn LoadCheckpointMetadata(
        &self,
        ctx: &Context,
    ) -> Result<CheckpointMetadataForSnapshotRestore> {
        let mut m = CheckpointMetadataForSnapshotRestore::default();
        loadCheckpointMeta(
            ctx,
            self.storage.as_ref(),
            &getCheckpointMetaPathByName(&self.taskName),
            &mut m,
        )?;
        Ok(m)
    }

    /// 将快照 meta 写入外部路径。
    fn SaveCheckpointMetadata(
        &self,
        ctx: &Context,
        meta: &CheckpointMetadataForSnapshotRestore,
    ) -> Result<()> {
        saveCheckpointMetadata(
            ctx,
            self.storage.as_ref(),
            meta,
            &getCheckpointMetaPathByName(&self.taskName),
        )
    }

    /// `FileExists` 探测 meta 文件。
    fn ExistsCheckpointMetadata(&self, ctx: &Context) -> Result<bool> {
        self.storage
            .FileExists(ctx, &getCheckpointMetaPathByName(&self.taskName))
    }

    /// 按 `checkpoints/restore-{taskName}` 前缀批量删除。
    fn RemoveCheckpointData(&self, ctx: &Context) -> Result<()> {
        let prefix = format!("checkpoints/restore-{}", self.taskName);
        removeCheckpointData(ctx, self.storage.as_ref(), &prefix)
    }

    /// 构造 `externalCheckpointStorage`（无 timer），携带 cipher 启动 Runner。
    fn StartCheckpointRunner(
        &self,
        ctx: &Context,
        cfg: tickDurationConfig,
        valueMarshaler: fn(&RangeGroup<RestoreKeyType, RestoreValueType>) -> Result<Vec<u8>>,
    ) -> Result<CheckpointRunner<RestoreKeyType, RestoreValueType>> {
        let checkpointStorage = newExternalCheckpointStorage(
            ctx,
            self.storage.clone(),
            None,
            flushPathForRestore(&self.taskName),
        )?;
        let runner = newCheckpointRunner(checkpointStorage, self.cipher.clone(), valueMarshaler);
        runner.startCheckpointMainLoop(
            ctx.clone(),
            cfg.tickDurationForFlush,
            cfg.tickDurationForChecksum,
            Duration::ZERO,
            cfg.retryDuration,
        );
        Ok(runner)
    }

    /// 外部存储无 session，空实现。
    fn Close(&self) {}
}

// —— StorageMetaManager：日志恢复路径（外部对象存储） ——
// progress / ingest 使用独立路径辅助函数；`TryGetStorage` 返回底层句柄。
impl LogMetaManager for StorageMetaManager {
    /// 同快照侧路径描述。
    fn String(&self) -> String {
        format!(
            "path[{}]",
            CheckpointRestoreDirFormat.replace("%s", &self.clusterID)
        )
    }

    /// 遍历 data 目录；回调压缩后的日志 value。
    fn LoadCheckpointData(
        &self,
        ctx: &Context,
        fn_: &mut dyn FnMut(LogRestoreKeyType, LogRestoreValueMarshaled) -> Result<()>,
    ) -> Result<Duration> {
        walkCheckpointFile(
            ctx,
            self.storage.as_ref(),
            self.cipher.as_ref(),
            &getCheckpointDataDirByName(&self.taskName),
            fn_,
        )
    }

    /// 加载外部 checksum 目录。
    fn LoadCheckpointChecksum(
        &self,
        ctx: &Context,
    ) -> Result<(HashMap<i64, ChecksumItem>, Duration)> {
        loadCheckpointChecksum(
            ctx,
            self.storage.as_ref(),
            &getCheckpointChecksumDirByName(&self.taskName),
        )
    }

    /// 读取日志 meta 文件。
    fn LoadCheckpointMetadata(&self, ctx: &Context) -> Result<CheckpointMetadataForLogRestore> {
        let mut m = CheckpointMetadataForLogRestore::default();
        loadCheckpointMeta(
            ctx,
            self.storage.as_ref(),
            &getCheckpointMetaPathByName(&self.taskName),
            &mut m,
        )?;
        Ok(m)
    }

    /// 写入日志 meta 文件。
    fn SaveCheckpointMetadata(
        &self,
        ctx: &Context,
        meta: &CheckpointMetadataForLogRestore,
    ) -> Result<()> {
        saveCheckpointMetadata(
            ctx,
            self.storage.as_ref(),
            meta,
            &getCheckpointMetaPathByName(&self.taskName),
        )
    }

    /// 探测 meta 文件。
    fn ExistsCheckpointMetadata(&self, ctx: &Context) -> Result<bool> {
        self.storage
            .FileExists(ctx, &getCheckpointMetaPathByName(&self.taskName))
    }

    /// 按任务前缀清理外部检查点。
    fn RemoveCheckpointData(&self, ctx: &Context) -> Result<()> {
        let prefix = format!("checkpoints/restore-{}", self.taskName);
        removeCheckpointData(ctx, self.storage.as_ref(), &prefix)
    }

    /// 外部存储 + cipher 启动日志 Runner；lock tick 为 0。
    fn StartCheckpointRunner(
        &self,
        ctx: &Context,
        cfg: tickDurationConfig,
        valueMarshaler: fn(&RangeGroup<LogRestoreKeyType, LogRestoreValueType>) -> Result<Vec<u8>>,
    ) -> Result<CheckpointRunner<LogRestoreKeyType, LogRestoreValueType>> {
        let checkpointStorage = newExternalCheckpointStorage(
            ctx,
            self.storage.clone(),
            None,
            flushPathForRestore(&self.taskName),
        )?;
        let runner = newCheckpointRunner(checkpointStorage, self.cipher.clone(), valueMarshaler);
        runner.startCheckpointMainLoop(
            ctx.clone(),
            cfg.tickDurationForFlush,
            cfg.tickDurationForChecksum,
            Duration::ZERO,
            cfg.retryDuration,
        );
        Ok(runner)
    }

    /// 空 Close。
    fn Close(&self) {}

    /// 从 progress 路径加载进度。
    fn LoadCheckpointProgress(&self, ctx: &Context) -> Result<CheckpointProgress> {
        let mut m = CheckpointProgress::default();
        loadCheckpointMeta(
            ctx,
            self.storage.as_ref(),
            &getCheckpointProgressPathByName(&self.taskName),
            &mut m,
        )?;
        Ok(m)
    }

    /// 将进度写入独立 progress 文件。
    fn SaveCheckpointProgress(&self, ctx: &Context, meta: &CheckpointProgress) -> Result<()> {
        saveCheckpointMetadata(
            ctx,
            self.storage.as_ref(),
            meta,
            &getCheckpointProgressPathByName(&self.taskName),
        )
    }

    /// 探测 progress 文件。
    fn ExistsCheckpointProgress(&self, ctx: &Context) -> Result<bool> {
        self.storage
            .FileExists(ctx, &getCheckpointProgressPathByName(&self.taskName))
    }

    /// 从 ingest-index 路径加载修复 SQL。
    fn LoadCheckpointIngestIndexRepairSQLs(
        &self,
        ctx: &Context,
    ) -> Result<CheckpointIngestIndexRepairSQLs> {
        let mut m = CheckpointIngestIndexRepairSQLs::default();
        loadCheckpointMeta(
            ctx,
            self.storage.as_ref(),
            &getCheckpointIngestIndexPathByName(&self.taskName),
            &mut m,
        )?;
        Ok(m)
    }

    /// 保存修复 SQL 到 ingest-index 路径。
    fn SaveCheckpointIngestIndexRepairSQLs(
        &self,
        ctx: &Context,
        meta: &CheckpointIngestIndexRepairSQLs,
    ) -> Result<()> {
        saveCheckpointMetadata(
            ctx,
            self.storage.as_ref(),
            meta,
            &getCheckpointIngestIndexPathByName(&self.taskName),
        )
    }

    /// 探测 ingest-index 文件。
    fn ExistsCheckpointIngestIndexRepairSQLs(&self, ctx: &Context) -> Result<bool> {
        self.storage
            .FileExists(ctx, &getCheckpointIngestIndexPathByName(&self.taskName))
    }

    /// 返回底层外部存储，供调用方直接访问对象路径。
    fn TryGetStorage(&self) -> Option<Arc<dyn Storage>> {
        Some(self.storage.clone())
    }
}

// 保留泛型约束引用，避免 Serialize/DeserializeOwned 在部分构建下被判定未使用。
// silence unused
fn _unused_table_info() -> TableInfoName {
    TableInfoName::default()
}
fn _unused_ser<T: Serialize + DeserializeOwned>(_t: T) {}
