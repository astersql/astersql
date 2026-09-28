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

// 自动分析（auto analyze）核心逻辑：作业生命周期、脏页清理与挑表调度。
//
// 维护 `mysql.analyze_jobs` 作业状态；按修改比率（modify_count / 行数）判断是否需 ANALYZE；
// 在时间窗口内随机挑选表/分区（含动态裁剪批次）生成 `AnalyzeRequest`；
// 并通过优先级队列包装器驱动实际分析。

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 查询当前实例上超时且仍 pending/running 的 analyze 作业。
pub const SELECT_ANALYZE_JOBS_ON_CURRENT_INSTANCE_SQL: &str = "SELECT id, process_id FROM mysql.analyze_jobs WHERE instance = %? AND state IN ('pending', 'running') AND update_time < CONVERT_TZ(%?, '+00:00', @@TIME_ZONE)";
/// 查询全局超时且仍 pending/running 的 analyze 作业（含 instance）。
pub const SELECT_ANALYZE_JOBS_SQL: &str = "SELECT id, instance FROM mysql.analyze_jobs WHERE state IN ('pending', 'running') AND update_time < CONVERT_TZ(%?, '+00:00', @@TIME_ZONE)";
/// 批量将指定作业标记为 failed（实例宕机或查询被终止）。
pub const BATCH_UPDATE_ANALYZE_JOB_SQL: &str = "UPDATE mysql.analyze_jobs SET state = 'failed', fail_reason = 'The TiDB Server has either shut down or the analyze query was terminated during the analyze job execution', process_id = NULL WHERE id IN (%?)";
/// 触发自动分析的最小表行数门槛；更小的表跳过。
pub const AUTO_ANALYZE_MIN_COUNT: i64 = 1_000;
/// job_info / fail_reason 等 TEXT 字段的最大字节长度。
const TEXT_MAX_LENGTH: usize = 65_535;
/// 进度增量超过该行数且距上次 dump 足够久时，才刷到 mysql.analyze_jobs。
const MAX_PROGRESS_DELTA: i64 = 10_000_000;
/// 进度刷盘的最小时间间隔。
const DUMP_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
/// 自动分析模块的简单错误包装。
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// ANALYZE 作业类型：整表分析或全局统计合并。
pub enum JobType {
    /// 表/分区/索引分析。
    TableAnalysis,
    /// 分区表全局统计合并。
    GlobalStatsMerge,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 作业进度累计器：按增量行数与时间间隔决定是否刷盘。
pub struct AnalyzeProgress {
    /// 尚未刷盘的处理行数增量。
    delta_count: i64,
    /// 上次刷盘时间。
    last_dump: Option<SystemTime>,
}

impl AnalyzeProgress {
    /// 累加处理行数；若超过阈值且间隔足够，返回应刷盘的增量并清零，否则返回 0。
    pub fn update(&mut self, row_count: i64, now: SystemTime) -> i64 {
        self.delta_count += row_count;
        let elapsed = self
            .last_dump
            .and_then(|last| now.duration_since(last).ok())
            .unwrap_or(Duration::MAX);
        if self.delta_count > MAX_PROGRESS_DELTA && elapsed > DUMP_INTERVAL {
            let result = self.delta_count;
            self.delta_count = 0;
            self.last_dump = Some(now);
            result
        } else {
            0
        }
    }

    /// 当前未刷盘的增量行数。
    pub fn delta_count(&self) -> i64 {
        self.delta_count
    }

