// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Same-path Go->Rust mapping for `pkg/statistics/handle/autoanalyze/exec`.
// `AutoAnalyze` / `RunAnalyzeStmt` escape SQL, emit the legacy-version rewrite
// warning, register the auto-analyze process id, and execute through the
// caller-supplied session executor — matching Go `exec.AutoAnalyze`.
//
// 自动分析执行路径：转义 ANALYZE SQL、按需告警改写 legacy 统计版本、
// 注册系统进程 ID 后通过会话执行器跑语句，并提供时间窗口解析与窗口外杀进程。

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use astersql_sessionctx_vardef as vardef;
use astersql_statistics::Version2;
use astersql_statistics_handle_logutil::StatsLogger;
use astersql_statistics_handle_logutil::log::{LogField, LogLevel};
use astersql_statistics_handle_util::{
    AutoAnalyzeProcIdGenerator, GLOBAL_AUTO_ANALYZE_PROCESS_LIST, TrackProc,
};
use astersql_util_sqlescape::{EscapeSQL, SqlArg};
use astersql_util_timeutil::time_zone::WithinDayTimePeriod;
use chrono::{FixedOffset, TimeZone, Utc};

/// 自动分析修改比率的系统变量名。
pub const TIDB_AUTO_ANALYZE_RATIO: &str = vardef::TiDBAutoAnalyzeRatio;
/// 自动分析时间窗口起始的系统变量名。
pub const TIDB_AUTO_ANALYZE_START_TIME: &str = vardef::TiDBAutoAnalyzeStartTime;
/// 自动分析时间窗口结束的系统变量名。
pub const TIDB_AUTO_ANALYZE_END_TIME: &str = vardef::TiDBAutoAnalyzeEndTime;
/// 默认自动分析修改比率。
pub const DEF_AUTO_ANALYZE_RATIO: f64 = vardef::DefAutoAnalyzeRatio;
/// 默认窗口起始时间字符串。
pub const DEF_AUTO_ANALYZE_START_TIME: &str = vardef::DefAutoAnalyzeStartTime;
/// 默认窗口结束时间字符串。
pub const DEF_AUTO_ANALYZE_END_TIME: &str = vardef::DefAutoAnalyzeEndTime;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 自动分析执行错误。
pub struct AnalyzeError(pub String);

impl fmt::Display for AnalyzeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AnalyzeError {}

/// Session-side analyze executor. Go uses `statsutil.ExecWithOpts`; Rust callers
/// supply a concrete executor (typically a TestKit-backed session).
/// 会话侧 ANALYZE 执行器；Go 用 `statsutil.ExecWithOpts`，此处由调用方注入。
/// 会话侧 ANALYZE 执行器（对应 Go `statsutil.ExecWithOpts`）。
pub trait AnalyzeExecutor: Send + Sync {
    fn Execute(&self, sql: &str) -> Result<(), AnalyzeError>;
}

/// 系统进程跟踪：登记/注销自动分析进程，并可发送 kill。
pub trait SysProcTracker: Send + Sync {
    fn Track(&self, id: u64, proc: TrackProc) -> Result<(), AnalyzeError>;
    fn UnTrack(&self, id: u64);
    fn KillSysProcess(&self, id: u64);
}

/// 统计 Handle 上分配/释放自动分析进程 ID 的接口。
pub trait StatsHandleOps: AutoAnalyzeProcIdGenerator + Send + Sync {
    fn AutoAnalyzeProcID(&self) -> u64 {
        self.auto_analyze_proc_id()
    }
    fn ReleaseAutoAnalyzeProcID(&self, id: u64) {
        self.release_auto_analyze_proc_id(id);
    }
}

/// 用 sqlescape 转义 `%n` 等占位符；失败时回退到原 SQL。
fn escape_analyze_sql(sql: &str, params: &[&str]) -> String {
    let args: Vec<SqlArg> = params
        .iter()
        .map(|p| SqlArg::String((*p).to_owned()))
        .collect();
    EscapeSQL(sql, &args).unwrap_or_else(|_| sql.to_owned())
}

