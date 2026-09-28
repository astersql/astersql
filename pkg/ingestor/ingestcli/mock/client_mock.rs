// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// GoMock 风格的 ingestcli Client / WriteClient 手写替身。
//
// 通过 EXPECT 排队 matcher+responder，调用时出队消费；verify 检查未消费期望与失败记录。
// 用于上层单测，避免真实 HTTP/TiKV。SST：有序键值文件；ingest：导入到 Region。

use astersql_ingestor_ingestcli as ingestcli;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

type IngestMatcher = Box<dyn Fn(&ingestcli::IngestRequest) -> bool + Send>;
type IngestResponder =
    Box<dyn FnOnce(ingestcli::IngestRequest) -> Result<(), ingestcli::Error> + Send>;
type WriteClientResponder =
    Box<dyn FnOnce() -> Result<Box<dyn ingestcli::WriteClient>, ingestcli::Error> + Send>;
type WriteMatcher = Box<dyn Fn(&ingestcli::WriteRequest) -> bool + Send>;
type WriteResponder =
    Box<dyn FnOnce(ingestcli::WriteRequest) -> Result<(), ingestcli::Error> + Send>;
type RecvResponder = Box<dyn FnOnce() -> Result<ingestcli::WriteResponse, ingestcli::Error> + Send>;

/// 一条 Ingest 期望：请求匹配器 + 一次性响应器。
struct IngestExpectation {
    matcher: IngestMatcher,
    responder: IngestResponder,
}

/// 一条 WriteClient 期望：可选 commit_ts 校验 + 工厂响应器。
struct WriteClientExpectation {
    commit_ts: Option<u64>,
    responder: WriteClientResponder,
}

/// MockClient 已记录的调用种类。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientCall {
    Ingest {
        context_cancelled: bool,
        request: ingestcli::IngestRequest,
    },
    WriteClient {
        context_cancelled: bool,
        commit_ts: u64,
    },
}

/// MockWriteClient 已记录的调用种类。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WriteClientCall {
    Write(ingestcli::WriteRequest),
    Recv,
    Close,
}

/// verify 失败时汇总的多条消息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationError {
    pub messages: Vec<String>,
}

impl std::fmt::Display for VerificationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.messages.join("; "))
    }
}

impl std::error::Error for VerificationError {}

/// MockClient 内部可变状态。
#[derive(Default)]
struct ClientState {
    ingest: VecDeque<IngestExpectation>,
    write_client: VecDeque<WriteClientExpectation>,
    calls: Vec<ClientCall>,
    failures: Vec<String>,
}

/// MockClient is a safe Rust equivalent of the generated GoMock client. Each
/// call consumes one queued expectation and all calls remain inspectable.
/// 安全的 GoMock Client 等价物：每次调用消费一条期望，调用历史可查询。
#[derive(Clone, Default)]
pub struct MockClient {
    state: Arc<Mutex<ClientState>>,
}

/// MockClient 的 EXPECT 录音器，用于排队 Ingest / WriteClient 期望。
#[derive(Clone)]
pub struct MockClientMockRecorder {
    state: Arc<Mutex<ClientState>>,
}

/// 构造空期望的 MockClient。
pub fn NewMockClient() -> MockClient {
    MockClient::default()
}

impl MockClient {
    /// 返回用于排队期望的 recorder。
    pub fn EXPECT(&self) -> MockClientMockRecorder {
        MockClientMockRecorder {
            state: self.state.clone(),
        }
    }

    /// GoMock 兼容空方法（标识本对象为 mock）。
    pub fn ISGOMOCK(&self) {}

    /// 返回已记录调用副本。
    pub fn calls(&self) -> Vec<ClientCall> {
        self.state
            .lock()
            .expect("mock client lock poisoned")
            .calls
            .clone()
    }

    /// 检查失败记录与未消费期望；全部清空则 Ok。
    pub fn verify(&self) -> Result<(), VerificationError> {
        let state = self.state.lock().expect("mock client lock poisoned");
        let mut messages = state.failures.clone();
        if !state.ingest.is_empty() {
            messages.push(format!(
                "{} Ingest expectation(s) not called",
                state.ingest.len()
            ));
        }
        if !state.write_client.is_empty() {
            messages.push(format!(
                "{} WriteClient expectation(s) not called",
                state.write_client.len()
            ));
        }
        if messages.is_empty() {
            Ok(())
        } else {
            Err(VerificationError { messages })
        }
    }
}

