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

// 执行器测试用 Mock 数据源与会话/Chunk 基础设施。
//
// 提供列类型、Datum 值、会话变量（含 Chunk 大小与内存跟踪）、按 NDV/有序性
// 生成列数据的 `MockDataSource`，以及实现 `Executor` 接口的拉取路径，供
// Agg/Limit/Sort/Window 等算子单测与基准构造输入。

use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use astersql_executor_internal_exec::executor::{
    Chunk, ExecContext, Executor, FieldType, Result, Schema,
};

/// 默认初始 Chunk 容量（行数）。
pub const DEF_INIT_CHUNK_SIZE: usize = 32;
/// 默认最大 Chunk 容量（行数）；批处理时一次最多装入这么多行。
pub const DEF_MAX_CHUNK_SIZE: usize = 1024;

/// 测试用字段类型（对应 MySQL/TiDB 列类型的简化枚举）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldKind {
    Tiny,
    Short,
    Int24,
    Long,
    LongLong,
    Float,
    Double,
    Decimal,
    Varchar,
    VarString,
    String,
    Blob,
    TinyBlob,
    MediumBlob,
    LongBlob,
    Year,
    Date,
    DateTime,
    Timestamp,
    Duration,
    Enum,
    Set,
    Bit,
    Json,
    Null,
}
impl Display for FieldKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Tiny => "tinyint",
            Self::Short => "smallint",
            Self::Int24 => "mediumint",
            Self::Long => "int",
            Self::LongLong => "bigint",
            Self::Float => "float",
            Self::Double => "double",
            Self::Decimal => "decimal",
            Self::Varchar => "varchar",
            Self::VarString => "varchar",
            Self::String => "char",
            Self::Blob => "blob",
            Self::TinyBlob => "tinyblob",
            Self::MediumBlob => "mediumblob",
            Self::LongBlob => "longblob",
            Self::Year => "year",
            Self::Date => "date",
            Self::DateTime => "datetime",
            Self::Timestamp => "timestamp",
            Self::Duration => "time",
            Self::Enum => "enum",
            Self::Set => "set",
            Self::Bit => "bit",
            Self::Json => "json",
            Self::Null => "null",
        };
        f.write_str(name)
    }
}

/// 列定义：下标、类型与是否可空。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnDef {
    /// 列在 schema 中的下标。
    pub index: usize,
    /// 列类型。
    pub kind: FieldKind,
    /// 是否允许 NULL。
    pub nullable: bool,
    /// 是否为无符号数值列（对应 Go FieldType 的 UnsignedFlag）。
    pub unsigned: bool,
}
impl ColumnDef {
    /// 构造默认可空的列定义。
    pub fn new(index: usize, kind: FieldKind) -> Self {
        Self {
            index,
            kind,
            nullable: true,
            unsigned: false,
        }
    }
}

