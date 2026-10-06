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

// 本地 MPP 协调器（LocalMppCoordinator）。
//
// 负责将物理计划 Fragment 编码为派发请求，经 `CoordinatorTransport` 下发到
// TiFlash，并行拉流聚合为 `kv::Response`；并可选等待各 task 的 execution
// summary（ReportStatus）合并到语句侧。MPP = Massively Parallel Processing。

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use astersql_errors as errors;
use astersql_kv as kv;
use astersql_planner_core_base::{BuildPBContext, PhysicalPlan};
use astersql_planner_core_operator_physicalop::{
    Fragment, PhysicalLimit, PhysicalTableReader, RootMppTaskGenerator, SessionRootMppTaskGenerator,
};
use astersql_util_execdetails::execdetails::{CopRuntimeStats, ExecDetails};
use kvproto::mpp::{MppDataPacket, TaskMeta};
use protobuf::{Message, ProtobufEnum};

/// 将 TiFlash execution summary 记入语句侧运行时统计的回调接口。
pub trait MppReportSink: Send + Sync {
    /// 记录单条 Cop 任务 summary，返回对应 plan id（失败或忽略可返回 -1）。
    fn RecordOneCopTask(
        &self,
        summary: &tipb::ExecutorExecutionSummary,
    ) -> Result<i32, errors::SharedError>;
    /// 合并 TiFlash RU（Resource Unit，资源计量单位）消耗。
    fn MergeTiFlashRUConsumption(
        &self,
        summaries: &[tipb::ExecutorExecutionSummary],
    ) -> Result<(), errors::SharedError>;
    /// 为未上报的 plan id 填充占位 summary。
    fn FillDummySummaries(
        &self,
        plan_ids: &[i32],
        recorded_plan_ids: &HashSet<i32>,
    ) -> Result<(), errors::SharedError>;
    /// report 等待超时时上报指标。
    fn ReportTimeout(&self, expected: usize, received: usize, start_ts: u64, gather_id: u64);
}

/// 空操作 ReportSink，测试或不需要 summary 时使用。
#[derive(Default)]
pub struct NoopMppReportSink;

impl MppReportSink for NoopMppReportSink {
    fn RecordOneCopTask(
        &self,
        _: &tipb::ExecutorExecutionSummary,
    ) -> Result<i32, errors::SharedError> {
        Ok(-1)
    }
    fn MergeTiFlashRUConsumption(
        &self,
        _: &[tipb::ExecutorExecutionSummary],
    ) -> Result<(), errors::SharedError> {
        Ok(())
    }
    fn FillDummySummaries(&self, _: &[i32], _: &HashSet<i32>) -> Result<(), errors::SharedError> {
        Ok(())
    }
    fn ReportTimeout(&self, _: usize, _: usize, _: u64, _: u64) {}
}

/// 单次派发流上的数据包及耗时。
pub(crate) struct DispatchResponse {
    pub(crate) packet: MppDataPacket,
    pub(crate) runtime_stats: Option<CopRuntimeStats>,
    pub(crate) elapsed: Duration,
}

/// 根任务建立连接后的拉流接口。
pub(crate) trait CoordinatorResponseStream: Send {
    fn Next(&mut self) -> Result<Option<DispatchResponse>, errors::SharedError>;
    fn Close(&mut self) -> Result<(), errors::SharedError>;
}

/// 协调器状态机使用的 RPC 边界：Dispatch / Cancel / CheckVisibility。
///
/// RPC boundary used by the coordinator state machine.
pub(crate) trait CoordinatorTransport: Send + Sync {
    fn Dispatch(
        &self,
        context: &kv::Context,
        request: &kv::MPPDispatchRequest,
    ) -> Result<Option<Box<dyn CoordinatorResponseStream>>, errors::SharedError>;

    fn Cancel(
        &self,
        context: &kv::Context,
        store_addresses: HashMap<String, bool>,
        requests: &[kv::MPPDispatchRequest],
    ) -> Result<(), errors::SharedError>;

    fn CheckVisibility(&self, start_ts: u64) -> Result<(), errors::SharedError>;
}

/// 生产环境传输适配：包装 `kv::MPPClient`；保留重试标志，包经流式拉取。
///
/// Production transport adapter for the repository's `kv::MPPClient` RPC
/// contract. Dispatch and stream establishment retain TiDB's retry flags, and
/// packets remain pull-based through `CoordinatorResponseStream`.
pub(crate) struct MppClientCoordinatorTransport {
    client: Arc<dyn kv::MPPClient>,
}

impl MppClientCoordinatorTransport {
    /// 用给定 MPPClient 构造传输层。
    pub(crate) fn New(client: Arc<dyn kv::MPPClient>) -> Self {
        Self { client }
    }
}

/// 将 MPPStreamResponse 适配为 CoordinatorResponseStream。
struct MppClientResponseStream {
    stream: kv::MPPStreamResponse,
    opened_at: Instant,
    closed: bool,
}

impl CoordinatorResponseStream for MppClientResponseStream {
    fn Next(&mut self) -> Result<Option<DispatchResponse>, errors::SharedError> {
        let packet = self.stream.Recv()?;
        Ok(packet.map(|packet| DispatchResponse {
            packet,
            runtime_stats: None,
            elapsed: self.opened_at.elapsed(),
        }))
    }

    fn Close(&mut self) -> Result<(), errors::SharedError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.stream.Close()
    }
}

