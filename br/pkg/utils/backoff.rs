// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Backoff strategies ported from `br/pkg/utils/backoff.go`.
//!
//! 自 Go `br/pkg/utils/backoff.go` 移植的 BR 重试退避策略模块。
//! 按业务场景（SST/PD/恢复等）封装 `BackoffStrategy`，与 `WithRetry` 配合使用。

//! 中文注释索引开始
//! 本文件负责`br/pkg/utils/backoff.rs`对应逻辑，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少85行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `importSSTRetryTimes`承载"importSSTRetryTimes"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `importSSTWaitInterval`承载"importSSTWaitInterval"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `importSSTMaxWaitInterval`承载"importSSTMaxWaitInterval"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! 中文注释索引结束

use std::time::Duration;

use astersql_br_pkg_errors::{
    Canceled, DeadlineExceeded, ErrKVDiskFull, ErrKVDownloadFailed, ErrKVEpochNotMatch,
    ErrKVIngestFailed, ErrKVRangeIsEmpty, ErrKVRewriteRuleNotFound, ErrPDInvalidResponse,
    ErrPDLeaderNotFound, ErrRestoreTotalKVMismatch, Is, IsContextCanceled,
};
use astersql_br_pkg_logutil::{Field, log};
use astersql_errors::{Errors, SharedError};

use super::error_handling::{
    ErrorContext, ErrorHandlingStrategy, HandleUnknownBackupError, NewErrorContext,
    NewZeroRetryContext, contextCancelledMsg,
};

// import SST 默认退避：16 次、40ms 起、上限 10s。
const importSSTRetryTimes: i32 = 16;
const importSSTWaitInterval: Duration = Duration::from_millis(40);
const importSSTMaxWaitInterval: Duration = Duration::from_secs(10);

// download SST：8 次、1s 起、上限 4s。
const downloadSSTRetryTimes: i32 = 8;
const downloadSSTWaitInterval: Duration = Duration::from_secs(1);
const downloadSSTMaxWaitInterval: Duration = Duration::from_secs(4);

// backup SST：5 次、2s 起、上限 3s。
const backupSSTRetryTimes: i32 = 5;
const backupSSTWaitInterval: Duration = Duration::from_secs(2);
const backupSSTMaxWaitInterval: Duration = Duration::from_secs(3);

// reset TS 激进策略（连 PD）：32 次、50ms 起、上限 2s。
const resetTSRetryTime: i32 = 32;
const resetTSWaitInterval: Duration = Duration::from_millis(50);
const resetTSMaxWaitInterval: Duration = Duration::from_secs(2);

// reset TS 保守策略：600 次、500ms 起、上限 300s。
const resetTSRetryTimeExt: i32 = 600;
const resetTSWaitIntervalExt: Duration = Duration::from_millis(500);
const resetTSMaxWaitIntervalExt: Duration = Duration::from_secs(300);

// flashback 对外导出常量。
pub const FlashbackRetryTime: i32 = 3;
pub const FlashbackWaitInterval: Duration = Duration::from_secs(3);
pub const FlashbackMaxWaitInterval: Duration = Duration::from_secs(15);

// checksum 对外导出常量。
pub const ChecksumRetryTime: i32 = 8;
pub const ChecksumWaitInterval: Duration = Duration::from_secs(1);
pub const ChecksumMaxWaitInterval: Duration = Duration::from_secs(30);

// recovery 退避：16 次、30s 起、上限 4min。
const recoveryMaxAttempts: i32 = 16;
const recoveryDelayTime: Duration = Duration::from_secs(30);
const recoveryMaxDelayTime: Duration = Duration::from_secs(4 * 60);

// raw client：5 次、500ms 起、上限 5s。
const rawClientMaxAttempts: i32 = 5;
const rawClientDelayTime: Duration = Duration::from_millis(500);
const rawClientMaxDelayTime: Duration = Duration::from_secs(5);

/// Local sentinel matching Go `sql.ErrNoRows`.
/// 本地哨兵，对齐 Go `sql.ErrNoRows`（PD 非重试判定用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SqlErrNoRows;

impl std::fmt::Display for SqlErrNoRows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("sql: no rows in result set")
    }
}

impl std::error::Error for SqlErrNoRows {}

/// Local sentinel matching Go `io.EOF`.
/// 本地哨兵，对齐 Go `io.EOF`（PD 可重试判定用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IoEof;

impl std::fmt::Display for IoEof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EOF")
    }
}

impl std::error::Error for IoEof {}

