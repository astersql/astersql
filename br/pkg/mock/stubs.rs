// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Local stand-ins for gomock / lightning / import_kvpb / PD / TiKV / server
//! boundaries (darwin arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! `br/pkg/mock` 子系统的本地替身层：在不便链接 kvproto、grpcio、domain、真实
//! TiKV/PD 的环境（尤其 darwin/arm64）中，提供 gomock 控制器、Lightning 元类型、
//! ImportKV protobuf 外形、测试集群/Server/DSN 探测等占位实现。
//! 本文件只保证测试契约形状与可注入钩子，不实现真实备份恢复或 gRPC 传输；
//! Go 侧对应 mockgen 产物与 `mock_cluster.go` 才是行为权威来源。
//! 被 backend/encode/importer/task_register/mock_cluster 等模块广泛引用。

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// 轻量错误替身：仅携带消息字符串，对齐 Go `errors`/`terror` 在 mock 路径的用法。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// 错误消息文本。
    pub msg: String,
}

impl Error {
    /// 由任意可转 String 的消息构造错误。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// Go `errors.Trace` 占位：不做堆栈增强，原样返回，保持调用链可编译。
    pub fn Trace(err: Self) -> Self {
        err
    }
}

impl std::fmt::Display for Error {
    /// 直接写出 msg，便于断言 contains。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

/// 接入标准 Error trait，便于 `?` 与装箱。
impl std::error::Error for Error {}

/// 本模块统一的 Result 别名，错误类型固定为上方 `Error`。
pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// `context.Context` 最小替身：仅暴露 cancelled，供接口签名对齐。
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// 是否已取消；多数替身路径仅透传该标志。
    pub cancelled: bool,
}

impl Context {
    /// 等价 Go `context.Background()`：未取消的根上下文。
    pub fn background() -> Self {
        Self { cancelled: false }
    }
}

// ---------------------------------------------------------------------------
// Lightweight GoMock controller (Call / Record / Return)
// ---------------------------------------------------------------------------
// 轻量 gomock：在未消费期望中匹配方法名并取出预置返回值；参数用 Any 装箱但默认忽略。
// 不支持 Go gomock 的 Times/After/DoAndReturn 等高级匹配器。

/// 单次期望调用：方法名 + 可变返回值列表（由 Call.Return* 填充）。
struct ExpectedCall {
    method: String,
    rets: Mutex<Vec<Box<dyn Any + Send>>>,
}

/// `*gomock.Controller` stand-in.
///
/// 线程安全控制器：Record 入队、Call 查找同名期望；找不到则 panic。
/// Go gomock 默认不要求 EXPECT 登记顺序，只有显式 `gomock.InOrder` 才强制顺序。
#[derive(Clone)]
pub struct Controller {
    inner: Arc<Mutex<ControllerInner>>,
}

/// 控制器内部状态：期望调用的双端队列。
struct ControllerInner {
    expected: VecDeque<Arc<ExpectedCall>>,
}

impl Controller {
    /// 创建空期望队列的控制器。
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ControllerInner {
                expected: VecDeque::new(),
            })),
        }
    }

    /// Go: `ctrl.T.Helper()` — no-op stand-in.
    /// 测试 helper 标记占位，Rust 侧无需栈帧调整。
    pub fn Helper(&self) {}

    /// Go: `ctrl.Call(receiver, method, args...)` → `[]any`.
    ///
    /// 取出首个同名期望；找不到即 panic，成功则 take 返回值向量。
    /// `_args` 保留签名以兼容生成代码，当前不参与匹配。
    pub fn Call(&self, method: &str, _args: Vec<Box<dyn Any + Send>>) -> Vec<Box<dyn Any + Send>> {
        let expected = {
            let mut inner = self.inner.lock().expect("gomock lock");
            let position = inner
                .expected
                .iter()
                .position(|call| call.method == method)
                .unwrap_or_else(|| panic!("Unexpected call to {method}"));
            inner
                .expected
                .remove(position)
                .expect("matched expectation")
        };
        std::mem::take(&mut *expected.rets.lock().expect("rets lock"))
    }

    /// Go: `RecordCallWithMethodType(...)` — records an expected call.
    ///
    /// 登记一条尚未填充返回值的期望，并返回可链式 `Return*` 的 `Call` 句柄。
    pub fn RecordCallWithMethodType(
        &self,
        method: &str,
        _method_type: &str,
        _args: Vec<Box<dyn Any + Send>>,
    ) -> Call {
        let expected = Arc::new(ExpectedCall {
            method: method.to_string(),
            rets: Mutex::new(Vec::new()),
        });
        self.inner
            .lock()
            .expect("gomock lock")
            .expected
            .push_back(Arc::clone(&expected));
        Call { expected }
    }

    /// 剩余未消费期望数；测试结束断言为 0 可发现漏调用。
    pub fn remaining(&self) -> usize {
        self.inner.lock().expect("gomock lock").expected.len()
    }
}

