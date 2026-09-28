// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 外排（external sort）抽象接口。
//
// 定义可落盘的键值排序器契约：多 Writer 写入、统一 `sort`、再通过 Iterator
// 有序扫描；实现需去重重复 key。对应 Go `pkg/util/extsort` 的 ExternalSorter。

use std::error::Error as StdError;
use tokio_util::sync::CancellationToken;

/// 外排相关错误的统一装箱类型。
pub type Error = Box<dyn StdError + Send + Sync + 'static>;
/// 外排 API 的标准 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;

/// 在外部存储中对键值对排序。
///
/// Sorts key-value pairs in external storage. Implementations remove duplicate
/// keys and support multiple independent writers and iterators.
///
/// 实现需：写入阶段允许多个独立 Writer；`sort` 后去重并保证有序；
/// 可创建多个 Iterator；`close_and_cleanup` 释放临时文件。
pub trait ExternalSorter: Send + Sync {
    /// 创建新的写入器；开始排序或已排序后必须返回错误，`ctx` 取消时应尽快失败。
    /// 多个写入器可以并发使用。
    fn new_writer(&self, ctx: &CancellationToken) -> Result<Box<dyn Writer>>;
    /// 将已写入数据排序并落盘为可读状态。
    ///
    /// 必须在所有写入器关闭后调用；实现须保证该操作幂等且原子。返回错误或进程在
    /// 排序期间退出后，后续调用必须可以恢复，且不得损坏外部存储。
    fn sort(&self, ctx: &CancellationToken) -> Result<()>;
    /// 是否已完成排序（完成后禁止再写）。
    fn is_sorted(&self) -> bool;
    /// 创建有序迭代器；未排序时返回错误。
    fn new_iterator(&self, ctx: &CancellationToken) -> Result<Box<dyn Iterator>>;
    /// 关闭排序器（可不删除数据文件）。
    fn close(&self) -> Result<()>;
    /// 关闭并清理临时目录/文件。
    fn close_and_cleanup(&self) -> Result<()>;
}

/// 外排写入器：缓冲键值并刷入底层存储。
pub trait Writer: Send {
    /// 写入一对 key/value。
    ///
    /// The implementation copies both slices before returning.
    /// 实现须在返回前复制两个切片，调用方缓冲区可立即复用。
    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<()>;
    /// 将缓冲数据刷入磁盘。
    fn flush(&mut self) -> Result<()>;
    /// 关闭写入器（通常先 flush）。
    fn close(&mut self) -> Result<()>;
}

/// 外排有序迭代器，语义类似 RocksDB Iterator。
pub trait Iterator: Send {
    /// 定位到第一个 `>= key` 的条目。
    fn seek(&mut self, key: &[u8]) -> bool;
    /// 定位到第一条记录。
    fn first(&mut self) -> bool;
    /// 前进到下一条；下一条记录的 key 必须严格大于当前 key。
    fn next(&mut self) -> bool;
    /// 定位到最后一条记录。
    fn last(&mut self) -> bool;
    /// 当前位置是否有效。
    fn valid(&self) -> bool;
    /// 最近一次操作产生的错误（若有）。
    fn error(&self) -> Option<&(dyn StdError + Send + Sync + 'static)>;
    /// Move out the original error without formatting or wrapping it. Call this
    /// after a failed operation and before close; the error remains valid after
    /// the iterator is destroyed. A second call returns None. After transferring
    /// an error, callers must close or reposition the iterator before reading.
    fn take_error(&mut self) -> Option<Error>;
    /// 当前 key；仅在 `valid()` 为真时安全，且可能指向内部缓冲。
    fn unsafe_key(&self) -> &[u8];
    /// 当前 value；约定同 `unsafe_key`。
    fn unsafe_value(&self) -> &[u8];
    /// 关闭迭代器并释放资源。
    fn close(&mut self) -> Result<()>;
}

/// Both the iterator operation and its cleanup failed (Go errors.Join).
/// The primary error is also exposed through std::error::Error::source;
/// callers can inspect both original errors through errors().
#[derive(Debug)]
pub struct JoinedError {
    errors: [Error; 2],
}
impl JoinedError {
    pub fn errors(&self) -> &[Error; 2] {
        &self.errors
    }
}
impl std::fmt::Display for JoinedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}\n{}", self.errors[0], self.errors[1])
    }
}
impl StdError for JoinedError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.errors[0].as_ref())
    }
}

pub(crate) fn join_errors(primary: Option<Error>, cleanup: Result<()>) -> Option<Error> {
    match (primary, cleanup) {
        (Some(primary), Err(cleanup)) => Some(Box::new(JoinedError {
            errors: [primary, cleanup],
        })),
        (Some(error), Ok(())) | (None, Err(error)) => Some(error),
        (None, Ok(())) => None,
    }
}