/// 运行时单元格值（Datum），覆盖整型、浮点、字符串、时间与 JSON 等。
#[derive(Clone, Debug, PartialEq)]
pub enum Datum {
    Null,
    Int(i64),
    UInt(u64),
    Float(f32),
    Double(f64),
    Decimal(String),
    Text(String),
    Time {
        year: i32,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        micros: u32,
    },
    Duration(i64),
    Enum(String, u64),
    Set(String, u64),
    Bytes(Vec<u8>),
    Json(String),
}
impl Display for Datum {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// 内存跟踪器：`limit` 为字节上限（-1 表示不限制），`attached` 表示是否已挂到语句。
#[derive(Clone, Debug, Default)]
pub struct MemoryTracker {
    pub limit: i64,
    pub attached: bool,
}
/// 会话变量子集：Chunk 大小与内存跟踪。
#[derive(Clone, Debug)]
pub struct SessionVars {
    pub init_chunk_size: usize,
    pub max_chunk_size: usize,
    pub memory_tracker: MemoryTracker,
    pub statement_memory_tracker: MemoryTracker,
}
impl Default for SessionVars {
    fn default() -> Self {
        Self {
            init_chunk_size: DEF_INIT_CHUNK_SIZE,
            max_chunk_size: DEF_MAX_CHUNK_SIZE,
            memory_tracker: MemoryTracker {
                limit: -1,
                attached: false,
            },
            statement_memory_tracker: MemoryTracker {
                limit: -1,
                attached: false,
            },
        }
    }
}
/// 测试用会话上下文，封装会话变量。
#[derive(Clone, Debug, Default)]
pub struct SessionContext {
    pub vars: SessionVars,
}
/// 将会话 Chunk 大小重置为默认初值/最大值。
pub fn resetChunkSizes(ctx: &mut SessionContext) {
    ctx.vars.init_chunk_size = DEF_INIT_CHUNK_SIZE;
    ctx.vars.max_chunk_size = DEF_MAX_CHUNK_SIZE;
}

/// 按行号与列类型生成 Datum 的回调类型。
pub type GenDataFunc = Arc<dyn Fn(usize, &FieldKind) -> Datum + Send + Sync>;
/// 构造 `MockDataSource` 的参数：schema、NDV、有序性、预置数据与行数等。
#[derive(Clone)]
pub struct MockDataSourceParameters {
    pub Ctx: SessionContext,
    pub DataSchema: Vec<ColumnDef>,
    pub GenDataFunc: Option<GenDataFunc>,
    /// 各列 NDV：0=逐行随机/回调；-1/-2=使用 Datums 预置；>0=从有限不同值池抽样。
    pub Ndvs: Vec<i32>,
    /// 各列是否要求生成后排序。
    pub Orders: Vec<bool>,
    /// 预置列数据（配合 NDV=-1/-2）。
    pub Datums: Vec<Option<Vec<Datum>>>,
    /// 可选 NULL 位图：`nulls[col][row]`。
    pub Nulls: Option<Vec<Vec<bool>>>,
    pub Rows: usize,
    /// 是否为每个 Chunk 生成 selection（隔行选取）。
    pub HasSel: bool,
}
impl Default for MockDataSourceParameters {
    fn default() -> Self {
        Self {
            Ctx: SessionContext::default(),
            DataSchema: Vec::new(),
            GenDataFunc: None,
            Ndvs: Vec::new(),
            Orders: Vec::new(),
            Datums: Vec::new(),
            Nulls: None,
            Rows: 0,
            HasSel: false,
        }
    }
}

/// 列式数据块：多列 Datum 向量，可选 selection 向量表示有效行下标。
#[derive(Clone, Debug, Default)]
pub struct DataChunk {
    pub columns: Vec<Vec<Datum>>,
    pub selection: Option<Vec<usize>>,
    pub rows: usize,
}
impl DataChunk {
    fn new(column_count: usize) -> Self {
        Self {
            columns: vec![Vec::new(); column_count],
            selection: None,
            rows: 0,
        }
    }
    /// 有效行数：有 selection 时取其长度，否则用 `rows`。
    pub fn NumRows(&self) -> usize {
        self.selection.as_ref().map_or(self.rows, Vec::len)
    }
}

/// Mock 执行器：预生成 Chunk 序列，按 `Next` 逐块返回。
pub struct MockDataSource {
    /// 预生成的完整数据副本（Open 时拷到 Chunks）。
    pub GenData: Vec<DataChunk>,
    /// 当前可拉取的 Chunk 队列。
    pub Chunks: Vec<DataChunk>,
    pub P: MockDataSourceParameters,
    /// 下一将返回的 Chunk 下标。
    pub ChunkPtr: usize,
    fields: Vec<FieldType>,
    schema: Schema,
}
impl MockDataSource {
    /// 按 NDV / Orders / GenDataFunc / Datums 规则生成单列全部行的 Datum。
    pub fn GenColDatums(&self, col: usize) -> Vec<Datum> {
        let kind = &self.P.DataSchema[col].kind;
        let ordered = self.P.Orders.get(col).copied().unwrap_or(false);
        let ndv = self.P.Ndvs.get(col).copied().unwrap_or_default();
        let mut results = if ndv == 0 {
            // 无 NDV 限制：逐行用回调或随机值。
            (0..self.P.Rows)
                .map(|row| {
                    self.P
                        .GenDataFunc
                        .as_ref()
                        .map_or_else(|| self.RandDatum(kind), |generate| generate(row, kind))
                })
                .collect()
        } else if ndv == -2 {
            self.P
                .Datums
                .get(col)
                .and_then(|values| values.clone())
                .expect("need to provide data")
        } else {
            // -1：直接用预置 base；>0：先采样 ndv 个不同值再随机回填到 Rows。
            let base = if ndv == -1 {
                self.P
                    .Datums
                    .get(col)
                    .and_then(|values| values.clone())
                    .expect("need to provide data")
            } else {
                let mut values = Vec::with_capacity(ndv.max(0) as usize);
                let mut seen = HashSet::new();
                while values.len() < ndv as usize {
                    let datum = self.RandDatum(kind);
                    if seen.insert(datum.to_string()) {
                        values.push(datum);
                    }
                }
                values
            };
            (0..self.P.Rows)
                .map(|_| base[(nextRandom() as usize) % base.len()].clone())
                .collect()
        };
        if ordered {
            results.sort_by(|left, right| match (left, right) {
                (Datum::Int(a), Datum::Int(b)) => a.cmp(b),
                (Datum::UInt(a), Datum::UInt(b)) => a.cmp(b),
                (Datum::Double(a), Datum::Double(b)) => a.total_cmp(b),
                (Datum::Text(a), Datum::Text(b)) => a.cmp(b),
                _ => panic!("not implement"),
            });
        }
        results
    }