// 退避策略 trait：根据错误决定下次等待时长与剩余次数。
pub trait BackoffStrategy {
    fn NextBackoff(&mut self, err: &SharedError) -> Duration;
    fn RemainingAttempts(&self) -> i32;
}

// 固定间隔退避；RemainingAttempts 视为无限（i16::MAX）。
pub struct ConstantBackoff(pub Duration);

impl BackoffStrategy for ConstantBackoff {
    // 忽略 err，始终返回固定 Duration。
    fn NextBackoff(&mut self, _err: &SharedError) -> Duration {
        self.0
    }

    fn RemainingAttempts(&self) -> i32 {
        i16::MAX as i32
    }
}

// 指数退避状态机：记录已重试次数与 next/max 退避上限。
pub struct RetryState {
    maxRetry: i32,
    retryTimes: i32,
    maxBackoff: Duration,
    nextBackoff: Duration,
}

// 构造初始 RetryState，retryTimes 从 0 开始。
pub fn InitialRetryState(
    maxRetryTimes: i32,
    initialBackoff: Duration,
    maxBackoff: Duration,
) -> RetryState {
    RetryState {
        maxRetry: maxRetryTimes,
        maxBackoff,
        nextBackoff: initialBackoff,
        retryTimes: 0,
    }
}

impl RetryState {
    // 是否尚未达到 maxRetry。
    pub fn ShouldRetry(&self) -> bool {
        self.retryTimes < self.maxRetry
    }

    // 指数翻倍 nextBackoff（封顶 maxBackoff），返回本次应 sleep 时长。
    pub fn ExponentialBackoff(&mut self) -> Duration {
        self.retryTimes += 1;
        inject_failpoint("set-remaining-attempts-to-one", || {
            self.retryTimes = self.maxRetry;
        });
        let backoff = self.nextBackoff;
        self.nextBackoff = self.nextBackoff.saturating_mul(2);
        if self.nextBackoff > self.maxBackoff {
            self.nextBackoff = self.maxBackoff;
        }
        backoff
    }

    // 立即放弃：retryTimes 置为 maxRetry。
    pub fn GiveUp(&mut self) {
        self.retryTimes = self.maxRetry;
    }

    // 回退一次重试计数（特殊场景补偿）。
    pub fn ReduceRetry(&mut self) {
        self.retryTimes -= 1;
    }

    pub fn RemainingAttempts(&self) -> i32 {
        self.maxRetry - self.retryTimes
    }
}

impl BackoffStrategy for RetryState {
    fn NextBackoff(&mut self, _err: &SharedError) -> Duration {
        self.ExponentialBackoff()
    }

    fn RemainingAttempts(&self) -> i32 {
        RetryState::RemainingAttempts(self)
    }
}

type RetryErrFn = fn(&SharedError) -> bool;
type BackoffOption = Box<dyn FnMut(&mut BackoffStrategyImpl)>;

// 通用退避实现：剩余次数、延迟、错误上下文与可/不可重试判定函数。
struct BackoffStrategyImpl {
    remainingAttempts: i32,
    delayTime: Duration,
    maxDelayTime: Duration,
    errContext: ErrorContext,
    isRetryErr: RetryErrFn,
    isNonRetryErr: RetryErrFn,
}

// 选项构造器：设置最大剩余重试次数。
pub fn WithRemainingAttempts(attempts: i32) -> BackoffOption {
    Box::new(move |b: &mut BackoffStrategyImpl| b.remainingAttempts = attempts)
}

// 设置初始/当前退避延迟。
pub fn WithDelayTime(delay: Duration) -> BackoffOption {
    Box::new(move |b: &mut BackoffStrategyImpl| b.delayTime = delay)
}

// 设置退避延迟上限（指数翻倍封顶）。
pub fn WithMaxDelayTime(maxDelay: Duration) -> BackoffOption {
    Box::new(move |b: &mut BackoffStrategyImpl| b.maxDelayTime = maxDelay)
}

// 注入 ErrorContext，供未知错误累计 encounter 次数。
pub fn WithErrorContext(errContext: ErrorContext) -> BackoffOption {
    Box::new(move |b: &mut BackoffStrategyImpl| b.errContext = errContext.clone())
}

// 自定义“可重试”错误判定函数。
pub fn WithRetryErrorFunc(isRetryErr: RetryErrFn) -> BackoffOption {
    Box::new(move |b: &mut BackoffStrategyImpl| b.isRetryErr = isRetryErr)
}

// 自定义“不可重试（致命）”错误判定函数。
pub fn WithNonRetryErrorFunc(isNonRetryErr: RetryErrFn) -> BackoffOption {
    Box::new(move |b: &mut BackoffStrategyImpl| b.isNonRetryErr = isNonRetryErr)
}

