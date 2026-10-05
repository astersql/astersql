// Copyright 2026 AsterSQL.

// Hash Join Probe 阶段的公共基础设施。
//
// 定义 Hash Join 上下文（build 哈希表、Joiner）、Probe trait、BaseJoinProbe 状态机，
// 以及按 JoinType 工厂创建具体 Probe。Probe（探测）侧逐批读入行，用序列化键在
// build 哈希表中查找候选匹配行。

use crate::joiner::{JoinType, Joiner, Row};
use crate::row_table_builder::Value;
use std::collections::BTreeMap;

/// 批量构造 build 行时的建议批大小（对应 Go 侧常量语义）。
pub const BATCH_BUILD_ROW_SIZE: usize = 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Join 键的存储/比较模式：单 int64、定长序列化或变长序列化。
pub enum KeyMode {
    /// 键为单个 int64（前 8 字节比较）。
    OneInt64,
    /// 定长序列化字节串整体比较。
    FixedSerialized,
    /// 变长序列化字节串整体比较。
    VariableSerialized,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 字节缓冲中一段连续区间的偏移与长度。
pub struct OffsetAndLength {
    /// 起始偏移。
    pub offset: usize,
    /// 字节长度。
    pub length: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 一次 key 匹配命中时，build 行与 probe 行的下标对。
pub struct MatchedRowInfo {
    /// build 侧行下标。
    pub build_row_index: usize,
    /// probe 侧行下标。
    pub probe_row_index: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 行位置及其哈希值，用于哈希链遍历。
pub struct PositionAndHash {
    /// 行在表中的位置。
    pub position: usize,
    /// 对应键的哈希值。
    pub hash_value: u64,
}

#[derive(Clone)]
/// Hash Join 共享上下文：build 行、哈希表、Joiner 与输出容量等。
pub struct HashJoinContext {
    /// build 侧全部行。
    pub build_rows: Vec<Row>,
    /// build 侧 join 键列下标。
    pub build_key_indices: Vec<usize>,
    /// probe 侧 join 键列下标。
    pub probe_key_indices: Vec<usize>,
    /// 哈希桶：hash → build 行下标列表。
    pub hash_table: BTreeMap<u64, Vec<usize>>,
    /// 标记各 build 行是否已被 probe 命中使用。
    pub build_row_used: Vec<bool>,
    /// 负责按 JoinType 生成匹配/未匹配输出的 Joiner。
    pub joiner: Joiner,
    /// 是否以右侧作为 build side。
    pub right_as_build_side: bool,
    /// 是否保留被过滤的行（部分 outer/anti 路径需要）。
    pub keep_filtered_rows: bool,
    /// 单次输出 Chunk 的最大行数。
    pub max_chunk_size: usize,
}
impl HashJoinContext {
    /// 用 build 行构建哈希表并初始化 used 标记。
    ///
    /// 键含 NULL 的 build 行不进入哈希表（等值连接忽略 NULL 键）。
    pub fn new(
        build_rows: Vec<Row>,
        build_key_indices: Vec<usize>,
        probe_key_indices: Vec<usize>,
        joiner: Joiner,
        right_as_build_side: bool,
        keep_filtered_rows: bool,
        max_chunk_size: usize,
    ) -> Self {
        let mut hash_table = BTreeMap::<u64, Vec<usize>>::new();
        // 序列化非 NULL 键并插入哈希桶。
        for (index, row) in build_rows.iter().enumerate() {
            let key = serialize_key(row, &build_key_indices);
            if !key_has_null(row, &build_key_indices) {
                hash_table.entry(hash_bytes(&key)).or_default().push(index);
            }
        }
        Self {
            build_row_used: vec![false; build_rows.len()],
            build_rows,
            build_key_indices,
            probe_key_indices,
            hash_table,
            joiner,
            right_as_build_side,
            keep_filtered_rows,
            max_chunk_size,
        }
    }
    /// 是否存在 join 之外的 other condition（非“无条件半连接”）。
    pub fn has_other_condition(&self) -> bool {
        !self.joiner.is_semi_join_without_condition()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// Probe worker 单次产出：结果行列表与可选错误。
pub struct WorkerResult {
    /// 本批输出行。
    pub rows: Vec<Row>,
    /// 执行错误（若有）。
    pub error: Option<String>,
}

/// Hash Join Probe 阶段统一接口，由各 JoinType 的具体 Probe 实现。
pub trait Probe {
    /// 装载本批 probe 行并完成键序列化与候选查找。
    fn set_chunk_for_probe(&mut self, chunk: Vec<Row>) -> Result<(), String>;
    /// 装载从磁盘恢复的 probe Chunk。
    fn set_restored_chunk_for_probe(&mut self, chunk: Vec<Row>) -> Result<(), String>;
    /// 刷出尚未处理的 probe 行供 spill。
    fn spill_remaining_probe_chunks(&mut self) -> Vec<Vec<Row>>;
    /// 推进 probe，产出一批结果。
    fn probe(&mut self) -> WorkerResult;
    /// 是否需要事后扫描 build 行表（如左 build 的 anti/semi）。
    fn need_scan_row_table(&self) -> bool;
    /// 初始化 build 行表扫描。
    fn init_for_scan_row_table(&mut self);
    /// 扫描 build 行表并输出符合条件的行。
    fn scan_row_table(&mut self) -> WorkerResult;
    /// build 行表扫描是否完成。
    fn is_scan_row_table_done(&self) -> bool;
    /// 当前 probe Chunk 是否已处理完。
    fn is_current_chunk_probe_done(&self) -> bool;
    /// 重置 probe 状态。
    fn reset_probe(&mut self);
}

#[derive(Clone)]
/// 各具体 Probe 共享的基础状态：当前 Chunk、哈希/序列化键、候选匹配与游标。
pub struct BaseJoinProbe {
    /// 共享 Hash Join 上下文。
    pub context: HashJoinContext,
    /// 所属 probe worker 编号。
    pub worker_id: usize,
    /// 当前 probe Chunk 的行。
    pub probe_chunk: Vec<Row>,
    /// 各 probe 行键的哈希值。
    pub hash_values: Vec<u64>,
    /// 各 probe 行序列化后的 join 键。
    pub serialized_keys: Vec<Vec<u8>>,
    /// 各 probe 行在哈希表中命中的 build 行下标列表。
    pub matched_rows: Vec<Vec<usize>>,
    /// 当前正在处理的 probe 行下标。
    pub current_probe_row: usize,
    /// 当前 probe 行内候选匹配的推进下标。
    pub current_candidate: usize,
    /// probe 过程中的哈希冲突计数。
    pub probe_collision: u64,
    /// 因 spill 暂存的未处理 probe Chunk。
    pub spilled_chunks: Vec<Vec<Row>>,
    /// 扫描 build 行表时的游标。
    pub scan_row_index: usize,
}
impl BaseJoinProbe {
    /// 创建空的 BaseJoinProbe，绑定上下文与 worker id。
    pub fn new(context: HashJoinContext, worker_id: usize) -> Self {
        Self {
            context,
            worker_id,
            probe_chunk: Vec::new(),
            hash_values: Vec::new(),
            serialized_keys: Vec::new(),
            matched_rows: Vec::new(),
            current_probe_row: 0,
            current_candidate: 0,
            probe_collision: 0,
            spilled_chunks: Vec::new(),
            scan_row_index: 0,
        }
    }
    /// 装载 probe Chunk：校验键下标、序列化键、查哈希表并过滤真匹配。
    pub fn set_chunk_for_probe(&mut self, chunk: Vec<Row>) -> Result<(), String> {
        if !self.is_current_chunk_probe_done() {
            return Err("previous chunk is not probed yet".into());
        }
        self.probe_chunk = chunk;
        self.hash_values.clear();
        self.serialized_keys.clear();
        self.matched_rows.clear();
        for (row_index, row) in self.probe_chunk.iter().enumerate() {
            if self
                .context
                .probe_key_indices
                .iter()
                .any(|index| *index >= row.len())
            {
                return Err(format!("probe row {row_index} key index out of range"));
            }
            let key = serialize_key(row, &self.context.probe_key_indices);
            let hash = hash_bytes(&key);
            // NULL 键无候选；否则在同哈希桶内再按完整键过滤。
            let candidates = if key_has_null(row, &self.context.probe_key_indices) {
                Vec::new()
            } else {
                self.context
                    .hash_table
                    .get(&hash)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|index| {
                        serialize_key(
                            &self.context.build_rows[*index],
                            &self.context.build_key_indices,
                        ) == key
                    })
                    .collect()
            };
            self.serialized_keys.push(key);
            self.hash_values.push(hash);
            self.matched_rows.push(candidates);
        }
        self.current_probe_row = 0;
        self.current_candidate = 0;
        Ok(())
    }
    /// 恢复 Chunk 与普通装载路径相同。
    pub fn set_restored_chunk_for_probe(&mut self, chunk: Vec<Row>) -> Result<(), String> {
        self.set_chunk_for_probe(chunk)
    }
    /// 将当前 Chunk 中尚未处理的尾部行加入 spill，并取出全部 spill 缓冲。
    pub fn spill_remaining_probe_chunks(&mut self) -> Vec<Vec<Row>> {
        if self.current_probe_row < self.probe_chunk.len() {
            self.spilled_chunks
                .push(self.probe_chunk[self.current_probe_row..].to_vec());
        }
        self.current_probe_row = self.probe_chunk.len();
        self.current_candidate = 0;
        std::mem::take(&mut self.spilled_chunks)
    }
    /// 是否已处理完当前 probe Chunk 的全部行。
    pub fn is_current_chunk_probe_done(&self) -> bool {
        self.current_probe_row >= self.probe_chunk.len()
    }
    /// 结束当前 probe 行的查找循环，推进到下一行。
    pub fn finish_current_lookup_loop(&mut self) {
        self.current_probe_row += 1;
        self.current_candidate = 0;
    }
    /// 清空本批 probe 相关缓冲与游标（不含 build 表 used 标记）。
    pub fn reset_probe(&mut self) {
        self.probe_chunk.clear();
        self.hash_values.clear();
        self.serialized_keys.clear();
        self.matched_rows.clear();
        self.current_probe_row = 0;
        self.current_candidate = 0;
        self.probe_collision = 0;
        self.scan_row_index = 0;
    }
    /// 返回累计的 probe 哈希冲突次数。
    pub fn get_probe_collision(&self) -> u64 {
        self.probe_collision
    }
    /// 清零 probe 哈希冲突计数。
    pub fn reset_probe_collision(&mut self) {
        self.probe_collision = 0;
    }
    /// 取出指定 probe 行的全部候选 build 行内容。
    pub fn candidate_rows(&self, probe_index: usize) -> Vec<Row> {
        self.matched_rows
            .get(probe_index)
            .into_iter()
            .flatten()
            .map(|index| self.context.build_rows[*index].clone())
            .collect()
    }
    /// 将指定 probe 行的全部候选 build 行标记为已使用。
    pub fn mark_build_rows_used(&mut self, probe_index: usize) {
        if let Some(indices) = self.matched_rows.get(probe_index) {
            for index in indices {
                self.context.build_row_used[*index] = true;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 由 JoinType 归类出的 Probe 实现风格。
pub enum ProbeFlavor {
    /// 内连接。
    Inner,
    /// 半连接。
    Semi,
    /// 反半连接。
    AntiSemi,
    /// 左外半连接。
    LeftOuterSemi,
    /// 反左外半连接。
    AntiLeftOuterSemi,
    /// 外连接（左/右）。
    Outer,
}
/// 将 JoinType 映射为 ProbeFlavor。
pub fn probe_flavor(join_type: JoinType) -> ProbeFlavor {
    match join_type {
        JoinType::Inner => ProbeFlavor::Inner,
        JoinType::Semi => ProbeFlavor::Semi,
        JoinType::AntiSemi => ProbeFlavor::AntiSemi,
        JoinType::LeftOuterSemi => ProbeFlavor::LeftOuterSemi,
        JoinType::AntiLeftOuterSemi => ProbeFlavor::AntiLeftOuterSemi,
        JoinType::LeftOuter | JoinType::RightOuter | JoinType::FullOuter => ProbeFlavor::Outer,
    }
}

/// 按 JoinType 与 build side 配置构造具体 Probe 实现。
///
/// LeftOuterSemi / AntiLeftOuterSemi 要求右侧为 build side。
pub fn new_join_probe(
    context: HashJoinContext,
    worker_id: usize,
    join_type: JoinType,
    right_as_build_side: bool,
    null_aware: bool,
) -> Result<Box<dyn Probe>, String> {
    let base = BaseJoinProbe::new(context, worker_id);
    Ok(match join_type {
        JoinType::Inner => Box::new(crate::inner_join_probe::InnerJoinProbe { base }),
        JoinType::LeftOuter => Box::new(crate::outer_join_probe::OuterJoinProbe {
            base,
            outer_side_build: !right_as_build_side,
            right_side_build: right_as_build_side,
        }),
        JoinType::RightOuter => Box::new(crate::outer_join_probe::OuterJoinProbe {
            base,
            outer_side_build: right_as_build_side,
            right_side_build: right_as_build_side,
        }),
        JoinType::Semi => Box::new(crate::semi_join_probe::SemiJoinProbe {
            semi: crate::base_semi_join::BaseSemiJoin::new(base, !right_as_build_side),
        }),
        JoinType::AntiSemi => Box::new(crate::anti_semi_join_probe::AntiSemiJoinProbe {
            semi: crate::base_semi_join::BaseSemiJoin::new(base, !right_as_build_side),
        }),
        JoinType::LeftOuterSemi | JoinType::AntiLeftOuterSemi if right_as_build_side => {
            Box::new(crate::left_outer_semi_join_probe::LeftOuterSemiJoinProbe {
                semi: crate::base_semi_join::BaseSemiJoin::new(base, false),
                anti: join_type == JoinType::AntiLeftOuterSemi,
                null_aware,
            })
        }
        JoinType::LeftOuterSemi | JoinType::AntiLeftOuterSemi => {
            return Err("left outer semi probes require the right build side".into());
        }
        JoinType::FullOuter => {
            return Err("full outer join uses the HashJoinV1 two-joiner probe path".into());
        }
    })
}
/// 按 KeyMode 比较 probe 键与 build 键是否相等。
pub fn is_key_matched(mode: KeyMode, probe_key: &[u8], build_key: &[u8]) -> bool {
    match mode {
        KeyMode::OneInt64 => match (probe_key.get(..8), build_key.get(..8)) {
            (Some(probe), Some(build)) => probe == build,
            _ => false,
        },
        KeyMode::FixedSerialized | KeyMode::VariableSerialized => probe_key == build_key,
    }
}
/// 将指定列序列化为 join 键字节串。
fn serialize_key(row: &Row, indices: &[usize]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for index in indices {
        encode_value(&row[*index], &mut bytes);
    }
    bytes
}
/// 把单个 Value 追加编码到键缓冲。
fn encode_value(value: &Value, bytes: &mut Vec<u8>) {
    match value {
        Value::Null => bytes.push(0),
        Value::Bool(v) => bytes.push(u8::from(*v)),
        Value::Int(v) => bytes.extend_from_slice(&v.to_le_bytes()),
        Value::UInt(v) => bytes.extend_from_slice(&v.to_le_bytes()),
        Value::Float(v) => bytes.extend_from_slice(&v.to_bits().to_le_bytes()),
        Value::Bytes(v) => {
            bytes.extend_from_slice(&(v.len() as u32).to_le_bytes());
            bytes.extend_from_slice(v);
        }
        Value::Text(v) => {
            bytes.extend_from_slice(&(v.len() as u32).to_le_bytes());
            bytes.extend_from_slice(v.as_bytes());
        }
    }
}
/// 判断 join 键列中是否存在 NULL。
fn key_has_null(row: &Row, indices: &[usize]) -> bool {
    indices
        .iter()
        .any(|index| matches!(row[*index], Value::Null))
}
/// FNV-1a 风格的 64 位哈希，用于键分桶。
fn hash_bytes(bytes: &[u8]) -> u64 {
    bytes.iter().fold(1469598103934665603_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(1099511628211)
    })
}
