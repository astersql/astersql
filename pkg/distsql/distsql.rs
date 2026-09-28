// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// DistSQL 核心发送与编码逻辑。
//
// 提供 DAG/Analyze/Checksum 请求如何包装为 `SelectResult`、TiFlash 配置如何写入
// 出站元数据、chunk RPC 编码选择，以及 SQL KV 执行计数 interceptor 绑定。
// 上方大块注释保留 Go→Rust 机械翻译草稿，供迁移对照；下方为当前可编译实现。

// DistSQL DAG/Analyze/Checksum 请求如何构造 SelectResult，并记录 TiFlash
// 元数据、chunk RPC 对齐检查和 client-go interceptor 绑定语义；传输由 `KvClient` 抽象承接。

/* Mechanical draft retained for migration history.
// GenSelectResultFromMPPResponse 对应 Go 的同名函数：从 MPP response 包装一个 selectResult 迭代器。
pub fn GenSelectResultFromMPPResponse(
    dctx: *mut distsqlctx::DistSQLContext,
    fieldTypes: Vec<*mut types::FieldType>,
    planIDs: Vec<i32>,
    rootID: i32,
    resp: kv::Response,
) -> Box<dyn SelectResult> {
    Box::new(selectResult {
        label: "mpp".to_string(),
        resp: Some(resp),
        rowLen: fieldTypes.len() as i32,
        fieldTypes,
        ctx: dctx,
        copPlanIDs: planIDs,
        rootPlanID: rootID,
        storeType: kv::TiFlash,
        ..Default::default()
    })
}

// Select 发送 DAG 请求并返回 SelectResult。
// Go 版本在这里启动 tracing region、挂测试 hook、配置 client send option 并调用 dctx.Client.Send。
pub fn Select(
    mut ctx: context::Context,
    dctx: *mut distsqlctx::DistSQLContext,
    kvReq: *mut kv::Request,
    fieldTypes: Vec<*mut types::FieldType>,
) -> Result<Box<dyn SelectResult>, errors::Error> {
    let (region, next_ctx) = tracing::StartRegionEx(ctx, "distsql.Select");
    ctx = next_ctx;
    defer_region_end(region);

    // 测试 hook 保留 Go 的 ctx.Value("CheckSelectRequestHook") 语义，用于检查即将发送的 kv.Request。
    if let Some(hook) = ctx.Value("CheckSelectRequestHook") {
        hook.call(kvReq);
    }

    let enabledRateLimitAction = unsafe { (*dctx).EnabledRateLimitAction };
    let originalSQL = unsafe { (*dctx).OriginalSQL.clone() };
    let eventCb = move |event: trxevents::TransactionEvent| {
        // Go 注释强调该回调可能不在同一个 goroutine 中执行；保留并发回调语义。
        if let Some(copMeetLock) = event.GetCopMeetLock() {
            logutil::Logger(ctx.clone()).Debug(
                "coprocessor encounters lock",
                zap::Uint64("startTS", unsafe { (*kvReq).StartTs }),
                zap::Stringer("lock", copMeetLock.LockInfo),
                zap::String("stmt", originalSQL.clone()),
            );
        }
    };

    ctx = WithSQLKvExecCounterInterceptor(ctx, unsafe { (*dctx).KvExecCounter });
    let mut option = kv::ClientSendOption {
        SessionMemTracker: unsafe { (*dctx).SessionMemTracker },
        EnabledRateLimitAction: enabledRateLimitAction,
        EventCb: Some(Box::new(eventCb)),
        EnableCollectExecutionInfo: config::GetGlobalConfig()
            .Instance
            .EnableCollectExecutionInfo
            .Load(),
        TryCopLiteWorker: unsafe { &mut (*dctx).TryCopLiteWorker },
        ..Default::default()
    };

    // failpoint 保留 Go 中强制设置 TryCopLiteWorker 的测试入口；真实注入机制留给后续接线。
    failpoint::Inject("TryCopLiteWorker", |val| {
        let n = val.as_i32().expect("TryCopLiteWorker expects int");
        option.TryCopLiteWorker.Store(n as u32);
        logutil::Logger(ctx.clone()).Info(
            "setting TryCopLiteWorker for test",
            zap::String("value", option.TryCopLiteWorker.String()),
        );
    });

    if unsafe { (*kvReq).StoreType == kv::TiFlash } {
        ctx = SetTiFlashConfVarsInContext(ctx, dctx);
        option.TiFlashReplicaRead = unsafe { (*dctx).TiFlashReplicaRead };
        option.AppendWarning = unsafe { (*dctx).AppendWarning };
    }

    let resp = unsafe { (*dctx).Client.Send(ctx, kvReq, (*dctx).KVVars, &mut option) };
    if resp.is_none() {
        return Err(errors::New("client returns nil response"));
    }

    let mut label = metrics::LblGeneral;
    if unsafe { (*dctx).InRestrictedSQL } {
        label = metrics::LblInternal;
    }

    // Go 中复用 kvReq.MemTracker 跟踪 DistSQL 层内存，selectResult 不另建 tracker。
    Ok(Box::new(selectResult {
        label: "dag".to_string(),
        resp,
        rowLen: fieldTypes.len() as i32,
        fieldTypes,
        ctx: dctx,
        sqlType: label.to_string(),
        memTracker: unsafe { (*kvReq).MemTracker },
        storeType: unsafe { (*kvReq).StoreType },
        paging: unsafe { (*kvReq).Paging.Enable || (*kvReq).Paging.PagingSizeBytes > 0 },
        distSQLConcurrency: unsafe { (*kvReq).Concurrency },
        ..Default::default()
    }))
}

// SetTiFlashConfVarsInContext 对应 Go 的 metadata.AppendToOutgoingContext 串联逻辑。
pub fn SetTiFlashConfVarsInContext(
    mut ctx: context::Context,
    dctx: *mut distsqlctx::DistSQLContext,
) -> context::Context {
    if unsafe { (*dctx).TiFlashMaxThreads != -1 } {
        ctx = metadata::AppendToOutgoingContext(
            ctx,
            vardef::TiDBMaxTiFlashThreads,
            unsafe { (*dctx).TiFlashMaxThreads }.to_string(),
        );
    }
    if unsafe { (*dctx).TiFlashMaxBytesBeforeExternalJoin != -1 } {
        ctx = metadata::AppendToOutgoingContext(
            ctx,
            vardef::TiDBMaxBytesBeforeTiFlashExternalJoin,
            unsafe { (*dctx).TiFlashMaxBytesBeforeExternalJoin }.to_string(),
        );
    }
    if unsafe { (*dctx).TiFlashMaxBytesBeforeExternalGroupBy != -1 } {
        ctx = metadata::AppendToOutgoingContext(
            ctx,
            vardef::TiFlashMaxBytesBeforeTiFlashExternalGroupBy,
            unsafe { (*dctx).TiFlashMaxBytesBeforeExternalGroupBy }.to_string(),
        );
    }
    if unsafe { (*dctx).TiFlashMaxBytesBeforeExternalSort != -1 } {
        ctx = metadata::AppendToOutgoingContext(
            ctx,
            vardef::TiFlashMaxBytesBeforeExternalSort,
            unsafe { (*dctx).TiFlashMaxBytesBeforeExternalSort }.to_string(),
        );
    }
    let quota = if unsafe { (*dctx).TiFlashMaxQueryMemoryPerNode <= 0 } {
        "0".to_string()
    } else {
        unsafe { (*dctx).TiFlashMaxQueryMemoryPerNode }.to_string()
    };
    ctx = metadata::AppendToOutgoingContext(ctx, vardef::TiFlashMemQuotaQueryPerNode, quota);
    ctx = metadata::AppendToOutgoingContext(
        ctx,
        vardef::TiFlashQuerySpillRatio,
        unsafe { (*dctx).TiFlashQuerySpillRatio }.to_string(),
    );
    metadata::AppendToOutgoingContext(
        ctx,
        "tiflash_use_hash_join_v2",
        joinversion::IsOptimizedVersion(unsafe { (*dctx).TiFlashHashJoinVersion }).to_string(),
    )
}

// SelectWithRuntimeStats 与 Select 的区别是把 copPlanIDs/rootPlanID 写回 selectResult 以便统计。
pub fn SelectWithRuntimeStats(
    ctx: context::Context,
    dctx: *mut distsqlctx::DistSQLContext,
    kvReq: *mut kv::Request,
    fieldTypes: Vec<*mut types::FieldType>,
    copPlanIDs: Vec<i32>,
    rootPlanID: i32,
) -> Result<Box<dyn SelectResult>, errors::Error> {
    let mut sr = Select(ctx, dctx, kvReq, fieldTypes)?;
    if let Some(select_result) = sr.downcast_mut::<selectResult>() {
        select_result.copPlanIDs = copPlanIDs;
        select_result.rootPlanID = rootPlanID;
    }
    Ok(sr)
}

// Analyze 发送 analyze 请求。Go 中 mockAnalyzeRequestWaitForCancel failpoint 会等待 ctx cancel 后返回。
pub fn Analyze(
    mut ctx: context::Context,
    client: kv::Client,
    kvReq: *mut kv::Request,
    vars: Any,
    isRestrict: bool,
    dctx: *mut distsqlctx::DistSQLContext,
) -> Result<Box<dyn SelectResult>, errors::Error> {
    ctx = WithSQLKvExecCounterInterceptor(ctx, unsafe { (*dctx).KvExecCounter });
    failpoint::Inject("mockAnalyzeRequestWaitForCancel", |val| {
        if val.as_bool() {
            ctx.Done().recv();
            let err = context::Cause(ctx.clone()).or_else(|| ctx.Err());
            failpoint::Return((None::<Box<dyn SelectResult>>, err));
        }
    });
    unsafe {
        (*kvReq).RequestSource.RequestSourceInternal = true;
        (*kvReq).RequestSource.RequestSourceType = kv::InternalTxnStats;
    }
    let resp = client.Send(ctx, kvReq, vars, &kv::ClientSendOption::default());
    if resp.is_none() {
        return Err(errors::New("client returns nil response"));
    }
    let sqlType = if isRestrict { metrics::LblInternal } else { metrics::LblGeneral };
    Ok(Box::new(selectResult {
        label: "analyze".to_string(),
        resp,
        sqlType: sqlType.to_string(),
        storeType: unsafe { (*kvReq).StoreType },
        ..Default::default()
    }))
}

// Checksum 发送 checksum 请求；Go 注释说明该签名受 BR 双向依赖影响暂不能改。
pub fn Checksum(
    ctx: context::Context,
    client: kv::Client,
    kvReq: *mut kv::Request,
    vars: Any,
) -> Result<Box<dyn SelectResult>, errors::Error> {
    let resp = client.Send(ctx, kvReq, vars, &kv::ClientSendOption::default());
    if resp.is_none() {
        return Err(errors::New("client returns nil response"));
    }
    Ok(Box::new(selectResult {
        label: "checksum".to_string(),
        resp,
        sqlType: metrics::LblGeneral.to_string(),
        storeType: unsafe { (*kvReq).StoreType },
        ..Default::default()
    }))
}

// SetEncodeType 设置 DAGRequest 的编码方式；chunk RPC 可用时还会写入内存布局。
pub fn SetEncodeType(ctx: *mut distsqlctx::DistSQLContext, dagReq: *mut tipb::DAGRequest) {
    if canUseChunkRPC(ctx) {
        unsafe { (*dagReq).EncodeType = tipb::EncodeType_TypeChunk };
        setChunkMemoryLayout(dagReq);
    } else {
        unsafe { (*dagReq).EncodeType = tipb::EncodeType_TypeDefault };
    }
}

// canUseChunkRPC 保留 Go 的两步检查：会话开关与 MyDecimal 对齐。
fn canUseChunkRPC(ctx: *mut distsqlctx::DistSQLContext) -> bool {
    if unsafe { !(*ctx).EnableChunkRPC } {
        return false;
    }
    if !checkAlignment() {
        return false;
    }
    true
}

// supportedAlignment 对应 Go 的包级变量，依赖 types.MyDecimal 的内存大小。
static supportedAlignment: bool = core::mem::size_of::<types::MyDecimal>() == 40;

// checkAlignment 检查当前系统环境的内存对齐是否满足 chunk RPC 预期。
pub fn checkAlignment() -> bool {
    supportedAlignment
}

// systemEndian 在 Go init 中探测并缓存；保留同一个包级状态形状。
static mut systemEndian: tipb::Endian = tipb::Endian_LittleEndian;

// setChunkMemoryLayout 为 DAGRequest 设置 chunk 内存布局。
fn setChunkMemoryLayout(dagReq: *mut tipb::DAGRequest) {
    unsafe {
        (*dagReq).ChunkMemoryLayout = Some(tipb::ChunkMemoryLayout {
            Endian: GetSystemEndian(),
        });
    }
}

// GetSystemEndian 返回 init 阶段探测到的系统端序。
pub fn GetSystemEndian() -> tipb::Endian {
    unsafe { systemEndian }
}

// init 对应 Go 包初始化函数：通过 0x0100 的首字节判断大端/小端。
pub fn init() {
    let i: u16 = 0x0100;
    let first = (&i as *const u16).cast::<u8>();
    unsafe {
        if 0x01 == *first {
            systemEndian = tipb::Endian_BigEndian;
        } else {
            systemEndian = tipb::Endian_LittleEndian;
        }
    }
}

// WithSQLKvExecCounterInterceptor 为 client-go 手动绑定 RPCInterceptor，用来统计各 TiKV SQL 执行次数。
pub fn WithSQLKvExecCounterInterceptor(
    ctx: context::Context,
    counter: *mut stmtstats::KvExecCounter,
) -> context::Context {
    if !counter.is_null() {
        // Go 中 DistSQL 直接面对 tikv Request，不能通过 Transaction/Snapshot 设置 interceptor。
        return interceptor::WithRPCInterceptor(ctx, unsafe { (*counter).RPCInterceptor() });
    }
    ctx
}
*/

