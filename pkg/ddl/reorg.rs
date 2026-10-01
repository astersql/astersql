// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// DDL 数据重组（reorganization / reorg）相关类型与工具。
//
// Reorg 指在线 DDL（如加索引、改列）在 schema 状态机进入 reorganization
// 阶段后，对表数据做回填（backfill）的过程：按 Key 范围扫描旧数据并写入
// 新索引或新编码行。本模块提供进度上下文、表达式/行编码配置、表范围
// 与临时索引键区间、以及是否可继续执行的检查。

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use crate::backfilling::Key;

/// 单个 reorg 作业的运行时上下文：行计数、最大进度与告警合并。
///
/// 进度以原子位模式存储 `f64`，支持多 worker 并发单调更新；
/// `resource_group_name` 用于资源组（Resource Group）限流标签。
#[derive(Debug, Default)]
pub struct ReorgContext {
    /// 已处理行数（跨 worker 累加）。
    row_count: AtomicI64,
    /// 历史最大进度的 f64 位表示，保证只升不降。
    max_progress_bits: AtomicU64,
    /// 按错误码合并的告警文案与出现次数。
    warnings: Mutex<BTreeMap<String, (String, i64)>>,
    /// 绑定的资源组名，供 TopSQL / 限流打标签。
    pub resource_group_name: String,
}

impl ReorgContext {
    /// 覆盖设置已处理行数。
    pub fn set_row_count(&self, count: i64) {
        self.row_count.store(count, Ordering::Release);
    }
    /// 原子累加已处理行数。
    pub fn increase_row_count(&self, count: i64) {
        self.row_count.fetch_add(count, Ordering::AcqRel);
    }
    /// 读取当前已处理行数。
    pub fn row_count(&self) -> i64 {
        self.row_count.load(Ordering::Acquire)
    }

    /// 将一批告警按错误码合并进上下文（文案覆盖，次数累加）。
    pub fn merge_warnings(
        &self,
        warnings: &BTreeMap<String, String>,
        counts: &BTreeMap<String, i64>,
    ) {
        if warnings.is_empty() || counts.is_empty() {
            return;
        }
        let mut merged = self.warnings.lock().expect("warnings lock poisoned");
        for (code, warning) in warnings {
            let entry = merged
                .entry(code.clone())
                .or_insert_with(|| (warning.clone(), 0));
            entry.1 += counts.get(code).copied().unwrap_or(0);
        }
    }

    /// 取出并清空已合并的告警。
    pub fn take_warnings(&self) -> BTreeMap<String, (String, i64)> {
        std::mem::take(&mut *self.warnings.lock().expect("warnings lock poisoned"))
    }

    /// CAS 更新为更大值，返回更新后的最大进度。
    pub fn set_max_progress(&self, new_progress: f64) -> f64 {
        // 并发下用 compare_exchange 保证进度单调递增。
        let mut current = self.max_progress_bits.load(Ordering::Acquire);
        loop {
            let current_value = f64::from_bits(current);
            if new_progress <= current_value {
                return current_value;
            }
            match self.max_progress_bits.compare_exchange_weak(
                current,
                new_progress.to_bits(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return new_progress,
                Err(actual) => current = actual,
            }
        }
    }
}

/// SQL 模式：严格模式把截断/非法 NULL 等视为错误，非严格则降级为 warning。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqlMode {
    /// 严格模式。
    Strict,
    /// 非严格模式（告警代替错误）。
    NonStrict,
}

/// Reorg 回填时求值表达式所用的告警策略与时区偏移。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReorgExpressionContext {
    /// 截断是否记为 warning 而非错误。
    pub truncate_as_warning: bool,
    /// 非法 NULL 是否记为 warning。
    pub bad_null_as_warning: bool,
    /// 除零是否记为 warning。
    pub division_by_zero_as_warning: bool,
    /// 会话时区相对 UTC 的秒偏移。
    pub time_zone_offset_seconds: i32,
}

/// 按 SQL 模式构造表达式上下文：非严格模式打开三类 warning 开关。
pub fn new_reorg_expression_context(
    mode: SqlMode,
    time_zone_offset_seconds: i32,
) -> ReorgExpressionContext {
    let warning = mode == SqlMode::NonStrict;
    ReorgExpressionContext {
        truncate_as_warning: warning,
        bad_null_as_warning: warning,
        division_by_zero_as_warning: warning,
        time_zone_offset_seconds,
    }
}

