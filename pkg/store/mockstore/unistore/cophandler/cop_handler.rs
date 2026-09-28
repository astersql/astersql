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

// Coprocessor 请求处理核心类型与入口（对应 Go cophandler）。
//
// 定义 Datum/表达式/执行计划树、KV 读取抽象、DAG/Analyze/Checksum 请求分派，
// 以及锁检查、Region 范围裁剪与响应组装。

use crate::analyze::{AnalyzeRequest, analyze};
use crate::mpp_exec::{ExecutionOutput, execute_executor};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

/// 每个响应 Chunk 默认行数上限。
pub const ROWS_PER_CHUNK: usize = 64;

/// 列值：NULL / 有符号整型 / 无符号 / 浮点 / 字节串。
#[derive(Clone, Debug)]
pub enum Datum {
    /// SQL NULL。
    Null,
    /// 有符号整数。
    Int(i64),
    /// 无符号整数。
    Uint(u64),
    /// 浮点（按 bit 比较）。
    Real(f64),
    /// 字节/字符串。
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
    /// 跨类型比较：NULL 最小；数值族互通；Bytes 按字典序；否则按类型标签。
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Null, Self::Null) => Ordering::Equal,
            (Self::Null, _) => Ordering::Less,
            (_, Self::Null) => Ordering::Greater,
            (Self::Int(a), Self::Int(b)) => a.cmp(b),
            (Self::Uint(a), Self::Uint(b)) => a.cmp(b),
            (Self::Int(a), Self::Uint(b)) => {
                if *a < 0 {
                    Ordering::Less
                } else {
                    (*a as u64).cmp(b)
                }
            }
            (Self::Uint(a), Self::Int(b)) => {
                if *b < 0 {
                    Ordering::Greater
                } else {
                    a.cmp(&(*b as u64))
                }
            }
            (Self::Real(a), Self::Real(b)) => a.total_cmp(b),
            (Self::Int(a), Self::Real(b)) => (*a as f64).total_cmp(b),
            (Self::Real(a), Self::Int(b)) => a.total_cmp(&(*b as f64)),
            (Self::Uint(a), Self::Real(b)) => (*a as f64).total_cmp(b),
            (Self::Real(a), Self::Uint(b)) => a.total_cmp(&(*b as f64)),
            (Self::Bytes(a), Self::Bytes(b)) => a.cmp(b),
            (a, b) => datum_tag(a).cmp(&datum_tag(b)),
        }
    }
}

/// 粗粒度类型标签，用于不可直接比较的类型排序。
fn datum_tag(value: &Datum) -> u8 {
    match value {
        Datum::Null => 0,
        Datum::Int(_) | Datum::Uint(_) | Datum::Real(_) => 1,
        Datum::Bytes(_) => 2,
    }
}

impl Datum {
    /// 逻辑真值：NULL/0/空/"0" 为假。
    pub fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Int(value) => *value != 0,
            Self::Uint(value) => *value != 0,
            Self::Real(value) => *value != 0.0,
            Self::Bytes(value) => !value.is_empty() && value != b"0",
        }
    }

    /// 将 Datum 编码到输出缓冲（类型标签 + 载荷）。
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

/// 一行：Datum 向量。
pub type Row = Vec<Datum>;

/// 半开键区间 `[start, end)`；`end` 为空表示无上界。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyRange {
    /// 区间下界（含）。
    pub start: Vec<u8>,
    /// 区间上界（不含）；空表示开放。
    pub end: Vec<u8>,
}

/// 扫描得到的键值对及提交时间戳。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KvPair {
    /// 编码后的用户/索引键。
    pub key: Vec<u8>,
    /// 行数据。
    pub value: Row,
    /// 提交时间戳（MVCC commit_ts）。
    pub commit_ts: u64,
}

