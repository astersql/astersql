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
// limitations under the License.

// AsterSQL 迁移对照测试：压缩验证 data/recorder/Meter 与 Go 语义一致性。

use crate::data::{
    CLUSTER_READ_BYTES_FIELD, CLUSTER_WRITE_BYTES_FIELD, Data, DataValues, GET_REQUESTS_FIELD,
    MeterValue, OBJ_STORE_READ_BYTES_FIELD, OBJ_STORE_WRITE_BYTES_FIELD, PUT_REQUESTS_FIELD,
};
use crate::metering::{
    Context, Meter, MeteringConfig, MeteringData, MeteringWriter, RegisterRecorder, SetMetering,
    UnregisterRecorder, WriterFactory,
};
use crate::recorder::Recorder;
use anyhow::{Result, anyhow};
use proto::step::StepInit;
use proto::task::{ExtraParams, TaskBase, TaskStatePending};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// 可按时间戳注入有限次写失败的 mock Writer。
#[derive(Default)]
struct MockWriter {
    writes: Mutex<Vec<MeteringData>>,
    failures_left: Mutex<HashMap<i64, usize>>,
    closes: AtomicUsize,
}

/// 让指定时间戳的写入再失败 `times` 次。
impl MockWriter {
    fn fail(&self, ts: i64, times: usize) {
        self.failures_left.lock().unwrap().insert(ts, times);
    }

    /// 返回已成功写入的载荷。
    fn writes(&self) -> Vec<MeteringData> {
        self.writes.lock().unwrap().clone()
    }
}

impl MeteringWriter for MockWriter {
    fn write(&self, ctx: &Context, data: MeteringData) -> Result<()> {
        assert!(
            ctx.deadline().is_some(),
            "write context must carry the Go timeout"
        );
        let mut failures = self.failures_left.lock().unwrap();
        if let Some(left) = failures.get_mut(&data.timestamp)
            && *left > 0
        {
            *left -= 1;
            return Err(anyhow!("injected write failure"));
        }
        drop(failures);
        self.writes.lock().unwrap().push(data);
        Ok(())
    }

