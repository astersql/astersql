// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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
// Meter 状态机单元测试：注册/注销、flush 增量、失败重试、本地文件回读等。

// limitations under the License.

use crate::data::{
    CLUSTER_READ_BYTES_FIELD, CLUSTER_WRITE_BYTES_FIELD, GET_REQUESTS_FIELD, MeterItem, MeterValue,
    OBJ_STORE_READ_BYTES_FIELD, OBJ_STORE_WRITE_BYTES_FIELD, PUT_REQUESTS_FIELD,
};
use crate::metering::{
    CATEGORY, Context, Meter, MeteringConfig, MeteringData, MeteringWriter, RegisterRecorder,
    SetMetering, UnregisterRecorder, WriterFactory,
};
use crate::recorder::Recorder;
use anyhow::{Result, anyhow};
use proto::step::StepInit;
use proto::task::{ExtraParams, TaskBase, TaskStatePending};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 测试用写入钩子：可注入失败或在写入时修改 recorder。
type WriteHook = Box<dyn FnMut(&Context, &MeteringData) -> Result<()> + Send>;

/// 可记录尝试次数、成功写入与关闭错误的 mock Writer。
#[derive(Default)]
struct TestWriter {
    hook: Mutex<Option<WriteHook>>,
    attempts: Mutex<Vec<MeteringData>>,
    successful_writes: Mutex<Vec<MeteringData>>,
    close_error: Mutex<Option<String>>,
    closes: AtomicUsize,
}

/// 安装/替换写入钩子。
impl TestWriter {
    fn set_hook<F>(&self, hook: F)
    where
        F: FnMut(&Context, &MeteringData) -> Result<()> + Send + 'static,
    {
        *self.hook.lock().unwrap() = Some(Box::new(hook));
    }

    /// 设置 close 时返回的错误信息。
    fn set_close_error(&self, message: Option<&str>) {
        *self.close_error.lock().unwrap() = message.map(str::to_owned);
    }

    /// 返回所有成功写入的载荷快照。
    fn successful_writes(&self) -> Vec<MeteringData> {
        self.successful_writes.lock().unwrap().clone()
    }

    /// 返回写入尝试总次数。
    fn attempt_count(&self) -> usize {
        self.attempts.lock().unwrap().len()
    }

    /// 返回指定时间戳的写入尝试次数。
    fn attempts_for(&self, timestamp: i64) -> usize {
        self.attempts
            .lock()
            .unwrap()
            .iter()
            .filter(|data| data.timestamp == timestamp)
            .count()
    }
}

impl MeteringWriter for TestWriter {
    fn write(&self, context: &Context, data: MeteringData) -> Result<()> {
        assert!(
            context.deadline().is_some(),
            "meter writes must retain the Go timeout boundary"
        );
        self.attempts.lock().unwrap().push(data.clone());
        let result = match self.hook.lock().unwrap().as_mut() {
            Some(hook) => hook(context, &data),
            None => Ok(()),
        };
        if result.is_ok() {
            self.successful_writes.lock().unwrap().push(data);
        }
        result
    }

    fn close(&self) -> Result<()> {
        self.closes.fetch_add(1, Ordering::Relaxed);
        match self.close_error.lock().unwrap().clone() {
            Some(message) => Err(anyhow!(message)),
            None => Ok(()),
        }
    }
}

/// 始终返回同一 TestWriter，并记录创建时的配置。
struct TestFactory {
    writer: Arc<TestWriter>,
    configs: Mutex<Vec<MeteringConfig>>,
}

/// 用给定 Writer 构造工厂。
impl TestFactory {
    fn new(writer: Arc<TestWriter>) -> Self {
        Self {
            writer,
            configs: Mutex::new(Vec::new()),
        }
    }
}

impl WriterFactory for TestFactory {
    fn create(&self, config: &MeteringConfig) -> Result<Arc<dyn MeteringWriter>> {
        self.configs.lock().unwrap().push(config.clone());
        Ok(self.writer.clone())
    }
}

/// 进程级全局 Meter 测试互斥锁，避免并行测试互相覆盖 SetMetering。
pub(crate) fn global_meter_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// RAII：Drop 时清空全局 Meter。
struct MeteringInstallation;

