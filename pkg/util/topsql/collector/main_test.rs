// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// TopSQL collector 集成/单元测试：pprof 采集、进程 CPU 更新与 sqlStats.tune。
//
// 通过 MockCollector / MockUpdater 与 MockCPULoad 验证开关切换与注销语义。

use super::*;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpuprofile::{
    CancellationToken, LabelSet, MockCPULoad, MockCPULoadV2, ProfileData, StartCPUProfiler,
    StopCPUProfiler, global_consumers_count, set_profile_duration,
};
use pprof::protos::{Label, Message, Profile, Sample, ValueType};
use serial_test::serial;
use testsetup::SetupForCommonTest;

/// 测试结束时关闭 TopSQL 与 CPU profiler。
struct TestRuntimeGuard;

impl Drop for TestRuntimeGuard {
    fn drop(&mut self) {
        topsql_state::DisableTopSQL();
        StopCPUProfiler();
    }
}

/// 公共测试前置：禁用 TopSQL、停止 profiler 并断言无消费者。
fn setup_test() -> TestRuntimeGuard {
    SetupForCommonTest();
    topsql_state::DisableTopSQL();
    StopCPUProfiler();
    assert_eq!(global_consumers_count(), 0);
    TestRuntimeGuard
}

/// 将 Collect 结果送入通道的模拟 Collector。
struct MockCollector {
    data_ch: crossbeam_channel::Sender<Vec<SQLCPUTimeRecord>>,
    data_rx: crossbeam_channel::Receiver<Vec<SQLCPUTimeRecord>>,
}

impl MockCollector {
    fn new() -> Self {
        let (data_ch, data_rx) = crossbeam_channel::bounded(10);
        Self { data_ch, data_rx }
    }
}

impl Collector for MockCollector {
    fn Collect(&self, records: Vec<SQLCPUTimeRecord>) {
        self.data_ch
            .send(records)
            .expect("collector receiver must remain alive");
    }
}

/// 记录连接→sqlID 映射并在凑满 3 条时发通知的模拟 Updater。
struct MockUpdater {
    data_ch: crossbeam_channel::Sender<bool>,
    data_rx: crossbeam_channel::Receiver<bool>,
    conn_id_set: Mutex<HashMap<u64, u64>>,
}

impl MockUpdater {
    fn new() -> Self {
        let (data_ch, data_rx) = crossbeam_channel::bounded(10);
        Self {
            data_ch,
            data_rx,
            conn_id_set: Mutex::new(HashMap::new()),
        }
    }

    fn reset_conn_id_set(&self) {
        *self.conn_id_set.lock().unwrap() = HashMap::new();
    }

    fn snapshot(&self) -> HashMap<u64, u64> {
        self.conn_id_set.lock().unwrap().clone()
    }

    fn drain_notifications(&self) {
        while self.data_rx.try_recv().is_ok() {}
    }
}

impl ProcessCPUTimeUpdater for MockUpdater {
    fn UpdateProcessCPUTime(&self, conn_id: u64, sql_id: u64, _duration: Duration) {
        let mut conn_id_set = self.conn_id_set.lock().unwrap();
        conn_id_set.insert(conn_id, sql_id);
        if conn_id_set.len() == 3 {
            let _ = self.data_ch.try_send(true);
        }
    }
}

/// 由标签集合构造简易 pprof Profile。
fn profile_from_label_sets(label_sets: &[LabelSet]) -> Profile {
    let mut profile = Profile {
        sample_type: vec![ValueType { ty: 0, unit: 0 }],
        string_table: vec![String::new()],
        ..Profile::default()
    };
    for labels in label_sets {
        let mut encoded_labels = Vec::with_capacity(labels.len());
        for (key, value) in labels {
            let key_index = profile.string_table.len() as i64;
            profile.string_table.push(key.clone());
            let value_index = profile.string_table.len() as i64;
            profile.string_table.push(value.clone());
            encoded_labels.push(Label {
                key: key_index,
                str: value_index,
                num: 0,
                num_unit: 0,
            });
        }
        profile.sample.push(Sample {
            location_id: Vec::new(),
            value: vec![10_000_000],
            label: encoded_labels,
        });
    }
    profile
}

/// 等待全局消费者数量达到期望值。
fn wait_for_consumer_count(expected: usize, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while global_consumers_count() != expected && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(global_consumers_count(), expected);
}

/// 将 Profile 编码为成功的 ProfileData。
fn profile_data(profile: &Profile) -> ProfileData {
    ProfileData::success(profile.encode_to_vec())
}

/// Rust's test runner owns process startup. Preserve Go TestMain's common setup
#[test]
fn TestMain() {
    SetupForCommonTest();
}

