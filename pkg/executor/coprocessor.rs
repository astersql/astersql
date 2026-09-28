// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 本地 Coprocessor（协处理器）DAG 请求处理。
//
// Coprocessor 将部分执行计划下推到存储节点；本模块在 TiDB/AsterSQL 侧以
// DAG（有向无环图）算子链形式解码请求、鉴权、构建执行器，并按 unary 或
// stream 方式编码 tipb 响应。`CoprocessorBackend` 抽象 protobuf、权限与
// 计划构建等生产边界。
#![allow(non_snake_case, non_upper_case_globals)]

use std::fmt::Display;

/// tipb 请求类型：DAG 执行计划。
pub const REQ_TYPE_DAG: i64 = 103;
/// Default 编码下每个 tipb Chunk 打包的行数上限。
pub const rowsPerChunk: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 请求来源语句信息，用于链路追踪（trace）。
pub struct SourceStatement {
    pub connection_id: u64,
    pub session_alias: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 注入到上下文的追踪信息（连接 ID 与会话别名）。
pub struct TraceInfo {
    pub connection_id: u64,
    pub session_alias: String,
}

/// `copHandlerCtx` 使用的上下文操作。
/// Context operations used by `copHandlerCtx`.
pub trait CopHandlerContext: Sized {
    fn with_trace_info(self, trace_info: TraceInfo) -> Self;
    fn with_trace_log_fields(self, trace_info: &TraceInfo) -> Self;
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Coprocessor 入站请求：类型、序列化 DAG、扫描范围与可选来源语句。
pub struct CoprocessorRequest<R> {
    pub request_type: i64,
    pub data: Vec<u8>,
    pub ranges: Vec<R>,
    pub source_statement: Option<SourceStatement>,
}

/// 若请求带有来源语句，则向上下文注入追踪信息与日志字段。
pub fn copHandlerCtx<C: CopHandlerContext, R>(context: C, request: &CoprocessorRequest<R>) -> C {
    let Some(source) = request.source_statement.as_ref() else {
        return context;
    };
    let trace_info = TraceInfo {
        connection_id: source.connection_id,
        session_alias: source.session_alias.clone(),
    };
    context
        .with_trace_info(trace_info.clone())
        .with_trace_log_fields(&trace_info)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// tipb 结果编码方式：逐行 Default 或列式 Chunk。
pub enum EncodeType {
    TypeDefault,
    TypeChunk,
    Unknown(i32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// DAG 请求携带的用户身份（用户名与主机）。
pub struct DagUser {
    pub user_name: String,
    pub user_host: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 解码后的 tipb DAGRequest：时区、标志位、算子列表与输出列偏移。
pub struct DAGRequest<X> {
    pub user: Option<DagUser>,
    pub time_zone_name: String,
    pub time_zone_offset: i64,
    pub flags: u64,
    pub executors: Vec<X>,
    pub output_offsets: Vec<u32>,
    pub encode_type: EncodeType,
    pub collect_execution_summaries: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单个执行器的执行摘要占位（耗时、行数等，完整字段由 tipb 定义）。
pub struct ExecutorExecutionSummary;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// tipb 响应中的一个数据块（行字节序列）。
pub struct TipbChunk {
    pub rows_data: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Unary SelectResponse：多个 tipb Chunk、编码类型与可选执行摘要。
pub struct SelectResponse {
    pub chunks: Vec<TipbChunk>,
    pub encode_type: EncodeType,
    pub execution_summaries: Vec<ExecutorExecutionSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 流式响应中的单帧数据。
pub struct StreamResponse {
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 返回给调用方的 Coprocessor 响应（序列化数据或错误栈）。
pub struct CoprocessorResponse {
    pub data: Vec<u8>,
    pub other_error: String,
}

/// Coprocessor 侧使用的列式 chunk 抽象。
pub trait CopChunk {
    type Datum;

    fn reset(&mut self);
    fn num_rows(&self) -> usize;
    fn datum(&self, row: usize, column: usize) -> Self::Datum;
}

/// DAG 物理执行器：Open/Next/Close 拉取结果 chunk。
pub trait DagExecutor<C> {
    type Error;
    type Chunk: CopChunk;
    type FieldType: Clone;

    fn open(&mut self, context: &C) -> Result<(), Self::Error>;
    fn close(&mut self) -> Result<(), Self::Error>;
    fn new_cache_chunk(&self) -> Self::Chunk;
    fn return_field_types(&self) -> Vec<Self::FieldType>;
    fn next(&mut self, context: &C, chunk: &mut Self::Chunk) -> Result<(), Self::Error>;
}

/// 生产边界：protobuf、权限、会话与计划构建等依赖的抽象接口。
/// Production boundary for protobuf, privilege, session, and plan-builder APIs.
pub trait CoprocessorBackend {
    type Context: CopHandlerContext + Clone;
    type Error: Display;
    type Range: Clone;
    type ExecutorSpec: Clone;
    type TimeZone;
    type Roles;
    type Plan;
    type Executor: DagExecutor<
            Self::Context,
            Error = Self::Error,
            Chunk = Self::Chunk,
            FieldType = Self::FieldType,
        >;
    type Chunk: CopChunk<Datum = Self::Datum>;
    type Datum;
    type FieldType: Clone;

    fn decode_dag_request(
        &mut self,
        data: &[u8],
    ) -> Result<DAGRequest<Self::ExecutorSpec>, Self::Error>;
    fn error(&self, message: String) -> Self::Error;
    fn error_stack(&self, error: &Self::Error) -> String;

    fn privilege_manager_available(&self) -> bool;
    fn set_request_user(&mut self, user_name: &str, user_host: &str);
    fn match_identity(
        &self,
        context: &Self::Context,
        user_name: &str,
        user_host: &str,
    ) -> Option<(String, String)>;
    fn auth_without_verification(&self, auth_name: &str, auth_host: &str) -> bool;
    fn default_roles(
        &self,
        context: &Self::Context,
        auth_name: &str,
        auth_host: &str,
    ) -> Self::Roles;
    fn set_authenticated_user(&mut self, auth_name: String, auth_host: String, roles: Self::Roles);

    fn construct_time_zone(&self, name: &str, offset: i64) -> Result<Self::TimeZone, Self::Error>;
    fn set_time_zone_and_statement_flags(&mut self, time_zone: Self::TimeZone, flags: u64);
    fn build_physical_plan(
        &mut self,
        ranges: &[Self::Range],
        executors: &[Self::ExecutorSpec],
    ) -> Result<Self::Plan, Self::Error>;
    fn inject_extra_projection(&mut self, plan: Self::Plan) -> Self::Plan;
    fn build_executor(
        &mut self,
        context: &Self::Context,
        plan: Self::Plan,
    ) -> Result<Self::Executor, Self::Error>;

    fn consume_statement_memory(&mut self, bytes: i64);
    fn serialized_chunk_size(&self, chunk: &TipbChunk) -> usize;
    fn debug(&mut self, context: &Self::Context, message: &str);
    fn encode_chunk(&self, chunk: &Self::Chunk, field_types: &[Self::FieldType]) -> Vec<u8>;
    fn encode_value(
        &self,
        datum: Self::Datum,
        field_type: &Self::FieldType,
    ) -> Result<Vec<u8>, Self::Error>;
    fn marshal_select_response(&self, response: &SelectResponse) -> Result<Vec<u8>, Self::Error>;
    fn marshal_chunk(&self, chunk: &TipbChunk) -> Result<Vec<u8>, Self::Error>;
    fn marshal_stream_response(&self, response: &StreamResponse) -> Result<Vec<u8>, Self::Error>;
}

/// 流式发送 CoprocessorResponse 的通道抽象。
pub trait CoprocessorStream<E> {
    fn send(&mut self, response: CoprocessorResponse) -> Result<(), E>;
}

/// 处理本地 Coprocessor DAG 请求的处理器。
/// Handles local coprocessor DAG requests.
pub struct CoprocessorDAGHandler<B: CoprocessorBackend> {
    pub sctx: B,
    pub dagReq: Option<DAGRequest<B::ExecutorSpec>>,
}

/// 使用给定会话后端构造 DAG 处理器。
pub fn NewCoprocessorDAGHandler<B: CoprocessorBackend>(
    session_context: B,
) -> CoprocessorDAGHandler<B> {
    CoprocessorDAGHandler {
        sctx: session_context,
        dagReq: None,
    }
}

impl<B: CoprocessorBackend> CoprocessorDAGHandler<B> {
    /// Unary 模式：构建执行器、拉满所有 chunk 后一次性编码 SelectResponse。
    pub fn HandleRequest(
        &mut self,
        context: B::Context,
        request: &CoprocessorRequest<B::Range>,
    ) -> CoprocessorResponse {
        let context = copHandlerCtx(context, request);
        let mut executor = match self.buildDAGExecutor(&context, request) {
            Ok(executor) => executor,
            Err(error) => return self.buildErrorResponse(&error),
        };
        if let Err(error) = executor.open(&context) {
            return self.buildErrorResponse(&error);
        }

        let mut chunk = executor.new_cache_chunk();
        let field_types = executor.return_field_types();
        // 循环拉取直至空 chunk，累计 tipb 块并记账语句内存
        let mut total_chunks = Vec::new();
        loop {
            chunk.reset();
            if let Err(error) = executor.next(&context, &mut chunk) {
                return self.buildErrorResponse(&error);
            }
            if chunk.num_rows() == 0 {
                break;
            }
            let part_chunks = match self.buildChunk(&chunk, &field_types) {
                Ok(chunks) => chunks,
                Err(error) => return self.buildErrorResponse(&error),
            };
            for part in &part_chunks {
                let size = self.sctx.serialized_chunk_size(part);
                self.sctx.consume_statement_memory(size as i64);
            }
            total_chunks.extend(part_chunks);
        }
        if let Err(error) = executor.close() {
            return self.buildErrorResponse(&error);
        }
        self.buildUnaryResponse(total_chunks)
    }

    /// Stream 模式：每批 chunk 编码后立即通过 `stream` 发送。
    pub fn HandleStreamRequest<S>(
        &mut self,
        context: B::Context,
        request: &CoprocessorRequest<B::Range>,
        stream: &mut S,
    ) -> Result<(), B::Error>
    where
        S: CoprocessorStream<B::Error>,
    {
        let context = copHandlerCtx(context, request);
        self.sctx
            .debug(&context, "handle coprocessor stream request");
        let mut executor = match self.buildDAGExecutor(&context, request) {
            Ok(executor) => executor,
            Err(error) => return stream.send(self.buildErrorResponse(&error)),
        };
        if let Err(error) = executor.open(&context) {
            return stream.send(self.buildErrorResponse(&error));
        }

        let mut chunk = executor.new_cache_chunk();
        let field_types = executor.return_field_types();
        loop {
            chunk.reset();
            if let Err(error) = executor.next(&context, &mut chunk) {
                return stream.send(self.buildErrorResponse(&error));
            }
            if chunk.num_rows() == 0 {
                return self.buildResponseAndSendToStream(&chunk, &field_types, stream);
            }
            if let Err(error) = self.buildResponseAndSendToStream(&chunk, &field_types, stream) {
                return stream.send(self.buildErrorResponse(&error));
            }
        }
    }

    /// 将当前 chunk 编码为 tipb 帧并写入流。
    fn buildResponseAndSendToStream<S>(
        &self,
        chunk: &B::Chunk,
        field_types: &[B::FieldType],
        stream: &mut S,
    ) -> Result<(), B::Error>
    where
        S: CoprocessorStream<B::Error>,
    {
        let chunks = match self.buildChunk(chunk, field_types) {
            Ok(chunks) => chunks,
            Err(error) => return stream.send(self.buildErrorResponse(&error)),
        };
        for chunk in &chunks {
            stream.send(self.buildStreamResponse(chunk))?;
        }
        Ok(())
    }

    /// 校验请求类型、解码 DAG、鉴权、设置时区并构建物理执行器。
    fn buildDAGExecutor(
        &mut self,
        context: &B::Context,
        request: &CoprocessorRequest<B::Range>,
    ) -> Result<B::Executor, B::Error> {
        if request.request_type != REQ_TYPE_DAG {
            return Err(self
                .sctx
                .error(format!("unsupported request type {}", request.request_type)));
        }
        let dag_request = self.sctx.decode_dag_request(&request.data)?;

        // 若开启权限管理，则按请求用户匹配身份并设置认证角色
        if let Some(user) = dag_request.user.as_ref()
            && self.sctx.privilege_manager_available()
        {
            self.sctx.set_request_user(&user.user_name, &user.user_host);
            if let Some((auth_name, auth_host)) =
                self.sctx
                    .match_identity(context, &user.user_name, &user.user_host)
                && self.sctx.auth_without_verification(&auth_name, &auth_host)
            {
                let roles = self.sctx.default_roles(context, &auth_name, &auth_host);
                self.sctx
                    .set_authenticated_user(auth_name, auth_host, roles);
            }
        }

        let time_zone = self
            .sctx
            .construct_time_zone(&dag_request.time_zone_name, dag_request.time_zone_offset)?;
        self.sctx
            .set_time_zone_and_statement_flags(time_zone, dag_request.flags);

        self.dagReq = Some(dag_request.clone());

        let plan = self
            .sctx
            .build_physical_plan(&request.ranges, &dag_request.executors)?;
        let plan = self.sctx.inject_extra_projection(plan);
        let executor = self.sctx.build_executor(context, plan)?;
        Ok(executor)
    }

    /// 按 DAG 声明的 `encode_type` 将内部 chunk 转为 tipb Chunk 列表。
    fn buildChunk(
        &self,
        chunk: &B::Chunk,
        field_types: &[B::FieldType],
    ) -> Result<Vec<TipbChunk>, B::Error> {
        let dag_request = self
            .dagReq
            .as_ref()
            .expect("DAG request must be built first");
        match dag_request.encode_type {
            EncodeType::TypeDefault => self.encodeDefault(chunk, field_types),
            EncodeType::TypeChunk => Ok(self.encodeChunk(chunk, field_types)),
            EncodeType::Unknown(value) => {
                Err(self.sctx.error(format!("unknown DAG encode type: {value}")))
            }
        }
    }

    /// 组装 unary SelectResponse 并 marshal；失败时返回错误响应。
    fn buildUnaryResponse(&self, chunks: Vec<TipbChunk>) -> CoprocessorResponse {
        let dag_request = self
            .dagReq
            .as_ref()
            .expect("DAG request must be built first");
        let execution_summaries = if dag_request.collect_execution_summaries {
            vec![ExecutorExecutionSummary; dag_request.executors.len()]
        } else {
            Vec::new()
        };
        let select_response = SelectResponse {
            chunks,
            encode_type: dag_request.encode_type,
            execution_summaries,
        };
        match self.sctx.marshal_select_response(&select_response) {
            Ok(data) => CoprocessorResponse {
                data,
                other_error: String::new(),
            },
            Err(error) => self.buildErrorResponse(&error),
        }
    }

    /// 将单个 tipb Chunk 封入 StreamResponse。
    fn buildStreamResponse(&self, chunk: &TipbChunk) -> CoprocessorResponse {
        let chunk_data = match self.sctx.marshal_chunk(chunk) {
            Ok(data) => data,
            Err(error) => return self.buildErrorResponse(&error),
        };
        let stream_response = StreamResponse { data: chunk_data };
        match self.sctx.marshal_stream_response(&stream_response) {
            Ok(data) => CoprocessorResponse {
                data,
                other_error: String::new(),
            },
            Err(error) => CoprocessorResponse {
                data: Vec::new(),
                other_error: error.to_string(),
            },
        }
    }

    /// 将错误栈写入 `other_error` 字段。
    fn buildErrorResponse(&self, error: &B::Error) -> CoprocessorResponse {
        CoprocessorResponse {
            data: Vec::new(),
            other_error: self.sctx.error_stack(error),
        }
    }

    /// Chunk 编码：按 `output_offsets` 选取列后一次性编码。
    fn encodeChunk(&self, chunk: &B::Chunk, column_types: &[B::FieldType]) -> Vec<TipbChunk> {
        let dag_request = self
            .dagReq
            .as_ref()
            .expect("DAG request must be built first");
        let response_column_types: Vec<B::FieldType> = dag_request
            .output_offsets
            .iter()
            .map(|ordinal| column_types[*ordinal as usize].clone())
            .collect();
        vec![TipbChunk {
            rows_data: self.sctx.encode_chunk(chunk, &response_column_types),
        }]
    }

    /// Default 编码：逐行序列化 datum，并按 `rowsPerChunk` 切分 tipb Chunk。
    fn encodeDefault(
        &self,
        chunk: &B::Chunk,
        field_types: &[B::FieldType],
    ) -> Result<Vec<TipbChunk>, B::Error> {
        let dag_request = self
            .dagReq
            .as_ref()
            .expect("DAG request must be built first");
        let mut chunks = Vec::new();
        for row_index in 0..chunk.num_rows() {
            let mut requested_row = Vec::new();
            for ordinal in &dag_request.output_offsets {
                let ordinal = *ordinal as usize;
                requested_row.extend(
                    self.sctx
                        .encode_value(chunk.datum(row_index, ordinal), &field_types[ordinal])?,
                );
            }
            chunks = self.appendRow(chunks, &requested_row, row_index);
        }
        Ok(chunks)
    }

    /// 将一行字节追加到 tipb Chunk 列表；行号对齐时新建块。
    fn appendRow(
        &self,
        mut chunks: Vec<TipbChunk>,
        data: &[u8],
        row_count: usize,
    ) -> Vec<TipbChunk> {
        if row_count.is_multiple_of(rowsPerChunk) {
            chunks.push(TipbChunk::default());
        }
        chunks
            .last_mut()
            .expect("the first row creates a response chunk")
            .rows_data
            .extend_from_slice(data);
        chunks
    }
}