    /// 设置上次刷盘时间（作业开始时初始化）。
    pub fn set_last_dump_time(&mut self, time: SystemTime) {
        self.last_dump = Some(time);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 一条 analyze 作业的内存表示，对应 `mysql.analyze_jobs` 一行。
pub struct AnalyzeJob {
    /// 作业主键；插入后由 last_insert_id 填充。
    pub id: Option<u64>,
    /// 库名。
    pub database: String,
    /// 表名。
    pub table: String,
    /// 分区名（非分区表可为空）。
    pub partition: String,
    /// 作业描述（列集合、桶数等）。
    pub job_info: String,
    /// 开始时间。
    pub start_time: Option<SystemTime>,
    /// 结束时间。
    pub end_time: Option<SystemTime>,
    /// 进度累计器。
    pub progress: AnalyzeProgress,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// SQL 绑定参数值。
pub enum SqlValue {
    I64(i64),
    U64(u64),
    Text(String),
    Null,
}

/// 作业存储抽象：执行 SQL、取自增 ID、查询过期作业与存活实例。
pub trait JobStore {
    fn execute(&mut self, sql: &str, args: &[SqlValue]) -> Result<(), Error>;
    fn last_insert_id(&mut self) -> Result<u64, Error>;
    fn stale_jobs_for_instance(&mut self, instance: &str)
    -> Result<Vec<(u64, Option<u64>)>, Error>;
    fn stale_jobs(&mut self) -> Result<Vec<(u64, String)>, Error>;
    fn live_instances(&mut self) -> Result<HashSet<String>, Error>;
}

/// 截断超长文本至 TEXT_MAX_LENGTH，并保证落在 UTF-8 字符边界上。
fn truncate_text(value: &str) -> String {
    if value.len() <= TEXT_MAX_LENGTH {
        return value.to_owned();
    }
    let mut end = TEXT_MAX_LENGTH;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

/// 插入 pending 状态的 analyze 作业，并回填 `job.id`。
pub fn insert_analyze_job(
    store: &mut impl JobStore,
    job: &mut AnalyzeJob,
    instance: &str,
    process_id: u64,
) -> Result<(), Error> {
    let info = truncate_text(&job.job_info);
    store.execute(
        "INSERT INTO mysql.analyze_jobs (table_schema, table_name, partition_name, job_info, state, instance, process_id) VALUES (%?, %?, %?, %?, %?, %?, %?)",
        &[
            SqlValue::Text(job.database.clone()),
            SqlValue::Text(job.table.clone()),
            SqlValue::Text(job.partition.clone()),
            SqlValue::Text(info),
            SqlValue::Text("pending".into()),
            SqlValue::Text(instance.into()),
            SqlValue::U64(process_id),
        ],
    )?;
    job.id = Some(store.last_insert_id()?);
    Ok(())
}

/// 将作业标记为 running，记录开始时间与进度 dump 起点。
pub fn start_analyze_job(store: &mut impl JobStore, job: Option<&mut AnalyzeJob>, now: SystemTime) {
    let Some(job) = job else { return };
    let Some(id) = job.id else { return };
    job.start_time = Some(now);
    job.progress.set_last_dump_time(now);
    let _ = store.execute(
        "UPDATE mysql.analyze_jobs SET start_time = CONVERT_TZ(%?, '+00:00', @@TIME_ZONE), state = %? WHERE id = %?",
        &[SqlValue::U64(epoch_seconds(now)), SqlValue::Text("running".into()), SqlValue::U64(id)],
    );
}

/// 按进度策略累加 processed_rows；仅在 `update` 返回非 0 时写库。
pub fn update_analyze_job_progress(
    store: &mut impl JobStore,
    job: Option<&mut AnalyzeJob>,
    row_count: i64,
    now: SystemTime,
) {
    let Some(job) = job else { return };
    let Some(id) = job.id else { return };
    let delta = job.progress.update(row_count, now);
    if delta == 0 {
        return;
    }
    let _ = store.execute(
        "UPDATE mysql.analyze_jobs SET processed_rows = processed_rows + %? WHERE id = %?",
        &[SqlValue::I64(delta), SqlValue::U64(id)],
    );
}

/// 结束作业：刷剩余进度（仅 TableAnalysis）、写 end_time/state/fail_reason，清空 process_id。
pub fn finish_analyze_job(
    store: &mut impl JobStore,
    job: Option<&mut AnalyzeJob>,
    failure: Option<&Error>,
    job_type: JobType,
    now: SystemTime,
) {
    let Some(job) = job else { return };
    let Some(id) = job.id else { return };
    job.end_time = Some(now);
    let state = if failure.is_some() {
        "failed"
    } else {
        "finished"
    };
    let mut assignments = Vec::new();
    let mut args = Vec::new();
    // 整表分析结束时把尚未刷盘的进度一并写入。
    if job_type == JobType::TableAnalysis {
        assignments.push("processed_rows = processed_rows + %?");
        args.push(SqlValue::I64(job.progress.delta_count()));
    }
    assignments.push("end_time = CONVERT_TZ(%?, '+00:00', @@TIME_ZONE)");
    args.push(SqlValue::U64(epoch_seconds(now)));
    assignments.push("state = %?");
    args.push(SqlValue::Text(state.into()));
    if let Some(error) = failure {
        assignments.push("fail_reason = %?");
        args.push(SqlValue::Text(truncate_text(&error.0)));
    }
    assignments.push("process_id = NULL");
    args.push(SqlValue::U64(id));
    let sql = format!(
        "UPDATE mysql.analyze_jobs SET {} WHERE id = %?",
        assignments.join(", ")
    );
    let _ = store.execute(&sql, &args);
}

/// 清理当前实例上 process_id 已不在运行集合中的超时作业。
pub fn cleanup_corrupted_jobs_on_current_instance(
    store: &mut impl JobStore,
    instance: &str,
    running_process_ids: &HashSet<u64>,
) -> Result<Vec<u64>, Error> {
    let ids = store
        .stale_jobs_for_instance(instance)?
        .into_iter()
        .filter_map(|(id, process)| {
            process
                .filter(|pid| !running_process_ids.contains(pid))
                .map(|_| id)
        })
        .collect::<Vec<_>>();
    batch_fail_jobs(store, &ids)?;
    Ok(ids)
}

/// 清理归属已下线实例的超时作业。
pub fn cleanup_corrupted_jobs_on_dead_instances(
    store: &mut impl JobStore,
) -> Result<Vec<u64>, Error> {
    let stale = store.stale_jobs()?;
    if stale.is_empty() {
        return Ok(Vec::new());
    }
    let live = store.live_instances()?;
    let ids = stale
        .into_iter()
        .filter_map(|(id, instance)| (!live.contains(&instance)).then_some(id))
        .collect::<Vec<_>>();
    batch_fail_jobs(store, &ids)?;
    Ok(ids)
}

/// 批量将作业标记为 failed。
fn batch_fail_jobs(store: &mut impl JobStore, ids: &[u64]) -> Result<(), Error> {
    if ids.is_empty() {
        return Ok(());
    }
    store.execute(
        BATCH_UPDATE_ANALYZE_JOB_SQL,
        &[SqlValue::Text(
            ids.iter().map(u64::to_string).collect::<Vec<_>>().join(","),
        )],
    )
}

/// SystemTime 转 Unix 秒，供 CONVERT_TZ 绑定。
fn epoch_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 索引元信息，供挑表时判断是否需对缺失索引做 ANALYZE。
pub struct IndexInfo {
    pub id: i64,
    pub name: String,
    /// 是否已 public（可用）。
    pub public: bool,
    /// 列存/向量类索引；自动分析会跳过。
    pub columnar: bool,
    /// 特殊全局索引；动态分区路径跳过。
    pub special_global: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 表级统计摘要，用于判断是否需要自动分析。
pub struct TableStats {
    /// 伪统计（尚未真实 ANALYZE）。
    pub pseudo: bool,
    /// 实时行数估计。
    pub realtime_count: i64,
    /// 自上次分析以来的修改行数。
    pub modify_count: i64,
    /// 上次分析时的行数基线。
    pub analyze_row_count: i64,
    /// 是否已真实分析过。
    pub analyzed: bool,
    /// 统计版本（如 Version2）。
    pub analyze_version: i32,
    /// 已分析过的索引 ID 集合。
    pub analyzed_indexes: HashSet<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 分区及其可选统计。
pub struct Partition {
    pub id: i64,
    pub name: String,
    pub stats: Option<TableStats>,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 表元信息及分区列表。
pub struct TableInfo {
    pub id: i64,
    pub name: String,
    /// 视图不参与自动分析。
    pub view: bool,
    pub stats: Option<TableStats>,
    pub indexes: Vec<IndexInfo>,
    pub partitions: Vec<Partition>,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// schema（数据库）及其表集合。
pub struct SchemaInfo {
    pub name: String,
    /// 系统库或内存库，跳过自动分析。
    pub system_or_memory: bool,
    pub tables: Vec<TableInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 分区裁剪模式：静态逐分区 vs 动态批量。
pub enum PartitionPruneMode {
    Static,
    Dynamic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 生成的一条 ANALYZE SQL 请求。
pub struct AnalyzeRequest {
    /// 带 `%n` 占位符的 SQL 模板。
    pub sql: String,
    /// 占位符对应的标识符参数。
    pub params: Vec<String>,
    /// 请求的统计版本。
    pub requested_version: i32,
    /// 是否需要 snapshot（版本不匹配时）。
    pub snapshot: bool,
}

/// 判断表是否因未分析或修改比率超阈值而需要 ANALYZE。
///
/// 返回 `(need, reason)`；`ratio == 0` 时已分析表不再因修改量触发。
pub fn need_analyze_table(table: &TableStats, ratio: f64) -> (bool, String) {
    if !table.analyzed {
        return (true, "table unanalyzed".into());
    }
    if ratio == 0.0 {
        return (false, String::new());
    }
    let count = if table.analyze_row_count > 0 {
        table.analyze_row_count as f64
    } else {
        table.realtime_count as f64
    };
    if table.modify_count as f64 / count <= ratio {
        return (false, String::new());
    }
    (
        true,
        format!(
            "too many modifications({}/{count}>{ratio})",
            table.modify_count
        ),
    )
}

/// 判断 `now_minute`（一天中的分钟）是否落在 `[start, end]` 窗口内（支持跨午夜）。
pub fn within_day_time_period(start_minute: u16, end_minute: u16, now_minute: u16) -> bool {
    if start_minute <= end_minute {
        start_minute <= now_minute && now_minute <= end_minute
    } else {
        // 跨午夜：例如 22:00–06:00。
        now_minute >= start_minute || now_minute <= end_minute
    }
}

/// 解析 `HH:MM` 起止时间，并判断当前分钟是否在窗口内。
pub fn parse_auto_analyze_window(
    start: &str,
    end: &str,
    now_minute: u16,
) -> Result<(u16, u16, bool), Error> {
    let start = parse_hhmm(start)?;
    let end = parse_hhmm(end)?;
    Ok((start, end, within_day_time_period(start, end, now_minute)))
}

/// 将 `HH:MM` 解析为一天中的分钟数。
fn parse_hhmm(value: &str) -> Result<u16, Error> {
    let (hour, minute) = value
        .split_once(':')
        .ok_or_else(|| Error(format!("invalid time: {value}")))?;
    let hour: u16 = hour
        .parse()
        .map_err(|_| Error(format!("invalid hour: {value}")))?;
    let minute: u16 = minute
        .parse()
        .map_err(|_| Error(format!("invalid minute: {value}")))?;
    if hour > 23 || minute > 59 {
        return Err(Error(format!("invalid time: {value}")));
    }
    Ok(hour * 60 + minute)
}

/// 随机打乱 schema/表后，挑选第一个需要分析的表/分区并返回请求列表。
///
/// `window_open` 为 false 时立即返回空；锁定表 ID、视图、系统库均跳过。
pub fn random_pick_one_table_and_try_auto_analyze(
    schemas: &mut [SchemaInfo],
    locked: &HashSet<i64>,
    ratio: f64,
    prune_mode: PartitionPruneMode,
    requested_version: i32,
    partition_batch_size: usize,
    seed: u64,
    window_open: impl Fn() -> bool,
) -> Vec<AnalyzeRequest> {
    shuffle(schemas, seed);
    for schema in schemas {
        if schema.system_or_memory {
            continue;
        }
        shuffle(&mut schema.tables, seed ^ schema.name.len() as u64);
        for table in &schema.tables {
            if !window_open() {
                return Vec::new();
            }
            if table.view || locked.contains(&table.id) {
                continue;
            }
            // 非分区表：直接尝试整表/索引分析。
            if table.partitions.is_empty() {
                if let Some(request) = analyze_table(
                    &schema.name,
                    table,
                    table.stats.as_ref(),
                    ratio,
                    requested_version,
                    &[],
                ) {
                    return vec![request];
                }
                continue;
            }
            let parts = table
                .partitions
                .iter()
                .filter(|part| !locked.contains(&part.id))
                .collect::<Vec<_>>();
            if prune_mode == PartitionPruneMode::Dynamic {
                let requests = analyze_dynamic_partitions(
                    &schema.name,
                    table,
                    &parts,
                    ratio,
                    requested_version,
                    partition_batch_size.max(1),
                );
                if !requests.is_empty() {
                    return requests;
                }
            } else {
                // 静态模式：逐分区独立生成请求。
                for part in parts {
                    if let Some(request) = analyze_table(
                        &schema.name,
                        table,
                        part.stats.as_ref(),
                        ratio,
                        requested_version,
                        std::slice::from_ref(&part.name),
                    ) {
                        return vec![request];
                    }
                }
            }
        }
    }
    Vec::new()
}

/// 对单表（或指定分区列表）生成 ANALYZE 请求：优先整表脏页，其次缺失索引。
fn analyze_table(
    database: &str,
    table: &TableInfo,
    stats: Option<&TableStats>,
    ratio: f64,
    requested_version: i32,
    partitions: &[String],
) -> Option<AnalyzeRequest> {
    let stats = stats?;
    if stats.pseudo || stats.realtime_count < AUTO_ANALYZE_MIN_COUNT {
        return None;
    }
    let mut params = vec![database.to_owned(), table.name.clone()];
    let mut sql = "analyze table %n.%n".to_owned();
    if !partitions.is_empty() {
        sql.push_str(" partition");
        for partition in partitions {
            sql.push_str(" %n");
            params.push(partition.clone());
        }
    }
    if need_analyze_table(stats, ratio).0 {
        return Some(AnalyzeRequest {
            sql,
            params,
            requested_version,
            snapshot: !analyze_version_matches(Some(stats), requested_version),
        });
    }
    // 表本身不需要分析时，检查是否有未分析的普通索引。
    for index in &table.indexes {
        if index.public && !index.columnar && !stats.analyzed_indexes.contains(&index.id) {
            sql.push_str(" index %n");
            params.push(index.name.clone());
            return Some(AnalyzeRequest {
                sql,
                params,
                requested_version,
                snapshot: !analyze_version_matches(Some(stats), requested_version),
            });
        }
    }
    None
}

/// 动态裁剪模式下，将需要分析的分区按 batch_size 切成多条 ANALYZE 请求。
fn analyze_dynamic_partitions(
    database: &str,
    table: &TableInfo,
    partitions: &[&Partition],
    ratio: f64,
    requested_version: i32,
    batch_size: usize,
) -> Vec<AnalyzeRequest> {
    let version_matches = partitions
        .iter()
        .all(|part| analyze_version_matches(part.stats.as_ref(), requested_version));
    let needed = partitions
        .iter()
        .filter(|part| {
            part.stats.as_ref().is_some_and(|stats| {
                !stats.pseudo
                    && stats.realtime_count >= AUTO_ANALYZE_MIN_COUNT
                    && need_analyze_table(stats, ratio).0
            })
        })
        .map(|part| part.name.clone())
        .collect::<Vec<_>>();
    if !needed.is_empty() {
        return partition_requests(
            database,
            table,
            &needed,
            None,
            requested_version,
            !version_matches,
            batch_size,
        );
    }
    // 无脏分区时，按索引查找缺失分析的分区。
    for index in &table.indexes {
        if !index.public || index.columnar || index.special_global {
            continue;
        }
        let missing = partitions
            .iter()
            .filter(|part| {
                part.stats.as_ref().is_some_and(|stats| {
                    !stats.pseudo && !stats.analyzed_indexes.contains(&index.id)
                })
            })
            .map(|part| part.name.clone())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return partition_requests(
                database,
                table,
                &missing,
                Some(&index.name),
                requested_version,
                !version_matches,
                batch_size,
            );
        }
    }
    Vec::new()
}

/// 将分区名列表按 batch_size 切块，生成带可选 index 子句的 AnalyzeRequest。
fn partition_requests(
    database: &str,
    table: &TableInfo,
    names: &[String],
    index: Option<&str>,
    requested_version: i32,
    snapshot: bool,
    batch_size: usize,
) -> Vec<AnalyzeRequest> {
    names
        .chunks(batch_size)
        .map(|chunk| {
            let placeholders = (0..chunk.len())
                .map(|_| "%n")
                .collect::<Vec<_>>()
                .join(", ");
            let mut sql = format!("analyze table %n.%n partition {placeholders}");
            let mut params = vec![database.to_owned(), table.name.clone()];
            params.extend(chunk.iter().cloned());
            if let Some(index) = index {
                sql.push_str(" index %n");
                params.push(index.to_owned());
            }
            AnalyzeRequest {
                sql,
                params,
                requested_version,
                snapshot,
            }
        })
        .collect()
}

/// 表及其所有分区的 analyze_version 是否都等于 requested_version。
pub fn analyze_version_matches_for_table(table: &TableInfo, requested_version: i32) -> bool {
    analyze_version_matches(table.stats.as_ref(), requested_version)
        && table
            .partitions
            .iter()
            .all(|part| analyze_version_matches(part.stats.as_ref(), requested_version))
}

/// Match Go `AnalyzeVersionMatchesForTableStats`: pseudo, missing, and
/// unversioned statistics do not require a legacy rewrite.
fn analyze_version_matches(stats: Option<&TableStats>, requested_version: i32) -> bool {
    let Some(stats) = stats else {
        return true;
    };
    stats.pseudo || stats.analyze_version <= 0 || stats.analyze_version == requested_version
}

/// xorshift 伪随机打乱，用于随机挑表顺序。
fn shuffle<T>(values: &mut [T], mut state: u64) {
    for index in (1..values.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        values.swap(index, state as usize % (index + 1));
    }
}

/// 优先级队列刷新器接口：分析最高优先级任务及测试钩子。
pub trait PriorityRefresher {
    fn analyze_highest_priority(&mut self) -> bool;
    fn process_dml_changes_for_test(&mut self);
    fn requeue_must_retry_jobs_for_test(&mut self);
    fn wait_finished_for_test(&mut self);
    fn close_priority_queue(&mut self);
    fn close(&mut self);
}

/// 持有 PriorityRefresher 的 StatsAnalyze 门面。
pub struct StatsAnalyze<R> {
    refresher: R,
}

impl<R: PriorityRefresher> StatsAnalyze<R> {
    /// 用给定刷新器构造。
    pub fn new(refresher: R) -> Self {
        Self { refresher }
    }

    /// 驱动优先级队列：测试模式下先处理 DML/重试，再分析最高优先级任务。
    pub fn handle_priority_queue(&mut self, in_test: bool) -> bool {
        if in_test {
            self.refresher.process_dml_changes_for_test();
            self.refresher.requeue_must_retry_jobs_for_test();
        }
        let analyzed = self.refresher.analyze_highest_priority();
        if in_test {
            self.refresher.wait_finished_for_test();
        }
        analyzed
    }

    /// 关闭优先级队列。
    pub fn close_priority_queue(&mut self) {
        self.refresher.close_priority_queue();
    }

    /// 关闭整个分析组件。
    pub fn close(&mut self) {
        self.refresher.close();
    }
}

/// 返回 `now` 往前 10 分钟的时间点（清理超时作业用）。
pub fn ten_minutes_ago(now: SystemTime) -> SystemTime {
    now.checked_sub(Duration::from_secs(600))
        .unwrap_or(UNIX_EPOCH)
}

/// 将分区列表中有统计的项映射为 `partition_id -> TableStats`。
pub fn group_stats_by_partition(partitions: &[Partition]) -> HashMap<i64, TableStats> {
    partitions
        .iter()
        .filter_map(|part| part.stats.clone().map(|stats| (part.id, stats)))
        .collect()
}
