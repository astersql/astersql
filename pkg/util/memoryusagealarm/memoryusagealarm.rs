// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 内存使用告警：周期性检测内存逼近 OOM 风险并落盘诊断记录。
//
// 对应 Go `pkg/util/memoryusagealarm`。OOM（Out Of Memory）指进程因内存不足被杀。
// 当用量超过告警比例，或短时间内增长过快时，写出 running_sql、heap、goroutine
// 等记录，并按保留份数清理旧目录。全局内存仲裁开启时跳过本路径。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use backtrace::Backtrace;
use chrono::{DateTime, Local, SecondsFormat};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use memory_stats::memory_stats;
use task_logutil::log::{BgLogger, LogField};

/// Provides the values used by the memory-usage alarm.
/// 告警读取的配置接口：比例、保留份数、日志目录、组件名。
pub trait ConfigProvider: Send + Sync {
    /// 内存用量相对上限的告警比例（0~1，越界则禁用告警）。
    fn GetMemoryUsageAlarmRatio(&self) -> f64;
    /// 保留的历史记录目录个数。
    fn GetMemoryUsageAlarmKeepRecordNum(&self) -> i64;
    /// 日志/记录根目录。
    fn GetLogDir(&self) -> String;
    /// 组件名（用于告警日志字段）。
    fn GetComponentName(&self) -> String;
}

/// Reads the TiDB runtime variables used by the Go implementation.
/// 从 TiDB 运行时变量与配置读取告警参数的默认实现。
#[derive(Clone, Copy, Debug, Default)]
pub struct TiDBConfigProvider;

impl ConfigProvider for TiDBConfigProvider {
    fn GetMemoryUsageAlarmRatio(&self) -> f64 {
        task_vardef::MemoryUsageAlarmRatio.Load()
    }

    fn GetMemoryUsageAlarmKeepRecordNum(&self) -> i64 {
        task_vardef::MemoryUsageAlarmKeepRecordNum.Load()
    }

    fn GetLogDir(&self) -> String {
        let filename = task_config::get_global_config().log.file.filename.clone();
        Path::new(&filename)
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_string_lossy()
            .into_owned()
    }

    fn GetComponentName(&self) -> String {
        "tidb-server".to_owned()
    }
}

/// Values produced by TiDB's `GenLogFields` for a process snapshot.
/// 进程快照日志字段的取值类型（对齐 Go zap 字段）。
#[derive(Clone, Debug, PartialEq)]
pub enum ProcessLogValue {
    String(String),
    Unsigned(u64),
    Signed(i64),
    Bool(bool),
}

/// A process log field, kept ordered to match zap field rendering in Go.
/// 有序进程日志字段，键值对渲染顺序与 Go 一致。
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessLogField {
    pub key: String,
    pub value: ProcessLogValue,
}

/// OOM-related session variables captured with a process snapshot.
/// 随进程快照一并捕获的 OOM 相关会话变量。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OOMAlarmVariablesInfo {
    /// 单查询内存配额（tidb_mem_quota_query）。
    pub SessionMemQuotaQuery: i64,
    /// ANALYZE 版本。
    pub SessionAnalyzeVersion: i32,
    /// 是否启用 rate limit action。
    pub SessionEnabledRateLimitAction: bool,
}

/// The subset of `sessmgr.ProcessInfo` required by this package.
///
/// Keeping a snapshot boundary makes the alarm independently testable while
/// allowing the session package to adapt its full ProcessInfo without a
/// reverse dependency on the alarm package.
/// 告警所需的会话进程信息子集，避免反向依赖完整 session 包。
#[derive(Clone, Debug)]
pub struct ProcessInfo {
    /// 语句开始时间。
    pub time: SystemTime,
    /// SQL 文本或摘要。
    pub info: String,
    /// 该会话已消耗的最大内存字节数。
    pub max_consumed: i64,
    /// 事务开始时间戳（txn start ts，MVCC 版本时间戳）。
    pub txn_start_ts: u64,
    /// 会话别名。
    pub session_alias: String,
    /// 影响行数。
    pub affected_rows: u64,
    /// OOM 相关会话变量快照。
    pub oom_alarm_variables_info: OOMAlarmVariablesInfo,
    /// 简要执行计划行（binary plan 表格行）。
    pub brief_binary_plan_rows: Vec<Vec<String>>,
    /// 预生成的日志字段；空则用默认字段。
    pub log_fields: Vec<ProcessLogField>,
}

