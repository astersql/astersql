// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// TopSQL reporter 数据模型：按 SQL/Plan digest 聚合时间序列指标并导出 tipb。
//
// 核心类型包括按时间戳对齐的 `Record`/`TsItem`、收集中的 `Collecting`、
// Top-N 裁剪的 `Records`/`CpuRecords`，以及带容量上限的规范化 SQL/Plan 元数据表。
// 「Others」条目用空 digest 汇总被淘汰的低贡献 SQL。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::tipb_protobuf as tipb;

/// 「Others」聚合记录的 map 键：空字节，表示被 Top-N 淘汰后的汇总桶。
const KEY_OTHERS: &[u8] = b"";
/// 单条 Record 内时间戳条目容量上限，防止精度过细时无界增长。
const MAX_TS_ITEMS_CAPACITY: usize = 1000;

/// 因超过 MaxCollect 而被忽略的 SQL 元数据注册次数。
pub static IGNORE_EXCEED_SQL_COUNT: AtomicU64 = AtomicU64::new(0);
/// 因超过 MaxCollect 而被忽略的 Plan 元数据注册次数。
pub static IGNORE_EXCEED_PLAN_COUNT: AtomicU64 = AtomicU64::new(0);

/// 单条语句在各 KV 目标（如 TiKV 地址）上的执行次数。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KvStatementStatsItem {
    pub KvExecCount: Option<HashMap<String, u64>>,
}

impl KvStatementStatsItem {
    /// 将另一份 KV 执行次数按目标键累加（溢出回绕）。
    pub fn merge(&mut self, other: &Self) {
        let Some(other_counts) = other.KvExecCount.as_ref() else {
            return;
        };
        let counts = self.KvExecCount.get_or_insert_with(HashMap::new);
        for (target, count) in other_counts {
            let current = counts.entry(target.clone()).or_default();
            *current = current.wrapping_add(*count);
        }
    }
}

/// 语句级统计：执行次数、耗时、网络字节及嵌套的 KV 统计。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatementStatsItem {
    pub KvStatsItem: KvStatementStatsItem,
    pub ExecCount: u64,
    pub SumDurationNs: u64,
    pub DurationCount: u64,
    pub NetworkInBytes: u64,
    pub NetworkOutBytes: u64,
}

impl StatementStatsItem {
    /// 字段级 wrapping 累加，并合并 KvStatsItem。
    pub fn merge(&mut self, other: &Self) {
        self.ExecCount = self.ExecCount.wrapping_add(other.ExecCount);
        self.SumDurationNs = self.SumDurationNs.wrapping_add(other.SumDurationNs);
        self.DurationCount = self.DurationCount.wrapping_add(other.DurationCount);
        self.NetworkInBytes = self.NetworkInBytes.wrapping_add(other.NetworkInBytes);
        self.NetworkOutBytes = self.NetworkOutBytes.wrapping_add(other.NetworkOutBytes);
        self.KvStatsItem.merge(&other.KvStatsItem);
    }
}

/// 某一时间戳上的采样点：语句统计 + CPU 毫秒。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TsItem {
    stmt_stats: StatementStatsItem,
    timestamp: u64,
    cpu_time_ms: u32,
}

