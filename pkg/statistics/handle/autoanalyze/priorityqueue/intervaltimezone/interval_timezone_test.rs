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

// Same-path Go->Rust mapping for `interval_timezone_test.go`.
//
// Verifies that `start_analyze_job` records start_time using the session
// timezone (Europe/Berlin) rather than a contaminated system timezone
// (America/New_York). WARNING: Do not add other test cases to this file; it
// exists solely to cover time zone contamination.
//
// 对应 Go `interval_timezone_test.go`：验证 `start_analyze_job` 用会话时区
//（Europe/Berlin）记录 start_time，而非被污染的系统时区（America/New_York）。
// 本文件仅覆盖时区污染场景，勿追加其它用例。

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use astersql_statistics_handle_autoanalyze::{
    AnalyzeJob, Error, JobStore, JobType, SqlValue, finish_analyze_job, insert_analyze_job,
    start_analyze_job,
};
use astersql_statistics_handle_autoanalyze_priorityqueue::{
    AnalysisHistoryReader, GetLastFailedAnalysisDuration,
};
use astersql_util_timeutil::time_zone::SetSystemTZ;

/// In-memory JobStore that applies `CONVERT_TZ(..., '+00:00', @@TIME_ZONE)`
/// using the session timezone — matching Go StartAnalyzeJob via CallWithSCtx.
///
/// 内存 JobStore：按会话时区模拟 `CONVERT_TZ(..., '+00:00', @@TIME_ZONE)`，
/// 对齐 Go 经 CallWithSCtx 调用 StartAnalyzeJob 的行为。
struct TimezoneAwareJobStore {
    /// 会话/全局时区名（测试中应为 Europe/Berlin）。
    session_tz: String,
    /// 自增作业 ID。
    next_id: u64,
    /// 已插入的分析作业。
    jobs: HashMap<u64, StoredJob>,
}

/// 内存中保存的一条 analyze_jobs 记录。
#[derive(Clone)]
struct StoredJob {
    database: String,
    table: String,
    state: String,
    start_time: Option<SystemTime>,
}

/// Return the offsets used by the two named zones in this contamination case.
///
/// The test only needs a wall-clock separation large enough to distinguish the
/// session zone from the contaminated system zone; both offsets are constant
/// for the scenario's current summer instant.
fn timezone_offset_seconds(timezone: &str) -> i64 {
    match timezone {
        "Europe/Berlin" => 2 * 60 * 60,
        "America/New_York" => -4 * 60 * 60,
        _ => 0,
    }
}

/// Model MySQL's `CONVERT_TZ(utc_value, '+00:00', target)` as a stored wall
/// clock value. The history query applies the same session-zone representation
/// to `CURRENT_TIMESTAMP`, so a correct conversion leaves only elapsed time.
fn convert_utc_epoch_to_wall_clock(epoch_seconds: u64, timezone: &str) -> SystemTime {
    let seconds = i128::from(epoch_seconds) + i128::from(timezone_offset_seconds(timezone));
    if seconds >= 0 {
        UNIX_EPOCH
            .checked_add(Duration::from_secs(seconds as u64))
            .expect("test timestamp should fit in SystemTime")
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_secs((-seconds) as u64))
            .expect("test timestamp should fit in SystemTime")
    }
}

impl TimezoneAwareJobStore {
    /// 以给定会话时区构造空存储。
    fn new(session_tz: &str) -> Self {
        Self {
            session_tz: session_tz.to_owned(),
            next_id: 1,
            jobs: HashMap::new(),
        }
    }
}

impl JobStore for TimezoneAwareJobStore {
    fn execute(&mut self, sql: &str, args: &[SqlValue]) -> Result<(), Error> {
        let sql_l = sql.to_ascii_lowercase();
        // 插入 pending 作业。
        if sql_l.starts_with("insert into mysql.analyze_jobs") {
            let database = match &args[0] {
                SqlValue::Text(s) => s.clone(),
                _ => String::new(),
            };
            let table = match &args[1] {
                SqlValue::Text(s) => s.clone(),
                _ => String::new(),
            };
            let id = self.next_id;
            self.next_id += 1;
            self.jobs.insert(
                id,
                StoredJob {
                    database,
                    table,
                    state: "pending".into(),
                    start_time: None,
                },
            );
            return Ok(());
        }
        if sql_l.contains("set start_time = convert_tz") {
            // Session timezone must be the global value (Europe/Berlin), not the
            // contaminated system zone. Recording start with the wrong zone is
            // what produces a negative TIMESTAMPDIFF in Go.
            // 会话时区必须是 Europe/Berlin，而非被污染的系统时区；错误时区会导致负 TIMESTAMPDIFF。
            assert_eq!(
                self.session_tz, "Europe/Berlin",
                "StartAnalyzeJob must use session/global TZ, not system TZ"
            );
            let id = match args.last() {
                Some(SqlValue::U64(v)) => *v,
                Some(SqlValue::I64(v)) => *v as u64,
                _ => return Ok(()),
            };
            let epoch_seconds = match args.first() {
                Some(SqlValue::U64(v)) => *v,
                Some(SqlValue::I64(v)) => *v as u64,
                _ => return Ok(()),
            };
            let target_timezone = if sql_l.contains("@@time_zone") {
                self.session_tz.as_str()
            } else {
                // A regression that uses the contaminated system zone should
                // produce the same large gap as the Go integration test.
                "America/New_York"
            };
            if let Some(job) = self.jobs.get_mut(&id) {
                job.start_time = Some(convert_utc_epoch_to_wall_clock(
                    epoch_seconds,
                    target_timezone,
                ));
                job.state = "running".into();
            }
            return Ok(());
        }
        // 带 fail_reason 的更新视为标记失败。
        if sql_l.contains("fail_reason") {
            let id = match args.last() {
                Some(SqlValue::U64(v)) => *v,
                Some(SqlValue::I64(v)) => *v as u64,
                _ => return Ok(()),
            };
            if let Some(job) = self.jobs.get_mut(&id) {
                job.state = "failed".into();
            }
            return Ok(());
        }
        Ok(())
    }

