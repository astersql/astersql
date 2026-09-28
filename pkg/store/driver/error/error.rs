// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// TiKV / PD 客户端错误到 TiDB 错误体系的适配层。
//
// Go 的 client-go 提供较稳定的类型化错误；本模块保留全部转换分支，并同时接受
// 官方 `tikv-client` 错误，经 `ToTiDBErr` 映射为 `terror` / `kv` / `exeerrors` 等
// TiDB 侧 SharedError，供上层统一处理超时、锁、Region、资源组等故障。

use std::error::Error as StdError;
use std::sync::{LazyLock, Once};

use crate::{dbterror, errno, errors, exeerrors, kv, sqlkiller, terror};
use errors::{ErrorArg, SharedError};
use thiserror::Error;

// TiKV client-go exposes more stable, typed errors than tikv-client currently
// does. This adapter keeps every Go conversion branch available to Rust callers
// while ToTiDBErr also accepts the official tikv-client error type directly.
/// TiKV 侧类型化错误枚举（对齐 client-go），覆盖未找到键、写冲突闩锁、超时、锁等待、Region 不可用等。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum TiKvError {
    #[error("key not found")]
    NotFound,
    #[error("write conflict in latch, start timestamp: {start_ts}")]
    /// 本地闩锁（latch）上的写冲突；start_ts 为冲突事务开始时间戳。
    WriteConflictInLatch { start_ts: u64 },
    #[error("transaction is too large, size: {size}")]
    TxnTooLarge { size: u64 },
    #[error("cannot set nil value")]
    CannotSetNilValue,
    #[error("entry too large, limit: {limit}, size: {size}")]
    EntryTooLarge { limit: u64, size: u64 },
    #[error("key is too large, size: {key_size}")]
    KeyTooLarge { key_size: u64 },
    #[error("invalid transaction")]
    InvalidTxn,
    #[error("TiKV server timeout")]
    TiKVServerTimeout,
    #[error("PD server timeout: {message}")]
    PdServerTimeout { message: String },
    #[error("TiFlash server timeout")]
    TiFlashServerTimeout,
    #[error("query interrupted with signal {signal}")]
    /// 查询被 sqlkiller 信号打断（中断、超时、内存超限、runaway 等）。
    QueryInterruptedWithSignal { signal: u32 },
    #[error("TiKV server is busy")]
    TiKVServerBusy,
    #[error("TiFlash server is busy")]
    TiFlashServerBusy,
    #[error("transaction {txn_start_ts} was aborted by GC at {txn_safe_point}")]
    /// 事务因 GC（垃圾回收）安全点推进而被中止。
    TxnAbortedByGC {
        txn_start_ts: u64,
        txn_start_time: String,
        txn_safe_point: u64,
        txn_safe_point_time: String,
    },
    #[error(
        "transaction started at {txn_start_time} is earlier than GC safe point {gc_safe_point}"
    )]
    /// 事务启动时间早于 GC 安全点（GcTooEarly），映射时复用 ErrTxnAbortedByGC。
    GcTooEarly {
        txn_start_time: String,
        gc_safe_point: String,
    },
    #[error("stale TiKV command")]
    TiKVStaleCommand,
    #[error("TiKV max timestamp is not synced")]
    TiKVMaxTimestampNotSynced,
    #[error("lock acquisition failed while no-wait is set")]
    LockAcquireFailAndNoWaitSet,
    #[error("resolve lock timeout")]
    ResolveLockTimeout,
    #[error("lock wait timeout")]
    LockWaitTimeout,
    #[error("region unavailable")]
    /// Region：键空间分片；不可用通常表示副本不足或调度中。
    RegionUnavailable,
    #[error("store {store_id} reached its token limit")]
    TokenLimit { store_id: u64 },
    #[error("unknown TiKV error")]
    Unknown,
    #[error("result undetermined")]
    /// 结果不确定（如网络中断后无法判断提交是否成功），对应 ErrResultUndetermined。
    ResultUndetermined,
    #[error("{0}")]
    Other(String),
}