/// KV 读取抽象：按范围扫描，可选 checksum。
pub trait KvReader: Send + Sync {
    /// 按 ranges 扫描可见版本（commit_ts <= start_ts）。
    fn scan(
        &self,
        ranges: &[KeyRange],
        start_ts: u64,
        descending: bool,
    ) -> Result<Vec<KvPair>, CopError>;
    /// 默认 checksum：对 key 与编码 value 做 FNV 异或，并统计行数/字节。
    fn checksum(&self, ranges: &[KeyRange], start_ts: u64) -> Result<(u64, u64, u64), CopError> {
        let rows = self.scan(ranges, start_ts, false)?;
        let mut checksum = 0_u64;
        let mut bytes = 0_u64;
        for pair in &rows {
            bytes += pair.key.len() as u64;
            let mut encoded = Vec::new();
            for datum in &pair.value {
                datum.encode(&mut encoded);
            }
            bytes += encoded.len() as u64;
            checksum ^= fnv64(&pair.key) ^ fnv64(&encoded);
        }
        Ok((checksum, rows.len() as u64, bytes))
    }
}

/// 内存 BTreeMap 实现的 KvReader，供单测与 mock。
#[derive(Clone, Debug, Default)]
pub struct MemoryReader {
    /// key -> (行, commit_ts)。
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
            for (key, (value, commit_ts)) in self.rows.iter().filter(|(key, _)| {
                key.as_slice() >= range.start.as_slice()
                    && (range.end.is_empty() || key.as_slice() < range.end.as_slice())
            }) {
                // 仅返回对 start_ts 可见的已提交版本。
                if *commit_ts <= start_ts {
                    result.push(KvPair {
                        key: key.clone(),
                        value: value.clone(),
                        commit_ts: *commit_ts,
                    });
                }
            }
        }
        if descending {
            result.reverse();
        }
        Ok(result)
    }
}

/// 简易表达式树：列引用、常量与比较/逻辑/算术。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expr {
    /// 列偏移。
    Column(usize),
    /// 常量。
    Constant(Datum),
    /// 等于。
    Eq(Box<Expr>, Box<Expr>),
    /// 不等于。
    Ne(Box<Expr>, Box<Expr>),
    /// 小于。
    Lt(Box<Expr>, Box<Expr>),
    /// 小于等于。
    Le(Box<Expr>, Box<Expr>),
    /// 大于。
    Gt(Box<Expr>, Box<Expr>),
    /// 大于等于。
    Ge(Box<Expr>, Box<Expr>),
    /// 逻辑与。
    And(Vec<Expr>),
    /// 逻辑或。
    Or(Vec<Expr>),
    /// 逻辑非。
    Not(Box<Expr>),
    /// IS NULL。
    IsNull(Box<Expr>),
    /// 加法。
    Add(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// 在给定行上求值。
    pub fn eval(&self, row: &[Datum]) -> Result<Datum, CopError> {
        use Expr::*;
        match self {
            Column(offset) => row
                .get(*offset)
                .cloned()
                .ok_or(CopError::ColumnOffset(*offset)),
            Constant(value) => Ok(value.clone()),
            Eq(a, b) => compare_datum(a.eval(row)?, b.eval(row)?, |ordering| {
                ordering == Ordering::Equal
            }),
            Ne(a, b) => compare_datum(a.eval(row)?, b.eval(row)?, |ordering| {
                ordering != Ordering::Equal
            }),
            Lt(a, b) => compare_datum(a.eval(row)?, b.eval(row)?, |ordering| {
                ordering == Ordering::Less
            }),
            Le(a, b) => compare_datum(a.eval(row)?, b.eval(row)?, |ordering| {
                ordering != Ordering::Greater
            }),
            Gt(a, b) => compare_datum(a.eval(row)?, b.eval(row)?, |ordering| {
                ordering == Ordering::Greater
            }),
            Ge(a, b) => compare_datum(a.eval(row)?, b.eval(row)?, |ordering| {
                ordering != Ordering::Less
            }),
            And(items) => eval_and(items, row),
            Or(items) => eval_or(items, row),
            Not(item) => match item.eval(row)? {
                Datum::Null => Ok(Datum::Null),
                value => bool_datum(!value.truthy()),
            },
            IsNull(item) => bool_datum(matches!(item.eval(row)?, Datum::Null)),
            Add(a, b) => add(a.eval(row)?, b.eval(row)?),
        }
    }
}

/// 将布尔转为 Int(0/1) Datum。
fn bool_datum(value: bool) -> Result<Datum, CopError> {
    Ok(Datum::Int(i64::from(value)))
}

/// SQL comparisons involving NULL produce NULL rather than a boolean.
fn compare_datum(
    left: Datum,
    right: Datum,
    predicate: impl FnOnce(Ordering) -> bool,
) -> Result<Datum, CopError> {
    if matches!(left, Datum::Null) || matches!(right, Datum::Null) {
        return Ok(Datum::Null);
    }
    bool_datum(predicate(left.cmp(&right)))
}