    fn last_insert_id(&mut self) -> Result<u64, Error> {
        Ok(self.next_id - 1)
    }

    fn stale_jobs_for_instance(
        &mut self,
        _instance: &str,
    ) -> Result<Vec<(u64, Option<u64>)>, Error> {
        Ok(Vec::new())
    }

    fn stale_jobs(&mut self) -> Result<Vec<(u64, String)>, Error> {
        Ok(Vec::new())
    }

    fn live_instances(&mut self) -> Result<std::collections::HashSet<String>, Error> {
        Ok(std::collections::HashSet::new())
    }
}

/// 从内存 JobStore 读取失败作业，供 GetLastFailedAnalysisDuration 使用。
struct StoreHistoryReader<'a> {
    store: &'a TimezoneAwareJobStore,
}

impl AnalysisHistoryReader for StoreHistoryReader<'_> {
    fn query_optional_f64(&self, _sql: &str, _params: &[String]) -> Result<Option<f64>, String> {
        Ok(None)
    }

    fn query_optional_i64(&self, _sql: &str, params: &[String]) -> Result<Option<i64>, String> {
        let schema = params.first().map(String::as_str).unwrap_or("");
        let table = params.get(1).map(String::as_str).unwrap_or("");
        let mut best: Option<SystemTime> = None;
        // 取同 schema/table 下最近一次 failed 的 start_time。
        for job in self.store.jobs.values() {
            if job.database == schema && job.table == table && job.state == "failed" {
                if let Some(start) = job.start_time {
                    best = Some(match best {
                        Some(prev) if start > prev => start,
                        Some(prev) => prev,
                        None => start,
                    });
                }
            }
        }
        let Some(start) = best else {
            return Ok(None);
        };
        let now_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("current time should be after Unix epoch")
            .as_secs();
        let now = convert_utc_epoch_to_wall_clock(now_epoch, &self.store.session_tz);
        // 若 start 在未来则返回负秒数，与 TIMESTAMPDIFF 符号一致。
        let secs = now
            .duration_since(start)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_else(|err| -(err.duration().as_secs() as i64));
        Ok(Some(secs))
    }
}

/// 系统时区被污染时，会话时区正确则失败间隔仍为正且小于一分钟。
#[test]
fn test_last_failed_analysis_duration_use_correct_timezone() {
    // Force the system time zone to America/New_York to simulate contamination.
    // This must happen before bootstrap because SetSystemTZ is guarded by sync.Once.
    // 将系统时区强制为 America/New_York 以模拟污染；须在 bootstrap 前调用（sync.Once）。
    SetSystemTZ("America/New_York");

    // Session timezone is Europe/Berlin — StartAnalyzeJob must use this, not the
    // contaminated system zone. (Go sets @@global.time_zone then CallWithSCtx
    // resets the stats session; here the JobStore is constructed with Berlin.)
    // 会话时区 Europe/Berlin；StartAnalyzeJob 必须用该时区而非系统时区。
    let mut store = TimezoneAwareJobStore::new("Europe/Berlin");

    let mut job = AnalyzeJob {
        database: "db_reset".to_owned(),
        table: "tbl_reset".to_owned(),
        job_info: "test job".to_owned(),
        ..Default::default()
    };
    insert_analyze_job(&mut store, &mut job, "test-instance", 1).expect("insert job");
    assert!(job.id.is_some());

    start_analyze_job(&mut store, Some(&mut job), SystemTime::now());
    let stored_start = store
        .jobs
        .get(&job.id.expect("inserted job id"))
        .and_then(|stored| stored.start_time)
        .expect("start time should be recorded");
    assert!(
        stored_start
            .duration_since(job.start_time.expect("job start time"))
            .expect("session conversion should not move start before input")
            >= Duration::from_secs(60 * 60),
        "stored start time must include the Europe/Berlin session conversion"
    );
    std::thread::sleep(Duration::from_secs(2));
    finish_analyze_job(
        &mut store,
        Some(&mut job),
        Some(&Error("test error".into())),
        JobType::TableAnalysis,
        SystemTime::now(),
    );

    let reader = StoreHistoryReader { store: &store };
    let dur = GetLastFailedAnalysisDuration(&reader, "db_reset", "tbl_reset", &[])
        .expect("query duration")
        .expect("failed job should exist");
    assert!(
        dur > Duration::from_secs(0),
        "duration should be positive; negative means timezone was not reset"
    );
    assert!(
        dur < Duration::from_secs(60),
        "duration should be less than a minute"
    );
}
