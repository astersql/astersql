// Copyright 2019 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// Lightning 导入后端抽象：引擎生命周期、本地写入与导入重试。
//
// Backend 负责打开/关闭/导入引擎；EngineManager 按表名与引擎 ID 生成稳定 UUID；
// OpenedEngine / ClosedEngine 分别表示可写入与可导入阶段。Region 拆分参数控制
// 导入时对 TiKV Region（键范围分片）的切分粒度。

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use encode::{Context, Rows};
use uuid::Uuid;

/// ImportEngine 可重试失败的最大次数。
const importMaxRetryTimes: usize = 3;
/// 用于基于表名+引擎 ID 派生稳定 UUID 的命名空间。
const ENGINE_NAMESPACE: Uuid = Uuid::from_u128(0xd68d6abec59e45d6ade8e2b0ceb7bedf);

/// 拼接引擎日志标签：`表名:引擎ID`。
fn makeTag(tableName: &str, engineID: i64) -> String {
    format!("{tableName}:{engineID}")
}

/// 在既有 logger 上附加 engineTag 与 engineUUID 字段。
fn makeLogger(logger: &Logger, tag: &str, engineUUID: Uuid) -> Logger {
    logger
        .clone()
        .with("engineTag", tag)
        .with("engineUUID", &engineUUID.to_string())
}

/// 由表名与引擎 ID 生成日志标签及确定性 UUID（UUID v5）。
pub fn MakeUUID(tableName: &str, engineID: i64) -> (String, Uuid) {
    let tag = makeTag(tableName, engineID);
    let uuid = Uuid::new_v5(&ENGINE_NAMESPACE, tag.as_bytes());
    (tag, uuid)
}

/// 轻量结构化日志字段容器（键值对列表）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Logger {
    /// 已附加的键值字段。
    pub fields: Vec<(String, String)>,
}

impl Logger {
    /// 追加一个字段并返回 self，便于链式调用。
    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.fields.push((key.to_string(), value.to_string()));
        self
    }
}

/// 引擎占用的磁盘/内存规模及是否正在导入。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EngineFileSize {
    /// 引擎 UUID。
    pub UUID: Uuid,
    /// 磁盘占用字节数。
    pub DiskSize: i64,
    /// 内存占用字节数。
    pub MemSize: i64,
    /// 是否处于导入中。
    pub IsImporting: bool,
}

/// 本地写入器配置，分 Local 与 TiDB 两路后端差异字段。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalWriterConfig {
    /// Local 后端写入参数。
    pub Local: LocalWriterLocalConfig,
    /// TiDB 后端写入参数。
    pub TiDB: LocalWriterTiDBConfig,
}

/// Local 后端：是否已排序 KV、内存缓存大小。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalWriterLocalConfig {
    /// 写入的 KV 是否已按键有序。
    pub IsKVSorted: bool,
    /// 写缓冲内存上限。
    pub MemCacheSize: i64,
}

/// TiDB 后端：目标表名。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalWriterTiDBConfig {
    /// 目标表名。
    pub TableName: String,
}

/// 打开引擎时的综合配置（表信息、本地/外部引擎、时间戳等）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EngineConfig {
    /// 可选表元信息。
    pub TableInfo: Option<TableInfo>,
    /// Local 引擎压缩与块大小等。
    pub Local: LocalEngineConfig,
    /// 外部存储引擎配置（若使用）。
    pub External: Option<ExternalEngineConfig>,
    /// 是否保留排序临时目录。
    pub KeepSortDir: bool,
    /// 写入使用的时间戳（TS）。
    pub TS: u64,
}

/// 远程表列元数据。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnInfo {
    pub name: String,
    pub offset: usize,
    pub public: bool,
    pub unsigned: bool,
    pub auto_increment: bool,
    pub primary_key: bool,
    pub generated_expression: String,
}

/// 表标识及逻辑导入需要的完整远程模型。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
    /// 表名。
    pub name: String,
    /// 表 ID。
    pub id: i64,
    pub public: bool,
    pub pk_is_handle: bool,
    pub auto_random_bits: u64,
    pub columns: Vec<ColumnInfo>,
}

