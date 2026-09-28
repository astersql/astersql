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

// Safe Rust counterpart of the generated GoMock `MiniTaskExecutor` mock.
//
// 手写 GoMock 风格的 MiniTaskExecutor mock：通过 Controller 记录期望调用并派发 Run，
// 供 IMPORT INTO 编码/排序管线单测注入假执行器。

use std::any::Any;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub use astersql_dxf_framework_taskexecutor_execute::Collector;
pub use astersql_lightning_backend::{BackendError, ChunkFlushStatus, EngineWriter};
pub use astersql_lightning_backend_encode::{Context, Rows};

/// 分配给每个 mock 接收者的递增 ID。
static NEXT_RECEIVER_ID: AtomicU64 = AtomicU64::new(1);

/// Mock 路径上的简易错误类型（消息字符串）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// 一次真实 `Run` 调用的完整参数。
pub struct RunArguments {
    pub context: Context,
    pub data_engine: Option<Box<dyn EngineWriter>>,
    pub index_engine: Option<Box<dyn EngineWriter>>,
    pub collector: Option<Arc<dyn Collector + Send + Sync>>,
}

/// Recorder 收到的四个期望参数，对应 Go 生成代码中的四个 `any`。
pub struct ExpectedRunArguments {
    pub context: Arc<dyn Any + Send + Sync>,
    pub data_engine: Arc<dyn Any + Send + Sync>,
    pub index_engine: Arc<dyn Any + Send + Sync>,
    pub collector: Arc<dyn Any + Send + Sync>,
}

/// Controller 记录下的一次方法调用。
pub struct RecordedCall {
    pub receiver_id: u64,
    pub method: &'static str,
    pub method_type: &'static str,
    pub arguments: ExpectedRunArguments,
}

/// Controller mirrors the four GoMock operations used by generated code:
/// helper marking, runtime dispatch, expectation recording, and return error.
/// 镜像 GoMock 的 helper 标记、运行时派发、期望记录与返回错误四类操作。
pub trait Controller: Send {
    /// 标记当前为测试 helper 调用栈（对应 GoMock ctrl.T.Helper）。
    fn helper(&mut self);
    /// 按期望派发方法调用，可返回预设错误。
    fn call(
        &mut self,
        receiver_id: u64,
        method: &'static str,
        arguments: RunArguments,
    ) -> Result<(), Error>;
    /// 记录一次带方法类型信息的期望调用。
    fn record_call_with_method_type(
        &mut self,
        receiver_id: u64,
        method: &'static str,
        method_type: &'static str,
        arguments: ExpectedRunArguments,
    ) -> Arc<RecordedCall>;
}

/// Mini 任务执行器接口：对单个 chunk 执行编码/写入。
pub trait MiniTaskExecutor {
    fn run(
        &self,
        context: Context,
        data_engine: Option<Box<dyn EngineWriter>>,
        index_engine: Option<Box<dyn EngineWriter>>,
        collector: Option<Arc<dyn Collector + Send + Sync>>,
    ) -> Result<(), Error>;
}

/// GoMock 风格的 MiniTaskExecutor 实现，委托给共享 Controller。
pub struct MockMiniTaskExecutor {
    receiver_id: u64,
    controller: Arc<Mutex<dyn Controller>>,
    recorder: MockMiniTaskExecutorMockRecorder,
}

/// 期望录制器：通过 `EXPECT().Run(...)` 登记调用。
pub struct MockMiniTaskExecutorMockRecorder {
    receiver_id: u64,
    controller: Arc<Mutex<dyn Controller>>,
}

/// 创建绑定给定 Controller 的 mock 执行器，并分配唯一 receiver_id。
pub fn new_mock_mini_task_executor(controller: Arc<Mutex<dyn Controller>>) -> MockMiniTaskExecutor {
    NewMockMiniTaskExecutor(controller)
}

/// `NewMockMiniTaskExecutor` creates a new mock instance.
pub fn NewMockMiniTaskExecutor(controller: Arc<Mutex<dyn Controller>>) -> MockMiniTaskExecutor {
    let receiver_id = NEXT_RECEIVER_ID.fetch_add(1, Ordering::Relaxed);
    let recorder = MockMiniTaskExecutorMockRecorder {
        receiver_id,
        controller: Arc::clone(&controller),
    };
    MockMiniTaskExecutor {
        receiver_id,
        controller,
        recorder,
    }
}

impl MockMiniTaskExecutor {
    /// 返回期望录制器（对应 Go `EXPECT()`）。
    pub fn expect(&self) -> &MockMiniTaskExecutorMockRecorder {
        self.EXPECT()
    }

    /// `EXPECT` returns an object that allows the caller to indicate expected use.
    pub fn EXPECT(&self) -> &MockMiniTaskExecutorMockRecorder {
        &self.recorder
    }

    /// GoMock 标记方法，表明本类型为生成 mock。
    pub fn is_gomock(&self) {
        self.ISGOMOCK()
    }

    /// `ISGOMOCK` indicates that this struct is a GoMock-style mock.
    pub fn ISGOMOCK(&self) {}

    /// `Run` mocks the base method and returns the controller's configured error.
    pub fn Run(
        &self,
        context: Context,
        data_engine: Option<Box<dyn EngineWriter>>,
        index_engine: Option<Box<dyn EngineWriter>>,
        collector: Option<Arc<dyn Collector + Send + Sync>>,
    ) -> Result<(), Error> {
        let mut controller = self
            .controller
            .lock()
            .map_err(|_| Error("mock controller lock poisoned".into()))?;
        controller.helper();
        controller.call(
            self.receiver_id,
            "Run",
            RunArguments {
                context,
                data_engine,
                index_engine,
                collector,
            },
        )
    }
}

impl MiniTaskExecutor for MockMiniTaskExecutor {
    fn run(
        &self,
        context: Context,
        data_engine: Option<Box<dyn EngineWriter>>,
        index_engine: Option<Box<dyn EngineWriter>>,
        collector: Option<Arc<dyn Collector + Send + Sync>>,
    ) -> Result<(), Error> {
        self.Run(context, data_engine, index_engine, collector)
    }
}

impl MockMiniTaskExecutorMockRecorder {
    /// 登记对 `Run` 的期望，返回 RecordedCall 供断言。
    pub fn run<A0, A1, A2, A3>(
        &self,
        context: A0,
        data_engine: A1,
        index_engine: A2,
        collector: A3,
    ) -> Result<Arc<RecordedCall>, Error>
    where
        A0: Any + Send + Sync,
        A1: Any + Send + Sync,
        A2: Any + Send + Sync,
        A3: Any + Send + Sync,
    {
        self.Run(context, data_engine, index_engine, collector)
    }

    /// `Run` records an expected call of the base method.
    pub fn Run<A0, A1, A2, A3>(
        &self,
        context: A0,
        data_engine: A1,
        index_engine: A2,
        collector: A3,
    ) -> Result<Arc<RecordedCall>, Error>
    where
        A0: Any + Send + Sync,
        A1: Any + Send + Sync,
        A2: Any + Send + Sync,
        A3: Any + Send + Sync,
    {
        let mut controller = self
            .controller
            .lock()
            .map_err(|_| Error("mock controller lock poisoned".into()))?;
        controller.helper();
        Ok(controller.record_call_with_method_type(
            self.receiver_id,
            "Run",
            "MockMiniTaskExecutor::Run",
            ExpectedRunArguments {
                context: Arc::new(context),
                data_engine: Arc::new(data_engine),
                index_engine: Arc::new(index_engine),
                collector: Arc::new(collector),
            },
        ))
    }
}