/// 行编码相关开关：新行格式与行级 checksum。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReorgRowEncodingConfig {
    /// 是否启用新版行编码器（非 RowFormat V1）。
    pub row_encoder_enabled: bool,
    /// 是否启用行级校验和。
    pub row_level_checksum_enabled: bool,
}

/// 回填写行时的会话侧上下文：表达式、编码、缓冲与连接属性。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReorgTableMutateContext {
    /// 表达式求值告警策略。
    expression_context: ReorgExpressionContext,
    /// 当前行编码配置。
    row_encoding_config: ReorgRowEncodingConfig,
    /// 可复用的写行字节缓冲。
    mutate_buffers: Vec<Vec<u8>>,
    /// 模拟连接 ID。
    connection_id: u64,
    /// 是否处于受限 SQL 路径。
    restricted_sql: bool,
    /// 是否开启事务断言（txn assertion）。
    txn_assertion_enabled: bool,
    /// 自增/分片 ID 预分配步长。
    shard_allocate_step: i64,
    /// 预留行 ID 剩余数量。
    reserved_row_ids: usize,
}

impl ReorgTableMutateContext {
    /// 访问表达式上下文。
    pub fn expression_context(&self) -> &ReorgExpressionContext {
        &self.expression_context
    }

    /// 返回连接 ID。
    pub const fn connection_id(&self) -> u64 {
        self.connection_id
    }

    /// 是否处于受限 SQL。
    pub const fn in_restricted_sql(&self) -> bool {
        self.restricted_sql
    }

    /// 事务断言是否开启。
    pub const fn txn_assertion_enabled(&self) -> bool {
        self.txn_assertion_enabled
    }

    /// 分片分配步长。
    pub const fn shard_allocate_step(&self) -> i64 {
        self.shard_allocate_step
    }

    /// 预留行 ID 是否已耗尽。
    pub const fn reserved_row_id_exhausted(&self) -> bool {
        self.reserved_row_ids == 0
    }

    /// 可变借用写行缓冲。
    pub fn mutate_buffers_mut(&mut self) -> &mut Vec<Vec<u8>> {
        &mut self.mutate_buffers
    }

    /// 访问行编码配置。
    pub const fn row_encoding_config(&self) -> &ReorgRowEncodingConfig {
        &self.row_encoding_config
    }

    /// 按会话 row_format 刷新编码器与 checksum 开关。
    pub fn refresh_row_encoding_config(&mut self, row_format: i64) {
        let enabled = row_format != astersql_sessionctx_vardef::DefTiDBRowFormatV1;
        self.row_encoding_config = ReorgRowEncodingConfig {
            row_encoder_enabled: enabled,
            row_level_checksum_enabled: enabled,
        };
    }
}

/// 用给定表达式上下文构造默认 mutate 上下文，并按全局 DDL reorg row format 刷新编码。
pub fn new_reorg_table_mutate_context(
    expression_context: ReorgExpressionContext,
) -> ReorgTableMutateContext {
    let mut context = ReorgTableMutateContext {
        expression_context,
        row_encoding_config: ReorgRowEncodingConfig {
            row_encoder_enabled: false,
            row_level_checksum_enabled: false,
        },
        mutate_buffers: Vec::new(),
        connection_id: 0,
        restricted_sql: false,
        txn_assertion_enabled: false,
        shard_allocate_step: astersql_sessionctx_vardef::DefTiDBShardAllocateStep,
        reserved_row_ids: 0,
    };
    context.refresh_row_encoding_config(astersql_sessionctx_vardef::GetDDLReorgRowFormat());
    context
}

/// Reorg 处理的单个元素（通常是索引）：ID 与元素类型字节。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReorgElement {
    /// 元素 ID（如索引 ID）。
    pub id: i64,
    /// 元素类型编码（区分索引/列等）。
    pub element_type: Vec<u8>,
}

