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

// 内存使用告警迁移（Aster）单元测试：阈值、刷新窗口、目录保留与 profile。
//
// 与 Go 行为对齐，覆盖 needRecord、updateVariable、初始化/清理、
// Top SQL 排序，以及 heap/goroutine 落盘。

use std::fs;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::memoryusagealarm::{
    AlarmReason, ConfigProvider, ProcessInfo, TiDBConfigProvider, memoryUsageAlarm,
    recordGoroutineProfile,
};

/// 简易 mock 配置提供者。
#[derive(Debug)]
struct MockConfigProvider {
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
}

impl ConfigProvider for MockConfigProvider {
    fn GetMemoryUsageAlarmRatio(&self) -> f64 {
        *self.ratio.read().expect("ratio lock poisoned")
    }

    fn GetMemoryUsageAlarmKeepRecordNum(&self) -> i64 {
        *self.keep_num.read().expect("keep-num lock poisoned")
    }

    fn GetLogDir(&self) -> String {
        self.log_dir.clone()
    }

    fn GetComponentName(&self) -> String {
        self.component_name.clone()
    }
}

/// 作用域内临时改写 ServerMemoryLimit，退出还原。
struct MemoryLimitGuard(u64);

impl MemoryLimitGuard {
    /// 设置新上限并返回持有旧值的守卫。
    fn set(value: u64) -> Self {
        let old = task_memory::tracker::ServerMemoryLimit.Load();
        task_memory::tracker::ServerMemoryLimit.Store(value);
        Self(old)
    }
}

impl Drop for MemoryLimitGuard {
    fn drop(&mut self) {
        task_memory::tracker::ServerMemoryLimit.Store(self.0);
    }
}

/// 迁移用例：阈值、冷却与增长规则与 Go 一致。
#[test]
fn migration_need_record_matches_threshold_cooldown_and_growth_rules() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let provider = Arc::new(MockConfigProvider::new(
        temp.path().display().to_string(),
        0.7,
        5,
    ));
    let mut record = memoryUsageAlarm::new(provider);
    record.serverMemoryLimit = 1_000;
    record.memoryUsageAlarmRatio = 0.7;

    assert_eq!(record.needRecord(690), (false, AlarmReason::NoReason));
    assert_eq!(
        record.needRecord(710),
        (true, AlarmReason::ExceedAlarmRatio)
    );

    record.lastCheckTime = SystemTime::now();
    record.lastRecordMemUsed = 710;
    assert_eq!(record.needRecord(800), (false, AlarmReason::NoReason));
    assert_eq!(record.needRecord(811), (true, AlarmReason::GrowTooFast));

    record.lastCheckTime = SystemTime::now() - Duration::from_secs(61);
    assert_eq!(
        record.needRecord(711),
        (true, AlarmReason::ExceedAlarmRatio)
    );

    record.serverMemoryLimit = u64::MAX;
    record.memoryUsageAlarmRatio = 0.1;
    record.lastCheckTime = SystemTime::now();
    record.lastRecordMemUsed = 0;
    assert_eq!(
        record.needRecord(i64::MAX as u64 + 100),
        (false, AlarmReason::NoReason)
    );
}

/// 迁移用例：默认配置提供者使用普通日志文件目录，而不是慢日志目录。
#[test]
fn migration_tidb_config_provider_uses_main_log_file_directory() {
    let _lock = crate::memoryusagealarm_test::GLOBAL_VARIABLE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let restore = task_config::restore_func();
    task_config::update_global(|config| {
        config.log.file.filename = "/tmp/astersql-main/tidb.log".to_owned();
        config.log.slow_query_file = "/tmp/astersql-slow/tidb-slow.log".to_owned();
    });

    assert_eq!(
        TiDBConfigProvider.GetLogDir(),
        "/tmp/astersql-main".to_owned()
    );
    restore();
}

/// 迁移用例：updateVariable 遵守 60s 刷新窗口。
#[test]
fn migration_update_variables_obeys_the_sixty_second_refresh_window() {
    let _lock = crate::memoryusagealarm_test::GLOBAL_VARIABLE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = MemoryLimitGuard::set(1_024);
    let temp = tempfile::tempdir().expect("create temp dir");
    let provider = Arc::new(MockConfigProvider::new(
        temp.path().display().to_string(),
        0.3,
        3,
    ));
    let mut record = memoryUsageAlarm::new(provider.clone());

    record.initMemoryUsageAlarmRecord();
    assert_eq!(record.memoryUsageAlarmRatio, 0.3);
    assert_eq!(record.memoryUsageAlarmKeepRecordNum, 3);
    assert_eq!(record.serverMemoryLimit, 1_024);

    *provider.ratio.write().expect("ratio lock poisoned") = 0.6;
    *provider.keep_num.write().expect("keep-num lock poisoned") = 6;
    task_memory::tracker::ServerMemoryLimit.Store(2_048);
    record.updateVariable();
    assert_eq!(record.memoryUsageAlarmRatio, 0.3);
    assert_eq!(record.serverMemoryLimit, 1_024);

    record.lastUpdateVariableTime = SystemTime::now() - Duration::from_secs(61);
    record.updateVariable();
    assert_eq!(record.memoryUsageAlarmRatio, 0.6);
    assert_eq!(record.memoryUsageAlarmKeepRecordNum, 6);
    assert_eq!(record.serverMemoryLimit, 2_048);
}

