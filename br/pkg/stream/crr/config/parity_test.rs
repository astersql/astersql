// Copyright 2026 AsterSQL.

//! Go/Rust 公共契约对照测试：验证 CRR config 默认值与 Parse 覆盖与 Go 一致。
//! 断言依据来自 Go `TestDefaultConfig` / `TestParse`；不改行为，仅锁定跨语言表面。
//! 依赖 checkpoint/service 默认常量，确保跨 crate 默认值链路不被静默漂移。

use std::time::Duration;

use astersql_br_pkg_stream_crr_internal_checkpoint::{
    DefaultMetaReadConcurrency, DefaultPollInterval,
};
use astersql_br_pkg_stream_crr_service::DefaultRetryInterval;

use crate::{Config, DefaultConfig, DefineFlags, FlagSet};

#[test]
fn go_rust_public_contract_matches() {
    // 正常路径：DefaultConfig 各字段对齐 Go TestDefaultConfig。
    let cfg = DefaultConfig();
    assert_eq!(cfg.RetryInterval(), DefaultRetryInterval);
    assert_eq!(cfg.PollInterval(), DefaultPollInterval);
    assert_eq!(cfg.MetaReadConcurrency(), DefaultMetaReadConcurrency);
    assert_eq!(cfg.TaskName(), "");

    // 正常路径：Parse 用 flag 覆盖默认值（对齐 Go TestParse）。
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    flags
        .Parse(&[
            "--task-name",
            "task",
            "--retry-interval",
            "7s",
            "--calc.poll-interval",
            "3s",
            "--calc.meta-read-concurrency",
            "9",
        ])
        .expect("pflag-equivalent parse");

    let mut cfg = Config::default();
    cfg.Parse(&flags).expect("parse");
    assert_eq!(cfg.TaskName(), "task");
    assert_eq!(cfg.RetryInterval(), Duration::from_secs(7));
    assert_eq!(cfg.PollInterval(), Duration::from_secs(3));
    assert_eq!(cfg.MetaReadConcurrency(), 9);

    // 边界：只 DefineFlags、不 Set*，Parse 后仍保持 Go 默认值。
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    let mut cfg = Config {
        inner: Default::default(),
    };
    cfg.Parse(&flags).expect("parse defaults");
    assert_eq!(cfg.TaskName(), "");
    assert_eq!(cfg.RetryInterval(), DefaultRetryInterval);
    assert_eq!(cfg.PollInterval(), DefaultPollInterval);
    assert_eq!(cfg.MetaReadConcurrency(), DefaultMetaReadConcurrency);

    // 错误路径：未定义 flag 上 Get* 应失败，防止静默零值。
    let empty = FlagSet::new();
    assert!(empty.GetString("missing").is_err());

    // 资源：FlagSet 为自有值，drop 即可释放，无外部句柄泄漏。
    drop(flags);
}