    fn close(&self) -> Result<()> {
        self.closes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// 断言 overwrite_existing 并复用同一 MockWriter 的工厂。
struct MockFactory {
    writer: Arc<MockWriter>,
    creates: AtomicUsize,
}

impl WriterFactory for MockFactory {
    fn create(&self, config: &MeteringConfig) -> Result<Arc<dyn MeteringWriter>> {
        assert!(config.overwrite_existing);
        self.creates.fetch_add(1, Ordering::Relaxed);
        Ok(self.writer.clone())
    }
}

/// 从条目取出 U64。
fn value_u64(item: &HashMap<String, MeterValue>, key: &str) -> Option<u64> {
    match item.get(key) {
        Some(MeterValue::U64(value)) => Some(*value),
        _ => None,
    }
}

/// 构造固定 keyspace=`ks` 的 Example 任务夹具。
fn task(id: i64) -> TaskBase {
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
        Keyspace: "ks".to_string(),
    }
}

/// 创建带 MockWriter 的 Meter。
fn meter() -> (Arc<Meter>, Arc<MockWriter>) {
    let writer = Arc::new(MockWriter::default());
    (Meter::with_writer(writer.clone()), writer)
}

/// 增量字段与 GetBaseMeterItem 基础字段对齐 Go。
#[test]
fn data_delta_and_base_fields_match_go() {
    let current = Data::new(
        1,
        "ks",
        "tt",
        DataValues {
            get_requests: 10,
            put_requests: 20,
            obj_store_read_bytes: 300,
            obj_store_write_bytes: 400,
            cluster_read_bytes: 500,
            cluster_write_bytes: 600,
        },
    );
    assert!(current.equals(&current));
    assert!(current.cal_meter_data_item(&current).is_none());

    let previous = Data::new(
        99,
        "ignored",
        "ignored",
        DataValues {
            get_requests: 5,
            put_requests: 5,
            obj_store_read_bytes: 100,
            obj_store_write_bytes: 100,
            cluster_read_bytes: 200,
            cluster_write_bytes: 200,
        },
    );
    let item = current.cal_meter_data_item(&previous).unwrap();
    assert_eq!(value_u64(&item, GET_REQUESTS_FIELD), Some(5));
    assert_eq!(value_u64(&item, PUT_REQUESTS_FIELD), Some(15));
    assert_eq!(value_u64(&item, OBJ_STORE_READ_BYTES_FIELD), Some(200));
    assert_eq!(value_u64(&item, OBJ_STORE_WRITE_BYTES_FIELD), Some(300));
    assert_eq!(value_u64(&item, CLUSTER_READ_BYTES_FIELD), Some(300));
    assert_eq!(value_u64(&item, CLUSTER_WRITE_BYTES_FIELD), Some(400));
    assert_eq!(item["version"], MeterValue::String("1".to_string()));
    assert_eq!(item["source_name"], MeterValue::String("dxf".to_string()));
    assert_eq!(item["cluster_id"], MeterValue::String("ks".to_string()));
    assert_eq!(item["task_type"], MeterValue::String("tt".to_string()));
    assert_eq!(item["task_id"], MeterValue::I64(1));

    let only_get = current
        .cal_meter_data_item(&Data::new(
            0,
            "",
            "",
            DataValues {
                get_requests: 5,
                ..current.values().clone()
            },
        ))
        .unwrap();
    assert_eq!(value_u64(&only_get, GET_REQUESTS_FIELD), Some(5));
    assert!(!only_get.contains_key(PUT_REQUESTS_FIELD));
}

#[test]
/// Go 的 uint64 相减按机器无符号语义环绕；畸形倒退快照也不得改成饱和归零。
fn data_delta_preserves_go_uint64_wraparound() {
    let current = Data::new(
        1,
        "ks",
        "tt",
        DataValues {
            get_requests: 4,
            ..Default::default()
        },
    );
    let previous = Data::new(
        1,
        "ks",
        "tt",
        DataValues {
            get_requests: 5,
            ..Default::default()
        },
    );

    let item = current.cal_meter_data_item(&previous).unwrap();
    assert_eq!(value_u64(&item, GET_REQUESTS_FIELD), Some(u64::MAX),);
}

/// Recorder 快照与 Display 格式对齐 Go。
#[test]
fn recorder_snapshot_and_display_match_go() {
    let recorder = Recorder::new(1, "ks", "tt");
    recorder.record_obj_store_get(100);
    recorder.record_obj_store_put(200);
    recorder.record_obj_store_read(11);
    recorder.record_obj_store_write(22);
    recorder.IncClusterReadBytes(300);
    recorder.IncClusterWriteBytes(400);

    let data = recorder.curr_data();
    assert_eq!(
        data.values(),
        &DataValues {
            get_requests: 100,
            put_requests: 200,
            obj_store_read_bytes: 11,
            obj_store_write_bytes: 22,
            cluster_read_bytes: 300,
            cluster_write_bytes: 400,
        }
    );
    assert!(data.to_string().contains("requests{get: 100, put: 200}"));
    assert!(data.to_string().contains("cluster{r: 300B, w: 400B}"));

    let formatted = Data::new(
        1,
        "ks",
        "tt",
        DataValues {
            obj_store_read_bytes: 1024,
            obj_store_write_bytes: 1536,
            cluster_read_bytes: 1_000_000,
            cluster_write_bytes: u64::MAX,
            ..Default::default()
        },
    )
    .to_string();
    assert!(formatted.contains("obj_store{r: 1KiB, w: 1.5KiB}"));
    assert!(formatted.contains("cluster{r: 976.6KiB, w: 16EiB}"));
}

/// 空配置禁用；合法配置创建 Writer 且 overwrite=true。
#[test]
fn factory_preserves_empty_config_and_overwrite_behavior() {
    let writer = Arc::new(MockWriter::default());
    let factory = MockFactory {
        writer,
        creates: AtomicUsize::new(0),
    };
    assert!(
        Meter::new(&MeteringConfig::default(), &factory)
            .unwrap()
            .is_none()
    );
    assert_eq!(factory.creates.load(Ordering::Relaxed), 0);

    let config = MeteringConfig::new("s3", "bucket");
    assert!(Meter::new(&config, &factory).unwrap().is_some());
    assert_eq!(factory.creates.load(Ordering::Relaxed), 1);
}

/// 同 task_id 复用、注销标记清除、最终 flush 后重新注册计数归零。
#[test]
fn register_unregister_and_reregister_match_go() {
    let (meter, _) = meter();
    let first = meter.get_or_register_recorder(Recorder::new(1, "ks", "tt"));
    let second = meter.get_or_register_recorder(Recorder::new(1, "other", "other"));
    assert!(Arc::ptr_eq(&first, &second));

    first.record_obj_store_get(1);
    meter.unregister_recorder(1);
    assert!(meter.is_unregistered(1));
    let third = meter.get_or_register_recorder(Recorder::new(1, "new", "new"));
    assert!(Arc::ptr_eq(&first, &third));
    assert!(!meter.is_unregistered(1));

    meter.unregister_recorder(1);
    meter.flush(&Context::background(), 1);
    assert!(!meter.contains_recorder(1));
    let fresh = meter.get_or_register_recorder(Recorder::new(1, "ks", "tt"));
    assert!(!Arc::ptr_eq(&first, &fresh));
    assert_eq!(fresh.curr_data().values().get_requests, 0);
}

/// 只上报增量；写失败仍推进快照，失败载荷按原时间戳重试。
#[test]
fn flush_reports_only_increments_and_retains_failed_payload_for_retry() {
    let (meter, writer) = meter();
    let recorder = meter.get_or_register_recorder(Recorder::new(1, "ks", "tt"));
    recorder.record_obj_store_get(1);
    recorder.record_obj_store_put(2);
    recorder.record_obj_store_read(11);
    recorder.record_obj_store_write(22);
    recorder.IncClusterReadBytes(3);
    recorder.IncClusterWriteBytes(4);
    meter.flush(&Context::background(), 1_000_000);

    let writes = writer.writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].items.len(), 1);
    assert_eq!(value_u64(&writes[0].items[0], GET_REQUESTS_FIELD), Some(1));
    assert_eq!(value_u64(&writes[0].items[0], PUT_REQUESTS_FIELD), Some(2));

    meter.flush(&Context::background(), 2_000_000);
    assert_eq!(
        writer.writes().len(),
        1,
        "unchanged counters must not write"
    );

    recorder.record_obj_store_get(3);
    writer.fail(3_000_000, 1);
    meter.flush(&Context::background(), 3_000_000);
    assert_eq!(meter.pending_retry_len(), 1);
    assert_eq!(meter.last_flushed_data(1).unwrap().values().get_requests, 4);

    recorder.record_obj_store_get(5);
    meter.flush(&Context::background(), 4_000_000);
    let writes = writer.writes();
    assert_eq!(
        value_u64(&writes.last().unwrap().items[0], GET_REQUESTS_FIELD),
        Some(5)
    );

    meter.retry_write(&Context::background());
    assert_eq!(meter.pending_retry_len(), 0);
    let retried = writer
        .writes()
        .into_iter()
        .find(|data| data.timestamp == 3_000_000)
        .unwrap();
    assert_eq!(value_u64(&retried.items[0], GET_REQUESTS_FIELD), Some(3));
}

