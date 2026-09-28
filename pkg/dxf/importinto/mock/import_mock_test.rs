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

// 验证 `MockMiniTaskExecutor` 与 Go gomock 生成代码保持一致的公开契约。
//
// 测试控制器同时记录期望注册与实际调用，并通过轻量写入器、采集器检查
// `Run` 的参数透传、副作用和错误返回。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use crate::*;

#[derive(Default)]
/// 记录 gomock 适配层触发的事件、期望参数及实际分发参数。
struct TestController {
    events: Vec<&'static str>,
    recorded: Option<Arc<RecordedCall>>,
    expected_arguments: Vec<String>,
    dispatched_arguments: Option<(bool, bool, bool)>,
}

impl Controller for TestController {
    fn helper(&mut self) {
        self.events.push("helper");
    }

    fn call(
        &mut self,
        _receiver_id: u64,
        method: &'static str,
        arguments: RunArguments,
    ) -> Result<(), Error> {
        self.events.push(method);
        // 在控制器边界读取 trait 对象状态，确认两个引擎参数未被调换或丢失。
        let data_synced = arguments
            .data_engine
            .as_ref()
            .is_some_and(|writer| writer.IsSynced());
        let index_synced = arguments
            .index_engine
            .as_ref()
            .is_some_and(|writer| writer.IsSynced());
        if let Some(collector) = arguments.collector {
            // 模拟执行器汇报进度，以证明采集器被原样传入实际调用。
            collector.Processed(7, 3);
        }
        self.dispatched_arguments = Some((arguments.context.cancelled, data_synced, index_synced));
        Err(Error("run failed".to_owned()))
    }

    fn record_call_with_method_type(
        &mut self,
        receiver_id: u64,
        method: &'static str,
        method_type: &'static str,
        arguments: ExpectedRunArguments,
    ) -> Arc<RecordedCall> {
        self.events.push("record");
        // 期望参数是类型擦除值；测试控制器还原静态字符串以核对注册顺序。
        self.expected_arguments = [
            &arguments.context,
            &arguments.data_engine,
            &arguments.index_engine,
            &arguments.collector,
        ]
        .into_iter()
        .map(|argument| {
            argument
                .downcast_ref::<&'static str>()
                .expect("test matcher must be a static string")
                .to_string()
        })
        .collect();
        let call = Arc::new(RecordedCall {
            receiver_id,
            method,
            method_type,
            arguments,
        });
        self.recorded = Some(Arc::clone(&call));
        call
    }
}

#[derive(Default)]
/// 用同步标志区分数据引擎与索引引擎，便于验证分发顺序。
struct TestWriter {
    synced: bool,
}

impl EngineWriter for TestWriter {
    fn AppendRows(
        &mut self,
        _ctx: &Context,
        _column_names: &[String],
        _rows: &dyn Rows,
    ) -> Result<(), BackendError> {
        Ok(())
    }

    fn IsSynced(&self) -> bool {
        self.synced
    }

    fn Close(&mut self, _ctx: &Context) -> Result<Option<ChunkFlushStatus>, BackendError> {
        Ok(None)
    }
}

#[derive(Default)]
/// 以原子计数保存控制器回调产生的进度，供调用完成后断言。
struct TestCollector {
    processed: AtomicI64,
    rows: AtomicI64,
}

impl Collector for TestCollector {
    fn Accepted(&self, _bytes: i64) {}

    fn Processed(&self, processed_units: i64, rows: i64) {
        self.processed.fetch_add(processed_units, Ordering::SeqCst);
        self.rows.fetch_add(rows, Ordering::SeqCst);
    }
}

type Arguments = (
    Context,
    Option<Box<dyn EngineWriter>>,
    Option<Box<dyn EngineWriter>>,
    Option<Arc<dyn Collector + Send + Sync>>,
);

/// 构造带有可辨识状态的 `Run` 参数组。
fn arguments(collector: Arc<TestCollector>) -> Arguments {
    (
        Context::cancelled(),
        Some(Box::new(TestWriter { synced: true })),
        Some(Box::new(TestWriter { synced: false })),
        Some(collector),
    )
}

#[test]
/// 覆盖期望注册、实际分发、错误透传以及 Go gomock 的辅助调用顺序。
fn generated_gomock_public_contract_and_dispatch_order_match_go() {
    let controller = Arc::new(Mutex::new(TestController::default()));
    let mock = NewMockMiniTaskExecutor(controller.clone());

    assert_eq!(mock.ISGOMOCK(), ());

    let recorded = mock
        .EXPECT()
        .Run("ctx", "data", "index", "collector")
        .expect("record expectation");
    assert_eq!(recorded.method, "Run");
    assert_eq!(recorded.method_type, "MockMiniTaskExecutor::Run");

    let collector = Arc::new(TestCollector::default());
    let (ctx, data, index, collector_arg) = arguments(Arc::clone(&collector));
    let err = mock
        .Run(ctx, data, index, collector_arg)
        .expect_err("controller error must be returned");
    assert_eq!(err, Error("run failed".to_owned()));
    assert_eq!(collector.processed.load(Ordering::SeqCst), 7);
    assert_eq!(collector.rows.load(Ordering::SeqCst), 3);

    let controller = controller.lock().expect("test controller lock");
    assert_eq!(controller.events, ["helper", "record", "helper", "Run"]);
    assert_eq!(
        controller.expected_arguments,
        ["ctx", "data", "index", "collector"]
    );
    assert_eq!(controller.dispatched_arguments, Some((true, true, false)));
    assert!(Arc::ptr_eq(
        controller.recorded.as_ref().expect("recorded call"),
        &recorded
    ));
}