/// SQL three-valued AND: FALSE dominates, otherwise NULL dominates TRUE.
fn eval_and(items: &[Expr], row: &[Datum]) -> Result<Datum, CopError> {
    let mut saw_null = false;
    for item in items {
        match item.eval(row)? {
            Datum::Null => saw_null = true,
            value if !value.truthy() => return bool_datum(false),
            _ => {}
        }
    }
    if saw_null {
        Ok(Datum::Null)
    } else {
        bool_datum(true)
    }
}

/// SQL three-valued OR: TRUE dominates, otherwise NULL dominates FALSE.
fn eval_or(items: &[Expr], row: &[Datum]) -> Result<Datum, CopError> {
    let mut saw_null = false;
    for item in items {
        match item.eval(row)? {
            Datum::Null => saw_null = true,
            value if value.truthy() => return bool_datum(true),
            _ => {}
        }
    }
    if saw_null {
        Ok(Datum::Null)
    } else {
        bool_datum(false)
    }
}

/// 同类型数值相加；含 NULL 则结果为 NULL。
fn add(a: Datum, b: Datum) -> Result<Datum, CopError> {
    match (a, b) {
        (Datum::Int(a), Datum::Int(b)) => Ok(Datum::Int(a.saturating_add(b))),
        (Datum::Uint(a), Datum::Uint(b)) => Ok(Datum::Uint(a.saturating_add(b))),
        (Datum::Real(a), Datum::Real(b)) => Ok(Datum::Real(a + b)),
        (Datum::Null, _) | (_, Datum::Null) => Ok(Datum::Null),
        _ => Err(CopError::Type(
            "addition expects matching numeric types".into(),
        )),
    }
}

/// ORDER BY / TopN 单项：表达式、升降序与 enum 无符号比较标志。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ByItem {
    /// 排序表达式。
    pub expr: Expr,
    /// 是否降序。
    pub descending: bool,
    /// enum 是否按无符号比较。
    pub enum_unsigned: bool,
}

/// 聚合函数种类。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AggKind {
    /// COUNT。
    Count,
    /// SUM。
    Sum,
    /// MIN。
    Min,
    /// MAX。
    Max,
    /// FIRST（组内首值）。
    First,
}

/// 一次聚合调用：种类与可选参数表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AggCall {
    /// 聚合种类。
    pub kind: AggKind,
    /// 参数表达式；COUNT(*) 等可为空。
    pub expr: Option<Expr>,
}

/// Join 类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JoinType {
    /// 内连接。
    Inner,
    /// 左外连接。
    LeftOuter,
    /// 半连接。
    Semi,
    /// 反半连接。
    AntiSemi,
}

/// MPP Exchange 分区类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExchangeType {
    /// 直通。
    PassThrough,
    /// 按哈希分区。
    Hash,
    /// 广播。
    Broadcast,
}

/// Coprocessor / MPP 执行计划算子树。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Executor {
    /// 表扫描。
    TableScan {
        columns: Vec<usize>,
        descending: bool,
    },
    /// 索引扫描。
    IndexScan {
        columns: Vec<usize>,
        unique: bool,
        descending: bool,
    },
    /// 过滤。
    Selection {
        condition: Expr,
        child: Box<Executor>,
    },
    /// 限制行数。
    Limit { limit: usize, child: Box<Executor> },
    /// TopN（排序 + Limit）。
    TopN {
        limit: usize,
        order_by: Vec<ByItem>,
        child: Box<Executor>,
    },
    /// 投影。
    Projection {
        expressions: Vec<Expr>,
        child: Box<Executor>,
    },
    /// ROLLUP/CUBE 展开。
    Expand {
        levels: Vec<Vec<Option<usize>>>,
        child: Box<Executor>,
    },
    /// 聚合；`stream` 表示流式聚合。
    Aggregation {
        group_by: Vec<Expr>,
        calls: Vec<AggCall>,
        child: Box<Executor>,
        stream: bool,
    },
    /// 连接。
    Join {
        join_type: JoinType,
        left_key: Expr,
        right_key: Expr,
        left: Box<Executor>,
        right: Box<Executor>,
    },
    /// MPP 发送端。
    ExchangeSender {
        exchange: ExchangeType,
        partition_keys: Vec<Expr>,
        child: Box<Executor>,
    },
    /// MPP 接收端。
    ExchangeReceiver { source_task_ids: Vec<i64> },
}

