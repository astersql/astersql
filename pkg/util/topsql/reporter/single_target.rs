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

// 单目标 DataSink：将 TopSQL 上报数据经 gRPC 发送到唯一接收端。
//
// 维护注册状态、有界发送队列与后台 worker；支持接收地址轮询切换。
// TopRU（按 RU 聚合的 Top SQL）经本通道暂不支持发送。
// DataSink 是 Reporter 向外部代理投递采样数据的出口抽象。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    static_mut_refs
)]

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::datasink::{DataSink, DataSinkError, DataSinkRegisterer, ReportData};
use crate::tipb;
use crate::tipb::top_sql_agent_client::TopSqlAgentClient;
use futures_util::{FutureExt, StreamExt};
use prost::Message as ProstMessage;
use protobuf::Message as ProtobufMessage;
use reporter_metrics::reporter_metrics as metrics;
use tokio::runtime::{Builder, Runtime};
use tonic::transport::{Channel, Endpoint};

/// Convert a panic in one parallel send branch into a normal report error.
pub(crate) async fn recoverSendPanic<F>(future: F) -> anyhow::Result<()>
where
    F: Future<Output = anyhow::Result<()>>,
{
    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_owned());
            Err(anyhow::anyhow!("single target send panicked: {message}"))
        }
    }
}

/// 拨号超时。
const dialTimeout: Duration = Duration::from_secs(5);
/// gRPC 流初始窗口大小。
const grpcInitialWindowSize: u32 = 1 << 30;
/// gRPC 连接初始窗口大小。
const grpcInitialConnWindowSize: u32 = 1 << 30;

/// 单目标 DataSink 的非阻塞发送错误。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SingleTargetError {
    /// 有界发送队列已满。
    #[error("the channel of single target dataSink is full")]
    ChannelFull,
    /// DataSink 已关闭。
    #[error("single target dataSink is closed")]
    Closed,
}

/// Supplies the current TopSQL receiver address. The production integration
/// can bind this to config.GetGlobalConfig, while focused tests use the
/// mutable implementation below.
/// 提供当前 TopSQL 接收端地址；生产可绑定全局配置，测试用可变实现。
pub trait ReceiverAddressProvider: Send + Sync {
    /// 返回当前接收端地址字符串；空串表示未配置。
    fn ReceiverAddress(&self) -> String;
}

/// 可在运行时修改的接收端地址提供者（测试/动态配置用）。
#[derive(Default)]
pub struct MutableReceiverAddress {
    address: RwLock<String>,
}

impl MutableReceiverAddress {
    /// 以初始地址构造。
    pub fn new(address: String) -> Self {
        Self {
            address: RwLock::new(address),
        }
    }

    /// 更新接收端地址。
    pub fn Set(&self, address: String) {
        *self
            .address
            .write()
            .expect("receiver address lock poisoned") = address;
    }
}

impl ReceiverAddressProvider for MutableReceiverAddress {
    fn ReceiverAddress(&self) -> String {
        self.address
            .read()
            .expect("receiver address lock poisoned")
            .clone()
    }
}

/// 进程级默认接收端地址单例。
fn globalReceiverAddress() -> &'static Arc<MutableReceiverAddress> {
    static ADDRESS: OnceLock<Arc<MutableReceiverAddress>> = OnceLock::new();
    ADDRESS.get_or_init(|| Arc::new(MutableReceiverAddress::default()))
}

/// 设置全局默认接收端地址。
pub fn SetGlobalReceiverAddress(address: String) {
    globalReceiverAddress().Set(address);
}

/// 待发送任务：载荷与截止时间。
struct sendTask {
    data: Arc<ReportData>,
    deadline: Instant,
}

/// 当前 gRPC 客户端与已连接地址。
struct connectionState {
    conn: Option<TopSqlAgentClient<Channel>>,
    curRPCAddr: String,
}

