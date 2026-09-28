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

// mock Coprocessor DAG（有向无环图执行计划）请求处理。
//
// 将 DAG 中的 TableScan/IndexScan/Selection/Agg/TopN/Limit 等算子串成执行器树，
// 驱动 `Next` 收集结果行并按 Default/Chunk 编码写入响应。

use crate::aggregate::{hashAggExec, streamAggExec};
use crate::copr_handler::{
    BatchResponse, Chunk, CopError, DagRequest, Datum, EncodeType, ExecDetail, ExecutorSpec, Expr,
    KeyRange, Request, RequestPayload, Response, Row, coprHandler,
};
use crate::executor::{executor, indexScanExec, limitExec, selectionExec, tableScanExec, topNExec};

/// 空字节切片占位，编码行数据时作为初始缓冲。
pub static dummySlice: &[u8] = &[];
/// 每个 Chunk 最多容纳的行数。
pub const rowsPerChunk: usize = 64;

/// DAG 执行上下文：请求、键范围、start_ts（MVCC 读时间戳）与求值列信息。
pub struct dagContext {
    pub dagReq: DagRequest,
    pub keyRanges: Vec<KeyRange>,
    pub startTS: u64,
    pub evalCtx: evalContext,
}

/// 列字段类型描述（类型码、标志、长度、精度、字符集与校对规则）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FieldType {
    pub type_code: u8,
    pub flag: u64,
    pub flen: i32,
    pub decimal: i32,
    pub charset: String,
    pub collate: i32,
}

/// 列元信息：列 ID、类型、是否主键句柄及默认值。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnInfo {
    pub column_id: i64,
    pub field_type: FieldType,
    pub pk_handle: bool,
    pub default_value: Option<Datum>,
}

/// 表达式求值上下文：列定义、字段类型与列 ID 列表。
#[derive(Clone, Debug, Default)]
pub struct evalContext {
    pub columns: Vec<ColumnInfo>,
    pub fieldTps: Vec<FieldType>,
    pub columnInfos: Vec<i64>,
}

impl evalContext {
    /// 用列信息刷新 fieldTps 与 columnInfos。
    pub fn setColumnInfo(&mut self, columns: &[ColumnInfo]) {
        self.columns = columns.to_vec();
        self.fieldTps = columns.iter().map(fieldTypeFromPBColumn).collect();
        self.columnInfos = columns.iter().map(|column| column.column_id).collect();
    }

    /// 按偏移把 `values` 中相关列写入输出行缓冲。
    pub fn decodeRelatedColumnVals(
        &self,
        offsets: &[usize],
        values: &[Datum],
        row: &mut [Datum],
    ) -> Result<(), CopError> {
        for offset in offsets {
            let value = values
                .get(*offset)
                .cloned()
                .ok_or(CopError::ColumnOffset(*offset))?;
            let target = row
                .get_mut(*offset)
                .ok_or(CopError::ColumnOffset(*offset))?;
            *target = value;
        }
        Ok(())
    }
}

impl mockClientStream {
    /// 模拟 gRPC Header；mock 恒成功。
    pub fn Header(&self) -> Result<(), CopError> {
        Ok(())
    }
    /// 模拟 gRPC Trailer；无操作。
    pub fn Trailer(&self) {}
    /// 模拟关闭发送端。
    pub fn CloseSend(&self) -> Result<(), CopError> {
        Ok(())
    }
    /// 返回默认会话求值上下文。
    pub fn Context(&self) -> EvalSessionContext {
        EvalSessionContext::default()
    }
    /// 模拟发送消息；忽略载荷。
    pub fn SendMsg<T: ?Sized>(&self, _message: &T) -> Result<(), CopError> {
        Ok(())
    }
    /// 模拟接收消息；忽略载荷。
    pub fn RecvMsg<T: ?Sized>(&self, _message: &mut T) -> Result<(), CopError> {
        Ok(())
    }
}

/// 由 SQL mode flags 与时区偏移构造会话求值上下文。
pub fn flagsAndTzToSessionContext(flags: u64, timezone_offset: i64) -> EvalSessionContext {
    EvalSessionContext {
        flags,
        timezone_offset,
    }
}

/// 表达式求值会话上下文：SQL mode 标志与时区偏移秒。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EvalSessionContext {
    pub flags: u64,
    pub timezone_offset: i64,
}