impl Default for ProcessInfo {
    fn default() -> Self {
        Self {
            time: UNIX_EPOCH,
            info: String::new(),
            max_consumed: 0,
            txn_start_ts: 0,
            session_alias: String::new(),
            affected_rows: 0,
            oom_alarm_variables_info: OOMAlarmVariablesInfo::default(),
            brief_binary_plan_rows: Vec::new(),
            log_fields: Vec::new(),
        }
    }
}

/// Supplies active process snapshots to the recorder.
/// 向记录器提供当前活跃进程列表。
pub trait SessionManager: Send + Sync {
    /// 返回当前进程列表快照。
    fn ShowProcessList(&self) -> Vec<Arc<ProcessInfo>>;
}

/// Handler for the server's periodic memory-usage checks.
/// 服务端周期性内存检查的句柄：持有退出通道、会话管理器与配置。
pub struct Handle {
    /// 退出信号接收端；收到或断开则结束 Run 循环。
    pub exitCh: Receiver<()>,
    sm: RwLock<Option<Arc<dyn SessionManager>>>,
    pub configProvider: Arc<dyn ConfigProvider>,
}

/// 构造告警句柄。
pub fn NewMemoryUsageAlarmHandle(
    exitCh: Receiver<()>,
    provider: Arc<dyn ConfigProvider>,
) -> Box<Handle> {
    Box::new(Handle {
        exitCh,
        sm: RwLock::new(None),
        configProvider: provider,
    })
}

impl Handle {
    /// 注入会话管理器，供告警时拉取 running SQL。
    pub fn SetSessionManager(&mut self, sm: Arc<dyn SessionManager>) -> &mut Handle {
        *self.sm.write().expect("session manager lock poisoned") = Some(sm);
        self
    }

    /// Runs the same 100ms ticker loop as Go until the exit channel fires.
    /// 与 Go 相同：每 100ms 超时检查一次内存，直到 exitCh 触发。
    pub fn Run(&self) {
        let mut record = memoryUsageAlarm::new(Arc::clone(&self.configProvider));
        loop {
            match self.exitCh.recv_timeout(Duration::from_millis(100)) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    let sm = self
                        .sm
                        .read()
                        .expect("session manager lock poisoned")
                        .clone();
                    record.alarm4ExcessiveMemUsage(sm.as_deref());
                }
            }
        }
    }
}

/// State retained between memory-usage checks.
/// 两次检查之间保留的告警状态（上次记录时间、用量、配置缓存等）。
pub struct memoryUsageAlarm {
    /// 上次触发记录的时间。
    pub lastCheckTime: SystemTime,
    /// 上次刷新配置变量的时间（至少间隔 60s）。
    pub lastUpdateVariableTime: SystemTime,
    /// 最近一次初始化/记录错误信息。
    pub err: Option<String>,
    pub configProvider: Arc<dyn ConfigProvider>,
    /// oom_record 根目录。
    pub baseRecordDir: String,
    /// 已保留的记录目录路径（按时间顺序）。
    pub lastRecordDirName: Vec<String>,
    /// 上次记录时的内存用量。
    pub lastRecordMemUsed: u64,
    pub memoryUsageAlarmRatio: f64,
    pub memoryUsageAlarmKeepRecordNum: i64,
    /// 内存上限：优先 ServerMemoryLimit，否则系统总内存。
    pub serverMemoryLimit: u64,
    /// 是否显式设置了 ServerMemoryLimit。
    pub isServerMemoryLimitSet: bool,
    /// 是否已完成记录目录初始化。
    pub initialized: bool,
}