impl Drop for MeteringInstallation {
    fn drop(&mut self) {
        SetMetering(None);
    }
}

/// 安装全局 Meter，并在返回守卫 Drop 时清理。
fn setup_meter_for_test(meter: Arc<Meter>) -> MeteringInstallation {
    SetMetering(Some(meter));
    MeteringInstallation
}

/// 构造最小化的 Example 类型 TaskBase 测试夹具。
fn task(id: i64, keyspace: &str) -> TaskBase {
    TaskBase {
        ID: id,
        Key: String::new(),
        Type: "Example",
        State: TaskStatePending,
        Step: StepInit,
        Priority: 512,
        RequiredSlots: 1,
        TargetScope: String::new(),
        CreateTime: SystemTime::now(),
        MaxNodeCount: 0,
        ExtraParams: ExtraParams::default(),
        Keyspace: keyspace.to_owned(),
    }
}

/// 创建带 TestWriter 的 Meter 对。
fn meter() -> (Arc<Meter>, Arc<TestWriter>) {
    let writer = Arc::new(TestWriter::default());
    (Meter::with_writer(writer.clone()), writer)
}

/// 从 MeterItem 取出 U64 字段。
fn value_u64(item: &MeterItem, field: &str) -> Option<u64> {
    match item.get(field) {
        Some(MeterValue::U64(value)) => Some(*value),
        _ => None,
    }
}

/// 从 MeterItem 取出 I64 字段。
fn value_i64(item: &MeterItem, field: &str) -> Option<i64> {
    match item.get(field) {
        Some(MeterValue::I64(value)) => Some(*value),
        _ => None,
    }
}

/// 断言期望字段值存在，且六类标准计数器中未列出的字段不出现。
fn check_meter_data(expected: &[(&str, u64)], got: &MeterItem) {
    for (field, value) in expected {
        assert_eq!(
            value_u64(got, field),
            Some(*value),
            "field {field} not equal"
        );
    }
    for field in [
        GET_REQUESTS_FIELD,
        PUT_REQUESTS_FIELD,
        OBJ_STORE_READ_BYTES_FIELD,
        OBJ_STORE_WRITE_BYTES_FIELD,
        CLUSTER_READ_BYTES_FIELD,
        CLUSTER_WRITE_BYTES_FIELD,
    ] {
        if !expected
            .iter()
            .any(|(expected_field, _)| *expected_field == field)
        {
            assert!(!got.contains_key(field), "field {field} should not exist");
        }
    }
}

/// 空 bucket 配置应禁用 Meter，且不调用工厂。
#[test]
fn test_new_meter_empty_bucket() {
    let writer = Arc::new(TestWriter::default());
    let factory = TestFactory::new(writer);
    let created = Meter::new(&MeteringConfig::default(), &factory).unwrap();
    assert!(created.is_none());
    assert!(factory.configs.lock().unwrap().is_empty());
}

/// 合法 s3/azure 配置应开启 overwrite、能注册 recorder 并成功 flush。
#[test]
fn test_new_meter_valid_config() {
    let cases = [
        ("s3", MeteringConfig::new("s3", "test-bucket")),
        (
            "azure",
            MeteringConfig::new("azure", "test-container/test-prefix"),
        ),
    ];

    for (name, config) in cases {
        let writer = Arc::new(TestWriter::default());
        let factory = TestFactory::new(writer.clone());
        let meter = Meter::new(&config, &factory)
            .unwrap_or_else(|error| panic!("{name} config failed: {error}"))
            .unwrap_or_else(|| panic!("{name} config unexpectedly disabled metering"));
        let configs = factory.configs.lock().unwrap();
        assert_eq!(configs.len(), 1, "{name}");
        assert!(configs[0].overwrite_existing, "{name}");
        drop(configs);

        let recorder = meter.get_or_register_recorder(Recorder::new(1, "ks", "tt"));
        recorder.record_obj_store_get(1);
        meter.flush(&Context::background(), 1_000_000);
        let writes = writer.successful_writes();
        assert_eq!(writes.len(), 1, "{name}");
        assert!(!writes[0].self_id.is_empty(), "{name}");
    }
}

