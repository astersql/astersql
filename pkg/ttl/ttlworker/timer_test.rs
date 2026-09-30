// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// TTL 定时器运行时启停状态的单元测试。
//
// 验证 pause/resume 可重复调用且状态幂等，避免重复启停导致竞态。

/// 连续 resume/pause 后，`running` 标志应与最后一次操作一致。
#[test]
fn ttl_timer_runtime_pause_and_resume_are_idempotent() {
    let mut runtime = crate::timer::TtlTimerRuntime::default();
    runtime.resume();
    runtime.resume();
    assert!(runtime.running);
    runtime.pause();
    runtime.pause();
    assert!(!runtime.running);
}

#[test]
fn ttl_schedule_window_matches_inclusive_go_boundaries() {
    let mut manager = crate::job_manager::JobManager::new("manager", 1);
    manager.is_leader = true;
    manager.refresh_tables([crate::session::PhysicalTable {
        partition_name: None,
        table_id: 1,
        physical_id: 1,
        schema: "test".into(),
        table: "t".into(),
        key_columns: vec!["id".into()],
        ttl_column: "created_at".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 1,
    }]);
    manager.now = 6 * 60;
    let mut hook = crate::timer::TtlTimerHook {
        adapter: manager,
        schedule_start_minute: 0,
        schedule_end_minute: 6,
    };
    let response = hook
        .on_event(&crate::timer::TimerEvent {
            event_id: "event".into(),
            table_id: 1,
            physical_id: 1,
            request_id: "request".into(),
            created_at: 0,
        })
        .unwrap();
    assert!(matches!(
        response,
        crate::timer::TimerResponse::Submitted(_)
    ));
}

#[test]
fn interval_defaults_match_go_failpoint_fallbacks() {
    use std::time::Duration;
    let intervals = crate::config::IntervalOverrides::default();
    assert_eq!(intervals.heartbeat(), Duration::from_secs(10));
    assert_eq!(intervals.sync_timer(), Duration::from_secs(1));
    assert_eq!(intervals.check_task(), Duration::from_secs(5));
    assert_eq!(intervals.check_triggered_job(), Duration::from_secs(2));
}

#[derive(Default)]
struct RecordingAdapter {
    now: u64,
    submitted: Option<(i64, i64, String, u64)>,
    job: Option<crate::timer::TtlJobTrace>,
}

impl crate::timer::TtlJobAdapter for RecordingAdapter {
    fn can_submit_job(&self, _table_id: i64, _physical_id: i64) -> bool {
        true
    }

    fn submit_job(
        &mut self,
        table_id: i64,
        physical_id: i64,
        request_id: &str,
        watermark: u64,
    ) -> Result<crate::timer::TtlJobTrace, String> {
        self.submitted = Some((table_id, physical_id, request_id.to_owned(), watermark));
        Ok(crate::timer::TtlJobTrace {
            request_id: request_id.to_owned(),
            finished: false,
            summary: None,
        })
    }

    fn get_job(
        &self,
        _table_id: i64,
        _physical_id: i64,
        _request_id: &str,
    ) -> Result<crate::timer::TtlJobTrace, String> {
        self.job.clone().ok_or_else(|| "job not found".to_owned())
    }

    fn now(&self) -> u64 {
        self.now
    }
}

fn timer_event() -> crate::timer::TimerEvent {
    crate::timer::TimerEvent {
        event_id: "event-id".to_owned(),
        table_id: 10,
        physical_id: 11,
        request_id: "legacy-request-id".to_owned(),
        created_at: 123,
    }
}

#[test]
fn cross_midnight_window_includes_go_end_boundary() {
    let mut hook = crate::timer::TtlTimerHook {
        adapter: RecordingAdapter {
            now: 6 * 60 * 60,
            ..Default::default()
        },
        schedule_start_minute: 22 * 60,
        schedule_end_minute: 6 * 60,
    };

    assert!(matches!(
        hook.on_event(&timer_event()).unwrap(),
        crate::timer::TimerResponse::Submitted(_)
    ));
}

#[test]
fn submission_uses_go_event_id_and_event_start_watermark() {
    let mut hook = crate::timer::TtlTimerHook {
        adapter: RecordingAdapter {
            now: 999,
            ..Default::default()
        },
        schedule_start_minute: 0,
        schedule_end_minute: 23 * 60 + 59,
    };

    hook.on_event(&timer_event()).unwrap();
    assert_eq!(
        hook.adapter.submitted,
        Some((10, 11, "event-id".to_owned(), 123))
    );
}

#[test]
fn closed_summary_records_go_event_id() {
    let mut hook = crate::timer::TtlTimerHook {
        adapter: RecordingAdapter {
            job: Some(crate::timer::TtlJobTrace {
                request_id: "event-id".to_owned(),
                finished: true,
                summary: None,
            }),
            ..Default::default()
        },
        schedule_start_minute: 0,
        schedule_end_minute: 23 * 60 + 59,
    };

    let crate::timer::TimerResponse::Closed(summary) = hook.poll(&timer_event()).unwrap() else {
        panic!("finished jobs must close the timer event");
    };
    assert_eq!(summary.last_job_request_id, "event-id");
}