impl CoordinatorTransport for MppClientCoordinatorTransport {
    fn Dispatch(
        &self,
        context: &kv::Context,
        request: &kv::MPPDispatchRequest,
    ) -> Result<Option<Box<dyn CoordinatorResponseStream>>, errors::SharedError> {
        let mut backoffer = kv::Backoffer::default();
        loop {
            let (response, retry) = self.client.DispatchMPPTask(kv::DispatchMPPTaskParam {
                Ctx: context,
                Req: request,
                EnableCollectExecutionInfo: request.ReportExecutionSummary,
                Bo: &mut backoffer,
            })?;
            if retry {
                continue;
            }
            if response.has_error() {
                return Err(errors::New(response.get_error().get_msg().to_owned()));
            }
            break;
        }

        if !request.IsRoot {
            return Ok(None);
        }

        let address = request
            .Meta
            .as_ref()
            .ok_or_else(|| {
                errors::New(format!(
                    "root MPP task {} has no store metadata",
                    request.ID
                ))
            })?
            .GetAddress();
        let task_meta = kv::TaskMeta {
            start_ts: request.StartTs,
            task_id: request.ID,
            address,
            gather_id: request.GatherID,
            query_ts: request.MppQueryID.QueryTs,
            local_query_id: request.MppQueryID.LocalQueryID,
            server_id: request.MppQueryID.ServerID,
            mpp_version: request.MppVersion.ToInt64(),
            resource_group_name: request.ResourceGroupName.clone(),
            connection_id: request.ConnectionID,
            connection_alias: request.ConnectionAlias.clone(),
            sql_digest: request.SQLDigest.clone(),
            plan_digest: request.PlanDigest.clone(),
            ..Default::default()
        };
        loop {
            let (stream, retry) = self.client.EstablishMPPConns(kv::EstablishMPPConnsParam {
                Ctx: context,
                Req: request,
                TaskMeta: &task_meta,
                Bo: &mut backoffer,
            })?;
            if retry {
                continue;
            }
            return Ok(Some(Box::new(MppClientResponseStream {
                stream,
                opened_at: Instant::now(),
                closed: false,
            })));
        }
    }

    fn Cancel(
        &self,
        _context: &kv::Context,
        store_addresses: HashMap<String, bool>,
        requests: &[kv::MPPDispatchRequest],
    ) -> Result<(), errors::SharedError> {
        self.client.CancelMPPTasks(kv::CancelMPPTasksParam {
            StoreAddr: store_addresses,
            Reqs: requests.to_vec(),
        });
        Ok(())
    }

    fn CheckVisibility(&self, start_ts: u64) -> Result<(), errors::SharedError> {
        self.client.CheckVisibility(start_ts)
    }
}

/// 单个派发任务的 status report 缓存。
#[derive(Clone)]
struct MppRequestReport {
    request_index: usize,
    error_message: String,
    execution_summaries: Vec<tipb::ExecutorExecutionSummary>,
    received_report: bool,
}

/// ReportStatus 等待期间的共享状态。
#[derive(Default)]
struct MppReportState {
    requests: HashMap<i64, MppRequestReport>,
    reported_request_count: usize,
    first_error_message: String,
}

/// wait_and_collect 的一次性结果快照。
struct MppReportCollection {
    reports: Vec<MppRequestReport>,
    expected: usize,
    received: usize,
    timed_out: bool,
}

/// 收集各 task ReportStatus 的句柄；实现 MppStatusReporter。
pub(crate) struct MppReportHandle {
    state: Mutex<MppReportState>,
    ready: Condvar,
    waiting: AtomicBool,
}

impl MppReportHandle {
    /// 创建空的 report 句柄。
    fn new() -> Self {
        Self {
            state: Mutex::new(MppReportState::default()),
            ready: Condvar::new(),
            waiting: AtomicBool::new(false),
        }
    }

    /// 为 task_id 预留 report 槽位。
    fn install_request(&self, task_id: i64, request_index: usize) {
        if let Ok(mut state) = self.state.lock() {
            state.requests.insert(
                task_id,
                MppRequestReport {
                    request_index,
                    error_message: String::new(),
                    execution_summaries: Vec::new(),
                    received_report: false,
                },
            );
        }
    }

    /// 阻塞等待全部 report 或超时，按 request_index 排序返回。
    fn wait_and_collect(
        &self,
        timeout: Duration,
    ) -> Result<MppReportCollection, errors::SharedError> {
        let state = self
            .state
            .lock()
            .map_err(|_| errors::New("MPP report state lock is poisoned"))?;
        let expected = state.requests.len();
        self.waiting.store(true, Ordering::Release);
        let (state, wait_result) = self
            .ready
            .wait_timeout_while(state, timeout, |state| {
                state.reported_request_count < expected
            })
            .map_err(|_| errors::New("MPP report state lock is poisoned"))?;
        self.waiting.store(false, Ordering::Release);
        let received = state.reported_request_count;
        let mut reports: Vec<_> = state.requests.values().cloned().collect();
        reports.sort_by_key(|report| report.request_index);
        Ok(MppReportCollection {
            reports,
            expected,
            received,
            timed_out: wait_result.timed_out() && received < expected,
        })
    }