/// Classic 模式下 Register/Unregister 为空操作，不真正登记 recorder。
#[test]
fn test_meter_register_unregister_recorder_in_classic() {
    let _lock = global_meter_lock().lock().unwrap();
    SetMetering(None);
    if kerneltype::IsNextGen() {
        return;
    }

    let recorder = RegisterRecorder(&task(1, ""));
    assert_eq!(recorder.curr_data().task_id(), 0);

    let (meter, _) = meter();
    let _installation = setup_meter_for_test(meter.clone());
    RegisterRecorder(&task(1, ""));
    UnregisterRecorder(1);
    assert!(!meter.contains_recorder(1));
}

/// 覆盖注册/注销时机：flush 后保留、注销后最终移除、写中途再注册、重新注册计数归零。
#[test]
fn test_meter_register_unregister_recorder() {
    let _lock = global_meter_lock().lock().unwrap();
    if kerneltype::IsClassic() {
        return;
    }
    let context = Context::background();

    // recorder still there after flush
    {
        let (meter, writer) = meter();
        let _installation = setup_meter_for_test(meter.clone());
        let recorder = RegisterRecorder(&task(1, ""));
        assert!(meter.contains_recorder(1));
        recorder.record_obj_store_get(1);
        meter.flush(&context, 1_000_000);
        assert!(meter.contains_recorder(1));
        assert_eq!(writer.attempt_count(), 1);
    }

    // If unregistered before flush, remove after all scraped data is written.
    {
        let (meter, writer) = meter();
        let _installation = setup_meter_for_test(meter.clone());
        let recorder = RegisterRecorder(&task(1, ""));
        recorder.record_obj_store_get(1);
        UnregisterRecorder(1);
        assert!(meter.contains_recorder(1));
        assert!(meter.is_unregistered(1));
        meter.flush(&context, 1_000_000);
        assert!(!meter.contains_recorder(1));
        assert_eq!(writer.attempt_count(), 1);

        RegisterRecorder(&task(2, ""));
        UnregisterRecorder(2);
        meter.flush(&context, 2_000_000);
        assert!(!meter.contains_recorder(2));
        assert_eq!(writer.attempt_count(), 1, "empty data must not be written");
    }

    // Unregistering after scrape but before write completion keeps the recorder
    // until the data added during the write has also been flushed.
    {
        let (meter, writer) = meter();
        let _installation = setup_meter_for_test(meter.clone());
        let recorder = RegisterRecorder(&task(1, ""));
        recorder.record_obj_store_get(1);
        let callback_recorder = recorder.clone();
        writer.set_hook(move |_, data| {
            assert_eq!(data.items.len(), 1);
            check_meter_data(&[(GET_REQUESTS_FIELD, 1)], &data.items[0]);
            callback_recorder.record_obj_store_put(2);
            UnregisterRecorder(1);
            Ok(())
        });
        meter.flush(&context, 1_000_000);
        assert!(meter.contains_recorder(1));

        writer.set_hook(|_, data| {
            assert_eq!(data.items.len(), 1);
            check_meter_data(&[(PUT_REQUESTS_FIELD, 2)], &data.items[0]);
            Ok(())
        });
        meter.flush(&context, 2_000_000);
        assert!(!meter.contains_recorder(1));
        assert_eq!(writer.attempt_count(), 2);
    }

    // Re-registering during write clears the unregistered marker before cleanup.
    {
        let (meter, writer) = meter();
        let _installation = setup_meter_for_test(meter.clone());
        let recorder = RegisterRecorder(&task(1, ""));
        recorder.record_obj_store_get(1);
        UnregisterRecorder(1);
        assert!(meter.is_unregistered(1));
        let callback_meter = meter.clone();
        writer.set_hook(move |_, data| {
            check_meter_data(&[(GET_REQUESTS_FIELD, 1)], &data.items[0]);
            RegisterRecorder(&task(1, ""));
            assert!(!callback_meter.is_unregistered(1));
            Ok(())
        });
        meter.flush(&context, 1_000_000);
        assert!(meter.contains_recorder(1));
    }

    // Registering after a completed unregister starts every counter at zero.
    {
        let (meter, writer) = meter();
        let _installation = setup_meter_for_test(meter.clone());
        let first = RegisterRecorder(&task(1, ""));
        first.IncClusterReadBytes(123_456_789);
        writer.set_hook(|_, data| {
            check_meter_data(&[(CLUSTER_READ_BYTES_FIELD, 123_456_789)], &data.items[0]);
            UnregisterRecorder(1);
            Ok(())
        });
        meter.flush(&context, 1_000_000);
        assert!(meter.last_flushed_data(1).is_none());
        assert!(!meter.contains_recorder(1));

        let second = RegisterRecorder(&task(1, ""));
        assert!(!Arc::ptr_eq(&first, &second));
        second.IncClusterReadBytes(123);
        let callback_meter = meter.clone();
        writer.set_hook(move |_, data| {
            check_meter_data(&[(CLUSTER_READ_BYTES_FIELD, 123)], &data.items[0]);
            assert!(!callback_meter.is_unregistered(1));
            Ok(())
        });
        meter.flush(&context, 2_000_000);
        assert_eq!(
            meter
                .last_flushed_data(1)
                .unwrap()
                .values()
                .cluster_read_bytes,
            123
        );
        assert!(meter.contains_recorder(1));

        UnregisterRecorder(1);
        meter.flush(&context, 3_000_000);
        assert!(meter.last_flushed_data(1).is_none());
        assert!(!meter.contains_recorder(1));
    }
}