/// 一次 reorg 任务的范围与元素描述：作业 ID、物理表、起止 Key、待处理元素。
///
/// Key 为 KV 键；物理表 ID 在分区表场景对应分区的 physical table id。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReorgInfo {
    /// DDL job ID。
    pub job_id: i64,
    /// 物理表 / 分区 ID。
    pub physical_table_id: i64,
    /// 回填扫描起始 Key（含）。
    pub start_key: Key,
    /// 回填扫描结束 Key（通常不含）。
    pub end_key: Key,
    /// 当前主元素（兼容单元素路径）。
    pub element: ReorgElement,
    /// 多元素时的完整列表；为空则仅用 `element`。
    pub elements: Vec<ReorgElement>,
    /// 是否处于合并临时索引阶段（进度权重减半）。
    pub merging_temporary_index: bool,
}

impl ReorgInfo {
    /// 返回待处理元素 ID 列表。
    pub fn element_ids(&self) -> Vec<i64> {
        self.elements
            .iter()
            .filter(|element| element.element_type == b"_idx_")
            .map(|element| element.id)
            .collect()
    }
    /// 检查点推进：更新下一批次的起始 Key。
    pub fn update_reorg_meta(&mut self, start_key: Key) {
        self.start_key = start_key;
    }
}

/// `is_reorg_runnable` 判定不可继续执行时的原因。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReorgRunnableError {
    /// 作业已取消。
    Cancelled,
    /// 作业已暂停。
    Paused,
    /// 本实例不是 DDL owner。
    NotOwner,
    /// 服务器正在关闭。
    ServerShuttingDown,
}

/// 检查当前是否允许继续跑 reorg（未取消/未暂停/是 owner/未关机）。
pub fn is_reorg_runnable(
    cancelled: bool,
    paused: bool,
    owner: bool,
    shutting_down: bool,
) -> Result<(), ReorgRunnableError> {
    if cancelled {
        return Err(ReorgRunnableError::Cancelled);
    }
    if paused {
        return Err(ReorgRunnableError::Paused);
    }
    if !owner {
        return Err(ReorgRunnableError::NotOwner);
    }
    if shutting_down {
        return Err(ReorgRunnableError::ServerShuttingDown);
    }
    Ok(())
}

/// 按已处理行数与估计总行数计算回填进度；临时索引合并使用独立指标标签但不改变比率。
pub fn update_backfill_progress(
    row_count: i64,
    estimated_row_count: i64,
    _merging_temporary_index: bool,
) -> f64 {
    if estimated_row_count <= 0 {
        return 0.0;
    }
    (row_count.max(0) as f64 / estimated_row_count as f64).clamp(0.0, 1.0)
}

/// 返回表 handle（行标识）最大值；第二项为是否空表。
pub fn table_max_handle(handles: &[i64]) -> (Option<i64>, bool) {
    (handles.iter().copied().max(), handles.is_empty())
}

/// 由记录前缀与 handle 列表拼出扫描 `[start, end)` 范围；空 handle 返回 None。
pub fn table_range(
    record_prefix: &[u8],
    record_prefix_end: &[u8],
    handles: &[Key],
) -> Option<(Key, Key)> {
    if handles.is_empty() {
        return None;
    }
    // 取最小 handle 作为 start，end 用 record_prefix_end。
    let mut sorted = handles.to_vec();
    sorted.sort();
    Some((
        record_prefix
            .iter()
            .chain(sorted[0].iter())
            .copied()
            .collect(),
        record_prefix_end.to_vec(),
    ))
}

/// 编码临时索引 KV 键区间：`t` + 物理表 ID + 索引 ID 范围。
pub fn encode_temporary_index_range(
    physical_id: i64,
    first_index_id: i64,
    last_index_id: i64,
) -> (Key, Key) {
    const TEMP_INDEX_PREFIX: i64 = 0x7fff_0000_0000_0000;
    fn encode_index_seek_key(table_id: i64, index_id: i64, suffix: &[u8]) -> Key {
        let mut key = b"t".to_vec();
        key.extend_from_slice(&((table_id as u64) ^ (1_u64 << 63)).to_be_bytes());
        key.extend_from_slice(b"_i");
        key.extend_from_slice(&((index_id as u64) ^ (1_u64 << 63)).to_be_bytes());
        key.extend_from_slice(suffix);
        key
    }
    let start = encode_index_seek_key(physical_id, TEMP_INDEX_PREFIX | first_index_id, &[]);
    let end = encode_index_seek_key(physical_id, TEMP_INDEX_PREFIX | last_index_id, &[u8::MAX]);
    (start, end)
}

