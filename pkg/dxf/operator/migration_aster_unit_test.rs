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

// Aster 迁移单元测试：覆盖 Compose 通道、管道 Open 回滚、Close 首错、
// reader/writer 暴露，以及真实 Source→Transform→Sink 流水线与 Context 取消。

use astersql_dxf_operator::compose::{
    Compose, DataChannel, NewSimpleDataChannel, WithSink, WithSource,
};
use astersql_dxf_operator::operator::{Operator, TunableOperator};
use astersql_dxf_operator::pipeline::NewAsyncPipeline;
use astersql_dxf_operator::workerpool::{Context, Error, TaskMayPanic};
use astersql_dxf_operator::wrapper::{NewSimpleDataSource, newSimpleOperator, newSimpleSink};
use std::sync::{Arc, Mutex};

/// 可在 worker 池中传递的整型任务载荷。
#[derive(Clone, Debug, Eq, PartialEq)]
struct NumberTask(i32);

impl TaskMayPanic for NumberTask {
    fn RecoverArgs(&self) -> (String, String, Option<Error>) {
        (String::new(), String::new(), None)
    }
}

/// 仅实现 WithSink，用于捕获 Compose 注入的上游通道。
#[derive(Default)]
struct SinkEndpoint<T> {
    channel: Option<astersql_dxf_operator::compose::SimpleDataChannel<T>>,
}

impl<T> WithSink<T> for SinkEndpoint<T> {
    fn SetSink(&mut self, channel: astersql_dxf_operator::compose::SimpleDataChannel<T>) {
        self.channel = Some(channel);
    }
}

/// 仅实现 WithSource，用于捕获 Compose 注入的下游通道。
#[derive(Default)]
struct SourceEndpoint<T> {
    channel: Option<astersql_dxf_operator::compose::SimpleDataChannel<T>>,
}

impl<T> WithSource<T> for SourceEndpoint<T> {
    fn SetSource(&mut self, channel: astersql_dxf_operator::compose::SimpleDataChannel<T>) {
        self.channel = Some(channel);
    }
}

/// 验证 Compose 共享无缓冲通道，且 Finish 能关闭通道。
#[test]
fn compose_shares_an_unbuffered_channel_and_finish_closes_it() {
    let mut upstream = SinkEndpoint::<i32>::default();
    let mut downstream = SourceEndpoint::<i32>::default();
    Compose(&mut upstream, &mut downstream);

    let sender = upstream.channel.take().expect("upstream sink channel");
    let receiver = downstream
        .channel
        .take()
        .expect("downstream source channel");
    // 无缓冲：发送与接收需并发，否则会死锁。
    let send_thread = std::thread::spawn(move || sender.Channel().send(42));
    assert_eq!(receiver.Channel().recv(), Some(42));
    assert!(send_thread.join().expect("sender thread"));

    receiver.Finish();
    assert!(receiver.Channel().is_closed());
    assert_eq!(receiver.Channel().recv(), None);

    let explicit = NewSimpleDataChannel(
        astersql_dxf_operator::workerpool::Channel::<i32>::bounded(1),
    );
    assert!(!explicit.Channel().is_closed());
}

/// 记录 Open/Close 事件，并可注入错误的测试算子。
struct RecordingOperator {
    name: &'static str,
    events: Arc<Mutex<Vec<String>>>,
    open_error: Option<Error>,
    close_error: Option<Error>,
}

impl RecordingOperator {
    fn new(name: &'static str, events: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            name,
            events,
            open_error: None,
            close_error: None,
        }
    }
}

impl Operator for RecordingOperator {
    fn Open(&mut self) -> Result<(), Error> {
        self.events
            .lock()
            .unwrap()
            .push(format!("open:{}", self.name));
        match &self.open_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn Close(&mut self) -> Result<(), Error> {
        self.events
            .lock()
            .unwrap()
            .push(format!("close:{}", self.name));
        match &self.close_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn String(&self) -> String {
        self.name.to_owned()
    }
}

/// Execute 中途 Open 失败时，应对已打开算子按逆序 Close。
#[test]
fn execute_rolls_back_opened_operators_in_reverse_order() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let first = RecordingOperator::new("first", Arc::clone(&events));
    let second = RecordingOperator::new("second", Arc::clone(&events));
    let mut failing = RecordingOperator::new("failing", Arc::clone(&events));
    failing.open_error = Some(Error::new("open failed"));
    let never_opened = RecordingOperator::new("never", Arc::clone(&events));
    let mut pipeline = NewAsyncPipeline(vec![
        Box::new(first),
        Box::new(second),
        Box::new(failing),
        Box::new(never_opened),
    ]);

    assert_eq!(pipeline.Execute().unwrap_err().message(), "open failed");
    assert!(!pipeline.IsStarted());
    // 期望：open first/second/failing，然后 close second、first（逆序回滚）。
    assert_eq!(
        *events.lock().unwrap(),
        [
            "open:first",
            "open:second",
            "open:failing",
            "close:second",
            "close:first"
        ]
    );
}

/// Close 应访问每个算子，并只返回第一个关闭错误。
#[test]
fn close_visits_every_operator_and_returns_the_first_error() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut first = RecordingOperator::new("first", Arc::clone(&events));
    first.close_error = Some(Error::new("first close error"));
    let mut second = RecordingOperator::new("second", Arc::clone(&events));
    second.close_error = Some(Error::new("second close error"));
    let mut pipeline = NewAsyncPipeline(vec![Box::new(first), Box::new(second)]);