/// Local 引擎的压缩与块参数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalEngineConfig {
    /// 是否在适当时机触发 compact。
    pub Compact: bool,
    /// 触发 compact 的阈值阈值。
    pub CompactThreshold: i64,
    /// compact 并发度。
    pub CompactConcurrency: usize,
    /// 数据块大小。
    pub BlockSize: usize,
}

/// 外部引擎：数据/统计文件、键范围、热点检测与重复键策略等。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExternalEngineConfig {
    /// 外部对象存储标识。
    pub ExtStore: Option<String>,
    /// 数据文件路径列表。
    pub DataFiles: Vec<String>,
    /// 统计文件路径列表。
    pub StatFiles: Vec<String>,
    /// 扫描起始键。
    pub StartKey: Vec<u8>,
    /// 扫描结束键。
    pub EndKey: Vec<u8>,
    /// 作业切分键。
    pub JobKeys: Vec<Vec<u8>>,
    /// Region 拆分键。
    pub SplitKeys: Vec<Vec<u8>>,
    /// 文件总大小。
    pub TotalFileSize: i64,
    /// KV 总条数。
    pub TotalKVCount: i64,
    /// 是否检测热点。
    pub CheckHotspot: bool,
    /// 内存容量上限。
    pub MemCapacity: i64,
    /// 遇到重复主键时的处理策略。
    pub OnDup: OnDuplicateKey,
    /// 文件名前缀。
    pub FilePrefix: String,
}

/// 重复主键处理策略：报错、替换或忽略。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OnDuplicateKey {
    #[default]
    Error,
    Replace,
    Ignore,
}

/// 导入前环境检查上下文，携带待检查的库元信息列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CheckCtx {
    /// 远端/本地数据库元信息标识列表。
    pub DBMetas: Vec<String>,
}

/// 远端数据库简要信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DatabaseInfo {
    /// 数据库名。
    pub name: String,
}

/// 从目标集群拉取库表模型并执行导入前置检查。
pub trait TargetInfoGetter: Send + Sync {
    /// 拉取远端全部数据库模型。
    fn FetchRemoteDBModels(&self, ctx: &Context) -> Result<Vec<DatabaseInfo>, BackendError>;
    /// 按 schema 与表名列表拉取远端表模型。
    fn FetchRemoteTableModels(
        &self,
        ctx: &Context,
        schemaName: &str,
        tableNames: &[String],
    ) -> Result<HashMap<String, TableInfo>, BackendError>;
    /// 校验导入前置条件（权限、版本、空间等）。
    fn CheckRequirements(&self, ctx: &Context, checkCtx: &CheckCtx) -> Result<(), BackendError>;
}

/// 导入后端核心接口：引擎打开/关闭/导入/清理与本地写入器创建。
pub trait Backend: Send + Sync {
    /// 关闭后端并释放资源。
    fn Close(&self);
    /// 导入失败可重试时的等待间隔。
    fn RetryImportDelay(&self) -> Duration;
    /// 导入完成后是否需要后处理。
    fn ShouldPostProcess(&self) -> bool;
    /// 打开指定 UUID 的引擎。
    fn OpenEngine(
        &self,
        ctx: &Context,
        config: &EngineConfig,
        engineUUID: Uuid,
    ) -> Result<(), BackendError>;
    /// 关闭引擎；config 可选，用于收尾参数。
    fn CloseEngine(
        &self,
        ctx: &Context,
        config: Option<&EngineConfig>,
        engineUUID: Uuid,
    ) -> Result<(), BackendError>;
    /// 将引擎数据导入到存储，并按 Region 拆分大小/键数切分。
    fn ImportEngine(
        &self,
        ctx: &Context,
        engineUUID: Uuid,
        regionSplitSize: i64,
        regionSplitKeys: i64,
    ) -> Result<(), BackendError>;
    /// 清理引擎临时数据。
    fn CleanupEngine(&self, ctx: &Context, engineUUID: Uuid) -> Result<(), BackendError>;
    /// 刷写单个引擎缓冲。
    fn FlushEngine(&self, ctx: &Context, engineUUID: Uuid) -> Result<(), BackendError>;
    /// 刷写全部引擎缓冲。
    fn FlushAllEngines(&self, ctx: &Context) -> Result<(), BackendError>;
    /// 为指定引擎创建本地行写入器。
    fn LocalWriter(
        &self,
        ctx: &Context,
        cfg: &LocalWriterConfig,
        engineUUID: Uuid,
    ) -> Result<Box<dyn EngineWriter>, BackendError>;
}