    /// 按字段类型生成一个伪随机 Datum。
    pub fn RandDatum(&self, kind: &FieldKind) -> Datum {
        let value = (nextRandom() % 1_000_000) as i64;
        match kind {
            FieldKind::Tiny
            | FieldKind::Short
            | FieldKind::Int24
            | FieldKind::Long
            | FieldKind::LongLong => Datum::Int(value),
            FieldKind::Float => Datum::Float(value as f32 / 1000.0),
            FieldKind::Double => Datum::Double(value as f64 / 1000.0),
            FieldKind::Decimal => Datum::Decimal(value.to_string()),
            FieldKind::Varchar
            | FieldKind::VarString
            | FieldKind::String
            | FieldKind::Blob
            | FieldKind::TinyBlob
            | FieldKind::MediumBlob
            | FieldKind::LongBlob => Datum::Text(format!("{:014x}", nextRandom())),
            _ => panic!("not implement"),
        }
    }

    /// 将 GenData 拷贝到 Chunks 并重置读取指针。
    pub fn PrepareChunks(&mut self) {
        self.Chunks.clone_from(&self.GenData);
        self.ChunkPtr = 0;
    }
}

impl Executor for MockDataSource {
    fn executorType(&self) -> &'static str {
        "*testutil.MockDataSource"
    }
    fn Open(&mut self, _ctx: &ExecContext) -> Result<()> {
        self.PrepareChunks();
        Ok(())
    }
    fn Next(&mut self, _ctx: &ExecContext, req: &mut Chunk) -> Result<()> {
        req.Reset();
        if let Some(data) = self.Chunks.get(self.ChunkPtr) {
            req.SetNumRows(data.NumRows());
            self.ChunkPtr += 1;
        }
        Ok(())
    }
    fn Close(&mut self) -> Result<()> {
        Ok(())
    }
    fn Schema(&self) -> Schema {
        self.schema.clone()
    }
    fn RetFieldTypes(&self) -> Vec<FieldType> {
        self.fields.clone()
    }
    fn InitCap(&self) -> usize {
        self.P.Ctx.vars.init_chunk_size
    }
    fn MaxChunkSize(&self) -> usize {
        self.P.Ctx.vars.max_chunk_size
    }
    fn AllChildren(&self) -> &[Box<dyn Executor>] {
        &[]
    }
    fn SetAllChildren(&mut self, _children: Vec<Box<dyn Executor>>) {}
}

/// 包装 Mock 执行器的物理计划桩，便于接入需要 PhysicalPlan 的测试路径。
pub struct MockDataPhysicalPlan {
    pub DataSchema: Schema,
    executor: Box<dyn Executor>,
}
impl MockDataPhysicalPlan {
    /// 返回内部执行器；与 Go 接口一样可重复获取同一个执行器。
    pub fn GetExecutor(&self) -> &dyn Executor {
        self.executor.as_ref()
    }
    /// 转移内部执行器所有权，供确实需要消费计划桩的 Rust 调用方使用。
    pub fn TakeExecutor(self) -> Box<dyn Executor> {
        self.executor
    }
    pub fn Schema(&self) -> &Schema {
        &self.DataSchema
    }
    pub fn ExplainID(&self) -> &'static str {
        "mockData_0"
    }
    pub fn ID(&self) -> i32 {
        0
    }
    pub fn SetID(&mut self, _id: i32) {
        panic!("not implement")
    }
    pub fn MemoryUsage(&self) -> i64 {
        0
    }
}
/// 用已有执行器构造 `MockDataPhysicalPlan`。
pub fn BuildMockDataPhysicalPlan(executor: Box<dyn Executor>) -> MockDataPhysicalPlan {
    let schema = executor.Schema();
    MockDataPhysicalPlan {
        DataSchema: schema,
        executor,
    }
}