/// PD（Placement Driver）客户端错误，含资源组不存在、配置不可用、限流等。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PdError {
    #[error("resource group {resource_group_name} does not exist")]
    ClientGetResourceGroup { resource_group_name: String },
    #[error("resource group configuration is unavailable")]
    ClientResourceGroupConfigUnavailable,
    #[error("resource group is throttled")]
    ClientResourceGroupThrottled,
    #[error("{0}")]
    Other(String),
}

/// Store token 限流错误（errno::ErrTiKVStoreLimit）。
pub static ErrTokenLimit: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrTiKVStoreLimit));
/// TiKV 服务端超时。
pub static ErrTiKVServerTimeout: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrTiKVServerTimeout));
/// TiFlash 服务端超时。
pub static ErrTiFlashServerTimeout: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrTiFlashServerTimeout));
/// 事务被 GC 中止。
pub static ErrTxnAbortedByGC: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrTxnAbortedByGC));
/// 陈旧 TiKV 命令。
pub static ErrTiKVStaleCommand: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrTiKVStaleCommand));
/// 查询被中断。
pub static ErrQueryInterrupted: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrQueryInterrupted));
/// TiKV 最大时间戳未同步。
pub static ErrTiKVMaxTimestampNotSynced: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrTiKVMaxTimestampNotSynced));
/// 设置了 no-wait 时加锁失败。
pub static ErrLockAcquireFailAndNoWaitSet: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrLockAcquireFailAndNoWaitSet));
/// Resolve Lock 超时。
pub static ErrResolveLockTimeout: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrResolveLockTimeout));
/// 锁等待超时。
pub static ErrLockWaitTimeout: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrLockWaitTimeout));
/// TiKV 繁忙。
pub static ErrTiKVServerBusy: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrTiKVServerBusy));
/// TiFlash 繁忙。
pub static ErrTiFlashServerBusy: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrTiFlashServerBusy));
/// PD 服务端超时。
pub static ErrPDServerTimeout: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrPDServerTimeout));
/// Region 不可用。
pub static ErrRegionUnavailable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrRegionUnavailable));
/// 资源组不存在。
pub static ErrResourceGroupNotExists: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrResourceGroupNotExists));
/// 资源组配置不可用。
pub static ErrResourceGroupConfigUnavailable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrResourceGroupConfigUnavailable));
/// 资源组被限流。
pub static ErrResourceGroupThrottled: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrResourceGroupThrottled));
/// 未知 TiKV 错误。
pub static ErrUnknown: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrUnknown));

static REGISTER_TIKV_RETURNED_ERRORS: Once = Once::new();

// Go uses blank-identifier package variables for these registrations. Once
// preserves the package-initialization effect without allowing duplicate calls.
/// 注册 TiKV 可能回传的数值类错误（越界、截断、除零），进程内只执行一次。
pub fn register_tikv_returned_errors() {
    REGISTER_TIKV_RETURNED_ERRORS.call_once(|| {
        let _ = dbterror::ClassTiKV.NewStd(errno::ErrDataOutOfRange);
        let _ = dbterror::ClassTiKV.NewStd(errno::ErrTruncatedWrongValue);
        let _ = dbterror::ClassTiKV.NewStd(errno::ErrDivisionByZero);
    });
}

/// 将 terror::Error 克隆为 SharedError。
fn clone_normalized(error: &terror::Error) -> SharedError {
    SharedError::new(error.clone())
}

/// 在错误链中查找具体类型 E（对应 Go errors.As）。
fn find_typed<E>(error: &SharedError) -> Option<SharedError>
where
    E: StdError + 'static,
{
    errors::Find(Some(error), |candidate| {
        candidate.downcast_ref::<E>().is_some()
    })
}

