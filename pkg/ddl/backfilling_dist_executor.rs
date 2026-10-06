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

// 分布式回填（backfill）任务执行器模块。
//
// "回填"指在执行 DDL（数据定义语言，如新增索引）时，为表中已有的
// 存量数据补建索引条目的过程。在分布式执行框架（dist task framework）
// 中，一个回填任务会被拆分成多个子任务分发到不同节点并行执行。
// 本模块定义了：
// - 任务级与子任务级的元数据结构（`BackfillTaskMeta` / `BackfillSubTaskMeta`）
//   及其二进制序列化/反序列化逻辑；
// - 子任务元数据的外部存储读写（元数据过大时存放到云存储等外部介质）；
// - 回填流程的阶段划分（`BackfillStep`）与阶段执行器的选择逻辑
//   （`BackfillDistExecutor`）。

use crate::backfilling::Key;
use crate::backfilling_read_index::SortedKvMeta;

/// 回填任务元数据的版本号 0（旧版格式）。
pub const BACKFILL_TASK_META_VERSION_0: u32 = 0;
/// 回填任务元数据的版本号 1（当前格式），用于兼容性判断。
pub const BACKFILL_TASK_META_VERSION_1: u32 = 1;

/// 回填任务的执行摘要。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackfillTaskSummary {
    /// 全局排序路径生成的索引 KV 总字节数。
    pub index_kv_size: u64,
}

/// 回填任务的整体元数据，对应一个 DDL 作业（如新增索引）在
/// 分布式任务框架中的任务描述。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackfillTaskMeta {
    /// 关联的 DDL 作业 ID。
    pub job_id: i64,
    /// 目标表所属数据库（schema）的 ID。
    pub schema_id: i64,
    /// 目标表的 ID。
    pub table_id: i64,
    /// 待回填的元素 ID 列表（通常是索引 ID）。
    pub element_ids: Vec<i64>,
    /// 元素类型键，用于区分回填的是索引还是列等类型。
    pub element_type_key: Vec<u8>,
    /// 云存储 URI；非空表示使用全局排序（外部排序到云存储）模式。
    pub cloud_storage_uri: String,
    /// 估算的单行大小，用于任务拆分与资源预估。
    pub estimate_row_size: usize,
    /// 是否需要合并临时索引（增量数据写入的临时索引与回填数据合并）。
    pub merge_temporary_index: bool,
    /// 任务执行摘要；本地回填及尚未完成写入计划时保持为空。
    pub summary: Option<BackfillTaskSummary>,
    /// 元数据版本号，见 `BACKFILL_TASK_META_VERSION_*` 常量。
    pub version: u32,
    /// 每批处理的行数。
    pub batch_size: usize,
    /// 写入限速（字节/秒），0 表示不限速。
    pub max_write_speed: usize,
}

/// 回填子任务的元数据，描述分配给单个执行节点的一段回填工作。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackfillSubTaskMeta {
    /// 外部存储路径；非空时表示完整元数据存放在外部存储中，
    /// 需要再读取一次才能得到真实内容。
    pub external_path: String,
    /// 物理表 ID（分区表时为具体分区的 ID，普通表与表 ID 相同）。
    pub physical_table_id: i64,
    /// 本子任务负责扫描的行键（row key）区间起点。
    pub row_start: Key,
    /// 本子任务负责扫描的行键区间终点。
    pub row_end: Key,
    /// 区间内部再切分出的作业键列表，用于并行处理。
    pub range_job_keys: Vec<Key>,
    /// 用于 Region（存储层的数据分片单元）预切分的键列表。
    pub range_split_keys: Vec<Key>,
    /// 全局排序阶段产生的数据文件路径列表。
    pub data_files: Vec<String>,
    /// 与数据文件对应的统计文件路径列表。
    pub stat_files: Vec<String>,
    /// 读取数据时使用的时间戳（TSO），保证读取快照的一致性。
    pub ts: u64,
    /// 按元素分组的有序 KV 元信息（每个索引一组）。
    pub meta_groups: Vec<SortedKvMeta>,
    /// 本子任务涉及的元素（索引）ID 列表。
    pub element_ids: Vec<i64>,
    /// 旧版格式中单一的有序 KV 元信息，仅为向后兼容保留。
    pub legacy_sorted_kv_meta: SortedKvMeta,
}

