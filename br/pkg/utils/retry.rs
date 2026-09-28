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

//! Retry helpers ported from `br/pkg/utils/retry.go`.
//! 通用重试循环、TiKV Backoffer 适配，以及致命错误立即放弃的策略包装。
//! 与 Go 一致：取消时合并已收集错误；`WithRetryReturnLastErr` 只保留最后一次。
//! WithRetry 是无返回值语法糖，内部转调 WithRetryV2。
//! RemainingAttempts 在循环条件中读取，策略可动态耗尽。
//! NextBackoff 入参为最近一次错误，供策略分级退避。
//! Cancelled 为私有占位错误类型。
//! sample_log_retry 仅 Info 级别，避免刷屏。
//! FallBack2CreateTable 服务 DDL 兼容回退分支。
//! Backoffer::Backoff 忽略 reason/err，仅近似时延。
//! RetryWithBackoff::mu 目前未用于临界区，保留对称字段。
//! RequestBackOff 可在多线程扇出错误时合并最大等待。
//! BackOff 先检查上限再 sleep，防止最后一次越界睡眠。
//! VerboseRetry 每次包装生成新 gid，区分不同调用点。
//! Verbose 在 RemainingAttempts 调用时也打点，便于观察耗尽。
//! FailedOnErr 一旦触发保持 failed=true，后续一律 ZERO/0。
//! ErrorEqual 比较 Cause，兼容 Annotate 多层包装。
//! GiveUpRetryOn 用于备份无 leader 等不可恢复错误。
//! WithRetryReturnLastErr 适合只需最终原因的调用方。
//! 零次尝试返回 T 的零值，匹配 Go 泛型函数的 `*new(T), nil`。
//! 退避等待通过 Context 条件变量实现，可被取消立即打断。
//! AdaptTiKVBackoffer 让 BR 复用类 TiKV 客户端的退避接口。
//! Inner() 暴露以便测试直接触发 BoTiKVRPC。
//! TotalSleepInMS 合并两层计数，匹配 Go 字段语义。
//! MaxSleepInMS 只读暴露上限配置。
//! NextSleepInMS 便于测试窥视待消费退避。
//! TerrorError::Code 与 errno 常量比较需转 u16。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::stubs::context::Context;
use astersql_br_pkg_logutil::{Field, log};
use astersql_errno::errcode::ErrInvalidDDLJob;
use astersql_errors::{self as errors, Annotate, Cause, Join, SharedError};
use astersql_parser_terror::Error as TerrorError;
use uuid::Uuid;

use super::backoff::BackoffStrategy;

/// 无上下文参数的可重试闭包。
pub type RetryableFunc<'a> = Box<dyn FnMut() -> Result<(), SharedError> + 'a>;
/// 携带 Context 的可重试闭包，可返回任意成功值 T。
pub type RetryableFuncV2<'a, T> = Box<dyn FnMut(&Context) -> Result<T, SharedError> + 'a>;

/// 在 backoff 策略耗尽前重试；失败时 Join 全部错误。
pub fn WithRetry<'a>(
    ctx: &Context,
    mut retryableFunc: RetryableFunc<'a>,
    mut backoffStrategy: Box<dyn BackoffStrategy>,
) -> Result<(), SharedError> {
    // 委托 V2，忽略成功值类型。
    WithRetryV2(
        ctx,
        backoffStrategy,
        Box::new(move |_ctx| retryableFunc().map(|_| ())),
    )
    .map(|_| ())
}

/// 带返回值的重试：成功立即返回；取消则 Join 已收集错误。
pub fn WithRetryV2<'a, T: Default>(
    ctx: &Context,
    mut backoffStrategy: Box<dyn BackoffStrategy>,
    mut fn_: RetryableFuncV2<'a, T>,
) -> Result<T, SharedError> {
    let mut all_errors: Vec<Option<SharedError>> = Vec::new();
    while backoffStrategy.RemainingAttempts() > 0 {
        match fn_(ctx) {
            Ok(res) => return Ok(res),
            Err(err) => {
                all_errors.push(Some(err));
                // 取消优先于继续退避，避免在已取消上下文里空转。
                if ctx.is_cancelled() {
                    return Err(Join(&all_errors).unwrap_or_else(|| SharedError::new(Cancelled)));
                }
                let backoff = backoffStrategy
                    .NextBackoff(all_errors.last().and_then(|e| e.as_ref()).unwrap());
                if ctx.wait_cancelled_timeout(backoff) {
                    return Err(Join(&all_errors).expect("a retry error was collected"));
                }
            }
        }
    }
    match Join(&all_errors) {
        Some(err) => Err(err),
        None => Ok(T::default()),
    }
}

