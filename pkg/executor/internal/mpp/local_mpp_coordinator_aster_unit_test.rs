// Copyright 2026 AsterSQL.

// `LocalMppCoordinator` 核心状态机的单元测试。
//
// 覆盖：V2 才启用 report、MPPClient 传输重试与可见性检查、
// 并行派发交错拉流、派发失败取消、ReportStatus 解码与去重、
// Close 超时指标，以及并发 Report 唤醒 Close 等待。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use astersql_errors as errors;
use astersql_kv as kv;
use astersql_planner_core_base::{PhysicalPlan, Plan};
use astersql_planner_core_operator_physicalop::{
    PhysicalExchangeSender, PhysicalLimit, PhysicalTableReader,
};
use kvproto::mpp::{
    DispatchTaskResponse, Error as MppError, MppDataPacket, ReportTaskStatusRequest, TaskMeta,
};
use protobuf::Message;

use crate::local_mpp_coordinator::{
    CoordinatorResponseStream, CoordinatorTransport, DispatchResponse, LocalMppCoordinator,
    MppClientCoordinatorTransport, MppReportSink, NoopMppReportSink, new_local_mpp_coordinator,
};
use crate::{
    CoordinatorRegistry, CoordinatorUniqueId, MppCoordinatorManager, SharedMppCoordinator,
};

/// 最小 PlanContext：分配 plan id，其余接口 panic。
struct TestPlanContext(
    AtomicI32,
    astersql_planner_core_base::BuiltinFunctionUsageCounter,
);

impl astersql_planner_core_base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &astersql_planner_planctx::variable::SessionVars {
        panic!("coordinator test does not read session variables")
    }

    fn GetExprCtx(&self) -> &dyn astersql_planner_planctx::exprctx::ExprContext {
        panic!("coordinator test does not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &astersql_planner_planctx::rangerctx::RangerContext<'_> {
        panic!("coordinator test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn astersql_planner_planctx::exprctx::ExprContext {
        panic!("coordinator test does not evaluate null rejection")
    }

    fn GetBuildPBCtx(&self) -> &astersql_planner_core_base::BuildPBContext {
        panic!("coordinator test does not build protobuf")
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.1.Inc(name)
    }
}

/// 构造测试用 ContextRef。
fn plan_context() -> astersql_planner_core_base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        astersql_planner_core_base::BuiltinFunctionUsageCounter::default(),
    ))
}

/// Limit -> TableReader -> ExchangeSender(id=10)，用于开启 report 路径。
fn statement_plan(context: astersql_planner_core_base::ContextRef) -> PhysicalLimit {
    let mut table_plan = PhysicalExchangeSender::New(context.clone());
    table_plan.set_id(10);
    let mut reader = PhysicalTableReader::New(context.clone());
    reader.SetTablePlanForTest(Box::new(table_plan));
    let mut limit = PhysicalLimit::New(context, 0, 1);
    PhysicalPlan::set_children(&mut limit, vec![Box::new(reader)]);
    limit
}