/// 元数据序列化/反序列化过程中可能出现的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetaError {
    /// 输入字节不足，数据被截断。
    Truncated,
    /// 字符串字段不是合法的 UTF-8 编码。
    InvalidUtf8,
    /// 魔数或版本号不匹配，携带读到的版本字节。
    InvalidVersion(u8),
    /// 外部存储读写失败，携带错误描述。
    External(String),
}

impl BackfillSubTaskMeta {
    /// 将子任务元数据序列化为自定义二进制格式。
    ///
    /// 格式为：魔数 "ABFM" + 版本字节 0x01，随后按固定顺序写入各字段，
    /// 变长字段（字符串、字节串、列表）均以小端 u64 长度前缀编码。
    pub fn marshal(&self) -> Vec<u8> {
        if self.external_path.is_empty() {
            return self.marshal_all();
        }

        // Match BaseExternalMeta.Marshal in Go: once ExternalPath is set, the
        // task payload contains only the non-external fields plus the pointer.
        let mut internal = self.clone();
        internal.range_job_keys.clear();
        internal.range_split_keys.clear();
        internal.data_files.clear();
        internal.stat_files.clear();
        internal.meta_groups.clear();
        internal.element_ids.clear();
        internal.legacy_sorted_kv_meta = SortedKvMeta::default();
        internal.marshal_all()
    }

    fn marshal_all(&self) -> Vec<u8> {
        let mut output = Vec::new();
        // 写入魔数与版本号，供反序列化时校验格式。
        output.extend_from_slice(b"ABFM\x01");
        put_string(&mut output, &self.external_path);
        put_i64(&mut output, self.physical_table_id);
        put_bytes(&mut output, &self.row_start);
        put_bytes(&mut output, &self.row_end);
        put_keys(&mut output, &self.range_job_keys);
        put_keys(&mut output, &self.range_split_keys);
        put_strings(&mut output, &self.data_files);
        put_strings(&mut output, &self.stat_files);
        put_u64(&mut output, self.ts);
        put_sorted_groups(&mut output, &self.meta_groups);
        put_i64s(&mut output, &self.element_ids);
        put_sorted_meta(&mut output, &self.legacy_sorted_kv_meta);
        output
    }

    /// 从二进制数据反序列化出子任务元数据，是 `marshal` 的逆操作。
    ///
    /// 先校验魔数与版本号，再按序读取各字段，最后执行兼容性修正。
    pub fn unmarshal(raw: &[u8]) -> Result<Self, MetaError> {
        let mut input = raw;
        // 校验前 5 字节的魔数与版本号，不匹配则返回版本错误。
        if take(&mut input, 5)? != b"ABFM\x01" {
            return Err(MetaError::InvalidVersion(raw.get(4).copied().unwrap_or(0)));
        }
        let mut meta = Self {
            external_path: take_string(&mut input)?,
            physical_table_id: take_i64(&mut input)?,
            row_start: take_bytes(&mut input)?,
            row_end: take_bytes(&mut input)?,
            range_job_keys: take_keys(&mut input)?,
            range_split_keys: take_keys(&mut input)?,
            data_files: take_strings(&mut input)?,
            stat_files: take_strings(&mut input)?,
            ts: take_u64(&mut input)?,
            meta_groups: take_sorted_groups(&mut input)?,
            element_ids: take_i64s(&mut input)?,
            legacy_sorted_kv_meta: take_sorted_meta(&mut input)?,
        };
        Ok(meta)
    }

    /// 兼容旧版元数据：旧格式只有 `legacy_sorted_kv_meta` 单一元信息，
    /// 这里将其填充到新字段（行键区间与分组元信息）中。
    fn apply_compatibility(&mut self) {
        // 行键区间为空说明是旧格式，从旧字段中恢复起止键。
        if self.row_start.is_empty() {
            self.row_start
                .clone_from(&self.legacy_sorted_kv_meta.start_key);
            self.row_end.clone_from(&self.legacy_sorted_kv_meta.end_key);
        }
        if self.meta_groups.is_empty() {
            self.meta_groups.push(self.legacy_sorted_kv_meta.clone());
        }
    }
}