    #[cfg(test)]
    pub(crate) fn is_waiting(&self) -> bool {
        self.waiting.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn reported_request_count(&self) -> usize {
        self.state
            .lock()
            .map(|state| state.reported_request_count)
            .unwrap_or_default()
    }

    #[cfg(test)]
    fn execution_summary_count(&self, task_id: i64) -> Option<usize> {
        self.state.lock().ok().and_then(|state| {
            state
                .requests
                .get(&task_id)
                .map(|report| report.execution_summaries.len())
        })
    }

    /// 返回 ReportStatus 中最先收到的 task error，供流错误保持 Go 的文案优先级。
    fn first_error_message(&self) -> Option<String> {
        self.state.lock().ok().and_then(|state| {
            (!state.first_error_message.is_empty()).then(|| state.first_error_message.clone())
        })
    }
}

impl kv::MppStatusReporter for MppReportHandle {
    fn ReportStatus(&self, info: kv::ReportStatusRequest) -> Result<(), errors::SharedError> {
        let request = info.Request;
        let task_id = request.get_meta().get_task_id();
        let execution_summaries = if request.get_data().is_empty() {
            Vec::new()
        } else {
            let mut execution_info = tipb::TiFlashExecutionInfo::new();
            execution_info
                .merge_from_bytes(request.get_data())
                .map_err(|error| errors::New(error.to_string()))?;
            execution_info.get_execution_summaries().to_vec()
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| errors::New("MPP report state lock is poisoned"))?;
        let report = state.requests.get_mut(&task_id).ok_or_else(|| {
            errors::New(format!(
                "ReportMPPTaskStatus task not exists taskID: {task_id}"
            ))
        })?;
        if report.received_report {
            return Err(errors::New(format!(
                "ReportMPPTaskStatus task already received taskID: {task_id}"
            )));
        }
        report.received_report = true;
        let error_message = if request.has_error() {
            request.get_error().get_msg().to_owned()
        } else {
            String::new()
        };
        report.error_message = error_message.clone();
        report.execution_summaries = execution_summaries;
        state.reported_request_count += 1;
        if state.first_error_message.is_empty() && !error_message.is_empty() {
            state.first_error_message = error_message;
        }
        drop(state);
        self.ready.notify_all();
        Ok(())
    }
}

/// 派发工作线程发回主循环的事件。
enum WorkerEvent {
    Response {
        request_index: usize,
        response: DispatchResponse,
    },
    Error {
        request_index: usize,
        error: errors::SharedError,
    },
    Finished {
        request_index: usize,
    },
}

/// 本地 MPP 协调器：派发、拉流、取消与 summary 收集的状态机。
pub(crate) struct LocalMppCoordinator {
    transport: Arc<dyn CoordinatorTransport>,
    plan_ids: Vec<i32>,
    start_ts: u64,
    mpp_query_id: kv::MPPQueryID,
    gather_id: u64,
    coordinator_address: String,
    report_execution_info: bool,
    requests: Vec<kv::MPPDispatchRequest>,
    report_handle: Arc<MppReportHandle>,
    dispatch_task_ids: Vec<i64>,
    dispatch_store_ids: Vec<u64>,
    response_receiver: Option<mpsc::Receiver<WorkerEvent>>,
    workers: Vec<JoinHandle<()>>,
    active_workers: usize,
    stop_requested: Arc<AtomicBool>,
    kv_ranges: Vec<kv::KeyRange>,
    first_error_message: String,
    node_count: i32,
    completed_execution_summaries: Vec<tipb::ExecutorExecutionSummary>,
    report_sink: Arc<dyn MppReportSink>,
    report_timeout: Duration,
    executed: bool,
    closed: bool,
    dispatch_failed: bool,
    all_reports_handled: bool,
    execution_context: Option<kv::Context>,
}

/// 构造协调器；MPP < V2 时清空 coordinator_address 并关闭 report。
#[allow(clippy::too_many_arguments)]
pub(crate) fn new_local_mpp_coordinator(
    transport: Arc<dyn CoordinatorTransport>,
    original_plan: Box<dyn PhysicalPlan>,
    statement_plan: Option<&dyn PhysicalPlan>,
    plan_ids: Vec<i32>,
    start_ts: u64,
    mpp_query_id: kv::MPPQueryID,
    gather_id: u64,
    mut coordinator_address: String,
    chosen_mpp_version: kv::MppVersion,
    kv_ranges: Vec<kv::KeyRange>,
    node_count: i32,
    report_sink: Arc<dyn MppReportSink>,
    report_timeout: Duration,
) -> LocalMppCoordinator {
    // V1 协议无 coordinator 地址，无法收集远端 summary。
    if chosen_mpp_version < kv::MppVersionV2 {
        coordinator_address.clear();
    }
    // 仅当地址非空且语句计划满足 Limit+TableReader 形状时开启 report。
    let report_execution_info = !coordinator_address.is_empty()
        && statement_plan
            .is_some_and(|plan| need_report_execution_summary(plan, original_plan.id(), false));
    LocalMppCoordinator {
        transport,
        plan_ids,
        start_ts,
        mpp_query_id,
        gather_id,
        coordinator_address,
        report_execution_info,
        requests: Vec::new(),
        report_handle: Arc::new(MppReportHandle::new()),
        dispatch_task_ids: Vec::new(),
        dispatch_store_ids: Vec::new(),
        response_receiver: None,
        workers: Vec::new(),
        active_workers: 0,
        stop_requested: Arc::new(AtomicBool::new(false)),
        kv_ranges,
        first_error_message: String::new(),
        node_count,
        completed_execution_summaries: Vec::new(),
        report_sink,
        report_timeout,
        executed: false,
        closed: false,
        dispatch_failed: false,
        all_reports_handled: false,
        execution_context: None,
    }
}

impl LocalMppCoordinator {
    /// 将 Fragment 编码为派发请求并登记 report 槽位；Execute 后不可再追加。
    pub(crate) fn appendMPPDispatchReq(
        &mut self,
        fragment: &Fragment,
        build_context: &mut astersql_planner_core_base::BuildPBContext,
        stores: &HashMap<String, TiFlashStoreInfo>,
        session: &DispatchSessionInfo,
    ) -> Result<(), errors::SharedError> {
        if self.executed {
            return Err(errors::New(
                "cannot append MPP dispatch requests after execution starts",
            ));
        }
        let prepared = append_mpp_dispatch_requests(
            fragment,
            build_context,
            stores,
            session,
            &self.coordinator_address,
            self.report_execution_info,
        )?;
        self.dispatch_task_ids.extend(prepared.task_ids);
        self.dispatch_store_ids.extend(prepared.store_ids);
        for request in prepared.requests {
            let request_index = self.requests.len();
            self.report_handle
                .install_request(request.ID, request_index);
            self.requests.push(request);
        }
        Ok(())
    }

