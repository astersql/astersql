// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 内存使用告警（memoryusagealarm）Go 对齐单元测试。
//
// 覆盖 needRecord 阈值/冷却、Top10 SQL 输出、配置变量 60s 刷新窗口，
// 以及 goroutine profile 写入。OOM 指内存耗尽风险告警场景。

use std::fs;
use std::io;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::memoryusagealarm::{
    AlarmReason, ConfigProvider, ProcessInfo, memoryUsageAlarm, recordGoroutineProfile,
};

/// Go's `MockConfigProvider`, with locks providing the pointer-style mutation
/// used by `TestUpdateVariables` after the recorder has retained the provider.
/// 可在运行中修改比例与保留份数的测试配置提供者。
#[derive(Debug)]
pub struct MockConfigProvider {
    log_dir: String,
    component_name: String,
    ratio: RwLock<f64>,
    keep_num: RwLock<i64>,
}

impl MockConfigProvider {
    fn new(log_dir: String, ratio: f64, keep_num: i64) -> Self {
        Self {
            log_dir,
            component_name: "test-component".to_owned(),
            ratio: RwLock::new(ratio),
            keep_num: RwLock::new(keep_num),
        }
    }

    fn set_ratio(&self, ratio: f64) {
        *self.ratio.write().expect("ratio lock poisoned") = ratio;
    }

    fn set_keep_num(&self, keep_num: i64) {
        *self.keep_num.write().expect("keep-num lock poisoned") = keep_num;
    }
}

impl ConfigProvider for MockConfigProvider {
    fn GetMemoryUsageAlarmRatio(&self) -> f64 {
        *self.ratio.read().expect("ratio lock poisoned")
    }

    fn GetMemoryUsageAlarmKeepRecordNum(&self) -> i64 {
        *self.keep_num.read().expect("keep-num lock poisoned")
    }

    fn GetLogDir(&self) -> String {
        if self.log_dir.is_empty() {
            std::env::temp_dir().to_string_lossy().into_owned()
        } else {
            self.log_dir.clone()
        }
    }

    fn GetComponentName(&self) -> String {
        if self.component_name.is_empty() {
            "test-component".to_owned()
        } else {
            self.component_name.clone()
        }
    }
}

/// 全局 ServerMemoryLimit / OOMAction 测试互斥锁，避免并行污染。
pub(crate) static GLOBAL_VARIABLE_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 进入作用域时设置内存上限与 OOMAction，退出时还原。
struct GlobalVariableGuard {
    memory_limit: u64,
    oom_action: String,
    mem_total: task_memory::meminfo::MemoryInfoProbe,
}

impl GlobalVariableGuard {
    /// 保存旧值并写入测试用全局变量。
    fn set(memory_limit: u64) -> Self {
        let old = Self {
            memory_limit: task_memory::tracker::ServerMemoryLimit.Load(),
            oom_action: task_vardef::OOMAction.Load(),
            mem_total: *task_memory::meminfo::MemTotal
                .read()
                .expect("MemTotal lock poisoned"),
        };
        task_memory::tracker::ServerMemoryLimit.Store(memory_limit);
        task_vardef::OOMAction.Store("CANCEL");
        old
    }
}

impl Drop for GlobalVariableGuard {
    fn drop(&mut self) {
        task_memory::tracker::ServerMemoryLimit.Store(self.memory_limit);
        task_vardef::OOMAction.Store(self.oom_action.clone());
        *task_memory::meminfo::MemTotal
            .write()
            .expect("MemTotal lock poisoned") = self.mem_total;
    }
}

fn failing_mem_total() -> task_memory::meminfo::MemoryInfoResult {
    Err(io::Error::other("injected MemTotal failure").into())
}

/// Go overwrites a MemTotal error with the following successful directory check,
/// so initialization still completes and subsequent alarm checks can refresh it.
#[test]
fn init_continues_after_mem_total_error_like_go() {
    let _lock = GLOBAL_VARIABLE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = GlobalVariableGuard::set(0);
    *task_memory::meminfo::MemTotal
        .write()
        .expect("MemTotal lock poisoned") = failing_mem_total;

    let temp = tempfile::tempdir().expect("create record temp dir");
    let provider = Arc::new(MockConfigProvider::new(
        temp.path().display().to_string(),
        0.7,
        5,
    ));
    let mut record = memoryUsageAlarm::new(provider);

    record.initMemoryUsageAlarmRecord();

    assert!(record.initialized);
    assert_eq!(record.err, None);
    assert!(temp.path().join("oom_record").is_dir());
}