impl TsItem {
    /// 构造零值项，并为 KvExecCount 预置空 HashMap（与 Go 侧非 nil map 对齐）。
    fn zero() -> Self {
        Self {
            stmt_stats: StatementStatsItem {
                KvStatsItem: KvStatementStatsItem {
                    KvExecCount: Some(HashMap::new()),
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// 转为 tipb::TopSqlRecordItem protobuf。
    pub fn to_proto(&self) -> tipb::TopSqlRecordItem {
        let mut item = tipb::TopSqlRecordItem::new();
        item.set_timestamp_sec(self.timestamp);
        item.set_cpu_time_ms(self.cpu_time_ms);
        item.set_stmt_exec_count(self.stmt_stats.ExecCount);
        item.set_stmt_kv_exec_count(
            self.stmt_stats
                .KvStatsItem
                .KvExecCount
                .clone()
                .unwrap_or_default(),
        );
        item.set_stmt_duration_sum_ns(self.stmt_stats.SumDurationNs);
        item.set_stmt_duration_count(self.stmt_stats.DurationCount);
        item.set_stmt_network_in_bytes(self.stmt_stats.NetworkInBytes);
        item.set_stmt_network_out_bytes(self.stmt_stats.NetworkOutBytes);
        item
    }
}

/// 单一 SQL+Plan 的时间序列记录：按 timestamp 索引的 TsItem 列表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    ts_index: HashMap<u64, usize>,
    sql_digest: Vec<u8>,
    plan_digest: Vec<u8>,
    ts_items: Vec<TsItem>,
    total_cpu_time_ms: u64,
}

impl Record {
    /// 按全局上报间隔与精度预分配容量，构造空 Record。
    pub fn new(sql_digest: Vec<u8>, plan_digest: Vec<u8>) -> Self {
        let precision = crate::topsql_state::GlobalState
            .PrecisionSeconds
            .load(Ordering::SeqCst)
            .max(1);
        let capacity = ((crate::topsql_state::DefTiDBTopSQLReportIntervalSeconds / precision) + 1)
            .clamp(0, MAX_TS_ITEMS_CAPACITY as i64) as usize;
        Self {
            ts_index: HashMap::with_capacity(capacity),
            sql_digest,
            plan_digest,
            ts_items: Vec::with_capacity(capacity),
            total_cpu_time_ms: 0,
        }
    }

    /// 在给定时间戳累加 CPU；若不存在该点则新建 TsItem。
    pub fn append_cpu_time(&mut self, timestamp: u64, cpu_time_ms: u32) {
        if let Some(index) = self.ts_index.get(&timestamp).copied() {
            self.ts_items[index].cpu_time_ms =
                self.ts_items[index].cpu_time_ms.wrapping_add(cpu_time_ms);
        } else {
            let mut item = TsItem::zero();
            item.timestamp = timestamp;
            item.cpu_time_ms = cpu_time_ms;
            self.ts_index.insert(timestamp, self.ts_items.len());
            self.ts_items.push(item);
        }
        self.total_cpu_time_ms = self.total_cpu_time_ms.wrapping_add(u64::from(cpu_time_ms));
    }

    /// 在给定时间戳合并语句统计；不存在则新建。
    pub fn append_stmt_stats_item(&mut self, timestamp: u64, item: StatementStatsItem) {
        if let Some(index) = self.ts_index.get(&timestamp).copied() {
            self.ts_items[index].stmt_stats.merge(&item);
        } else {
            let mut ts_item = TsItem::zero();
            ts_item.timestamp = timestamp;
            ts_item.stmt_stats = item;
            self.ts_index.insert(timestamp, self.ts_items.len());
            self.ts_items.push(ts_item);
        }
    }

    /// 按时间戳归并另一条 Record（双指针合并），累加 CPU 与语句统计。
    pub fn merge(&mut self, other: Option<&mut Record>) {
        let Some(other) = other else {
            return;
        };
        if other.ts_items.is_empty() {
            return;
        }
        self.sort_and_rebuild();
        other.sort_and_rebuild();
        if self.ts_items.is_empty() {
            self.total_cpu_time_ms = other.total_cpu_time_ms;
            self.ts_items = other.ts_items.clone();
            self.ts_index = other.ts_index.clone();
            return;
        }

        // 双指针按 timestamp 归并，相同时间戳则字段累加。
        let mut merged = Vec::with_capacity(self.ts_items.len() + other.ts_items.len());
        let (mut left, mut right) = (0, 0);
        while left < self.ts_items.len() && right < other.ts_items.len() {
            let a = &self.ts_items[left];
            let b = &other.ts_items[right];
            match a.timestamp.cmp(&b.timestamp) {
                std::cmp::Ordering::Less => {
                    merged.push(a.clone());
                    left += 1;
                }
                std::cmp::Ordering::Greater => {
                    merged.push(b.clone());
                    right += 1;
                }
                std::cmp::Ordering::Equal => {
                    let mut item = a.clone();
                    item.cpu_time_ms = item.cpu_time_ms.wrapping_add(b.cpu_time_ms);
                    item.stmt_stats.merge(&b.stmt_stats);
                    merged.push(item);
                    left += 1;
                    right += 1;
                }
            }
        }
        merged.extend_from_slice(&self.ts_items[left..]);
        merged.extend_from_slice(&other.ts_items[right..]);
        self.ts_items = merged;
        self.total_cpu_time_ms = self.total_cpu_time_ms.wrapping_add(other.total_cpu_time_ms);
        self.rebuild_ts_index();
    }

    /// 若时间戳无序则排序并重建索引。
    fn sort_and_rebuild(&mut self) {
        if !self
            .ts_items
            .windows(2)
            .all(|pair| pair[0].timestamp <= pair[1].timestamp)
        {
            self.ts_items.sort_by_key(|item| item.timestamp);
            self.rebuild_ts_index();
        }
    }

    /// 根据当前 ts_items 顺序重建 timestamp → 下标索引。
    fn rebuild_ts_index(&mut self) {
        self.ts_index.clear();
        self.ts_index.reserve(self.ts_items.len());
        for (index, item) in self.ts_items.iter().enumerate() {
            self.ts_index.insert(item.timestamp, index);
        }
    }

    /// 导出为 tipb::TopSqlRecord，附带 keyspace 名称。
    pub fn to_proto(&self, keyspace_name: Vec<u8>) -> tipb::TopSqlRecord {
        let mut record = tipb::TopSqlRecord::new();
        record.set_keyspace_name(keyspace_name);
        record.set_sql_digest(self.sql_digest.clone());
        record.set_plan_digest(self.plan_digest.clone());
        record.set_items(self.ts_items.iter().map(TsItem::to_proto).collect());
        record
    }

    /// 按顺序返回各采样点时间戳。
    pub fn timestamps(&self) -> Vec<u64> {
        self.ts_items.iter().map(|item| item.timestamp).collect()
    }

    /// 按顺序返回各采样点 CPU 毫秒。
    pub fn cpu_times_ms(&self) -> Vec<u32> {
        self.ts_items.iter().map(|item| item.cpu_time_ms).collect()
    }

    /// 按顺序返回各采样点语句统计引用。
    pub fn statement_stats(&self) -> Vec<&StatementStatsItem> {
        self.ts_items.iter().map(|item| &item.stmt_stats).collect()
    }

    /// 累计 CPU 毫秒总和。
    pub fn total_cpu_time_ms(&self) -> u64 {
        self.total_cpu_time_ms
    }

    /// SQL digest 字节。
    pub fn sql_digest(&self) -> &[u8] {
        &self.sql_digest
    }

    /// Plan digest 字节。
    pub fn plan_digest(&self) -> &[u8] {
        &self.plan_digest
    }
}

/// 多条 Record 的集合，支持按总 CPU 做 Top-N 裁剪。
#[derive(Clone, Debug, Default)]
pub struct Records(Vec<Record>);

impl Records {
    /// 保留 CPU 最高的 n 条；其余作为被淘汰集合返回（若有）。
    pub fn top_n(mut self, n: usize) -> (Self, Option<Self>) {
        if self.0.len() <= n {
            return (self, None);
        }
        self.0.select_nth_unstable_by(n, |left, right| {
            right.total_cpu_time_ms.cmp(&left.total_cpu_time_ms)
        });
        self.0[..n].sort_by(|left, right| right.total_cpu_time_ms.cmp(&left.total_cpu_time_ms));
        let evicted = self.0.split_off(n);
        (self, Some(Self(evicted)))
    }

    /// 批量转为 tipb::TopSqlRecord 列表。
    pub fn to_proto(&self, keyspace_name: Vec<u8>) -> Vec<tipb::TopSqlRecord> {
        self.0
            .iter()
            .map(|record| record.to_proto(keyspace_name.clone()))
            .collect()
    }
}

impl std::ops::Deref for Records {
    type Target = [Record];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// 上报周期内正在收集的 Record 与按时间戳的淘汰标记。
#[derive(Default)]
pub struct Collecting {
    records: HashMap<Vec<u8>, Record>,
    evicted: HashMap<u64, HashSet<Vec<u8>>>,
    key_buf: Vec<u8>,
}

impl Collecting {
    /// 预分配 key_buf 的 Collecting。
    pub fn new() -> Self {
        Self {
            key_buf: Vec::with_capacity(64),
            ..Default::default()
        }
    }

    /// 按 SQL+Plan digest 取或创建 Record。
    pub fn get_or_create_record(&mut self, sql_digest: &[u8], plan_digest: &[u8]) -> &mut Record {
        let key = encode_key(&mut self.key_buf, sql_digest, plan_digest);
        self.records
            .entry(key)
            .or_insert_with(|| Record::new(sql_digest.to_vec(), plan_digest.to_vec()))
    }

    /// 标记某时间戳下该 SQL+Plan 已被 Top-N 淘汰。
    pub fn mark_as_evicted(&mut self, timestamp: u64, sql_digest: &[u8], plan_digest: &[u8]) {
        let key = encode_key(&mut self.key_buf, sql_digest, plan_digest);
        self.evicted.entry(timestamp).or_default().insert(key);
    }

    /// 查询某时间戳下是否已标记淘汰。
    pub fn has_evicted(&mut self, timestamp: u64, sql_digest: &[u8], plan_digest: &[u8]) -> bool {
        let key = encode_key(&mut self.key_buf, sql_digest, plan_digest);
        self.evicted
            .get(&timestamp)
            .is_some_and(|digests| digests.contains(&key))
    }

    /// 将 CPU 累加到 Others 桶（空 digest）；零值直接跳过。
    pub fn append_others_cpu_time(&mut self, timestamp: u64, cpu_time_ms: u32) {
        if cpu_time_ms == 0 {
            return;
        }
        self.records
            .entry(KEY_OTHERS.to_vec())
            .or_insert_with(|| Record::new(Vec::new(), Vec::new()))
            .append_cpu_time(timestamp, cpu_time_ms);
    }

    /// 将语句统计累加到 Others 桶。
    pub fn append_others_stmt_stats_item(&mut self, timestamp: u64, item: StatementStatsItem) {
        self.records
            .entry(KEY_OTHERS.to_vec())
            .or_insert_with(|| Record::new(Vec::new(), Vec::new()))
            .append_stmt_stats_item(timestamp, item);
    }

    /// 若同一 SQL 仅有一个非空 plan 与一个空 plan，则把空 plan 记录合并进有效 plan。
    fn remove_invalid_plan_record(&mut self) {
        let mut plans_by_sql: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
        for record in self.records.values() {
            plans_by_sql
                .entry(record.sql_digest.clone())
                .or_default()
                .push(record.plan_digest.clone());
        }
        for (sql_digest, plans) in plans_by_sql {
            if plans.len() != 2 || plans.iter().all(|plan| !plan.is_empty()) {
                continue;
            }
            let empty_key = encode_key(&mut self.key_buf, &sql_digest, b"");
            let Some(mut empty_record) = self.records.remove(&empty_key) else {
                continue;
            };
            let Some(valid_plan) = plans.iter().find(|plan| !plan.is_empty()) else {
                self.records.insert(empty_key, empty_record);
                continue;
            };
            let valid_key = encode_key(&mut self.key_buf, &sql_digest, valid_plan);
            if let Some(valid_record) = self.records.get_mut(&valid_key) {
                valid_record.merge(Some(&mut empty_record));
            } else {
                self.records.insert(empty_key, empty_record);
            }
        }
    }

    /// 取出全部记录供上报：先合并无效空 plan，Others 放在末尾。
    pub fn report_records(&mut self) -> Vec<Record> {
        let others = self.records.remove(KEY_OTHERS);
        self.remove_invalid_plan_record();
        let mut records: Vec<_> = self.records.values().cloned().collect();
        if let Some(others) = others {
            records.push(others);
        }
        records
    }

    /// 取走当前 records/evicted，留下空 Collecting（保留 key_buf 容量）。
    pub fn take(&mut self) -> Self {
        Self {
            records: std::mem::take(&mut self.records),
            evicted: std::mem::take(&mut self.evicted),
            key_buf: Vec::with_capacity(64),
        }
    }

    /// 当前非 Others 与 Others 合计的 Record 条数。
    pub fn record_count(&self) -> usize {
        self.records.len()
    }
}

/// reporter 侧 SQL CPU 时间记录（可由 collector 类型转换而来）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SQLCPUTimeRecord {
    pub SQLDigest: Vec<u8>,
    pub PlanDigest: Vec<u8>,
    pub CPUTimeMs: u32,
}

impl From<crate::collector::SQLCPUTimeRecord> for SQLCPUTimeRecord {
    fn from(record: crate::collector::SQLCPUTimeRecord) -> Self {
        Self {
            SQLDigest: record.SQLDigest,
            PlanDigest: record.PlanDigest,
            CPUTimeMs: record.CPUTimeMs,
        }
    }
}

/// CPU 记录列表，支持按 CPUTimeMs 做 Top-N。
#[derive(Clone, Debug, Default)]
pub struct CpuRecords(pub Vec<SQLCPUTimeRecord>);

impl CpuRecords {
    /// 保留 CPU 最高的 n 条；其余作为被淘汰集合返回。
    pub fn top_n(mut self, n: usize) -> (Self, Option<Self>) {
        if self.0.len() <= n {
            return (self, None);
        }
        self.0
            .select_nth_unstable_by(n, |left, right| right.CPUTimeMs.cmp(&left.CPUTimeMs));
        self.0[..n].sort_by(|left, right| right.CPUTimeMs.cmp(&left.CPUTimeMs));
        let evicted = self.0.split_off(n);
        (self, Some(Self(evicted)))
    }
}

/// 规范化 SQL 元数据：文本与是否内部 SQL。
#[derive(Clone, Debug)]
struct SqlMeta {
    normalized_sql: String,
    is_internal: bool,
}

/// 带 MaxCollect 上限的 SQL digest → 规范化文本映射（线程安全）。
pub struct NormalizedSqlMap {
    data: Mutex<HashMap<Vec<u8>, SqlMeta>>,
    length: AtomicUsize,
    max_collect: usize,
}

impl NormalizedSqlMap {
    /// 以指定上限构造空映射。
    pub fn new(max_collect: usize) -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
            length: AtomicUsize::new(0),
            max_collect,
        }
    }

    /// 使用全局 `GlobalState.MaxCollect` 作为上限。
    pub fn with_global_limit() -> Self {
        Self::new(
            crate::topsql_state::GlobalState
                .MaxCollect
                .load(Ordering::SeqCst)
                .max(0) as usize,
        )
    }

    /// 首次注册成功返回 true；超限或已存在则返回 false（超限时计数 IGNORE）。
    pub fn register(&self, sql_digest: &[u8], normalized_sql: String, is_internal: bool) -> bool {
        let mut data = self.data.lock().unwrap_or_else(|error| error.into_inner());
        if self.length.load(Ordering::SeqCst) >= self.max_collect {
            IGNORE_EXCEED_SQL_COUNT.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        if data.contains_key(sql_digest) {
            return false;
        }
        data.insert(
            sql_digest.to_vec(),
            SqlMeta {
                normalized_sql,
                is_internal,
            },
        );
        self.length.fetch_add(1, Ordering::SeqCst);
        true
    }

    /// 原子取出全部元数据并清空本表，供上报快照使用。
    pub fn take(&self) -> Self {
        let mut data = self.data.lock().unwrap_or_else(|error| error.into_inner());
        let taken = std::mem::take(&mut *data);
        let length = self.length.swap(0, Ordering::SeqCst);
        Self {
            data: Mutex::new(taken),
            length: AtomicUsize::new(length),
            max_collect: self.max_collect,
        }
    }

    /// 导出为 tipb::SqlMeta 列表。
    pub fn to_proto(&self, keyspace_name: Vec<u8>) -> Vec<tipb::SqlMeta> {
        let data = self.data.lock().unwrap_or_else(|error| error.into_inner());
        data.iter()
            .map(|(digest, meta)| {
                let mut proto = tipb::SqlMeta::new();
                proto.set_keyspace_name(keyspace_name.clone());
                proto.set_sql_digest(digest.clone());
                proto.set_normalized_sql(meta.normalized_sql.clone());
                proto.set_is_internal_sql(meta.is_internal);
                proto
            })
            .collect()
    }

    /// 当前已注册条数。
    pub fn len(&self) -> usize {
        self.length.load(Ordering::SeqCst)
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 规范化执行计划元数据：二进制计划文本与是否超大。
#[derive(Clone, Debug)]
struct PlanMeta {
    binary_normalized_plan: String,
    is_large: bool,
}

/// 带 MaxCollect 上限的 Plan digest → 规范化计划映射（线程安全）。
pub struct NormalizedPlanMap {
    data: Mutex<HashMap<Vec<u8>, PlanMeta>>,
    length: AtomicUsize,
    max_collect: usize,
}

impl NormalizedPlanMap {
    /// 以指定上限构造空映射。
    pub fn new(max_collect: usize) -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
            length: AtomicUsize::new(0),
            max_collect,
        }
    }

    /// 使用全局 `GlobalState.MaxCollect` 作为上限。
    pub fn with_global_limit() -> Self {
        Self::new(
            crate::topsql_state::GlobalState
                .MaxCollect
                .load(Ordering::SeqCst)
                .max(0) as usize,
        )
    }

    /// 首次注册成功返回 true；超限或已存在则返回 false。
    pub fn register(&self, plan_digest: &[u8], normalized_plan: String, is_large: bool) -> bool {
        let mut data = self.data.lock().unwrap_or_else(|error| error.into_inner());
        if self.length.load(Ordering::SeqCst) >= self.max_collect {
            IGNORE_EXCEED_PLAN_COUNT.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        if data.contains_key(plan_digest) {
            return false;
        }
        data.insert(
            plan_digest.to_vec(),
            PlanMeta {
                binary_normalized_plan: normalized_plan,
                is_large,
            },
        );
        self.length.fetch_add(1, Ordering::SeqCst);
        true
    }

    /// 原子取出全部元数据并清空本表。
    pub fn take(&self) -> Self {
        let mut data = self.data.lock().unwrap_or_else(|error| error.into_inner());
        let taken = std::mem::take(&mut *data);
        let length = self.length.swap(0, Ordering::SeqCst);
        Self {
            data: Mutex::new(taken),
            length: AtomicUsize::new(length),
            max_collect: self.max_collect,
        }
    }

    /// 导出 PlanMeta：大计划走压缩编码，普通计划先解码再写 normalized_plan。
    pub fn to_proto<D, C>(
        &self,
        keyspace_name: Vec<u8>,
        decode_plan: D,
        compress_plan: C,
    ) -> Vec<tipb::PlanMeta>
    where
        D: Fn(&str) -> Result<String, String>,
        C: Fn(&[u8]) -> String,
    {
        let data = self.data.lock().unwrap_or_else(|error| error.into_inner());
        let mut metas = Vec::with_capacity(data.len());
        for (digest, meta) in data.iter() {
            let mut proto = tipb::PlanMeta::new();
            proto.set_keyspace_name(keyspace_name.clone());
            proto.set_plan_digest(digest.clone());
            if meta.is_large {
                proto.set_encoded_normalized_plan(compress_plan(
                    meta.binary_normalized_plan.as_bytes(),
                ));
            } else {
                let Ok(decoded) = decode_plan(&meta.binary_normalized_plan) else {
                    log::warn!("decode plan failed; category=top-sql");
                    continue;
                };
                proto.set_normalized_plan(decoded);
            }
            metas.push(proto);
        }
        metas
    }

    /// 当前已注册条数。
    pub fn len(&self) -> usize {
        self.length.load(Ordering::SeqCst)
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 将 SQL digest 与 Plan digest 拼接为 map 键（复用 buf 减少分配）。
fn encode_key(buf: &mut Vec<u8>, sql_digest: &[u8], plan_digest: &[u8]) -> Vec<u8> {
    buf.clear();
    buf.extend_from_slice(sql_digest);
    buf.extend_from_slice(plan_digest);
    buf.clone()
}