/// `*gomock.Call` stand-in with chainable Return helpers.
///
/// 与期望条目共享 Arc，链式 Return 写入后由后续 Call 弹出。
#[derive(Clone)]
pub struct Call {
    expected: Arc<ExpectedCall>,
}

impl Call {
    /// Set raw return values (Go `Return(rets...)`).
    /// 覆盖整表返回值；元素须为 `Box<dyn Any + Send>`。
    pub fn Return(self, rets: Vec<Box<dyn Any + Send>>) -> Self {
        *self.expected.rets.lock().expect("rets lock") = rets;
        self
    }

    /// Convenience: single error return (nil ⇒ Ok).
    /// 单槽 `Option<Error>`，配合 `take_error` 解包。
    pub fn ReturnError(self, err: Option<Error>) -> Self {
        self.Return(vec![Box::new(err)])
    }

    /// Convenience: typed dual return `(T, error)`.
    /// 双槽 `(T, Option<Error>)`，配合 `take_pair`。
    pub fn Return2<T: Any + Send>(self, val: T, err: Option<Error>) -> Self {
        self.Return(vec![Box::new(val), Box::new(err)])
    }

    /// Convenience: single typed return.
    /// 单槽有类型返回值，配合 `take_one`。
    pub fn Return1<T: Any + Send>(self, val: T) -> Self {
        self.Return(vec![Box::new(val)])
    }
}

/// Extract Go-style `error` from `ret[0]` (`nil` / missing ⇒ Ok).
///
/// 兼容 `Option<Error>`、裸 `Error`；空向量或未知类型视为成功，避免过度 panic。
pub fn take_error(mut rets: Vec<Box<dyn Any + Send>>) -> Result<()> {
    if rets.is_empty() {
        return Ok(());
    }
    let r = rets.remove(0);
    if r.is::<Option<Error>>() {
        return match *r.downcast::<Option<Error>>().unwrap() {
            Some(e) => Err(e),
            None => Ok(()),
        };
    }
    if r.is::<Error>() {
        return Err(*r.downcast::<Error>().unwrap());
    }
    // 类型不符时吞掉，保持与宽松 mock 解包策略一致。
    Ok(())
}

/// Extract `(T, error)` from `ret[0], ret[1]`.
///
/// 值槽 downcast 失败回落 `T::default()`；错误槽委托 `take_error`。
pub fn take_pair<T: 'static>(mut rets: Vec<Box<dyn Any + Send>>) -> Result<T>
where
    T: Default,
{
    let val = if rets.is_empty() {
        T::default()
    } else {
        let r = rets.remove(0);
        r.downcast::<T>()
            .map(|b| *b)
            .unwrap_or_else(|_| T::default())
    };
    let err = if rets.is_empty() {
        Ok(())
    } else {
        take_error(rets)
    };
    err.map(|_| val)
}

/// Extract single typed return (default if missing).
/// 无错误槽的查询方法解包；缺失或类型错时返回 Default。
pub fn take_one<T: 'static + Default>(mut rets: Vec<Box<dyn Any + Send>>) -> T {
    if rets.is_empty() {
        return T::default();
    }
    let r = rets.remove(0);
    r.downcast::<T>()
        .map(|b| *b)
        .unwrap_or_else(|_| T::default())
}