/// 覆盖正常增量 flush、无变化跳过，以及失败后快照推进与载荷重试分离。
#[test]
fn test_meter_flush() {
    let _lock = global_meter_lock().lock().unwrap();
    if kerneltype::IsClassic() {
        return;
    }
    let context = Context::background();

    // Normal flush writes all counters, skips an unchanged snapshot, then writes
    // only the newly accumulated field.
    {
        let (meter, writer) = meter();
        let _installation = setup_meter_for_test(meter.clone());
        let recorder = RegisterRecorder(&task(1, ""));
        recorder.record_obj_store_get(1);
        recorder.record_obj_store_put(2);
        recorder.record_obj_store_read(11);
        recorder.record_obj_store_write(22);
        recorder.IncClusterReadBytes(3);
        recorder.IncClusterWriteBytes(4);
        writer.set_hook(|_, data| {
            assert_eq!(data.items.len(), 1);
            check_meter_data(
                &[
                    (GET_REQUESTS_FIELD, 1),
                    (PUT_REQUESTS_FIELD, 2),
                    (OBJ_STORE_READ_BYTES_FIELD, 11),
                    (OBJ_STORE_WRITE_BYTES_FIELD, 22),
                    (CLUSTER_READ_BYTES_FIELD, 3),
                    (CLUSTER_WRITE_BYTES_FIELD, 4),
                ],
                &data.items[0],
            );
            Ok(())
        });
        meter.flush(&context, 1_000_000);
        assert_eq!(writer.attempt_count(), 1);

        meter.flush(&context, 2_000_000);
        assert_eq!(writer.attempt_count(), 1);

        recorder.record_obj_store_put(100);
        writer.set_hook(|_, data| {
            assert_eq!(data.items.len(), 1);
            check_meter_data(&[(PUT_REQUESTS_FIELD, 100)], &data.items[0]);
            Ok(())
        });
        meter.flush(&context, 3_000_000);
        assert_eq!(writer.attempt_count(), 2);
    }

    // A failed write still advances the snapshot; the exact failed payload is
    // retained for retry while the next regular flush sends only its increment.
    {
        let (meter, writer) = meter();
        let _installation = setup_meter_for_test(meter.clone());
        let recorder = RegisterRecorder(&task(1, ""));
        recorder.record_obj_store_get(1);
        assert!(meter.last_flushed_data(1).is_none());
        writer.set_hook(|_, _| Err(anyhow!("some err")));
        meter.flush(&context, 1_000_000);
        assert_eq!(meter.last_flushed_data(1).unwrap().values().get_requests, 1);
        assert_eq!(meter.pending_retry_len(), 1);

        recorder.record_obj_store_get(3);
        writer.set_hook(|_, data| {
            assert_eq!(data.items.len(), 1);
            check_meter_data(&[(GET_REQUESTS_FIELD, 3)], &data.items[0]);
            Ok(())
        });
        meter.flush(&context, 2_000_000);
        assert_eq!(meter.last_flushed_data(1).unwrap().values().get_requests, 4);
        assert_eq!(writer.attempt_count(), 2);
    }
}