use std::sync::Arc;

use crate::request_builder::KvRequest;
use crate::select_result::{SelectResult, SelectResultIter, selectResult};
use crate::{DistSqlError, DistSqlResult, ResponseSource, SelectResponse, StoreType};

/// 为装箱的响应源实现委托，便于统一持有 `Box<dyn ResponseSource>`。
impl ResponseSource for Box<dyn ResponseSource> {
    fn next_response(&mut self) -> DistSqlResult<Option<SelectResponse>> {
        (**self).next_response()
    }
    fn close(&mut self) -> DistSqlResult<()> {
        (**self).close()
    }
}

/// KV 客户端抽象：发送 DistSQL 请求并返回响应源。
pub trait KvClient: Send + Sync {
    /// 发送请求；返回可按批拉取的响应源。
    fn send(&self, request: &KvRequest) -> DistSqlResult<Box<dyn ResponseSource>>;
}

/// Go `selectResult` 在发送入口处写入、供后续解码和统计使用的请求元数据。
pub struct DistSQLSelectResult {
    inner: selectResult<Box<dyn ResponseSource>>,
    pub label: &'static str,
    pub row_len: usize,
    pub cop_plan_ids: Vec<i32>,
    pub root_plan_id: i32,
    pub sql_type: &'static str,
    pub store_type: StoreType,
    pub paging: bool,
}