// ---------------------------------------------------------------------------
// Lightning / meta / types stand-ins
// ---------------------------------------------------------------------------
// Lightning / 元信息类型外形：字段尽量对齐 Go，但不执行真实编码或校验。
// 空结构体配置类型刻意保持零大小，避免伪造未迁移字段。

/// 引擎 UUID 替身；`new` 为零值，仅用于 mock 身份区分。
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct UUID(pub [u8; 16]);

impl UUID {
    /// 全零 UUID，便于默认构造路径。
    pub fn new() -> Self {
        Self([0u8; 16])
    }

    /// 从固定 16 字节构造，供测试指定引擎身份。
    pub fn from_bytes(b: [u8; 16]) -> Self {
        Self(b)
    }
}

/// 打开引擎时的配置占位；当前无字段，仅占签名。
#[derive(Clone, Debug, Default)]
pub struct EngineConfig {}

/// LocalWriter 配置占位。
#[derive(Clone, Debug, Default)]
pub struct LocalWriterConfig {}

/// 目标集群需求检查上下文占位。
#[derive(Clone, Debug, Default)]
pub struct CheckCtx {}

/// 行编码配置占位。
#[derive(Clone, Debug, Default)]
pub struct EncodingConfig {}

/// 单列 Datum 占位，Encode 入参用。
#[derive(Clone, Debug, Default)]
pub struct Datum {}

/// KV 校验和摘要：校验值、键数与字节总量。
#[derive(Clone, Debug, Default)]
pub struct KVChecksum {
    pub checksum: u64,
    pub total_kvs: u64,
    pub total_bytes: u64,
}

/// 远端库信息模型的最小字段（名称）。
#[derive(Clone, Debug, Default)]
pub struct DBInfo {
    pub name: String,
}

/// 远端表信息模型的最小字段（名称）。
#[derive(Clone, Debug, Default)]
pub struct TableInfo {
    pub name: String,
}

/// `common.ChunkFlushStatus` interface stand-in.
/// 刷盘状态查询接口；Send 以便跨线程持有。
pub trait ChunkFlushStatus: Send {
    fn Flushed(&self) -> bool;
}

/// 简单布尔实现，供 NilEngineWriter / 测试直接构造。
#[derive(Clone, Debug, Default)]
pub struct SimpleChunkFlushStatus {
    pub flushed: bool,
}

impl ChunkFlushStatus for SimpleChunkFlushStatus {
    fn Flushed(&self) -> bool {
        self.flushed
    }
}

/// Opaque rows / row handles returned by encode mocks.
/// 行批句柄：真实行数据不驻留此处，仅用 id 区分批次。
#[derive(Clone, Debug, Default)]
pub struct RowsHandle {
    pub id: u64,
}

/// 单行句柄：id + 估计大小，供 Encode 返回。
#[derive(Clone, Debug, Default)]
pub struct RowHandle {
    pub id: u64,
    pub size: u64,
}

/// `backend.EngineWriter` interface stand-in (object-safe).
/// 对象安全写入接口：追加行、关闭取刷盘状态、查询是否已同步。
pub trait EngineWriter: Send {
    fn AppendRows(&mut self, ctx: Context, column_names: &[String], rows: RowsHandle)
    -> Result<()>;
    fn Close(&mut self, ctx: Context) -> Result<Box<dyn ChunkFlushStatus>>;
    fn IsSynced(&self) -> bool;
}

/// Empty writer used when LocalWriter is not configured.
/// 空写入器：所有操作成功但不上盘；Close 返回 flushed=false。
#[derive(Default)]
pub struct NilEngineWriter;