/// Escapes SQL and optionally emits the legacy stats-version rewrite warning.
/// 转义 SQL；若 need_warn 则记录 legacy 统计版本被改写为 v2 的告警。
pub fn execOptionForAnalyzeVersion(
    stats_ver: i32,
    need_warn: bool,
    sql: &str,
    params: &[&str],
) -> String {
    assert_eq!(
        stats_ver, Version2,
        "auto analyze should use stats version 2"
    );
    let escaped = escape_analyze_sql(sql, params);
    if need_warn {
        StatsLogger()
            .with_fields([LogField::String("sql".into(), escaped.clone())])
            .log(
                LogLevel::Warn,
                "auto analyze rewrites legacy statistics version 1 to version 2",
                [],
            );
    }
    escaped
}

/// AutoAnalyze executes the auto analyze task.
/// 执行自动分析任务；成功返回 true，失败记错误日志并返回 false。
pub fn AutoAnalyze(
    sctx: &dyn AnalyzeExecutor,
    handle: &dyn StatsHandleOps,
    tracker: &dyn SysProcTracker,
    stats_ver: i32,
    need_warn: bool,
    sql: &str,
    params: &[&str],
) -> bool {
    let start = Instant::now();
    let result = RunAnalyzeStmt(sctx, handle, tracker, stats_ver, need_warn, sql, params);
    let _duration = start.elapsed();
    match result {
        Ok(()) => true,
        Err(err) => {
            let escaped = escape_analyze_sql(sql, params);
            StatsLogger()
                .with_fields([
                    LogField::String("sql".into(), escaped),
                    LogField::String("error".into(), err.to_string()),
                ])
                .log(LogLevel::Error, "auto analyze failed", []);
            false
        }
    }
}

/// Guard that releases the auto-analyze process id and untracks it, matching
/// Go's `defer statsHandle.ReleaseAutoAnalyzeProcID` + tracker UnTrack.
/// 析构时释放进程 ID 并 UnTrack，对应 Go 的 defer Release + UnTrack。
struct ProcIdGuard<'a> {
    handle: &'a dyn StatsHandleOps,
    tracker: &'a dyn SysProcTracker,
    id: u64,
}

impl Drop for ProcIdGuard<'_> {
    fn drop(&mut self) {
        GLOBAL_AUTO_ANALYZE_PROCESS_LIST.untrack(self.id);
        self.tracker.UnTrack(self.id);
        self.handle.ReleaseAutoAnalyzeProcID(self.id);
    }
}

/// RunAnalyzeStmt executes the analyze statement with sys-proc tracking.
/// 转义 SQL、登记进程、执行 ANALYZE；`ProcIdGuard` 保证退出时清理。
pub fn RunAnalyzeStmt(
    sctx: &dyn AnalyzeExecutor,
    handle: &dyn StatsHandleOps,
    tracker: &dyn SysProcTracker,
    stats_ver: i32,
    need_warn: bool,
    sql: &str,
    params: &[&str],
) -> Result<(), AnalyzeError> {
    let escaped = execOptionForAnalyzeVersion(stats_ver, need_warn, sql, params);
    let auto_analyze_proc_id = handle.AutoAnalyzeProcID();
    // Go records the ID in GlobalAutoAnalyzeProcessList before the Track callback.
    // Install the guard before the fallible callback: Go's deferred process-ID
    // release also covers an error returned while installing execution options.
    // 先写入全局进程列表并建立清理 guard，再 Track（与 Go 的 defer 范围一致）。
    GLOBAL_AUTO_ANALYZE_PROCESS_LIST.track(auto_analyze_proc_id);
    let _guard = ProcIdGuard {
        handle,
        tracker,
        id: auto_analyze_proc_id,
    };
    tracker.Track(
        auto_analyze_proc_id,
        TrackProc {
            database: String::new(),
            table: String::new(),
            statement: escaped.clone(),
        },
    )?;
    sctx.Execute(&escaped)
}