impl SelectResult for DistSQLSelectResult {
    fn NextRaw(&mut self) -> DistSqlResult<Option<Vec<u8>>> {
        self.inner.NextRaw()
    }

    fn Next(
        &mut self,
        rows: &mut Vec<crate::select_result::Row>,
        capacity: usize,
    ) -> DistSqlResult<()> {
        self.inner.Next(rows, capacity)
    }

    fn IntoIter(self: Box<Self>) -> DistSqlResult<Box<dyn SelectResultIter>> {
        Box::new(self.inner).IntoIter()
    }

    fn Close(&mut self) -> DistSqlResult<()> {
        self.inner.Close()
    }

    fn concurrency(&self) -> Option<(usize, usize)> {
        self.inner.concurrency()
    }
}

fn new_select_result(
    response: Box<dyn ResponseSource>,
    concurrency: usize,
    label: &'static str,
    row_len: usize,
    sql_type: &'static str,
    store_type: StoreType,
    paging: bool,
) -> DistSQLSelectResult {
    DistSQLSelectResult {
        inner: selectResult::new(response, concurrency),
        label,
        row_len,
        cop_plan_ids: Vec::new(),
        root_plan_id: 0,
        sql_type,
        store_type,
        paging,
    }
}

/// DAG 结果行的编码方式。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EncodeType {
    /// 默认 protobuf 行编码。
    #[default]
    Default,
    /// Chunk（列式内存块）RPC 编码，需内存对齐支持。
    Chunk,
}
/// 字节序：写入 chunk 内存布局时使用。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Endian {
    #[default]
    Little,
    Big,
}