impl memoryUsageAlarm {
    /// 以给定配置提供者构造未初始化的告警状态。
    pub fn new(configProvider: Arc<dyn ConfigProvider>) -> Self {
        Self {
            lastCheckTime: UNIX_EPOCH,
            lastUpdateVariableTime: UNIX_EPOCH,
            err: None,
            configProvider,
            baseRecordDir: String::new(),
            lastRecordDirName: Vec::new(),
            lastRecordMemUsed: 0,
            memoryUsageAlarmRatio: 0.0,
            memoryUsageAlarmKeepRecordNum: 0,
            serverMemoryLimit: 0,
            isServerMemoryLimitSet: false,
            initialized: false,
        }
    }

    /// 距上次刷新超过 60s 时重新读取告警比例、保留数与内存上限。
    pub fn updateVariable(&mut self) {
        if elapsed_since(self.lastUpdateVariableTime) < Duration::from_secs(60) {
            return;
        }

        self.memoryUsageAlarmRatio = self.configProvider.GetMemoryUsageAlarmRatio();
        self.memoryUsageAlarmKeepRecordNum = self.configProvider.GetMemoryUsageAlarmKeepRecordNum();
        self.serverMemoryLimit = task_memory::tracker::ServerMemoryLimit.Load();
        if self.serverMemoryLimit != 0 {
            self.isServerMemoryLimitSet = true;
        } else {
            // 未设置 ServerMemoryLimit 时回退到系统总内存。
            let probe = *task_memory::meminfo::MemTotal
                .read()
                .expect("MemTotal lock poisoned");
            match probe() {
                Ok(total) => {
                    self.serverMemoryLimit = total;
                    self.isServerMemoryLimitSet = false;
                }
                Err(error) => {
                    let message = error.to_string();
                    log_error("get system total memory fail", &message);
                    self.err = Some(message);
                    return;
                }
            }
        }
        self.lastUpdateVariableTime = SystemTime::now();
    }