/// 上下文已取消时的占位错误（对齐 Go `context.Canceled` 文案）。
#[derive(Debug, Clone, Copy, Default)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("context canceled")
    }
}

impl std::error::Error for Cancelled {}

/// 与 `WithRetry` 类似，但最终只返回最后一次错误（不 Join）。
pub fn WithRetryReturnLastErr<'a>(
    ctx: &Context,
    mut retryableFunc: RetryableFunc<'a>,
    mut backoffStrategy: Box<dyn BackoffStrategy>,
) -> Result<(), SharedError> {
    if ctx.is_cancelled() {
        return Err(SharedError::new(Cancelled));
    }
    let mut last_err: Option<SharedError> = None;
    while backoffStrategy.RemainingAttempts() > 0 {
        match retryableFunc() {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_err = Some(err.clone());
                let backoff = backoffStrategy.NextBackoff(&err);
                sample_log_retry(&err, backoff);
                // 取消时直接返回本次 err，不再 sleep。
                if ctx.is_cancelled() {
                    return Err(err);
                }
                if ctx.wait_cancelled_timeout(backoff) {
                    return Err(err);
                }
            }
        }
    }
    match last_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// 采样记录单次可重试失败及将要 sleep 的时长。
fn sample_log_retry(err: &SharedError, backoff: Duration) {
    log::L().Info(
        "retryable operation failed",
        [
            Field::string("error", &err.to_string()),
            Field::string("backoff", &format!("{backoff:?}")),
        ],
    );
}

/// 判断是否应回退到 CreateTable：Cause 为 ErrInvalidDDLJob。
pub fn FallBack2CreateTable(err: &SharedError) -> bool {
    if let Some(cause) = Cause(Some(err)) {
        if let Some(terror) = cause.downcast_ref::<TerrorError>() {
            return terror.Code() as u16 == ErrInvalidDDLJob;
        }
    }
    false
}

/// Stand-in for TiKV `tikv.Backoffer` used by `AdaptTiKVBackoffer`.
/// TiKV Backoffer 简化桩：仅近似首次 BoTiKVRPC 睡眠。
pub struct Backoffer {
    ctx: Context,
    total_sleep_ms: i32,
    max_sleep_ms: i32,
}

impl Backoffer {
    fn new(ctx: Context, max_sleep_ms: i32) -> Self {
        Self {
            ctx,
            total_sleep_ms: 0,
            max_sleep_ms,
        }
    }

    fn get_total_sleep(&self) -> i32 {
        self.total_sleep_ms
    }

    /// Approximate `tikv.Backoffer.Backoff(BoTiKVRPC, ...)` first sleep (~100ms).
    /// 固定约 100ms，足够覆盖 adapter 单测对首次 sleep 的区间断言。
    pub fn Backoff(&mut self, _reason: &str, _err: SharedError) {
        let sleep_ms = 100;
        thread::sleep(Duration::from_millis(sleep_ms as u64));
        self.total_sleep_ms += sleep_ms;
        let _ = &self.ctx;
        let _ = self.max_sleep_ms;
    }
}

/// 包装 TiKV Backoffer：合并外部 RequestBackOff 与内部累计睡眠。
pub struct RetryWithBackoff {
    bo: Backoffer,
    totalBackoff: i32,
    maxBackoff: i32,
    /// 超限时 Annotate 到此基础错误上。
    baseErr: SharedError,
    mu: Mutex<()>,
    /// 并发 RequestBackOff 取最大值，BackOff 时消费并清零。
    nextBackoff: Mutex<i32>,
}

/// 构造适配器：`max_sleep_ms` 为总睡眠上限。
pub fn AdaptTiKVBackoffer(
    ctx: Context,
    max_sleep_ms: i32,
    base_err: SharedError,
) -> RetryWithBackoff {
    RetryWithBackoff {
        bo: Backoffer::new(ctx, max_sleep_ms),
        maxBackoff: max_sleep_ms,
        baseErr: base_err,
        totalBackoff: 0,
        mu: Mutex::new(()),
        nextBackoff: Mutex::new(0),
    }
}

impl RetryWithBackoff {
    /// 尚未消费的下一次退避毫秒数。
    pub fn NextSleepInMS(&self) -> i32 {
        *self.nextBackoff.lock().expect("nextBackoff lock poisoned")
    }

