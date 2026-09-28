// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/stream/crr/config/config_test.go`.
//!
//! Pure config defaults + FlagSet parsing; no PD/TiKV/network. Rust uses the
//! local `FlagSet` stand-in to parse the same argv shape as pflag.
//! CRR 配置默认值与 FlagSet 覆盖解析；无网络依赖。
//! Rust 直接覆盖 Go pflag.Parse 的参数解析契约。
//! 同时覆盖等号形式、复合 duration 与非法标志路径。

use std::time::Duration;

use astersql_br_pkg_stream_crr_internal_checkpoint::{
    DefaultMetaReadConcurrency, DefaultPollInterval,
};
use astersql_br_pkg_stream_crr_service::DefaultRetryInterval;

use crate::{Config, DefaultConfig, DefineFlags, FlagSet};

/// Corresponds to Go `TestDefaultConfig`.
/// 默认配置应等于各子包导出的 Default* 常量。
#[test]
fn test_default_config() {
    let cfg = DefaultConfig();
    assert_eq!(cfg.RetryInterval(), DefaultRetryInterval);
    assert_eq!(cfg.PollInterval(), DefaultPollInterval);
    assert_eq!(cfg.MetaReadConcurrency(), DefaultMetaReadConcurrency);
}

/// Corresponds to Go `TestParse`.
///
/// Go: `pflag.Parse([]string{"--task-name", "task", "--retry-interval", "7s", ...})`
/// then `cfg.Parse(flags)`. Rust parses the same argument vector.
/// 覆盖 task-name/retry/poll/concurrency 后 Parse 应写入 Config。
#[test]
fn test_parse() {
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
        .expect("Go require.NoError: flags.Parse");

    let mut cfg = Config::default();
    cfg.Parse(&flags).expect("Go require.NoError: cfg.Parse");
    assert_eq!(cfg.TaskName(), "task");
    assert_eq!(cfg.RetryInterval(), Duration::from_secs(7));
    assert_eq!(cfg.PollInterval(), Duration::from_secs(3));
    assert_eq!(cfg.MetaReadConcurrency(), 9);
}

#[test]
fn test_flag_parse_forms_and_errors() {
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    flags
        .Parse(&[
            "--task-name=task-equals",
            "--retry-interval=1m30s",
            "--calc.poll-interval=250ms",
            "--calc.meta-read-concurrency=4",
        ])
        .expect("pflag long options accept equals and separated forms");

    let mut cfg = Config::default();
    cfg.Parse(&flags).expect("parsed flags remain readable");
    assert_eq!(cfg.TaskName(), "task-equals");
    assert_eq!(cfg.RetryInterval(), Duration::from_secs(90));
    assert_eq!(cfg.PollInterval(), Duration::from_millis(250));
    assert_eq!(cfg.MetaReadConcurrency(), 4);

    assert!(flags.Parse(&["--unknown", "value"]).is_err());
    assert!(flags.Parse(&["--retry-interval", "never"]).is_err());
    assert!(
        flags
            .Parse(&["--calc.meta-read-concurrency", "many"])
            .is_err()
    );
    assert!(flags.Parse(&["--", "--unknown", "value"]).is_ok());
}

/// pflag rejects single-dash input when no matching shorthand is registered.
#[test]
fn test_unknown_shorthand_is_rejected() {
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);

    assert!(flags.Parse(&["-task-name", "task"]).is_err());

    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    assert!(flags.Parse(&["-"]).is_ok());
}

/// Go's `time.Duration` is a signed 64-bit nanosecond count. Values above its
/// positive maximum must fail even though `std::time::Duration` could hold them.
#[test]
fn test_duration_rejects_values_above_go_maximum() {
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);

    assert!(
        flags
            .Parse(&["--retry-interval", "2562047h47m16.854775808s"])
            .is_err()
    );
}

#[test]
fn test_duration_accepts_go_positive_boundaries() {
    let cases = [
        (".5s", Duration::from_millis(500)),
        ("+1s", Duration::from_secs(1)),
        (
            "2562047h47m16.854775807s",
            Duration::from_nanos(i64::MAX as u64),
        ),
    ];

    for (value, expected) in cases {
        let mut flags = FlagSet::new();
        DefineFlags(&mut flags);
        flags
            .Parse(&["--retry-interval", value])
            .expect("Go time.ParseDuration accepts the positive boundary");
        let mut cfg = Config::default();
        cfg.Parse(&flags).expect("parsed duration remains readable");
        assert_eq!(cfg.RetryInterval(), expected);
    }
}
