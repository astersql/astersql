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

// mock Coprocessor 处理器核心类型与请求分发。
//
// 定义 Datum/Expr/DAG/ANALYZE 等协议结构、`KvReader` 扫描接口，以及 `coprHandler`
// 将 Batch/单请求路由到 DAG、ANALYZE、Checksum 处理路径。

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// Coprocessor 错误：Region 错误、锁冲突、非法请求、类型/编解码问题等。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CopError {
    Region(String),
    Locked {
        key: Vec<u8>,
        primary: Vec<u8>,
        start_ts: u64,
        ttl: u64,
    },
    InvalidRequest(String),
    ColumnOffset(usize),
    Type(String),
    Codec(String),
    EndOfStream,
}

impl fmt::Display for CopError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Region(message)
            | Self::InvalidRequest(message)
            | Self::Type(message)
            | Self::Codec(message) => formatter.write_str(message),
            Self::Locked { key, .. } => write!(formatter, "key is locked: {key:?}"),
            Self::ColumnOffset(offset) => {
                write!(formatter, "column offset {offset} is out of range")
            }
            Self::EndOfStream => formatter.write_str("end of stream"),
        }
    }
}

impl std::error::Error for CopError {}

/// 简化版 SQL 值类型：NULL、整数、无符号整数、浮点与字节串。
#[derive(Clone, Debug)]
pub enum Datum {
    Null,
    Int(i64),
    Uint(u64),
    Real(f64),
    Bytes(Vec<u8>),
}

impl Default for Datum {
    fn default() -> Self {
        Self::Null
    }
}

impl PartialEq for Datum {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Datum {}
impl PartialOrd for Datum {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Datum {
    fn cmp(&self, other: &Self) -> Ordering {
        use Datum::*;
        // 跨数值类型按数值大小比较；不同类型族按 tag 排序；NULL 最小。
        match (self, other) {
            (Null, Null) => Ordering::Equal,
            (Null, _) => Ordering::Less,
            (_, Null) => Ordering::Greater,
            (Int(left), Int(right)) => left.cmp(right),
            (Uint(left), Uint(right)) => left.cmp(right),
            (Int(left), Uint(right)) => {
                if *left < 0 {
                    Ordering::Less
                } else {
                    (*left as u64).cmp(right)
                }
            }
            (Uint(left), Int(right)) => {
                if *right < 0 {
                    Ordering::Greater
                } else {
                    left.cmp(&(*right as u64))
                }
            }
            (Real(left), Real(right)) => left.total_cmp(right),
            (Int(left), Real(right)) => (*left as f64).total_cmp(right),
            (Real(left), Int(right)) => left.total_cmp(&(*right as f64)),
            (Uint(left), Real(right)) => (*left as f64).total_cmp(right),
            (Real(left), Uint(right)) => left.total_cmp(&(*right as f64)),
            (Bytes(left), Bytes(right)) => left.cmp(right),
            (left, right) => datum_tag(left).cmp(&datum_tag(right)),
        }
    }
}

/// 比较用类型族标签：NULL < 数值 < 字节串。
fn datum_tag(value: &Datum) -> u8 {
    match value {
        Datum::Null => 0,
        Datum::Int(_) | Datum::Uint(_) | Datum::Real(_) => 1,
        Datum::Bytes(_) => 2,
    }
}

impl Datum {
    /// SQL 真值判定：NULL/0/空/"0" 为假，其余为真。
    pub fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Int(value) => *value != 0,
            Self::Uint(value) => *value != 0,
            Self::Real(value) => *value != 0.0,
            Self::Bytes(value) => !value.is_empty() && value != b"0",
        }
    }

    /// 以类型标签 + 大端载荷编码到输出缓冲。
    pub fn encode(&self, output: &mut Vec<u8>) {
        match self {
            Self::Null => output.push(0),
            Self::Int(value) => {
                output.push(1);
                output.extend_from_slice(&value.to_be_bytes());
            }
            Self::Uint(value) => {
                output.push(2);
                output.extend_from_slice(&value.to_be_bytes());
            }
            Self::Real(value) => {
                output.push(3);
                output.extend_from_slice(&value.to_bits().to_be_bytes());
            }
            Self::Bytes(value) => {
                output.push(4);
                output.extend_from_slice(&(value.len() as u32).to_be_bytes());
                output.extend_from_slice(value);
            }
        }
    }
}

/// 一行数据：按列偏移排列的 Datum 向量。
pub type Row = Vec<Datum>;

/// 半开键区间 `[start, end)`；空 end 表示正无穷。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyRange {
    pub start: Vec<u8>,
    pub end: Vec<u8>,
}