/// 覆盖空队列、部分成功、多次失败后达 MAX_RETRY_COUNT 丢弃。
#[test]
fn test_meter_retry_write() {
    if kerneltype::IsClassic() {
        return;
    }
    let context = Context::background();

    let (empty_meter, empty_writer) = meter();
    assert_eq!(empty_meter.pending_retry_len(), 0);
    empty_meter.retry_write(&context);
    assert_eq!(empty_meter.pending_retry_len(), 0);
    assert_eq!(empty_writer.attempt_count(), 0);

    let (meter, writer) = meter();
    meter.add_failed_data(
        1_000_000,
        vec![MeterItem::from([(
            GET_REQUESTS_FIELD.to_owned(),
            MeterValue::from(12_u64),
        )])],
    );
    meter.add_failed_data(
        2_000_000,
        vec![MeterItem::from([(
            GET_REQUESTS_FIELD.to_owned(),
            MeterValue::from(23_u64),
        )])],
    );
    meter.add_failed_data(
        3_000_000,
        vec![MeterItem::from([(
            GET_REQUESTS_FIELD.to_owned(),
            MeterValue::from(34_u64),
        )])],
    );
    assert_eq!(meter.pending_retry_len(), 3);

    let mut third_retry_count = 0;
    writer.set_hook(move |_, data| {
        assert_eq!(data.items.len(), 1);
        match data.timestamp {
            1_000_000 => Err(anyhow!("some err")),
            2_000_000 => Ok(()),
            3_000_000 => {
                third_retry_count += 1;
                if third_retry_count < 5 {
                    Err(anyhow!("some err"))
                } else {
                    Ok(())
                }
            }
            timestamp => panic!("unexpected timestamp {timestamp}"),
        }
    });

    for retry_count in 1..=10 {
        meter.retry_write(&context);
        if retry_count < 5 {
            assert_eq!(meter.pending_retry_len(), 2);
        } else if retry_count == 10 {
            assert_eq!(meter.pending_retry_len(), 0);
        } else {
            assert_eq!(meter.pending_retry_len(), 1);
        }
        assert_eq!(writer.attempts_for(1_000_000), retry_count);
        assert_eq!(writer.attempts_for(2_000_000), 1);
        assert_eq!(writer.attempts_for(3_000_000), retry_count.min(5));
    }
    assert_eq!(writer.attempt_count(), 16);
}

/// StartFlushLoop 风格时间线测试中累计的写入/丢失统计。
#[derive(Default)]
struct FlushLoopWriteState {
    failures_by_timestamp: HashMap<i64, usize>,
    written_by_task: HashMap<i64, u64>,
    first_timestamp_for_task3: Option<i64>,
    lost_data_for_task3: u64,
}