/// 简化版 DAG 请求：编码类型、chunk 布局与执行摘要收集开关。
#[derive(Clone, Debug, Default)]
pub struct DAGRequest {
    /// 行编码类型。
    pub encode_type: EncodeType,
    /// Chunk 内存布局的端序；仅 chunk 编码时设置。
    pub chunk_memory_layout: Option<Endian>,
    /// 是否收集执行摘要（execution summaries）。
    pub collect_execution_summaries: bool,
}

/// DistSQL 运行时上下文（简化版）：客户端、chunk 开关、并发与 TiFlash 配额。
#[derive(Clone)]
pub struct DistSQLContext {
    /// 发送请求的 KV 客户端。
    pub client: Arc<dyn KvClient>,
    /// 是否为内部受限 SQL；决定结果统计标签。
    pub in_restricted_sql: bool,
    /// 是否允许使用 chunk RPC。
    pub enable_chunk_rpc: bool,
    /// 是否流式返回。
    pub streaming: bool,
    /// 默认并发度。
    pub concurrency: usize,
    /// TiFlash 最大线程数；负值表示未设置。
    pub tiflash_max_threads: i64,
    /// TiFlash Join 落盘前最大字节数。
    pub tiflash_max_bytes_before_external_join: i64,
    /// TiFlash GroupBy 落盘前最大字节数。
    pub tiflash_max_bytes_before_external_group_by: i64,
    /// TiFlash Sort 落盘前最大字节数。
    pub tiflash_max_bytes_before_external_sort: i64,
    /// TiFlash query memory quota; non-positive values are encoded as zero.
    pub tiflash_max_query_memory_per_node: i64,
    /// TiFlash spill ratio, always forwarded with a request.
    pub tiflash_query_spill_ratio: f64,
    /// Whether the optimized TiFlash hash join implementation is enabled.
    pub tiflash_use_hash_join_v2: bool,
}

