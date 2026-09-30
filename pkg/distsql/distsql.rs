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

// DistSQL DAG/Analyze/Checksum 请求如何构造 SelectResult，并记录 TiFlash
// 元数据、chunk RPC 对齐检查和 client-go interceptor 绑定语义；传输由 `KvClient` 抽象承接。

use astersql_util_execdetails::execdetails;
use std::sync::{Arc, Mutex};

use crate::request_builder::KvRequest;
use crate::select_result::{ResultStatsContext, SelectResult, SelectResultIter, selectResult};
use crate::{DistSqlError, DistSqlResult, ResponseSource, SelectResponse, StoreType};

/// 为装箱的响应源实现委托，便于统一持有 `Box<dyn ResponseSource>`。
impl ResponseSource for Box<dyn ResponseSource> {
    fn next_response(&mut self) -> DistSqlResult<Option<SelectResponse>> {
        (**self).next_response()
    }
    fn close(&mut self) -> DistSqlResult<()> {
        (**self).close()
    }
    fn next_response_with_error(&mut self) -> (Option<SelectResponse>, Option<DistSqlError>) {
        (**self).next_response_with_error()
    }
    fn collect_unconsumed_cop_stats(&mut self) -> Vec<crate::CopRuntimeEvidence> {
        (**self).collect_unconsumed_cop_stats()
    }
    fn limiter_wait_stats(&self) -> crate::LimiterWaitStats {
        (**self).limiter_wait_stats()
    }
}

/// KV 客户端抽象：发送 DistSQL 请求并返回响应源。
pub trait KvClient: Send + Sync {
    /// 发送请求；返回可按批拉取的响应源。
    fn send(&self, request: &KvRequest) -> DistSqlResult<Box<dyn ResponseSource>>;
    /// Analyze captures the execution-info setting at send time.
    fn send_with_options(
        &self,
        request: &KvRequest,
        _options: &ClientSendOptions,
    ) -> DistSqlResult<Box<dyn ResponseSource>> {
        self.send(request)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClientSendOptions {
    pub enable_collect_execution_info: bool,
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
    /// Shared per-store cop request limiter for this query.
    pub query_cop_store_limiter: Option<Arc<astersql_kv::QueryCopStoreLimiter>>,
    pub exec_details: Option<Arc<execdetails::SyncExecDetails>>,
    pub runtime_stats: Option<Arc<Mutex<execdetails::RuntimeStatsColl>>>,
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
    context: &DistSQLContext,
    response: Box<dyn ResponseSource>,
    _field_types: &[String],
    _plan_ids: &[i32],
    _root_id: i32,
    reports_directly: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Box<DistSQLSelectResult> {
    let mut result = DistSQLSelectResult {
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
    };
    result.inner = result.inner.with_stats_context(ResultStatsContext {
        exec_details: context.exec_details.clone(),
        runtime_stats: context.runtime_stats.clone(),
        root_plan_id: _root_id,
        cop_plan_ids: _plan_ids.to_vec(),
        store_type: StoreType::TiFlash,
        is_analyze: false,
        collect_raw_details: false,
        mpp_reports_directly: Some(reports_directly),
    });
    Box::new(result)
}

/// 发送 DAG 类请求并返回 SelectResult。
pub fn Select(
    context: &DistSQLContext,
    request: &KvRequest,
    field_types: &[String],
) -> DistSqlResult<Box<DistSQLSelectResult>> {
    let mut request = request.clone();
    if let Some(limiter) = &context.query_cop_store_limiter {
        request.query_cop_store_limiter = Some(Arc::clone(limiter));
    }
    let response = context.client.send(&request)?;
    let mut result = new_select_result(
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
    );
    result.inner = result.inner.with_stats_context(ResultStatsContext {
        exec_details: context.exec_details.clone(),
        runtime_stats: context.runtime_stats.clone(),
        root_plan_id: 0,
        cop_plan_ids: Vec::new(),
        store_type: request.store_type,
        is_analyze: false,
        collect_raw_details: false,
        mpp_reports_directly: None,
    });
    Ok(Box::new(result))
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
    result.inner = result.inner.with_stats_context(ResultStatsContext {
        exec_details: context.exec_details.clone(),
        runtime_stats: context.runtime_stats.clone(),
        root_plan_id: _root_id,
        cop_plan_ids: _plan_ids.to_vec(),
        store_type: request.store_type,
        is_analyze: false,
        collect_raw_details: false,
        mpp_reports_directly: None,
    });
    Ok(result)
}

/// 发送 Analyze 请求；payload 必须为 Analyze 类型。
pub fn Analyze(
    client: &dyn KvClient,
    request: &KvRequest,
    is_restricted: bool,
    context: &DistSQLContext,
    plan_id: i32,
) -> DistSqlResult<Box<DistSQLSelectResult>> {
    let mut request = request.clone();
    request.request_source = "stats".to_owned();
    let collect_execution_info = astersql_config::get_global_config()
        .instance
        .enable_collect_execution_info
        .load();
    let options = ClientSendOptions {
        enable_collect_execution_info: collect_execution_info,
    };
    let mut result = new_select_result(
        client.send_with_options(&request, &options)?,
        request.concurrency,
        "analyze",
        0,
        if is_restricted { "internal" } else { "general" },
        request.store_type,
        false,
    );
    result.root_plan_id = plan_id;
    result.inner = result.inner.with_stats_context(ResultStatsContext {
        exec_details: context.exec_details.clone(),
        runtime_stats: context.runtime_stats.clone(),
        root_plan_id: plan_id,
        cop_plan_ids: Vec::new(),
        store_type: request.store_type,
        is_analyze: true,
        collect_raw_details: options.enable_collect_execution_info,
        mpp_reports_directly: None,
    });
    Ok(Box::new(result))
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
