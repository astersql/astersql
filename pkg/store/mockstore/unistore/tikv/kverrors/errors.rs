// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// unistore mock TiKV 的 KV/事务错误类型定义。
//
// 对应 Go `errors.go`：锁冲突、可重试错误、写冲突、死锁、
// 断言失败等，供 MVCC 读写路径向上返回，并由客户端决定退避或清理。

use crate::{deadlockpb, kvrpcpb, mvcc};
use std::borrow::Cow;
use std::fmt;
use std::sync::LazyLock;

/// 为错误类型实现 Display + std::error::Error，委托到 `Error()` 文案。
macro_rules! impl_std_error {
    ($error:ty) => {
        impl fmt::Display for $error {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.Error())
            }
        }

        impl std::error::Error for $error {}
    };
}

// 读/写遇到键上已有锁时返回；客户端应退避或清理锁后重试。
// ErrLocked is returned when trying to Read/Write on a locked key. Client should
// backoff or cleanup the lock then retry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 键被锁定错误。
pub struct ErrLocked {
    /// 被锁住的键。
    /// 已存在的键。
    pub Key: Vec<u8>,
    /// 当前持有的 MVCC 锁信息。
    /// 实际看到的锁（其 Primary 与预期不符）。
    pub Lock: Box<mvcc::Lock>,
}

// 构造 ErrLocked 并装箱返回。
// BuildLockErr generates ErrKeyLocked objects.
/// 由 key 与 lock 生成 `ErrLocked`。
pub fn BuildLockErr(key: Vec<u8>, lock: Box<mvcc::Lock>) -> Box<ErrLocked> {
    Box::new(ErrLocked {
        Key: key,
        Lock: lock,
    })
}

// Error 将锁格式化为字符串（hex key + Lock.String）。
// Error formats the lock to a string.
/// ErrLocked 的人类可读消息。
impl ErrLocked {
    /// 返回与 Go 对齐的锁错误描述。
    pub fn Error(&self) -> String {
        format!(
            "key is locked, key: {}, lock: {}",
            hex::encode(&self.Key),
            self.Lock.String(),
        )
    }
}

impl_std_error!(ErrLocked);

// 提示客户端可重启事务（例如写冲突）。
// ErrRetryable suggests that client may restart the txn. e.g. write conflict.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
/// 可重试错误，携带原因字符串。
pub struct ErrRetryable(pub Cow<'static, str>);

/// 从静态字符串构造。
impl From<&'static str> for ErrRetryable {
    fn from(message: &'static str) -> Self {
        Self(Cow::Borrowed(message))
    }
}

/// 从拥有所有权的字符串构造。
impl From<String> for ErrRetryable {
    fn from(message: String) -> Self {
        Self(Cow::Owned(message))
    }
}

/// ErrRetryable 消息格式化。
impl ErrRetryable {
    /// 前缀 `retryable: ` + 原因。
    pub fn Error(&self) -> String {
        format!("retryable: {}", self.0)
    }
}

impl_std_error!(ErrRetryable);

// 预定义可重试错误，顺序与 Go var 块一致。
// ErrRetryable values kept in the same order as the Go var block.
/// 锁未找到。
pub static ErrLockNotFound: LazyLock<ErrRetryable> =
    LazyLock::new(|| ErrRetryable::from("lock not found"));
/// 事务已回滚。
pub static ErrAlreadyRollback: LazyLock<ErrRetryable> =
    LazyLock::new(|| ErrRetryable::from("already rollback"));
/// 锁被其他事务替换。
pub static ErrReplaced: LazyLock<ErrRetryable> =
    LazyLock::new(|| ErrRetryable::from("replaced by another transaction"));

// 操作无法完成时返回。
// ErrInvalidOp is returned when an operation cannot be completed.
#[derive(Clone, Debug, Eq, PartialEq)]
/// 非法/不支持的 Op。
pub struct ErrInvalidOp {
    /// 触发错误的操作类型。
    pub Op: kvrpcpb::Op,
}

/// 默认 Op 为 Put（与 Go 零值习惯对齐的占位）。
impl Default for ErrInvalidOp {
    fn default() -> Self {
        Self {
            Op: kvrpcpb::Op::Put,
        }
    }
}

/// ErrInvalidOp 消息。
impl ErrInvalidOp {
    pub fn Error(&self) -> String {
        format!("invalid op: {:?}", self.Op)
    }
}

impl_std_error!(ErrInvalidOp);

// 客户端尝试回滚已提交的锁时特殊返回。
// ErrAlreadyCommitted is returned specially when client tries to rollback a
// committed lock.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
/// 事务已提交错误；内部 u64 为提交时间戳。
pub struct ErrAlreadyCommitted(pub u64);

/// ErrAlreadyCommitted 固定文案。
impl ErrAlreadyCommitted {
    pub fn Error(&self) -> String {
        "txn already committed".to_owned()
    }
}

impl_std_error!(ErrAlreadyCommitted);

// 键已存在时返回（如 Insert 冲突）。
// ErrKeyAlreadyExists is returned when a key already exists.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 键已存在错误。
pub struct ErrKeyAlreadyExists {
    /// 冲突键。
    pub Key: Vec<u8>,
}

/// ErrKeyAlreadyExists 固定文案。
impl ErrKeyAlreadyExists {
    pub fn Error(&self) -> String {
        "key already exists".to_owned()
    }
}

impl_std_error!(ErrKeyAlreadyExists);