/// DAG 请求：根执行器或扁平列表、输出列与编码选项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DagRequest {
    /// 已组装的根执行器（优先于 executors）。
    pub root: Option<Executor>,
    /// 自底向上扁平执行器列表。
    pub executors: Vec<Executor>,
    /// 输出列偏移。
    pub output_offsets: Vec<usize>,
    /// 是否收集 range 计数。
    pub collect_range_counts: bool,
    /// 是否将行编码进 `Response.data`。
    pub encode_chunk: bool,
}

/// Coprocessor 请求载荷类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestPayload {
    /// DAG 查询。
    Dag(DagRequest),
    /// Analyze 统计。
    Analyze(AnalyzeRequest),
    /// Checksum。
    Checksum,
}

/// 完整 Coprocessor 请求：载荷、范围、时间戳与缓存控制。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    /// 请求载荷。
    pub payload: RequestPayload,
    /// 扫描键范围。
    pub ranges: Vec<KeyRange>,
    /// 事务 start_ts。
    pub start_ts: u64,
    /// 已解析锁的 start_ts。
    pub resolved_locks: Vec<u64>,
    /// 分页大小；0 表示不分页。
    pub paging_size: usize,
    /// 是否启用响应缓存。
    pub cache_enabled: bool,
    /// 缓存命中所需版本。
    pub cache_if_match_version: u64,
}

/// 一批结果行。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Chunk {
    /// 行列表。
    pub rows: Vec<Row>,
}

/// 执行摘要：迭代、产出行与耗时纳秒。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecutionSummary {
    /// 迭代次数。
    pub iterations: u64,
    /// 产出行数。
    pub produced_rows: u64,
    /// 耗时（纳秒）。
    pub elapsed_ns: u64,
}

/// 键上的悲观/乐观锁信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LockInfo {
    /// 被锁的键。
    pub key: Vec<u8>,
    /// 主键（事务 primary key）。
    pub primary: Vec<u8>,
    /// 持锁事务 start_ts。
    pub start_ts: u64,
    /// 锁 TTL。
    pub ttl: u64,
    /// 锁类型编码。
    pub lock_type: u8,
}

/// Coprocessor 响应：chunks/data、锁错误、统计与缓存标志。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Response {
    /// 分行结果。
    pub chunks: Vec<Chunk>,
    /// 编码后的原始数据或 Analyze/Checksum 载荷。
    pub data: Vec<u8>,
    /// 非锁类错误信息。
    pub other_error: Option<String>,
    /// 键被锁时的锁信息。
    pub locked: Option<LockInfo>,
    /// 各 range 行计数。
    pub range_counts: Vec<i64>,
    /// 各 range NDV。
    pub ndvs: Vec<i64>,
    /// 执行摘要列表。
    pub summaries: Vec<ExecutionSummary>,
    /// 分页时的下一范围提示。
    pub last_range: Option<KeyRange>,
    /// 是否缓存命中。
    pub cache_hit: bool,
    /// 缓存版本。
    pub cache_last_version: u64,
}

/// Coprocessor 执行错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CopError {
    /// 不支持的算子/形态。
    Unsupported(&'static str),
    /// 非法请求。
    InvalidRequest(String),
    /// 列偏移越界。
    ColumnOffset(usize),
    /// 类型错误。
    Type(String),
    /// 键被锁。
    Locked(LockInfo),
    /// 执行取消。
    Cancelled,
    /// MPP tunnel 错误。
    Tunnel(String),
}

impl fmt::Display for CopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(value) => write!(f, "executor not supported: {value}"),
            Self::InvalidRequest(value) | Self::Type(value) | Self::Tunnel(value) => {
                f.write_str(value)
            }
            Self::ColumnOffset(offset) => write!(f, "column offset {offset} out of range"),
            Self::Locked(lock) => write!(f, "key is locked: {:?}", lock.key),
            Self::Cancelled => f.write_str("execution cancelled"),
        }
    }
}
impl std::error::Error for CopError {}