impl connectionState {
    /// 以当前地址创建，连接稍后建立。
    fn new(curRPCAddr: String) -> Self {
        Self {
            conn: None,
            curRPCAddr,
        }
    }
}

/// Reports data to the single gRPC receiver configured by ReceiverAddress.
/// 将上报数据发送到 `ReceiverAddress` 配置的单一 gRPC 接收端。
pub struct SingleTargetDataSink {
    /// 向 Reporter 注册/注销本 DataSink。
    registerer: Arc<dyn DataSinkRegisterer>,
    /// 动态提供接收端地址。
    receiverAddress: Arc<dyn ReceiverAddressProvider>,
    sendTaskCh: crossbeam_channel::Sender<sendTask>,
    sendTaskRecv: crossbeam_channel::Receiver<sendTask>,
    cancelCh: crossbeam_channel::Sender<()>,
    cancelRecv: crossbeam_channel::Receiver<()>,
    /// 是否已在 registerer 中注册。
    registered: AtomicBool,
    /// Start 是否已启动 worker。
    started: AtomicBool,
    /// 是否已取消/关闭。
    cancelled: AtomicBool,
    /// 轮询地址与注册状态的间隔。
    pollInterval: Mutex<Duration>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

/// 使用全局接收端地址创建 DataSink。
pub fn NewSingleTargetDataSink(
    registerer: Arc<dyn DataSinkRegisterer>,
) -> Arc<SingleTargetDataSink> {
    NewSingleTargetDataSinkWithReceiver(registerer, globalReceiverAddress().clone())
}

/// 使用显式接收端地址提供者创建 DataSink（容量为 1 的有界队列）。
pub fn NewSingleTargetDataSinkWithReceiver(
    registerer: Arc<dyn DataSinkRegisterer>,
    receiverAddress: Arc<dyn ReceiverAddressProvider>,
) -> Arc<SingleTargetDataSink> {
    // 容量 1：与 Go 非阻塞语义一致，第二笔 TrySend 得 ChannelFull。
    let (sendTaskCh, sendTaskRecv) = crossbeam_channel::bounded(1);
    let (cancelCh, cancelRecv) = crossbeam_channel::bounded(1);
    Arc::new(SingleTargetDataSink {
        registerer,
        receiverAddress,
        sendTaskCh,
        sendTaskRecv,
        cancelCh,
        cancelRecv,
        registered: AtomicBool::new(false),
        started: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        pollInterval: Mutex::new(Duration::from_secs(1)),
        worker: Mutex::new(None),
    })
}

impl SingleTargetDataSink {
    /// Overrides the one-second Go registration ticker for focused tests.
    /// 覆盖默认 1s 注册轮询间隔，须在 Start 前调用。
    pub fn SetPollInterval(&self, interval: Duration) {
        assert!(!interval.is_zero(), "poll interval must be positive");
        assert!(
            !self.started.load(Ordering::SeqCst),
            "poll interval must be set before Start"
        );
        *self
            .pollInterval
            .lock()
            .expect("poll interval lock poisoned") = interval;
    }

    /// 启动后台 worker；若地址非空则立即注册 DataSink。
    pub fn Start(self: &Arc<Self>) {
        if self.cancelled.load(Ordering::SeqCst) || self.started.swap(true, Ordering::SeqCst) {
            return;
        }

        let addr = self.receiverAddress.ReceiverAddress();
        if !addr.is_empty() {
            match self.registerer.register(self.clone() as Arc<dyn DataSink>) {
                Ok(()) => self.registered.store(true, Ordering::SeqCst),
                Err(error) => log::warn!("failed to register single target datasink: {error}"),
            }
        }

        let workerSink = self.clone();
        let workerAddr = addr;
        let worker = std::thread::spawn(move || workerSink.recoverRun(workerAddr));
        *self.worker.lock().expect("worker lock poisoned") = Some(worker);
    }

