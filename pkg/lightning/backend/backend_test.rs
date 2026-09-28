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
// Copyright 2026 AsterSQL.

// Lightning `EngineManager` / Backend 生命周期单元测试。
//
// 使用 MockBackend / MockWriter 模拟打开、关闭、导入、清理与本地写入引擎等路径，
// 覆盖成功流程、打开失败、可恢复/不可恢复导入错误及编码工厂冒烟测试。
// Engine（引擎）是按表分片的中间存储单元；导入（Import）将其数据刷入 TiKV/目标存储。

use std::any::Any;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use encode::{Context, Datum, EncodeError, Encoder, EncodingBuilder, EncodingConfig, Row, Rows};
use uuid::Uuid;

use crate::*;

/// 测试用空行缓冲实现。
#[derive(Default)]
struct DummyRows(Vec<Vec<u8>>);

impl Rows for DummyRows {
    fn Clear(self: Box<Self>) -> Box<dyn Rows> {
        Box::new(Self(Vec::with_capacity(self.0.capacity())))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Mock Backend 共享状态：记录调用序列、导入结果队列与失败开关。
#[derive(Default)]
struct MockState {
    calls: Mutex<Vec<String>>,
    /// 按次序弹出的 Import 返回值，用于模拟重试场景。
    imports: Mutex<VecDeque<Result<(), BackendError>>>,
    writes: AtomicUsize,
    fail_open: AtomicBool,
    fail_write: AtomicBool,
    closed: AtomicBool,
}

/// 可注入失败行为的 Backend 替身。
#[derive(Default)]
struct MockBackend {
    state: Arc<MockState>,
}

impl Backend for MockBackend {
    fn Close(&self) {
        self.state.closed.store(true, Ordering::SeqCst);
    }
    fn RetryImportDelay(&self) -> Duration {
        Duration::ZERO
    }
    fn ShouldPostProcess(&self) -> bool {
        true
    }
    fn OpenEngine(&self, _: &Context, _: &EngineConfig, uuid: Uuid) -> Result<(), BackendError> {
        self.state
            .calls
            .lock()
            .unwrap()
            .push(format!("open:{uuid}"));
        if self.state.fail_open.load(Ordering::SeqCst) {
            Err(BackendError::new("fake unrecoverable open error"))
        } else {
            Ok(())
        }
    }
    fn CloseEngine(
        &self,
        _: &Context,
        _: Option<&EngineConfig>,
        uuid: Uuid,
    ) -> Result<(), BackendError> {
        self.state
            .calls
            .lock()
            .unwrap()
            .push(format!("close:{uuid}"));
        Ok(())
    }
    fn ImportEngine(&self, _: &Context, uuid: Uuid, _: i64, _: i64) -> Result<(), BackendError> {
        self.state
            .calls
            .lock()
            .unwrap()
            .push(format!("import:{uuid}"));
        // 从队列取下一次导入结果；队列空则视为成功。
        self.state
            .imports
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(()))
    }
    fn CleanupEngine(&self, _: &Context, uuid: Uuid) -> Result<(), BackendError> {
        self.state
            .calls
            .lock()
            .unwrap()
            .push(format!("cleanup:{uuid}"));
        Ok(())
    }
    fn FlushEngine(&self, _: &Context, uuid: Uuid) -> Result<(), BackendError> {
        self.state
            .calls
            .lock()
            .unwrap()
            .push(format!("flush:{uuid}"));
        Ok(())
    }
    fn FlushAllEngines(&self, _: &Context) -> Result<(), BackendError> {
        Ok(())
    }
    fn LocalWriter(
        &self,
        _: &Context,
        _: &LocalWriterConfig,
        _: Uuid,
    ) -> Result<Box<dyn EngineWriter>, BackendError> {
        Ok(Box::new(MockWriter {
            state: Arc::clone(&self.state),
        }))
    }
}

/// 统计 AppendRows 次数并可注入写失败的本地 Writer。
struct MockWriter {
    state: Arc<MockState>,
}

impl EngineWriter for MockWriter {
    fn AppendRows(&mut self, _: &Context, _: &[String], _: &dyn Rows) -> Result<(), BackendError> {
        self.state.writes.fetch_add(1, Ordering::SeqCst);
        if self.state.fail_write.load(Ordering::SeqCst) {
            Err(BackendError::new("fake recoverable write batch error"))
        } else {
            Ok(())
        }
    }
    fn IsSynced(&self) -> bool {
        true
    }
    fn Close(&mut self, _: &Context) -> Result<Option<ChunkFlushStatus>, BackendError> {
        Ok(Some(ChunkFlushStatus { flushed: true }))
    }
}

/// 测试套件：持有 MockBackend 与 EngineManager。
struct backendSuite {
    mockBackend: Arc<MockBackend>,
    engineMgr: EngineManager,
}

fn createBackendSuite() -> backendSuite {
    let mockBackend = Arc::new(MockBackend::default());
    let backend: Arc<dyn Backend> = mockBackend.clone();
    backendSuite {
        engineMgr: MakeEngineManager(backend),
        mockBackend,
    }
}

impl backendSuite {
    fn tearDownTest(&self) {}
}

/// 覆盖 Open → Close → Import → Cleanup 完整成功路径及确定性 UUID。
#[test]
fn TestOpenCloseImportCleanUpEngine() {
    let suite = createBackendSuite();
    let ctx = Context::default();
    let opened = suite
        .engineMgr
        .OpenEngine(&ctx, &EngineConfig::default(), "`db`.`table`", 1)
        .unwrap();
    assert_eq!(
        opened.GetEngineUUID(),
        Uuid::parse_str("902efee3-a3f9-53d4-8c82-f12fb1900cd1").unwrap()
    );
    let closed = opened.Close(&ctx).unwrap();
    closed.Import(&ctx, 1, 1).unwrap();
    closed.Cleanup(&ctx).unwrap();
    assert_eq!(suite.mockBackend.state.calls.lock().unwrap().len(), 4);
    suite.tearDownTest();
}

/// 未先 Open 时通过表名不安全关闭引擎，并清理。
#[test]
fn TestUnsafeCloseEngine() {
    let suite = createBackendSuite();
    let closed = suite
        .engineMgr
        .UnsafeCloseEngine(&Context::default(), None, "`db`.`table`", -1)
        .unwrap();
    assert_eq!(closed.GetID(), -1);
    closed.Cleanup(&Context::default()).unwrap();
}

/// 按给定 UUID 不安全关闭引擎。
#[test]
fn TestUnsafeCloseEngineWithUUID() {
    let suite = createBackendSuite();
    let uuid = Uuid::new_v4();
    let closed = suite
        .engineMgr
        .UnsafeCloseEngineWithUUID(&Context::default(), None, "tag", uuid, 0)
        .unwrap();
    assert_eq!(closed.GetUUID(), uuid);
    closed.Cleanup(&Context::default()).unwrap();
    assert!(
        suite
            .mockBackend
            .state
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call == &format!("cleanup:{uuid}"))
    );
}

