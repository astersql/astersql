// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/utils/retry_test.go` (`package utils_test`).
//! 覆盖 TiKV Backoffer 适配与“致命错误立即放弃重试”包装器。

use std::thread;
use std::time::{Duration, Instant};

use astersql_br_pkg_errors::ErrBackupNoLeader;
use astersql_errors::{Annotate, New, SharedError};

use crate::backoff::{BackoffStrategy, InitialRetryState};
use crate::retry::{
    AdaptTiKVBackoffer, GiveUpRetryOn, RetryableFunc, WithRetry, WithRetryReturnLastErr,
    WithRetryV2,
};
use crate::stubs::context::Context;

#[test]
fn test_retry_adapter() {
    // 校验 AdaptTiKVBackoffer：首次 BoTiKVRPC 睡眠、并发 RequestBackOff 取 max、超限返回 baseErr。
    let begin = Instant::now();
    let mut bo = AdaptTiKVBackoffer(Context::new(), 200, New("everything is alright"));
    // Approximate TiKV BoTiKVRPC first sleep (~100ms).
    // Go 侧首次 Backoff 约 100ms；允许 50–150ms 抖动。
    bo.Inner()
        .Backoff("BoTiKVRPC", New("TiKV is in a deep dream"));
    let sleeped = bo.TotalSleepInMS();
    assert!(sleeped >= 50, "sleeped={sleeped}");
    assert!(sleeped <= 150, "sleeped={sleeped}");

    // 多线程 RequestBackOff 应收敛为最大值 48。
    let requested_backoff = [10, 20, 5, 0, 42, 48];
    thread::scope(|scope| {
        for bms in requested_backoff {
            let bo_ref = &bo;
            scope.spawn(move || {
                bo_ref.RequestBackOff(bms);
            });
        }
    });
    assert_eq!(bo.NextSleepInMS(), 48);
    assert!(bo.BackOff().is_ok());
    assert_eq!(bo.TotalSleepInMS(), sleeped + 48);

    // 再消费一次合法退避后，累计睡眠将越过 max=200，下一次 BackOff 失败。
    bo.RequestBackOff(150);
    assert!(bo.BackOff().is_ok());

    bo.RequestBackOff(150);
    let err = bo.BackOff().unwrap_err();
    assert!(
        err.to_string().contains("everything is alright"),
        "total = {} / {}, err={err}",
        bo.TotalSleepInMS(),
        bo.MaxSleepInMS()
    );

    // 墙钟时间应覆盖实际 sleep，避免纯 mock 假通过。
    // 若实现改为零睡眠假时钟，本断言应同步调整。
    assert!(begin.elapsed() > Duration::from_millis(200));
}

#[test]
fn test_fail_now_if() {
    // GiveUpRetryOn：普通错误走内层策略；命中致命错误（含 annotate 包装）则立即耗尽。
    let mock_bo = InitialRetryState(100, Duration::from_secs(1), Duration::from_secs(1));
    let err1 = New("error1");
    let err2 = New("error2");

    let mut bo = GiveUpRetryOn(Box::new(mock_bo), vec![err1.clone()]);

    // err2 不在放弃列表，仍按内层 1s 退避且尝试次数未清零。
    assert_eq!(bo.NextBackoff(&err2), Duration::from_secs(1));
    assert_ne!(bo.RemainingAttempts(), 0);

    // annotate 多层包装后 Cause 仍等于 err1，应立即放弃。
    let annotated_err = Annotate(Annotate(Some(err1), "meow?"), "nya?").expect("annotate");
    assert_eq!(bo.NextBackoff(&annotated_err), Duration::ZERO);
    assert_eq!(bo.RemainingAttempts(), 0);

    // 预定义 ErrBackupNoLeader 经 FastGen 后也应被 ErrorEqual 识别为致命。
    // 与 Go errors.ErrorEqual 语义对齐，不能只比字符串。
    let mock_bo = InitialRetryState(100, Duration::from_secs(1), Duration::from_secs(1));
    let mut bo = GiveUpRetryOn(
        Box::new(mock_bo),
        vec![SharedError::new((*ErrBackupNoLeader).clone())],
    );
    let annotated_err = ErrBackupNoLeader.FastGen("leader is taking an adventure", &[]);
    assert_eq!(bo.NextBackoff(&annotated_err), Duration::ZERO);
    assert_eq!(bo.RemainingAttempts(), 0);
}

#[test]
fn zero_attempts_matches_go_zero_value_success() {
    let ctx = Context::new();

    let result = WithRetryV2(
        &ctx,
        Box::new(InitialRetryState(0, Duration::ZERO, Duration::ZERO)),
        Box::new(|_| -> Result<i32, SharedError> { panic!("operation must not run") }),
    );
    assert_eq!(result.expect("Go returns the zero value and nil error"), 0);

    let retryable: RetryableFunc<'_> =
        Box::new(|| -> Result<(), SharedError> { panic!("operation must not run") });
    assert!(
        WithRetry(
            &ctx,
            retryable,
            Box::new(InitialRetryState(0, Duration::ZERO, Duration::ZERO)),
        )
        .is_ok()
    );

    let retryable: RetryableFunc<'_> =
        Box::new(|| -> Result<(), SharedError> { panic!("operation must not run") });
    assert!(
        WithRetryReturnLastErr(
            &ctx,
            retryable,
            Box::new(InitialRetryState(0, Duration::ZERO, Duration::ZERO)),
        )
        .is_ok()
    );
}

#[test]
fn cancellation_interrupts_backoff_wait() {
    let ctx = Context::new();
    let cancel = ctx.clone();
    let started = Instant::now();
    let handle = thread::spawn(move || {
        WithRetryV2(
            &ctx,
            Box::new(InitialRetryState(
                2,
                Duration::from_secs(5),
                Duration::from_secs(5),
            )),
            Box::new(|_| Err::<(), _>(New("retry me"))),
        )
    });

    thread::sleep(Duration::from_millis(20));
    cancel.cancel();
    assert!(handle.join().expect("retry thread").is_err());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "Go select must let cancellation interrupt the backoff timer"
    );
}