/// 验证 needRecord：比例阈值、60s 冷却与 10% 增长加速触发。
#[test]
pub fn TestIfNeedDoRecord() {
    let _lock = GLOBAL_VARIABLE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = GlobalVariableGuard::set(1_000);
    let temp = tempfile::tempdir().expect("create record temp dir");
    let provider = Arc::new(MockConfigProvider::new(
        temp.path().display().to_string(),
        0.7,
        5,
    ));
    let mut record = memoryUsageAlarm::new(provider);
    record.initMemoryUsageAlarmRecord();
    assert!(record.initialized, "initialization error: {:?}", record.err);

    // mem usage ratio < 70% will not be recorded.
    let mut mem_used = 0.69 * record.serverMemoryLimit as f64;
    assert_eq!(
        record.needRecord(mem_used as u64),
        (false, AlarmReason::NoReason)
    );

    // The Go comment says "will not", but its assertion correctly expects a record.
    mem_used = 0.71 * record.serverMemoryLimit as f64;
    assert_eq!(
        record.needRecord(mem_used as u64),
        (true, AlarmReason::ExceedAlarmRatio)
    );
    record.lastCheckTime = SystemTime::now();
    record.lastRecordMemUsed = mem_used as u64;

    // check time - last record time < 60s will not be recorded.
    assert_eq!(
        record.needRecord(mem_used as u64),
        (false, AlarmReason::NoReason)
    );

    // check time - last record time > 60s will be recorded.
    record.lastCheckTime = record.lastCheckTime - Duration::from_secs(61);
    assert_eq!(
        record.needRecord(mem_used as u64),
        (true, AlarmReason::ExceedAlarmRatio)
    );
    record.lastCheckTime = SystemTime::now();
    record.lastRecordMemUsed = mem_used as u64;

    // mem usage ratio - last mem usage ratio < 10% will not be recorded.
    mem_used = 0.80 * record.serverMemoryLimit as f64;
    assert_eq!(
        record.needRecord(mem_used as u64),
        (false, AlarmReason::NoReason)
    );

    // Growth beyond 10% records immediately, even inside the cooldown window.
    mem_used = 0.82 * record.serverMemoryLimit as f64;
    assert_eq!(
        record.needRecord(mem_used as u64),
        (true, AlarmReason::GrowTooFast)
    );
}