/// 基于 Backend 的引擎管理器：按表打开或不安全关闭引擎。
#[derive(Clone)]
pub struct EngineManager {
    backend: Arc<dyn Backend>,
}

/// 引擎内部句柄：后端、日志、UUID 与数值 ID。
#[derive(Clone)]
struct engine {
    backend: Arc<dyn Backend>,
    logger: Logger,
    uuid: Uuid,
    id: i32,
}

/// 已打开、可写入的引擎句柄。
pub struct OpenedEngine {
    engine: engine,
    /// 关联表名。
    pub tableName: String,
    config: EngineConfig,
}

/// 由 Backend 句柄构造 EngineManager。
pub fn MakeEngineManager(ab: Arc<dyn Backend>) -> EngineManager {
    EngineManager { backend: ab }
}

impl EngineManager {
    /// 打开表对应引擎并返回 OpenedEngine。
    pub fn OpenEngine(
        &self,
        ctx: &Context,
        config: &EngineConfig,
        tableName: &str,
        engineID: i32,
    ) -> Result<OpenedEngine, BackendError> {
        let (tag, uuid) = MakeUUID(tableName, i64::from(engineID));
        self.backend.OpenEngine(ctx, config, uuid)?;
        Ok(OpenedEngine {
            engine: engine {
                backend: Arc::clone(&self.backend),
                logger: makeLogger(&Logger::default(), &tag, uuid),
                uuid,
                id: engineID,
            },
            tableName: tableName.to_string(),
            config: config.clone(),
        })
    }

    /// 按表名与引擎 ID 关闭引擎（不要求持有 OpenedEngine），返回 ClosedEngine。
    pub fn UnsafeCloseEngine(
        &self,
        ctx: &Context,
        cfg: Option<&EngineConfig>,
        tableName: &str,
        engineID: i32,
    ) -> Result<ClosedEngine, BackendError> {
        let (tag, uuid) = MakeUUID(tableName, i64::from(engineID));
        self.UnsafeCloseEngineWithUUID(ctx, cfg, &tag, uuid, engineID)
    }

    /// 按已知 tag/UUID 关闭引擎。
    pub fn UnsafeCloseEngineWithUUID(
        &self,
        ctx: &Context,
        cfg: Option<&EngineConfig>,
        tag: &str,
        engineUUID: Uuid,
        id: i32,
    ) -> Result<ClosedEngine, BackendError> {
        engine {
            backend: Arc::clone(&self.backend),
            logger: makeLogger(&Logger::default(), tag, engineUUID),
            uuid: engineUUID,
            id,
        }
        .unsafeClose(ctx, cfg)
    }
}

impl engine {
    /// 调用后端 CloseEngine 并包装为 ClosedEngine。
    fn unsafeClose(
        self,
        ctx: &Context,
        cfg: Option<&EngineConfig>,
    ) -> Result<ClosedEngine, BackendError> {
        self.backend.CloseEngine(ctx, cfg, self.uuid)?;
        Ok(ClosedEngine { engine: self })
    }

    /// 返回数值引擎 ID。
    pub fn GetID(&self) -> i32 {
        self.id
    }

    /// 返回引擎 UUID。
    pub fn GetUUID(&self) -> Uuid {
        self.uuid
    }
}

impl OpenedEngine {
    /// 关闭打开中的引擎，进入可导入的 ClosedEngine 状态。
    pub fn Close(self, ctx: &Context) -> Result<ClosedEngine, BackendError> {
        let config = self.config.clone();
        self.engine.unsafeClose(ctx, Some(&config))
    }

    /// 刷写当前引擎缓冲到持久层。
    pub fn Flush(&self, ctx: &Context) -> Result<(), BackendError> {
        self.engine.backend.FlushEngine(ctx, self.engine.uuid)
    }