    /// 为每条请求启动工作线程：Dispatch、拉流并经 channel 回传事件。
    fn dispatchAll(&mut self, context: &kv::Context) -> Result<(), errors::SharedError> {
        if self.executed {
            return Err(errors::New("MPP coordinator has already executed"));
        }
        if self.closed {
            return Err(errors::New("MPP coordinator is closed"));
        }
        self.executed = true;
        self.stop_requested.store(false, Ordering::Release);
        let (sender, receiver) = mpsc::channel();
        self.response_receiver = Some(receiver);
        self.active_workers = 0;

        for (request_index, request) in self.requests.iter_mut().enumerate() {
            // Execute and cancellation both require exclusive coordinator access.
            // A task already cancelled must not be revived or dispatched.
            if request.State != kv::MppTaskStates::MppTaskReady {
                continue;
            }
            request.State = kv::MppTaskStates::MppTaskRunning;
            self.active_workers += 1;
            let request = request.clone();
            let context = context.clone();
            let transport = self.transport.clone();
            let sender = sender.clone();
            let stop_requested = self.stop_requested.clone();
            let report_handle = self.report_handle.clone();
            self.workers.push(thread::spawn(move || {
                match transport.Dispatch(&context, &request) {
                    Ok(Some(mut stream)) => {
                        while !stop_requested.load(Ordering::Acquire) {
                            match stream.Next() {
                                Ok(Some(response)) => {
                                    if response.packet.has_error() {
                                        let message =
                                            report_handle.first_error_message().unwrap_or_else(
                                                || response.packet.get_error().get_msg().to_owned(),
                                            );
                                        let _ = sender.send(WorkerEvent::Error {
                                            request_index,
                                            error: errors::New(format!(
                                                "other error for mpp stream: {message}"
                                            )),
                                        });
                                        break;
                                    }
                                    if sender
                                        .send(WorkerEvent::Response {
                                            request_index,
                                            response,
                                        })
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                                Ok(None) => break,
                                Err(error) => {
                                    let _ = sender.send(WorkerEvent::Error {
                                        request_index,
                                        error,
                                    });
                                    break;
                                }
                            }
                        }
                        if let Err(error) = stream.Close()
                            && !stop_requested.load(Ordering::Acquire)
                        {
                            let _ = sender.send(WorkerEvent::Error {
                                request_index,
                                error,
                            });
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = sender.send(WorkerEvent::Error {
                            request_index,
                            error,
                        });
                    }
                }
                let _ = sender.send(WorkerEvent::Finished { request_index });
            }));
        }
        drop(sender);
        Ok(())
    }

    /// 等待全部派发线程结束；任一 panic 则报错。
    fn joinWorkers(&mut self) -> Result<(), errors::SharedError> {
        let mut worker_panicked = false;
        for worker in self.workers.drain(..) {
            if worker.join().is_err() {
                worker_panicked = true;
            }
        }
        if worker_panicked {
            Err(errors::New("MPP dispatch worker panicked"))
        } else {
            Ok(())
        }
    }

    /// 停止拉流并对仍 Running 的任务发起 Cancel。
    fn cancelMppTasks(&mut self, context: &kv::Context) -> Result<(), errors::SharedError> {
        self.stop_requested.store(true, Ordering::Release);
        if self
            .requests
            .first()
            .is_some_and(|request| request.State == kv::MppTaskStates::MppTaskCancelled)
        {
            return Ok(());
        }
        let mut addresses = HashMap::new();
        for request in &mut self.requests {
            if request.State == kv::MppTaskStates::MppTaskRunning {
                if let Some(meta) = request.Meta.as_ref() {
                    addresses.insert(meta.GetAddress(), true);
                }
            }
            request.State = kv::MppTaskStates::MppTaskCancelled;
        }
        if !self.requests.is_empty() {
            self.transport.Cancel(context, addresses, &self.requests)?;
        }
        Ok(())
    }

    /// Close 时等待并合并全部 execution summary；超时只记指标。
    fn handleAllReports(&mut self) -> Result<(), errors::SharedError> {
        if self.all_reports_handled || !self.report_execution_info || self.dispatch_failed {
            return Ok(());
        }
        self.all_reports_handled = true;
        let collection = self.report_handle.wait_and_collect(self.report_timeout)?;
        if collection.timed_out {
            self.report_sink.ReportTimeout(
                collection.expected,
                collection.received,
                self.start_ts,
                self.gather_id,
            );
            return Ok(());
        }
        let mut recorded_plan_ids = HashSet::new();
        for report in &collection.reports {
            for summary in &report.execution_summaries {
                // Match Go's RuntimeStatsColl contract: incomplete summaries
                // are still eligible for RU merging, but cannot be recorded as
                // a cop task without the three required counters.
                if summary.has_time_processed_ns()
                    && summary.has_num_produced_rows()
                    && summary.has_num_iterations()
                {
                    recorded_plan_ids.insert(self.report_sink.RecordOneCopTask(summary)?);
                }
            }
            self.report_sink
                .MergeTiFlashRUConsumption(&report.execution_summaries)?;
        }
        self.report_sink
            .FillDummySummaries(&self.plan_ids, &recorded_plan_ids)?;
        self.completed_execution_summaries = collection
            .reports
            .into_iter()
            .flat_map(|report| report.execution_summaries)
            .collect();
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn coordinator_address(&self) -> &str {
        &self.coordinator_address
    }

    #[cfg(test)]
    pub(crate) fn report_execution_info(&self) -> bool {
        self.report_execution_info
    }

    #[cfg(test)]
    pub(crate) fn install_request(&mut self, request: kv::MPPDispatchRequest) {
        let request_index = self.requests.len();
        self.report_handle
            .install_request(request.ID, request_index);
        self.requests.push(request);
    }

    #[cfg(test)]
    pub(crate) fn reported_request_count(&self) -> usize {
        self.report_handle.reported_request_count()
    }

    #[cfg(test)]
    pub(crate) fn execution_summary_count(&self, task_id: i64) -> Option<usize> {
        self.report_handle.execution_summary_count(task_id)
    }

    #[cfg(test)]
    pub(crate) fn report_handle(&self) -> Arc<MppReportHandle> {
        self.report_handle.clone()
    }

    #[cfg(test)]
    pub(crate) fn completed_execution_summary_count(&self) -> usize {
        self.completed_execution_summaries.len()
    }
}

impl kv::Response for LocalMppCoordinator {
    /// 消费工作线程事件：成功包、错误（取消全部）或 Finished 减计数。
    fn Next(
        &mut self,
        context: &kv::Context,
    ) -> Result<Option<Box<dyn kv::ResultSubset>>, errors::SharedError> {
        loop {
            if self.active_workers == 0 {
                self.joinWorkers()?;
                return Ok(None);
            }
            let event = match self.response_receiver.as_ref() {
                Some(receiver) => receiver.recv(),
                None => return Ok(None),
            };
            match event {
                Ok(WorkerEvent::Response {
                    request_index,
                    response,
                }) => {
                    debug_assert!(request_index < self.requests.len());
                    self.transport.CheckVisibility(self.start_ts)?;
                    return Ok(Some(Box::new(mppResponse::new(
                        response.packet,
                        response.runtime_stats,
                        response.elapsed,
                    ))));
                }
                Ok(WorkerEvent::Error {
                    request_index,
                    error,
                }) => {
                    debug_assert!(request_index < self.requests.len());
                    if self.first_error_message.is_empty() {
                        self.first_error_message = error.to_string();
                    }
                    self.dispatch_failed = true;
                    self.cancelMppTasks(context)?;
                    return Err(error);
                }
                Ok(WorkerEvent::Finished { request_index }) => {
                    if self.requests[request_index].State == kv::MppTaskStates::MppTaskRunning {
                        self.requests[request_index].State = kv::MppTaskStates::MppTaskDone;
                    }
                    self.active_workers = self.active_workers.saturating_sub(1);
                }
                Err(_) => {
                    self.active_workers = 0;
                    self.joinWorkers()?;
                    return Ok(None);
                }
            }
        }
    }

    /// 幂等关闭：取消任务、join workers，再 handleAllReports。
    fn Close(&mut self) -> Result<(), errors::SharedError> {
        let mut close_result = Ok(());
        if !self.closed {
            self.closed = true;
            let context = self
                .execution_context
                .clone()
                .unwrap_or_else(kv::Context::todo);
            if let Err(error) = self.cancelMppTasks(&context) {
                close_result = Err(error);
            }
            if let Err(error) = self.joinWorkers()
                && close_result.is_ok()
            {
                close_result = Err(error);
            }
            self.active_workers = 0;
        }
        let report_result = self.handleAllReports();
        close_result.and(report_result)
    }
}

impl kv::MppCoordinator for LocalMppCoordinator {
    fn Execute(&mut self, context: &kv::Context) -> Result<Vec<kv::KeyRange>, errors::SharedError> {
        self.execution_context = Some(context.clone());
        self.dispatchAll(context)?;
        Ok(self.kv_ranges.clone())
    }

    fn ReportStatus(&mut self, info: kv::ReportStatusRequest) -> Result<(), errors::SharedError> {
        kv::MppStatusReporter::ReportStatus(self.report_handle.as_ref(), info)
    }

    fn StatusReporter(&self) -> Arc<dyn kv::MppStatusReporter> {
        self.report_handle.clone()
    }

    fn ReportsExecutionSummariesDirectly(&self) -> bool {
        self.report_execution_info
    }

    fn IsClosed(&self) -> bool {
        self.closed
    }

    fn GetNodeCnt(&self) -> i32 {
        self.node_count
    }
}

/// 写入 DAG/派发请求的会话与连接元数据。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DispatchSessionInfo {
    pub(crate) time_zone_name: String,
    pub(crate) time_zone_offset: i64,
    pub(crate) flags: u64,
    pub(crate) collect_execution_summaries: bool,
    pub(crate) div_precision_increment: Option<u32>,
    pub(crate) schema_version: i64,
    pub(crate) resource_group_name: String,
    pub(crate) connection_id: u64,
    pub(crate) connection_alias: String,
    pub(crate) sql_digest: String,
    pub(crate) plan_digest: String,
    pub(crate) tidb_zone: String,
}

/// 一批已编码的派发请求及其 task/store id。
#[derive(Default)]
pub(crate) struct PreparedDispatchRequests {
    pub(crate) requests: Vec<kv::MPPDispatchRequest>,
    pub(crate) task_ids: Vec<i64>,
    pub(crate) store_ids: Vec<u64>,
}

/// 准备派发请求时只读访问的 TiFlash store 信息。
///
/// Read-only TiFlash store data consumed while preparing dispatch requests.
pub(crate) trait TiFlashStore: Send + Sync {
    fn StoreID(&self) -> u64;
    fn Address(&self) -> &str;
    fn LabelValue(&self, key: &str) -> Option<&str>;
}

/// 按地址缓存的 zone 与 store_id。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TiFlashStoreInfo {
    pub(crate) zone: String,
    pub(crate) store_id: u64,
}

/// 从 TiFlashStore 抽取地址、zone（DC 标签）、store_id 写入 map。
pub(crate) fn add_tiflash_store_info(
    stores: &mut HashMap<String, TiFlashStoreInfo>,
    store: &dyn TiFlashStore,
) {
    stores.insert(
        store.Address().to_owned(),
        TiFlashStoreInfo {
            zone: store
                .LabelValue(astersql_ddl_placement::DCLabelKey)
                .unwrap_or_default()
                .to_owned(),
            store_id: store.StoreID(),
        },
    );
}

/// 按分区/表 ID 改写 DAG 执行器树中的 table_id（可递归子节点）。
fn update_executor_table_id(
    executor: &mut tipb::Executor,
    recursive: bool,
    partition_ids: &[i64],
) -> Result<(), errors::SharedError> {
    use tipb::ExecType;

    let next = match executor.get_tp() {
        ExecType::TypeTableScan => {
            let table_id = partition_ids
                .first()
                .copied()
                .ok_or_else(|| errors::New("table scan requires one partition ID"))?;
            executor.mut_tbl_scan().set_table_id(table_id);
            None
        }
        ExecType::TypePartitionTableScan => {
            executor
                .mut_partition_table_scan()
                .set_partition_ids(partition_ids.to_vec());
            None
        }
        ExecType::TypeIndexScan => {
            let table_id = partition_ids
                .first()
                .copied()
                .ok_or_else(|| errors::New("index scan requires one partition ID"))?;
            executor.mut_idx_scan().set_table_id(table_id);
            None
        }
        ExecType::TypeSelection => Some(executor.mut_selection().mut_child()),
        ExecType::TypeAggregation | ExecType::TypeStreamAgg => {
            Some(executor.mut_aggregation().mut_child())
        }
        ExecType::TypeTopN => Some(executor.mut_top_n().mut_child()),
        ExecType::TypeLimit => Some(executor.mut_limit().mut_child()),
        ExecType::TypeExchangeSender => Some(executor.mut_exchange_sender().mut_child()),
        ExecType::TypeExchangeReceiver | ExecType::TypeCteSource => None,
        ExecType::TypeCteSink => Some(executor.mut_cte_sink().mut_child()),
        ExecType::TypeJoin => {
            let outer_index = 1usize.saturating_sub(executor.get_join().get_inner_idx() as usize);
            let children = executor.mut_join().mut_children();
            Some(
                children
                    .get_mut(outer_index)
                    .ok_or_else(|| errors::New("join executor is missing its outer child"))?,
            )
        }
        ExecType::TypeProjection => Some(executor.mut_projection().mut_child()),
        ExecType::TypeWindow => Some(executor.mut_window().mut_child()),
        ExecType::TypeSort => Some(executor.mut_sort().mut_child()),
        ExecType::TypeExpand => Some(executor.mut_expand().mut_child()),
        ExecType::TypeExpand2 => Some(executor.mut_expand2().mut_child()),
        unknown => {
            return Err(errors::New(format!(
                "unknown new tipb protocol {}",
                unknown.value()
            )));
        }
    };

    if recursive && let Some(child) = next {
        update_executor_table_id(child, true, partition_ids)?;
    }
    Ok(())
}

/// 为每个 MPPTask 克隆 root executor、改写分区、填 same-zone，并序列化 DAG。
pub(crate) fn prepare_dispatch_requests_from_root(
    root_executor: &tipb::Executor,
    output_column_count: usize,
    is_root: bool,
    tasks: &[kv::MPPTask],
    stores: &HashMap<String, TiFlashStoreInfo>,
    session: &DispatchSessionInfo,
    coordinator_address: &str,
    report_execution_summary: bool,
) -> Result<PreparedDispatchRequests, errors::SharedError> {
    let mut prepared = PreparedDispatchRequests {
        requests: Vec::with_capacity(tasks.len()),
        task_ids: Vec::with_capacity(tasks.len()),
        store_ids: Vec::with_capacity(tasks.len()),
    };
    let mut zone_helper = TaskZoneInfoHelper::new(stores.clone(), session.tidb_zone.clone());

    for task in tasks {
        let task_meta = task
            .Meta
            .as_ref()
            .ok_or_else(|| errors::New(format!("MPP task {} has no store metadata", task.ID)))?;
        let address = task_meta.GetAddress();
        let current_zone = stores
            .get(&address)
            .map(|store| store.zone.clone())
            .unwrap_or_default();
        zone_helper.set_fragment(is_root, current_zone);

        let mut executor = root_executor.clone();
        if !task.PartitionTableIDs.is_empty() {
            update_executor_table_id(&mut executor, true, &task.PartitionTableIDs)?;
        } else if !task.TiFlashStaticPrune {
            update_executor_table_id(&mut executor, true, &[task.TableID])?;
        }
        zone_helper.fill_same_zone_flag_for_exchange(&mut executor)?;

        let mut dag_request = tipb::DagRequest::new();
        dag_request.set_time_zone_name(session.time_zone_name.clone());
        dag_request.set_time_zone_offset(session.time_zone_offset);
        dag_request.set_flags(session.flags);
        if session.collect_execution_summaries {
            dag_request.set_collect_execution_summaries(true);
        }
        if let Some(increment) = session.div_precision_increment {
            dag_request.set_div_precision_increment(increment);
        }
        dag_request.set_output_offsets((0..output_column_count as u32).collect());
        dag_request.set_encode_type(if is_root {
            tipb::EncodeType::TypeChunk
        } else {
            tipb::EncodeType::TypeChBlock
        });
        dag_request.set_root_executor(executor);
        let data = dag_request
            .write_to_bytes()
            .map_err(|error| errors::New(error.to_string()))?;

        prepared.task_ids.push(task.ID);
        prepared
            .store_ids
            .push(stores.get(&address).map_or(0, |store| store.store_id));
        prepared.requests.push(kv::MPPDispatchRequest {
            Data: data,
            Meta: task.Meta.clone(),
            IsRoot: is_root,
            Timeout: 10,
            SchemaVar: session.schema_version,
            StartTs: task.StartTs,
            MppQueryID: task.MppQueryID,
            GatherID: task.GatherID,
            ID: task.ID,
            MppVersion: task.MppVersion,
            CoordinatorAddress: coordinator_address.to_owned(),
            ReportExecutionSummary: report_execution_summary,
            State: kv::MppTaskStates::MppTaskReady,
            ResourceGroupName: session.resource_group_name.clone(),
            ConnectionID: session.connection_id,
            ConnectionAlias: session.connection_alias.clone(),
            SQLDigest: session.sql_digest.clone(),
            PlanDigest: session.plan_digest.clone(),
        });
    }
    Ok(prepared)
}

/// 将 Fragment Sink 转为 protobuf root，再调用 prepare_dispatch_requests_from_root。
pub(crate) fn append_mpp_dispatch_requests(
    fragment: &Fragment,
    build_context: &mut astersql_planner_core_base::BuildPBContext,
    stores: &HashMap<String, TiFlashStoreInfo>,
    session: &DispatchSessionInfo,
    coordinator_address: &str,
    report_execution_summary: bool,
) -> Result<PreparedDispatchRequests, errors::SharedError> {
    let sink = fragment
        .Sink
        .lock()
        .map_err(|_| errors::New("MPP fragment sink lock is poisoned"))?;
    let root = sink
        .to_pb(build_context, kv::StoreType::TiFlash)
        .map_err(|error| errors::New(error.to_string()))?;
    prepare_dispatch_requests_from_root(
        root.as_ref(),
        sink.schema().Columns.len(),
        fragment.IsRoot,
        sink.get_self_tasks(),
        stores,
        session,
        coordinator_address,
        report_execution_summary,
    )
}

/// 计算 Exchange 目标是否与当前 task 同 zone，并按 executor 缓存解码结果。
///
/// Computes TiFlash same-zone flags and caches decoded task zones per executor.
pub(crate) struct TaskZoneInfoHelper {
    all_tiflash_store_info: HashMap<String, TiFlashStoreInfo>,
    exchange_zone_info: HashMap<String, Vec<String>>,
    tidb_zone: String,
    current_task_zone: String,
    is_root: bool,
}

impl TaskZoneInfoHelper {
    /// 用全量 store 信息与 TiDB zone 构造 helper。
    pub(crate) fn new(
        all_tiflash_store_info: HashMap<String, TiFlashStoreInfo>,
        tidb_zone: String,
    ) -> Self {
        Self {
            all_tiflash_store_info,
            exchange_zone_info: HashMap::with_capacity(2),
            tidb_zone,
            current_task_zone: String::new(),
            is_root: false,
        }
    }

