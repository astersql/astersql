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
// Copyright 2026 AsterSQL.

// 排序执行器公共工具：排序值、行、键、比较器、chunk、内存追踪与磁盘 run。
//
// 本模块为 sortexec 的基础类型层：`SortValue`/`Row`/`SortKey` 描述可比较数据，
// `MemoryTracker` 跟踪配额，`DiskRun` 表示 spill 后的有序 chunk 序列，
// spill 状态常量与游标辅助串行/并行落盘路径。

use std::cmp::Ordering;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering as AtomicOrdering};

/// 落盘时每个磁盘 chunk 的行数上限（可被测试临时调小）。
pub static spillChunkSize: AtomicUsize = AtomicUsize::new(1024);
/// 排序过程中检查 kill/信号的行数检查点间隔。
pub const signalCheckpointForSort: usize = 10_240;
/// spill 状态：尚未落盘。
pub const notSpilled: i32 = 0;
/// spill 状态：已标记需要落盘。
pub const needSpill: i32 = 1;
/// spill 状态：正在落盘中。
pub const inSpilling: i32 = 2;
/// spill 状态：已完成落盘。
pub const spillTriggered: i32 = 3;

/// 排序单元格值：支持 NULL 与常见标量类型的字典序比较。
#[derive(Clone, Debug, PartialEq)]
pub enum SortValue {
    /// SQL NULL。
    Null,
    /// 有符号整数。
    Int(i64),
    /// 无符号整数。
    UInt(u64),
    /// 浮点数（比较用 total_cmp）。
    Float(f64),
    /// 字节串 / 字符串编码。
    Bytes(Vec<u8>),
}

impl SortValue {
    /// 类型判别序号，用于跨类型比较时的兜底顺序。
    fn kind(&self) -> u8 {
        match self {
            Self::Null => 0,
            Self::Int(_) => 1,
            Self::UInt(_) => 2,
            Self::Float(_) => 3,
            Self::Bytes(_) => 4,
        }
    }

    /// 估算该值占用的字节数（Bytes 含内容长度）。
    pub fn memory_usage(&self) -> i64 {
        match self {
            Self::Bytes(v) => 24 + v.len() as i64,
            _ => 16,
        }
    }
}

impl PartialOrd for SortValue {
    fn partial_cmp(&self, rhs: &Self) -> Option<Ordering> {
        use SortValue::*;
        Some(match (self, rhs) {
            (Null, Null) => Ordering::Equal,
            (Int(a), Int(b)) => a.cmp(b),
            (UInt(a), UInt(b)) => a.cmp(b),
            (Float(a), Float(b)) => a.total_cmp(b),
            (Bytes(a), Bytes(b)) => a.cmp(b),
            (Int(a), UInt(b)) if *a >= 0 => (*a as u64).cmp(b),
            (UInt(a), Int(b)) if *b >= 0 => a.cmp(&(*b as u64)),
            (Int(a), Float(b)) => (*a as f64).total_cmp(b),
            (Float(a), Int(b)) => a.total_cmp(&(*b as f64)),
            (UInt(a), Float(b)) => (*a as f64).total_cmp(b),
            (Float(a), UInt(b)) => a.total_cmp(&(*b as f64)),
            // 无法数值对齐的跨类型比较退回到 kind 序号
            _ => self.kind().cmp(&rhs.kind()),
        })
    }
}

/// 一行：由若干 [`SortValue`] 组成的列向量。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row(pub Vec<SortValue>);

impl Row {
    /// 估算整行内存占用。
    pub fn memory_usage(&self) -> i64 {
        24 + self.0.iter().map(SortValue::memory_usage).sum::<i64>()
    }
}

/// 单个排序键：列下标、是否降序、NULL 是否排前。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortKey {
    /// 参与比较的列下标（0-based）。
    pub column: usize,
    /// 为 true 时按降序比较非 NULL 值。
    pub desc: bool,
    /// 为 true 时 NULL 排在最前（NULLS FIRST）。
    pub nulls_first: bool,
}

impl SortKey {
    /// 升序键，默认 NULLS FIRST。
    pub fn asc(column: usize) -> Self {
        Self {
            column,
            desc: false,
            nulls_first: true,
        }
    }
    /// 降序键，默认 NULLS LAST。
    pub fn desc(column: usize) -> Self {
        Self {
            column,
            desc: true,
            nulls_first: false,
        }
    }
}