    pipeline.Execute().unwrap();
    assert!(pipeline.IsStarted());
    assert_eq!(pipeline.String(), "AsyncPipeline[first -> second]");
    assert_eq!(pipeline.Close().unwrap_err().message(), "first close error");
    assert!(!pipeline.IsStarted());
    assert!(
        events
            .lock()
            .unwrap()
            .ends_with(&["close:first".into(), "close:second".into()])
    );
}

/// 在 RecordingOperator 外包一层 TunableOperator，用于 reader/writer 测试。
struct TunableRecordingOperator {
    inner: RecordingOperator,
    size: i32,
}

impl Operator for TunableRecordingOperator {
    fn Open(&mut self) -> Result<(), Error> {
        self.inner.Open()
    }

    fn Close(&mut self) -> Result<(), Error> {
        self.inner.Close()
    }

    fn String(&self) -> String {
        self.inner.String()
    }

    fn as_tunable_operator(&self) -> Option<&dyn TunableOperator> {
        Some(self)
    }

    fn as_tunable_operator_mut(&mut self) -> Option<&mut dyn TunableOperator> {
        Some(self)
    }
}

impl TunableOperator for TunableRecordingOperator {
    fn TuneWorkerPoolSize(&mut self, worker_num: i32, _wait: bool) {
        self.size = worker_num;
    }

    fn GetWorkerPoolSize(&self) -> i32 {
        self.size
    }
}

/// 仅四算子管道暴露第 2/3 个为 reader/writer；其它长度返回 (None, None)。
#[test]
fn reader_and_writer_are_only_exposed_for_four_operator_pipelines() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let plain =
        || Box::new(RecordingOperator::new("plain", Arc::clone(&events))) as Box<dyn Operator>;
    let tunable = |name, size| {
        Box::new(TunableRecordingOperator {
            inner: RecordingOperator::new(name, Arc::clone(&events)),
            size,
        }) as Box<dyn Operator>
    };
    let mut four = NewAsyncPipeline(vec![
        plain(),
        tunable("reader", 3),
        tunable("writer", 5),
        plain(),
    ]);
    let (reader, writer) = four.GetReaderAndWriter();
    let reader = reader.unwrap();
    let writer = writer.unwrap();
    assert_eq!(reader.GetWorkerPoolSize(), 3);
    assert_eq!(writer.GetWorkerPoolSize(), 5);
    reader.TuneWorkerPoolSize(7, true);
    writer.TuneWorkerPoolSize(9, false);
    assert_eq!(reader.GetWorkerPoolSize(), 7);
    assert_eq!(writer.GetWorkerPoolSize(), 9);

    let mut three = NewAsyncPipeline(vec![plain(), plain(), plain()]);
    assert!(matches!(three.GetReaderAndWriter(), (None, None)));
}

/// 真实 Source→Transform→Sink 流水线应处理每一项输入。
#[test]
fn real_source_transform_and_sink_pipeline_processes_every_item() {
    let context = Context::background();
    let collected = Arc::new(Mutex::new(Vec::new()));
    let mut source = NewSimpleDataSource(
        context.clone(),
        vec![NumberTask(1), NumberTask(2), NumberTask(3), NumberTask(4)],
    );
    let mut transform = newSimpleOperator(context.clone(), |task: NumberTask| task.0 * 10, 2);
    let sink_values = Arc::clone(&collected);
    let mut sink = newSimpleSink(context, move |value| {
        sink_values.lock().unwrap().push(value)
    });
    Compose(&mut source, &mut transform);
    Compose(&mut transform, &mut sink);

    let mut pipeline =
        NewAsyncPipeline(vec![Box::new(source), Box::new(transform), Box::new(sink)]);
    pipeline.Execute().unwrap();
    pipeline.Close().unwrap();

    // 并发 transform 不保证顺序，排序后再断言。
    let mut actual = collected.lock().unwrap().clone();
    actual.sort_unstable();
    assert_eq!(actual, [10, 20, 30, 40]);
}

/// Context 上报错误后，Close 应带回该错误（管道被取消）。
#[test]
fn context_error_cancels_the_pipeline_and_close_reports_it() {
    let context = Context::background();
    let mut source = NewSimpleDataSource(
        context.clone(),
        (0..100).map(NumberTask).collect::<Vec<_>>(),
    );
    let transform_context = context.clone();
    let mut transform = newSimpleOperator(
        context.clone(),
        move |task: NumberTask| {
            // 在 transform 中注入错误，模拟运行期失败取消。
            transform_context.OnError(Error::new("mock error for testing"));
            task.0
        },
        1,
    );
    let mut sink = newSimpleSink(context, |_| {});
    Compose(&mut source, &mut transform);
    Compose(&mut transform, &mut sink);

    let mut pipeline =
        NewAsyncPipeline(vec![Box::new(source), Box::new(transform), Box::new(sink)]);
    pipeline.Execute().unwrap();
    assert_eq!(
        pipeline.Close().unwrap_err().message(),
        "mock error for testing"
    );
}