    /// 捕获 panic 后重启 `run`，对齐 Go recover 循环。
    fn recoverRun(self: Arc<Self>, curRPCAddr: String) {
        let runtime = match Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(error) => {
                log::error!("failed to create single target runtime: {error}");
                return;
            }
        };
        let mut connection = connectionState::new(curRPCAddr);
        loop {
            let result = catch_unwind(AssertUnwindSafe(|| self.run(&runtime, &mut connection)));
            match result {
                Ok(false) => return,
                Ok(true) => continue,
                Err(_) => log::error!("panic in SingleTargetDataSink, rerun"),
            }
        }
    }

    /// Runs until cancelled. A panic is converted to `true` by recoverRun so
    /// the loop is restarted, matching the Go recover behavior.
    /// 直到取消：处理发送任务与地址轮询，并按需切换注册。
    fn run(self: &Arc<Self>, runtime: &Runtime, connection: &mut connectionState) -> bool {
        let interval = *self
            .pollInterval
            .lock()
            .expect("poll interval lock poisoned");
        let ticker = crossbeam_channel::tick(interval);
        loop {
            let targetRPCAddr = crossbeam_channel::select! {
                recv(self.cancelRecv) -> _ => return false,
                recv(self.sendTaskRecv) -> task => {
                    let Ok(task) = task else { return false; };
                    let addr = self.receiverAddress.ReceiverAddress();
                    if !addr.is_empty() {
                        if let Err(error) = runtime.block_on(self.doSend(connection, &addr, task)) {
                            log::warn!("single target data sink failed to send data to receiver (category=top-sql): {error}");
                        }
                    }
                    addr
                },
                recv(ticker) -> _ => self.receiverAddress.ReceiverAddress(),
            };

            if let Err(error) = self.trySwitchRegistration(&targetRPCAddr) {
                log::warn!("failed to register the single target datasink: {error}");
                return false;
            }
        }
    }