impl EngineWriter for NilEngineWriter {
    fn AppendRows(
        &mut self,
        _ctx: Context,
        _column_names: &[String],
        _rows: RowsHandle,
    ) -> Result<()> {
        Ok(())
    }
    fn Close(&mut self, _ctx: Context) -> Result<Box<dyn ChunkFlushStatus>> {
        Ok(Box::new(SimpleChunkFlushStatus { flushed: false }))
    }
    fn IsSynced(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// import_kvpb / grpc stand-ins
// ---------------------------------------------------------------------------
// ImportKV protobuf / gRPC 外形替身：不编解码字节流，仅保留测试需要的字段。
// uuid 以 `Vec<u8>` 存放，对应 Go `[]byte` 而非强类型 UUID。

/// 清理引擎请求：按 uuid 标识目标引擎。
#[derive(Clone, Debug, Default)]
pub struct CleanupEngineRequest {
    pub uuid: Vec<u8>,
}
/// 清理引擎空响应。
#[derive(Clone, Debug, Default)]
pub struct CleanupEngineResponse {}

/// 关闭引擎请求。
#[derive(Clone, Debug, Default)]
pub struct CloseEngineRequest {
    pub uuid: Vec<u8>,
}
/// 关闭引擎空响应。
#[derive(Clone, Debug, Default)]
pub struct CloseEngineResponse {}

/// 集群 Compact 请求/响应占位（无字段）。
#[derive(Clone, Debug, Default)]
pub struct CompactClusterRequest {}
/// Compact 空响应占位。
#[derive(Clone, Debug, Default)]
pub struct CompactClusterResponse {}

/// 拉取 metrics：响应携带文本指标串。
#[derive(Clone, Debug, Default)]
pub struct GetMetricsRequest {}
#[derive(Clone, Debug, Default)]
pub struct GetMetricsResponse {
    /// 指标文本（Prometheus 风格占位）。
    pub metrics: String,
}

/// 版本查询：响应携带 importer 版本字符串。
#[derive(Clone, Debug, Default)]
pub struct GetVersionRequest {}
#[derive(Clone, Debug, Default)]
pub struct GetVersionResponse {
    /// importer 版本字符串占位。
    pub version: String,
}

/// 导入引擎请求：uuid 指向已打开引擎。
#[derive(Clone, Debug, Default)]
pub struct ImportEngineRequest {
    pub uuid: Vec<u8>,
}
/// 导入引擎空响应。
/// 导入引擎空响应。
#[derive(Clone, Debug, Default)]
pub struct ImportEngineResponse {}

/// 打开引擎请求/响应。
#[derive(Clone, Debug, Default)]
pub struct OpenEngineRequest {
    pub uuid: Vec<u8>,
}
/// 打开引擎空响应。
#[derive(Clone, Debug, Default)]
pub struct OpenEngineResponse {}

/// 切换 TiKV 导入模式；mode 为枚举整型替身。
#[derive(Clone, Debug, Default)]
pub struct SwitchModeRequest {
    pub mode: i32,
}
/// 切换模式空响应。
#[derive(Clone, Debug, Default)]
pub struct SwitchModeResponse {}

/// WriteEngine V3 / V1 请求与响应；响应 error 非空表示流式写入失败信息。
#[derive(Clone, Debug, Default)]
pub struct WriteEngineV3Request {
    pub uuid: Vec<u8>,
}
#[derive(Clone, Debug, Default)]
pub struct WriteEngineRequest {
    pub uuid: Vec<u8>,
}
#[derive(Clone, Debug, Default)]
pub struct WriteEngineResponse {
    /// 非空表示流式写失败详情。
    pub error: String,
}

/// gRPC call option stand-in.
/// 调用选项占位，不解释 deadline/metadata 语义。
#[derive(Clone, Debug, Default)]
pub struct CallOption {}

/// gRPC metadata stand-in.
/// 键值对列表，供 Header/Trailer 返回。
#[derive(Clone, Debug, Default)]
pub struct Metadata {
    /// header/trailer 键值列表。
    pub entries: Vec<(String, String)>,
}

/// `ImportKV_WriteEngineClient` stream interface stand-in.
/// 双向流客户端接口外形；真实 gRPC 状态机未实现。
pub trait WriteEngineClient: Send {
    fn CloseAndRecv(&mut self) -> Result<WriteEngineResponse>;
    fn CloseSend(&mut self) -> Result<()>;
    fn Context(&self) -> Context;
    fn Header(&mut self) -> Result<Metadata>;
    fn RecvMsg(&mut self, msg: &mut dyn Any) -> Result<()>;
    fn Send(&mut self, req: &WriteEngineRequest) -> Result<()>;
    fn SendMsg(&mut self, msg: &dyn Any) -> Result<()>;
    fn Trailer(&self) -> Metadata;
}

/// 空流客户端：所有方法成功返回默认值，用于未配置真实流时的回退。
#[derive(Default)]
pub struct NilWriteEngineClient {
    pub ctx: Context,
}

impl WriteEngineClient for NilWriteEngineClient {
    fn CloseAndRecv(&mut self) -> Result<WriteEngineResponse> {
        Ok(WriteEngineResponse::default())
    }
    fn CloseSend(&mut self) -> Result<()> {
        Ok(())
    }
    fn Context(&self) -> Context {
        self.ctx.clone()
    }
    fn Header(&mut self) -> Result<Metadata> {
        Ok(Metadata::default())
    }
    fn RecvMsg(&mut self, _msg: &mut dyn Any) -> Result<()> {
        Ok(())
    }
    fn Send(&mut self, _req: &WriteEngineRequest) -> Result<()> {
        Ok(())
    }
    fn SendMsg(&mut self, _msg: &dyn Any) -> Result<()> {
        Ok(())
    }
    fn Trailer(&self) -> Metadata {
        Metadata::default()
    }
}

// ---------------------------------------------------------------------------
// Cluster / PD / TiKV / server stand-ins
// ---------------------------------------------------------------------------
// 测试集群 / PD / Storage / Server 替身：模拟 Bootstrap→Start→Stop 生命周期标志。
// 不拉起真实 PD/TiKV 进程，端口与 HTTP 探测均可被钩子改写。

/// 内存 TiKV 集群状态：store 数与是否已 Bootstrap。
#[derive(Clone, Debug)]
pub struct TiKVCluster {
    pub stores: usize,
    pub bootstrapped: bool,
}

impl Default for TiKVCluster {
    fn default() -> Self {
        Self {
            stores: 0,
            bootstrapped: false,
        }
    }
}

/// KV Storage 替身：持有 PD 客户端、关闭标志与共享集群句柄。
#[derive(Clone, Debug)]
pub struct Storage {
    pub name: String,
    pub closed: Arc<AtomicBool>,
    pub pd_client: PDClient,
    pub pd_http: PDHTTPClient,
    pub cluster: Arc<Mutex<TiKVCluster>>,
}

impl Storage {
    /// 标记 closed=true；不释放底层真实资源（本就无）。
    pub fn Close(&self) -> Result<()> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// 由 pd_client 派生 RegionCache，对齐 Go Storage.GetRegionCache。
    pub fn GetRegionCache(&self) -> RegionCache {
        RegionCache {
            pd: self.pd_client.clone(),
        }
    }

    /// 返回 PD HTTP 客户端克隆。
    pub fn GetPDHTTPClient(&self) -> PDHTTPClient {
        self.pd_http.clone()
    }
}

/// RegionCache 替身：仅缓存 PD 引用。
#[derive(Clone, Debug, Default)]
pub struct RegionCache {
    pub pd: PDClient,
}

impl RegionCache {
    /// 暴露内部 PDClient，供上层取调度客户端。
    pub fn PDClient(&self) -> PDClient {
        self.pd.clone()
    }
}

/// PD gRPC 客户端占位（id 区分实例）。
#[derive(Clone, Debug, Default)]
pub struct PDClient {
    pub id: u64,
}

/// PD HTTP 客户端占位。
#[derive(Clone, Debug, Default)]
pub struct PDHTTPClient {
    pub id: u64,
}

/// Domain 会话替身；Close 只翻 closed 标志。
#[derive(Clone, Debug)]
pub struct Domain {
    pub closed: Arc<AtomicBool>,
}

impl Domain {
    pub fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// TiDB 驱动替身：记录关联 storage 名。
#[derive(Clone, Debug)]
pub struct TiDBDriver {
    pub storage_name: String,
}

/// SQL Server 替身：端口、运行/关闭原子标志与 ready 条件变量。
#[derive(Clone, Debug)]
pub struct Server {
    pub port: u16,
    pub status_port: u16,
    pub closed: Arc<AtomicBool>,
    pub running: Arc<AtomicBool>,
    ready: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl Server {
    /// 置 running、通知 ready，并可选向 GoTest chan 发信号；循环直至 Close。
    pub fn Run(&self, _signal: Option<()>) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        {
            let (lock, cv) = &*self.ready;
            let mut ready = lock.lock().unwrap();
            *ready = true;
            cv.notify_all();
        }
        // Go: when RunInGoTest, server notifies RunInGoTestChan once ready.
        // 与 Go 测试路径一致：就绪后通知等待方。
        notify_run_in_go_test();
        // Stay "running" until Close — matches Go server lifecycle for tests.
        // 自旋等待 Close，避免测试线程过早认为服务已退出。
        while self.running.load(Ordering::SeqCst) && !self.closed.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    /// 关闭：翻转 closed/running 并唤醒 wait_ready 等待者。
    pub fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
        let (lock, cv) = &*self.ready;
        let _g = lock.lock().unwrap();
        cv.notify_all();
    }

    /// 阻塞直到 Run 将 ready 置真。
    pub fn wait_ready(&self) {
        let (lock, cv) = &*self.ready;
        let mut ready = lock.lock().unwrap();
        while !*ready {
            ready = cv.wait(ready).unwrap();
        }
    }
}

/// 状态 HTTP 服务替身：ListenAndServe 仅翻 listening 标志。
#[derive(Clone, Debug)]
pub struct HttpServer {
    pub Addr: String,
    pub closed: Arc<AtomicBool>,
    pub listening: Arc<AtomicBool>,
}

impl HttpServer {
    /// 标记已监听；不真正 bind 端口。
    pub fn ListenAndServe(&self) -> Result<()> {
        self.listening.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// 关闭监听标志。
    pub fn Close(&self) -> Result<()> {
        self.closed.store(true, Ordering::SeqCst);
        self.listening.store(false, Ordering::SeqCst);
        Ok(())
    }
}

/// 服务器配置：SQL 端口、Store 类型、Status 与 Socket。
#[derive(Clone, Debug)]
pub struct Config {
    pub Port: u16,
    pub Store: StoreType,
    pub Status: StatusConfig,
    pub Socket: String,
}

/// 存储类型枚举替身；测试集群固定 TiKV。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreType {
    TiKV,
}

/// 状态服务配置：端口与是否上报。
#[derive(Clone, Debug)]
pub struct StatusConfig {
    pub StatusPort: u16,
    pub ReportStatus: bool,
}

impl Config {
    /// 默认 4000/10080，ReportStatus=false，对齐 Go NewConfig 常用测试默认。
    pub fn NewConfig() -> Self {
        Self {
            Port: 4000,
            Store: StoreType::TiKV,
            Status: StatusConfig {
                StatusPort: 10080,
                ReportStatus: false,
            },
            Socket: String::new(),
        }
    }
}

/// 包级构造，转发到 `Config::NewConfig`。
pub fn NewConfig() -> Config {
    Config::NewConfig()
}

/// 是否处于 Go 测试协作模式（影响就绪通知等）。
pub static RUN_IN_GO_TEST: AtomicBool = AtomicBool::new(false);

static GO_TEST_NOTIFY: Mutex<Option<std::sync::mpsc::Sender<()>>> = Mutex::new(None);

/// Go: `server.RunInGoTestChan = make(chan struct{})`.
/// 创建并登记就绪通知 channel；返回接收端供测试等待。
pub fn make_run_in_go_test_chan() -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    *GO_TEST_NOTIFY.lock().unwrap() = Some(tx);
    rx
}

/// Go server signals readiness on `RunInGoTestChan`.
/// Server.Run 就绪时发送；无发送端则静默忽略。
pub fn notify_run_in_go_test() {
    if let Some(tx) = GO_TEST_NOTIFY.lock().unwrap().as_ref() {
        let _ = tx.send(());
    }
}

/// 由 Storage 名称构造驱动替身。
pub fn NewTiDBDriver(storage: &Storage) -> TiDBDriver {
    TiDBDriver {
        storage_name: storage.name.clone(),
    }
}

/// 按配置构造 Server；端口为 0 时填入测试用非零占位端口。
pub fn NewServer(cfg: &Config, _driver: &TiDBDriver) -> Result<Server> {
    let status_port = if cfg.Status.StatusPort == 0 {
        // Allocate an ephemeral-looking status port for waitUntilServerOnline.
        // 0 表示“自动分配”；此处给固定伪临时端口供 HTTP 探测拼 URL。
        18080
    } else {
        cfg.Status.StatusPort
    };
    let port = if cfg.Port == 0 { 14001 } else { cfg.Port };
    Ok(Server {
        port,
        status_port,
        closed: Arc::new(AtomicBool::new(false)),
        running: Arc::new(AtomicBool::new(false)),
        ready: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
    })
}

/// 测试路径禁用统计，避免额外内存；桩为空操作。
pub fn DisableStats4Test() {}

/// 单 store Bootstrap：stores=1 且 bootstrapped=true。
pub fn BootstrapWithSingleStore(cluster: &mut TiKVCluster) {
    cluster.stores = 1;
    cluster.bootstrapped = true;
}

/// 创建未 Bootstrap 的 mock store，允许 inspector 在锁内改集群状态。
pub fn NewMockStoreWithoutBootstrap(inspector: impl FnOnce(&mut TiKVCluster)) -> Result<Storage> {
    let cluster = Arc::new(Mutex::new(TiKVCluster::default()));
    {
        let mut c = cluster.lock().unwrap();
        inspector(&mut c);
    }
    Ok(Storage {
        name: "mockstore".into(),
        closed: Arc::new(AtomicBool::new(false)),
        pd_client: PDClient { id: 1 },
        pd_http: PDHTTPClient { id: 1 },
        cluster,
    })
}

/// Bootstrap 会话 Domain；忽略 storage 内容，仅返回可 Close 的 Domain。
pub fn BootstrapSession(storage: &Storage) -> Result<Domain> {
    let _ = storage;
    Ok(Domain {
        closed: Arc::new(AtomicBool::new(false)),
    })
}

/// Go `view.Stop` 占位，测试 teardown 可调用。
pub fn view_Stop() {}

// ---------------------------------------------------------------------------
// MySQL DSN / SQL / HTTP probes (injectable for tests)
// ---------------------------------------------------------------------------
// MySQL DSN 与 SQL/HTTP 探测钩子：测试可注入失败/成功，驱动 waitUntilServerOnline。
// 钩子存储在 thread_local，并行测试需各自 set/reset。

/// go-sql-driver 风格配置子集，用于 FormatDSN。
#[derive(Clone, Debug)]
pub struct MysqlConfig {
    pub User: String,
    pub Net: String,
    pub Addr: String,
    pub Passwd: String,
    pub DBName: String,
}

impl Default for MysqlConfig {
    fn default() -> Self {
        // 默认 root@tcp(127.0.0.1:4001)/，与常见 mock 集群端口习惯一致。
        Self {
            User: "root".into(),
            Net: "tcp".into(),
            Addr: "127.0.0.1:4001".into(),
            Passwd: String::new(),
            DBName: String::new(),
        }
    }
}

impl MysqlConfig {
    /// 拼 DSN；空密码省略 `:passwd`，对齐 go-sql-driver 常用格式。
    pub fn FormatDSN(&self) -> String {
        // Mirrors go-sql-driver FormatDSN for the fields we set.
        let user = if self.Passwd.is_empty() {
            self.User.clone()
        } else {
            format!("{}:{}", self.User, self.Passwd)
        };
        format!("{}@{}({})/{}", user, self.Net, self.Addr, self.DBName)
    }
}

thread_local! {
    // 每测试线程独立：重试次数、sleep 毫秒、是否跳过 sleep。
    static RETRY_TIME: Cell<i32> = const { Cell::new(100) };
    static SLEEP_MS: Cell<u64> = const { Cell::new(10) };
    static SKIP_SLEEP: Cell<bool> = const { Cell::new(false) };
}

/// Test hook: override retry count (Go `retryTime`).
/// 覆盖 waitUntilServerOnline 等重试上限。
pub fn set_retry_time(n: i32) {
    RETRY_TIME.set(n);
}

/// 读取当前重试上限。
pub fn retry_time() -> i32 {
    RETRY_TIME.get()
}

/// 设置重试间隔毫秒。
pub fn set_sleep_ms(ms: u64) {
    SLEEP_MS.set(ms);
}

/// 为 true 时 sleep_retry 立即返回，加速单测。
pub fn set_skip_sleep(skip: bool) {
    SKIP_SLEEP.set(skip);
}

/// 按 SLEEP_MS 休眠；SKIP_SLEEP 时跳过。
pub fn sleep_retry() {
    if SKIP_SLEEP.get() {
        return;
    }
    std::thread::sleep(Duration::from_millis(SLEEP_MS.get()));
}

type SqlOpenFn = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;
type HttpGetFn = Arc<dyn Fn(&str) -> Result<Vec<u8>> + Send + Sync>;

thread_local! {
    static SQL_OPEN: RefCell<Option<SqlOpenFn>> = const { RefCell::new(None) };
    static HTTP_GET: RefCell<Option<HttpGetFn>> = const { RefCell::new(None) };
    /// When true, default probes succeed (cluster marked online).
    /// 无自定义钩子时，为 true 则 SQL/HTTP 默认成功。
    static CLUSTER_ONLINE: Cell<bool> = const { Cell::new(false) };
}

/// 标记集群“在线”，影响默认探测成败。
pub fn set_cluster_online(online: bool) {
    CLUSTER_ONLINE.set(online);
}

/// 注入 SQL Open 钩子；Some 时优先调用。
pub fn set_sql_open(f: Option<SqlOpenFn>) {
    SQL_OPEN.with(|hook| *hook.borrow_mut() = f);
}

/// 注入 HTTP GET 钩子；Some 时优先调用并包装为 HttpResponse。
pub fn set_http_get(f: Option<HttpGetFn>) {
    HTTP_GET.with(|hook| *hook.borrow_mut() = f);
}

/// 打开 SQL：钩子 → CLUSTER_ONLINE → 否则返回失败错误。
pub fn sql_open(_driver: &str, dsn: &str) -> Result<SqlDB> {
    if let Some(f) = SQL_OPEN.with(|hook| hook.borrow().clone()) {
        f(dsn)?;
        return Ok(SqlDB {});
    }
    if CLUSTER_ONLINE.get() {
        return Ok(SqlDB {});
    }
    Err(Error::new(format!("sql open failed for {dsn}")))
}

/// HTTP GET：钩子体优先；在线默认返回 `{"status":"ok"}` JSON 字节。
pub fn http_get(url: &str) -> Result<HttpResponse> {
    if let Some(f) = HTTP_GET.with(|hook| hook.borrow().clone()) {
        let body = f(url)?;
        return Ok(HttpResponse { body });
    }
    if CLUSTER_ONLINE.get() {
        return Ok(HttpResponse {
            body: br#"{"status":"ok"}"#.to_vec(),
        });
    }
    Err(Error::new(format!("http get failed for {url}")))
}

/// SQL 连接句柄占位；无真实连接。
#[derive(Debug)]
pub struct SqlDB {}

impl SqlDB {
    /// 关闭占位，无副作用。
    pub fn Close(&self) {}
}

/// HTTP 响应体容器。
#[derive(Debug)]
pub struct HttpResponse {
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// 返回响应体切片视图。
    pub fn Body(&self) -> &[u8] {
        &self.body
    }
}

/// Reset injectable cluster/test hooks (call from tests).
/// 复位全部线程局部钩子与 RUN_IN_GO_TEST，防止跨测试污染。
pub fn reset_test_hooks() {
    set_retry_time(100);
    set_sleep_ms(10);
    set_skip_sleep(false);
    set_cluster_online(false);
    set_sql_open(None);
    set_http_get(None);
    RUN_IN_GO_TEST.store(false, Ordering::SeqCst);
}