/// 为相邻临时索引元素生成 Region split 用的边界 Key。
pub fn split_keys_for_temporary_index_ranges(
    physical_id: i64,
    elements: &[ReorgElement],
) -> Vec<Key> {
    elements
        .iter()
        .filter(|element| element.element_type == b"_idx_")
        .map(|element| encode_temporary_index_range(physical_id, element.id, element.id).0)
        .collect()
}

/// 按 (job_id, element_type, element_id) 登记/查询 reorg 句柄信息。
#[derive(Clone, Debug, Default)]
pub struct ReorgHandler {
    /// 内存中的 reorg 句柄表。
    handles: BTreeMap<(i64, Vec<u8>, i64), ReorgInfo>,
}

impl ReorgHandler {
    /// 注册或覆盖一条 ReorgInfo。
    pub fn init_ddl_reorg_handle(&mut self, info: ReorgInfo) {
        self.handles.insert(
            (
                info.job_id,
                info.element.element_type.clone(),
                info.element.id,
            ),
            info,
        );
    }
    /// 按元素列表移除指定 job 的句柄。
    pub fn remove_ddl_reorg_handles(&mut self, job_id: i64, elements: &[ReorgElement]) {
        for element in elements {
            self.handles
                .remove(&(job_id, element.element_type.clone(), element.id));
        }
    }
    /// 查询指定 job 与元素的 ReorgInfo。
    pub fn get_ddl_reorg_handle(&self, job_id: i64, element: &ReorgElement) -> Option<&ReorgInfo> {
        self.handles
            .get(&(job_id, element.element_type.clone(), element.id))
    }
}

/// 兼容旧版 reorg meta（version=0）：在 end_key 末尾补 0 字节以对齐区间语义。
pub fn adjust_end_key_across_version(reorg_meta_version: Option<u32>, mut end_key: Key) -> Key {
    if reorg_meta_version == Some(0) {
        end_key.push(0);
    }
    end_key
}