/// 按一组 SortKey 比较两行；NULL 放置独立于 ASC/DESC。
pub fn compare_rows(lhs: &Row, rhs: &Row, keys: &[SortKey]) -> Ordering {
    for key in keys {
        let a = lhs.0.get(key.column).unwrap_or(&SortValue::Null);
        let b = rhs.0.get(key.column).unwrap_or(&SortValue::Null);
        let mut order = match (a, b) {
            (SortValue::Null, SortValue::Null) => Ordering::Equal,
            (SortValue::Null, _) => {
                if key.nulls_first {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
            (_, SortValue::Null) => {
                if key.nulls_first {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
            _ => a.partial_cmp(b).unwrap_or(Ordering::Equal),
        };
        // NULL placement is specified independently of ASC/DESC. Only reverse
        // the comparison of two non-NULL values for descending order.
        // NULL 放置与 ASC/DESC 独立：仅对两个非 NULL 值在降序时取反比较结果。
        if key.desc && !matches!(a, SortValue::Null) && !matches!(b, SortValue::Null) {
            order = order.reverse();
        }
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

/// 可跨线程共享的行比较闭包。
pub type RowComparator = Arc<dyn Fn(&Row, &Row) -> Ordering + Send + Sync>;
/// 由排序键列表构造共享比较器。
pub fn comparator(keys: Vec<SortKey>) -> RowComparator {
    Arc::new(move |a, b| compare_rows(a, b, &keys))
}

/// 一批行构成的数据块（对应执行器 chunk）。
#[derive(Clone, Debug, Default)]
pub struct DataChunk {
    pub rows: Vec<Row>,
}

impl DataChunk {
    /// 由行列表构造 chunk。
    pub fn new(rows: Vec<Row>) -> Self {
        Self { rows }
    }
    /// 行数。
    pub fn num_rows(&self) -> usize {
        self.rows.len()
    }
    /// 是否为空 chunk。
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    /// 估算 chunk 内存占用。
    pub fn memory_usage(&self) -> i64 {
        24 + self.rows.iter().map(Row::memory_usage).sum::<i64>()
    }
}

/// 排序子系统错误，携带说明字符串。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortError(pub String);

impl Display for SortError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SortError {}
/// 排序子系统的 Result 别名。
pub type Result<T> = std::result::Result<T, SortError>;

/// 构造“不能 spill 空 chunk”错误。
pub fn errSpillEmptyChunk() -> SortError {
    SortError("can not spill empty chunk to disk".into())
}
/// 构造“无法向分区添加 chunk”错误。
pub fn errFailToAddChunk() -> SortError {
    SortError("fail to add chunk".into())
}

/// 原子内存用量追踪器：记录已消费字节与限额。
#[derive(Default)]
pub struct MemoryTracker {
    consumed: AtomicI64,
    limit: AtomicI64,
}

impl MemoryTracker {
    /// 构造追踪器；`limit < 0` 表示无限制。
    pub fn new(limit: i64) -> Self {
        Self {
            consumed: AtomicI64::new(0),
            limit: AtomicI64::new(limit),
        }
    }
    /// 增加已消费字节数。
    pub fn consume(&self, bytes: i64) {
        self.consumed.fetch_add(bytes, AtomicOrdering::Relaxed);
    }
    /// 归还已消费字节数。
    pub fn release(&self, bytes: i64) {
        if bytes <= 0 {
            return;
        }
        let mut current = self.consumed.load(AtomicOrdering::Relaxed);
        loop {
            let next = current.saturating_sub(bytes);
            match self.consumed.compare_exchange_weak(
                current,
                next,
                AtomicOrdering::Relaxed,
                AtomicOrdering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }
    /// 当前已消费字节数。
    pub fn bytes_consumed(&self) -> i64 {
        self.consumed.load(AtomicOrdering::Relaxed)
    }
    /// 当前内存限额。
    pub fn bytes_limit(&self) -> i64 {
        self.limit.load(AtomicOrdering::Relaxed)
    }
    /// 更新内存限额。
    pub fn set_bytes_limit(&self, limit: i64) {
        self.limit.store(limit, AtomicOrdering::Relaxed);
    }
    /// 是否已达到或超过限额（无限制时恒为 false）。
    pub fn exceeded(&self) -> bool {
        let l = self.bytes_limit();
        l >= 0 && self.bytes_consumed() >= l
    }
}

/// 磁盘上的有序 run：由多个非空 chunk 组成，关闭后不可再写入。
#[derive(Clone, Debug, Default)]
pub struct DiskRun {
    chunks: Vec<DataChunk>,
    rows: usize,
    closed: bool,
}

impl DiskRun {
    /// 追加一个非空 chunk；已关闭或空 chunk 返回错误。
    pub fn add(&mut self, chunk: DataChunk) -> Result<()> {
        if chunk.is_empty() {
            return Err(errSpillEmptyChunk());
        }
        if self.closed {
            return Err(SortError("disk run is closed".into()));
        }
        self.rows += chunk.num_rows();
        self.chunks.push(chunk);
        Ok(())
    }
    /// 已写入的 chunk 个数。
    pub fn num_chunks(&self) -> usize {
        self.chunks.len()
    }
    /// 已写入的总行数。
    pub fn num_rows(&self) -> usize {
        self.rows
    }
    /// 估算 run 当前持有的 chunk 与行数据内存。
    pub fn memory_usage(&self) -> i64 {
        self.chunks.iter().map(DataChunk::memory_usage).sum()
    }
    /// 按索引取回一个 chunk 的克隆。
    pub fn get_chunk(&self, id: usize) -> Result<DataChunk> {
        self.chunks
            .get(id)
            .cloned()
            .ok_or_else(|| SortError(format!("disk chunk {id} is out of range")))
    }
    /// 关闭 run 并清空内部 chunk，释放内存引用。
    pub fn close(&mut self) {
        self.closed = true;
        self.chunks.clear();
        self.rows = 0;
    }
    /// 消费自身，展平为行列表。
    pub fn into_rows(self) -> Vec<Row> {
        self.chunks.into_iter().flat_map(|c| c.rows).collect()
    }
}

/// 遍历 DiskRun 时的游标：记录当前 chunk 下标与行下标。
#[derive(Clone, Debug, Default)]
pub struct dataCursor {
    chkID: isize,
    rowID: usize,
    chk: Option<DataChunk>,
}

/// 构造初始游标（chkID=-1 表示尚未定位到任何 chunk）。
pub fn NewDataCursor() -> dataCursor {
    dataCursor {
        chkID: -1,
        rowID: 0,
        chk: None,
    }
}

impl dataCursor {
    /// 当前 chunk 下标。
    pub fn getChkID(&self) -> isize {
        self.chkID
    }
    /// 重置到当前 chunk 首行并返回该行。
    pub fn begin(&mut self) -> Option<Row> {
        self.rowID = 0;
        self.chk.as_ref()?.rows.first().cloned()
    }
    /// 前进到下一行并返回；越界返回 None。
    pub fn next(&mut self) -> Option<Row> {
        self.rowID += 1;
        self.chk.as_ref()?.rows.get(self.rowID).cloned()
    }
    /// 切换到指定 id 的 chunk，行下标归零。
    pub fn setChunk(&mut self, chk: DataChunk, id: isize) {
        self.chkID = id;
        self.rowID = 0;
        self.chk = Some(chk);
    }
}

/// 将游标推进到 DiskRun 的下一个 chunk；若已无更多 chunk 返回 false。
pub fn reloadCursor(cursor: &mut dataCursor, inDisk: &DiskRun) -> Result<bool> {
    let next = cursor.getChkID() + 1;
    if next as usize >= inDisk.num_chunks() {
        return Ok(false);
    }
    cursor.setChunk(inDisk.get_chunk(next as usize)?, next);
    Ok(true)
}

/// 带分区编号的行，供多路归并堆使用。
pub struct rowWithPartition {
    /// 行数据。
    pub row: Row,
    /// 所属分区编号。
    pub partitionID: usize,
}
/// 可能携带错误的行结果。
pub struct rowWithError {
    /// 成功时的行；失败时为 None。
    pub row: Option<Row>,
    /// 失败时的错误；成功时为 None。
    pub err: Option<SortError>,
}
/// 带内存用量标注的 chunk。
pub struct chunkWithMemoryUsage {
    /// chunk 数据。
    pub Chk: DataChunk,
    /// 该 chunk 估算占用字节数。
    pub MemoryUsage: i64,
}

/// 测试辅助：临时将 spillChunkSize 设为较小值，返回恢复闭包。
pub fn SetSmallSpillChunkSizeForTest(size: usize) -> impl FnOnce() {
    let old = spillChunkSize.swap(size.max(1), AtomicOrdering::SeqCst);
    move || {
        spillChunkSize.store(old, AtomicOrdering::SeqCst);
    }
}