    /// 创建绑定本引擎的本地行写入器。
    pub fn LocalWriter(
        &self,
        ctx: &Context,
        cfg: &LocalWriterConfig,
    ) -> Result<Box<dyn EngineWriter>, BackendError> {
        self.engine.backend.LocalWriter(ctx, cfg, self.engine.uuid)
    }

    /// 返回引擎 UUID。
    pub fn GetEngineUUID(&self) -> Uuid {
        self.engine.uuid
    }

    /// 返回数值引擎 ID。
    pub fn GetID(&self) -> i32 {
        self.engine.GetID()
    }
}

/// 已关闭、可执行 Import/Cleanup 的引擎句柄。
pub struct ClosedEngine {
    engine: engine,
}

/// 从已有组件直接构造 ClosedEngine（用于恢复或不安全关闭路径）。
pub fn NewClosedEngine(
    backend: Arc<dyn Backend>,
    logger: Logger,
    uuid: Uuid,
    id: i32,
) -> ClosedEngine {
    ClosedEngine {
        engine: engine {
            backend,
            logger,
            uuid,
            id,
        },
    }
}

impl ClosedEngine {
    /// 导入引擎数据；可重试错误会按 RetryImportDelay 退避，直至成功或达最大次数。
    pub fn Import(
        &self,
        ctx: &Context,
        regionSplitSize: i64,
        regionSplitKeys: i64,
    ) -> Result<(), BackendError> {
        let mut last = None;
        // 对 retryable 错误循环重试，非可重试错误立即返回。
        for _ in 0..importMaxRetryTimes {
            match self.engine.backend.ImportEngine(
                ctx,
                self.engine.uuid,
                regionSplitSize,
                regionSplitKeys,
            ) {
                Ok(()) => return Ok(()),
                Err(error) if !error.retryable => return Err(error),
                Err(error) => {
                    last = Some(error);
                    thread::sleep(self.engine.backend.RetryImportDelay());
                }
            }
        }
        let error = last.expect("retry loop always records an error");
        Err(BackendError {
            message: format!(
                "[{}] import reach max retry {} and still failed: {}",
                self.engine.uuid, importMaxRetryTimes, error
            ),
            retryable: error.retryable,
            duplicate: error.duplicate,
        })
    }

    /// 清理引擎残留文件/状态。
    pub fn Cleanup(&self, ctx: &Context) -> Result<(), BackendError> {
        self.engine.backend.CleanupEngine(ctx, self.engine.uuid)
    }

    /// 返回绑定的 logger。
    pub fn Logger(&self) -> &Logger {
        &self.engine.logger
    }

    /// 返回数值引擎 ID。
    pub fn GetID(&self) -> i32 {
        self.engine.GetID()
    }

    /// 返回引擎 UUID。
    pub fn GetUUID(&self) -> Uuid {
        self.engine.GetUUID()
    }
}

/// 一次 chunk 刷写是否已落盘的状态。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChunkFlushStatus {
    /// 是否已刷写完成。
    pub flushed: bool,
}

/// 向引擎追加行数据的写入器接口。
pub trait EngineWriter: Send {
    /// 追加一批行；columnNames 与 rows 列对齐。
    fn AppendRows(
        &mut self,
        ctx: &Context,
        columnNames: &[String],
        rows: &dyn Rows,
    ) -> Result<(), BackendError>;
    /// 写入是否已与后端同步。
    fn IsSynced(&self) -> bool;
    /// 关闭写入器并返回刷写状态。
    fn Close(&mut self, ctx: &Context) -> Result<Option<ChunkFlushStatus>, BackendError>;
}

/// 后端操作错误：消息、是否可重试、是否因重复键。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendError {
    /// 错误描述。
    pub message: String,
    /// 调用方是否可按退避策略重试。
    pub retryable: bool,
    /// 是否与重复键相关。
    pub duplicate: bool,
}

impl BackendError {
    /// 构造不可重试错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            duplicate: false,
        }
    }

    /// 构造可重试错误。
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
            duplicate: false,
        }
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BackendError {}