// 检测到死锁时返回。
// ErrDeadlock is returned when deadlock is detected.
#[derive(Clone, Debug, Default, PartialEq)]
/// 死锁错误，携带锁键、锁 TS、key hash 与等待链。
pub struct ErrDeadlock {
    /// 发生死锁时相关的锁键。
    pub LockKey: Vec<u8>,
    /// 持锁事务的 start_ts。
    pub LockTS: u64,
    /// 闭环关键边的 key hash。
    pub DeadlockKeyHash: u64,
    /// 完整等待链。
    pub WaitChain: Vec<deadlockpb::WaitForEntry>,
}

/// ErrDeadlock 固定文案 `"deadlock"`。
impl ErrDeadlock {
    pub fn Error(&self) -> String {
        "deadlock".to_owned()
    }
}

impl_std_error!(ErrDeadlock);

// 提交遇到写冲突时返回。
// ErrConflict is the error when the commit meets an write conflict error.
#[derive(Clone, Debug, Eq, PartialEq)]
/// 写冲突错误（MVCC 版本冲突）。
pub struct ErrConflict {
    /// 当前事务 start_ts。
    pub StartTS: u64,
    /// 冲突版本的 start_ts。
    pub ConflictTS: u64,
    /// 冲突版本的 commit_ts。
    pub ConflictCommitTS: u64,
    /// 相关键。
    pub Key: Vec<u8>,
    /// 写冲突原因枚举（如 RcCheckTs）。
    pub Reason: kvrpcpb::WriteConflictReason,
}

/// 默认 Reason 为 Unknown。
impl Default for ErrConflict {
    fn default() -> Self {
        Self {
            StartTS: 0,
            ConflictTS: 0,
            ConflictCommitTS: 0,
            Key: Vec::new(),
            Reason: kvrpcpb::WriteConflictReason::Unknown,
        }
    }
}

/// ErrConflict 固定文案 `"write conflict"`。
impl ErrConflict {
    pub fn Error(&self) -> String {
        "write conflict".to_owned()
    }
}

impl_std_error!(ErrConflict);

// 提交时间戳小于锁的 MinCommitTs 时返回（提交过期）。
// ErrCommitExpire is returned when commit key commitTs smaller than lock.MinCommitTs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 提交过期错误。
pub struct ErrCommitExpire {
    /// 事务 start_ts。
    pub StartTs: u64,
    /// 尝试使用的 commit_ts。
    pub CommitTs: u64,
    /// 锁要求的最小 commit_ts。
    pub MinCommitTs: u64,
    /// 断言作用的键。
    pub Key: Vec<u8>,
}

/// ErrCommitExpire 固定文案 `"commit expired"`。
impl ErrCommitExpire {
    pub fn Error(&self) -> String {
        "commit expired".to_owned()
    }
}

impl_std_error!(ErrCommitExpire);

// 存储上找不到所需事务信息时返回。
// ErrTxnNotFound is returned if the required txn info not found on storage.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 事务未找到错误。
pub struct ErrTxnNotFound {
    /// 查找的事务 start_ts。
    pub StartTS: u64,
    /// 主键（Primary Key）。
    pub PrimaryKey: Vec<u8>,
}

/// ErrTxnNotFound 固定文案 `"txn not found"`。
impl ErrTxnNotFound {
    pub fn Error(&self) -> String {
        "txn not found".to_owned()
    }
}

impl_std_error!(ErrTxnNotFound);

// 事务请求上的断言失败时返回。
// ErrAssertionFailed is returned if any assertion fails on a transaction request.
#[derive(Clone, Debug, Eq, PartialEq)]
/// 断言失败错误（Exist/NotExist 等）。
pub struct ErrAssertionFailed {
    /// 当前事务 start_ts。
    pub StartTS: u64,
    /// 当前键。
    pub Key: Vec<u8>,
    /// 失败的断言类型。
    pub Assertion: kvrpcpb::Assertion,
    /// 已存在版本的 start_ts。
    pub ExistingStartTS: u64,
    /// 已存在版本的 commit_ts。
    pub ExistingCommitTS: u64,
}

/// 默认 Assertion 为 None。
impl Default for ErrAssertionFailed {
    fn default() -> Self {
        Self {
            StartTS: 0,
            Key: Vec::new(),
            Assertion: kvrpcpb::Assertion::None,
            ExistingStartTS: 0,
            ExistingCommitTS: 0,
        }
    }
}

/// 详细格式化，含 hex Key 与断言枚举。
impl ErrAssertionFailed {
    pub fn Error(&self) -> String {
        format!(
            "AssertionFailed {{ StartTS: {}, Key: {}, Assertion: {:?}, ExistingStartTS: {}, ExistingCommitTS: {} }}",
            self.StartTS,
            hex::encode(&self.Key),
            self.Assertion,
            self.ExistingStartTS,
            self.ExistingCommitTS,
        )
    }
}

impl_std_error!(ErrAssertionFailed);

// CheckTxnStatus 打到二级锁（非主键锁）时返回。
// ErrPrimaryMismatch is returned if CheckTxnStatus request is sent to a secondary lock.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 主键不匹配错误。
pub struct ErrPrimaryMismatch {
    pub Key: Vec<u8>,
    pub Lock: Box<mvcc::Lock>,
}

/// 格式化为 primary mismatch + hex key + lock。
impl ErrPrimaryMismatch {
    pub fn Error(&self) -> String {
        format!(
            "primary mismatch, key: {}, lock: {}",
            hex::encode(&self.Key),
            self.Lock.String(),
        )
    }
}

impl_std_error!(ErrPrimaryMismatch);