/// 外部元数据存储的抽象接口。
///
/// 当子任务元数据体积过大（如包含大量文件路径）时，会被写入外部存储
/// （如云对象存储），元数据本体中只保留一个路径引用。
pub trait ExternalMetaStorage {
    /// 按路径读取外部存储中的原始字节。
    fn read(&self, path: &str) -> Result<Vec<u8>, String>;
    /// 将字节写入外部存储的指定路径。
    fn write(&mut self, path: &str, value: &[u8]) -> Result<(), String>;
}

/// 解码子任务元数据；若其中记录了外部存储路径，则再从外部存储
/// 读取并解码出完整的元数据。
pub fn decode_backfill_subtask_meta(
    storage: Option<&dyn ExternalMetaStorage>,
    raw: &[u8],
) -> Result<BackfillSubTaskMeta, MetaError> {
    let mut meta = BackfillSubTaskMeta::unmarshal(raw)?;
    // 外部路径非空且提供了存储实现时，说明真实元数据在外部存储，
    // 需要二次读取并重新解码。
    if let (Some(storage), false) = (storage, meta.external_path.is_empty()) {
        let external = storage
            .read(&meta.external_path)
            .map_err(MetaError::External)?;
        let external = BackfillSubTaskMeta::unmarshal(&external)?;
        meta.range_job_keys = external.range_job_keys;
        meta.range_split_keys = external.range_split_keys;
        meta.data_files = external.data_files;
        meta.stat_files = external.stat_files;
        meta.meta_groups = external.meta_groups;
        meta.element_ids = external.element_ids;
        meta.legacy_sorted_kv_meta = external.legacy_sorted_kv_meta;
    }
    meta.apply_compatibility();
    Ok(meta)
}

/// 将子任务元数据序列化后写入外部存储，并把外部路径回填到元数据中。
/// 若未提供存储实现则直接返回成功（表示不使用外部存储）。
pub fn write_external_backfill_subtask_meta(
    storage: Option<&mut dyn ExternalMetaStorage>,
    subtask: &mut BackfillSubTaskMeta,
    path: impl Into<String>,
) -> Result<(), MetaError> {
    let Some(storage) = storage else {
        return Ok(());
    };
    subtask.external_path = path.into();
    // Go writes only fields tagged `external:"true"`; internal fields and the
    // external path itself must not be copied into the referenced payload.
    let mut external = subtask.clone();
    external.external_path.clear();
    external.physical_table_id = 0;
    external.row_start.clear();
    external.row_end.clear();
    external.ts = 0;
    storage
        .write(&subtask.external_path, &external.marshal_all())
        .map_err(MetaError::External)
}

/// 回填任务的执行阶段（step）。
///
/// 分布式回填按阶段推进：先扫描表数据生成索引 KV（ReadIndex），
/// 全局排序模式下再做归并排序（MergeSort）与写入导入
/// （WriteAndIngest），最后可能合并临时索引（MergeTemporaryIndex）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackfillStep {
    /// 初始阶段，尚未开始执行。
    Init,
    /// 读取表数据并生成索引记录的阶段。
    ReadIndex,
    /// 对外部存储中的索引数据做归并排序的阶段（全局排序模式）。
    MergeSort,
    /// 将排序后的数据写入并导入（ingest）到存储引擎的阶段。
    WriteAndIngest,
    /// 合并临时索引（回填期间的增量写入）的阶段。
    MergeTemporaryIndex,
    /// 全部完成。
    Done,
}

/// 执行器在选择或运行阶段执行器时可能返回的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutorError {
    /// 执行器尚未通过 `init` 注入任务元数据。
    NotInitialized,
    /// 元数据中引用的索引 ID 在可用索引列表中不存在。
    IndexInfoNotFound(i64),
    /// 本地导入模式（未配置云存储）没有 WriteAndIngest 阶段。
    LocalImportHasNoWriteAndIngest,
    /// 遇到不支持的阶段。
    UnknownStep,
    /// 元数据解码失败。
    Decode(MetaError),
}

