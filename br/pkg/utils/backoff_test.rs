// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/utils/backoff_test.go` (`package utils_test`).
//!
//! 覆盖 TiKV/PD/Import/Download/Backup SST 等退避策略与 WithRetry 组合行为。
//! 使用纳秒级 backoff 压缩等待；通过计数器与 Join 错误列表断言重试次数。
//! 致命错误（如 RangeIsEmpty、Canceled）应立即停止；可重试错误耗尽 attempts。
//! ConstantBackoff 分段：有限退避可被 cancel 打断；零退避可近似无限重试直至成功。
//! grpc_status 仅模拟文案，不解析真正 tonic Status。
//! NewDefaultContext 提供策略所需的错误分类上下文。
//! 各 SST 策略工厂返回独立 BackoffStrategy 实现。
//! Join 顺序与失败发生顺序一致，供 err_msgs 精确比对。
//! 本文件不测真实网络抖动，只验证控制流与错误聚合。
//! counter 在闭包内递增，反映实际调用次数而非 attempts 配置。
//! WithRetry 与 WithRetryReturnLastErr 共用同一失败序列以便对照。
//! AggressivePD 对 Unavailable/IoEof 更宽容，Canceled 仍终止。
//! Import 路径以 DownloadFailed 作为可重试代表错误。
//! Download/Backup 路径以 IngestFailed 作为可重试代表错误。
//! ConstantBackoff(Duration::ZERO) 几乎不睡眠，适合长次数成功路径。
//! 并行线程 + cancel 覆盖上下文协作式退出。
//! 断言文案使用错误 Display，避免依赖内部类型相等。
//! 纳秒 backoff 仍会走 sleep 接口，但不显著拖慢 CI。
//! 未知错误用例证明 allow-list 外错误不会直接标致命。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use astersql_br_pkg_errors::{
    Canceled, ErrKVDownloadFailed, ErrKVEpochNotMatch, ErrKVIngestFailed, ErrKVRangeIsEmpty,
};
use astersql_errors::{Errors, New, SharedError};

use crate::backoff::{
    ConstantBackoff, IoEof, NewAggressivePDBackoffStrategy, NewBackupSSTBackoffStrategy,
    NewChecksumBackoffStrategy, NewDownloadSSTBackoffStrategy, NewFlashBackBackoffStrategy,
    NewImportSSTBackoffStrategy, NewRecoveryBackoffStrategy, NewTiKVStoreBackoffStrategy,
};
use crate::error_handling::NewDefaultContext;
use crate::retry::{WithRetry, WithRetryReturnLastErr, WithRetryV2};
use crate::stubs::context::Context;

/// 构造类 gRPC status 文案的 SharedError，供策略识别。
fn grpc_status(code: &str, message: &str) -> SharedError {
    New(format!("rpc error: code = {code} desc = {message}"))
}

/// 展开 Join 错误链为字符串列表，便于与 Go 期望逐条对照。
fn err_msgs(err: &SharedError) -> Vec<String> {
    Errors(err).into_iter().map(|e| e.to_string()).collect()
}

fn retry_every_error(_err: &SharedError) -> bool {
    true
}

/// Go 工厂未覆盖 maxDelayTime；三个策略都必须沿用 NewBackoffStrategy 的 10s 默认上限。
#[test]
fn test_factory_default_max_delay_matches_go() {
    let err = New("retryable");

    let mut recovery = NewRecoveryBackoffStrategy(retry_every_error);
    assert_eq!(recovery.NextBackoff(&err), Duration::from_secs(10));

    let mut flashback = NewFlashBackBackoffStrategy();
    assert_eq!(flashback.NextBackoff(&err), Duration::from_secs(6));
    assert_eq!(flashback.NextBackoff(&err), Duration::from_secs(10));

    let mut checksum = NewChecksumBackoffStrategy();
    assert_eq!(checksum.NextBackoff(&err), Duration::from_secs(2));
    assert_eq!(checksum.NextBackoff(&err), Duration::from_secs(4));
    assert_eq!(checksum.NextBackoff(&err), Duration::from_secs(8));
    assert_eq!(checksum.NextBackoff(&err), Duration::from_secs(10));
}

/// 先 Unavailable 再 EpochNotMatch，第三次成功：验证可重试路径最终 Ok。
#[test]
fn test_backoff_with_success() {
    let mut counter = 0;
    // 纳秒级 min/max：单测不真实睡眠过久。
    let backoff_strategy = NewTiKVStoreBackoffStrategy(
        10,
        Duration::from_nanos(1),
        Duration::from_nanos(1),
        NewDefaultContext(),
    );
    let err = WithRetry(
        &Context::new(),
        Box::new(|| {
            let current = counter;
            counter += 1;
            match current {
                // 可重试：运输层关闭。
                0 => Err(grpc_status("Unavailable", "transport is closing")),
                // 可重试：epoch 不匹配。
                1 => Err(SharedError::new((*ErrKVEpochNotMatch).clone())),
                // 第三次及以后成功。
                _ => Ok(()),
            }
        }),
        backoff_strategy,
    );
    assert_eq!(counter, 3);
    assert!(err.is_ok());
}

