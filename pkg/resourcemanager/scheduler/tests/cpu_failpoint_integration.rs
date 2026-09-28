// Copyright 2026 AsterSQL.

// CPU 观测失败点与资源调度器联动的跨 crate 集成测试。
//
// 验证 cgroup 探测失败时，观测器会发布“不支持”状态，调度器因而保持当前并发度，
// 避免把缺失的 CPU 数据误判为低负载并扩容。

use astersql_resourcemanager_scheduler::{Command, NewCPUScheduler, util};
use std::time::{Duration, SystemTime};

/// 提供调度器所需最小状态的固定容量线程池桩。
struct MockPool {
    /// 保留可控的最近调容时间，以便测试越过调度防抖间隔并真正触发 CPU 采样。
    last_tuner_ts: SystemTime,
}

impl util::GoroutinePool for MockPool {
    fn ReleaseAndWait(&self) {}
    fn Tune(&self, _size: i32) {}
    fn LastTunerTs(&self) -> SystemTime {
        self.last_tuner_ts
    }
    fn Cap(&self) -> i32 {
        10
    }
    fn Running(&self) -> i32 {
        0
    }
    fn Name(&self) -> &str {
        "test"
    }
    fn GetOriginConcurrency(&self) -> i32 {
        10
    }
}

#[test]
/// 验证 cgroup 探测失败会沿观测器状态传播，并最终使 CPU 调度器返回 `Hold`。
fn cgroup_failpoint_drives_cpu_scheduler_to_hold() {
    // 失败场景对象存活期间持续启用失败点，离开作用域后自动清理全局配置。
    let _scenario = cpu_crate::setup_cgroup_cpu_error_failpoint_for_test();

    let mut observer = cpu_crate::NewCPUObserver();
    observer.Start();

    // Start 在失败点处分支返回，不启动采样线程，并将共享快照标记为不支持。
    let (value, unsupported) = cpu_crate::GetCPUUsage();
    assert!(unsupported);
    assert_eq!(value, 0.0);

    let scheduler = NewCPUScheduler();
    // 一秒前已超过默认防抖间隔，确保 Tune 读取上述 CPU 快照而非提前返回。
    let pool = MockPool {
        last_tuner_ts: SystemTime::now() - Duration::from_secs(1),
    };
    assert_eq!(scheduler.Tune(util::UNKNOWN, &pool), Command::Hold);
}
