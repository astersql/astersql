// Copyright 2026 AsterSQL.

// Runaway（失控查询）资源管控 crate 入口。
//
// 资源组（Resource Group）可为查询配置 runaway 规则：当执行耗时、RU（Request Unit，
// 请求单元，衡量读写资源消耗）或 processed keys（已处理键数）超限时，触发 DryRun /
// CoolDown / Kill / SwitchGroup 等动作，并可写入 quarantine watch（隔离监视）列表，
// 使后续相似查询在执行前即被拦截。本模块导出子模块、公共错误类型与配置结构。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]

use std::fmt::{Display, Formatter};
use std::sync::Arc;

/// 单条查询的 runaway 阈值检查器。
pub mod checker;
/// 按阈值/时间窗口批量刷盘的通用缓冲器。
pub mod flusher;
/// 监视列表、记录队列与系统表同步的中枢管理器。
pub mod manager;
/// runaway 查询记录与 quarantine 记录的持久化 SQL 构造。
pub mod record;
/// 从 `mysql.tidb_runaway_watch` / `_done` 系统表增量扫描的同步器。
pub mod syncer;

#[cfg(test)]
#[path = "checker_test.rs"]
mod checker_test;
#[cfg(test)]
#[path = "flusher_test.rs"]
mod flusher_test;
#[cfg(test)]
#[path = "manager_test.rs"]
mod manager_test;
#[cfg(test)]
#[path = "record_test.rs"]
mod record_test;
#[cfg(test)]
#[path = "syncer_test.rs"]
mod syncer_test;

/// 微秒级 Unix 时间戳，与 TiDB runaway 系统表时间列对齐。
pub type Timestamp = i64;

/// 返回当前 UTC 微秒时间戳。
pub fn nowMicros() -> Timestamp {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as i64
}

/// runaway 子系统公共错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// 管理器已停止，拒绝继续入队或刷盘。
    Closed,
    /// 参数非法（例如 flush 阈值为 0）。
    InvalidArgument(String),
    /// 按 ID/组名查找监视记录失败。
    NotFound(String),
    /// 共享锁（Mutex）中毒。
    Poisoned,
    /// 查询因超限被中断（Kill 动作）。
    QueryInterrupted(String),
    /// 查询命中 quarantine watch，在执行前被隔离。
    Quarantined,
    /// 系统表读写或获取自增 ID 失败。
    Storage(String),
}
impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("manager stopped"),
            Self::InvalidArgument(v) | Self::NotFound(v) | Self::Storage(v) => f.write_str(v),
            Self::Poisoned => f.write_str("shared state lock poisoned"),
            Self::QueryInterrupted(v) => write!(f, "query interrupted: {v}"),
            Self::Quarantined => f.write_str("query is quarantined by runaway watch"),
        }
    }
}
impl std::error::Error for Error {}
/// runaway 子系统统一结果类型。
pub type Result<T> = std::result::Result<T, Error>;

/// 触发 runaway 后采取的处置动作。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RunawayAction {
    #[default]
    /// 无动作。
    NoneAction,
    /// 仅记录，不改变执行行为。
    DryRun,
    /// 降优先级（CoolDown），例如压低 Coprocessor 请求优先级。
    CoolDown,
    /// 直接杀掉查询。
    Kill,
    /// 切换到另一个资源组继续执行。
    SwitchGroup,
}
impl Display for RunawayAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoneAction => "NoneAction",
            Self::DryRun => "DryRun",
            Self::CoolDown => "CoolDown",
            Self::Kill => "Kill",
            Self::SwitchGroup => "SwitchGroup",
        })
    }
}

/// quarantine watch 的匹配方式。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RunawayWatchType {
    #[default]
    /// 未指定。
    None,
    /// 精确匹配原始 SQL 文本。
    Exact,
    /// 按 SQL digest（规范化后的指纹）匹配相似语句。
    Similar,
    /// 按执行计划 digest 匹配。
    Plan,
}

/// runaway 触发阈值规则。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RunawayRule {
    /// 执行耗时阈值（毫秒）；0 表示不启用截止时间。
    pub exec_elapsed_time_ms: i64,
    /// RU 消耗阈值；0 表示不检查。
    pub request_unit: i64,
    /// 已处理键数阈值；0 表示不检查。
    pub processed_keys: i64,
}
/// 触发后写入监视列表的配置。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RunawayWatch {
    /// 监视匹配类型（Exact / Similar / Plan）。
    pub kind: RunawayWatchType,
    /// 监视有效期（毫秒）；0 表示不过期。
    pub lasting_duration_ms: i64,
}
/// 资源组上的完整 runaway 设置。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RunawaySettings {
    /// 阈值规则。
    pub rule: RunawayRule,
    /// 超限后的动作。
    pub action: RunawayAction,
    /// SwitchGroup 动作的目标资源组名。
    pub switch_group_name: String,
    /// 可选的 quarantine watch 配置。
    pub watch: Option<RunawayWatch>,
}
/// 资源组元数据中与 runaway 相关的子集。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroup {
    /// 资源组名称。
    pub name: String,
    /// 可选 runaway 设置；为 None 时仅依赖已有 watch 列表。
    pub runaway_settings: Option<RunawaySettings>,
}

/// 按名称查询资源组的目录接口。
pub trait ResourceGroupCatalog: Send + Sync {
    /// 返回指定名称的资源组；不存在时为 `Ok(None)`。
    fn GetResourceGroup(&self, name: &str) -> Result<Option<ResourceGroup>>;
}

/// 受限 SQL 执行器：用于向系统表写入/读取 runaway 记录，不走普通用户会话。
pub trait RestrictedSqlExecutor: Send + Sync {
    /// 执行 SQL 并返回行集合。
    fn Execute(&self, sql: &str, params: &[record::SqlValue]) -> Result<Vec<syncer::SqlRow>>;
    /// 最近一次插入的自增 ID；默认 0 表示尚未拿到。
    fn LastInsertId(&self) -> u64 {
        0
    }
}

/// 空实现执行器，测试与无存储场景使用。
#[derive(Clone, Default)]
pub struct NoopExecutor;
impl RestrictedSqlExecutor for NoopExecutor {
    fn Execute(&self, _sql: &str, _params: &[record::SqlValue]) -> Result<Vec<syncer::SqlRow>> {
        Ok(Vec::new())
    }
}

/// Coprocessor（协处理器，下推到 TiKV 的计算任务）请求上可由 runaway 改写的字段。
#[derive(Clone, Debug, Default)]
pub struct CopRequest {
    /// 覆盖优先级；CoolDown 时通常设为最低优先级 1。
    pub override_priority: Option<u32>,
    /// 实际使用的资源组名；SwitchGroup 时可能被改写。
    pub resource_group_name: String,
    /// 最大执行时长（毫秒），Kill 动作下可收紧到剩余截止时间。
    pub max_execution_duration_ms: u64,
}

/// 一次请求的 RU 明细（读写分别累计）。
#[derive(Clone, Copy, Debug, Default)]
pub struct RUDetails {
    /// 写路径消耗的 RU。
    pub write_ru: f64,
    /// 读路径消耗的 RU。
    pub read_ru: f64,
}

/// 共享资源组目录引用。
pub type CatalogRef = Arc<dyn ResourceGroupCatalog>;
/// 共享受限 SQL 执行器引用。
pub type ExecutorRef = Arc<dyn RestrictedSqlExecutor>;