    /// 创建 oom_record 目录并扫描已有 record* 子目录。
    pub fn initMemoryUsageAlarmRecord(&mut self) {
        self.lastCheckTime = UNIX_EPOCH;
        self.lastUpdateVariableTime = UNIX_EPOCH;
        self.err = None;
        self.updateVariable();

        self.baseRecordDir = Path::new(&self.configProvider.GetLogDir())
            .join("oom_record")
            .to_string_lossy()
            .into_owned();
        match task_disk::CheckAndCreateDir(&self.baseRecordDir) {
            Ok(()) => self.err = None,
            Err(error) => {
                self.err = Some(error.to_string());
                return;
            }
        }

        let entries = match fs::read_dir(&self.baseRecordDir) {
            Ok(entries) => entries,
            Err(error) => {
                self.err = Some(error.to_string());
                return;
            }
        };
        let mut record_dirs = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.err = Some(error.to_string());
                    return;
                }
            };
            record_dirs.push(entry.path());
        }
        // Go's os.ReadDir returns entries sorted by filename.
        record_dirs.sort();
        for record_dir in record_dirs {
            if record_dir
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains("record"))
            {
                self.lastRecordDirName
                    .push(record_dir.to_string_lossy().into_owned());
            }
        }
        self.initialized = true;
    }

    /// Checks memory usage and performs the same record/retention sequence as Go.
    /// 检测内存并在需要时落盘记录、清理超额历史目录（与 Go 流程一致）。
    pub fn alarm4ExcessiveMemUsage(&mut self, sm: Option<&dyn SessionManager>) {
        // 全局内存仲裁开启时由仲裁器负责，本告警路径直接跳过。
        if task_memory::global_arbitrator::UsingGlobalMemArbitration() {
            return;
        }
        if !self.initialized {
            self.initMemoryUsageAlarmRecord();
            if self.err.is_some() {
                return;
            }
        } else {
            self.updateVariable();
        }
        if self.memoryUsageAlarmRatio <= 0.0 || self.memoryUsageAlarmRatio >= 1.0 {
            return;
        }

        let instance_stats = task_memory::memstats::ReadMemStats();
        // 有 ServerMemoryLimit 时用进程堆分配；否则用系统已用内存。
        let memory_usage = if self.isServerMemoryLimitSet {
            instance_stats.heap_alloc
        } else {
            let probe = *task_memory::meminfo::MemUsed
                .read()
                .expect("MemUsed lock poisoned");
            match probe() {
                Ok(used) => used,
                Err(error) => {
                    let message = error.to_string();
                    log_error("get system memory usage fail", &message);
                    self.err = Some(message);
                    return;
                }
            }
        };

        let (need_record, reason) = self.needRecord(memory_usage);
        if need_record {
            self.lastCheckTime = SystemTime::now();
            self.lastRecordMemUsed = memory_usage;
            self.doRecord(memory_usage, instance_stats.heap_alloc, sm, reason);
            self.tryRemoveRedundantRecords();
        }
    }

    /// 判断是否需要落盘：超告警比例且（距上次 >60s 或增长超过上限 10%）。
    pub fn needRecord(&self, memoryUsage: u64) -> (bool, AlarmReason) {
        if memoryUsage as f64 <= self.serverMemoryLimit as f64 * self.memoryUsageAlarmRatio {
            return (false, AlarmReason::NoReason);
        }
        let interval = elapsed_since(self.lastCheckTime);
        // Go converts both uint64 operands to int64 before subtracting.
        let mem_diff = (memoryUsage as i64).wrapping_sub(self.lastRecordMemUsed as i64);
        if interval > Duration::from_secs(60) {
            return (true, AlarmReason::ExceedAlarmRatio);
        }
        if mem_diff as f64 > 0.1 * self.serverMemoryLimit as f64 {
            return (true, AlarmReason::GrowTooFast);
        }
        (false, AlarmReason::NoReason)
    }

    /// 打告警日志，创建带时间戳的 record 目录，写入 SQL 与 profile。
    pub fn doRecord(
        &mut self,
        memUsage: u64,
        instanceMemoryUsage: u64,
        sm: Option<&dyn SessionManager>,
        alarmReason: AlarmReason,
    ) {
        let component_name = self.configProvider.GetComponentName();
        let mut fields = vec![LogField::Bool(
            format!("is {component_name}_memory_limit set"),
            self.isServerMemoryLimitSet,
        )];
        if self.isServerMemoryLimitSet {
            fields.push(LogField::U64(
                format!("{component_name}_memory_limit"),
                self.serverMemoryLimit,
            ));
            fields.push(LogField::U64(
                format!("{component_name} memory usage"),
                memUsage,
            ));
        } else {
            fields.push(LogField::U64(
                "system memory total".to_owned(),
                self.serverMemoryLimit,
            ));
            fields.push(LogField::U64("system memory usage".to_owned(), memUsage));
            fields.push(LogField::U64(
                format!("{component_name} memory usage"),
                instanceMemoryUsage,
            ));
        }
        fields.push(LogField::String(
            "memory-usage-alarm-ratio".to_owned(),
            self.memoryUsageAlarmRatio.to_string(),
        ));
        fields.push(LogField::String(
            "record path".to_owned(),
            self.baseRecordDir.clone(),
        ));
        BgLogger().with_fields(fields).warn(format!(
            "{component_name} has the risk of OOM because of {}. Running profiles will be recorded in record path",
            alarmReason.String()
        ));

        let timestamp: DateTime<Local> = self.lastCheckTime.into();
        let record_dir = Path::new(&self.baseRecordDir).join(format!(
            "record{}",
            timestamp.to_rfc3339_opts(SecondsFormat::Secs, true)
        ));
        if let Err(error) = task_disk::CheckAndCreateDir(&record_dir) {
            self.err = Some(error.to_string());
            return;
        }
        let record_dir = record_dir.to_string_lossy().into_owned();
        self.lastRecordDirName.push(record_dir.clone());

        if let Some(sm) = sm
            && let Err(error) = self.recordSQL(sm, &record_dir)
        {
            self.err = Some(error.to_string());
            return;
        }
        if let Err(error) = self.recordProfile(&record_dir) {
            self.err = Some(error.to_string());
        }
    }

    /// 删除超出保留份数的最旧记录目录。
    pub fn tryRemoveRedundantRecords(&mut self) {
        while self.lastRecordDirName.len() as i64 > self.memoryUsageAlarmKeepRecordNum {
            if self.lastRecordDirName.is_empty() {
                break;
            }
            let old = self.lastRecordDirName.remove(0);
            if let Err(error) = fs::remove_dir_all(old) {
                log_error("remove temp files failed", &error.to_string());
            }
        }
    }

    /// 向文件写入按内存与按耗时各 Top10 的 SQL 诊断信息。
    pub fn printTop10SqlInfo(&self, pinfo: &mut [Arc<ProcessInfo>], file: &mut File) {
        write_best_effort(
            file,
            "The 10 SQLs with the most memory usage for OOM analysis\n",
            "write top 10 memory sql info fail",
        );
        let memory = self.getTop10SqlInfoByMemoryUsage(pinfo);
        write_best_effort(file, &memory, "write top 10 memory sql info fail");
        write_best_effort(
            file,
            "The 10 SQLs with the most time usage for OOM analysis\n",
            "write top 10 time cost sql info fail",
        );
        let cost = self.getTop10SqlInfoByCostTime(pinfo);
        write_best_effort(file, &cost, "write top 10 time cost sql info fail");
    }

    /// 按给定比较器排序后格式化最多 10 条 SQL 及 OOM 相关变量。
    fn getTop10SqlInfo<F>(&self, pinfo: &mut [Arc<ProcessInfo>], mut compare: F) -> String
    where
        F: FnMut(&Arc<ProcessInfo>, &Arc<ProcessInfo>) -> Ordering,
    {
        pinfo.sort_unstable_by(|left, right| compare(left, right));
        let mut output = String::new();
        let oom_action = task_vardef::OOMAction.Load();
        let server_memory_limit = task_memory::tracker::ServerMemoryLimit.Load();

        for (index, info) in pinfo.iter().take(10).enumerate() {
            output.push_str(&format!("SQL {index}: \n"));
            // 无预生成字段时用默认 cost_time/sql 等字段。
            let fields = if info.log_fields.is_empty() {
                default_process_fields(duration_between(self.lastCheckTime, info.time), info)
            } else {
                info.log_fields.clone()
            };
            for field in fields {
                append_process_field(&mut output, field);
            }
            append_process_field(
                &mut output,
                ProcessLogField {
                    key: "tidb_mem_oom_action".to_owned(),
                    value: ProcessLogValue::String(oom_action.clone()),
                },
            );
            append_process_field(
                &mut output,
                ProcessLogField {
                    key: "tidb_server_memory_limit".to_owned(),
                    value: ProcessLogValue::Unsigned(server_memory_limit),
                },
            );
            append_process_field(
                &mut output,
                ProcessLogField {
                    key: "tidb_mem_quota_query".to_owned(),
                    value: ProcessLogValue::Signed(
                        info.oom_alarm_variables_info.SessionMemQuotaQuery,
                    ),
                },
            );
            append_process_field(
                &mut output,
                ProcessLogField {
                    key: "tidb_analyze_version".to_owned(),
                    value: ProcessLogValue::Signed(
                        info.oom_alarm_variables_info.SessionAnalyzeVersion as i64,
                    ),
                },
            );
            append_process_field(
                &mut output,
                ProcessLogField {
                    key: "tidb_enable_rate_limit_action".to_owned(),
                    value: ProcessLogValue::Bool(
                        info.oom_alarm_variables_info.SessionEnabledRateLimitAction,
                    ),
                },
            );
            append_process_field(
                &mut output,
                ProcessLogField {
                    key: "current_analyze_plan".to_owned(),
                    value: ProcessLogValue::String(getPlanString(info)),
                },
            );
        }
        output.push('\n');
        output
    }

    /// 按 max_consumed 降序取 Top10。
    pub fn getTop10SqlInfoByMemoryUsage(&self, pinfo: &mut [Arc<ProcessInfo>]) -> String {
        self.getTop10SqlInfo(pinfo, |left, right| {
            right.max_consumed.cmp(&left.max_consumed)
        })
    }

    /// 按开始时间升序（更早启动视为更耗时）取 Top10。
    pub fn getTop10SqlInfoByCostTime(&self, pinfo: &mut [Arc<ProcessInfo>]) -> String {
        self.getTop10SqlInfo(pinfo, |left, right| left.time.cmp(&right.time))
    }

    /// 过滤非空 SQL 进程列表并写入 `running_sql` 文件。
    pub fn recordSQL(&self, sm: &dyn SessionManager, recordDir: &str) -> io::Result<()> {
        let mut processes = sm
            .ShowProcessList()
            .into_iter()
            .filter(|info| !info.info.is_empty())
            .collect::<Vec<_>>();
        let mut file = File::create(Path::new(recordDir).join("running_sql"))?;
        self.printTop10SqlInfo(&mut processes, &mut file);
        Ok(())
    }

    #[allow(clippy::single_element_loop)]
    /// 写入 heap 快照与 goroutine（线程回溯）profile。
    pub fn recordProfile(&self, recordDir: &str) -> io::Result<()> {
        for profile in [item {
            Name: "heap".to_owned(),
            Debug: 0,
        }] {
            write(profile, recordDir)?;
        }
        recordGoroutineProfile(recordDir)
    }
}