impl MockClientMockRecorder {
    /// 排队一条 Ingest 期望。
    pub fn Ingest(
        &self,
        matcher: impl Fn(&ingestcli::IngestRequest) -> bool + Send + 'static,
        responder: impl FnOnce(ingestcli::IngestRequest) -> Result<(), ingestcli::Error>
        + Send
        + 'static,
    ) -> &Self {
        self.state
            .lock()
            .expect("mock client lock poisoned")
            .ingest
            .push_back(IngestExpectation {
                matcher: Box::new(matcher),
                responder: Box::new(responder),
            });
        self
    }

    /// 排队一条 WriteClient 期望；commit_ts 为 Some 时校验相等。
    pub fn WriteClient(
        &self,
        commit_ts: Option<u64>,
        responder: impl FnOnce() -> Result<Box<dyn ingestcli::WriteClient>, ingestcli::Error>
        + Send
        + 'static,
    ) -> &Self {
        self.state
            .lock()
            .expect("mock client lock poisoned")
            .write_client
            .push_back(WriteClientExpectation {
                commit_ts,
                responder: Box::new(responder),
            });
        self
    }
}

impl ingestcli::Client for MockClient {
    fn write_client(
        &self,
        context: &dyn ingestcli::RequestContext,
        commit_ts: u64,
    ) -> Result<Box<dyn ingestcli::WriteClient>, ingestcli::Error> {
        let expectation = {
            let mut state = self.state.lock().expect("mock client lock poisoned");
            state.calls.push(ClientCall::WriteClient {
                context_cancelled: context.is_cancelled(),
                commit_ts,
            });
            let position = state.write_client.iter().position(|expectation| {
                expectation
                    .commit_ts
                    .is_none_or(|expected| expected == commit_ts)
            });
            position.and_then(|position| state.write_client.remove(position))
        };
        let Some(expectation) = expectation else {
            let message = "unexpected mock call: WriteClient".to_owned();
            self.state
                .lock()
                .expect("mock client lock poisoned")
                .failures
                .push(message.clone());
            return Err(ingestcli::Error::InvalidHttpResponse(message));
        };
        (expectation.responder)()
    }

    fn ingest(
        &self,
        context: &dyn ingestcli::RequestContext,
        request: ingestcli::IngestRequest,
    ) -> Result<(), ingestcli::Error> {
        let expectation = {
            let mut state = self.state.lock().expect("mock client lock poisoned");
            state.calls.push(ClientCall::Ingest {
                context_cancelled: context.is_cancelled(),
                request: request.clone(),
            });
            let position = state
                .ingest
                .iter()
                .position(|expectation| (expectation.matcher)(&request));
            position.and_then(|position| state.ingest.remove(position))
        };
        let Some(expectation) = expectation else {
            let message = "unexpected mock call: Ingest".to_owned();
            self.state
                .lock()
                .expect("mock client lock poisoned")
                .failures
                .push(message.clone());
            return Err(ingestcli::Error::InvalidHttpResponse(message));
        };
        (expectation.responder)(request)
    }
}

/// 一条 Write 期望。
struct WriteExpectation {
    matcher: WriteMatcher,
    responder: WriteResponder,
}

/// MockWriteClient 内部可变状态。
#[derive(Default)]
struct WriteClientState {
    write: VecDeque<WriteExpectation>,
    recv: VecDeque<RecvResponder>,
    expected_close: usize,
    calls: Vec<WriteClientCall>,
    failures: Vec<String>,
}

/// MockWriteClient implements the production WriteClient trait and consumes
/// Write, Recv and Close expectations in the same order as generated GoMock.
/// 实现生产 WriteClient，并按 GoMock 顺序消费 Write/Recv/Close 期望。
#[derive(Clone, Default)]
pub struct MockWriteClient {
    state: Arc<Mutex<WriteClientState>>,
}

/// MockWriteClient 的 EXPECT 录音器。
#[derive(Clone)]
pub struct MockWriteClientMockRecorder {
    state: Arc<Mutex<WriteClientState>>,
}