/// 从 MPP（Massively Parallel Processing）响应包装 SelectResult；字段类型/计划 ID 占位保留签名。
pub fn GenSelectResultFromMPPResponse(
    response: Box<dyn ResponseSource>,
    _field_types: &[String],
    _plan_ids: &[i32],
    _root_id: i32,
) -> Box<DistSQLSelectResult> {
    Box::new(DistSQLSelectResult {
        cop_plan_ids: _plan_ids.to_vec(),
        root_plan_id: _root_id,
        ..new_select_result(
            response,
            0,
            "mpp",
            _field_types.len(),
            "general",
            StoreType::TiFlash,
            false,
        )
    })
}

/// 发送 DAG 类请求并返回 SelectResult。
pub fn Select(
    context: &DistSQLContext,
    request: &KvRequest,
    field_types: &[String],
) -> DistSqlResult<Box<DistSQLSelectResult>> {
    let response = context.client.send(request)?;
    Ok(Box::new(new_select_result(
        response,
        request.concurrency,
        "dag",
        field_types.len(),
        if context.in_restricted_sql {
            "internal"
        } else {
            "general"
        },
        request.store_type,
        request.paging,
    )))
}

/// 与 Select 相同，额外保留运行时统计相关 plan ID，供结果消费阶段归集统计。
pub fn SelectWithRuntimeStats(
    context: &DistSQLContext,
    request: &KvRequest,
    _field_types: &[String],
    _plan_ids: &[i32],
    _root_id: i32,
) -> DistSqlResult<Box<DistSQLSelectResult>> {
    let mut result = Select(context, request, _field_types)?;
    result.cop_plan_ids = _plan_ids.to_vec();
    result.root_plan_id = _root_id;
    Ok(result)
}

/// 发送 Analyze 请求；payload 必须为 Analyze 类型。
pub fn Analyze(
    client: &dyn KvClient,
    request: &KvRequest,
    is_restricted: bool,
) -> DistSqlResult<Box<DistSQLSelectResult>> {
    let mut request = request.clone();
    request.request_source = "stats".to_owned();
    Ok(Box::new(new_select_result(
        client.send(&request)?,
        request.concurrency,
        "analyze",
        0,
        if is_restricted { "internal" } else { "general" },
        request.store_type,
        false,
    )))
}