/// 迁移用例：初始化扫描 record 目录并按保留数清理。
#[test]
fn migration_initialization_and_retention_match_go_directory_rules() {
    let _lock = crate::memoryusagealarm_test::GLOBAL_VARIABLE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _restore = MemoryLimitGuard::set(4_096);
    let temp = tempfile::tempdir().expect("create temp dir");
    let base = temp.path().join("oom_record");
    fs::create_dir_all(base.join("record-z")).expect("create z record");
    fs::create_dir_all(base.join("record-a")).expect("create a record");
    fs::create_dir_all(base.join("unrelated")).expect("create unrelated dir");
    let provider = Arc::new(MockConfigProvider::new(
        temp.path().display().to_string(),
        0.7,
        2,
    ));
    let mut record = memoryUsageAlarm::new(provider);

    record.initMemoryUsageAlarmRecord();
    assert!(record.initialized);
    assert_eq!(record.lastRecordDirName.len(), 2);
    assert!(record.lastRecordDirName[0].ends_with("record-a"));
    assert!(record.lastRecordDirName[1].ends_with("record-z"));

    let second = base.join("record-second");
    let third = base.join("record-third");
    fs::create_dir_all(&second).expect("create second record");
    fs::create_dir_all(&third).expect("create third record");
    record.lastRecordDirName.push(second.display().to_string());
    record.lastRecordDirName.push(third.display().to_string());
    record.tryRemoveRedundantRecords();

    assert!(!base.join("record-a").exists());
    assert!(!base.join("record-z").exists());
    assert!(base.join("unrelated").exists());
    assert!(second.exists());
    assert!(third.exists());
    assert_eq!(record.lastRecordDirName.len(), 2);
}

/// 构造仅含内存与开始时间的简易 ProcessInfo。
fn process_info(memory: i64, start: SystemTime) -> Arc<ProcessInfo> {
    Arc::new(ProcessInfo {
        time: start,
        max_consumed: memory,
        ..ProcessInfo::default()
    })
}

/// 迁移用例：Top SQL 排序、条数上限与字段保留。
#[test]
fn migration_top_sql_output_preserves_sorting_limit_and_fields() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let provider = Arc::new(MockConfigProvider::new(
        temp.path().display().to_string(),
        0.7,
        5,
    ));
    let mut record = memoryUsageAlarm::new(provider);
    record.lastCheckTime = UNIX_EPOCH + Duration::from_secs(123_456);
    let mut processes = vec![
        process_info(1_000, UNIX_EPOCH + Duration::from_secs(1_234)),
        process_info(87_263_523, UNIX_EPOCH + Duration::from_secs(123_456)),
        process_info(34_223, UNIX_EPOCH + Duration::from_secs(12)),
    ];

    let by_memory = record.getTop10SqlInfoByMemoryUsage(&mut processes);
    assert!(by_memory.starts_with("SQL 0: \n"));
    assert!(by_memory.contains("mem_max: 87263523 Bytes (83.2 MB)"));
    assert!(by_memory.contains("tidb_mem_oom_action: CANCEL"));
    assert!(
        by_memory.contains("current_analyze_plan: |id|estRows|task|access object|operator info|")
    );

    let by_cost = record.getTop10SqlInfoByCostTime(&mut processes);
    let first = by_cost.find("mem_max: 34223 Bytes").expect("oldest SQL");
    let second = by_cost.find("mem_max: 1000 Bytes").expect("second SQL");
    assert!(first < second);

    let mut many = (0..13)
        .map(|index| process_info(index + 1, UNIX_EPOCH + Duration::from_secs(index as u64)))
        .collect::<Vec<_>>();
    let limited = record.getTop10SqlInfoByMemoryUsage(&mut many);
    assert_eq!(limited.matches("SQL ").count(), 10);
    assert!(!limited.contains("SQL 10:"));
}

/// 迁移用例：doRecord/recordProfile 写出 heap 与线程 profile。
#[test]
fn migration_record_action_writes_heap_and_thread_profiles() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let provider = Arc::new(MockConfigProvider::new(
        temp.path().display().to_string(),
        0.7,
        5,
    ));
    let mut record = memoryUsageAlarm::new(provider);
    record.baseRecordDir = temp.path().join("oom_record").display().to_string();
    fs::create_dir_all(&record.baseRecordDir).expect("create record base");
    record.lastCheckTime = UNIX_EPOCH + Duration::from_secs(123_456);

    record.doRecord(800, 400, None, AlarmReason::ExceedAlarmRatio);
    assert!(record.err.is_none(), "record error: {:?}", record.err);
    assert_eq!(record.lastRecordDirName.len(), 1);
    let record_dir = &record.lastRecordDirName[0];
    let timestamp: chrono::DateTime<chrono::Local> = record.lastCheckTime.into();
    assert!(record_dir.ends_with(&format!(
        "record{}",
        timestamp.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    )));
    assert!(
        !fs::read(format!("{record_dir}/heap"))
            .expect("read heap profile")
            .is_empty()
    );
    let stack = fs::read_to_string(format!("{record_dir}/goroutine")).expect("read thread profile");
    assert!(stack.contains("thread"));
}

/// 迁移用例：目标目录不可写时 goroutine profile 返回创建错误。
#[test]
fn migration_goroutine_profile_reports_create_errors() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let missing = temp.path().join("missing");
    assert!(recordGoroutineProfile(missing.to_str().expect("utf8 path")).is_err());
}