/// 按参数生成列数据并切分为多个 DataChunk，返回可 Open/Next 的 Mock 数据源。
pub fn BuildMockDataSource(mut opt: MockDataSourceParameters) -> MockDataSource {
    if opt.Ctx.vars.max_chunk_size == 0 {
        resetChunkSizes(&mut opt.Ctx);
    }
    let fields: Vec<_> = opt
        .DataSchema
        .iter()
        .map(|column| FieldType {
            type_code: fieldCode(&column.kind),
        })
        .collect();
    let schema = Schema {
        fields: fields.clone(),
    };
    let shell = MockDataSource {
        GenData: Vec::new(),
        Chunks: Vec::new(),
        P: opt,
        ChunkPtr: 0,
        fields,
        schema,
    };
    // 先按列生成全部 Datum，再按 max_chunk_size 行切块填入。
    let column_data: Vec<_> = (0..shell.P.DataSchema.len())
        .map(|column| shell.GenColDatums(column))
        .collect();
    let max_chunk = shell.P.Ctx.vars.max_chunk_size.max(1);
    let count = shell.P.Rows.div_ceil(max_chunk);
    let nulls = shell
        .P
        .Nulls
        .clone()
        .unwrap_or_else(|| vec![vec![false; shell.P.Rows]; shell.P.DataSchema.len()]);
    let mut generated: Vec<_> = (0..count)
        .map(|_| DataChunk::new(shell.P.DataSchema.len()))
        .collect();
    for row in 0..shell.P.Rows {
        let chunk_index = row / max_chunk;
        for column in 0..shell.P.DataSchema.len() {
            generated[chunk_index].columns[column].push(if nulls[column][row] {
                Datum::Null
            } else {
                column_data[column][row].clone()
            });
        }
        generated[chunk_index].rows += 1;
    }
    if shell.P.HasSel {
        // 从随机起点起每隔一行选取，模拟带 selection 的 Chunk。
        for chunk in &mut generated {
            let start = (nextRandom() % 2) as usize;
            chunk.selection = Some((start..chunk.rows).step_by(2).collect());
        }
    }
    MockDataSource {
        GenData: generated,
        ..shell
    }
}

/// 在 `BuildMockDataSource` 基础上将 `indexes` 指定列标记为有序。
pub fn BuildMockDataSourceWithIndex(
    mut opt: MockDataSourceParameters,
    indexes: &[usize],
) -> MockDataSource {
    opt.Orders = vec![false; opt.DataSchema.len()];
    for index in indexes {
        opt.Orders[*index] = true;
    }
    BuildMockDataSource(opt)
}

/// 内存超限时触发的 Mock 动作，记录触发次数。
#[derive(Default)]
pub struct MockActionOnExceed {
    triggered_num: AtomicI32,
}
impl MockActionOnExceed {
    /// 记录一次超限动作。
    pub fn Action(&self) {
        self.triggered_num.fetch_add(1, Ordering::AcqRel);
    }
    pub fn GetPriority(&self) -> i64 {
        1
    }
    /// 已触发次数。
    pub fn GetTriggeredNum(&self) -> i32 {
        self.triggered_num.load(Ordering::Acquire)
    }
}

/// 按 schema 生成指定行数的随机 DataChunk（覆盖更多字段类型）。
pub fn GenRandomChunks(schema: &[ColumnDef], size: usize) -> DataChunk {
    let mut chunk = DataChunk::new(schema.len());
    for (index, field) in schema.iter().enumerate() {
        for row in 0..size {
            chunk.columns[index].push(randomDatumForField(field, row));
        }
    }
    chunk.rows = size;
    chunk
}

