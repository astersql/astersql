// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/utils/memory_monitor_test.go`.
//! 校验 BR 内存告警 ConfigProvider：比例/保留条数/日志目录回退与组件名。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use astersql_util_memoryusagealarm::ConfigProvider;

use crate::memory_monitor::{BRConfigProvider, DefaultProfilesDir, spawn_memory_alarm};
use crate::stubs::context::Context;

#[test]
fn test_br_config_provider() {
    // 构造自定义阈值与目录，断言 getter 原样返回。
    let mut provider = BRConfigProvider::new(0.8, 3, "/custom/dir");
    assert!((provider.GetMemoryUsageAlarmRatio() - 0.8).abs() < 1e-9);
    assert_eq!(provider.GetMemoryUsageAlarmKeepRecordNum(), 3);
    assert_eq!(provider.GetLogDir(), "/custom/dir");

    // 空日志目录回退到 DefaultProfilesDir；组件名固定为 br。
    provider.set_log_dir("");
    assert_eq!(provider.GetLogDir(), DefaultProfilesDir);
    assert_eq!(provider.GetComponentName(), "br");
}

#[test]
fn br_config_provider_preserves_ratio_precision() {
    let ratio = 0.812_345_678_901_234_5;
    let provider = BRConfigProvider::new(ratio, 3, "/custom/dir");

    assert_eq!(
        provider.GetMemoryUsageAlarmRatio().to_bits(),
        ratio.to_bits()
    );
}

struct CountingProvider {
    reads: Arc<AtomicUsize>,
}

impl ConfigProvider for CountingProvider {
    fn GetMemoryUsageAlarmRatio(&self) -> f64 {
        self.reads.fetch_add(1, Ordering::SeqCst);
        2.0
    }

    fn GetMemoryUsageAlarmKeepRecordNum(&self) -> i64 {
        1
    }

    fn GetLogDir(&self) -> String {
        DefaultProfilesDir.to_string()
    }

    fn GetComponentName(&self) -> String {
        "br".to_string()
    }
}

#[test]
fn run_memory_monitor_starts_alarm_and_stops_on_cancel() {
    let reads = Arc::new(AtomicUsize::new(0));
    let ctx = Context::new();
    let join = spawn_memory_alarm(
        ctx.clone(),
        Arc::new(CountingProvider {
            reads: Arc::clone(&reads),
        }),
    );
    std::thread::sleep(Duration::from_millis(250));
    assert!(
        reads.load(Ordering::SeqCst) > 0,
        "alarm loop did not poll config"
    );
    ctx.cancel();
    join.join().expect("memory alarm watcher must stop cleanly");
}