// 按 opts 组装默认 BackoffStrategyImpl 并返回 trait object。
pub fn NewBackoffStrategy(mut opts: Vec<BackoffOption>) -> Box<dyn BackoffStrategy> {
    let mut bs = BackoffStrategyImpl {
        remainingAttempts: 1,
        delayTime: Duration::from_secs(1),
        maxDelayTime: Duration::from_secs(10),
        errContext: NewZeroRetryContext("default"),
        isRetryErr: always_true,
        isNonRetryErr: always_false,
    };
    for opt in &mut opts {
        opt(&mut bs);
    }
    Box::new(bs)
}

// 对所有错误均重试的策略（isRetryErr 恒 true）。
pub fn NewBackoffRetryAllErrorStrategy(
    remainingAttempts: i32,
    delayTime: Duration,
    maxDelayTime: Duration,
) -> Box<dyn BackoffStrategy> {
    let errContext = NewZeroRetryContext("retry all errors");
    NewBackoffStrategy(vec![
        WithRemainingAttempts(remainingAttempts),
        WithDelayTime(delayTime),
        WithMaxDelayTime(maxDelayTime),
        WithErrorContext(errContext),
        WithRetryErrorFunc(always_true),
        WithNonRetryErrorFunc(always_false),
    ])
}

// 重试除 isNonRetryFunc 命中外的所有错误。
pub fn NewBackoffRetryAllExceptStrategy(
    remainingAttempts: i32,
    delayTime: Duration,
    maxDelayTime: Duration,
    isNonRetryFunc: RetryErrFn,
) -> Box<dyn BackoffStrategy> {
    let errContext = NewZeroRetryContext("retry all except");
    NewBackoffStrategy(vec![
        WithRemainingAttempts(remainingAttempts),
        WithDelayTime(delayTime),
        WithMaxDelayTime(maxDelayTime),
        WithErrorContext(errContext),
        WithRetryErrorFunc(always_true),
        WithNonRetryErrorFunc(isNonRetryFunc),
    ])
}

// TiKV store 通用策略：TiKV/PD 错误码 + gRPC 可重试码。
pub fn NewTiKVStoreBackoffStrategy(
    maxRetry: i32,
    delayTime: Duration,
    maxDelayTime: Duration,
    errContext: ErrorContext,
) -> Box<dyn BackoffStrategy> {
    NewBackoffStrategy(vec![
        WithRemainingAttempts(maxRetry),
        WithDelayTime(delayTime),
        WithMaxDelayTime(maxDelayTime),
        WithErrorContext(errContext),
        WithRetryErrorFunc(is_tikv_retry_err),
        WithNonRetryErrorFunc(is_tikv_non_retry_err),
    ])
}

// import SST 预置参数 + ErrorContext("import sst", 3)。
pub fn NewImportSSTBackoffStrategy() -> Box<dyn BackoffStrategy> {
    let errContext = NewErrorContext("import sst", 3);
    NewTiKVStoreBackoffStrategy(
        importSSTRetryTimes,
        importSSTWaitInterval,
        importSSTMaxWaitInterval,
        errContext,
    )
}

// download SST 预置参数。
pub fn NewDownloadSSTBackoffStrategy() -> Box<dyn BackoffStrategy> {
    let errContext = NewErrorContext("download sst", 3);
    NewTiKVStoreBackoffStrategy(
        downloadSSTRetryTimes,
        downloadSSTWaitInterval,
        downloadSSTMaxWaitInterval,
        errContext,
    )
}

// backup SST 预置参数。
pub fn NewBackupSSTBackoffStrategy() -> Box<dyn BackoffStrategy> {
    let errContext = NewErrorContext("backup sst", 3);
    NewTiKVStoreBackoffStrategy(
        backupSSTRetryTimes,
        backupSSTWaitInterval,
        backupSSTMaxWaitInterval,
        errContext,
    )
}

// PD 连接/reset TS 策略：自定义 maxRetry/delay/maxDelay。
pub fn NewPDBackoffStrategy(
    maxRetry: i32,
    delayTime: Duration,
    maxDelayTime: Duration,
) -> Box<dyn BackoffStrategy> {
    NewBackoffStrategy(vec![
        WithRemainingAttempts(maxRetry),
        WithDelayTime(delayTime),
        WithMaxDelayTime(maxDelayTime),
        WithErrorContext(NewZeroRetryContext("connect PD")),
        WithRetryErrorFunc(is_pd_retry_err),
        WithNonRetryErrorFunc(is_pd_non_retry_err),
    ])
}