/// GetAutoAnalyzeParameters reads the three auto-analyze globals from the
/// supplied parameter source (Go queries `mysql.global_variables`).
/// 从参数表读取三个自动分析相关全局变量（对应 Go 查 `mysql.global_variables`）。
pub fn GetAutoAnalyzeParameters(vars: &HashMap<String, String>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for key in [
        TIDB_AUTO_ANALYZE_RATIO,
        TIDB_AUTO_ANALYZE_START_TIME,
        TIDB_AUTO_ANALYZE_END_TIME,
    ] {
        if let Some(value) = vars.get(key) {
            out.insert(key.to_owned(), value.clone());
        }
    }
    out
}

/// 解析修改比率字符串；非法或负数时回退默认值。
pub fn ParseAutoAnalyzeRatio(ratio: &str) -> f64 {
    ratio
        .parse::<f64>()
        .map(|v| if v.is_nan() { v } else { v.max(0.0) })
        .unwrap_or(DEF_AUTO_ANALYZE_RATIO)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 带时区偏移的分析时间点（时、分、偏移分钟）。
pub struct AnalysisTime {
    pub Hour: u8,
    pub Minute: u8,
    pub OffsetMinutes: i16,
}

/// 解析 `HH:MM ±HHMM` 格式的分析时间。
fn parse_time(value: &str) -> Result<AnalysisTime, AnalyzeError> {
    if value.len() != 11 || value.as_bytes()[5] != b' ' {
        return Err(AnalyzeError("invalid analysis time".into()));
    }
    let mut parts = value.split_whitespace();
    let hm = parts
        .next()
        .ok_or_else(|| AnalyzeError("invalid analysis time".into()))?;
    let zone = parts
        .next()
        .ok_or_else(|| AnalyzeError("missing timezone".into()))?;
    if parts.next().is_some() {
        return Err(AnalyzeError("invalid analysis time".into()));
    }
    let (h, m) = hm
        .split_once(':')
        .ok_or_else(|| AnalyzeError("invalid analysis time".into()))?;
    if h.len() != 2 || m.len() != 2 {
        return Err(AnalyzeError("invalid analysis time".into()));
    }
    let hour: u8 = h.parse().map_err(|_| AnalyzeError("invalid hour".into()))?;
    let minute: u8 = m
        .parse()
        .map_err(|_| AnalyzeError("invalid minute".into()))?;
    if hour > 23 || minute > 59 || zone.len() != 5 {
        return Err(AnalyzeError("invalid analysis time".into()));
    }
    let sign = match &zone[..1] {
        "+" => 1,
        "-" => -1,
        _ => return Err(AnalyzeError("invalid timezone".into())),
    };
    let zh: i16 = zone[1..3]
        .parse()
        .map_err(|_| AnalyzeError("invalid timezone".into()))?;
    let zm: i16 = zone[3..5]
        .parse()
        .map_err(|_| AnalyzeError("invalid timezone".into()))?;
    if zh > 23 || zm > 59 {
        return Err(AnalyzeError("invalid timezone".into()));
    }
    Ok(AnalysisTime {
        Hour: hour,
        Minute: minute,
        OffsetMinutes: sign * (zh * 60 + zm),
    })
}

/// ParseAutoAnalysisWindow parses the time window for auto analysis (UTC).
/// 解析自动分析时间窗口（空串用默认起止），结果按 UTC 语义比较。
pub fn ParseAutoAnalysisWindow(
    start: &str,
    end: &str,
) -> Result<(AnalysisTime, AnalysisTime), AnalyzeError> {
    parse_time(if start.is_empty() {
        DEF_AUTO_ANALYZE_START_TIME
    } else {
        start
    })
    .and_then(|s| {
        parse_time(if end.is_empty() {
            DEF_AUTO_ANALYZE_END_TIME
        } else {
            end
        })
        .map(|e| (s, e))
    })
}

/// 将 AnalysisTime 转为 UTC DateTime，供 WithinDayTimePeriod 比较。
fn analysis_time_as_utc(t: AnalysisTime) -> chrono::DateTime<Utc> {
    // Go ParseAutoAnalysisWindow parses with time.ParseInLocation(..., time.UTC)
    // using format "15:04 -0700", so the offset in the string is applied to the
    // instant; WithinDayTimePeriod then compares UTC hour/minute.
    let offset = FixedOffset::east_opt(t.OffsetMinutes as i32 * 60)
        .unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    offset
        .with_ymd_and_hms(1970, 1, 1, t.Hour as u32, t.Minute as u32, 0)
        .single()
        .map(|local| local.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap())
}

/// CheckAutoAnalyzeWindow mirrors Go `autoanalyze.CheckAutoAnalyzeWindow`:
/// parse the configured window and report whether `now` falls inside it.
/// 检查当前时间是否落在配置窗口内；解析失败返回关闭窗口。
pub fn CheckAutoAnalyzeWindow(parameters: &HashMap<String, String>) -> (String, String, bool) {
    let start_raw = parameters
        .get(TIDB_AUTO_ANALYZE_START_TIME)
        .map(String::as_str)
        .unwrap_or("");
    let end_raw = parameters
        .get(TIDB_AUTO_ANALYZE_END_TIME)
        .map(String::as_str)
        .unwrap_or("");
    let (start, end) = match ParseAutoAnalysisWindow(start_raw, end_raw) {
        Ok(pair) => pair,
        Err(err) => {
            StatsLogger()
                .with_fields([LogField::String("error".into(), err.to_string())])
                .log(LogLevel::Error, "parse auto analyze period failed", []);
            return ("00:00".to_owned(), "00:00".to_owned(), false);
        }
    };
    let start_str = format!("{:02}:{:02}", start.Hour, start.Minute);
    let end_str = format!("{:02}:{:02}", end.Hour, end.Minute);
    let start_t = analysis_time_as_utc(start);
    let end_t = analysis_time_as_utc(end);
    let now = Utc::now();
    let ok = WithinDayTimePeriod(start_t, end_t, now);
    (start_str, end_str, ok)
}

/// Kills every tracked auto-analyze process when the current time is outside
/// the configured window — Go `Domain.CheckAutoAnalyzeWindows`.
/// 窗口外时杀掉所有已跟踪的自动分析进程（对应 Go Domain.CheckAutoAnalyzeWindows）。
pub fn KillAutoAnalyzeOutsideWindow(
    parameters: &HashMap<String, String>,
    tracker: &dyn SysProcTracker,
) -> (String, String, bool) {
    let (start, end, ok) = CheckAutoAnalyzeWindow(parameters);
    if !ok {
        // 窗口外：逐个告警并 KillSysProcess，同时从全局列表移除。
        for id in GLOBAL_AUTO_ANALYZE_PROCESS_LIST.all() {
            StatsLogger()
                .with_fields([
                    LogField::U64("processID".into(), id),
                    LogField::String("start".into(), start.clone()),
                    LogField::String("end".into(), end.clone()),
                ])
                .log(
                    LogLevel::Warn,
                    "Kill auto analyze process because it exceeded the window",
                    [],
                );
            tracker.KillSysProcess(id);
            GLOBAL_AUTO_ANALYZE_PROCESS_LIST.untrack(id);
        }
    }
    (start, end, ok)
}

/// Test helper: a process-id generator that allocates sequential ids.
/// 测试用进程 ID 生成器：顺序分配 ID。
pub fn new_test_handle_ops() -> Arc<dyn StatsHandleOps> {
    use std::sync::atomic::{AtomicU64, Ordering};
    struct Gen {
        next: AtomicU64,
    }
    impl AutoAnalyzeProcIdGenerator for Gen {
        fn auto_analyze_proc_id(&self) -> u64 {
            self.next.fetch_add(1, Ordering::SeqCst)
        }
        fn release_auto_analyze_proc_id(&self, _id: u64) {}
    }
    impl StatsHandleOps for Gen {}
    Arc::new(Gen {
        next: AtomicU64::new(1),
    })
}