/// 按列定义生成单个随机 Datum；可空列约 10% 概率为 NULL。
fn randomDatumForField(field: &ColumnDef, row: usize) -> Datum {
    if field.nullable && nextRandom().is_multiple_of(10) {
        return Datum::Null;
    }
    let value = nextRandom();
    match field.kind {
        FieldKind::Tiny | FieldKind::Short | FieldKind::Int24 | FieldKind::Long | FieldKind::LongLong => {
            let upper_bound = match field.kind {
                FieldKind::Tiny => i8::MAX as u64,
                FieldKind::Short => i16::MAX as u64,
                FieldKind::Int24 => (1_u64 << 23) - 1,
                FieldKind::Long => i32::MAX as u64,
                FieldKind::LongLong => i64::MAX as u64,
                _ => unreachable!(),
            };
            let value = value % upper_bound;
            if field.unsigned {
                Datum::UInt(value * 2)
            } else if value.is_multiple_of(3) {
                Datum::Int(value as i64)
            } else {
                Datum::Int(-(value as i64))
            }
        }
        FieldKind::Float => Datum::Float(((value % 20_001) as f32 - 10_000.0) / 1_000.0),
        FieldKind::Double => Datum::Double(((value % 20_001) as f64 - 10_000.0) / 1_000.0),
        FieldKind::Decimal => {
            let base = (value % i32::MAX as u64) as i64;
            let divisor = (nextRandom() % 10) + 1;
            let signed = if nextRandom().is_multiple_of(2) { base } else { -base };
            Datum::Decimal(format!("{}", signed as f64 / divisor as f64))
        }
        FieldKind::Varchar
        | FieldKind::VarString
        | FieldKind::String
        | FieldKind::Blob
        | FieldKind::TinyBlob
        | FieldKind::MediumBlob
        | FieldKind::LongBlob => Datum::Text(format!("row-{row}-{value:x}")),
        FieldKind::Year => Datum::Int(1901 + (value % 255) as i64),
        FieldKind::Date | FieldKind::DateTime | FieldKind::Timestamp => {
            let month = 1 + (nextRandom() % 12) as u8;
            let max_day = [31_u8, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
                [(month - 1) as usize];
            let is_date = field.kind == FieldKind::Date;
            Datum::Time {
                year: 1900 + (value % 200) as i32,
                month,
                day: (nextRandom() % u64::from(max_day)) as u8,
                hour: if is_date { 0 } else { (nextRandom() % 24) as u8 },
                minute: if is_date { 0 } else { (nextRandom() % 60) as u8 },
                second: if is_date { 0 } else { (nextRandom() % 60) as u8 },
                micros: if is_date { 0 } else { (nextRandom() % 1_000_000) as u32 },
            }
        }
        FieldKind::Duration => {
            let hour = (value % 839) as i64;
            let minute = (nextRandom() % 60) as i64;
            let second = (nextRandom() % 60) as i64;
            let micros = (nextRandom() % 1_000_000) as i64;
            let duration = (((hour * 60 + minute) * 60 + second) * 1_000_000) + micros;
            Datum::Duration(if hour * minute % 3 == 0 {
                -duration
            } else {
                duration
            })
        }
        FieldKind::Enum | FieldKind::Set => {
            const VALUES: [&str; 6] = ["abc", "bcd", "cde", "def", "efg", "fgh"];
            let index = (value as usize) % VALUES.len();
            if field.kind == FieldKind::Enum {
                Datum::Enum(VALUES[index].into(), index as u64)
            } else {
                Datum::Set(VALUES[index].into(), index as u64)
            }
        }
        FieldKind::Bit => Datum::Bytes((value % i64::MAX as u64).to_ne_bytes().to_vec()),
        FieldKind::Json => match value % 6 {
            0 => Datum::Json((nextRandom() % i16::MAX as u64).to_string()),
            1 => Datum::Json(format!("{}", (nextRandom() % 20_001) as f64 / 1_000.0)),
            2 => Datum::Json("null".into()),
            3 => Datum::Json(format!("[{},{}]", nextRandom(), nextRandom())),
            4 => Datum::Json(format!("{{\"{}\":{}}}", nextRandom(), nextRandom())),
            _ => Datum::Json("\"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ;.,!@#$%^&*()-=+_平凯数据库\"".into()),
        },
        FieldKind::Null => Datum::Null,
    }
}

/// 将 `FieldKind` 映射为 MySQL type code（简化子集）。
fn fieldCode(kind: &FieldKind) -> u8 {
    match kind {
        FieldKind::Tiny => 1,
        FieldKind::Short => 2,
        FieldKind::Int24 => 9,
        FieldKind::Long => 3,
        FieldKind::LongLong => 8,
        FieldKind::Float => 4,
        FieldKind::Double => 5,
        FieldKind::Decimal => 246,
        FieldKind::Varchar => 15,
        FieldKind::VarString => 253,
        FieldKind::String => 254,
        FieldKind::Blob => 252,
        FieldKind::TinyBlob => 249,
        FieldKind::MediumBlob => 250,
        FieldKind::LongBlob => 251,
        FieldKind::Year => 13,
        FieldKind::Date => 10,
        FieldKind::DateTime => 12,
        FieldKind::Timestamp => 7,
        FieldKind::Duration => 11,
        FieldKind::Enum => 247,
        FieldKind::Set => 248,
        FieldKind::Bit => 16,
        FieldKind::Json => 245,
        FieldKind::Null => 6,
    }
}
/// 进程内伪随机数发生器（LCG），供测试数据生成复用。
fn nextRandom() -> u64 {
    static STATE: AtomicU64 = AtomicU64::new(0x9e3779b97f4a7c15);
    let mut old = STATE.load(Ordering::Relaxed);
    loop {
        let next = old
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        match STATE.compare_exchange_weak(old, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(current) => old = current,
        }
    }
}