impl KeyRange {
    /// 判断是否为单点区间：`end == next_key(start)`。
    pub fn is_point(&self) -> bool {
        !self.start.is_empty() && next_key(&self.start) == self.end
    }
}

/// 返回键的后继（末尾追加 0 字节），用于构造点查区间。
pub fn next_key(key: &[u8]) -> Vec<u8> {
    let mut result = key.to_vec();
    result.push(0);
    result
}

/// KV 扫描结果对：键、行值与提交时间戳。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KvPair {
    pub key: Vec<u8>,
    pub value: Row,
    pub commit_ts: u64,
}

/// KV 读取抽象：按范围扫描；默认实现还可计算校验和。
pub trait KvReader: Send + Sync {
    fn scan(
        &self,
        ranges: &[KeyRange],
        start_ts: u64,
        descending: bool,
    ) -> Result<Vec<KvPair>, CopError>;

    /// 对可见版本做 FNV 校验，返回 (checksum, 行数, 总字节数)。
    fn checksum(&self, ranges: &[KeyRange], start_ts: u64) -> Result<(u64, u64, u64), CopError> {
        let pairs = self.scan(ranges, start_ts, false)?;
        let mut checksum = 0;
        let mut total_bytes = 0;
        for pair in &pairs {
            let mut row = Vec::new();
            for datum in &pair.value {
                datum.encode(&mut row);
            }
            total_bytes += pair.key.len() as u64 + row.len() as u64;
            checksum ^= fnv64(&pair.key) ^ fnv64(&row);
        }
        Ok((checksum, pairs.len() as u64, total_bytes))
    }
}

/// 内存版 KvReader：BTreeMap 存键 → (行, commit_ts)，按 start_ts 过滤可见版本。
#[derive(Clone, Debug, Default)]
pub struct MemoryReader {
    pub rows: BTreeMap<Vec<u8>, (Row, u64)>,
}

impl KvReader for MemoryReader {
    fn scan(
        &self,
        ranges: &[KeyRange],
        start_ts: u64,
        descending: bool,
    ) -> Result<Vec<KvPair>, CopError> {
        let mut result = Vec::new();
        for range in ranges {
            if !range.end.is_empty() && range.start >= range.end {
                return Err(CopError::InvalidRequest(
                    "invalid range, start should be smaller than end".into(),
                ));
            }
        }
        let mut ordered_ranges: Vec<_> = ranges.iter().collect();
        if descending {
            ordered_ranges.reverse();
        }
        for range in ordered_ranges {
            // 仅返回 commit_ts <= start_ts 的版本（简化 MVCC 可见性）。
            if descending {
                let mut pairs: Vec<_> = if range.end.is_empty() {
                    self.rows.range(range.start.clone()..).collect()
                } else {
                    self.rows
                        .range(range.start.clone()..range.end.clone())
                        .collect()
                };
                pairs.reverse();
                for (key, (value, commit_ts)) in pairs {
                    if *commit_ts <= start_ts {
                        result.push(KvPair {
                            key: key.clone(),
                            value: value.clone(),
                            commit_ts: *commit_ts,
                        });
                    }
                }
            } else {
                let pairs: Vec<_> = if range.end.is_empty() {
                    self.rows.range(range.start.clone()..).collect()
                } else {
                    self.rows
                        .range(range.start.clone()..range.end.clone())
                        .collect()
                };
                for (key, (value, commit_ts)) in pairs {
                    if *commit_ts <= start_ts {
                        result.push(KvPair {
                            key: key.clone(),
                            value: value.clone(),
                            commit_ts: *commit_ts,
                        });
                    }
                }
            }
        }
        Ok(result)
    }
}

/// FNV-1a 64 位哈希，用于 mock 校验和。
fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