/// 分布式回填执行器，负责在单个执行节点上根据任务元数据
/// 选择并驱动各阶段的执行。
#[derive(Clone, Debug)]
pub struct BackfillDistExecutor {
    /// 分布式框架分配的任务 ID。
    pub task_id: i64,
    /// 任务元数据，`init` 之后才有值。
    raw_meta: Option<BackfillTaskMeta>,
    /// 当前节点上可用（存在）的索引 ID 列表，用于校验元数据。
    available_index_ids: Vec<i64>,
    /// 是否已关闭。
    closed: bool,
}

impl BackfillDistExecutor {
    /// 创建一个尚未初始化元数据的执行器。
    pub fn new(task_id: i64, available_index_ids: Vec<i64>) -> Self {
        Self {
            task_id,
            raw_meta: None,
            available_index_ids,
            closed: false,
        }
    }

    /// 注入任务元数据并重置关闭状态，使执行器进入可用状态。
    pub fn init(&mut self, meta: BackfillTaskMeta) {
        self.raw_meta = Some(meta);
        self.closed = false;
    }

    /// 校验元数据并为给定阶段选择执行器（此处以返回阶段本身表示）。
    ///
    /// 校验内容：执行器已初始化、元数据中的索引在本节点均可用、
    /// 阶段与导入模式匹配（本地导入没有 WriteAndIngest 阶段）。
    pub fn get_step_executor(&self, step: BackfillStep) -> Result<BackfillStep, ExecutorError> {
        let meta = self
            .raw_meta
            .as_ref()
            .ok_or(ExecutorError::NotInitialized)?;
        // 逐一校验元数据中引用的索引是否在本节点可用。
        for element_id in &meta.element_ids {
            if !self.available_index_ids.contains(element_id) {
                return Err(ExecutorError::IndexInfoNotFound(*element_id));
            }
        }
        match step {
            BackfillStep::ReadIndex
            | BackfillStep::MergeSort
            | BackfillStep::MergeTemporaryIndex => Ok(step),
            // 未配置云存储即为本地导入模式，不存在写入导入阶段。
            BackfillStep::WriteAndIngest if meta.cloud_storage_uri.is_empty() => {
                Err(ExecutorError::LocalImportHasNoWriteAndIngest)
            }
            BackfillStep::WriteAndIngest => Ok(step),
            _ => Err(ExecutorError::UnknownStep),
        }
    }

    /// 回填操作是幂等的：重复执行同一子任务不会破坏正确性。
    pub const fn is_idempotent(&self) -> bool {
        true
    }

    /// 判断错误是否可重试。Go 实现仅显式排除索引元数据缺失，其他未知错误
    /// 交给 `isRetryableError(err, true)` 并按可重试处理。
    pub fn is_retryable_error(error: &ExecutorError) -> bool {
        !matches!(error, ExecutorError::IndexInfoNotFound(_))
    }

    /// 关闭执行器，释放其占用的资源（此处仅置标志位）。
    pub fn close(&mut self) {
        self.closed = true;
    }

    /// 返回执行器是否已关闭。
    pub const fn is_closed(&self) -> bool {
        self.closed
    }
}

// 以下 put_*/take_* 为二进制编解码的辅助函数：
// put_* 以小端字节序写入，变长内容带 u64 长度前缀；
// take_* 从输入切片前端按同样格式读取并推进切片。