/// 固定地址的 TaskMeta。
#[derive(Clone)]
struct TestTaskMeta(&'static str);

impl kv::MPPTaskMeta for TestTaskMeta {
    fn GetAddress(&self) -> String {
        self.0.to_owned()
    }

    fn CloneBox(&self) -> Box<dyn kv::MPPTaskMeta> {
        Box::new(self.clone())
    }
}

/// 构造一条 root 派发请求。
fn request(id: i64) -> kv::MPPDispatchRequest {
    kv::MPPDispatchRequest {
        Meta: Some(Box::new(TestTaskMeta("tiflash-1:3930"))),
        IsRoot: true,
        StartTs: 11,
        GatherID: 12,
        ID: id,
        MppVersion: kv::MppVersionV2,
        ..Default::default()
    }
}

/// 构造带数据的 MppDataPacket。
fn packet(data: &[u8]) -> MppDataPacket {
    let mut packet = MppDataPacket::new();
    packet.set_data(data.to_vec());
    packet
}

/// 构造带 MPP stream 错误的响应包。
fn error_packet(message: &str) -> MppDataPacket {
    let mut packet = MppDataPacket::new();
    let mut error = MppError::new();
    error.set_msg(message.to_owned());
    packet.set_error(error);
    packet
}

/// 使用 Noop sink 构造协调器；`report_plan` 控制是否挂 statement plan。
fn coordinator(transport: Arc<dyn CoordinatorTransport>, report_plan: bool) -> LocalMppCoordinator {
    coordinator_with_sink(
        transport,
        report_plan,
        Arc::new(NoopMppReportSink),
        Duration::from_millis(1),
    )
}

/// 可注入 ReportSink 与超时的协调器构造。
fn coordinator_with_sink(
    transport: Arc<dyn CoordinatorTransport>,
    report_plan: bool,
    report_sink: Arc<dyn MppReportSink>,
    report_timeout: Duration,
) -> LocalMppCoordinator {
    let context = plan_context();
    let plan = statement_plan(context.clone());
    let mut original = PhysicalExchangeSender::New(context);
    original.set_id(10);
    new_local_mpp_coordinator(
        transport,
        Box::new(original),
        report_plan.then_some(&plan as &dyn PhysicalPlan),
        vec![10],
        11,
        kv::MPPQueryID::default(),
        12,
        "tidb:4000".to_owned(),
        kv::MppVersionV2,
        vec![kv::KeyRange::default()],
        3,
        report_sink,
        report_timeout,
    )
}

/// 记录 Record/Merge/Fill/Timeout 调用次数的 sink。
#[derive(Default)]
struct RecordingReportSink {
    records: AtomicUsize,
    merges: AtomicUsize,
    fills: AtomicUsize,
    timeouts: AtomicUsize,
}

impl MppReportSink for RecordingReportSink {
    fn RecordOneCopTask(
        &self,
        _: &tipb::ExecutorExecutionSummary,
    ) -> Result<i32, errors::SharedError> {
        self.records.fetch_add(1, Ordering::SeqCst);
        Ok(10)
    }
    fn MergeTiFlashRUConsumption(
        &self,
        _: &[tipb::ExecutorExecutionSummary],
    ) -> Result<(), errors::SharedError> {
        self.merges.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn FillDummySummaries(
        &self,
        _: &[i32],
        _: &std::collections::HashSet<i32>,
    ) -> Result<(), errors::SharedError> {
        self.fills.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn ReportTimeout(&self, _: usize, _: usize, _: u64, _: u64) {
        self.timeouts.fetch_add(1, Ordering::SeqCst);
    }
}

/// 队列化数据包流。
struct PacketStream {
    packets: VecDeque<MppDataPacket>,
    closes: Arc<AtomicUsize>,
}

impl kv::MPPDataPacketStream for PacketStream {
    fn Recv(&mut self) -> Result<Option<MppDataPacket>, kv::Error> {
        Ok(self.packets.pop_front())
    }

    fn Close(&mut self) -> Result<(), kv::Error> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// 首次 Dispatch/Establish 返回 retry，第二次成功的 MPPClient。
struct TestMPPClient {
    packets: Mutex<Option<VecDeque<MppDataPacket>>>,
    dispatches: AtomicUsize,
    establishes: AtomicUsize,
    cancels: AtomicUsize,
    visibility_checks: AtomicUsize,
    stream_closes: Arc<AtomicUsize>,
}

impl TestMPPClient {
    fn new(packets: Vec<MppDataPacket>) -> Self {
        Self {
            packets: Mutex::new(Some(packets.into())),
            dispatches: AtomicUsize::new(0),
            establishes: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
            visibility_checks: AtomicUsize::new(0),
            stream_closes: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl kv::MPPClient for TestMPPClient {
    fn ConstructMPPTasks(
        &self,
        _context: &kv::Context,
        _request: &kv::MPPBuildTasksRequest,
        _timeout: Duration,
        _policy: kv::tiflashcompute::DispatchPolicy,
        _replica_read: kv::tiflash::ReplicaRead,
        _on_error: &mut dyn FnMut(kv::Error),
    ) -> Result<Vec<Box<dyn kv::MPPTaskMeta>>, kv::Error> {
        Ok(Vec::new())
    }

    fn DispatchMPPTask(
        &self,
        _param: kv::DispatchMPPTaskParam<'_>,
    ) -> Result<(DispatchTaskResponse, bool), kv::Error> {
        let attempt = self.dispatches.fetch_add(1, Ordering::SeqCst);
        Ok((DispatchTaskResponse::new(), attempt == 0))
    }

    fn EstablishMPPConns(
        &self,
        _param: kv::EstablishMPPConnsParam<'_>,
    ) -> Result<(kv::MPPStreamResponse, bool), kv::Error> {
        let attempt = self.establishes.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            return Ok((kv::MPPStreamResponse::default(), true));
        }
        let packets = self
            .packets
            .lock()
            .expect("packet queue lock")
            .take()
            .expect("stream is established once");
        Ok((
            kv::MPPStreamResponse::New(
                None,
                Box::new(PacketStream {
                    packets,
                    closes: self.stream_closes.clone(),
                }),
            ),
            false,
        ))
    }

    fn CancelMPPTasks(&self, _param: kv::CancelMPPTasksParam) {
        self.cancels.fetch_add(1, Ordering::SeqCst);
    }

    fn CheckVisibility(&self, start_time: u64) -> Result<(), kv::Error> {
        assert_eq!(start_time, 11);
        self.visibility_checks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn GetMPPStoreCount(&self) -> Result<i32, kv::Error> {
        Ok(3)
    }
}

/// Dispatch 恒失败并记录 Cancel 参数的传输层。
struct FailingTransport {
    dispatches: AtomicUsize,
    cancels: AtomicUsize,
    cancelled_addresses: Mutex<HashMap<String, bool>>,
    cancelled_states: Mutex<Vec<kv::MppTaskStates>>,
}

impl Default for FailingTransport {
    fn default() -> Self {
        Self {
            dispatches: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
            cancelled_addresses: Mutex::new(HashMap::new()),
            cancelled_states: Mutex::new(Vec::new()),
        }
    }
}

impl CoordinatorTransport for FailingTransport {
    fn Dispatch(
        &self,
        _context: &kv::Context,
        _request: &kv::MPPDispatchRequest,
    ) -> Result<Option<Box<dyn CoordinatorResponseStream>>, errors::SharedError> {
        self.dispatches.fetch_add(1, Ordering::SeqCst);
        Err(errors::New("dispatch failed"))
    }

    fn Cancel(
        &self,
        _context: &kv::Context,
        store_addresses: HashMap<String, bool>,
        requests: &[kv::MPPDispatchRequest],
    ) -> Result<(), errors::SharedError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        *self.cancelled_addresses.lock().expect("address lock") = store_addresses;
        *self.cancelled_states.lock().expect("state lock") =
            requests.iter().map(|request| request.State).collect();
        Ok(())
    }

    fn CheckVisibility(&self, _start_ts: u64) -> Result<(), errors::SharedError> {
        panic!("failed responses are rejected before visibility checking")
    }
}

/// 等 gate 信号后才吐出单包的流。
struct GatedStream {
    gate: Option<mpsc::Receiver<()>>,
    packet: Option<MppDataPacket>,
    closes: Arc<AtomicUsize>,
}

impl CoordinatorResponseStream for GatedStream {
    fn Next(&mut self) -> Result<Option<DispatchResponse>, errors::SharedError> {
        let Some(gate) = self.gate.take() else {
            return Ok(None);
        };
        gate.recv()
            .map_err(|_| errors::New("test stream gate disconnected"))?;
        Ok(self.packet.take().map(|packet| DispatchResponse {
            packet,
            runtime_stats: None,
            elapsed: Duration::ZERO,
        }))
    }

    fn Close(&mut self) -> Result<(), errors::SharedError> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// 按请求 ID 挂起流，用于验证并行派发与交错 Next。
struct ParallelTransport {
    gates: Mutex<HashMap<i64, mpsc::Receiver<()>>>,
    dispatched: mpsc::Sender<i64>,
    closes: Arc<AtomicUsize>,
    cancels: AtomicUsize,
}

impl CoordinatorTransport for ParallelTransport {
    fn Dispatch(
        &self,
        _context: &kv::Context,
        request: &kv::MPPDispatchRequest,
    ) -> Result<Option<Box<dyn CoordinatorResponseStream>>, errors::SharedError> {
        let gate = self
            .gates
            .lock()
            .expect("gate map lock")
            .remove(&request.ID)
            .ok_or_else(|| errors::New("missing request gate"))?;
        self.dispatched
            .send(request.ID)
            .map_err(|_| errors::New("dispatch observer disconnected"))?;
        Ok(Some(Box::new(GatedStream {
            gate: Some(gate),
            packet: Some(packet(&[request.ID as u8])),
            closes: self.closes.clone(),
        })))
    }

    fn Cancel(
        &self,
        _context: &kv::Context,
        _store_addresses: HashMap<String, bool>,
        _requests: &[kv::MPPDispatchRequest],
    ) -> Result<(), errors::SharedError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn CheckVisibility(&self, _start_ts: u64) -> Result<(), errors::SharedError> {
        Ok(())
    }
}

/// 仅 MppVersionV2 + Limit 路径时保留 coordinator_address 并开启 report。
#[test]
fn constructor_enables_reports_only_for_v2_coordinator_with_limit_path() {
    let transport = Arc::new(FailingTransport::default());
    let context = plan_context();
    let plan = statement_plan(context.clone());
    let mut original = PhysicalExchangeSender::New(context.clone());
    original.set_id(10);
    let coordinator = new_local_mpp_coordinator(
        transport.clone(),
        Box::new(original),
        Some(&plan),
        vec![10],
        11,
        kv::MPPQueryID::default(),
        12,
        "tidb:4000".to_owned(),
        kv::MppVersionV2,
        Vec::new(),
        3,
        Arc::new(NoopMppReportSink),
        Duration::from_millis(1),
    );
    assert_eq!(coordinator.coordinator_address(), "tidb:4000");
    assert!(coordinator.report_execution_info());

    let mut old_original = PhysicalExchangeSender::New(context);
    old_original.set_id(10);
    let old = new_local_mpp_coordinator(
        transport,
        Box::new(old_original),
        Some(&plan),
        vec![10],
        11,
        kv::MPPQueryID::default(),
        12,
        "tidb:4000".to_owned(),
        kv::MppVersionV1,
        Vec::new(),
        3,
        Arc::new(NoopMppReportSink),
        Duration::from_millis(1),
    );
    assert!(old.coordinator_address().is_empty());
    assert!(!old.report_execution_info());
}

/// MPPClient 传输：重试后建流，Next 校验可见性，Close 幂等只 Cancel 一次。
#[test]
fn mpp_client_transport_retries_and_streams_packets_with_visibility_checks() {
    let client = Arc::new(TestMPPClient::new(vec![packet(&[1]), packet(&[2, 3])]));
    let transport = Arc::new(MppClientCoordinatorTransport::New(client.clone()));
    let mut coordinator = coordinator(transport, false);
    coordinator.install_request(request(101));

    let ranges = kv::MppCoordinator::Execute(&mut coordinator, &kv::Context::todo())
        .expect("dispatch succeeds");
    assert_eq!(ranges.len(), 1);
    let first = kv::Response::Next(&mut coordinator, &kv::Context::todo())
        .expect("first response")
        .expect("first packet");
    let second = kv::Response::Next(&mut coordinator, &kv::Context::todo())
        .expect("second response")
        .expect("second packet");
    assert_eq!(first.GetData(), &[1]);
    assert_eq!(second.GetData(), &[2, 3]);
    assert!(
        kv::Response::Next(&mut coordinator, &kv::Context::todo())
            .expect("stream exhausted")
            .is_none()
    );
    assert_eq!(client.dispatches.load(Ordering::SeqCst), 2);
    assert_eq!(client.establishes.load(Ordering::SeqCst), 2);
    assert_eq!(client.visibility_checks.load(Ordering::SeqCst), 2);
    assert_eq!(client.stream_closes.load(Ordering::SeqCst), 1);
    assert_eq!(kv::MppCoordinator::GetNodeCnt(&coordinator), 3);

    kv::Response::Close(&mut coordinator).expect("first close");
    kv::Response::Close(&mut coordinator).expect("idempotent close");
    assert!(kv::MppCoordinator::IsClosed(&coordinator));
    assert_eq!(client.cancels.load(Ordering::SeqCst), 1);
}

/// Execute 并行启动多请求；Next 可交错消费不同流的包。
#[test]
fn execute_starts_requests_in_parallel_and_next_observes_interleaved_streams() {
    let (first_gate_sender, first_gate) = mpsc::channel();
    let (second_gate_sender, second_gate) = mpsc::channel();
    let (dispatch_sender, dispatch_receiver) = mpsc::channel();
    let closes = Arc::new(AtomicUsize::new(0));
    let transport = Arc::new(ParallelTransport {
        gates: Mutex::new(HashMap::from([(201, first_gate), (202, second_gate)])),
        dispatched: dispatch_sender,
        closes: closes.clone(),
        cancels: AtomicUsize::new(0),
    });
    let mut coordinator = coordinator(transport.clone(), false);
    coordinator.install_request(request(201));
    coordinator.install_request(request(202));

    kv::MppCoordinator::Execute(&mut coordinator, &kv::Context::todo())
        .expect("Execute only starts workers and does not wait for stream packets");
    let mut dispatched = vec![
        dispatch_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("first request dispatched"),
        dispatch_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("second request dispatched while first stream is blocked"),
    ];
    dispatched.sort_unstable();
    assert_eq!(dispatched, vec![201, 202]);

    second_gate_sender.send(()).expect("release second stream");
    let second = kv::Response::Next(&mut coordinator, &kv::Context::todo())
        .expect("second stream response")
        .expect("second packet");
    assert_eq!(second.GetData(), &[202]);

    first_gate_sender.send(()).expect("release first stream");
    let first = kv::Response::Next(&mut coordinator, &kv::Context::todo())
        .expect("first stream response")
        .expect("first packet");
    assert_eq!(first.GetData(), &[201]);
    assert!(
        kv::Response::Next(&mut coordinator, &kv::Context::todo())
            .expect("workers finish")
            .is_none()
    );
    assert_eq!(closes.load(Ordering::SeqCst), 2);

    kv::Response::Close(&mut coordinator).expect("close parallel coordinator");
    kv::Response::Close(&mut coordinator).expect("idempotent close");
    assert_eq!(transport.cancels.load(Ordering::SeqCst), 1);
}

/// 派发失败由 Next 返回，并 Cancel 运行中 store 一次。
#[test]
fn dispatch_failure_is_delivered_by_next_and_cancels_running_store_once() {
    let transport = Arc::new(FailingTransport::default());
    let mut coordinator = coordinator(transport.clone(), false);
    coordinator.install_request(request(102));

    kv::MppCoordinator::Execute(&mut coordinator, &kv::Context::todo())
        .expect("dispatch starts asynchronously");
    let error = match kv::Response::Next(&mut coordinator, &kv::Context::todo()) {
        Err(error) => error,
        Ok(_) => panic!("dispatch error must be returned by Next"),
    };
    assert_eq!(error.to_string(), "dispatch failed");
    assert_eq!(transport.dispatches.load(Ordering::SeqCst), 1);
    assert_eq!(transport.cancels.load(Ordering::SeqCst), 1);
    assert_eq!(
        transport
            .cancelled_addresses
            .lock()
            .expect("address lock")
            .get("tiflash-1:3930"),
        Some(&true)
    );
    assert_eq!(
        *transport.cancelled_states.lock().expect("state lock"),
        vec![kv::MppTaskStates::MppTaskCancelled]
    );

    kv::Response::Close(&mut coordinator).expect("close after failure");
    kv::Response::Close(&mut coordinator).expect("second close");
    assert_eq!(transport.cancels.load(Ordering::SeqCst), 1);
}

/// ReportStatus 先到达时，stream error 应沿用 Go 的首个错误文案。
#[test]
fn stream_error_prefers_the_first_reported_task_error() {
    let client = Arc::new(TestMPPClient::new(vec![error_packet("packet error")]));
    let transport = Arc::new(MppClientCoordinatorTransport::New(client));
    let mut coordinator = coordinator(transport, false);
    coordinator.install_request(request(106));

    let mut report = ReportTaskStatusRequest::new();
    let mut meta = TaskMeta::new();
    meta.set_task_id(106);
    report.set_meta(meta);
    let mut report_error = MppError::new();
    report_error.set_msg("status error".to_owned());
    report.set_error(report_error);
    kv::MppCoordinator::ReportStatus(
        &mut coordinator,
        kv::ReportStatusRequest { Request: report },
    )
    .expect("task report accepted");

    kv::MppCoordinator::Execute(&mut coordinator, &kv::Context::todo()).expect("dispatch starts");
    let error = match kv::Response::Next(&mut coordinator, &kv::Context::todo()) {
        Err(error) => error,
        Ok(_) => panic!("stream error is returned"),
    };
    assert_eq!(
        error.to_string(),
        "other error for mpp stream: status error"
    );
}

/// ReportStatus 解码 execution info；拒绝重复与未知 task。
#[test]
fn report_status_decodes_execution_info_and_rejects_duplicate_or_unknown_tasks() {
    let transport = Arc::new(FailingTransport::default());
    let sink = Arc::new(RecordingReportSink::default());
    let mut coordinator =
        coordinator_with_sink(transport, true, sink.clone(), Duration::from_secs(1));
    coordinator.install_request(request(103));

    let mut execution_info = tipb::TiFlashExecutionInfo::new();
    let invalid_summary = tipb::ExecutorExecutionSummary::new();
    let mut valid_summary = tipb::ExecutorExecutionSummary::new();
    valid_summary.set_time_processed_ns(1);
    valid_summary.set_num_produced_rows(2);
    valid_summary.set_num_iterations(3);
    execution_info.set_execution_summaries(vec![invalid_summary, valid_summary].into());
    let mut request = ReportTaskStatusRequest::new();
    let mut meta = TaskMeta::new();
    meta.set_task_id(103);
    request.set_meta(meta);
    request.set_data(execution_info.write_to_bytes().expect("serialize report"));
    kv::MppCoordinator::ReportStatus(
        &mut coordinator,
        kv::ReportStatusRequest {
            Request: request.clone(),
        },
    )
    .expect("first report accepted");
    assert_eq!(coordinator.reported_request_count(), 1);
    assert_eq!(coordinator.execution_summary_count(103), Some(2));

    kv::Response::Close(&mut coordinator).expect("close consumes completed reports");
    assert_eq!(coordinator.completed_execution_summary_count(), 2);
    assert_eq!(sink.records.load(Ordering::SeqCst), 1);
    assert_eq!(sink.merges.load(Ordering::SeqCst), 1);
    assert_eq!(sink.fills.load(Ordering::SeqCst), 1);
    assert_eq!(sink.timeouts.load(Ordering::SeqCst), 0);

    let duplicate = kv::MppCoordinator::ReportStatus(
        &mut coordinator,
        kv::ReportStatusRequest { Request: request },
    )
    .expect_err("duplicate report rejected");
    assert!(duplicate.to_string().contains("already received"));

    let mut unknown = ReportTaskStatusRequest::new();
    let mut meta = TaskMeta::new();
    meta.set_task_id(999);
    unknown.set_meta(meta);
    let unknown = kv::MppCoordinator::ReportStatus(
        &mut coordinator,
        kv::ReportStatusRequest { Request: unknown },
    )
    .expect_err("unknown task rejected");
    assert!(unknown.to_string().contains("task not exists"));
}

/// Close 等待 report 超时记指标，仍返回成功且幂等。
#[test]
fn close_reports_timeout_as_metric_and_returns_success() {
    let transport = Arc::new(FailingTransport::default());
    let sink = Arc::new(RecordingReportSink::default());
    let mut coordinator =
        coordinator_with_sink(transport, true, sink.clone(), Duration::from_millis(1));
    coordinator.install_request(request(104));

    kv::Response::Close(&mut coordinator).expect("report timeout is non-fatal");
    kv::Response::Close(&mut coordinator).expect("second close remains idempotent");
    assert_eq!(sink.timeouts.load(Ordering::SeqCst), 1);
}

/// Close 在等待 report 时，可经 Manager.ReportStatus 唤醒且不依赖协调器互斥锁。
#[test]
fn report_status_wakes_close_without_coordinator_mutex() {
    let transport = Arc::new(FailingTransport::default());
    let sink = Arc::new(RecordingReportSink::default());
    let mut coordinator =
        coordinator_with_sink(transport, true, sink.clone(), Duration::from_secs(2));
    coordinator.install_request(request(105));
    let report_handle = coordinator.report_handle();
    let reporter = kv::MppCoordinator::StatusReporter(&coordinator);
    let shared: SharedMppCoordinator = Arc::new(Mutex::new(Box::new(coordinator)));
    let manager = Arc::new(MppCoordinatorManager::default());
    let id = CoordinatorUniqueId {
        query_id: kv::MPPQueryID::default(),
        gather_id: 12,
    };
    manager
        .Register(id, shared.clone(), reporter)
        .expect("register coordinator");
    let close_thread = thread::spawn(move || {
        let mut coordinator = shared.lock().expect("coordinator lock");
        kv::Response::Close(coordinator.as_mut())
    });

    while !report_handle.is_waiting() {
        thread::yield_now();
    }

    let mut report = ReportTaskStatusRequest::new();
    let mut meta = TaskMeta::new();
    meta.set_task_id(105);
    report.set_meta(meta);
    manager
        .ReportStatus(id, kv::ReportStatusRequest { Request: report })
        .expect("concurrent report accepted");
    close_thread
        .join()
        .expect("close thread")
        .expect("close awakened by report");
    assert_eq!(sink.merges.load(Ordering::SeqCst), 1);
    assert_eq!(sink.fills.load(Ordering::SeqCst), 1);
    assert_eq!(sink.timeouts.load(Ordering::SeqCst), 0);
    manager.Unregister(id);
}