#[test]
#[serial]
/// 验证 TopSQL 开关下 SQL CPU profile 采集与注销。
fn TestPProfCPUProfile() {
    let _runtime = setup_test();
    let interval = Duration::from_millis(20);
    set_profile_duration(interval);
    StartCPUProfiler().unwrap();
    topsql_state::EnableTopSQL();

    let collector = Arc::new(MockCollector::new());
    let mut sql_cpu_collector = NewSQLCPUCollector(collector.clone());
    sql_cpu_collector.set_collect_interval(interval);
    sql_cpu_collector.Start();
    wait_for_consumer_count(1, Duration::from_secs(1));

    let cancel = CancellationToken::new();
    let load = MockCPULoad(
        &cancel,
        vec!["sql".into(), "sql_digest".into(), "plan_digest".into()],
    );
    let data = profile_data(&profile_from_label_sets(load.label_sets()));
    cancel.cancel();
    assert!(load.join_timeout(Duration::from_secs(1)));

    handleProfileData::<MockCollector, MockUpdater>(&data, &collector, None);
    let records = collector.data_rx.recv_timeout(interval * 4).unwrap();
    assert!(!records.is_empty());
    assert_eq!(records[0].SQLDigest, b"sql_digest value");

    topsql_state::DisableTopSQL();
    wait_for_consumer_count(0, Duration::from_secs(1));
    let data_ch_len = collector.data_rx.len();

    topsql_state::EnableTopSQL();
    wait_for_consumer_count(1, Duration::from_secs(1));
    handleProfileData::<MockCollector, MockUpdater>(&data, &collector, None);
    let mut records = Vec::new();
    let mut delta_len = 0;
    for _ in 0..10 {
        let started = Instant::now();
        records = collector.data_rx.recv_timeout(interval * 4).unwrap();
        assert!(started.elapsed() < interval * 4);
        if !records.is_empty() {
            delta_len += 1;
            if delta_len > data_ch_len {
                break;
            }
        }
    }
    assert!(!records.is_empty());
    assert!(delta_len > data_ch_len);
    assert_eq!(records[0].SQLDigest, b"sql_digest value");

    sql_cpu_collector.Stop();
    assert_eq!(global_consumers_count(), 0);
}

#[test]
#[serial]
/// 验证进程级 CPU 更新在开关切换后的暂停与恢复。
fn TestProcessProfCPUProfile() {
    let _runtime = setup_test();
    let interval = Duration::from_millis(20);
    set_profile_duration(interval);
    StartCPUProfiler().unwrap();
    topsql_state::EnableTopSQL();

    let updater = Arc::new(MockUpdater::new());
    updater.reset_conn_id_set();
    let collector = Arc::new(MockCollector::new());
    let mut sql_cpu_collector = NewSQLCPUCollector(collector.clone());
    sql_cpu_collector.SetProcessCPUUpdater(updater.clone());
    sql_cpu_collector.set_collect_interval(interval);
    sql_cpu_collector.Start();
    wait_for_consumer_count(1, Duration::from_secs(1));

    let cancel = CancellationToken::new();
    let load = MockCPULoadV2(&cancel, vec!["0_0".into(), "1_0".into(), "2_1".into()]);
    let data = profile_data(&profile_from_label_sets(load.label_sets()));
    cancel.cancel();
    assert!(load.join_timeout(Duration::from_secs(1)));

    handleProfileData(&data, &collector, Some(&updater));
    updater
        .data_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    let conn_id_set = updater.snapshot();
    assert_eq!(conn_id_set.len(), 3);
    assert_eq!(conn_id_set[&0], 0);
    assert_eq!(conn_id_set[&1], 0);
    assert_eq!(conn_id_set[&2], 1);

    topsql_state::DisableTopSQL();
    wait_for_consumer_count(0, Duration::from_secs(1));
    updater.drain_notifications();
    updater.reset_conn_id_set();
    std::thread::sleep(interval * 2);
    assert_eq!(updater.data_rx.len(), 0);
    assert!(updater.snapshot().is_empty());

    topsql_state::EnableTopSQL();
    wait_for_consumer_count(1, Duration::from_secs(1));
    handleProfileData(&data, &collector, Some(&updater));
    updater
        .data_rx
        .recv_timeout(interval * 8)
        .expect("process updates resume after TopSQL is re-enabled");
    let conn_id_set = updater.snapshot();
    assert_eq!(conn_id_set.len(), 3);
    assert_eq!(conn_id_set[&0], 0);
    assert_eq!(conn_id_set[&1], 0);
    assert_eq!(conn_id_set[&2], 1);

    sql_cpu_collector.Stop();
    assert_eq!(global_consumers_count(), 0);
}

#[test]
/// 验证 sqlStats.tune 单计划与多计划余量归空 digest。
fn TestSQLStatsTune() {
    SetupForCommonTest();

    let mut stats = sqlStats {
        plans: HashMap::from([("plan-1".to_owned(), 80)]),
        total: 100,
    };
    stats.tune();
    assert_eq!(stats.total, 100);
    assert_eq!(stats.plans["plan-1"], 100);

    stats = sqlStats {
        plans: HashMap::from([("plan-1".to_owned(), 30), ("plan-2".to_owned(), 30)]),
        total: 100,
    };
    stats.tune();
    assert_eq!(stats.total, 100);
    assert_eq!(stats.plans["plan-1"], 30);
    assert_eq!(stats.plans["plan-2"], 30);
    assert_eq!(stats.plans[""], 40);
}
