// Copyright 2026 AsterSQL.

// Store driver 事务（txn）适配层 crate 入口。
//
// 聚合快照、扫描器、联合迭代、批量获取与错误格式化等子模块，并定义
// 键值条目、点查/批量查选项及公共 Getter / BatchGetter trait。
// commit_ts（提交时间戳）用于 MVCC 版本可见性；空值表示删除墓碑。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::HashMap;

/// 三层批量获取适配器。
mod batch_getter;
/// 事务驱动错误类型与键美化打印。
mod error;
/// 键范围扫描器。
mod scanner;
/// TiKV 风格快照与拦截器。
mod snapshot;
/// 事务驱动主体。
mod txn_driver;
/// 联合迭代（缓冲与快照合并扫描）。
mod union_iter;
/// UnionStore 驱动封装。
mod unionstore_driver;

pub use batch_getter::*;
pub use error::*;
pub use scanner::*;
pub use snapshot::*;
pub use txn_driver::*;
pub use union_iter::*;
pub use unionstore_driver::*;

/// TiDB keys are byte strings ordered lexicographically.
///
/// TiDB 键为按字典序比较的字节串。
pub type Key = Vec<u8>;

/// Value plus the commit timestamp returned by a snapshot read.
///
/// 快照读返回的值及其提交时间戳（commit_ts）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ValueEntry {
    /// 值载荷；空切片表示删除墓碑。
    pub value: Vec<u8>,
    /// 写入该版本的提交时间戳；未请求返回时通常为 0。
    pub commit_ts: u64,
}

impl ValueEntry {
    /// 构造带指定 commit_ts 的值条目。
    pub fn new(value: impl Into<Vec<u8>>, commit_ts: u64) -> Self {
        Self {
            value: value.into(),
            commit_ts,
        }
    }

    /// 值是否为空（删除墓碑判定）。
    pub fn is_value_empty(&self) -> bool {
        self.value.is_empty()
    }
}

/// Options are opaque to this adapter, but their ordering is retained when
/// batch options are converted to per-key get options.
///
/// 点查选项；对本适配器不透明，批量转点查时保留顺序。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GetOption(pub String);

/// 批量获取选项（字符串标签，与点查选项同名空间）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchGetOption(pub String);

/// 请求在 ValueEntry 中返回 commit_ts 的选项名。
pub const RETURN_COMMIT_TS: &str = "return_commit_ts";

/// WithReturnCommitTS mirrors kv.WithReturnCommitTS for this driver adapter.
///
/// 点查时要求返回 commit_ts（镜像 kv.WithReturnCommitTS）。
pub fn WithReturnCommitTS() -> GetOption {
    GetOption(RETURN_COMMIT_TS.to_owned())
}

/// WithReturnCommitTSBatch is the batch-get form of [`WithReturnCommitTS`].
///
/// 批量获取时要求返回 commit_ts。
pub fn WithReturnCommitTSBatch() -> BatchGetOption {
    BatchGetOption(RETURN_COMMIT_TS.to_owned())
}

/// 点查选项列表是否包含返回 commit_ts。
pub fn wants_return_commit_ts_get(options: &[GetOption]) -> bool {
    options.iter().any(|option| option.0 == RETURN_COMMIT_TS)
}

/// 批量选项列表是否包含返回 commit_ts。
pub fn wants_return_commit_ts_batch(options: &[BatchGetOption]) -> bool {
    options.iter().any(|option| option.0 == RETURN_COMMIT_TS)
}

/// 若未请求返回 commit_ts，则将其清零后返回条目。
pub fn apply_commit_ts_option(mut entry: ValueEntry, options: &[GetOption]) -> ValueEntry {
    if !wants_return_commit_ts_get(options) {
        entry.commit_ts = 0;
    }
    entry
}

/// 批量版：未请求返回 commit_ts 时清零。
pub fn apply_commit_ts_option_batch(
    mut entry: ValueEntry,
    options: &[BatchGetOption],
) -> ValueEntry {
    if !wants_return_commit_ts_batch(options) {
        entry.commit_ts = 0;
    }
    entry
}

/// 将批量选项按顺序转为点查选项（保留标签字符串）。
pub fn BatchGetToGetOptions(options: &[BatchGetOption]) -> Vec<GetOption> {
    options
        .iter()
        .map(|option| GetOption(option.0.clone()))
        .collect()
}

/// Common iterator contract used by snapshots, memory buffers and union scans.
///
/// 快照、内存缓冲与联合扫描共用的迭代器契约。
pub trait KvIterator: Send {
    /// 前进到下一个键值对。
    fn next(&mut self) -> Result<(), DriverError>;
    /// 当前键。
    fn key(&self) -> &[u8];
    /// 当前值。
    fn value(&self) -> &[u8];
    /// 当前位置是否有效。
    fn valid(&self) -> bool;
    /// 关闭并释放资源。
    fn close(&mut self);
}

/// Basic point-get interface used by the three-layer batch getter.
///
/// 三层批量获取使用的点查接口。
pub trait Getter: Send + Sync {
    /// 按键点查，返回值条目或错误。
    fn get(&self, key: &[u8], options: &[GetOption]) -> Result<ValueEntry, DriverError>;
}

/// Storage batch-get interface.
///
/// 存储批量获取接口。
pub trait BatchGetter: Send + Sync {
    /// 批量获取多个键。
    fn batch_get(
        &self,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError>;
}

/// Memory-buffer batch-get interface.
///
/// 内存写缓冲的批量获取接口（含长度查询）。
pub trait BatchBufferGetter: Getter {
    /// 缓冲中条目数量。
    fn len(&self) -> usize;
    /// 按字节键批量获取缓冲内容。
    fn batch_get_bytes(
        &self,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError>;
}

#[cfg(test)]
#[path = "batch_getter_test.rs"]
mod batch_getter_test;
#[cfg(test)]
#[path = "driver_test.rs"]
mod driver_test;
#[cfg(test)]
#[path = "error_test.rs"]
mod error_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "snapshot_test.rs"]
mod snapshot_test;
#[cfg(test)]
#[path = "txn_driver_test.rs"]
mod txn_driver_test;
#[cfg(test)]
#[path = "union_iter_test.rs"]
mod union_iter_test;
#[cfg(test)]
#[path = "unionstore_driver_test.rs"]
mod unionstore_driver_test;