/// 表达式树：列引用、常量与常见比较/逻辑/算术算子。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expr {
    Column(usize),
    Constant(Datum),
    Eq(Box<Expr>, Box<Expr>),
    Ne(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
    Le(Box<Expr>, Box<Expr>),
    Gt(Box<Expr>, Box<Expr>),
    Ge(Box<Expr>, Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    IsNull(Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// 在给定行上求值表达式。
    pub fn eval(&self, row: &[Datum]) -> Result<Datum, CopError> {
        use Expr::*;
        match self {
            Column(offset) => row
                .get(*offset)
                .cloned()
                .ok_or(CopError::ColumnOffset(*offset)),
            Constant(value) => Ok(value.clone()),
            Eq(left, right) => Ok(compare_datums(
                left.eval(row)?,
                right.eval(row)?,
                |ordering| ordering == Ordering::Equal,
            )),
            Ne(left, right) => Ok(compare_datums(
                left.eval(row)?,
                right.eval(row)?,
                |ordering| ordering != Ordering::Equal,
            )),
            Lt(left, right) => Ok(compare_datums(
                left.eval(row)?,
                right.eval(row)?,
                |ordering| ordering == Ordering::Less,
            )),
            Le(left, right) => Ok(compare_datums(
                left.eval(row)?,
                right.eval(row)?,
                |ordering| ordering != Ordering::Greater,
            )),
            Gt(left, right) => Ok(compare_datums(
                left.eval(row)?,
                right.eval(row)?,
                |ordering| ordering == Ordering::Greater,
            )),
            Ge(left, right) => Ok(compare_datums(
                left.eval(row)?,
                right.eval(row)?,
                |ordering| ordering != Ordering::Less,
            )),
            And(items) => {
                let mut saw_null = false;
                for item in items {
                    match item.eval(row)? {
                        Datum::Null => saw_null = true,
                        value if !value.truthy() => return Ok(Datum::Int(0)),
                        _ => {}
                    }
                }
                Ok(if saw_null { Datum::Null } else { Datum::Int(1) })
            }
            Or(items) => {
                let mut saw_null = false;
                for item in items {
                    match item.eval(row)? {
                        Datum::Null => saw_null = true,
                        value if value.truthy() => return Ok(Datum::Int(1)),
                        _ => {}
                    }
                }
                Ok(if saw_null { Datum::Null } else { Datum::Int(0) })
            }
            Not(item) => match item.eval(row)? {
                Datum::Null => Ok(Datum::Null),
                value => Ok(Datum::Int(i64::from(!value.truthy()))),
            },
            IsNull(item) => Ok(Datum::Int(i64::from(matches!(
                item.eval(row)?,
                Datum::Null
            )))),
            Add(left, right) => add_datums(left.eval(row)?, right.eval(row)?),
        }
    }
}

/// SQL comparisons return NULL when either operand is NULL.
fn compare_datums(left: Datum, right: Datum, predicate: impl FnOnce(Ordering) -> bool) -> Datum {
    if matches!(&left, Datum::Null) || matches!(&right, Datum::Null) {
        Datum::Null
    } else {
        Datum::Int(i64::from(predicate(left.cmp(&right))))
    }
}

/// 同类型数值相加；遇 NULL 传播 NULL。
fn add_datums(left: Datum, right: Datum) -> Result<Datum, CopError> {
    match (left, right) {
        (Datum::Null, _) | (_, Datum::Null) => Ok(Datum::Null),
        (Datum::Int(left), Datum::Int(right)) => Ok(Datum::Int(left.saturating_add(right))),
        (Datum::Uint(left), Datum::Uint(right)) => Ok(Datum::Uint(left.saturating_add(right))),
        (Datum::Real(left), Datum::Real(right)) => Ok(Datum::Real(left + right)),
        _ => Err(CopError::Type(
            "addition expects matching numeric values".into(),
        )),
    }
}

/// ORDER BY / TopN 单项：表达式与是否降序。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ByItem {
    pub expr: Expr,
    pub descending: bool,
}

/// 聚合函数种类。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AggKind {
    Count,
    Sum,
    Min,
    Max,
    First,
}

/// 聚合调用：种类与可选输入表达式（COUNT(*) 可无表达式）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AggCall {
    pub kind: AggKind,
    pub expr: Option<Expr>,
}

/// DAG 中单个算子规范（扫描、过滤、聚合、TopN、Limit）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutorSpec {
    TableScan {
        descending: bool,
    },
    IndexScan {
        descending: bool,
        unique: bool,
    },
    Selection {
        conditions: Vec<Expr>,
    },
    HashAgg {
        aggregates: Vec<AggCall>,
        group_by: Vec<Expr>,
    },
    StreamAgg {
        aggregates: Vec<AggCall>,
        group_by: Vec<Expr>,
    },
    TopN {
        order_by: Vec<ByItem>,
        limit: usize,
    },
    Limit {
        limit: usize,
    },
}

/// 结果行编码类型。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EncodeType {
    #[default]
    Default,
    Chunk,
}

/// DAG 请求：算子列表、输出列偏移、编码类型与是否收集执行摘要。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DagRequest {
    pub executors: Vec<ExecutorSpec>,
    pub output_offsets: Vec<usize>,
    pub encode_type: EncodeType,
    pub collect_execution_summaries: bool,
}

/// ANALYZE 目标：索引或列。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnalyzeType {
    Index,
    Columns,
}

