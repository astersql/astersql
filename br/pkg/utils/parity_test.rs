// Copyright 2026 AsterSQL.

//! Go/Rust 公开契约对齐冒烟：抽样校验 utils 包跨模块导出符号的边界行为。
//! 不替代各子模块单测，仅保证常见入口在迁移后仍满足 Go 侧期望。
//! 覆盖 pointer/schema/error/encryption/retry/backoff/tracker 抽样契约。
//! HandleBackupError(None) 期望 StrategyRetry，对齐空错误默认策略。
//! Decrypt 无 cipher 时明文透传，避免误拒未加密备份。
//! Plaintext 方法不算有效加密，防止“假加密”配置。
//! Unknown cipher 必须失败，阻断不安全解密。
//! context canceled by user 走 GiveUp，避免无意义重试。
//! connection reset 属可重试存储错误消息。
//! ImportSST 策略在无效参数上仍应至少尝试一次。
//! ConstantBackoff 不衰减，RemainingAttempts 保持正数。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::kvproto::brpb;
use crate::kvproto::encryptionpb::EncryptionMethod;
use crate::stubs::context::Context;
use crate::stubs::{DecodeMetaKey, EncodeMetaKey, TableKey};
use astersql_br_pkg_errors::ErrInvalidArgument;
use astersql_errors::SharedError;

use crate::{
    BackoffStrategy, EncloseName, ErrorHandlingStrategy, GetOrZero, HandleBackupError,
    HandleUnknownBackupError, IsSysDB, MessageIsRetryableStorageError, NewDefaultContext,
    NewPiTRIdTracker, NewZeroRetryContext, StripTempDBPrefixIfNeeded, UnquoteName, WaitUntil,
    WithRetry,
    backoff::{
        ConstantBackoff, InitialRetryState, NewChecksumBackoffStrategy, NewImportSSTBackoffStrategy,
    },
    encryption::{Decrypt, IsEffectiveEncryptionMethod},
};

#[test]
fn go_rust_public_contract_matches() {
    // normal: pointer helper and schema quoting
    // 指针缺省与 SQL 标识符/临时库前缀剥离。
    assert_eq!(GetOrZero(Some(&7_i32)), 7);
    assert_eq!(GetOrZero(None::<&i32>), 0);
    assert_eq!(EncloseName("a`b"), "`a``b`");
    assert_eq!(UnquoteName("`a``b`"), "a`b");
    assert_eq!(
        StripTempDBPrefixIfNeeded("__TiDB_BR_Temporary_mysql"),
        "mysql"
    );
    assert!(IsSysDB("mysql"));

    // boundary: empty backup error and encryption plaintext passthrough
    // 空备份错误默认重试；明文加密视为无效密文方法。
    let mut ec = NewDefaultContext();
    let res = HandleBackupError(None, 1, &mut ec);
    assert_eq!(res.Strategy, ErrorHandlingStrategy::StrategyRetry);
    assert_eq!(Decrypt(vec![1, 2, 3], None, &[]).unwrap(), vec![1, 2, 3]);
    assert!(!IsEffectiveEncryptionMethod(EncryptionMethod::Plaintext));

    // error: invalid cipher type and unknown backup message give-up/retry
    // Unknown cipher 解密失败；用户取消类消息应 GiveUp；存储重置可重试。
    let mut cipher = brpb::CipherInfo::new();
    cipher.set_cipher_type(EncryptionMethod::Unknown);
    assert!(Decrypt(vec![1], Some(&cipher), &[]).is_err());
    let mut ec = NewZeroRetryContext("test");
    let give_up = HandleUnknownBackupError("context canceled by user", 9, &mut ec);
    assert_eq!(give_up.Strategy, ErrorHandlingStrategy::StrategyGiveUp);
    assert!(MessageIsRetryableStorageError("connection reset by peer"));

    // resource / lifecycle: wait until condition and retry state exhaustion
    // WaitUntil 条件立即为真应成功；指数退避状态在耗尽后 ShouldRetry=false。
    let ctx = Context::new();
    let mut done = false;
    let err = WaitUntil(
        &ctx,
        || {
            done = true;
            done
        },
        Duration::from_millis(1),
        Duration::from_millis(50),
    );
    assert!(err.is_ok());

    let mut state = InitialRetryState(2, Duration::from_millis(1), Duration::from_millis(4));
    assert!(state.ShouldRetry());
    let _ = state.ExponentialBackoff();
    assert!(state.ShouldRetry());
    let _ = state.ExponentialBackoff();
    assert!(!state.ShouldRetry());

    // retry/backoff integration: import strategy eventually stops
    // ImportSST 策略在持续 ErrInvalidArgument 下应有限次重试后失败。
    let attempts = Arc::new(AtomicU32::new(0));
    let strategy = NewImportSSTBackoffStrategy();
    let ctx = Context::new();
    let err = WithRetry(
        &ctx,
        Box::new({
            let attempts = Arc::clone(&attempts);
            move || {
                attempts.fetch_add(1, Ordering::Relaxed);
                Err(SharedError::new((*ErrInvalidArgument).clone()))
            }
        }),
        strategy,
    );
    assert!(err.is_err());
    assert!(attempts.load(Ordering::Relaxed) > 0);

    // filter tracker membership
    // PiTR 表 ID 跟踪器按 (db, table) 精确匹配。
    let mut tracker = NewPiTRIdTracker();
    tracker.TrackTableId(1, 10);
    assert!(tracker.ContainsDBAndTableId(1, 10));
    assert!(!tracker.ContainsDBAndTableId(2, 10));

    // constant backoff keeps returning the configured delay
    // 常量退避始终返回固定间隔，且剩余次数为正。
    let mut constant = ConstantBackoff(Duration::from_millis(5));
    assert_eq!(
        constant.NextBackoff(&SharedError::new((*ErrInvalidArgument).clone())),
        Duration::from_millis(5)
    );
    assert!(constant.RemainingAttempts() > 0);

    // 构造 checksum 策略仅验证工厂可调用，不驱动重试循环。
    let _ = NewChecksumBackoffStrategy();
}

#[test]
fn wait_until_uses_ticker_interval_and_honors_timeout_deadline() {
    let ctx = Context::new();
    let mut checks = 0;
    let started = Instant::now();
    WaitUntil(
        &ctx,
        || {
            checks += 1;
            checks == 2
        },
        Duration::from_millis(40),
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(30),
        "the second condition check must be driven by the ticker"
    );

    let started = Instant::now();
    let err = WaitUntil(
        &ctx,
        || false,
        Duration::from_millis(200),
        Duration::from_millis(30),
    )
    .unwrap_err();
    assert!(err.to_string().contains("timed out"));
    assert!(
        started.elapsed() < Duration::from_millis(150),
        "a long check interval must not postpone max_timeout"
    );
}

#[test]
fn cancelling_child_context_does_not_cancel_parent() {
    let parent = Context::new();
    let child = parent.child_token();
    child.cancel();
    assert!(child.is_cancelled());
    assert!(
        !parent.is_cancelled(),
        "Go context cancellation only propagates from parent to child"
    );
}

#[test]
fn decode_meta_key_reports_invalid_hash_flag_as_a_character_like_go() {
    let mut encoded = EncodeMetaKey(b"", b"field").0;
    // m + mem-comparable empty byte slice occupies ten bytes; the following
    // big-endian u64 is structure.HashData. Replace its low byte with `x`.
    encoded[17] = b'x';

    let err = DecodeMetaKey(TableKey(encoded)).unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid encoded hash data key flag x",
        "Go tablecodec.DecodeMetaKey formats the invalid flag with %c"
    );
}