/// 本地 Writer 连续 AppendRows 两次后关闭，写计数为 2。
#[test]
fn TestWriteEngine() {
    let suite = createBackendSuite();
    let ctx = Context::default();
    let opened = suite
        .engineMgr
        .OpenEngine(&ctx, &EngineConfig::default(), "`db`.`table`", 1)
        .unwrap();
    let mut writer = opened
        .LocalWriter(&ctx, &LocalWriterConfig::default())
        .unwrap();
    writer
        .AppendRows(&ctx, &["c1".into(), "c2".into()], &DummyRows::default())
        .unwrap();
    writer
        .AppendRows(&ctx, &["c1".into(), "c2".into()], &DummyRows::default())
        .unwrap();
    assert!(writer.Close(&ctx).unwrap().unwrap().flushed);
    assert_eq!(suite.mockBackend.state.writes.load(Ordering::SeqCst), 2);
}

/// 空列名列表的 AppendRows 也应成功。
#[test]
fn TestWriteToEngineWithNothing() {
    let suite = createBackendSuite();
    let ctx = Context::default();
    let opened = suite
        .engineMgr
        .OpenEngine(&ctx, &EngineConfig::default(), "t", 1)
        .unwrap();
    let mut writer = opened
        .LocalWriter(&ctx, &LocalWriterConfig::default())
        .unwrap();
    writer.AppendRows(&ctx, &[], &DummyRows::default()).unwrap();
    writer.Close(&ctx).unwrap();
}

/// OpenEngine 在 fail_open 时返回不可恢复错误。
#[test]
fn TestOpenEngineFailed() {
    let suite = createBackendSuite();
    suite
        .mockBackend
        .state
        .fail_open
        .store(true, Ordering::SeqCst);
    let error = suite
        .engineMgr
        .OpenEngine(&Context::default(), &EngineConfig::default(), "t", 1)
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "fake unrecoverable open error");
}

/// AppendRows 在 fail_write 时返回错误，Close 仍可完成。
#[test]
fn TestWriteEngineFailed() {
    let suite = createBackendSuite();
    suite
        .mockBackend
        .state
        .fail_write
        .store(true, Ordering::SeqCst);
    let ctx = Context::default();
    let opened = suite
        .engineMgr
        .OpenEngine(&ctx, &EngineConfig::default(), "t", 1)
        .unwrap();
    let mut writer = opened
        .LocalWriter(&ctx, &LocalWriterConfig::default())
        .unwrap();
    assert!(writer.AppendRows(&ctx, &[], &DummyRows::default()).is_err());
    writer.Close(&ctx).unwrap();
}