/// ANALYZE 请求参数：类型、直方图桶数与采样大小。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyzeRequest {
    pub analyze_type: AnalyzeType,
    pub bucket_size: usize,
    pub sample_size: usize,
}

/// Coprocessor 请求载荷：DAG / ANALYZE / Checksum。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestPayload {
    Dag(DagRequest),
    Analyze(AnalyzeRequest),
    Checksum,
}

/// 单次 Coprocessor 请求：键范围、start_ts 与载荷。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    pub ranges: Vec<KeyRange>,
    pub start_ts: u64,
    pub payload: RequestPayload,
}

/// 算子执行明细：耗时、产出行数与迭代次数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecDetail {
    pub time_processed: Duration,
    pub produced_rows: u64,
    pub iterations: u64,
}

/// 编码后的行块：原始字节与结构化行。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Chunk {
    pub rows_data: Vec<u8>,
    pub rows: Vec<Row>,
}

/// Coprocessor 响应：数据、分块、错误与执行摘要。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Response {
    pub data: Vec<u8>,
    pub chunks: Vec<Chunk>,
    pub region_error: Option<CopError>,
    pub other_error: Option<String>,
    pub locked: Option<CopError>,
    pub counts: Vec<i64>,
    pub execution_summaries: Vec<ExecDetail>,
}

/// 批量 Coprocessor 请求。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BatchRequest {
    pub requests: Vec<Request>,
}

/// 批量 Coprocessor 响应。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BatchResponse {
    pub responses: Vec<Response>,
    pub other_error: Option<String>,
}

/// mock Coprocessor 处理器：持有 KvReader，可注入 Region 错误。
pub struct coprHandler {
    pub reader: Arc<dyn KvReader>,
    pub region_error: Option<CopError>,
}

impl coprHandler {
    /// 用给定读取器构造处理器。
    pub fn new(reader: Arc<dyn KvReader>) -> Self {
        Self {
            reader,
            region_error: None,
        }
    }

    /// 处理批量请求，返回可逐条 Recv 的 mock 流客户端。
    pub fn handleBatchCopRequest(
        &self,
        request: &BatchRequest,
    ) -> Result<mockBatchCopDataClient, CopError> {
        if let Some(error) = &self.region_error {
            return Err(error.clone());
        }
        let mut responses = Vec::with_capacity(request.requests.len());
        for request in &request.requests {
            let (_context, mut execution, dag_request) = self.buildDAGExecutor(request)?;
            let chunk = drainRowsFromExecutor(execution.as_mut(), &dag_request)?;
            responses.push(BatchResponse {
                responses: vec![Response {
                    chunks: vec![chunk],
                    ..Response::default()
                }],
                other_error: None,
            });
        }
        Ok(mockBatchCopDataClient {
            responses,
            offset: 0,
        })
    }

    /// 按载荷类型分发到 DAG / ANALYZE / Checksum 处理。
    pub fn handle_request(&self, request: &Request) -> BatchResponse {
        let response = match &request.payload {
            RequestPayload::Dag(_) => self.handleCopDAGRequest(request),
            RequestPayload::Analyze(_) => self.handleCopAnalyzeRequest(request),
            RequestPayload::Checksum => self.handleCopChecksumRequest(request),
        };
        BatchResponse {
            responses: vec![response],
            other_error: None,
        }
    }
}

/// 排空执行器，按 output_offsets 编码为单个 Chunk。
pub fn drainRowsFromExecutor(
    execution: &mut dyn crate::executor::executor,
    request: &DagRequest,
) -> Result<Chunk, CopError> {
    let mut chunk = Chunk::default();
    while let Some(row) = execution.Next()? {
        let mut selected = Vec::new();
        for offset in &request.output_offsets {
            let value = row
                .get(*offset)
                .cloned()
                .ok_or(CopError::ColumnOffset(*offset))?;
            value.encode(&mut chunk.rows_data);
            selected.push(value);
        }
        chunk.rows.push(selected);
    }
    Ok(chunk)
}

/// 批量 Cop 数据流客户端：按偏移依次吐出预计算的 BatchResponse。
pub struct mockBatchCopDataClient {
    pub responses: Vec<BatchResponse>,
    pub offset: usize,
}

impl mockBatchCopDataClient {
    /// 接收下一条批量响应；耗尽后返回 EndOfStream。
    pub fn Recv(&mut self) -> Result<BatchResponse, CopError> {
        let response = self
            .responses
            .get(self.offset)
            .cloned()
            .ok_or(CopError::EndOfStream)?;
        self.offset += 1;
        Ok(response)
    }
}