    /// 地址变空则注销；非空且未注册则注册。
    fn trySwitchRegistration(self: &Arc<Self>, addr: &str) -> anyhow::Result<()> {
        if addr.is_empty() && self.registered.swap(false, Ordering::SeqCst) {
            let data_sink: Arc<dyn DataSink> = self.clone();
            self.registerer.deregister(&data_sink);
            return Ok(());
        }

        if self.cancelled.load(Ordering::SeqCst) {
            return Ok(());
        }

        if !addr.is_empty() && !self.registered.load(Ordering::SeqCst) {
            self.registerer
                .register(self.clone() as Arc<dyn DataSink>)?;
            self.registered.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    /// 非阻塞入队；队列满返回 ChannelFull，已关闭返回 Closed。
    pub fn TrySend(&self, data: ReportData, deadline: Instant) -> Result<(), SingleTargetError> {
        self.try_send_inner(Arc::new(data), deadline)
    }

    /// 公开 DataSink trait 的 Arc 载荷入队实现。
    fn try_send_inner(
        &self,
        data: Arc<ReportData>,
        deadline: Instant,
    ) -> Result<(), SingleTargetError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(SingleTargetError::Closed);
        }
        match self.sendTaskCh.try_send(sendTask { data, deadline }) {
            Ok(()) => Ok(()),
            Err(crossbeam_channel::TrySendError::Full(_)) => {
                incrementIgnoreReportChannelFull();
                Err(SingleTargetError::ChannelFull)
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => Err(SingleTargetError::Closed),
        }
    }

    /// Reporter 关闭回调：取消 worker。
    pub fn OnReporterClosing(&self) {
        self.cancelWorker();
    }

    /// 关闭：取消 worker、注销并 join 后台线程。
    pub fn Close(self: &Arc<Self>) {
        self.cancelWorker();
        if self.registered.swap(false, Ordering::SeqCst) {
            let data_sink: Arc<dyn DataSink> = self.clone();
            self.registerer.deregister(&data_sink);
        }
        if let Some(worker) = self.worker.lock().expect("worker lock poisoned").take() {
            if worker.thread().id() != std::thread::current().id() && worker.join().is_err() {
                log::error!("single target dataSink worker panicked");
            }
        }
    }

    /// 幂等取消：首次置位并通知 cancel 通道。
    fn cancelWorker(&self) {
        if !self.cancelled.swap(true, Ordering::SeqCst) {
            let _ = self.cancelCh.try_send(());
        }
    }

    /// 建立连接后并行发送 SQL/Plan 元数据与 TopSQL 记录（TopRU 空操作）。
    async fn doSend(
        &self,
        connection: &mut connectionState,
        addr: &str,
        task: sendTask,
    ) -> anyhow::Result<()> {
        let start = Instant::now();
        let result = async {
            self.tryEstablishConnection(connection, addr, task.deadline)
                .await?;
            let client = connection
                .conn
                .as_ref()
                .expect("connection established")
                .clone();
            let DataRecords = protobuf_to_prost_vec(&task.data.data_records)?;
            let RURecords = protobuf_to_prost_vec(&task.data.ru_records)?;
            let SQLMetas = protobuf_to_prost_vec(&task.data.sql_metas)?;
            let PlanMetas = protobuf_to_prost_vec(&task.data.plan_metas)?;

            let (sql, plan, records, ru) = tokio::join!(
                recoverSendPanic(self.sendBatchSQLMeta(client.clone(), SQLMetas, task.deadline)),
                recoverSendPanic(self.sendBatchPlanMeta(client.clone(), PlanMetas, task.deadline)),
                recoverSendPanic(self.sendBatchTopSQLRecord(client, DataRecords, task.deadline)),
                recoverSendPanic(self.sendBatchTopRURecord(RURecords)),
            );
            sql?;
            plan?;
            records?;
            ru?;
            anyhow::Ok(())
        }
        .await;

        observeAll(start.elapsed(), result.is_ok());
        result
    }

    /// 流式上报 TopSQL 记录并观察耗时/条数指标。
    async fn sendBatchTopSQLRecord(
        &self,
        mut client: TopSqlAgentClient<Channel>,
        records: Vec<tipb::TopSqlRecord>,
        deadline: Instant,
    ) -> anyhow::Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let start = Instant::now();
        let sentCount = Arc::new(AtomicUsize::new(0));
        let counter = sentCount.clone();
        let stream = tokio_stream::iter(records).inspect(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let result = async {
            let mut request = tonic::Request::new(stream);
            request.set_timeout(remaining(deadline)?);
            client.report_top_sql_records(request).await?;
            anyhow::Ok(())
        }
        .await;
        observeRecords(
            start.elapsed(),
            sentCount.load(Ordering::Relaxed),
            result.is_ok(),
        );
        result
    }

    /// TopRU over SingleTarget is intentionally unsupported now.
    /// 单目标通道暂不发送 TopRU 记录。
    async fn sendBatchTopRURecord(&self, _records: Vec<tipb::TopRuRecord>) -> anyhow::Result<()> {
        Ok(())
    }

    /// 流式上报 SQL 元数据。
    async fn sendBatchSQLMeta(
        &self,
        mut client: TopSqlAgentClient<Channel>,
        sqlMetas: Vec<tipb::SqlMeta>,
        deadline: Instant,
    ) -> anyhow::Result<()> {
        if sqlMetas.is_empty() {
            return Ok(());
        }
        let start = Instant::now();
        let sentCount = Arc::new(AtomicUsize::new(0));
        let counter = sentCount.clone();
        let stream = tokio_stream::iter(sqlMetas).inspect(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let result = async {
            let mut request = tonic::Request::new(stream);
            request.set_timeout(remaining(deadline)?);
            client.report_sql_meta(request).await?;
            anyhow::Ok(())
        }
        .await;
        observeSQL(
            start.elapsed(),
            sentCount.load(Ordering::Relaxed),
            result.is_ok(),
        );
        result
    }

    /// 流式上报执行计划元数据。
    async fn sendBatchPlanMeta(
        &self,
        mut client: TopSqlAgentClient<Channel>,
        planMetas: Vec<tipb::PlanMeta>,
        deadline: Instant,
    ) -> anyhow::Result<()> {
        if planMetas.is_empty() {
            return Ok(());
        }
        let start = Instant::now();
        let sentCount = Arc::new(AtomicUsize::new(0));
        let counter = sentCount.clone();
        let stream = tokio_stream::iter(planMetas).inspect(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let result = async {
            let mut request = tonic::Request::new(stream);
            request.set_timeout(remaining(deadline)?);
            client.report_plan_meta(request).await?;
            anyhow::Ok(())
        }
        .await;
        observePlans(
            start.elapsed(),
            sentCount.load(Ordering::Relaxed),
            result.is_ok(),
        );
        result
    }

    /// 若地址变化则重建 gRPC 连接；复用未变化的已有连接。
    async fn tryEstablishConnection(
        &self,
        connection: &mut connectionState,
        targetRPCAddr: &str,
        deadline: Instant,
    ) -> anyhow::Result<()> {
        if connection.curRPCAddr == targetRPCAddr && connection.conn.is_some() {
            return Ok(());
        }

        // Dropping tonic's Channel clone closes the old connection when the
        // final in-flight RPC completes.
        // 丢弃旧 Channel；在途 RPC 结束后连接关闭。
        connection.conn.take();
        let uri = if targetRPCAddr.contains("://") {
            targetRPCAddr.to_owned()
        } else {
            format!("http://{targetRPCAddr}")
        };
        let timeout = remaining(deadline)?.min(dialTimeout);
        let endpoint = Endpoint::from_shared(uri)?
            .connect_timeout(timeout)
            .initial_stream_window_size(grpcInitialWindowSize)
            .initial_connection_window_size(grpcInitialConnWindowSize);
        let channel = tokio::time::timeout(timeout, endpoint.connect())
            .await
            .map_err(|_| anyhow::anyhow!("single target dial timed out"))??;
        connection.conn = Some(
            TopSqlAgentClient::new(channel)
                .max_decoding_message_size(usize::MAX)
                .max_encoding_message_size(usize::MAX),
        );
        connection.curRPCAddr = targetRPCAddr.to_owned();
        Ok(())
    }
}

impl DataSink for SingleTargetDataSink {
    fn try_send(&self, data: Arc<ReportData>, deadline: Instant) -> Result<(), DataSinkError> {
        self.try_send_inner(data, deadline)
            .map_err(|error| match error {
                SingleTargetError::ChannelFull => DataSinkError::ChannelFull,
                SingleTargetError::Closed => DataSinkError::Closed,
            })
    }

    fn on_reporter_closing(&self) {
        SingleTargetDataSink::OnReporterClosing(self)
    }
}

/// TopRU 记录流抽象，便于单测模拟 Unimplemented 等状态。
pub trait TopRURecordStream {
    /// 发送单条 TopRU 记录。
    fn Send(&mut self, record: &tipb::TopRuRecord) -> Result<(), tonic::Status>;
    /// 关闭流并接收空响应。
    fn CloseAndRecv(&mut self) -> Result<tipb::EmptyResponse, tonic::Status>;
}

/// 向流发送 TopRU 记录；对 Unimplemented 视为兼容并关闭流后返回已发送数。
pub fn sendTopRURecords(
    stream: &mut dyn TopRURecordStream,
    records: &[tipb::TopRuRecord],
) -> Result<usize, tonic::Status> {
    let mut sentCount = 0;
    let mut retErr = None;
    for record in records {
        if let Err(error) = stream.Send(record) {
            retErr = Some(error);
            break;
        }
        sentCount += 1;
    }

    // 对端未实现 TopRU RPC：关闭流并当作成功兼容。
    if retErr
        .as_ref()
        .is_some_and(|error| error.code() == tonic::Code::Unimplemented)
    {
        let _ = stream.CloseAndRecv();
        return Ok(sentCount);
    }

    if let Err(closeError) = stream.CloseAndRecv() {
        if closeError.code() == tonic::Code::Unimplemented {
            return Ok(sentCount);
        }
        if retErr.is_none() {
            retErr = Some(closeError);
        }
    }

    match retErr {
        Some(error) => Err(error),
        None => Ok(sentCount),
    }
}

/// 计算距 deadline 的剩余时长；已超时则报错。
fn remaining(deadline: Instant) -> anyhow::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| anyhow::anyhow!("single target send deadline exceeded"))
}

/// protobuf-codec 与 prost 共享 wire format；仅在 tonic 边界做可靠转换。
fn protobuf_to_prost_vec<P, T>(messages: &[P]) -> anyhow::Result<Vec<T>>
where
    P: ProtobufMessage,
    T: ProstMessage + Default,
{
    messages
        .iter()
        .map(|message| {
            let bytes = message.write_to_bytes()?;
            T::decode(bytes.as_slice()).map_err(Into::into)
        })
        .collect()
}

/// 队列满时递增忽略上报计数指标。
fn incrementIgnoreReportChannelFull() {
    unsafe {
        if let Some(counter) = metrics::IgnoreReportChannelFullCounter.as_ref() {
            counter.inc();
        }
    }
}

/// 观察整批发送耗时直方图。
fn observeAll(elapsed: Duration, succeeded: bool) {
    unsafe {
        let histogram = if succeeded {
            metrics::ReportAllDurationSuccHistogram.as_ref()
        } else {
            metrics::ReportAllDurationFailedHistogram.as_ref()
        };
        if let Some(histogram) = histogram {
            histogram.observe(elapsed.as_secs_f64());
        }
    }
}

/// 观察 TopSQL 记录条数与发送耗时。
fn observeRecords(elapsed: Duration, sentCount: usize, succeeded: bool) {
    unsafe {
        if let Some(histogram) = metrics::TopSQLReportRecordCounterHistogram.as_ref() {
            histogram.observe(sentCount as f64);
        }
        let histogram = if succeeded {
            metrics::ReportRecordDurationSuccHistogram.as_ref()
        } else {
            metrics::ReportRecordDurationFailedHistogram.as_ref()
        };
        if let Some(histogram) = histogram {
            histogram.observe(elapsed.as_secs_f64());
        }
    }
}

/// 观察 SQL 元数据条数与发送耗时。
fn observeSQL(elapsed: Duration, sentCount: usize, succeeded: bool) {
    unsafe {
        if let Some(histogram) = metrics::TopSQLReportSQLCountHistogram.as_ref() {
            histogram.observe(sentCount as f64);
        }
        let histogram = if succeeded {
            metrics::ReportSQLDurationSuccHistogram.as_ref()
        } else {
            metrics::ReportSQLDurationFailedHistogram.as_ref()
        };
        if let Some(histogram) = histogram {
            histogram.observe(elapsed.as_secs_f64());
        }
    }
}

/// 观察 Plan 元数据条数与发送耗时。
fn observePlans(elapsed: Duration, sentCount: usize, succeeded: bool) {
    unsafe {
        if let Some(histogram) = metrics::TopSQLReportPlanCountHistogram.as_ref() {
            histogram.observe(sentCount as f64);
        }
        let histogram = if succeeded {
            metrics::ReportPlanDurationSuccHistogram.as_ref()
        } else {
            metrics::ReportPlanDurationFailedHistogram.as_ref()
        };
        if let Some(histogram) = histogram {
            histogram.observe(elapsed.as_secs_f64());
        }
    }
}