/// 重试达 Go 上限才丢弃；成功项从 pending 移除。
#[test]
fn retry_drops_only_after_go_maximum_and_removes_successes() {
    let (meter, writer) = meter();
    writer.fail(1, 20);
    writer.fail(2, 0);
    writer.fail(3, 4);
    meter.add_failed_data(
        1,
        vec![HashMap::from([(
            GET_REQUESTS_FIELD.to_string(),
            MeterValue::U64(12),
        )])],
    );
    meter.add_failed_data(
        2,
        vec![HashMap::from([(
            GET_REQUESTS_FIELD.to_string(),
            MeterValue::U64(23),
        )])],
    );
    meter.add_failed_data(
        3,
        vec![HashMap::from([(
            GET_REQUESTS_FIELD.to_string(),
            MeterValue::U64(34),
        )])],
    );

    for round in 1..=10 {
        meter.retry_write(&Context::background());
        if round < 5 {
            assert_eq!(meter.pending_retry_len(), 2);
        } else if round < 10 {
            assert_eq!(meter.pending_retry_len(), 1);
        } else {
            assert_eq!(meter.pending_retry_len(), 0);
        }
    }
}

/// 全局 API + 已取消的 StartFlushLoop 应关闭 Writer 并清理 recorder。
#[test]
fn global_api_and_flush_loop_close_writer() {
    let _lock = crate::metering_test::global_meter_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (meter, writer) = meter();
    SetMetering(Some(meter.clone()));
    let recorder = RegisterRecorder(&task(7));
    recorder.record_obj_store_get(9);
    UnregisterRecorder(7);

    let context = Context::background();
    context.cancel();
    meter.clone().StartFlushLoop(context);
    assert_eq!(writer.closes.load(Ordering::Relaxed), 1);
    assert!(!meter.contains_recorder(7));
    SetMetering(None);
}

/// Context::wait 可被 cancel 打断并返回 true。
#[test]
fn context_wait_is_interruptible() {
    let context = Context::background();
    let cloned = context.clone();
    let join = std::thread::spawn(move || cloned.wait(Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(10));
    context.cancel();
    assert!(join.join().unwrap());
}