impl coprHandler {
    /// 构建并驱动 DAG 执行器树，编码 Select 响应或错误。
    pub fn handleCopDAGRequest(&self, request: &Request) -> Response {
        let (mut dag_context, mut execution, dag_request) = match self.buildDAGExecutor(request) {
            Ok(result) => result,
            Err(error) => return buildResp(None, Vec::new(), Some(error)),
        };
        let mut rows = Vec::new();
        // 排空执行器，收集全部结果行或首个错误。
        let error = loop {
            match execution.Next() {
                Ok(Some(row)) => rows.push(row),
                Ok(None) => break None,
                Err(error) => break Some(error),
            }
        };
        let counts = execution.Counts();
        let details = if dag_request.collect_execution_summaries {
            execution.ExecDetails()
        } else {
            Vec::new()
        };
        let mut response = self.initSelectResponse(error.clone(), counts);
        if error.is_none() {
            if let Err(error) =
                self.fillUpData4SelectResponse(&mut response, &dag_request, &mut dag_context, &rows)
            {
                return buildResp(Some(response), details, Some(error));
            }
        }
        buildResp(Some(response), details, error)
    }

    /// 校验请求载荷并构建 DAG 上下文与根执行器。
    pub fn buildDAGExecutor(
        &self,
        request: &Request,
    ) -> Result<(dagContext, Box<dyn executor>, DagRequest), CopError> {
        if let Some(error) = &self.region_error {
            return Err(error.clone());
        }
        if request.ranges.is_empty() {
            return Err(CopError::InvalidRequest("request range is null".into()));
        }
        let RequestPayload::Dag(dag_request) = &request.payload else {
            return Err(CopError::InvalidRequest(
                "request is not a DAG request".into(),
            ));
        };
        if dag_request.executors.is_empty() {
            return Err(CopError::InvalidRequest(
                "DAG request has no executors".into(),
            ));
        }
        let mut context = dagContext {
            dagReq: dag_request.clone(),
            keyRanges: request.ranges.clone(),
            startTS: request.start_ts,
            evalCtx: evalContext::default(),
        };
        let execution = self.buildDAG(&mut context, &dag_request.executors)?;
        Ok((context, execution, dag_request.clone()))
    }

    /// 按规范列表自底向上串接执行器（叶子为 Scan）。
    pub fn buildDAG(
        &self,
        context: &mut dagContext,
        specifications: &[ExecutorSpec],
    ) -> Result<Box<dyn executor>, CopError> {
        let mut source = None;
        for specification in specifications {
            source = Some(self.buildExec(context, specification, source)?);
        }
        source.ok_or_else(|| CopError::InvalidRequest("DAG executor list is empty".into()))
    }

    /// TiFlash 路径复用同一套 DAG 构建逻辑。
    pub fn buildDAGForTiFlash(
        &self,
        context: &mut dagContext,
        specifications: &[ExecutorSpec],
    ) -> Result<Box<dyn executor>, CopError> {
        self.buildDAG(context, specifications)
    }

    /// 根据算子规范构造单个执行器，并可选挂接上游 `source`。
    pub fn buildExec(
        &self,
        context: &dagContext,
        specification: &ExecutorSpec,
        source: Option<Box<dyn executor>>,
    ) -> Result<Box<dyn executor>, CopError> {
        let mut execution: Box<dyn executor> = match specification {
            ExecutorSpec::TableScan { descending } => {
                if source.is_some() {
                    return Err(CopError::InvalidRequest(
                        "table scan must be the DAG leaf".into(),
                    ));
                }
                Box::new(self.buildTableScan(context, *descending))
            }
            ExecutorSpec::IndexScan { descending, unique } => {
                if source.is_some() {
                    return Err(CopError::InvalidRequest(
                        "index scan must be the DAG leaf".into(),
                    ));
                }
                Box::new(self.buildIndexScan(context, *descending, *unique))
            }
            ExecutorSpec::Selection { conditions } => {
                Box::new(self.buildSelection(conditions.clone()))
            }
            ExecutorSpec::HashAgg {
                aggregates,
                group_by,
            } => Box::new(self.buildHashAgg(aggregates.clone(), group_by.clone())),
            ExecutorSpec::StreamAgg {
                aggregates,
                group_by,
            } => Box::new(self.buildStreamAgg(aggregates.clone(), group_by.clone())),
            ExecutorSpec::TopN { order_by, limit } => {
                Box::new(self.buildTopN(order_by.clone(), *limit))
            }
            ExecutorSpec::Limit { limit } => Box::new(limitExec::new(*limit)),
        };
        if source.is_some() {
            execution.SetSrcExec(source);
        }
        Ok(execution)
    }