    /// 切换到当前 Fragment：是否 root、当前 task 所在 zone。
    pub(crate) fn set_fragment(&mut self, is_root: bool, current_task_zone: String) {
        self.is_root = is_root;
        self.current_task_zone = current_task_zone;
    }

    /// 快速路径：zone 空则全 true；root sender 则与 tidb_zone 比较。
    pub(crate) fn try_quick_fill_with_uncertain_zones(
        &self,
        executor_id: &str,
        is_exchange_sender: bool,
        slots: usize,
    ) -> Option<Vec<bool>> {
        if executor_id.is_empty() || self.current_task_zone.is_empty() {
            return Some(vec![true; slots]);
        }
        if self.is_root && is_exchange_sender {
            return Some(vec![
                self.tidb_zone.is_empty() || self.current_task_zone == self.tidb_zone,
            ]);
        }
        None
    }

    /// 解码 encoded_task_meta，查表得到各目标 zone（未知为空串）。
    fn collect_exchange_zone_infos(&self, encoded_task_meta: &[Vec<u8>]) -> Vec<String> {
        encoded_task_meta
            .iter()
            .map(|encoded| {
                let mut meta = TaskMeta::new();
                meta.merge_from_bytes(encoded)
                    .ok()
                    .map(|_| meta)
                    .and_then(|meta| {
                        self.all_tiflash_store_info
                            .get(meta.get_address())
                            .map(|store| store.zone.clone())
                    })
                    .unwrap_or_default()
            })
            .collect()
    }

