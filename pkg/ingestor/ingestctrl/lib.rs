// Copyright 2026 AsterSQL.

// ingestctrl：Lightning local backend 的 ingest 控制面。
//
// 负责将已排序的 KV 以 SST（Sorted String Table）形式写入本地 Engine，再导入 TiKV Region。
// 本文件声明子模块、统一错误类型、引擎 ID、键值对/键范围、重复键处理策略与取消令牌。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]

use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// 本地/远端校验和（checksum）计算与比对。
pub mod checksum;
/// Gzip 压缩与解压封装。
pub mod compress;
/// 磁盘配额监控与超限处理。
pub mod disk_quota;
/// 重复键检测与冲突处理。
pub mod duplicate;
/// 本地 ingest Engine（Pebble 风格 KV 存储）。
pub mod engine;
/// 多 Engine 生命周期管理。
pub mod engine_mgr;
/// Worker 生命周期、生成、重试与结果分发。
pub mod import_pipeline;
/// 有序迭代器与重复键适配器。
pub mod iterator;
/// Region 导入任务 Worker。
pub mod job_worker;
/// Local backend：版本检查、导入流程与 Backend 实现。
pub mod local;
/// FreeBSD 平台 rlimit 日志辅助。
pub mod local_freebsd;
/// Unix 平台打开文件数等资源限制校验。
pub mod local_unix;
/// 通用 Unix rlimit 实现。
pub mod local_unix_generic;
/// Windows 平台资源限制占位实现。
pub mod local_windows;
/// Local backend 辅助函数。
pub mod localhelper;
/// 导入速率限制。
pub mod rate_limiter;
/// 速率限制参数。
pub mod rate_limiter_param;
/// Region 任务调度相关。
pub mod region_job;
/// TiKV 导入模式相关。
pub mod tikv_mode;

#[cfg(test)]
/// checksum 单元测试。
mod checksum_test;
#[cfg(test)]
/// compress 单元测试。
mod compress_test;
#[cfg(test)]
/// disk_quota 单元测试。
mod disk_quota_test;
#[cfg(test)]
/// duplicate 单元测试。
mod duplicate_test;
#[cfg(test)]
/// engine_mgr 单元测试。
mod engine_mgr_test;
#[cfg(test)]
/// engine 单元测试。
mod engine_test;
#[cfg(test)]
/// iterator 单元测试。
mod iterator_test;
#[cfg(test)]
/// job_worker 单元测试。
mod job_worker_test;
#[cfg(test)]
/// local 前置检查单元测试。
mod local_check_test;
#[cfg(test)]
/// local backend 综合单元测试。
mod local_test;
#[cfg(test)]
/// Unix rlimit parity tests.
mod local_unix_test;
#[cfg(test)]
/// Windows rlimit parity tests.
mod local_windows_test;
#[cfg(test)]
/// localhelper 单元测试。
mod localhelper_test;
#[cfg(test)]
/// rate_limiter 单元测试。
mod rate_limiter_test;
#[cfg(test)]
/// region_job 单元测试。
mod region_job_test;
#[cfg(test)]
/// TiKV mode switcher parity tests.
mod tikv_mode_test;

#[derive(Clone, Debug, Eq, PartialEq)]
/// ingestctrl 统一错误类型。
pub enum Error {
    /// 操作被取消令牌取消。
    Cancelled,
    /// Engine 已关闭。
    Closed,
    /// 发现重复键冲突。
    Conflict { key: Vec<u8>, value: Vec<u8> },
    /// 磁盘配额超限。
    DiskQuotaExceeded { used: i64, quota: i64 },
    /// 非法参数。
    InvalidArgument(String),
    /// 数据不合法。
    InvalidData(String),
    /// IO 错误。
    Io(String),
    /// 资源未找到。
    NotFound(String),
    /// 共享锁被毒化（poisoned）。
    Poisoned,
    /// 可重试错误。
    Retryable(String),
    /// 操作超时。
    Timeout,
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("operation cancelled"),
            Self::Closed => f.write_str("engine is closed"),
            Self::Conflict { key, .. } => write!(f, "duplicate key found: {key:?}"),
            Self::DiskQuotaExceeded { used, quota } => {
                write!(f, "disk quota exceeded: used {used}, quota {quota}")
            }
            Self::InvalidArgument(message)
            | Self::InvalidData(message)
            | Self::Io(message)
            | Self::NotFound(message)
            | Self::Retryable(message) => f.write_str(message),
            Self::Poisoned => f.write_str("shared state lock is poisoned"),
            Self::Timeout => f.write_str("operation timed out"),
        }
    }
}

impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}
/// 本 crate 的 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
/// 本地 Engine 唯一标识（内部递增 u128）。
pub struct EngineId(pub u128);

impl EngineId {
    /// 分配下一个 EngineId。
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed) as u128)
    }
}

impl Display for EngineId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 键值对。
pub struct KvPair {
    /// 键字节。
    pub key: Vec<u8>,
    /// 值字节。
    pub value: Vec<u8>,
}

impl KvPair {
    /// 键与值的字节长度之和。
    pub fn size(&self) -> usize {
        self.key.len() + self.value.len()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 半开键范围 [start, end)；空 end 表示无上界。
pub struct KeyRange {
    /// 范围起始键（含）。
    pub start: Vec<u8>,
    /// 范围结束键（不含）；空表示无上界。
    pub end: Vec<u8>,
}

impl KeyRange {
    /// 判断两个键范围是否相交。
    pub fn overlaps(&self, other: &Self) -> bool {
        (self.end.is_empty() || other.start < self.end)
            && (other.end.is_empty() || self.start < other.end)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 重复键处理策略。
pub enum DuplicateResolution {
    #[default]
    /// 不处理。
    None,
    /// 遇重复报错。
    Error,
    /// 删除重复。
    Remove,
    /// 记录重复但不中断。
    Record,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 冲突统计：重复条数与占用字节。
pub struct ConflictInfo {
    /// 冲突条数。
    pub count: u64,
    /// 冲突占用字节数。
    pub size: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Engine 磁盘/内存占用与是否正在 import。
pub struct EngineFileSize {
    /// Engine 标识。
    pub UUID: EngineId,
    /// 磁盘占用字节。
    pub DiskSize: i64,
    /// 内存占用字节。
    pub MemSize: i64,
    /// 是否正持有 import 锁。
    pub IsImporting: bool,
}

#[derive(Clone, Default)]
/// 协作式取消令牌（CancellationToken）。
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    parent: Option<Arc<CancellationToken>>,
    worker_context: Option<astersql_resourcemanager_pool_workerpool::Context>,
}

impl CancellationToken {
    pub(crate) fn for_workers(
        &self,
        context: astersql_resourcemanager_pool_workerpool::Context,
    ) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            parent: Some(Arc::new(self.clone())),
            worker_context: Some(context),
        }
    }

    /// 发出取消信号。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// 是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self.parent.as_ref().is_some_and(|p| p.is_cancelled())
            || self
                .worker_context
                .as_ref()
                .is_some_and(|c| c.IsCancelled())
    }

    /// 已取消则返回 `Error::Cancelled`。
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