    /// 构造表扫描叶子：按 keyRanges 与 startTS 读行。
    pub fn buildTableScan(&self, context: &dagContext, descending: bool) -> tableScanExec {
        tableScanExec::new(
            self.reader.clone(),
            context.keyRanges.clone(),
            context.startTS,
            descending,
        )
    }
    /// 构造索引扫描叶子；`unique` 表示唯一索引扫描语义。
    pub fn buildIndexScan(
        &self,
        context: &dagContext,
        descending: bool,
        unique: bool,
    ) -> indexScanExec {
        indexScanExec::new(
            self.reader.clone(),
            context.keyRanges.clone(),
            context.startTS,
            descending,
            unique,
        )
    }
    /// 构造过滤（Selection）算子。
    pub fn buildSelection(&self, conditions: Vec<Expr>) -> selectionExec {
        selectionExec::new(conditions)
    }
    /// 透传聚合与分组表达式（Go 侧会做额外解析，此处保持形状）。
    pub fn getAggInfo(
        &self,
        aggregates: &[crate::copr_handler::AggCall],
        group_by: &[Expr],
    ) -> (Vec<crate::copr_handler::AggCall>, Vec<Expr>) {
        (aggregates.to_vec(), group_by.to_vec())
    }
    /// 构造 Hash 聚合算子。
    pub fn buildHashAgg(
        &self,
        aggregates: Vec<crate::copr_handler::AggCall>,
        group_by: Vec<Expr>,
    ) -> hashAggExec {
        hashAggExec::new(aggregates, group_by)
    }
    /// 构造流式聚合算子。
    pub fn buildStreamAgg(
        &self,
        aggregates: Vec<crate::copr_handler::AggCall>,
        group_by: Vec<Expr>,
    ) -> streamAggExec {
        streamAggExec::new(aggregates, group_by)
    }
    /// 构造 TopN（排序取前 N）算子。
    pub fn buildTopN(&self, order_by: Vec<crate::copr_handler::ByItem>, limit: usize) -> topNExec {
        topNExec::new(order_by, limit)
    }

    /// 按扫描方向调整键范围顺序；降序时反转区间列表。
    pub fn extractKVRanges(&self, ranges: &[KeyRange], descending: bool) -> Vec<KeyRange> {
        let mut ranges = ranges.to_vec();
        if descending {
            reverseKVRanges(&mut ranges);
        }
        ranges
    }

    /// 初始化 Select 响应骨架，填入扫描计数与可选错误。
    pub fn initSelectResponse(&self, error: Option<CopError>, counts: Vec<i64>) -> Response {
        let mut response = Response {
            counts,
            ..Response::default()
        };
        if let Some(error) = error {
            response.other_error = Some(error.to_string());
        }
        response
    }

    /// 按编码类型把结果行写入响应（Default 或 Chunk）。
    pub fn fillUpData4SelectResponse(
        &self,
        response: &mut Response,
        request: &DagRequest,
        _context: &mut dagContext,
        rows: &[Row],
    ) -> Result<(), CopError> {
        match request.encode_type {
            EncodeType::Default => self.encodeDefault(response, rows, &request.output_offsets),
            EncodeType::Chunk => self.encodeChunk(response, rows, &request.output_offsets),
        }
        Ok(())
    }

    /// 从求值上下文取出响应 schema 的字段类型列表。
    pub fn constructRespSchema(&self, context: &dagContext) -> Vec<FieldType> {
        context.evalCtx.fieldTps.clone()
    }

    /// Default 编码：按 output_offsets 选取列，写入 chunks。
    pub fn encodeDefault(&self, response: &mut Response, rows: &[Row], offsets: &[usize]) {
        for row in rows {
            let mut encoded = dummySlice.to_vec();
            let mut selected = Vec::new();
            for offset in offsets {
                if let Some(value) = row.get(*offset) {
                    value.encode(&mut encoded);
                    selected.push(value.clone());
                }
            }
            response.chunks = appendRow(std::mem::take(&mut response.chunks), encoded, selected);
        }
    }

    /// Chunk 编码路径目前复用 Default 实现。
    pub fn encodeChunk(&self, response: &mut Response, rows: &[Row], offsets: &[usize]) {
        self.encodeDefault(response, rows, offsets);
    }
}