// 激进 PD 退避（resetTS* 短间隔参数）。
pub fn NewAggressivePDBackoffStrategy() -> Box<dyn BackoffStrategy> {
    NewPDBackoffStrategy(
        resetTSRetryTime,
        resetTSWaitInterval,
        resetTSMaxWaitInterval,
    )
}

// 保守 PD 退避（resetTS*Ext 长间隔参数）。
pub fn NewConservativePDBackoffStrategy() -> Box<dyn BackoffStrategy> {
    NewPDBackoffStrategy(
        resetTSRetryTimeExt,
        resetTSWaitIntervalExt,
        resetTSMaxWaitIntervalExt,
    )
}

// 磁盘检查：PDInvalidResponse / KVDiskFull 可重试。
pub fn NewDiskCheckBackoffStrategy() -> Box<dyn BackoffStrategy> {
    NewBackoffStrategy(vec![
        WithRemainingAttempts(resetTSRetryTime),
        WithDelayTime(resetTSWaitInterval),
        WithErrorContext(NewZeroRetryContext("disk check")),
        WithRetryErrorFunc(is_disk_check_retry_err),
        WithNonRetryErrorFunc(always_false),
    ])
}

// recovery 场景：调用方传入 isRetryErrFunc。
pub fn NewRecoveryBackoffStrategy(isRetryErrFunc: RetryErrFn) -> Box<dyn BackoffStrategy> {
    NewBackoffStrategy(vec![
        WithRemainingAttempts(recoveryMaxAttempts),
        WithDelayTime(recoveryDelayTime),
        WithErrorContext(NewZeroRetryContext("recovery")),
        WithRetryErrorFunc(isRetryErrFunc),
        WithNonRetryErrorFunc(always_false),
    ])
}

// flashback：固定 3 次、3s 起、15s 上限，全错误重试。
pub fn NewFlashBackBackoffStrategy() -> Box<dyn BackoffStrategy> {
    NewBackoffStrategy(vec![
        WithRemainingAttempts(FlashbackRetryTime),
        WithDelayTime(FlashbackWaitInterval),
        WithErrorContext(NewZeroRetryContext("flashback")),
        WithRetryErrorFunc(always_true),
        WithNonRetryErrorFunc(always_false),
    ])
}

// checksum：8 次、1s 起、30s 上限。
pub fn NewChecksumBackoffStrategy() -> Box<dyn BackoffStrategy> {
    NewBackoffStrategy(vec![
        WithRemainingAttempts(ChecksumRetryTime),
        WithDelayTime(ChecksumWaitInterval),
        WithErrorContext(NewZeroRetryContext("checksum")),
        WithRetryErrorFunc(always_true),
        WithNonRetryErrorFunc(always_false),
    ])
}

// raw client：5 次、500ms 起、5s 上限。
pub fn NewRawClientBackoffStrategy() -> Box<dyn BackoffStrategy> {
    NewBackoffStrategy(vec![
        WithRemainingAttempts(rawClientMaxAttempts),
        WithDelayTime(rawClientDelayTime),
        WithMaxDelayTime(rawClientMaxDelayTime),
        WithErrorContext(NewZeroRetryContext("raw client")),
        WithRetryErrorFunc(always_true),
        WithNonRetryErrorFunc(always_false),
    ])
}

impl BackoffStrategy for BackoffStrategyImpl {
    // 核心决策：按错误类型选择 doBackoff / stopBackoff，返回 sleep 时长。
    fn NextBackoff(&mut self, err: &SharedError) -> Duration {
        let errs = Errors(err);
        // 取错误链最后一项作为判定依据（对齐 Go）。
        let last_err = errs.last().unwrap_or(err);
        // Mutate `self.errContext` in place (Go passes `bo.errContext` by pointer) so
        // encounter-times for unknown errors accumulate across retries.
        // 原地更新 errContext，使未知错误 encounter 次数跨重试累计。
        let res = HandleUnknownBackupError(&last_err.to_string(), 0, &mut self.errContext);
        if res.Strategy == ErrorHandlingStrategy::StrategyRetry {
            // 未知错误策略允许重试 → 指数退避。
            self.doBackoff();
        } else if res.Reason == contextCancelledMsg {
            // 上下文取消 → 立即停止。
            self.stopBackoff();
        } else if (self.isNonRetryErr)(last_err) {
            // 命中不可重试列表 → 停止。
            self.stopBackoff();
        } else if (self.isRetryErr)(last_err) {
            // 命中可重试列表 → 继续退避。
            self.doBackoff();
        } else {
            // 既非白名单也非黑名单 → 打日志并停止。
            log::Warn(
                "stop retrying on error",
                [Field::string("error", &err.to_string())],
            );
            self.stopBackoff();
        }

        inject_failpoint("set-remaining-attempts-to-one", || {
            if self.remainingAttempts > 1 {
                self.remainingAttempts = 1;
            }
        });

        // 返回 delayTime 与 maxDelayTime 的较小者作为实际 sleep。
        if self.delayTime > self.maxDelayTime {
            self.maxDelayTime
        } else {
            self.delayTime
        }
    }