/// 未知错误仍允许后续可重试错误并最终成功（策略不因未知立即放弃）。
#[test]
fn test_backoff_with_unknown_error_success() {
    let mut counter = 0;
    // 纳秒级 min/max：单测不真实睡眠过久。
    let backoff_strategy = NewTiKVStoreBackoffStrategy(
        10,
        Duration::from_nanos(1),
        Duration::from_nanos(1),
        NewDefaultContext(),
    );
    let err = WithRetry(
        &Context::new(),
        Box::new(|| {
            let current = counter;
            counter += 1;
            match current {
                // 不在白名单的未知错误，仍不阻断后续重试。
                0 => Err(New("unknown error: not in the allow list")),
                // 可重试：epoch 不匹配。
                1 => Err(SharedError::new((*ErrKVEpochNotMatch).clone())),
                // 第三次及以后成功。
                _ => Ok(()),
            }
        }),
        backoff_strategy,
    );
    assert_eq!(counter, 3);
    assert!(err.is_ok());
}

/// RangeIsEmpty 为致命：在第四次失败处停止，Join 含全部已收集错误。
#[test]
fn test_backoff_with_fatal_error() {
    let mut counter = 0;
    // 纳秒级 min/max：单测不真实睡眠过久。
    let backoff_strategy = NewTiKVStoreBackoffStrategy(
        10,
        Duration::from_nanos(1),
        Duration::from_nanos(1),
        NewDefaultContext(),
    );
    // 预构造错误以便与 Join 列表逐条比对。
    let grpc_error = grpc_status("Unavailable", "transport is closing");
    let err = WithRetry(
        &Context::new(),
        Box::new(|| {
            let current = counter;
            counter += 1;
            match current {
                0 => Err(grpc_error.clone()),
                // 可重试：epoch 不匹配。
                1 => Err(SharedError::new((*ErrKVEpochNotMatch).clone())),
                // 可重试下载失败。
                2 => Err(SharedError::new((*ErrKVDownloadFailed).clone())),
                // 致命：空 range，应停止。
                3 => Err(SharedError::new((*ErrKVRangeIsEmpty).clone())),
                // 第三次及以后成功。
                _ => Ok(()),
            }
        }),
        backoff_strategy,
    )
    .unwrap_err();
    assert_eq!(counter, 4);
    assert_eq!(
        err_msgs(&err),
        vec![
            grpc_error.to_string(),
            (*ErrKVEpochNotMatch).to_string(),
            (*ErrKVDownloadFailed).to_string(),
            (*ErrKVRangeIsEmpty).to_string(),
        ]
    );
}

/// WithRetryReturnLastErr：同样序列但只保留最后一次（RangeIsEmpty）错误。
#[test]
fn test_with_retry_return_last_err() {
    let mut counter = 0;
    // 纳秒级 min/max：单测不真实睡眠过久。
    let backoff_strategy = NewTiKVStoreBackoffStrategy(
        10,
        Duration::from_nanos(1),
        Duration::from_nanos(1),
        NewDefaultContext(),
    );
    // 预构造错误以便与 Join 列表逐条比对。
    let grpc_error = grpc_status("Unavailable", "transport is closing");
    let err = WithRetryReturnLastErr(
        &Context::new(),
        Box::new(|| {
            let current = counter;
            counter += 1;
            match current {
                0 => Err(grpc_error.clone()),
                // 可重试：epoch 不匹配。
                1 => Err(SharedError::new((*ErrKVEpochNotMatch).clone())),
                // 可重试下载失败。
                2 => Err(SharedError::new((*ErrKVDownloadFailed).clone())),
                // 致命：空 range，应停止。
                3 => Err(SharedError::new((*ErrKVRangeIsEmpty).clone())),
                // 第三次及以后成功。
                _ => Ok(()),
            }
        }),
        backoff_strategy,
    )
    .unwrap_err();
    assert_eq!(counter, 4);
    // 只断言最后一次错误文案。
    assert_eq!(err.to_string(), (*ErrKVRangeIsEmpty).to_string());
}