/// Durable reorg state recovered by an owner from the job and SQL system table.
pub struct PersistentReorgContext {
    pub snapshot_ver: u64,
    pub info: ReorgInfo,
    pub runtime: ReorgContext,
}
/// SQL reorg handler; it never uses the legacy in-memory handle map.
pub struct PersistentReorgHandler;
impl PersistentReorgHandler {
    /// Initialize in an independent transaction, matching Go initDDLReorgHandle.
    pub fn initialize(
        session: &mut dyn crate::job_worker::DurableJobSession,
        info: &ReorgInfo,
    ) -> Result<(), String> {
        let meta = serde_json::json!({"reorg_checkpoint": {"local_sync_key": null, "local_key_count":0,"global_sync_key":null,"global_key_count":0,"instance_addr":"","physical_id":info.physical_table_id,"ts":0,"version":1}}).to_string();
        reorg_transaction(session, |session| {
            session.query(
                &format!(
                    "delete from mysql.tidb_ddl_reorg where job_id = {}",
                    info.job_id
                ),
                "init_handle",
            )?;
            session.query(&format!("insert into mysql.tidb_ddl_reorg(job_id,ele_id,ele_type,start_key,end_key,physical_id,reorg_meta) values ({},{},X'{}',X'{}',X'{}',{},X'{}')",info.job_id,info.element.id,key_hex(&info.element.element_type),key_hex(&info.start_key),key_hex(&info.end_key),info.physical_table_id,key_hex(meta.as_bytes())),"init_handle")?;
            Ok(())
        })
    }
    pub fn restore(
        session: &mut dyn crate::job_worker::DurableJobSession,
        job: &mut astersql_meta_model::group_3::Job,
    ) -> Result<PersistentReorgContext, String> {
        restore_reorg(job, |sql| session.query(sql, "get_handle"))
    }
    /// Stage only; callers needing conflict injection own the transaction boundary.
    pub fn stage_update(
        session: &mut dyn crate::job_worker::DurableJobSession,
        info: &ReorgInfo,
        start: &[u8],
    ) -> Result<(), String> {
        session.query(&format!("update mysql.tidb_ddl_reorg set ele_id={},ele_type=X'{}',start_key=X'{}',end_key=X'{}',physical_id={} where job_id={}", info.element.id,key_hex(&info.element.element_type),key_hex(start),key_hex(&info.end_key),info.physical_table_id,info.job_id),"update_handle")?;
        Ok(())
    }
    /// Publish the in-memory cursor only after the independent commit succeeds.
    pub fn update(
        session: &mut dyn crate::job_worker::DurableJobSession,
        info: &mut ReorgInfo,
        start: Vec<u8>,
    ) -> Result<(), String> {
        if start.is_empty() && info.end_key.is_empty() {
            return Ok(());
        }
        reorg_transaction(session, |session| Self::stage_update(session, info, &start))?;
        info.start_key = start;
        Ok(())
    }
    /// Paused, cancelling and rolling-back jobs retain their recovery record.
    pub fn cleanup(
        session: &mut dyn crate::job_worker::DurableJobSession,
        job: &astersql_meta_model::group_3::Job,
    ) -> Result<(), String> {
        use astersql_meta_model::group_3::JobState;
        if !matches!(
            job.state,
            JobState::Done | JobState::Synced | JobState::Cancelled | JobState::RollbackDone
        ) {
            return Ok(());
        }
        reorg_transaction(session, |session| {
            session.query(
                &format!("delete from mysql.tidb_ddl_reorg where job_id={}", job.id),
                "clean_handle",
            )?;
            Ok(())
        })
    }
}
fn key_hex(key: &[u8]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}
fn decode_key_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
        return Err("invalid reorg key hex".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|p| {
            let a = (p[0] as char).to_digit(16).ok_or("invalid reorg key hex")?;
            let b = (p[1] as char).to_digit(16).ok_or("invalid reorg key hex")?;
            Ok((a * 16 + b) as u8)
        })
        .collect()
}
fn reorg_transaction(
    session: &mut dyn crate::job_worker::DurableJobSession,
    operation: impl FnOnce(&mut dyn crate::job_worker::DurableJobSession) -> Result<(), String>,
) -> Result<(), String> {
    if let Err(error) = session.begin() {
        session.rollback();
        return Err(error);
    }
    let result = operation(session).and_then(|_| session.commit());
    if result.is_err() {
        session.rollback();
    }
    result
}
pub(crate) fn restore_reorg(
    job: &mut astersql_meta_model::group_3::Job,
    mut query: impl FnMut(&str) -> Result<Vec<Vec<String>>, String>,
) -> Result<PersistentReorgContext, String> {
    // The session ABI uses UTF-8 rows. HEX preserves arbitrary binary handles.
    let rows = query(&format!(
        "select ele_id,HEX(ele_type),HEX(start_key),HEX(end_key),physical_id from mysql.tidb_ddl_reorg where job_id={}",
        job.id
    ))?;
    let Some(row) = rows.first() else {
        // Go restarts initialization when upgrading a job with no element row.
        job.snapshot_ver = 0;
        return Err("DDL reorg element does not exist".into());
    };
    if row.len() != 5 {
        return Err("invalid DDL reorg row".into());
    }
    let info = ReorgInfo {
        job_id: job.id,
        physical_table_id: row[4].parse().map_err(|_| "invalid reorg physical ID")?,
        start_key: decode_key_hex(&row[2])?,
        end_key: adjust_end_key_across_version(
            job.reorg_meta.as_ref().map(|m| m.Version as u32),
            decode_key_hex(&row[3])?,
        ),
        element: ReorgElement {
            id: row[0].parse().map_err(|_| "invalid reorg element ID")?,
            element_type: decode_key_hex(&row[1])?,
        },
        ..Default::default()
    };
    let runtime = ReorgContext {
        resource_group_name: job
            .reorg_meta
            .as_ref()
            .map(|m| m.ResourceGroupName.clone())
            .unwrap_or_default(),
        ..Default::default()
    };
    runtime.set_row_count(job.get_row_count());
    Ok(PersistentReorgContext {
        snapshot_ver: job.snapshot_ver,
        info,
        runtime,
    })
}