/// 用确定性 flush/retry 时间线模拟 Go synctest；并验证取消后 Close。
#[test]
fn test_meter_start_flush_loop() {
    let _lock = global_meter_lock().lock().unwrap();
    if kerneltype::IsClassic() {
        return;
    }
    let context = Context::background();
    let (meter, writer) = meter();
    let _installation = setup_meter_for_test(meter.clone());
    let write_state = Arc::new(Mutex::new(FlushLoopWriteState::default()));
    let callback_state = write_state.clone();
    writer.set_hook(move |_, data| {
        let mut state = callback_state.lock().unwrap();
        let only_task3 = data.items.len() == 1 && value_i64(&data.items[0], "task_id") == Some(3);
        if only_task3 && state.first_timestamp_for_task3.is_none() {
            state.first_timestamp_for_task3 = Some(data.timestamp);
        }
        let failures = *state
            .failures_by_timestamp
            .get(&data.timestamp)
            .unwrap_or(&0);
        if failures < 5 {
            state
                .failures_by_timestamp
                .insert(data.timestamp, failures + 1);
            return Err(anyhow!("some err"));
        }
        if only_task3 && state.first_timestamp_for_task3 == Some(data.timestamp) {
            state.lost_data_for_task3 = value_u64(&data.items[0], GET_REQUESTS_FIELD).unwrap();
            return Err(anyhow!("some err"));
        }
        for item in &data.items {
            let task_id = value_i64(item, "task_id").unwrap();
            *state.written_by_task.entry(task_id).or_default() +=
                value_u64(item, GET_REQUESTS_FIELD).unwrap();
        }
        Ok(())
    });

    let recorder1 = RegisterRecorder(&task(1, ""));
    let recorder2 = RegisterRecorder(&task(2, ""));
    let mut timestamp = 1_000_000;
    // Go adds every 30 seconds and flushes every minute. Two additions per
    // synthetic minute preserve all 80 additions while avoiding wall-clock sleeps.
    for round in 0..80 {
        recorder1.record_obj_store_get(11);
        recorder2.record_obj_store_get(33);
        if round % 2 == 1 {
            meter.flush(&context, timestamp);
            for _ in 0..5 {
                meter.retry_write(&context);
            }
            assert_eq!(meter.pending_retry_len(), 0);
            timestamp += 60;
        }
    }
    UnregisterRecorder(1);
    UnregisterRecorder(2);
    meter.flush(&context, timestamp);
    assert!(!meter.contains_recorder(1));
    assert!(!meter.contains_recorder(2));
    timestamp += 600;

    let recorder3 = RegisterRecorder(&task(3, ""));
    // The first minute contains six 10-second additions and is permanently
    // failed, then dropped after the same ten retries as Go.
    for _ in 0..6 {
        recorder3.record_obj_store_get(23);
    }
    meter.flush(&context, timestamp);
    for _ in 0..10 {
        meter.retry_write(&context);
    }
    assert_eq!(meter.pending_retry_len(), 0);
    timestamp += 60;

    for _ in 0..4 {
        recorder3.record_obj_store_get(23);
    }
    meter.flush(&context, timestamp);
    for _ in 0..5 {
        meter.retry_write(&context);
    }
    assert_eq!(meter.pending_retry_len(), 0);
    UnregisterRecorder(3);
    meter.flush(&context, timestamp + 60);
    assert!(!meter.contains_recorder(3));

    let expected = HashMap::from([
        (1, recorder1.curr_data().values().get_requests),
        (2, recorder2.curr_data().values().get_requests),
        (3, recorder3.curr_data().values().get_requests),
    ]);
    assert_eq!(expected, HashMap::from([(1, 880), (2, 2_640), (3, 230)]));

    // Exercise StartFlushLoop's cancellation, final flush, retry-loop join, and
    // writer close without waiting for real time; the data/retry timeline above
    // is the deterministic equivalent of Go's synctest virtual clock.
    let cancelled = Context::background();
    cancelled.cancel();
    meter.clone().StartFlushLoop(cancelled);
    assert_eq!(writer.closes.load(Ordering::Relaxed), 1);

    let state = write_state.lock().unwrap();
    assert_eq!(state.written_by_task.len(), expected.len());
    assert_eq!(state.written_by_task[&1], expected[&1]);
    assert_eq!(state.written_by_task[&2], expected[&2]);
    assert!(state.lost_data_for_task3 > 0);
    assert_eq!(
        state.written_by_task[&3],
        expected[&3] - state.lost_data_for_task3
    );
}

/// 校验 Close 成功与失败路径均递增 closes 计数。
#[test]
fn test_meter_close() {
    if kerneltype::IsClassic() {
        return;
    }

    let (normal_meter, normal_writer) = meter();
    assert!(normal_meter.Close().is_ok());
    assert_eq!(normal_writer.closes.load(Ordering::Relaxed), 1);

    let (failing_meter, failing_writer) = meter();
    failing_writer.set_close_error(Some("some err"));
    let error = failing_meter.Close().unwrap_err();
    assert!(error.to_string().contains("some err"));
    assert_eq!(failing_writer.closes.load(Ordering::Relaxed), 1);
}

