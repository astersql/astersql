// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// TopSQL / TopRU 的 PubSub DataSink 实现。
//
// 解析订阅请求中的 collector 类型与 TopRU 采样间隔，将 ReportData
// 经有界通道异步推送到 `PubSubStream`；通道满时丢弃并记指标。
// `TopSqlPubSubService` 负责向 registerer 注册/注销订阅生命周期。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded, select};

use crate::datasink::{
    DataSink, DataSinkError, DataSinkRegisterer, ReportData, SubscriptionConfig,
};
use crate::tipb_protobuf as tipb;

/// 解析是否订阅 TopSQL：无请求或 collectors 为空/含 Topsql/Unspecified 则启用。
pub fn parse_top_sql_subscription(request: Option<&tipb::TopSqlSubRequest>) -> bool {
    let Some(request) = request else {
        return true;
    };
    let collectors = request.get_collectors();
    collectors.is_empty()
        || collectors.iter().any(|collector| {
            let collector = *collector;
            matches!(
                collector,
                tipb::CollectorType::CollectorTypeTopsql
                    | tipb::CollectorType::CollectorTypeUnspecified
            )
        })
}

/// 解析 TopRU 订阅：需含 CollectorTypeTopru 且带 TopRuConfig，返回 (启用, 间隔秒)。
pub fn parse_top_ru_subscription(
    request: Option<&tipb::TopSqlSubRequest>,
) -> Result<(bool, i32), DataSinkError> {
    let Some(request) = request else {
        return Ok((false, 0));
    };
    let enabled = request
        .get_collectors()
        .contains(&tipb::CollectorType::CollectorTypeTopru);
    if !enabled {
        return Ok((false, 0));
    }
    if !request.has_topru() {
        return Err(DataSinkError::TopRuConfigEmpty);
    }
    Ok((true, request.get_topru().get_item_interval_seconds() as i32))
}

/// 推送给订阅端的统一响应变体。
#[derive(Clone, Debug)]
pub enum PubSubResponse {
    TopSqlRecord(tipb::TopSqlRecord),
    TopRuRecord(tipb::TopRuRecord),
    SqlMeta(tipb::SqlMeta),
    PlanMeta(tipb::PlanMeta),
}

/// 订阅流抽象：将一条 PubSubResponse 写出。
pub trait PubSubStream: Send + Sync + 'static {
    fn send(&self, response: PubSubResponse) -> Result<(), DataSinkError>;
}

/// PubSub 发送侧原子指标（通道满忽略、各类发送成功/失败计数）。
#[derive(Default)]
pub struct PubSubMetrics {
    pub ignored_channel_full: AtomicU64,
    pub sent_top_sql_records: AtomicU64,
    pub sent_top_ru_records: AtomicU64,
    pub sent_sql_metas: AtomicU64,
    pub sent_plan_metas: AtomicU64,
    pub send_failures: AtomicU64,
}

/// 进程级 PubSub 指标单例。
pub static PUBSUB_METRICS: PubSubMetrics = PubSubMetrics {
    ignored_channel_full: AtomicU64::new(0),
    sent_top_sql_records: AtomicU64::new(0),
    sent_top_ru_records: AtomicU64::new(0),
    sent_sql_metas: AtomicU64::new(0),
    sent_plan_metas: AtomicU64::new(0),
    send_failures: AtomicU64::new(0),
};

/// 待发送任务：报告数据与截止时间。
struct SendTask {
    data: Arc<ReportData>,
    deadline: Instant,
}

/// 基于有界通道的 DataSink：收集侧 try_send，运行侧 execute_task/do_send。
pub struct PubSubDataSink {
    stream: Arc<dyn PubSubStream>,
    sender: Sender<SendTask>,
    receiver: Receiver<SendTask>,
    cancel_sender: Sender<()>,
    cancel_receiver: Receiver<()>,
    cancelled: AtomicBool,
    enable_top_sql: bool,
    enable_top_ru: bool,
    item_interval: i32,
}

impl PubSubDataSink {
    /// 构造 sink；发送通道容量为 1，与 Go 背压语义对齐。
    pub fn new(
        stream: Arc<dyn PubSubStream>,
        enable_top_sql: bool,
        enable_top_ru: bool,
        item_interval: i32,
    ) -> Self {
        let (sender, receiver) = bounded(1);
        let (cancel_sender, cancel_receiver) = bounded(1);
        Self {
            stream,
            sender,
            receiver,
            cancel_sender,
            cancel_receiver,
            cancelled: AtomicBool::new(false),
            enable_top_sql,
            enable_top_ru,
            item_interval,
        }
    }

    /// 从订阅请求解析开关后构造 sink。
    pub fn from_request(
        request: Option<&tipb::TopSqlSubRequest>,
        stream: Arc<dyn PubSubStream>,
    ) -> Result<Self, DataSinkError> {
        let enable_top_sql = parse_top_sql_subscription(request);
        let (enable_top_ru, item_interval) = parse_top_ru_subscription(request)?;
        Ok(Self::new(
            stream,
            enable_top_sql,
            enable_top_ru,
            item_interval,
        ))
    }

    /// 阻塞循环：处理 cancel 或发送任务直至关闭。
    pub fn run(&self) -> Result<(), DataSinkError> {
        loop {
            select! {
                recv(self.cancel_receiver) -> _ => return Err(DataSinkError::Closed),
                recv(self.receiver) -> task => {
                    self.execute_task(task.map_err(|_| DataSinkError::Closed)?)?;
                }
            }
        }
    }

