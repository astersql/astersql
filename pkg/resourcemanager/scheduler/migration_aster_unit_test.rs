// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

// CPU 调度器迁移期单元测试：阈值、调容间隔与 trait 分派。
//
// 用 MockPool 模拟 goroutine 池，验证 `command_for_cpu_usage` 与 Go 侧阈值一致，
// 以及近期刚调容时返回 Hold、通过 Scheduler trait 能正确委托到 CPUScheduler。

use astersql_resourcemanager_scheduler::{
    CPUScheduler, Command, NewCPUScheduler, Scheduler, command_for_cpu_usage, util,
};
use std::time::{Duration, SystemTime};

/// 测试用假池：仅提供 LastTunerTs 等查询，Tune 为空操作。
struct MockPool {
    /// 上次调容时间戳。
    last_tuner_ts: SystemTime,
}

impl util::GoroutinePool for MockPool {
    fn ReleaseAndWait(&self) {}
    fn Tune(&self, _size: i32) {}
    fn LastTunerTs(&self) -> SystemTime {
        self.last_tuner_ts
    }
    fn Cap(&self) -> i32 {
        1
    }
    fn Running(&self) -> i32 {
        0
    }
    fn Name(&self) -> &str {
        "mock"
    }
    fn GetOriginConcurrency(&self) -> i32 {
        1
    }
}

/// 验证 CPU 阈值边界与 unsupported 分支与 Go 一致。
#[test]
fn migration_cpu_thresholds_match_go() {
    assert_eq!(command_for_cpu_usage(0.49, false), Command::Overclock);
    assert_eq!(command_for_cpu_usage(0.50, false), Command::Hold);
    assert_eq!(command_for_cpu_usage(0.70, false), Command::Hold);
    assert_eq!(command_for_cpu_usage(0.71, false), Command::Downclock);
    assert_eq!(command_for_cpu_usage(0.10, true), Command::Hold);
}

/// 刚调容过的池在最小间隔内应返回 Hold，不采样升容。
#[test]
fn migration_recent_tune_is_held_before_sampling() {
    let scheduler = NewCPUScheduler();
    let pool = MockPool {
        last_tuner_ts: SystemTime::now(),
    };

    assert_eq!(scheduler.Tune(util::DDL, &pool), Command::Hold);
}

/// 通过 dyn Scheduler 分派到 CPUScheduler，且距上次调容已久时应 Overclock。
#[test]
fn migration_scheduler_trait_dispatches_to_cpu_scheduler() {
    let scheduler: Box<dyn Scheduler> = Box::new(CPUScheduler);
    let pool = MockPool {
        last_tuner_ts: SystemTime::now() - Duration::from_secs(1),
    };

    assert_eq!(scheduler.Tune(util::DistTask, &pool), Command::Overclock);
}