/// 触发告警的原因分类。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlarmReason {
    /// 相对上次记录增长过快（超过上限的 10%）。
    GrowTooFast,
    /// 超过告警比例且距上次记录超过 60s。
    ExceedAlarmRatio,
    /// 无需记录。
    NoReason,
}

impl AlarmReason {
    /// 返回写入告警日志的英文原因字符串。
    pub fn String(self) -> &'static str {
        match self {
            Self::GrowTooFast => "memory usage grows too fast",
            Self::ExceedAlarmRatio => "memory usage exceeds alarm ratio",
            Self::NoReason => "no reason",
        }
    }
}

/// 将 brief binary plan 行格式化为简易表格字符串。
pub fn getPlanString(info: &ProcessInfo) -> String {
    let mut output = "|id|estRows|task|access object|operator info|".to_owned();
    for row in &info.brief_binary_plan_rows {
        output.push_str("\n|");
        for column in row {
            output.push_str(column);
            output.push('|');
        }
    }
    output
}

/// pprof 风格 profile 项：名称与 debug 级别。
pub struct item {
    pub Name: String,
    pub Debug: i32,
}

/// Writes a native Rust thread backtrace to the Go-compatible profile filename.
/// 将当前线程回溯写入与 Go 兼容的 `goroutine` 文件名。
pub fn recordGoroutineProfile(recordDir: &str) -> io::Result<()> {
    let mut file = File::create(Path::new(recordDir).join("goroutine"))?;
    writeln!(file, "thread {:?} [running]:", std::thread::current().id())?;
    writeln!(file, "{:#?}", Backtrace::new())?;
    Ok(())
}