    /// 非阻塞处理至多一个任务；无任务返回 Ok(false)。
    pub fn run_one(&self) -> Result<bool, DataSinkError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(DataSinkError::Closed);
        }
        match self.receiver.try_recv() {
            Ok(task) => {
                self.execute_task(task)?;
                Ok(true)
            }
            Err(TryRecvError::Empty) => Ok(false),
            Err(TryRecvError::Disconnected) => Err(DataSinkError::Closed),
        }
    }

    /// 检查 deadline 后调用 do_send_until；失败累计 send_failures。
    fn execute_task(&self, task: SendTask) -> Result<(), DataSinkError> {
        if Instant::now() >= task.deadline {
            PUBSUB_METRICS.send_failures.fetch_add(1, Ordering::Relaxed);
            return Err(DataSinkError::DeadlineExceeded);
        }
        let result = self.do_send_until(task.data.as_ref(), Some(task.deadline));
        if result.is_err() {
            PUBSUB_METRICS.send_failures.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// 无截止时间地发送一整份 ReportData。
    pub fn do_send(&self, data: &ReportData) -> Result<(), DataSinkError> {
        self.do_send_until(data, None)
    }

    /// 按开关顺序发送 TopSQL → TopRU → SqlMeta → PlanMeta。
    fn do_send_until(
        &self,
        data: &ReportData,
        deadline: Option<Instant>,
    ) -> Result<(), DataSinkError> {
        // TopSQL 与 TopRU 受订阅开关/全局 TopRUEnabled 门控；meta 始终发送。
        if self.enable_top_sql {
            for record in &data.data_records {
                self.send(PubSubResponse::TopSqlRecord(record.clone()), deadline)?;
                PUBSUB_METRICS
                    .sent_top_sql_records
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        if self.enable_top_ru && crate::topsql_state::TopRUEnabled() {
            for record in &data.ru_records {
                self.send(PubSubResponse::TopRuRecord(record.clone()), deadline)?;
                PUBSUB_METRICS
                    .sent_top_ru_records
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        for meta in &data.sql_metas {
            self.send(PubSubResponse::SqlMeta(meta.clone()), deadline)?;
            PUBSUB_METRICS
                .sent_sql_metas
                .fetch_add(1, Ordering::Relaxed);
        }
        for meta in &data.plan_metas {
            self.send(PubSubResponse::PlanMeta(meta.clone()), deadline)?;
            PUBSUB_METRICS
                .sent_plan_metas
                .fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    /// 单条写出；已取消或逾时则返回对应错误。
    fn send(
        &self,
        response: PubSubResponse,
        deadline: Option<Instant>,
    ) -> Result<(), DataSinkError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(DataSinkError::Closed);
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(DataSinkError::DeadlineExceeded);
        }
        self.stream.send(response)?;
        // Go 在每次 stream.Send 成功后检查 ctx.Done；最后一条发送期间越过
        // deadline 或收到取消也必须向 run 返回对应错误，而不是误报整批成功。
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(DataSinkError::Closed);
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(DataSinkError::DeadlineExceeded);
        }
        Ok(())
    }

    /// 幂等取消：置位并通知 run 循环退出。
    pub fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::SeqCst) {
            let _ = self.cancel_sender.try_send(());
        }
    }
}

/// PubSub 订阅服务：注册 DataSink 并驱动 `run` 直至结束。
pub struct TopSqlPubSubService {
    registerer: Arc<dyn DataSinkRegisterer>,
}

impl TopSqlPubSubService {
    /// 绑定 DataSinkRegisterer。
    pub fn new(registerer: Arc<dyn DataSinkRegisterer>) -> Self {
        Self { registerer }
    }

    /// 注册 sink、阻塞 run，结束后 deregister/cancel；panic 转 Stream 错误。
    pub fn subscribe(
        &self,
        request: Option<&tipb::TopSqlSubRequest>,
        stream: Arc<dyn PubSubStream>,
    ) -> Result<(), DataSinkError> {
        let sink = Arc::new(PubSubDataSink::from_request(request, stream)?);
        let data_sink: Arc<dyn DataSink> = sink.clone();
        self.registerer.register(data_sink.clone())?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink.run()));
        self.registerer.deregister(&data_sink);
        sink.cancel();
        // Go 的 run defer 会 recover 发送路径 panic，完成注销/取消后向
        // Subscribe 返回 nil；这里保持相同的服务端契约。
        result.unwrap_or(Ok(()))
    }
}

/// Go 风格构造函数。
#[allow(non_snake_case)]
pub fn NewTopSQLPubSubService(registerer: Arc<dyn DataSinkRegisterer>) -> TopSqlPubSubService {
    TopSqlPubSubService::new(registerer)
}

impl DataSink for PubSubDataSink {
    // 通道满记 ignored_channel_full 并返回 ChannelFull。
    fn try_send(&self, data: Arc<ReportData>, deadline: Instant) -> Result<(), DataSinkError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(DataSinkError::Closed);
        }
        match self.sender.try_send(SendTask { data, deadline }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                PUBSUB_METRICS
                    .ignored_channel_full
                    .fetch_add(1, Ordering::Relaxed);
                Err(DataSinkError::ChannelFull)
            }
            Err(TrySendError::Disconnected(_)) => Err(DataSinkError::Closed),
        }
    }

    fn on_reporter_closing(&self) {
        self.cancel();
    }

    fn subscription_config(&self) -> Option<SubscriptionConfig> {
        Some(SubscriptionConfig {
            enable_top_sql: self.enable_top_sql,
            enable_top_ru: self.enable_top_ru,
            item_interval: self.item_interval,
        })
    }
}