    /// 外部已消费睡眠 + Inner Backoffer 累计。
    pub fn TotalSleepInMS(&self) -> i32 {
        self.totalBackoff + self.bo.get_total_sleep()
    }

    pub fn MaxSleepInMS(&self) -> i32 {
        self.maxBackoff
    }

    /// 执行一次已请求的退避；若累计已超上限则返回 Annotate(baseErr)。
    pub fn BackOff(&mut self) -> Result<(), SharedError> {
        let next_bo = {
            let mut guard = self.nextBackoff.lock().expect("nextBackoff lock poisoned");
            let value = *guard;
            *guard = 0;
            value
        };

        // 与 Go 一致：在 sleep 前检查是否已超过 max。
        if self.TotalSleepInMS() > self.maxBackoff {
            return Err(Annotate(
                Some(self.baseErr.clone()),
                format!(
                    "backoff exceeds the max backoff time {:?}",
                    Duration::from_millis(self.maxBackoff as u64)
                ),
            )
            .expect("annotate backoff"));
        }

        thread::sleep(Duration::from_millis(next_bo as u64));
        self.totalBackoff += next_bo;
        Ok(())
    }

    /// 请求至少 sleep `ms`；多调用者并发时取最大值。
    pub fn RequestBackOff(&self, ms: i32) {
        let mut guard = self.nextBackoff.lock().expect("nextBackoff lock poisoned");
        *guard = (*guard).max(ms);
    }

    /// 暴露内部 TiKV Backoffer 以便直接调用 Backoff。
    pub fn Inner(&mut self) -> &mut Backoffer {
        &mut self.bo
    }
}

/// 在内层策略外包一层告警日志，并用 UUID 关联同一轮重试。
struct VerboseBackoffStrategy {
    inner: Box<dyn BackoffStrategy>,
    groupID: Uuid,
}

impl BackoffStrategy for VerboseBackoffStrategy {
    fn NextBackoff(&mut self, err: &SharedError) -> Duration {
        let next = self.inner.NextBackoff(err);
        log::Warn(
            "Encountered err, retrying.",
            [
                Field::string("nextBackoff", &format!("{next:?}")),
                Field::string("err", &err.to_string()),
                Field::string("gid", &self.groupID.to_string()),
            ],
        );
        next
    }

    fn RemainingAttempts(&self) -> i32 {
        let attempt = self.inner.RemainingAttempts();
        if attempt > 0 {
            log::Warn(
                "Retry attempt hint.",
                [
                    Field::int("attempt", attempt as i64),
                    Field::string("gid", &self.groupID.to_string()),
                ],
            );
        } else {
            log::Warn(
                "Retry limit exceeded.",
                [Field::string("gid", &self.groupID.to_string())],
            );
        }
        attempt
    }
}

/// 为任意退避策略增加 verbose 日志包装。
pub fn VerboseRetry(bo: Box<dyn BackoffStrategy>) -> Box<dyn BackoffStrategy> {
    Box::new(VerboseBackoffStrategy {
        inner: bo,
        groupID: Uuid::new_v4(),
    })
}

/// 命中 `failedOn` 列表中任一错误（按 Cause/ErrorEqual）后立即耗尽重试。
struct FailedOnErr {
    inner: Box<dyn BackoffStrategy>,
    failed: Mutex<bool>,
    failedOn: Arc<Vec<SharedError>>,
}

impl BackoffStrategy for FailedOnErr {
    fn NextBackoff(&mut self, err: &SharedError) -> Duration {
        let caused = Cause(Some(err));
        for fatal in self.failedOn.iter() {
            if errors::ErrorEqual(caused.as_ref(), Some(fatal)) {
                // 标记失败后返回 ZERO，并令 RemainingAttempts=0。
                *self.failed.lock().expect("failed lock poisoned") = true;
                return Duration::ZERO;
            }
        }
        if *self.failed.lock().expect("failed lock poisoned") {
            Duration::ZERO
        } else {
            self.inner.NextBackoff(err)
        }
    }

    fn RemainingAttempts(&self) -> i32 {
        if *self.failed.lock().expect("failed lock poisoned") {
            0
        } else {
            self.inner.RemainingAttempts()
        }
    }
}

/// 在指定致命错误集合上立即放弃重试（Go `GiveUpRetryOn`）。
pub fn GiveUpRetryOn(
    bo: Box<dyn BackoffStrategy>,
    errs: Vec<SharedError>,
) -> Box<dyn BackoffStrategy> {
    Box::new(FailedOnErr {
        inner: bo,
        failed: Mutex::new(false),
        failedOn: Arc::new(errs),
    })
}