/// 时区名到 UTC 偏移秒数的全局映射（对齐 Go locationMap）。
#[derive(Default)]
pub struct LocationMap(RwLock<HashMap<String, i32>>);
impl LocationMap {
    /// 查询时区偏移秒数。
    pub fn get(&self, name: &str) -> Option<i32> {
        self.0
            .read()
            .expect("location lock poisoned")
            .get(name)
            .copied()
    }
    /// 写入或更新时区偏移。
    pub fn set(&self, name: String, offset_seconds: i32) {
        self.0
            .write()
            .expect("location lock poisoned")
            .insert(name, offset_seconds);
    }
}
/// 返回进程级 LocationMap 单例。
pub fn global_location_map() -> &'static LocationMap {
    static MAP: OnceLock<LocationMap> = OnceLock::new();
    MAP.get_or_init(LocationMap::default)
}

/// 将自底向上的扁平执行器列表组装成树（首元素为叶子扫描）。
pub fn executor_list_to_tree(mut executors: Vec<Executor>) -> Result<Executor, CopError> {
    if executors.is_empty() {
        return Err(CopError::InvalidRequest("executor list is empty".into()));
    }
    let mut root = executors.remove(0);
    for executor in executors {
        root = attach_child(executor, root)?;
    }
    Ok(root)
}

/// 把已有子树挂到可接受 child 的父算子上。
fn attach_child(parent: Executor, child: Executor) -> Result<Executor, CopError> {
    Ok(match parent {
        Executor::Selection { condition, .. } => Executor::Selection {
            condition,
            child: Box::new(child),
        },
        Executor::Limit { limit, .. } => Executor::Limit {
            limit,
            child: Box::new(child),
        },
        Executor::TopN {
            limit, order_by, ..
        } => Executor::TopN {
            limit,
            order_by,
            child: Box::new(child),
        },
        Executor::Projection { expressions, .. } => Executor::Projection {
            expressions,
            child: Box::new(child),
        },
        Executor::Expand { levels, .. } => Executor::Expand {
            levels,
            child: Box::new(child),
        },
        Executor::Aggregation {
            group_by,
            calls,
            stream,
            ..
        } => Executor::Aggregation {
            group_by,
            calls,
            child: Box::new(child),
            stream,
        },
        Executor::ExchangeSender {
            exchange,
            partition_keys,
            ..
        } => Executor::ExchangeSender {
            exchange,
            partition_keys,
            child: Box::new(child),
        },
        _ => {
            return Err(CopError::InvalidRequest(
                "executor cannot accept a child".into(),
            ));
        }
    })
}

/// Coprocessor 请求入口：缓存命中、DAG/Analyze/Checksum 分派与错误映射。
pub fn handle_cop_request(reader: &dyn KvReader, request: &Request) -> Response {
    // 缓存版本与 start_ts 匹配时直接返回命中。
    if request.cache_enabled && request.cache_if_match_version == request.start_ts {
        return Response {
            cache_hit: true,
            cache_last_version: request.start_ts,
            ..Response::default()
        };
    }
    let started = Instant::now();
    let result = match &request.payload {
        RequestPayload::Dag(dag) => handle_dag(reader, request, dag),
        RequestPayload::Analyze(analyze_request) => {
            analyze(reader, &request.ranges, request.start_ts, analyze_request).map(|data| {
                Response {
                    data,
                    ..Response::default()
                }
            })
        }
        RequestPayload::Checksum => {
            // The unistore Go mock returns a fixed checksum response rather
            // than reading the backing DB; keep the Rust mock wire behavior identical.
            let mut data = Vec::with_capacity(24);
            for value in [1_u64; 3] {
                data.extend_from_slice(&value.to_be_bytes());
            }
            Ok(Response {
                data,
                ..Response::default()
            })
        }
    };
    match result {
        Ok(mut response) => {
            if response.summaries.is_empty() {
                response.summaries.push(ExecutionSummary {
                    iterations: 1,
                    produced_rows: response.chunks.iter().map(|c| c.rows.len() as u64).sum(),
                    elapsed_ns: started.elapsed().as_nanos() as u64,
                });
            }
            response
        }
        Err(CopError::Locked(lock)) => Response {
            locked: Some(lock),
            ..Response::default()
        },
        Err(error) => Response {
            other_error: Some(error.to_string()),
            ..Response::default()
        },
    }
}