/// 原始 gRPC Canceled：第一次即致命，counter 仅为 1。
#[test]
fn test_backoff_with_fatal_raw_grpc_error() {
    let mut counter = 0;
    // 致命取消错误样本。
    let canceled_error = grpc_status("Canceled", "context canceled");
    // 纳秒级 min/max：单测不真实睡眠过久。
    let backoff_strategy = NewTiKVStoreBackoffStrategy(
        10,
        Duration::from_nanos(1),
        Duration::from_nanos(1),
        NewDefaultContext(),
    );
    let err = WithRetry(
        &Context::new(),
        Box::new(|| {
            counter += 1;
            // 每次都返回 Canceled，第一次即应退出。
            Err(canceled_error.clone())
        }),
        backoff_strategy,
    )
    .unwrap_err();
    assert_eq!(counter, 1);
    assert_eq!(err_msgs(&err), vec![canceled_error.to_string()]);
}

/// 持续 EpochNotMatch：耗尽 10 次 attempts，Join 十条相同错误。
#[test]
fn test_backoff_with_retryable_error() {
    let mut counter = 0;
    // 纳秒级 min/max：单测不真实睡眠过久。
    let backoff_strategy = NewTiKVStoreBackoffStrategy(
        10,
        Duration::from_nanos(1),
        Duration::from_nanos(1),
        NewDefaultContext(),
    );
    let err = WithRetry(
        &Context::new(),
        Box::new(|| {
            counter += 1;
            // 纯可重试错误刷满 attempts。
            Err(SharedError::new((*ErrKVEpochNotMatch).clone()))
        }),
        backoff_strategy,
    )
    .unwrap_err();
    assert_eq!(counter, 10);
    // 期望十条相同 EpochNotMatch 文案。
    let expected = vec![(*ErrKVEpochNotMatch).to_string(); 10];
    assert_eq!(err_msgs(&err), expected);
}

/// Aggressive PD 策略：夹杂 IoEof 与最终 Canceled，校验错误序列。
#[test]
fn test_pd_backoff_with_retryable_error() {
    let mut counter = 0;
    // PD 激进退避：更高容忍 Unavailable。
    let backoff_strategy = NewAggressivePDBackoffStrategy();
    // 预构造错误以便与 Join 列表逐条比对。
    let grpc_error = grpc_status("Unavailable", "transport is closing");
    let err = WithRetry(
        &Context::new(),
        Box::new(|| {
            let current = counter;
            counter += 1;
            // 插入 IoEof，验证 PD 策略仍继续。
            if current == 2 {
                return Err(SharedError::new(IoEof));
            }
            // 最终 Canceled 结束循环。
            if current == 6 {
                return Err(SharedError::new(Canceled));
            }
            Err(grpc_error.clone())
        }),
        backoff_strategy,
    )
    .unwrap_err();
    assert_eq!(counter, 7);
    assert_eq!(
        err_msgs(&err),
        vec![
            grpc_error.to_string(),
            grpc_error.to_string(),
            IoEof.to_string(),
            grpc_error.to_string(),
            grpc_error.to_string(),
            grpc_error.to_string(),
            Canceled.to_string(),
        ]
    );
}

/// ImportSST：前 5 次 DownloadFailed，第 6 次成功。
#[test]
fn test_new_import_sst_backoffer_with_success() {
    let mut counter = 0;
    // Import SST 专用退避策略。
    let backoff_strategy = NewImportSSTBackoffStrategy();
    let err = WithRetry(
        &Context::new(),
        Box::new(|| {
            let current = counter;
            counter += 1;
            // 第 6 次调用（current==5）成功。
            if current == 5 {
                Ok(())
            } else {
                Err(SharedError::new((*ErrKVDownloadFailed).clone()))
            }
        }),
        backoff_strategy,
    );
    assert_eq!(counter, 6);
    assert!(err.is_ok());
}

/// DownloadSST：三次 IngestFailed 后 Canceled，Join 四条。
#[test]
fn test_new_download_sst_backoffer_with_cancel() {
    let mut counter = 0;
    // Download SST 专用退避策略。
    let backoff_strategy = NewDownloadSSTBackoffStrategy();
    let err = WithRetry(
        &Context::new(),
        Box::new(|| {
            let current = counter;
            counter += 1;
            // 第 4 次调用返回取消。
            if current == 3 {
                Err(SharedError::new(Canceled))
            } else {
                Err(SharedError::new((*ErrKVIngestFailed).clone()))
            }
        }),
        backoff_strategy,
    )
    .unwrap_err();
    assert_eq!(counter, 4);
    assert_eq!(
        err_msgs(&err),
        vec![
            (*ErrKVIngestFailed).to_string(),
            (*ErrKVIngestFailed).to_string(),
            (*ErrKVIngestFailed).to_string(),
            Canceled.to_string(),
        ]
    );
}