    /// 推断 same_zone_flag：先 quick fill，再按缓存 zone 与 current_task_zone 比较。
    fn infer_same_zone_flag(
        &mut self,
        executor_id: &str,
        is_exchange_sender: bool,
        encoded_task_meta: &[Vec<u8>],
    ) -> Vec<bool> {
        let slots = encoded_task_meta.len();
        if let Some(flags) =
            self.try_quick_fill_with_uncertain_zones(executor_id, is_exchange_sender, slots)
        {
            return flags;
        }

        if !self.exchange_zone_info.contains_key(executor_id) {
            let zones = self.collect_exchange_zone_infos(encoded_task_meta);
            self.exchange_zone_info
                .insert(executor_id.to_owned(), zones);
        }
        let zones = &self.exchange_zone_info[executor_id];
        if zones.len() != slots {
            return vec![true; slots];
        }
        zones
            .iter()
            .map(|zone| zone.is_empty() || *zone == self.current_task_zone)
            .collect()
    }

    /// 递归遍历 DAG，为 ExchangeSender 写入 same_zone_flag。
    pub(crate) fn fill_same_zone_flag_for_exchange(
        &mut self,
        executor: &mut tipb::Executor,
    ) -> Result<(), errors::SharedError> {
        use tipb::ExecType;

        match executor.get_tp() {
            ExecType::TypeTableScan
            | ExecType::TypePartitionTableScan
            | ExecType::TypeIndexScan
            | ExecType::TypeCteSource => Ok(()),
            ExecType::TypeExchangeReceiver => {
                let executor_id = executor.get_executor_id().to_owned();
                let encoded = executor
                    .get_exchange_receiver()
                    .get_encoded_task_meta()
                    .to_vec();
                let flags = self.infer_same_zone_flag(&executor_id, false, &encoded);
                executor.mut_exchange_receiver().set_same_zone_flag(flags);
                Ok(())
            }
            ExecType::TypeExchangeSender => {
                let executor_id = executor.get_executor_id().to_owned();
                let encoded = executor
                    .get_exchange_sender()
                    .get_encoded_task_meta()
                    .to_vec();
                let flags = self.infer_same_zone_flag(&executor_id, true, &encoded);
                executor.mut_exchange_sender().set_same_zone_flag(flags);
                if executor.get_exchange_sender().has_child() {
                    self.fill_same_zone_flag_for_exchange(
                        executor.mut_exchange_sender().mut_child(),
                    )?;
                }
                Ok(())
            }
            ExecType::TypeSelection => {
                self.fill_same_zone_flag_for_exchange(executor.mut_selection().mut_child())
            }
            ExecType::TypeAggregation | ExecType::TypeStreamAgg => {
                self.fill_same_zone_flag_for_exchange(executor.mut_aggregation().mut_child())
            }
            ExecType::TypeTopN => {
                self.fill_same_zone_flag_for_exchange(executor.mut_top_n().mut_child())
            }
            ExecType::TypeLimit => {
                self.fill_same_zone_flag_for_exchange(executor.mut_limit().mut_child())
            }
            ExecType::TypeCteSink => {
                self.fill_same_zone_flag_for_exchange(executor.mut_cte_sink().mut_child())
            }
            ExecType::TypeProjection => {
                self.fill_same_zone_flag_for_exchange(executor.mut_projection().mut_child())
            }
            ExecType::TypeWindow => {
                self.fill_same_zone_flag_for_exchange(executor.mut_window().mut_child())
            }
            ExecType::TypeSort => {
                self.fill_same_zone_flag_for_exchange(executor.mut_sort().mut_child())
            }
            ExecType::TypeExpand => {
                self.fill_same_zone_flag_for_exchange(executor.mut_expand().mut_child())
            }
            ExecType::TypeExpand2 => {
                self.fill_same_zone_flag_for_exchange(executor.mut_expand2().mut_child())
            }
            ExecType::TypeJoin => {
                for child in executor.mut_join().mut_children().iter_mut() {
                    self.fill_same_zone_flag_for_exchange(child)?;
                }
                Ok(())
            }
            // Go logs unknown executor kinds and continues; preserve that
            // forward-compatible behavior while leaving the subtree untouched.
            _unknown => Ok(()),
        }
    }
}

/// 仅当在 Limit 之下遇到 TableReader，且其下推 table plan id 为目标时才上报。
///
/// Reports through the coordinator only when a table reader is reached below
/// a Limit and its pushed-down table plan is the requested destination.
pub(crate) fn need_report_execution_summary(
    plan: &dyn PhysicalPlan,
    destination_table_plan_id: i32,
    found_limit: bool,
) -> bool {
    if plan.as_any().is::<PhysicalLimit>() {
        return plan.children().first().is_some_and(|child| {
            need_report_execution_summary(*child, destination_table_plan_id, true)
        });
    }
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalTableReader>() {
        return found_limit
            && reader
                .GetTablePlan()
                .is_some_and(|table_plan| table_plan.id() == destination_table_plan_id);
    }
    plan.children()
        .into_iter()
        .any(|child| need_report_execution_summary(child, destination_table_plan_id, found_limit))
}

/// 本地协调器返回的单包响应，实现 `kv::ResultSubset`。
///
/// 拥有 packet 与 runtime stats，便于 recovery handler 安全缓冲。
///
/// A single data packet returned by a local MPP coordinator.
///
/// The packet and runtime statistics are owned so the response can safely be
/// buffered by the recovery handler without the raw-pointer lifetime hazards
/// in the ported code.
pub(crate) struct mppResponse {
    error: Option<errors::SharedError>,
    packet: Option<MppDataPacket>,
    runtime_stats: Option<Box<CopRuntimeStats>>,
    response_time: Duration,
    response_size: Cell<i64>,
}

impl mppResponse {
    /// 由成功数据包构造响应。
    pub(crate) fn new(
        packet: MppDataPacket,
        runtime_stats: Option<CopRuntimeStats>,
        response_time: Duration,
    ) -> Self {
        Self {
            error: None,
            packet: Some(packet),
            runtime_stats: runtime_stats.map(Box::new),
            response_time,
            response_size: Cell::new(0),
        }
    }