/// 处理 DAG：组装根执行器、投影输出列、可选分页截断。
fn handle_dag(
    reader: &dyn KvReader,
    request: &Request,
    dag: &DagRequest,
) -> Result<Response, CopError> {
    if request.ranges.is_empty() {
        return Err(CopError::InvalidRequest("request range is null".into()));
    }
    let root = match &dag.root {
        Some(root) => root.clone(),
        None => executor_list_to_tree(dag.executors.clone())?,
    };
    let mut output = execute_executor(reader, &request.ranges, request.start_ts, &root)?;
    if !dag.output_offsets.is_empty() {
        for row in &mut output.rows {
            *row = dag
                .output_offsets
                .iter()
                .map(|offset| {
                    row.get(*offset)
                        .cloned()
                        .ok_or(CopError::ColumnOffset(*offset))
                })
                .collect::<Result<_, _>>()?;
        }
    }
    // 分页：超出 paging_size 时截断并带上 last_range。
    let last_range = if request.paging_size > 0 && output.rows.len() > request.paging_size {
        output.rows.truncate(request.paging_size);
        request.ranges.last().cloned()
    } else {
        None
    };
    Ok(response_from_output(output, last_range, dag.encode_chunk))
}

/// 将执行输出拆成 Chunk，并按需编码到 `data`。
fn response_from_output(
    output: ExecutionOutput,
    last_range: Option<KeyRange>,
    encode_chunk: bool,
) -> Response {
    let mut chunks = Vec::new();
    for batch in output.rows.chunks(ROWS_PER_CHUNK) {
        chunks.push(Chunk {
            rows: batch.to_vec(),
        });
    }
    let mut data = Vec::new();
    if encode_chunk {
        for chunk in &chunks {
            for row in &chunk.rows {
                for datum in row {
                    datum.encode(&mut data);
                }
            }
        }
    }
    Response {
        chunks,
        data,
        range_counts: output.range_counts,
        ndvs: output.ndvs,
        summaries: output.summaries,
        last_range,
        ..Response::default()
    }
}

/// 将请求 ranges 与 Region `[region_start, region_end)` 求交，可选倒序。
pub fn extract_kv_ranges(
    region_start: &[u8],
    region_end: &[u8],
    ranges: &[KeyRange],
    descending: bool,
) -> Result<Vec<KeyRange>, CopError> {
    let mut result = Vec::new();
    for range in ranges {
        if range.start >= range.end {
            return Err(CopError::InvalidRequest(
                "invalid range, start should be smaller than end".into(),
            ));
        }
        let start = if range.start.as_slice() > region_start {
            range.start.clone()
        } else {
            region_start.to_vec()
        };
        let end = if region_end.is_empty()
            || (!range.end.is_empty() && range.end.as_slice() < region_end)
        {
            range.end.clone()
        } else {
            region_end.to_vec()
        };
        // 空交集跳过。
        if !end.is_empty() && start >= end {
            continue;
        }
        result.push(KeyRange { start, end });
    }
    if descending {
        result.reverse();
    }
    Ok(result)
}

/// 向 chunks 追加一行；当前块满则新建 Chunk。
pub fn append_row(chunks: &mut Vec<Chunk>, row: Row) {
    if chunks
        .last()
        .is_none_or(|chunk| chunk.rows.len() >= ROWS_PER_CHUNK)
    {
        chunks.push(Chunk::default());
    }
    chunks.last_mut().expect("chunk exists").rows.push(row);
}

/// 若锁覆盖 key 且 start_ts 可见且未 resolved，则返回 Locked。
pub fn check_lock(
    lock: &LockInfo,
    key: &[u8],
    start_ts: u64,
    resolved: &[u64],
) -> Result<(), CopError> {
    if lock.key == key && lock.start_ts <= start_ts && !resolved.contains(&lock.start_ts) {
        Err(CopError::Locked(lock.clone()))
    } else {
        Ok(())
    }
}

/// 由耗时与行数构造单条 ExecutionSummary。
pub fn duration_summary(duration: Duration, rows: usize) -> ExecutionSummary {
    ExecutionSummary {
        iterations: 1,
        produced_rows: rows as u64,
        elapsed_ns: duration.as_nanos() as u64,
    }
}

/// FNV-1a 64 位哈希，用于 checksum。
fn fnv64(data: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