/// Writes a native heap snapshot. `memory-stats` supplies allocator/process
/// values without inventing a replacement for Go's runtime/pprof package.
/// 写入堆快照：优先 memory-stats，失败则 ForceReadMemStats。
pub fn write(profile: item, recordDir: &str) -> io::Result<()> {
    let mut file = File::create(Path::new(recordDir).join(&profile.Name))?;
    if profile.Name == "heap" {
        if let Some(stats) = memory_stats() {
            writeln!(file, "heap profile: debug={}", profile.Debug)?;
            writeln!(file, "physical_mem: {}", stats.physical_mem)?;
            writeln!(file, "virtual_mem: {}", stats.virtual_mem)?;
        } else {
            let stats = task_memory::memstats::ForceReadMemStats();
            writeln!(file, "heap profile: debug={}", profile.Debug)?;
            writeln!(file, "heap_alloc: {}", stats.heap_alloc)?;
            writeln!(file, "heap_inuse: {}", stats.heap_inuse)?;
        }
    }
    Ok(())
}

/// 距给定时间点的经过时长；时钟回拨时返回 0。
fn elapsed_since(time: SystemTime) -> Duration {
    SystemTime::now()
        .duration_since(time)
        .unwrap_or(Duration::ZERO)
}

/// 计算 later - earlier；失败则 0。
fn duration_between(later: SystemTime, earlier: SystemTime) -> Duration {
    later.duration_since(earlier).unwrap_or(Duration::ZERO)
}