/// 将 TiKvError 各变体映射为 TiDB SharedError；无法识别的信号或 Other 返回 None。
fn convert_tikv_error(error: &TiKvError) -> Option<SharedError> {
    let converted = match error {
        TiKvError::NotFound => clone_normalized(&kv::ErrNotExist),
        TiKvError::WriteConflictInLatch { start_ts } => {
            kv::ErrWriteConflictInTiDB.FastGenByArgs(&[ErrorArg::from(*start_ts)])
        }
        TiKvError::TxnTooLarge { size } => {
            kv::ErrTxnTooLarge.GenWithStackByArgs(&[ErrorArg::from(*size)])
        }
        TiKvError::CannotSetNilValue => clone_normalized(&kv::ErrCannotSetNilValue),
        TiKvError::EntryTooLarge { limit, size } => kv::ErrEntryTooLarge
            .GenWithStackByArgs(&[ErrorArg::from(*limit), ErrorArg::from(*size)]),
        TiKvError::KeyTooLarge { key_size } => {
            kv::ErrKeyTooLarge.GenWithStackByArgs(&[ErrorArg::from(*key_size)])
        }
        TiKvError::InvalidTxn => clone_normalized(&kv::ErrInvalidTxn),
        TiKvError::TiKVServerTimeout => clone_normalized(&ErrTiKVServerTimeout),
        TiKvError::PdServerTimeout { message } => {
            ErrPDServerTimeout.GenWithStackByArgs(&[ErrorArg::from(message.clone())])
        }
        TiKvError::TiFlashServerTimeout => clone_normalized(&ErrTiFlashServerTimeout),
        // 按 sqlkiller 信号分流到查询中断 / 超时 / 内存 / runaway 错误。
        TiKvError::QueryInterruptedWithSignal { signal }
            if *signal == sqlkiller::QueryInterrupted =>
        {
            clone_normalized(&ErrQueryInterrupted)
        }
        TiKvError::QueryInterruptedWithSignal { signal }
            if *signal == sqlkiller::MaxExecTimeExceeded =>
        {
            exeerrors::ErrMaxExecTimeExceeded.GenWithStackByArgs(&[])
        }
        TiKvError::QueryInterruptedWithSignal { signal }
            if *signal == sqlkiller::QueryMemoryExceeded =>
        {
            exeerrors::ErrMemoryExceedForQuery.GenWithStackByArgs(&[ErrorArg::from(-1_i64)])
        }
        TiKvError::QueryInterruptedWithSignal { signal }
            if *signal == sqlkiller::ServerMemoryExceeded =>
        {
            exeerrors::ErrMemoryExceedForInstance.GenWithStackByArgs(&[ErrorArg::from(-1_i64)])
        }
        TiKvError::QueryInterruptedWithSignal { signal }
            if *signal == sqlkiller::RunawayQueryExceeded =>
        {
            exeerrors::ErrResourceGroupQueryRunawayInterrupted
                .FastGenByArgs(&[ErrorArg::from("exceed tidb side")])
        }
        TiKvError::QueryInterruptedWithSignal { .. } => return None,
        TiKvError::TiKVServerBusy => clone_normalized(&ErrTiKVServerBusy),
        TiKvError::TiFlashServerBusy => clone_normalized(&ErrTiFlashServerBusy),
        TiKvError::TxnAbortedByGC {
            txn_start_ts,
            txn_start_time,
            txn_safe_point,
            txn_safe_point_time,
        } => ErrTxnAbortedByGC.GenWithStackByArgs(&[
            ErrorArg::from(*txn_start_ts),
            ErrorArg::from(txn_start_time.clone()),
            ErrorArg::from(*txn_safe_point),
            ErrorArg::from(txn_safe_point_time.clone()),
        ]),
        // GcTooEarly 复用 ErrTxnAbortedByGC，未知字段填 "<unknown>"。
        TiKvError::GcTooEarly {
            txn_start_time,
            gc_safe_point,
        } => ErrTxnAbortedByGC.GenWithStackByArgs(&[
            ErrorArg::from("<unknown>"),
            ErrorArg::from(txn_start_time.clone()),
            ErrorArg::from("<unknown>"),
            ErrorArg::from(gc_safe_point.clone()),
        ]),
        TiKvError::TiKVStaleCommand => clone_normalized(&ErrTiKVStaleCommand),
        TiKvError::TiKVMaxTimestampNotSynced => clone_normalized(&ErrTiKVMaxTimestampNotSynced),
        TiKvError::LockAcquireFailAndNoWaitSet => clone_normalized(&ErrLockAcquireFailAndNoWaitSet),
        TiKvError::ResolveLockTimeout => clone_normalized(&ErrResolveLockTimeout),
        TiKvError::LockWaitTimeout => clone_normalized(&ErrLockWaitTimeout),
        TiKvError::RegionUnavailable => clone_normalized(&ErrRegionUnavailable),
        TiKvError::TokenLimit { store_id } => {
            ErrTokenLimit.GenWithStackByArgs(&[ErrorArg::from(*store_id)])
        }
        TiKvError::Unknown => clone_normalized(&ErrUnknown),
        TiKvError::ResultUndetermined => clone_normalized(&terror::ErrResultUndetermined),
        TiKvError::Other(_) => return None,
    };
    Some(converted)
}