/// 组装最终 Response：挂上执行明细，并按错误种类填入 locked/region/other 字段。
pub fn buildResp(
    response: Option<Response>,
    details: Vec<ExecDetail>,
    error: Option<CopError>,
) -> Response {
    let mut response = response.unwrap_or_default();
    response.execution_summaries = details;
    if let Some(error) = error {
        match error {
            CopError::Locked { .. } => response.locked = Some(error),
            CopError::Region(_) => response.region_error = Some(error),
            _ => response.other_error = Some(error.to_string()),
        }
    }
    response
}

/// 将 CopError 转为简易 PB 错误结构（固定 code=1）。
pub fn toPBError(error: Option<&CopError>) -> Option<PbError> {
    error.map(|error| PbError {
        code: 1,
        message: error.to_string(),
    })
}

/// Protobuf 风格错误载荷（code + message）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PbError {
    pub code: i32,
    pub message: String,
}

/// 原地反转键范围列表（用于降序扫描）。
pub fn reverseKVRanges(ranges: &mut [KeyRange]) {
    ranges.reverse();
}

/// 向 chunks 追加一行；当前块满 `rowsPerChunk` 时开新块。
pub fn appendRow(mut chunks: Vec<Chunk>, data: Vec<u8>, row: Row) -> Vec<Chunk> {
    if chunks
        .last()
        .is_none_or(|chunk| chunk.rows.len() >= rowsPerChunk)
    {
        chunks.push(Chunk::default());
    }
    let chunk = chunks.last_mut().expect("chunk was just inserted");
    chunk.rows_data.extend_from_slice(&data);
    chunk.rows.push(row);
    chunks
}

/// 取请求范围起点与 Region 起点的较大者（半开区间裁剪）。
pub fn maxStartKey(range_start: Vec<u8>, region_start: Vec<u8>) -> Vec<u8> {
    std::cmp::max(range_start, region_start)
}

/// 取请求范围终点与 Region 终点的较小者；空终点表示正无穷。
pub fn minEndKey(range_end: Vec<u8>, region_end: Vec<u8>) -> Vec<u8> {
    if range_end.is_empty() {
        return region_end;
    }
    if region_end.is_empty() {
        return range_end;
    }
    std::cmp::min(range_end, region_end)
}

/// 判断偏移是否已出现在收集列表中。
pub fn isDuplicated(offsets: &[usize], offset: usize) -> bool {
    offsets.contains(&offset)
}

/// 递归收集表达式中引用的列偏移（去重）。
pub fn extractOffsetsInExpr(expression: &Expr, collector: &mut Vec<usize>) {
    match expression {
        Expr::Column(offset) => {
            if !collector.contains(offset) {
                collector.push(*offset);
            }
        }
        Expr::Constant(_) => {}
        Expr::Not(expression) | Expr::IsNull(expression) => {
            extractOffsetsInExpr(expression, collector)
        }
        Expr::Eq(left, right)
        | Expr::Ne(left, right)
        | Expr::Lt(left, right)
        | Expr::Le(left, right)
        | Expr::Gt(left, right)
        | Expr::Ge(left, right)
        | Expr::Add(left, right) => {
            extractOffsetsInExpr(left, collector);
            extractOffsetsInExpr(right, collector);
        }
        Expr::And(expressions) | Expr::Or(expressions) => {
            for expression in expressions {
                extractOffsetsInExpr(expression, collector);
            }
        }
    }
}

/// 从列元信息提取 FieldType。
pub fn fieldTypeFromPBColumn(column: &ColumnInfo) -> FieldType {
    column.field_type.clone()
}

/// 模拟 gRPC 客户端流（空实现，供接口形状对齐）。
#[derive(Default)]
pub struct mockClientStream;
/// 构造默认的 mock gRPC 客户端流。
pub fn MockGRPCClientStream() -> mockClientStream {
    mockClientStream
}

/// 模拟批处理 Coprocessor 错误客户端：Recv 返回带错误的空响应。
pub struct mockBathCopErrClient {
    pub Error: CopError,
}
impl mockBathCopErrClient {
    /// 每次都返回含 `Error` 的 BatchResponse，与 Go mock 保持一致。
    pub fn Recv(&mut self) -> Result<BatchResponse, CopError> {
        Ok(BatchResponse {
            responses: Vec::new(),
            other_error: Some(self.Error.to_string()),
        })
    }
}