/// 批发送失败场景独立保留 Go 用例的错误形状与资源关闭断言。
#[test]
fn TestWriteBatchSendFailedWithRetry() {
    let suite = createBackendSuite();
    suite
        .mockBackend
        .state
        .fail_write
        .store(true, Ordering::SeqCst);
    let ctx = Context::default();
    let opened = suite
        .engineMgr
        .OpenEngine(&ctx, &EngineConfig::default(), "t", 1)
        .unwrap();
    let mut writer = opened
        .LocalWriter(&ctx, &LocalWriterConfig::default())
        .unwrap();

    let error = writer
        .AppendRows(&ctx, &[], &DummyRows::default())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .ends_with("fake recoverable write batch error")
    );
    assert!(writer.Close(&ctx).unwrap().unwrap().flushed);
    assert_eq!(suite.mockBackend.state.writes.load(Ordering::SeqCst), 1);
}

/// 不可恢复 Import 错误只调用一次 import，不重试。
#[test]
fn TestImportFailedNoRetry() {
    let suite = createBackendSuite();
    suite
        .mockBackend
        .state
        .imports
        .lock()
        .unwrap()
        .push_back(Err(BackendError::new("fake unrecoverable import error")));
    let closed = suite
        .engineMgr
        .UnsafeCloseEngine(&Context::default(), None, "t", 1)
        .unwrap();
    let error = closed.Import(&Context::default(), 1, 1).unwrap_err();
    assert_eq!(error.to_string(), "fake unrecoverable import error");
    assert_eq!(
        suite
            .mockBackend
            .state
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.starts_with("import:"))
            .count(),
        1
    );
}

/// 连续可恢复 Import 错误最终仍失败，错误信息保留。
#[test]
fn TestImportFailedWithRetry() {
    let suite = createBackendSuite();
    let mut imports = suite.mockBackend.state.imports.lock().unwrap();
    for _ in 0..3 {
        imports.push_back(Err(BackendError::retryable(
            "fake recoverable import error",
        )));
    }
    drop(imports);
    let closed = suite
        .engineMgr
        .UnsafeCloseEngine(&Context::default(), None, "t", 1)
        .unwrap();
    let error = closed.Import(&Context::default(), 1, 1).unwrap_err();
    assert!(error.to_string().contains("fake recoverable import error"));
    assert!(
        error.retryable,
        "Go errors.Annotatef preserves the final retryable cause"
    );
    assert_eq!(
        suite
            .mockBackend
            .state
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.starts_with("import:"))
            .count(),
        3
    );
}

/// 先失败一次后成功，Import 最终 Ok。
#[test]
fn TestImportFailedRecovered() {
    let suite = createBackendSuite();
    let mut imports = suite.mockBackend.state.imports.lock().unwrap();
    imports.push_back(Err(BackendError::retryable("invalid connection")));
    imports.push_back(Ok(()));
    drop(imports);
    let closed = suite
        .engineMgr
        .UnsafeCloseEngine(&Context::default(), None, "t", 1)
        .unwrap();
    closed.Import(&Context::default(), 1, 1).unwrap();
}

/// Backend.Close 将 closed 标志置位。
#[test]
fn TestClose() {
    let suite = createBackendSuite();
    suite.mockBackend.Close();
    assert!(suite.mockBackend.state.closed.load(Ordering::SeqCst));
}

/// 测试用 Encoder：Encode 恒返回错误。
struct DummyEncoder;
impl Encoder for DummyEncoder {
    fn Close(&mut self) {}
    fn Encode(
        &mut self,
        _: &[Datum],
        _: i64,
        _: &[i32],
        _: i64,
    ) -> Result<Box<dyn Row>, EncodeError> {
        Err(EncodeError("unused".into()))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 测试用 EncodingBuilder：产出 DummyEncoder / DummyRows。
struct DummyBuilder;
impl EncodingBuilder for DummyBuilder {
    fn NewEncoder(&self, _: &Context, _: &EncodingConfig) -> Result<Box<dyn Encoder>, EncodeError> {
        Ok(Box::new(DummyEncoder))
    }
    fn MakeEmptyRows(&self) -> Box<dyn Rows> {
        Box::new(DummyRows::default())
    }
}

#[test]
fn TestMakeEmptyRows() {
    assert!(DummyBuilder.MakeEmptyRows().as_any().is::<DummyRows>());
}

#[test]
fn TestNewEncoder() {
    let options = EncodingConfig {
        SessionOptions: encode::SessionOptions {
            SQLMode: 4,
            Timestamp: 1_234_567_890,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(
        DummyBuilder
            .NewEncoder(&Context::default(), &options)
            .unwrap()
            .as_any()
            .is::<DummyEncoder>()
    );
}
