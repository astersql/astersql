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

// Backoffer（退避重试器）的 Aster 迁移单元测试。
//
// 验证构造函数保留 Context / 会话变量、退避次数与睡眠累计、
// 达到 maxSleep 后的错误归一化、txnLockFast 单次睡眠上限，
// 以及 Killed / 取消 Context 时停止重试并返回 TiDB 风格错误。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::{
    BackoffConfig, Context, ExecDetails, Jitter, NewBackoffer, NewBackofferWithTikvBo,
    NewBackofferWithVars, TiKvBackoffer, driver_error, errors, kv,
};

/// 构造可重试的 TiKV Other 错误，供 Backoff 调用。
fn retry_error(message: &str) -> errors::SharedError {
    errors::SharedError::new(driver_error::TiKvError::Other(message.to_owned()))
}

/// 校验 WithVars / WithTikvBo / NewBackoffer 三种构造路径的上下文与上限。
#[test]
fn constructors_preserve_context_vars_and_inner_backoffer() {
    let killed = AtomicU32::new(0);
    let vars = kv::Variables {
        BackoffLockFast: 13,
        BackOffWeight: 3,
        Killed: &killed,
    };
    let ctx = Context::new();

    let with_vars = NewBackofferWithVars(ctx.clone(), 20, Some(&vars));
    assert_eq!(with_vars.GetCtx(), ctx);
    assert!(std::ptr::eq(with_vars.GetVars(), &vars));
    // BackOffWeight=3 时 maxSleep 被放大为 20*3=60。
    assert_eq!(with_vars.TiKVBackoffer().MaxSleep(), 60);

    let inner = TiKvBackoffer::new(ctx.clone(), 17);
    let wrapped = NewBackofferWithTikvBo(inner);
    assert_eq!(wrapped.TiKVBackoffer().MaxSleep(), 17);
    assert_eq!(wrapped.GetCtx(), ctx);

    let defaulted = NewBackoffer(ctx.clone(), 9);
    assert_eq!(defaulted.GetVars().BackoffLockFast, kv::DefBackoffLockFast);
    assert_eq!(defaulted.GetVars().BackOffWeight, kv::DefBackOffWeight);
    assert_eq!(defaulted.TiKVBackoffer().MaxSleep(), 9);
}

/// 校验 regionMiss 退避会累计次数与睡眠毫秒，并与 Go 语义对齐。
#[test]
fn backoff_records_go_compatible_counts_sleep_and_total() {
    let ctx = Context::new();
    let mut backoffer = NewBackoffer(ctx, 100);
    let cfg = BackoffConfig::new("regionMiss", 0, 0, Jitter::NoJitter);

    assert!(backoffer.Backoff(&cfg, retry_error("first")).is_ok());
    assert!(backoffer.Backoff(&cfg, retry_error("second")).is_ok());

    assert_eq!(backoffer.GetBackoffTimes().get("regionMiss"), Some(&2));
    assert_eq!(backoffer.GetBackoffSleepMS().get("regionMiss"), Some(&0));
    assert_eq!(backoffer.GetTotalSleep(), 0);
}

/// 达到 maxSleep 后再次 Backoff 应返回 regionMiss 配置的 RegionUnavailable。
#[test]
fn max_sleep_error_is_converted_through_driver_error() {
    let killed = AtomicU32::new(0);
    let vars = kv::Variables {
        BackoffLockFast: kv::DefBackoffLockFast,
        BackOffWeight: 1,
        Killed: &killed,
    };
    let mut backoffer = NewBackofferWithVars(Context::new(), 1, Some(&vars));
    let cfg = BackoffConfig::new("regionMiss", 1, 1, Jitter::NoJitter);

    assert!(
        backoffer
            .Backoff(
                &cfg,
                errors::SharedError::new(driver_error::TiKvError::NotFound)
            )
            .is_ok()
    );
    let converted = backoffer
        .Backoff(
            &cfg,
            errors::SharedError::new(driver_error::TiKvError::NotFound),
        )
        .expect_err("the next call observes that maxSleep was reached");

    assert!(driver_error::ErrRegionUnavailable.Equal(Some(&converted)));
    assert_eq!(backoffer.GetTotalSleep(), 1);
    assert_eq!(backoffer.GetBackoffTimes().get("regionMiss"), Some(&1));
}

/// 校验事务快速锁退避路径会遵守单次 maxSleepMs 上限。
#[test]
fn txn_lock_fast_honors_per_sleep_cap() {
    let mut backoffer = NewBackoffer(Context::new(), 1_000);

    assert!(
        backoffer
            .BackoffWithMaxSleepTxnLockFast(5, retry_error("locked"))
            .is_ok()
    );
    assert_eq!(backoffer.GetTotalSleep(), 5);
    assert_eq!(backoffer.GetBackoffTimes().get("txnLockFast"), Some(&1));
    assert_eq!(backoffer.GetBackoffSleepMS().get("txnLockFast"), Some(&5));
}