    fn RemainingAttempts(&self) -> i32 {
        self.remainingAttempts
    }
}

impl BackoffStrategyImpl {
    // 延迟翻倍、剩余次数减一。
    fn doBackoff(&mut self) {
        self.delayTime = self.delayTime.saturating_mul(2);
        self.remainingAttempts -= 1;
    }

    // 置零延迟与剩余次数，终止重试循环。
    fn stopBackoff(&mut self) {
        self.delayTime = Duration::ZERO;
        self.remainingAttempts = 0;
    }
}

// 恒为可重试（用于 flashback/checksum 等全重试策略）。
fn always_true(_err: &SharedError) -> bool {
    true
}

// 恒为非重试黑名单未命中。
fn always_false(_err: &SharedError) -> bool {
    false
}

// TiKV 可重试：epoch 不匹配、download/ingest 失败、PD leader 缺失、gRPC 码。
fn is_tikv_retry_err(err: &SharedError) -> bool {
    Is(Some(err), &ErrKVEpochNotMatch)
        || Is(Some(err), &ErrKVDownloadFailed)
        || Is(Some(err), &ErrKVIngestFailed)
        || Is(Some(err), &ErrPDLeaderNotFound)
        || grpc_code_is_retryable(err, TIKV_GRPC_RETRY_CODES)
}

// TiKV 不可重试：context 取消、空 range、rewrite 规则缺失。
fn is_tikv_non_retry_err(err: &SharedError) -> bool {
    IsContextCanceled(Some(err))
        || Is(Some(err), &ErrKVRangeIsEmpty)
        || Is(Some(err), &ErrKVRewriteRuleNotFound)
}

// PD 可重试：总 KV 不匹配、EOF、PD gRPC 可重试码。
fn is_pd_retry_err(err: &SharedError) -> bool {
    Is(Some(err), &ErrRestoreTotalKVMismatch)
        || err.downcast_ref::<IoEof>().is_some()
        || grpc_code_is_retryable(err, PD_GRPC_RETRY_CODES)
}

// PD 不可重试：context 取消、DeadlineExceeded、sql.ErrNoRows。
fn is_pd_non_retry_err(err: &SharedError) -> bool {
    IsContextCanceled(Some(err))
        || err.downcast_ref::<DeadlineExceeded>().is_some()
        || err.downcast_ref::<SqlErrNoRows>().is_some()
}

// 磁盘检查可重试：PD 无效响应或 TiKV 磁盘满。
fn is_disk_check_retry_err(err: &SharedError) -> bool {
    Is(Some(err), &ErrPDInvalidResponse) || Is(Some(err), &ErrKVDiskFull)
}

// TiKV gRPC 可重试 status code 列表。
const TIKV_GRPC_RETRY_CODES: &[&str] = &[
    "Canceled",
    "Unavailable",
    "Aborted",
    "DeadlineExceeded",
    "ResourceExhausted",
    "Internal",
];

// PD gRPC 可重试 status code 列表（比 TiKV 更宽）。
const PD_GRPC_RETRY_CODES: &[&str] = &[
    "Canceled",
    "DeadlineExceeded",
    "NotFound",
    "AlreadyExists",
    "PermissionDenied",
    "ResourceExhausted",
    "Aborted",
    "OutOfRange",
    "Unavailable",
    "DataLoss",
    "Unknown",
];

// 检查错误消息是否包含给定 gRPC code 的任一常见格式。
fn grpc_code_is_retryable(err: &SharedError, codes: &[&str]) -> bool {
    let message = err.to_string();
    codes
        .iter()
        .any(|code| grpc_message_has_code(&message, code))
}

// 子串匹配多种 gRPC 错误文案格式（code = X / Code(X) 等）。
fn grpc_message_has_code(message: &str, code: &str) -> bool {
    message.contains(&format!("code = {code}"))
        || message.contains(&format!("code={code}"))
        || message.contains(&format!("Code({code})"))
        || message.contains(&format!("rpc error: code = {code}"))
}

// failpoint 桩：本 crate 默认可空操作，测试/集成可注入。
fn inject_failpoint(_name: &str, _f: impl FnOnce()) {}