/// BackupSST：与 Download 同形，确认策略独立但行为对齐。
#[test]
fn test_new_backup_sst_backoffer_with_cancel() {
    let mut counter = 0;
    // Backup SST 专用退避策略。
    let backoff_strategy = NewBackupSSTBackoffStrategy();
    let err = WithRetry(
        &Context::new(),
        Box::new(|| {
            let current = counter;
            counter += 1;
            // 第 4 次调用返回取消。
            if current == 3 {
                Err(SharedError::new(Canceled))
            } else {
                Err(SharedError::new((*ErrKVIngestFailed).clone()))
            }
        }),
        backoff_strategy,
    )
    .unwrap_err();
    assert_eq!(counter, 4);
    assert_eq!(
        err_msgs(&err),
        vec![
            (*ErrKVIngestFailed).to_string(),
            (*ErrKVIngestFailed).to_string(),
            (*ErrKVIngestFailed).to_string(),
            Canceled.to_string(),
        ]
    );
}

/// ConstantBackoff：backedOff 段验证 cancel 截断；infRetry 段零等待直至成功。
#[test]
fn test_constant_backoff() {
    // backedOff：有限常量退避，外部 cancel 后应尽快结束。
    {
        // 10ms 常量退避，便于观察 cancel 效果。
        let backoff_strategy = Box::new(ConstantBackoff(Duration::from_millis(10)));
        let ctx = Context::new();
        let cancel = ctx.clone();
        let i = Arc::new(Mutex::new(0i32));
        let i2 = Arc::clone(&i);
        let ctx2 = ctx.clone();
        let handle = thread::spawn(move || {
            WithRetryV2(
                &ctx2,
                backoff_strategy,
                Box::new(move |_ctx| {
                    let mut guard = i2.lock().unwrap();
                    *guard += 1;
                    let n = *guard;
                    Err::<(), _>(New(format!("{n} times, no meaning")))
                }),
            )
        });
        // 给重试线程若干次迭代时间后再 cancel。
        thread::sleep(Duration::from_millis(100));
        // 取消上下文，打断 WithRetryV2。
        cancel.cancel();
        let result = handle.join().expect("join");
        assert!(result.is_err());
        let count = *i.lock().unwrap();
        // 有退避则迭代次数应明显小于无退避狂奔。
        assert!(count < 20, "expected backoff; got i={count}");
    }

    // infRetry：零等待常量退避，循环直至 i 减到 0。
    {
        // 零等待：快速完成大量重试。
        let backoffer = Box::new(ConstantBackoff(Duration::ZERO));
        let ctx = Context::new();
        // 从 i16::MAX 递减到 0，覆盖大量快速重试。
        let mut i = i16::MAX as i32;
        let err = WithRetryV2(
            &ctx,
            backoffer,
            Box::new(move |_ctx| {
                i -= 1;
                if i == 0 {
                    Ok(())
                } else {
                    Err(New(format!("try {i} more times")))
                }
            }),
        );
        assert!(err.is_ok());
    }
}

#[test]
fn test_peer_download_grpc_cancel_retries_but_context_cancel_stops() {
    let grpc = grpc_status("Canceled", "context canceled");
    let mut peer = crate::backoff::NewPeerDownloadSSTBackoffStrategy();
    assert_eq!(peer.NextBackoff(&grpc), Duration::from_secs(2));
    assert_eq!(peer.RemainingAttempts(), 7);
    let mut legacy = NewDownloadSSTBackoffStrategy();
    assert_eq!(legacy.NextBackoff(&grpc), Duration::ZERO);
    assert_eq!(legacy.RemainingAttempts(), 0);
    let mut peer = crate::backoff::NewPeerDownloadSSTBackoffStrategy();
    let canceled = astersql_errors::Annotate(Some(SharedError::new(Canceled)), "download").unwrap();
    assert_eq!(peer.NextBackoff(&canceled), Duration::ZERO);
    assert_eq!(peer.RemainingAttempts(), 0);
}

#[test]
fn test_peer_download_retry_call_counts_match_context_error_identity() {
    for real_cancel in [false, true] {
        let mut calls = 0;
        let error = if real_cancel {
            SharedError::new(Canceled)
        } else {
            grpc_status("Canceled", "context canceled")
        };
        let result = WithRetry(
            &Context::new(),
            Box::new(|| {
                calls += 1;
                if calls == 1 {
                    Err(error.clone())
                } else {
                    Ok(())
                }
            }),
            crate::backoff::NewPeerDownloadSSTBackoffStrategy(),
        );
        assert_eq!(calls, if real_cancel { 1 } else { 2 });
        assert_eq!(result.is_err(), real_cancel);
        if real_cancel {
            assert_eq!(err_msgs(&result.unwrap_err()), vec!["context canceled"]);
        }
    }
}