/// Go's `time.Date(1970, 1, 0, ...).Unix()` is -86400.
/// 生成与 Go 测试对齐的时间点（相对 1970-01-00 偏移）。
pub fn genTime(sec: i64) -> SystemTime {
    let unix_seconds = sec - 86_400;
    if unix_seconds >= 0 {
        UNIX_EPOCH + Duration::from_secs(unix_seconds as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs((-unix_seconds) as u64)
    }
}

/// 由 (耗时秒, 内存, SQL) 行构造 mock ProcessInfo 列表。
fn genMockProcessInfoList(
    mem_consume_list: &[i64],
    start_time_list: &[SystemTime],
    size: usize,
) -> Vec<Arc<ProcessInfo>> {
    assert!(size <= mem_consume_list.len());
    assert!(size <= start_time_list.len());
    (0..size)
        .map(|index| {
            Arc::new(ProcessInfo {
                time: start_time_list[index],
                max_consumed: mem_consume_list[index],
                ..ProcessInfo::default()
            })
        })
        .collect()
}

/// Builds the same complete literal shape used by the Go assertions. Only the
/// table's order, cost, byte count, and formatted size vary between cases.
/// 期望的 Top SQL 文本片段构造辅助。
fn expectedTopSql(rows: &[(u64, i64, &str)]) -> String {
    let mut output = String::new();
    for (index, (cost, bytes, formatted)) in rows.iter().enumerate() {
        output.push_str(&format!(
            "SQL {index}: \n\
             cost_time: {cost}s\n\
             txn_start_ts: 0\n\
             mem_max: {bytes} Bytes ({formatted})\n\
             sql: \n\
             session_alias: \n\
             affected rows: 0\n\
             tidb_mem_oom_action: CANCEL\n\
             tidb_server_memory_limit: 0\n\
             tidb_mem_quota_query: 0\n\
             tidb_analyze_version: 0\n\
             tidb_enable_rate_limit_action: false\n\
             current_analyze_plan: |id|estRows|task|access object|operator info|\n"
        ));
    }
    output.push('\n');
    output
}

/// 验证按内存/按耗时 Top10 排序与字段输出。
#[test]
pub fn TestGetTop10Sql() {
    let _lock = GLOBAL_VARIABLE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = GlobalVariableGuard::set(0);
    let temp = tempfile::tempdir().expect("create record temp dir");
    let provider = Arc::new(MockConfigProvider::new(
        temp.path().display().to_string(),
        0.7,
        5,
    ));
    let mut record = memoryUsageAlarm::new(provider);
    record.lastCheckTime = genTime(123_456);

    let memory = [1_000, 87_263_523, 34_223];
    let starts = [genTime(1_234), genTime(123_456), genTime(12)];
    let process_info = genMockProcessInfoList(&memory, &starts, 3);

    let mut by_memory = process_info.clone();
    assert_eq!(
        record.getTop10SqlInfoByMemoryUsage(&mut by_memory),
        expectedTopSql(&[
            (0, 87_263_523, "83.2 MB"),
            (123_444, 34_223, "33.4 KB"),
            (122_222, 1_000, "1000 Bytes"),
        ])
    );

    let mut by_cost = process_info;
    assert_eq!(
        record.getTop10SqlInfoByCostTime(&mut by_cost),
        expectedTopSql(&[
            (123_444, 34_223, "33.4 KB"),
            (122_222, 1_000, "1000 Bytes"),
            (0, 87_263_523, "83.2 MB"),
        ])
    );

    let memory = [
        1_000,
        87_263_523,
        34_223,
        532_355,
        123_225_151,
        231_231_515,
        12_312,
        12_515_134_234,
        232,
        12_414,
        15_263_236,
        123_123_123,
        15,
    ];
    let starts = [
        genTime(1_234),
        genTime(123_456),
        genTime(12),
        genTime(3_241),
        genTime(12_515),
        genTime(3_215),
        genTime(61_314),
        genTime(12_234),
        genTime(1_123),
        genTime(512),
        genTime(11_111),
        genTime(22_222),
        genTime(5_512),
    ];
    let process_info = genMockProcessInfoList(&memory, &starts, 13);

    let mut by_memory = process_info.clone();
    assert_eq!(
        record.getTop10SqlInfoByMemoryUsage(&mut by_memory),
        expectedTopSql(&[
            (111_222, 12_515_134_234, "11.7 GB"),
            (120_241, 231_231_515, "220.5 MB"),
            (110_941, 123_225_151, "117.5 MB"),
            (101_234, 123_123_123, "117.4 MB"),
            (0, 87_263_523, "83.2 MB"),
            (112_345, 15_263_236, "14.6 MB"),
            (120_215, 532_355, "519.9 KB"),
            (123_444, 34_223, "33.4 KB"),
            (122_944, 12_414, "12.1 KB"),
            (62_142, 12_312, "12.0 KB"),
        ])
    );

    let mut by_cost = process_info;
    assert_eq!(
        record.getTop10SqlInfoByCostTime(&mut by_cost),
        expectedTopSql(&[
            (123_444, 34_223, "33.4 KB"),
            (122_944, 12_414, "12.1 KB"),
            (122_333, 232, "232 Bytes"),
            (122_222, 1_000, "1000 Bytes"),
            (120_241, 231_231_515, "220.5 MB"),
            (120_215, 532_355, "519.9 KB"),
            (117_944, 15, "15 Bytes"),
            (112_345, 15_263_236, "14.6 MB"),
            (111_222, 12_515_134_234, "11.7 GB"),
            (110_941, 123_225_151, "117.5 MB"),
        ])
    );
}

/// 验证 updateVariable 受 60s 刷新窗口约束，窗口外可读到新配置。
#[test]
pub fn TestUpdateVariables() {
    let _lock = GLOBAL_VARIABLE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = GlobalVariableGuard::set(1_024);
    let temp = tempfile::tempdir().expect("create record temp dir");
    let mock_config = Arc::new(MockConfigProvider::new(
        temp.path().display().to_string(),
        0.3,
        3,
    ));
    let mut record = memoryUsageAlarm::new(mock_config.clone());

    record.initMemoryUsageAlarmRecord();
    assert_eq!(record.configProvider.GetMemoryUsageAlarmRatio(), 0.3);
    assert_eq!(record.configProvider.GetMemoryUsageAlarmKeepRecordNum(), 3);
    assert_eq!(record.serverMemoryLimit, 1_024);

    mock_config.set_ratio(0.6);
    mock_config.set_keep_num(6);
    task_memory::tracker::ServerMemoryLimit.Store(2_048);

    record.updateVariable();
    assert_eq!(record.configProvider.GetMemoryUsageAlarmRatio(), 0.6);
    assert_eq!(record.configProvider.GetMemoryUsageAlarmKeepRecordNum(), 6);
    assert_eq!(record.memoryUsageAlarmRatio, 0.3);
    assert_eq!(record.memoryUsageAlarmKeepRecordNum, 3);
    assert_eq!(record.serverMemoryLimit, 1_024);

    record.lastUpdateVariableTime = record.lastUpdateVariableTime - Duration::from_secs(60);
    record.updateVariable();
    assert_eq!(record.configProvider.GetMemoryUsageAlarmRatio(), 0.6);
    assert_eq!(record.configProvider.GetMemoryUsageAlarmKeepRecordNum(), 6);
    assert_eq!(record.memoryUsageAlarmRatio, 0.6);
    assert_eq!(record.memoryUsageAlarmKeepRecordNum, 6);
    assert_eq!(record.serverMemoryLimit, 2_048);
}

/// 在有后台线程时写入 goroutine profile 应成功。
#[test]
pub fn TestRecordGoroutineProfileWithBackgroundGoroutine() {
    let record_dir = tempfile::tempdir().expect("create profile temp dir");
    recordGoroutineProfile(record_dir.path().to_str().expect("utf8 temp path"))
        .expect("record native thread profile");

    let content = fs::read_to_string(record_dir.path().join("goroutine"))
        .expect("read generated thread profile");
    assert!(!content.is_empty());
    // Rust has native threads rather than Go goroutines; these are the direct
    // equivalents of the Go profile's goroutine header and stack payload.
    assert!(content.contains("thread "));
    assert!(content.contains("[running]:"));
    assert!(
        content.lines().count() > 2,
        "profile lacks a backtrace: {content}"
    );
}

/// Stable Rust has no built-in `testing.B`; retain the Go benchmark workload as
/// a callable native helper so it is compiled without turning it into a unit test.
/// goroutine profile 基准：有后台线程时反复写入。
pub fn benchmarkRecordGoroutineProfileWithBackgroundGoroutine(
    iterations: usize,
    background_thread_count: usize,
) -> io::Result<()> {
    let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(0);
    let mut workers = Vec::with_capacity(background_thread_count);
    for _ in 0..background_thread_count {
        let stop_rx = stop_rx.clone();
        workers.push(thread::spawn(move || {
            let _ = stop_rx.recv();
        }));
    }

    for _ in 0..iterations {
        let record_dir = tempfile::tempdir()?;
        recordGoroutineProfile(record_dir.path().to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 benchmark temp path")
        })?)?;
    }

    drop(stop_tx);
    for worker in workers {
        worker.join().expect("background benchmark thread panicked");
    }
    Ok(())
}

/// 无额外后台线程时的 goroutine profile 基准入口。
pub fn BenchmarkRecordGoroutineProfile(iterations: usize) -> io::Result<()> {
    for (_name, count) in [
        ("WithBackgroundGoroutine/10", 10),
        ("WithBackgroundGoroutine/100", 100),
        ("WithBackgroundGoroutine/1000", 1_000),
        // The Go label says 10000 but intentionally passes 1000; preserve it.
        ("WithBackgroundGoroutine/10000", 1_000),
    ] {
        benchmarkRecordGoroutineProfileWithBackgroundGoroutine(iterations, count)?;
    }
    Ok(())
}
