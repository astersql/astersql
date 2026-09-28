// Copyright 2022 PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// PhaseTracer 单元测试：验证相位切换上报耗时与 EndPhase 清空行为。

use astersql_ttl_metrics::ttl_metrics::newPhaseTracer;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 用可注入时钟驱动 EnterPhase / EndPhase，断言上报的相位名与时长。
#[test]
fn test_phase_tracer() {
    let now = Arc::new(Mutex::new(Instant::now()));
    let last_report = Arc::new(Mutex::new(None));
    let clock = Arc::clone(&now);
    let report = Arc::clone(&last_report);
    // 每次上报必须先被 take 消费，保证一次只积压一条记录。
    let mut tracer = newPhaseTracer(
        move || *clock.lock().unwrap(),
        move |status, duration| {
            let mut report = report.lock().unwrap();
            assert!(report.is_none(), "previous report must be consumed first");
            *report = Some((status.to_owned(), duration));
        },
    );

    *now.lock().unwrap() += Duration::from_secs(2);
    tracer.EnterPhase("p1");
    assert!(last_report.lock().unwrap().is_none());
    assert_eq!(tracer.Phase(), "p1");

    *now.lock().unwrap() += Duration::from_secs(5);
    tracer.EnterPhase("p2");
    assert_eq!(
        last_report.lock().unwrap().take(),
        Some(("p1".to_owned(), Duration::from_secs(5)))
    );
    assert_eq!(tracer.Phase(), "p2");

    *now.lock().unwrap() += Duration::from_secs(10);
    tracer.EnterPhase("p2");
    assert_eq!(
        last_report.lock().unwrap().take(),
        Some(("p2".to_owned(), Duration::from_secs(10)))
    );
    assert_eq!(tracer.Phase(), "p2");

    *now.lock().unwrap() += Duration::from_secs(20);
    tracer.EndPhase();
    assert_eq!(
        last_report.lock().unwrap().take(),
        Some(("p2".to_owned(), Duration::from_secs(20)))
    );
    assert_eq!(tracer.Phase(), "");
}