/// 将计量载荷写入本地目录的简单 Writer（按 timestamp 分子目录）。
struct LocalFileWriter {
    root: PathBuf,
}

impl MeteringWriter for LocalFileWriter {
    fn write(&self, context: &Context, data: MeteringData) -> Result<()> {
        assert!(context.deadline().is_some());
        let timestamp_dir = self.root.join(data.timestamp.to_string());
        fs::create_dir_all(&timestamp_dir)?;
        let path = timestamp_dir.join(format!("{}-{}.json", data.category, data.self_id));
        let items: Vec<HashMap<String, Value>> = data
            .items
            .iter()
            .map(|item| {
                item.iter()
                    .map(|(field, value)| {
                        let value = match value {
                            MeterValue::String(value) => Value::String(value.clone()),
                            MeterValue::I64(value) => Value::from(*value),
                            MeterValue::U64(value) => Value::from(*value),
                        };
                        (field.clone(), value)
                    })
                    .collect()
            })
            .collect();
        fs::write(path, serde_json::to_vec(&items)?)?;
        Ok(())
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

/// 从本地目录回读指定 timestamp 的计量 JSON。
struct LocalMeterReader {
    root: PathBuf,
}

/// 在临时目录上创建 Meter 与对应 Reader。
fn create_local_meter(dir: &Path) -> (Arc<Meter>, LocalMeterReader) {
    let root = dir.to_path_buf();
    let writer = Arc::new(LocalFileWriter { root: root.clone() });
    (Meter::with_writer(writer), LocalMeterReader { root })
}

/// 读取某 timestamp 目录下所有 dxf-*.json 并合并条目。
fn read_metering_data(reader: &LocalMeterReader, timestamp: i64) -> Vec<HashMap<String, Value>> {
    let timestamp_dir = reader.root.join(timestamp.to_string());
    if !timestamp_dir.exists() {
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = fs::read_dir(timestamp_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(CATEGORY))
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .flat_map(|path| {
            serde_json::from_slice::<Vec<HashMap<String, Value>>>(&fs::read(path).unwrap()).unwrap()
        })
        .collect()
}

/// 端到端：累计量、flush 到本地文件、回读字段与基础元数据。
#[test]
fn test_meter_simple_flush_and_read_back() {
    let _lock = global_meter_lock().lock().unwrap();
    if kerneltype::IsClassic() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let (meter, reader) = create_local_meter(directory.path());
    let _installation = setup_meter_for_test(meter.clone());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let write_time = now - now % 60 + 60;

    assert!(read_metering_data(&reader, write_time - 60).is_empty());
    let recorder = RegisterRecorder(&task(1, "ks1"));
    recorder.record_obj_store_get(10);
    recorder.record_obj_store_put(20);
    recorder.record_obj_store_read(11);
    recorder.record_obj_store_write(22);
    recorder.IncClusterReadBytes(300);
    recorder.IncClusterWriteBytes(400);
    meter.flush(&Context::background(), write_time);

    assert!(read_metering_data(&reader, write_time - 60).is_empty());
    let data = read_metering_data(&reader, write_time);
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["version"], Value::from("1"));
    assert_eq!(data[0]["cluster_id"], Value::from("ks1"));
    assert_eq!(data[0]["source_name"], Value::from("dxf"));
    assert_eq!(data[0][GET_REQUESTS_FIELD], Value::from(10_u64));
    assert_eq!(data[0][PUT_REQUESTS_FIELD], Value::from(20_u64));
    assert_eq!(data[0][OBJ_STORE_READ_BYTES_FIELD], Value::from(11_u64));
    assert_eq!(data[0][OBJ_STORE_WRITE_BYTES_FIELD], Value::from(22_u64));
    assert_eq!(data[0][CLUSTER_READ_BYTES_FIELD], Value::from(300_u64));
    assert_eq!(data[0][CLUSTER_WRITE_BYTES_FIELD], Value::from(400_u64));
    assert!(meter.Close().is_ok());
}