    /// 由错误构造空数据响应。
    pub(crate) fn from_error(error: errors::SharedError, response_time: Duration) -> Self {
        Self {
            error: Some(error),
            packet: None,
            runtime_stats: None,
            response_time,
            response_size: Cell::new(0),
        }
    }

    /// 若为错误响应则返回原因。
    pub(crate) fn Error(&self) -> Option<&errors::SharedError> {
        self.error.as_ref()
    }

    /// 可选的 Cop 运行时统计。
    pub(crate) fn GetCopRuntimeStats(&self) -> Option<&CopRuntimeStats> {
        self.runtime_stats.as_deref()
    }

    /// 包内业务数据；错误响应为空切片。
    fn data(&self) -> &[u8] {
        self.packet
            .as_ref()
            .map(MppDataPacket::get_data)
            .unwrap_or_default()
    }

    /// 估算内存：可选 ExecDetails + protobuf 包大小，结果缓存于 Cell。
    fn memory_size(&self) -> i64 {
        let cached = self.response_size.get();
        if cached != 0 {
            return cached;
        }

        let mut size = 0;
        if self.runtime_stats.is_some() {
            size += mem::size_of::<ExecDetails>() as i64;
        }
        if let Some(packet) = self.packet.as_ref() {
            size += packet.compute_size() as i64;
        }
        self.response_size.set(size);
        size
    }
}

impl kv::ResultSubset for mppResponse {
    fn GetData(&self) -> &[u8] {
        self.data()
    }