/// 构造默认进程日志字段（耗时、txn_start_ts、mem_max、sql 等）。
fn default_process_fields(cost: Duration, info: &ProcessInfo) -> Vec<ProcessLogField> {
    vec![
        ProcessLogField {
            key: "cost_time".to_owned(),
            value: ProcessLogValue::String(format!("{}s", cost.as_secs_f64())),
        },
        ProcessLogField {
            key: "txn_start_ts".to_owned(),
            value: ProcessLogValue::Unsigned(info.txn_start_ts),
        },
        ProcessLogField {
            key: "mem_max".to_owned(),
            value: ProcessLogValue::String(format!(
                "{} Bytes ({})",
                info.max_consumed,
                task_memory::tracker::FormatBytes(info.max_consumed)
            )),
        },
        ProcessLogField {
            key: "sql".to_owned(),
            value: ProcessLogValue::String(info.info.clone()),
        },
        ProcessLogField {
            key: "session_alias".to_owned(),
            value: ProcessLogValue::String(info.session_alias.clone()),
        },
        ProcessLogField {
            key: "affected rows".to_owned(),
            value: ProcessLogValue::Unsigned(info.affected_rows),
        },
    ]
}

/// 将进程日志字段追加到输出文本。
fn append_process_field(output: &mut String, field: ProcessLogField) {
    output.push_str(&field.key);
    output.push_str(": ");
    match field.value {
        ProcessLogValue::String(value) => output.push_str(&value),
        ProcessLogValue::Unsigned(value) => output.push_str(&value.to_string()),
        ProcessLogValue::Signed(value) => output.push_str(&value.to_string()),
        ProcessLogValue::Bool(value) => output.push_str(if value { "true" } else { "false" }),
    }
    output.push('\n');
}

/// 尽力写入文件，失败只记日志不向上抛。
fn write_best_effort(file: &mut File, contents: &str, message: &str) {
    if let Err(error) = file.write_all(contents.as_bytes()) {
        log_error(message, &error.to_string());
    }
}

/// 带 error 字段的后台错误日志。
fn log_error(message: &str, error: &str) {
    BgLogger()
        .with_fields([LogField::String("error".to_owned(), error.to_owned())])
        .error(message);
}

#[allow(dead_code)]
/// 拼接记录路径（测试/辅助）。
fn record_path(base: &str, child: &str) -> PathBuf {
    Path::new(base).join(child)
}