/// 发送 Checksum 请求；payload 必须为 Checksum 类型。
pub fn Checksum(
    client: &dyn KvClient,
    request: &KvRequest,
) -> DistSqlResult<Box<DistSQLSelectResult>> {
    Ok(Box::new(new_select_result(
        client.send(request)?,
        request.concurrency,
        "checksum",
        0,
        "general",
        request.store_type,
        false,
    )))
}

/// 将 TiFlash 会话变量写入出站 metadata。
pub fn SetTiFlashConfVarsInContext(context: &DistSQLContext, metadata: &mut Vec<(String, String)>) {
    let variables = [
        ("tidb_max_tiflash_threads", context.tiflash_max_threads),
        (
            "tidb_max_bytes_before_tiflash_external_join",
            context.tiflash_max_bytes_before_external_join,
        ),
        (
            "tidb_max_bytes_before_tiflash_external_group_by",
            context.tiflash_max_bytes_before_external_group_by,
        ),
        (
            "tidb_max_bytes_before_tiflash_external_sort",
            context.tiflash_max_bytes_before_external_sort,
        ),
    ];
    for (key, value) in variables {
        // Go skips only the sentinel -1; other negative values are still
        // explicit session settings and must be forwarded unchanged.
        if value != -1 {
            metadata.push((key.to_owned(), value.to_string()));
        }
    }
    let quota = if context.tiflash_max_query_memory_per_node <= 0 {
        "0".to_owned()
    } else {
        context.tiflash_max_query_memory_per_node.to_string()
    };
    metadata.push(("tiflash_mem_quota_query_per_node".to_owned(), quota));
    metadata.push((
        "tiflash_query_spill_ratio".to_owned(),
        context.tiflash_query_spill_ratio.to_string(),
    ));
    metadata.push((
        "tiflash_use_hash_join_v2".to_owned(),
        context.tiflash_use_hash_join_v2.to_string(),
    ));
}

/// 按会话开关与对齐检查设置 DAG 编码类型；chunk 时同步写入内存布局。
pub fn SetEncodeType(context: &DistSQLContext, request: &mut DAGRequest) {
    request.encode_type = if canUseChunkRPC(context) {
        EncodeType::Chunk
    } else {
        EncodeType::Default
    };
    if request.encode_type == EncodeType::Chunk {
        setChunkMemoryLayout(request);
    }
}
/// 判断是否可使用 chunk RPC：需开启开关且满足内存对齐。
pub fn canUseChunkRPC(context: &DistSQLContext) -> bool {
    context.enable_chunk_rpc && checkAlignment()
}
/// 检查当前平台对齐是否满足 chunk RPC 预期（i128 对齐至少 8）。
pub fn checkAlignment() -> bool {
    std::mem::align_of::<i128>() >= 8
}
/// 返回编译目标字节序。
pub fn GetSystemEndian() -> Endian {
    if cfg!(target_endian = "big") {
        Endian::Big
    } else {
        Endian::Little
    }
}
/// 为 DAG 请求写入 chunk 内存布局端序。
pub fn setChunkMemoryLayout(request: &mut DAGRequest) {
    request.chunk_memory_layout = Some(GetSystemEndian());
}

/// SQL KV 执行次数计数器回调。
pub trait KvExecCounter: Send + Sync {
    /// 在发往指定存储类型的请求时回调。
    fn on_request(&self, store_type: StoreType);
}
/// 携带可选 KvExecCounter 的请求上下文（对齐 Go interceptor 绑定）。
#[derive(Clone, Default)]
pub struct RequestContext {
    /// 可选的执行计数器。
    pub counter: Option<Arc<dyn KvExecCounter>>,
}
/// 将 KvExecCounter 绑定到请求上下文，供后续 RPC 拦截统计。
pub fn WithSQLKvExecCounterInterceptor(
    mut context: RequestContext,
    counter: Arc<dyn KvExecCounter>,
) -> RequestContext {
    context.counter = Some(counter);
    context
}