/// 以小端字节序写入一个 u64。
fn put_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}
/// 以小端字节序写入一个 i64。
fn put_i64(output: &mut Vec<u8>, value: i64) {
    output.extend_from_slice(&value.to_le_bytes());
}
/// 写入变长字节串：先写长度前缀，再写内容。
fn put_bytes(output: &mut Vec<u8>, value: &[u8]) {
    put_u64(output, value.len() as u64);
    output.extend_from_slice(value);
}
/// 写入字符串，按 UTF-8 字节串编码。
fn put_string(output: &mut Vec<u8>, value: &str) {
    put_bytes(output, value.as_bytes());
}
/// 写入键列表：先写元素个数，再逐个写入。
fn put_keys(output: &mut Vec<u8>, values: &[Key]) {
    put_u64(output, values.len() as u64);
    for value in values {
        put_bytes(output, value);
    }
}
/// 写入字符串列表：先写元素个数，再逐个写入。
fn put_strings(output: &mut Vec<u8>, values: &[String]) {
    put_u64(output, values.len() as u64);
    for value in values {
        put_string(output, value);
    }
}
/// 写入 i64 列表：先写元素个数，再逐个写入。
fn put_i64s(output: &mut Vec<u8>, values: &[i64]) {
    put_u64(output, values.len() as u64);
    for value in values {
        put_i64(output, *value);
    }
}
/// 写入单个有序 KV 元信息（起止键、文件数、总 KV 大小）。
fn put_sorted_meta(output: &mut Vec<u8>, meta: &SortedKvMeta) {
    put_bytes(output, &meta.start_key);
    put_bytes(output, &meta.end_key);
    put_u64(output, meta.file_count as u64);
    put_u64(output, meta.total_kv_size);
}
/// 写入有序 KV 元信息列表：先写元素个数，再逐个写入。
fn put_sorted_groups(output: &mut Vec<u8>, groups: &[SortedKvMeta]) {
    put_u64(output, groups.len() as u64);
    for meta in groups {
        put_sorted_meta(output, meta);
    }
}
/// 从输入前端取出 `count` 个字节并推进切片；不足则报截断错误。
fn take<'a>(input: &mut &'a [u8], count: usize) -> Result<&'a [u8], MetaError> {
    if input.len() < count {
        return Err(MetaError::Truncated);
    }
    let (taken, rest) = input.split_at(count);
    *input = rest;
    Ok(taken)
}
/// 读取一个小端 u64。
fn take_u64(input: &mut &[u8]) -> Result<u64, MetaError> {
    Ok(u64::from_le_bytes(
        take(input, 8)?.try_into().expect("eight bytes"),
    ))
}
/// 读取一个小端 i64。
fn take_i64(input: &mut &[u8]) -> Result<i64, MetaError> {
    Ok(i64::from_le_bytes(
        take(input, 8)?.try_into().expect("eight bytes"),
    ))
}
/// 读取带长度前缀的变长字节串。
fn take_bytes(input: &mut &[u8]) -> Result<Vec<u8>, MetaError> {
    let count = usize::try_from(take_u64(input)?).map_err(|_| MetaError::Truncated)?;
    Ok(take(input, count)?.to_vec())
}
/// 读取带长度前缀的字符串，并校验 UTF-8 合法性。
fn take_string(input: &mut &[u8]) -> Result<String, MetaError> {
    String::from_utf8(take_bytes(input)?).map_err(|_| MetaError::InvalidUtf8)
}
/// 读取键列表：先读元素个数，再逐个读取。
fn take_keys(input: &mut &[u8]) -> Result<Vec<Key>, MetaError> {
    let count = usize::try_from(take_u64(input)?).map_err(|_| MetaError::Truncated)?;
    (0..count).map(|_| take_bytes(input)).collect()
}
/// 读取字符串列表：先读元素个数，再逐个读取。
fn take_strings(input: &mut &[u8]) -> Result<Vec<String>, MetaError> {
    let count = usize::try_from(take_u64(input)?).map_err(|_| MetaError::Truncated)?;
    (0..count).map(|_| take_string(input)).collect()
}
/// 读取 i64 列表：先读元素个数，再逐个读取。
fn take_i64s(input: &mut &[u8]) -> Result<Vec<i64>, MetaError> {
    let count = usize::try_from(take_u64(input)?).map_err(|_| MetaError::Truncated)?;
    (0..count).map(|_| take_i64(input)).collect()
}
/// 读取单个有序 KV 元信息。
fn take_sorted_meta(input: &mut &[u8]) -> Result<SortedKvMeta, MetaError> {
    Ok(SortedKvMeta {
        start_key: take_bytes(input)?,
        end_key: take_bytes(input)?,
        file_count: usize::try_from(take_u64(input)?).map_err(|_| MetaError::Truncated)?,
        total_kv_size: take_u64(input)?,
    })
}
/// 读取有序 KV 元信息列表：先读元素个数，再逐个读取。
fn take_sorted_groups(input: &mut &[u8]) -> Result<Vec<SortedKvMeta>, MetaError> {
    let count = usize::try_from(take_u64(input)?).map_err(|_| MetaError::Truncated)?;
    (0..count).map(|_| take_sorted_meta(input)).collect()
}