/// 将官方 tikv-client::Error 中已知变体映射为 TiDB 错误。
fn convert_official_tikv_error(error: &tikv_client::Error) -> Option<SharedError> {
    match error {
        tikv_client::Error::UndeterminedError(_) => {
            Some(clone_normalized(&terror::ErrResultUndetermined))
        }
        tikv_client::Error::InvalidTransactionType
        | tikv_client::Error::OperationAfterCommitError => {
            Some(clone_normalized(&kv::ErrInvalidTxn))
        }
        _ => None,
    }
}

/// 将 PdError 映射为资源组相关 TiDB 错误。
fn convert_pd_error(error: &PdError) -> Option<SharedError> {
    match error {
        PdError::ClientGetResourceGroup {
            resource_group_name,
        } => Some(
            ErrResourceGroupNotExists.FastGenByArgs(&[ErrorArg::from(resource_group_name.clone())]),
        ),
        PdError::ClientResourceGroupConfigUnavailable => {
            Some(clone_normalized(&ErrResourceGroupConfigUnavailable))
        }
        PdError::ClientResourceGroupThrottled => Some(clone_normalized(&ErrResourceGroupThrottled)),
        PdError::Other(_) => None,
    }
}

// ToTiDBErr checks and converts a TiKV or PD client error to a TiDB error.
// Option preserves Go's nil behavior; errors::Find preserves errors.As/Is
// matching through the migrated pingcap/errors wrapper chain.
/// 检查并转换 TiKV / PD / 官方 client 错误为 TiDB SharedError；None 保持 Go 的 nil 语义。
pub fn ToTiDBErr(error: Option<SharedError>) -> Option<SharedError> {
    register_tikv_returned_errors();
    let error = error?;

    // 优先匹配 client-go 风格的 TiKvError。
    if let Some(source) = find_typed::<TiKvError>(&error)
        && let Some(converted) = source
            .downcast_ref::<TiKvError>()
            .and_then(convert_tikv_error)
    {
        return Some(converted);
    }

    // 其次匹配官方 tikv-client 错误。
    if let Some(source) = find_typed::<tikv_client::Error>(&error)
        && let Some(converted) = source
            .downcast_ref::<tikv_client::Error>()
            .and_then(convert_official_tikv_error)
    {
        return Some(converted);
    }

    // 最后匹配 PD 资源组错误。
    if let Some(source) = find_typed::<PdError>(&error)
        && let Some(converted) = source.downcast_ref::<PdError>().and_then(convert_pd_error)
    {
        return Some(converted);
    }

    // 无法转换时 Trace 包装原错误返回。
    errors::Trace(Some(error))
}