/// Killed 标志与已取消 Context 应分别返回查询中断与原始触发错误。
#[test]
fn killed_and_cancelled_contexts_stop_retry_with_tidb_errors() {
    let killed = AtomicU32::new(0);
    let vars = kv::Variables {
        BackoffLockFast: kv::DefBackoffLockFast,
        BackOffWeight: 1,
        Killed: &killed,
    };
    let cfg = BackoffConfig::new("regionMiss", 0, 0, Jitter::NoJitter);
    let mut killed_backoffer = NewBackofferWithVars(Context::new(), 100, Some(&vars));
    killed.store(1, Ordering::Relaxed);

    let killed_error = killed_backoffer
        .Backoff(&cfg, retry_error("retry"))
        .expect_err("the kill flag stops retries after recording the sleep");
    assert!(driver_error::ErrQueryInterrupted.Equal(Some(&killed_error)));
    assert!(killed_error.to_string().contains('1'));
    assert_eq!(
        killed_backoffer.GetBackoffTimes().get("regionMiss"),
        Some(&1)
    );

    let ctx = Context::new();
    ctx.cancel();
    let mut cancelled = NewBackoffer(ctx, 100);
    let cancelled_error = cancelled
        .Backoff(&cfg, retry_error("cancelled retry"))
        .expect_err("a cancelled context returns the triggering error immediately");
    assert!(cancelled_error.to_string().contains("cancelled retry"));
    assert!(cancelled.GetBackoffTimes().is_empty());
}

/// Go 的 int 在当前 64 位平台可容纳超过 i32::MAX 的 maxSleep。
#[test]
fn public_sleep_values_are_platform_width() {
    let max_sleep = i32::MAX as isize + 1;
    let backoffer = NewBackoffer(Context::new(), max_sleep);

    assert_eq!(backoffer.TiKVBackoffer().MaxSleep(), max_sleep);
    assert_eq!(backoffer.GetTotalSleep(), 0_isize);
    let _: std::collections::HashMap<String, isize> = backoffer.GetBackoffTimes();
    let _: std::collections::HashMap<String, isize> = backoffer.GetBackoffSleepMS();
}

/// client-go 仅保留最近三条触发错误，顺序为从旧到新。
#[test]
fn error_history_is_a_three_entry_ring() {
    let mut backoffer = NewBackoffer(Context::new(), 100);
    let cfg = BackoffConfig::new("regionMiss", 0, 0, Jitter::NoJitter);

    for message in ["first", "second", "third", "fourth"] {
        assert!(backoffer.Backoff(&cfg, retry_error(message)).is_ok());
    }

    let reasons: Vec<_> = backoffer
        .b
        .latest_errors()
        .into_iter()
        .map(|error| error.reason.as_str())
        .collect();
    assert_eq!(reasons, ["second", "third", "fourth"]);
}

/// 超限时应返回累计睡眠最长的非排除配置错误，而不是当前调用错误。
#[test]
fn max_sleep_returns_longest_non_excluded_config_error() {
    let slow = BackoffConfig::new("regionMiss", 2, 2, Jitter::NoJitter).with_error(
        errors::SharedError::new(driver_error::TiKvError::RegionUnavailable),
    );
    let fast = BackoffConfig::new("txnNotFound", 1, 1, Jitter::NoJitter).with_error(
        errors::SharedError::new(driver_error::TiKvError::ResolveLockTimeout),
    );
    let excluded = BackoffConfig::new("tikvServerBusy", 3, 3, Jitter::NoJitter);
    let mut backoffer = NewBackoffer(Context::new(), 3);

    assert!(backoffer.Backoff(&slow, retry_error("slow")).is_ok());
    assert!(
        backoffer
            .Backoff(&excluded, retry_error("excluded"))
            .is_ok()
    );
    assert!(backoffer.Backoff(&fast, retry_error("fast")).is_ok());
    let error = backoffer
        .Backoff(
            &fast,
            errors::SharedError::new(driver_error::TiKvError::NotFound),
        )
        .expect_err("the accumulated sleep has reached maxSleep");

    assert!(driver_error::ErrRegionUnavailable.Equal(Some(&error)));
    assert!(!driver_error::kv::ErrNotExist.Equal(Some(&error)));
    assert_eq!(backoffer.GetTotalSleep(), 6);
}

/// tikvServerBusy 的睡眠计入总统计，但不消耗普通 maxSleep 配额。
#[test]
fn server_busy_sleep_is_excluded_from_regular_limit() {
    let cfg = BackoffConfig::new("tikvServerBusy", 1, 1, Jitter::NoJitter);
    let mut backoffer = NewBackoffer(Context::new(), 1);

    assert!(backoffer.Backoff(&cfg, retry_error("busy once")).is_ok());
    assert!(backoffer.Backoff(&cfg, retry_error("busy twice")).is_ok());
    assert_eq!(backoffer.GetTotalSleep(), 2);
    assert_eq!(backoffer.b.excluded_sleep_ms, 2);
}

/// Context 中的 ExecDetails 与 client-go 一样累计退避次数和纳秒时长。
#[test]
fn context_exec_details_are_updated_after_backoff() {
    let details = Arc::new(ExecDetails::default());
    let ctx = Context::new().with_exec_details(details.clone());
    let cfg = BackoffConfig::new("regionMiss", 2, 2, Jitter::NoJitter);
    let mut backoffer = NewBackoffer(ctx, 100);

    assert!(backoffer.Backoff(&cfg, retry_error("region miss")).is_ok());
    assert_eq!(details.backoff_count(), 1);
    assert_eq!(details.backoff_duration_ns(), 2_000_000);
}