    fn GetStartKey(&self) -> kv::Key {
        kv::Key::default()
    }

    fn MemSize(&self) -> i64 {
        self.memory_size()
    }

    fn RespTime(&self) -> Duration {
        self.response_time
    }
}

/// 编码任务放置与 same-zone 标志所需的 store 拓扑。
///
/// Store topology required to encode task placement and same-zone flags.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MppStoreInfo {
    pub address: String,
    pub store_id: u64,
    pub zone: String,
}

/// 一次为全部派发请求捕获的语句/会话字段。
///
/// Statement/session values captured once for all dispatch requests.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MppDispatchSession {
    pub coordinator_address: String,
    pub chosen_mpp_version: kv::MppVersion,
    pub time_zone_name: String,
    pub time_zone_offset: i64,
    pub flags: u64,
    pub collect_execution_summaries: bool,
    pub div_precision_increment: Option<u32>,
    pub schema_version: i64,
    pub resource_group_name: String,
    pub connection_id: u64,
    pub connection_alias: String,
    pub sql_digest: String,
    pub plan_digest: String,
    pub tidb_zone: String,
}

/// 根任务生成器消费的物理语句身份（计划、时间戳、gather）。
///
/// Physical statement identity consumed by the root task generator.
pub struct MppCoordinatorPlan {
    pub original_plan: Box<dyn PhysicalPlan>,
    pub statement_plan: Option<Box<dyn PhysicalPlan>>,
    pub plan_ids: Vec<i32>,
    pub start_ts: u64,
    pub query_id: kv::MPPQueryID,
    pub gather_id: u64,
}

/// 公开入口：生成根 MPP 任务、安装传输层、构造 DAG 请求并返回可执行协调器。
///
/// Public coordinator entry point. The session generator owns the repository
/// MPP client and InfoSchema/range encoder; the facade invokes
/// `GenerateRootMPPTasks` for the physical plan and owns
/// transport installation, store lookup, DAG/request construction, node
/// counting, and the executable state machine.
pub fn NewLocalMppCoordinator(
    task_generator: &SessionRootMppTaskGenerator,
    build_context: &mut BuildPBContext,
    plan: MppCoordinatorPlan,
    session: MppDispatchSession,
    stores: Vec<MppStoreInfo>,
    report_sink: Arc<dyn MppReportSink>,
    report_timeout: Duration,
) -> Result<Box<dyn kv::MppCoordinator>, errors::SharedError> {
    let MppCoordinatorPlan {
        original_plan,
        statement_plan,
        plan_ids,
        start_ts,
        query_id,
        gather_id,
    } = plan;

    // 由会话侧生成器切分 Fragment 与 KV 范围。
    let generated = task_generator
        .GenerateRootMPPTasks(original_plan.as_ref(), start_ts, gather_id, query_id)
        .map_err(|error| errors::New(error.to_string()))?;
    let fragments = generated.fragments;
    let kv_ranges = generated.kv_ranges;
    if kv_ranges.is_empty() {
        return Err(errors::New("kvRanges for MPPTask should not be empty"));
    }

    let mut node_addresses = generated.node_addresses;
    for fragment in &fragments {
        let sink = fragment
            .Sink
            .lock()
            .map_err(|_| errors::New("MPP fragment sink lock is poisoned"))?;
        for task in sink.get_self_tasks() {
            let meta = task.Meta.as_ref().ok_or_else(|| {
                errors::New(format!(
                    "scheduled MPP task {} has no store metadata",
                    task.ID
                ))
            })?;
            node_addresses.insert(meta.GetAddress());
        }
    }
    if node_addresses.is_empty() {
        return Err(errors::New("scheduled MPP plan contains no TiFlash tasks"));
    }

    let store_map: HashMap<_, _> = stores
        .into_iter()
        .map(|store| {
            (
                store.address,
                TiFlashStoreInfo {
                    zone: store.zone,
                    store_id: store.store_id,
                },
            )
        })
        .collect();
    let dispatch_session = DispatchSessionInfo {
        time_zone_name: session.time_zone_name,
        time_zone_offset: session.time_zone_offset,
        flags: session.flags,
        collect_execution_summaries: session.collect_execution_summaries,
        div_precision_increment: session.div_precision_increment,
        schema_version: session.schema_version,
        resource_group_name: session.resource_group_name,
        connection_id: session.connection_id,
        connection_alias: session.connection_alias,
        sql_digest: session.sql_digest,
        plan_digest: session.plan_digest,
        tidb_zone: session.tidb_zone,
    };
    let transport = std::sync::Arc::new(MppClientCoordinatorTransport::New(
        task_generator.client.clone(),
    ));
    let mut coordinator = new_local_mpp_coordinator(
        transport,
        original_plan,
        statement_plan.as_deref(),
        plan_ids,
        start_ts,
        query_id,
        gather_id,
        session.coordinator_address,
        session.chosen_mpp_version,
        kv_ranges,
        node_addresses.len() as i32,
        report_sink,
        report_timeout,
    );
    // 为每个 Fragment 追加派发请求后返回装箱协调器。
    for fragment in &fragments {
        coordinator.appendMPPDispatchReq(fragment, build_context, &store_map, &dispatch_session)?;
    }
    Ok(Box::new(coordinator))
}