/// 构造空期望的 MockWriteClient。
pub fn NewMockWriteClient() -> MockWriteClient {
    MockWriteClient::default()
}

impl MockWriteClient {
    /// 返回用于排队期望的 recorder。
    pub fn EXPECT(&self) -> MockWriteClientMockRecorder {
        MockWriteClientMockRecorder {
            state: self.state.clone(),
        }
    }

    /// GoMock 兼容空方法。
    pub fn ISGOMOCK(&self) {}

    /// 返回已记录调用副本。
    pub fn calls(&self) -> Vec<WriteClientCall> {
        self.state
            .lock()
            .expect("mock write client lock poisoned")
            .calls
            .clone()
    }

    /// 检查失败、未消费 Write/Recv 以及未满足的 Close 计数。
    pub fn verify(&self) -> Result<(), VerificationError> {
        let state = self.state.lock().expect("mock write client lock poisoned");
        let mut messages = state.failures.clone();
        if !state.write.is_empty() {
            messages.push(format!(
                "{} Write expectation(s) not called",
                state.write.len()
            ));
        }
        if !state.recv.is_empty() {
            messages.push(format!(
                "{} Recv expectation(s) not called",
                state.recv.len()
            ));
        }
        if state.expected_close > 0 {
            messages.push(format!(
                "{} Close expectation(s) not called",
                state.expected_close
            ));
        }
        if messages.is_empty() {
            Ok(())
        } else {
            Err(VerificationError { messages })
        }
    }
}

impl MockWriteClientMockRecorder {
    /// 排队一条 Write 期望。
    pub fn Write(
        &self,
        matcher: impl Fn(&ingestcli::WriteRequest) -> bool + Send + 'static,
        responder: impl FnOnce(ingestcli::WriteRequest) -> Result<(), ingestcli::Error> + Send + 'static,
    ) -> &Self {
        self.state
            .lock()
            .expect("mock write client lock poisoned")
            .write
            .push_back(WriteExpectation {
                matcher: Box::new(matcher),
                responder: Box::new(responder),
            });
        self
    }

    /// 排队一条 Recv 期望。
    pub fn Recv(
        &self,
        responder: impl FnOnce() -> Result<ingestcli::WriteResponse, ingestcli::Error> + Send + 'static,
    ) -> &Self {
        self.state
            .lock()
            .expect("mock write client lock poisoned")
            .recv
            .push_back(Box::new(responder));
        self
    }

    /// 期望再收到一次 Close（计数 +1）。
    pub fn Close(&self) -> &Self {
        self.state
            .lock()
            .expect("mock write client lock poisoned")
            .expected_close += 1;
        self
    }
}

impl ingestcli::WriteClient for MockWriteClient {
    fn write(&mut self, request: ingestcli::WriteRequest) -> Result<(), ingestcli::Error> {
        let expectation = {
            let mut state = self.state.lock().expect("mock write client lock poisoned");
            state.calls.push(WriteClientCall::Write(request.clone()));
            let position = state
                .write
                .iter()
                .position(|expectation| (expectation.matcher)(&request));
            position.and_then(|position| state.write.remove(position))
        };
        let Some(expectation) = expectation else {
            let message = "unexpected mock call: Write".to_owned();
            self.state
                .lock()
                .expect("mock write client lock poisoned")
                .failures
                .push(message.clone());
            return Err(ingestcli::Error::InvalidHttpResponse(message));
        };
        (expectation.responder)(request)
    }

    fn recv(&mut self) -> Result<ingestcli::WriteResponse, ingestcli::Error> {
        let responder = {
            let mut state = self.state.lock().expect("mock write client lock poisoned");
            state.calls.push(WriteClientCall::Recv);
            state.recv.pop_front()
        };
        let Some(responder) = responder else {
            let message = "unexpected mock call: Recv".to_owned();
            self.state
                .lock()
                .expect("mock write client lock poisoned")
                .failures
                .push(message.clone());
            return Err(ingestcli::Error::InvalidHttpResponse(message));
        };
        responder()
    }

    fn close(&mut self) {
        let mut state = self.state.lock().expect("mock write client lock poisoned");
        state.calls.push(WriteClientCall::Close);
        if state.expected_close == 0 {
            state.failures.push("unexpected Close call".to_owned());
        } else {
            state.expected_close -= 1;
        }
    }
}
