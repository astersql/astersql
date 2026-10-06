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

//! ANALYZE, statistics publication, SHOW metadata, and mysql statistics tables.

use super::session::RuntimeTimeZone;
use super::*;

/// ANALYZE 发布前失败点名称。
const ANALYZE_BEFORE_PUBLISH_FAILPOINT: &str = "executor/analyze-before-publish";
/// ANALYZE 写回存储失败点名称。
const ANALYZE_SAVE_ERROR_FAILPOINT: &str =
    "statistics/handle/storage/saveAnalyzeResultToStorageErr";

#[derive(Default)]
/// ANALYZE 保存错误注入状态：目标 killer 与 failpoint 守卫。
struct AnalyzeSaveErrorState {
    targets: BTreeMap<usize, usize>,
    failpoint: Option<astersql_testkit_testfailpoint::FailGuard>,
}

static ANALYZE_SAVE_ERROR_STATE: LazyLock<Mutex<AnalyzeSaveErrorState>> =
    LazyLock::new(|| Mutex::new(AnalyzeSaveErrorState::default()));

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// ANALYZE 可暂停阶段：构建或保存。
enum AnalyzePauseStage {
    Build,
    #[default]
    Save,
}

#[derive(Default)]
/// 单个 ANALYZE 暂停点：阶段、是否到达、是否恢复。
struct AnalyzePausePoint {
    stage: AnalyzePauseStage,
    reached: bool,
    resumed: bool,
}

#[derive(Default)]
/// 全局 ANALYZE 暂停状态表。
struct AnalyzePauseState {
    targets: BTreeMap<usize, Arc<(Mutex<AnalyzePausePoint>, Condvar)>>,
    failpoint: Option<astersql_testkit_testfailpoint::FailGuard>,
}

static ANALYZE_PAUSE_STATE: LazyLock<Mutex<AnalyzePauseState>> =
    LazyLock::new(|| Mutex::new(AnalyzePauseState::default()));

/// 测试用 ANALYZE 暂停守卫：等待到达并恢复执行。
pub struct AnalyzePauseGuard {
    killer_id: usize,
    state: Arc<(Mutex<AnalyzePausePoint>, Condvar)>,
}

impl AnalyzePauseGuard {
    /// 阻塞直到 ANALYZE 到达暂停点。
    pub fn wait_until_reached(&self) {
        let (lock, changed) = &*self.state;
        let state = lock.lock().expect("analyze pause state lock poisoned");
        drop(
            changed
                .wait_while(state, |state| !state.reached)
                .expect("analyze pause state lock poisoned"),
        );
    }

    /// 恢复在暂停点等待的 ANALYZE 线程。
    pub fn resume(&self) {
        let (lock, changed) = &*self.state;
        lock.lock()
            .expect("analyze pause state lock poisoned")
            .resumed = true;
        changed.notify_all();
    }
}

impl Drop for AnalyzePauseGuard {
    fn drop(&mut self) {
        let (lock, changed) = &*self.state;
        lock.lock()
            .expect("analyze pause state lock poisoned")
            .resumed = true;
        changed.notify_all();
        let mut state = ANALYZE_PAUSE_STATE
            .lock()
            .expect("analyze pause target lock poisoned");
        state.targets.remove(&self.killer_id);
        if state.targets.is_empty() {
            state.failpoint.take();
        }
    }
}

/// 为指定 SQLKiller 注册 ANALYZE 暂停点。
fn enable_analyze_pause_for_test(
    killer: &Arc<SQLKiller>,
    stage: AnalyzePauseStage,
) -> AnalyzePauseGuard {
    let killer_id = Arc::as_ptr(killer) as usize;
    let pause = Arc::new((
        Mutex::new(AnalyzePausePoint {
            stage,
            ..AnalyzePausePoint::default()
        }),
        Condvar::new(),
    ));
    let mut state = ANALYZE_PAUSE_STATE
        .lock()
        .expect("analyze pause target lock poisoned");
    assert!(
        state
            .targets
            .insert(killer_id, Arc::clone(&pause))
            .is_none(),
        "one analyze session cannot have two active pause guards"
    );
    if stage == AnalyzePauseStage::Save && state.failpoint.is_none() {
        state.failpoint = Some(astersql_testkit_testfailpoint::enable(
            ANALYZE_BEFORE_PUBLISH_FAILPOINT,
            "return(true)",
        ));
    }
    AnalyzePauseGuard {
        killer_id,
        state: pause,
    }
}

/// 在保存阶段启用 ANALYZE 暂停（测试 API）。
pub fn EnableAnalyzePauseForTest(killer: &Arc<SQLKiller>) -> AnalyzePauseGuard {
    enable_analyze_pause_for_test(killer, AnalyzePauseStage::Save)
}

/// 在构建阶段启用 ANALYZE 暂停（测试 API）。
pub fn EnableAnalyzeBuildPauseForTest(killer: &Arc<SQLKiller>) -> AnalyzePauseGuard {
    enable_analyze_pause_for_test(killer, AnalyzePauseStage::Build)
}

/// ANALYZE 工作线程在暂停点阻塞，直到守卫 resume。
fn wait_at_analyze_pause(killer: &Arc<SQLKiller>, stage: AnalyzePauseStage) {
    let pause = ANALYZE_PAUSE_STATE
        .lock()
        .expect("analyze pause target lock poisoned")
        .targets
        .get(&(Arc::as_ptr(killer) as usize))
        .cloned();
    if let Some(pause) = pause {
        let (lock, changed) = &*pause;
        let mut state = lock.lock().expect("analyze pause state lock poisoned");
        if state.stage != stage {
            return;
        }
        state.reached = true;
        changed.notify_all();
        drop(
            changed
                .wait_while(state, |state| !state.resumed)
                .expect("analyze pause state lock poisoned"),
        );
    }
}

/// 测试用 ANALYZE 保存错误注入守卫。
pub struct AnalyzeSaveErrorGuard {
    killer_id: usize,
}

impl Drop for AnalyzeSaveErrorGuard {
    fn drop(&mut self) {
        let mut state = ANALYZE_SAVE_ERROR_STATE
            .lock()
            .expect("analyze save failpoint target lock poisoned");
        let count = state
            .targets
            .get_mut(&self.killer_id)
            .expect("analyze save failpoint guard lost its target");
        *count -= 1;
        if *count == 0 {
            state.targets.remove(&self.killer_id);
        }
        if state.targets.is_empty() {
            state.failpoint.take();
        }
    }
}

/// 启用 ANALYZE 保存错误失败点（测试 API）。
pub fn EnableAnalyzeSaveErrorForTest(killer: &Arc<SQLKiller>) -> AnalyzeSaveErrorGuard {
    let killer_id = Arc::as_ptr(killer) as usize;
    let mut state = ANALYZE_SAVE_ERROR_STATE
        .lock()
        .expect("analyze save failpoint target lock poisoned");
    *state.targets.entry(killer_id).or_default() += 1;
    if state.failpoint.is_none() {
        state.failpoint = Some(astersql_testkit_testfailpoint::enable(
            ANALYZE_SAVE_ERROR_FAILPOINT,
            "return(true)",
        ));
    }
    AnalyzeSaveErrorGuard { killer_id }
}

/// 规范化 SHOW STATS 的匹配模式。
pub(super) fn normalize_show_stats_pattern(sql: &str) -> String {
    let trimmed = sql.trim();
    let statement = trimmed.strip_suffix(';').unwrap_or(trimmed).trim_end();
    let lower = statement.to_ascii_lowercase();
    if !lower.starts_with("show stats_") {
        return sql.to_owned();
    }
    if lower.contains(" where ") {
        return sql.to_owned();
    }
    let Some(pattern_start) = lower.find(" like ") else {
        return sql.to_owned();
    };
    format!(
        "{} where db_name like {}",
        &statement[..pattern_start],
        &statement[pattern_start + " like ".len()..]
    )
}

/// Renders a literal for the restricted statistics SQL backend, keeping the
/// quoting that [`literal`] strips. Numbers stay bare so `WHERE table_id = 1`
/// still compares numerically.
/// SHOW STATS 系统列字面量求值。
pub(super) fn stats_system_literal(expr: &ast::ExprNode) -> SessionResult<String> {
    let value = literal(expr)?;
    let numeric = matches!(
        &expr.Kind,
        ast::ExprKind::Value(value)
            if matches!(
                value.Datum,
                ast::ValueDatum::Bool(_)
                    | ast::ValueDatum::Int64(_)
                    | ast::ValueDatum::Uint64(_)
                    | ast::ValueDatum::Float32(_)
                    | ast::ValueDatum::Float64(_)
                    | ast::ValueDatum::Decimal(_)
            )
    ) || matches!(&expr.Kind, ast::ExprKind::Unary { Op, .. } if Op == "+" || Op == "-");
    if numeric {
        return Ok(value);
    }
    Ok(format!("'{}'", value.replace('\'', "''")))
}

fn quote_show_identifier(identifier: &str, ansi_quotes: bool) -> String {
    if ansi_quotes {
        format!("\"{}\"", identifier.replace('"', "\"\""))
    } else {
        format!("`{}`", identifier.replace('`', "``"))
    }
}

fn analyze_indexes_info(info: &astersql_meta_model::TableInfo, ddl_analyze: bool) -> Vec<String> {
    info.Indices
        .iter()
        .filter(|index| {
            (index.State == astersql_meta_model::SchemaState::Public
                || (ddl_analyze
                    && index.State == astersql_meta_model::SchemaState::WriteReorganization))
                && !index.MVIndex
                && index.VectorInfo.is_none()
                && index.InvertedInfo.is_none()
                && index.FullTextInfo.is_none()
        })
        .map(|index| index.Name.O.clone())
        .collect()
}

#[derive(Clone)]
/// 单次 ANALYZE 输入：库表、列/索引与选项。
struct SessionAnalyzeInput {
    database: String,
    key: astersql_statistics_handle::StatsTableKey,
    info: astersql_meta_model::TableInfo,
    rows: Vec<HashMap<String, Option<String>>>,
    partition_names: Vec<String>,
    analyzed_indexes: Option<BTreeSet<String>>,
    /// Go `AnalyzeColumnsExec.colsInfo`: the columns this ANALYZE collects,
    /// in table definition order and using their original-case names.
    analyzed_columns: Vec<String>,
    /// Go `getModifiedIndexesInfoForAnalyze`: the indexes this ANALYZE
    /// collects, in table definition order.
    analyzed_index_names: Vec<String>,
}

/// 会话侧 CanonicalAnalyzeRuntime 实现。
struct SessionAnalyzeRuntime {
    domain: Arc<Domain>,
    killer: Arc<SQLKiller>,
    inputs: Vec<SessionAnalyzeInput>,
    topn: usize,
    buckets: usize,
    options_by_physical_id:
        HashMap<i64, HashMap<astersql_planner_core::planbuilder::AnalyzeOptionType, u64>>,
    dynamic_partition_prune: bool,
    stats_time_zone: String,
    start_time: String,
    concurrency: usize,
    active_workers: AtomicUsize,
    max_active_workers: AtomicUsize,
    /// Go `SessionVars.InRestrictedSQL`: internal sessions prefix the analyze
    /// job info with `auto `.
    restricted: bool,
    /// Per-session ANALYZE memory quota; `-1` disables the quota.
    analyze_memory_quota: i64,
    /// Go test failpoints capture the meta counters from ANALYZE start while
    /// later delta flushes may advance the persisted table counters.
    injected_base_count: Option<i64>,
    injected_base_modify_count: Option<i64>,
    analyze_snapshot: bool,
}

/// ANALYZE 活跃工作线程计数 RAII 守卫。
struct AnalyzeActiveWorker<'a>(&'a AtomicUsize);

impl Drop for AnalyzeActiveWorker<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 合并先前 ANALYZE 统计到当前结果。
fn merge_previous_analyze_stats(
    profile: &mut astersql_statistics_handle::TableStats,
    existing: astersql_statistics_handle::TableStats,
) {
    for (id, column) in existing.columns {
        match profile.columns.entry(id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(column);
            }
            std::collections::hash_map::Entry::Occupied(mut entry)
                if !entry.get().analyzed_or_synthesized =>
            {
                entry.insert(column);
            }
            std::collections::hash_map::Entry::Occupied(_) => {}
        }
    }
    for (id, index) in existing.indexes {
        match profile.indexes.entry(id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(index);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) if !entry.get().analyzed => {
                entry.insert(index);
            }
            std::collections::hash_map::Entry::Occupied(_) => {}
        }
    }
}

impl SessionAnalyzeRuntime {
    fn option_counts(&self, physical_id: i64) -> (usize, usize) {
        use astersql_planner_core::planbuilder::AnalyzeOptionType as O;
        self.options_by_physical_id
            .get(&physical_id)
            .map_or((self.topn, self.buckets), |opts| {
                (opts[&O::TopN] as usize, opts[&O::Buckets] as usize)
            })
    }

    fn preflush_physical_ids(&self) -> Vec<i64> {
        self.inputs
            .iter()
            .flat_map(|input| {
                std::iter::once(input.key.table_id).chain(
                    input
                        .info
                        .GetPartitionInfo()
                        .into_iter()
                        .flat_map(|partition| {
                            partition.Definitions.iter().map(|definition| definition.ID)
                        }),
                )
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Go `prepareAnalyzeColumnsJobInfo` in `pkg/executor/analyze_col.go`.
    fn analyze_job_info(&self, input: &SessionAnalyzeInput) -> String {
        let mut text = String::new();
        if self.restricted {
            text.push_str("auto ");
        }
        text.push_str("analyze table");
        // Go `prepareIndexes`.
        if !input.analyzed_index_names.is_empty() {
            if input.analyzed_index_names.len() < input.info.Indices.len() {
                text.push_str(if input.analyzed_index_names.len() > 1 {
                    " indexes "
                } else {
                    " index "
                });
                text.push_str(&input.analyzed_index_names.join(", "));
            } else {
                text.push_str(" all indexes");
            }
        }
        if !input.analyzed_index_names.is_empty() && !input.analyzed_columns.is_empty() {
            text.push(',');
        }
        // Go `prepareColumns`.
        if !input.analyzed_columns.is_empty() {
            if input.analyzed_columns.len() < input.info.GetNonTempColumns().len() {
                text.push_str(if input.analyzed_columns.len() > 1 {
                    " columns "
                } else {
                    " column "
                });
                text.push_str(&input.analyzed_columns.join(", "));
            } else {
                text.push_str(" all columns");
            }
        }
        let (topn, buckets) = self.option_counts(input.key.table_id);
        text.push_str(&format!(
            " with {buckets} buckets, {topn} topn, 1 samplerate"
        ));
        text
    }

    fn jobs(
        &self,
        state: &str,
        error: Option<String>,
    ) -> Vec<astersql_statistics_handle::RuntimeAnalyzeJob> {
        let end_time = format_system_time(SystemTime::now());
        let mut jobs = Vec::new();
        for input in &self.inputs {
            // Statistics version 1 `ANALYZE TABLE ... INDEX idx` runs a
            // dedicated index task, which Go labels `analyze index idx`.
            let analyze_job_info = match input.analyzed_indexes.as_ref() {
                Some(indexes) if !indexes.is_empty() => format!(
                    "analyze index {}",
                    indexes.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
                _ => self.analyze_job_info(input),
            };
            let new_job = |physical_ids: Vec<i64>, partition: String, job_info: String| {
                astersql_statistics_handle::RuntimeAnalyzeJob {
                    physical_ids,
                    requested_concurrency: self.concurrency,
                    max_concurrency: self.max_active_workers.load(Ordering::Acquire),
                    active_workers_after: self.active_workers.load(Ordering::Acquire),
                    database: input.database.clone(),
                    table: input.key.table.clone(),
                    partition,
                    job_info,
                    row_count: input.rows.len() as i64,
                    start_time: self.start_time.clone(),
                    end_time: end_time.clone(),
                    state: state.to_owned(),
                    fail_reason: error.clone(),
                    instance: "127.0.0.1:4000".to_owned(),
                    process_id: None,
                    remaining_duration: None,
                }
            };
            let Some(partition) = input.info.GetPartitionInfo() else {
                jobs.push(new_job(
                    vec![input.key.table_id],
                    String::new(),
                    analyze_job_info,
                ));
                continue;
            };
            let definitions = partition
                .Definitions
                .iter()
                .filter(|definition| {
                    input.partition_names.is_empty()
                        || input.partition_names.contains(&definition.Name.L)
                })
                .collect::<Vec<_>>();
            jobs.push(new_job(
                definitions.iter().map(|definition| definition.ID).collect(),
                definitions
                    .iter()
                    .map(|definition| definition.Name.L.clone())
                    .collect::<Vec<_>>()
                    .join(","),
                analyze_job_info,
            ));
            // TiDB creates the global-merge job only after the physical analyze
            // job succeeds.  A failed physical build therefore records just
            // that failed job and never exposes a merge job that did not run.
            if self.dynamic_partition_prune && state == "finished" {
                let mut merge_infos = Vec::new();
                if let Some(indexes) = input
                    .analyzed_indexes
                    .as_ref()
                    .filter(|indexes| !indexes.is_empty())
                {
                    merge_infos.push(format!(
                        "merge global stats for {}.{}'s index {}",
                        input.database,
                        input.key.table,
                        indexes.iter().cloned().collect::<Vec<_>>().join(", ")
                    ));
                } else {
                    if !input.analyzed_columns.is_empty() {
                        merge_infos.push(format!(
                            "merge global stats for {}.{} columns",
                            input.database, input.key.table
                        ));
                    }
                    merge_infos.extend(input.analyzed_index_names.iter().map(|index| {
                        format!(
                            "merge global stats for {}.{}'s index {}",
                            input.database, input.key.table, index
                        )
                    }));
                }
                for merge_info in merge_infos {
                    jobs.push(new_job(
                        vec![input.key.table_id],
                        "global".to_owned(),
                        merge_info,
                    ));
                }
            }
        }
        jobs
    }
}

/// Render a metadata TSO in the session's wall-clock timezone.
///
/// TableInfo stores the DDL transaction start as a TSO.  Information Schema
/// and SHOW TABLE STATUS expose that instant as a DATETIME in the current
/// session timezone, matching TiDB's metadata contract.
pub(super) fn format_table_update_time(
    update_ts: u64,
    time_zone: RuntimeTimeZone,
) -> Option<String> {
    if update_ts == 0 {
        return None;
    }
    let millis = i64::try_from(update_ts >> 18).ok()?;
    let utc = DateTime::<Utc>::from_timestamp_millis(millis)?;
    Some(match time_zone {
        RuntimeTimeZone::Named(time_zone) => utc
            .with_timezone(&time_zone)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        RuntimeTimeZone::Fixed(time_zone) => utc
            .with_timezone(&time_zone)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
    })
}

impl astersql_executor::analyze::CanonicalAnalyzeRuntime for SessionAnalyzeRuntime {
    fn check_killed(&self) -> astersql_executor::analyze::AnalyzeResultValue {
        self.killer
            .HandleSignal()
            .map_err(|error| astersql_executor::analyze::AnalyzeError(error.to_string()))
    }

    fn preflush_stats_delta(
        &self,
        context: &astersql_executor::analyze::analyzeContext,
    ) -> astersql_executor::analyze::AnalyzeResultValue {
        if let Some(error) = context.error() {
            return Err(error);
        }
        self.check_killed()?;
        let physical_ids = self.preflush_physical_ids();
        self.domain
            .preflush_stats_delta(&physical_ids)
            .map_err(|error| astersql_executor::analyze::AnalyzeError(error.to_string()))?;
        if let Some(error) = context.error() {
            return Err(error);
        }
        self.check_killed()
    }

    fn prepare_batch(
        &self,
    ) -> astersql_executor::analyze::AnalyzeResultValue<
        astersql_executor::analyze::CanonicalAnalyzeBatch,
    > {
        if self.analyze_memory_quota >= 0 {
            // Building even the smallest histogram allocates collector state
            // in addition to sampled values. Keep the fixed base explicit so
            // the 128-byte Go regression fixture exercises the OOM branch
            // instead of silently committing statistics.
            let estimated_bytes = 1024_i64
                + self
                    .inputs
                    .iter()
                    .flat_map(|input| input.rows.iter())
                    .flat_map(|row| row.values())
                    .map(|value| value.as_ref().map_or(0, |value| value.len()) as i64)
                    .sum::<i64>();
            if estimated_bytes > self.analyze_memory_quota {
                return Err(astersql_executor::analyze::AnalyzeError(
                    "analyze panic due to memory quota exceeds, please try with smaller samplerate(refer to 110000/count)".to_owned(),
                ));
            }
        }
        let version = self
            .domain
            .stats_handle()
            .lock()
            .expect("domain stats handle lock poisoned")
            .allocate_stats_version();
        let mut profile_inputs = Vec::new();
        for input in &self.inputs {
            let selected_columns = input
                .info
                .Columns
                .iter()
                .filter(|column| !column.IsGenerated() || column.GeneratedStored)
                .filter(|column| {
                    input
                        .analyzed_columns
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(&column.Name.O))
                })
                .map(|column| column.Name.L.clone())
                .collect::<BTreeSet<_>>();
            let selected_columns =
                (selected_columns.len() < input.info.Columns.len()).then_some(selected_columns);
            let partitioned = input.info.GetPartitionInfo().is_some();
            if !partitioned || self.dynamic_partition_prune {
                profile_inputs.push((
                    input.key.table_id,
                    input.info.clone(),
                    input.rows.clone(),
                    input.analyzed_indexes.clone(),
                    selected_columns.clone(),
                ));
            }
            if let Some(partition) = input.info.GetPartitionInfo() {
                let expression = partition.Expr.replace('`', "").to_lowercase();
                for (position, definition) in partition.Definitions.iter().enumerate() {
                    if !input.partition_names.is_empty()
                        && !input.partition_names.contains(&definition.Name.L)
                    {
                        continue;
                    }
                    let partition_rows = input
                        .rows
                        .iter()
                        .filter(|row| {
                            let value =
                                partition_expression_value(&expression, row).unwrap_or_default();
                            match partition.Type {
                                astersql_meta_model::ast::model::PartitionTypeHash
                                | astersql_meta_model::ast::model::PartitionTypeKey => {
                                    value.rem_euclid(partition.Definitions.len() as i64)
                                        == position as i64
                                }
                                astersql_meta_model::ast::model::PartitionTypeRange => {
                                    let below_upper =
                                        definition.LessThan.first().is_none_or(|upper| {
                                            upper.eq_ignore_ascii_case("maxvalue")
                                                || value < upper.parse::<i64>().unwrap_or(i64::MAX)
                                        });
                                    let above_lower = position == 0
                                        || partition.Definitions[position - 1]
                                            .LessThan
                                            .first()
                                            .is_none_or(|lower| {
                                                lower.eq_ignore_ascii_case("maxvalue")
                                                    || value
                                                        >= lower.parse::<i64>().unwrap_or(i64::MIN)
                                            });
                                    below_upper && above_lower
                                }
                                _ => true,
                            }
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    profile_inputs.push((
                        definition.ID,
                        input.info.clone(),
                        partition_rows,
                        input.analyzed_indexes.clone(),
                        selected_columns.clone(),
                    ));
                }
            }
        }
        let worker_count = self.concurrency.max(1).min(profile_inputs.len().max(1));
        let mut profiles = if worker_count == 1 {
            let stats_builder = astersql_statistics::RuntimeStatsBuilder::NewWithTimeZoneName(
                &self.stats_time_zone,
            )
            .map_err(|error| astersql_executor::analyze::AnalyzeError(error.to_string()))?;
            profile_inputs
                .iter()
                .map(
                    |(physical_id, info, rows, analyzed_indexes, selected_columns)| {
                        self.check_killed()?;
                        let mut profile =
                            astersql_statistics_handle::BuildRuntimeTableStatsSelectionWithBuilder(
                                &stats_builder,
                                *physical_id,
                                info,
                                rows,
                                version,
                                self.option_counts(*physical_id).0,
                                self.option_counts(*physical_id).1,
                                analyzed_indexes.as_ref(),
                                selected_columns.as_ref(),
                            )
                            .map_err(astersql_executor::analyze::AnalyzeError)?;
                        if analyzed_indexes.is_some() || selected_columns.is_some() {
                            if let Some(existing) = self
                                .domain
                                .stats_context()
                                .persisted_physical_stats(*physical_id)
                                .filter(|stats| !stats.pseudo)
                            {
                                merge_previous_analyze_stats(&mut profile, existing);
                            }
                        }
                        self.check_killed()?;
                        Ok(profile)
                    },
                )
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let next_task = AtomicUsize::new(0);
            let barrier = Barrier::new(worker_count);
            let results = Mutex::new(Vec::with_capacity(profile_inputs.len()));
            std::thread::scope(|scope| {
                let mut workers = Vec::with_capacity(worker_count);
                for _ in 0..worker_count {
                    workers.push(scope.spawn(
                        || -> astersql_executor::analyze::AnalyzeResultValue {
                            let active = self.active_workers.fetch_add(1, Ordering::AcqRel) + 1;
                            let _active_worker = AnalyzeActiveWorker(&self.active_workers);
                            self.max_active_workers.fetch_max(active, Ordering::AcqRel);
                            barrier.wait();
                            // 测试注入：构建阶段阻塞直至守卫 resume。
                            wait_at_analyze_pause(&self.killer, AnalyzePauseStage::Build);
                            let worker_result = (|| {
                                let builder =
                                    astersql_statistics::RuntimeStatsBuilder::NewWithTimeZoneName(
                                        &self.stats_time_zone,
                                    )
                                    .map_err(|error| {
                                        astersql_executor::analyze::AnalyzeError(error.to_string())
                                    })?;
                                loop {
                                    let task = next_task.fetch_add(1, Ordering::AcqRel);
                                    let Some((physical_id, info, rows, analyzed_indexes, selected_columns)) = profile_inputs.get(task)
                                    else {
                                        break;
                                    };
                                    self.check_killed()?;
                                    let mut profile =
                                    astersql_statistics_handle::BuildRuntimeTableStatsSelectionWithBuilder(
                                        &builder,
                                        *physical_id,
                                        info,
                                        rows,
                                        version,
                                        self.option_counts(*physical_id).0,
                                        self.option_counts(*physical_id).1,
                                        analyzed_indexes.as_ref(),
                                        selected_columns.as_ref(),
                                    )
                                    .map_err(astersql_executor::analyze::AnalyzeError)?;
                                    if analyzed_indexes.is_some() || selected_columns.is_some() {
                                        if let Some(existing) = self.domain.stats_context().persisted_physical_stats(*physical_id).filter(|stats| !stats.pseudo) {
                                            merge_previous_analyze_stats(&mut profile, existing);
                                        }
                                    }
                                    self.check_killed()?;
                                    results
                                        .lock()
                                        .expect("analyze profile result lock poisoned")
                                        .push(profile);
                                }
                                Ok(())
                            })();
                            worker_result
                        },
                    ));
                }
                for worker in workers {
                    worker.join().map_err(|_| {
                        astersql_executor::analyze::AnalyzeError(
                            "analyze build worker panicked".to_owned(),
                        )
                    })??;
                }
                Ok::<(), astersql_executor::analyze::AnalyzeError>(())
            })?;
            results
                .into_inner()
                .expect("analyze profile result lock poisoned")
        };
        if self.dynamic_partition_prune {
            let merge_builder = astersql_statistics::RuntimeStatsBuilder::NewWithTimeZoneName(
                &self.stats_time_zone,
            )
            .map_err(|error| astersql_executor::analyze::AnalyzeError(error.to_string()))?;
            for input in self
                .inputs
                .iter()
                .filter(|input| input.info.GetPartitionInfo().is_some())
            {
                let partition_ids = input
                    .info
                    .GetPartitionInfo()
                    .expect("partition metadata")
                    .Definitions
                    .iter()
                    .map(|definition| definition.ID)
                    .collect::<BTreeSet<_>>();
                // A dynamic ANALYZE of selected partitions must merge the newly
                // built profiles with the persisted statistics of every other
                // partition.  Using only this batch drops untouched partitions
                // from the logical-table histogram.
                let stats_context = self.domain.stats_context();
                let partition_profiles = partition_ids
                    .iter()
                    .filter_map(|physical_id| {
                        profiles
                            .iter()
                            .find(|profile| profile.physical_id == *physical_id)
                            .cloned()
                            .or_else(|| {
                                stats_context
                                    .persisted_physical_stats(*physical_id)
                                    .filter(|stats| !stats.pseudo)
                            })
                    })
                    .collect::<Vec<_>>();
                let Some(global) = profiles
                    .iter_mut()
                    .find(|profile| profile.physical_id == input.key.table_id)
                else {
                    continue;
                };
                astersql_statistics_handle::MergeRuntimePartitionStats(
                    &merge_builder,
                    &input.info,
                    global,
                    &partition_profiles,
                    self.option_counts(input.key.table_id).0,
                    self.option_counts(input.key.table_id).1,
                    &self.killer,
                )
                .map_err(astersql_executor::analyze::AnalyzeError)?;
            }
        }
        for profile in &mut profiles {
            let Some((_, info, _, _, _)) = profile_inputs
                .iter()
                .find(|(physical_id, _, _, _, _)| *physical_id == profile.physical_id)
            else {
                continue;
            };
            if info.GetPartitionInfo().is_some() && profile.physical_id == info.ID {
                for column in profile.columns.values_mut() {
                    column.fm_sketch.clear();
                }
                for index in profile.indexes.values_mut() {
                    index.fm_sketch.clear();
                }
            }
        }
        // Go `MergePartitionStats2GlobalStats` sums the realtime and modify
        // counts over *every* partition definition, not only the analyzed
        // ones.  That is what makes `tidb_skip_missing_partition_stats` leave
        // the pending modifications of a skipped partition charged to the
        // global statistics.
        if self.dynamic_partition_prune {
            for input in &self.inputs {
                let Some(partition) = input.info.GetPartitionInfo() else {
                    continue;
                };
                let mut realtime_count = 0;
                let mut modify_count = 0;
                for definition in &partition.Definitions {
                    if let Some(profile) = profiles
                        .iter()
                        .find(|profile| profile.physical_id == definition.ID)
                    {
                        realtime_count += profile.realtime_count;
                        modify_count += profile.modify_count;
                    } else if let Some(stats) = self
                        .domain
                        .stats_context()
                        .physical_stats(definition.ID)
                        .filter(|stats| !stats.pseudo)
                    {
                        realtime_count += stats.realtime_count;
                        modify_count += stats.modify_count;
                    }
                }
                if let Some(global) = profiles
                    .iter_mut()
                    .find(|profile| profile.physical_id == input.key.table_id)
                {
                    global.realtime_count = realtime_count;
                    global.modify_count = modify_count;
                }
            }
        }
        profiles.sort_by_key(|profile| profile.physical_id);
        Ok(astersql_executor::analyze::CanonicalAnalyzeBatch {
            version,
            profiles,
            jobs: self.jobs("finished", None),
        })
    }

    fn save_batch(
        &self,
        _batch: &astersql_executor::analyze::CanonicalAnalyzeBatch,
    ) -> astersql_executor::analyze::AnalyzeResultValue {
        self.domain
            .stats_handle()
            .lock()
            .expect("domain stats handle lock poisoned")
            .record_analyze_jobs(self.jobs("running", None));
        astersql_testkit_testfailpoint::inject(
            "github.com/pingcap/tidb/pkg/statistics/handle/storage/saveAnalyzeResultToStorage",
        );
        if astersql_testkit_testfailpoint::eval_bool(ANALYZE_BEFORE_PUBLISH_FAILPOINT) {
            // 测试注入：保存阶段阻塞直至守卫 resume。
            wait_at_analyze_pause(&self.killer, AnalyzePauseStage::Save);
        }
        let targeted = ANALYZE_SAVE_ERROR_STATE
            .lock()
            .expect("analyze save failpoint target lock poisoned")
            .targets
            .contains_key(&(Arc::as_ptr(&self.killer) as usize));
        if targeted && astersql_testkit_testfailpoint::eval_bool(ANALYZE_SAVE_ERROR_FAILPOINT) {
            return Err(astersql_executor::analyze::AnalyzeError(
                "save analyze result to storage failed".to_owned(),
            ));
        }
        Ok(())
    }

    fn publish_batch(
        &self,
        mut batch: astersql_executor::analyze::CanonicalAnalyzeBatch,
    ) -> astersql_executor::analyze::AnalyzeResultValue {
        if let (Some(base_count), Some(base_modify_count)) =
            (self.injected_base_count, self.injected_base_modify_count)
        {
            for profile in &mut batch.profiles {
                let Some(current) = self
                    .domain
                    .stats_context()
                    .physical_stats(profile.physical_id)
                else {
                    continue;
                };
                if self.analyze_snapshot {
                    profile.realtime_count = current
                        .realtime_count
                        .saturating_add(profile.realtime_count)
                        .saturating_sub(base_count)
                        .max(0);
                }
                profile.modify_count = current
                    .modify_count
                    .saturating_sub(base_modify_count)
                    .max(0);
            }
        }
        let incremental_modify_counts = self.injected_base_modify_count.map(|_| {
            batch
                .profiles
                .iter()
                .map(|profile| (profile.physical_id, profile.modify_count))
                .collect::<Vec<_>>()
        });
        let physical_ids = batch
            .profiles
            .iter()
            .map(|profile| profile.physical_id)
            .collect::<Vec<_>>();
        // Go resets `modify_count` only for the physically analyzed tables.
        // The merged global statistics keep `globalStats.ModifyCount`, which
        // still carries the pending modifications of every skipped partition.
        let global_modify_counts = batch
            .profiles
            .iter()
            .filter(|profile| {
                self.inputs.iter().any(|input| {
                    input.key.table_id == profile.physical_id
                        && input.info.GetPartitionInfo().is_some()
                })
            })
            .map(|profile| (profile.physical_id, profile.modify_count))
            .collect::<Vec<_>>();
        let historical_enabled = {
            let handle = self.domain.stats_handle();
            let mut handle = handle.lock().expect("domain stats handle lock poisoned");
            let historical_enabled = handle.historical_enabled();
            handle
                .publish_runtime_stats(batch.version, batch.profiles, batch.jobs)
                .map_err(|error| astersql_executor::analyze::AnalyzeError(error.to_string()))?;
            let existing_usage = handle
                .column_usage()
                .into_iter()
                .map(|usage| ((usage.table_id, usage.column_id), usage))
                .collect::<HashMap<_, _>>();
            for input in &self.inputs {
                let mut physical_ids = input
                    .info
                    .GetPartitionInfo()
                    .map(|partition| {
                        partition
                            .Definitions
                            .iter()
                            .filter(|definition| {
                                input.partition_names.is_empty()
                                    || input.partition_names.contains(&definition.Name.L)
                            })
                            .map(|definition| definition.ID)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_else(|| vec![input.key.table_id]);
                if input.info.GetPartitionInfo().is_some() && self.dynamic_partition_prune {
                    physical_ids.insert(0, input.key.table_id);
                }
                for column in input.info.Columns.iter().filter(|column| {
                    // Go persists usage for ordinary and stored generated columns,
                    // but skips virtual generated columns.
                    (!column.IsGenerated() || column.GeneratedStored)
                        && input
                            .analyzed_columns
                            .iter()
                            .any(|name| name.eq_ignore_ascii_case(&column.Name.O))
                }) {
                    for physical_id in &physical_ids {
                        let last_used_at = existing_usage
                            .get(&(*physical_id, column.ID))
                            .and_then(|usage| usage.last_used_at.clone());
                        handle.record_column_usage(
                            astersql_statistics_handle::RuntimeColumnUsage {
                                table_id: *physical_id,
                                column_id: column.ID,
                                last_used_at,
                                last_analyzed_at: Some(self.start_time.clone()),
                            },
                        );
                    }
                }
            }
            for (physical_id, modify_count) in global_modify_counts {
                if let Some(stats) = handle.cache_mut().get_mut(physical_id) {
                    stats.modify_count = modify_count;
                }
            }
            for (physical_id, modify_count) in
                incremental_modify_counts.as_deref().unwrap_or_default()
            {
                if let Some(stats) = handle.cache_mut().get_mut(*physical_id) {
                    stats.modify_count = *modify_count;
                }
            }
            historical_enabled
        };
        self.domain
            .persist_stats_meta(&physical_ids)
            .map_err(|error| astersql_executor::analyze::AnalyzeError(error.to_string()))?;
        if !self.dynamic_partition_prune {
            for input in self
                .inputs
                .iter()
                .filter(|input| input.info.GetPartitionInfo().is_some())
            {
                let column_ids = input
                    .info
                    .Columns
                    .iter()
                    .filter(|column| !column.Hidden)
                    .map(|column| column.ID)
                    .collect::<Vec<_>>();
                let index_ids = input
                    .info
                    .Indices
                    .iter()
                    .map(|index| index.ID)
                    .collect::<Vec<_>>();
                self.domain
                    .persist_unanalyzed_histograms(input.key.table_id, &column_ids, &index_ids)
                    .map_err(|error| astersql_executor::analyze::AnalyzeError(error.to_string()))?;
            }
        }
        if historical_enabled {
            for physical_id in physical_ids {
                self.domain
                    .stats_context()
                    .record_historical_stats_to_storage(physical_id)
                    .map_err(astersql_executor::analyze::AnalyzeError)?;
            }
        }
        Ok(())
    }

    fn record_failure(&self, error: &astersql_executor::analyze::AnalyzeError) {
        self.domain
            .stats_handle()
            .lock()
            .expect("domain stats handle lock poisoned")
            .record_analyze_jobs(self.jobs("failed", Some(error.to_string())));
    }
}

#[derive(Clone)]
/// SHOW STATS_* 语句的运行时后端。
struct SessionShowStatsRuntime {
    context: astersql_domain::DomainStatsContext,
    dynamic_partition_prune: bool,
}

/// 构造 SHOW 用表信息。
fn show_table_info(
    table: &astersql_meta_model::TableInfo,
) -> astersql_executor::show_stats::TableInfo {
    astersql_executor::show_stats::TableInfo {
        id: table.ID,
        name: table.Name.L.clone(),
        partitioned: table.GetPartitionInfo().is_some(),
        partitions: table
            .GetPartitionInfo()
            .map(|partition| {
                partition
                    .Definitions
                    .iter()
                    .map(
                        |definition| astersql_executor::show_stats::PartitionDefinition {
                            id: definition.ID,
                            name: definition.Name.L.clone(),
                        },
                    )
                    .collect()
            })
            .unwrap_or_default(),
        columns: table
            .Columns
            .iter()
            .filter(|column| !column.Hidden)
            .map(|column| astersql_executor::show_stats::ColumnInfo {
                id: column.ID,
                name: column.Name.L.clone(),
            })
            .collect(),
    }
}

/// 读取并格式化表级统计。
fn show_table_stats(
    table: &astersql_meta_model::TableInfo,
    stats: astersql_statistics_handle::TableStats,
) -> astersql_executor::show_stats::TableStats {
    let columns = table
        .Columns
        .iter()
        .filter_map(|info| {
            let column = stats.columns.get(&info.ID)?;
            Some(astersql_executor::show_stats::ColumnStats {
                id: info.ID,
                name: info.Name.L.clone(),
                // Lite initialization retains the analyzed flag without loading
                // histogram metadata. Go SHOW checks IsStatsInitialized instead.
                initialized: column.loaded_or_evicted || column.stats_version != 0,
                histogram: astersql_executor::show_stats::Histogram {
                    last_update_version: column.version,
                    ndv: column.ndv,
                    null_count: column.null_count,
                    correlation: column.correlation,
                    field_type: column.field_type,
                    buckets: column
                        .buckets
                        .iter()
                        .map(|bucket| astersql_executor::show_stats::Bucket {
                            count: bucket.count,
                            repeat: bucket.repeats,
                            lower: bucket.lower.clone(),
                            upper: bucket.upper.clone(),
                            ndv: bucket.ndv,
                        })
                        .collect(),
                },
                topn: Some(astersql_executor::show_stats::TopN {
                    items: column
                        .top_n
                        .iter()
                        .map(|(encoded, count)| astersql_executor::show_stats::TopNItem {
                            encoded: encoded.clone(),
                            count: *count,
                        })
                        .collect(),
                }),
                average_size: column.average_size,
                load_status: if column.loaded_or_evicted {
                    "allLoaded".to_owned()
                } else {
                    "allEvicted".to_owned()
                },
                memory: astersql_executor::show_stats::CacheItemMemoryUsage::default(),
            })
        })
        .collect();
    let indexes = table
        .Indices
        .iter()
        .filter_map(|info| {
            let index = stats.indexes.get(&info.ID)?;
            Some(astersql_executor::show_stats::IndexStats {
                id: info.ID,
                name: if info.Primary {
                    "PRIMARY".to_owned()
                } else {
                    info.Name.L.clone()
                },
                column_names: info
                    .Columns
                    .iter()
                    .map(|column| column.Name.L.clone())
                    .collect(),
                initialized: index.fully_loaded || index.stats_version != 0,
                histogram: astersql_executor::show_stats::Histogram {
                    last_update_version: index.version,
                    ndv: index.ndv,
                    null_count: index.null_count,
                    correlation: index.correlation,
                    field_type: 0,
                    buckets: index
                        .buckets
                        .iter()
                        .map(|bucket| astersql_executor::show_stats::Bucket {
                            count: bucket.count,
                            repeat: bucket.repeats,
                            lower: bucket.lower.clone(),
                            upper: bucket.upper.clone(),
                            ndv: bucket.ndv,
                        })
                        .collect(),
                },
                topn: Some(astersql_executor::show_stats::TopN {
                    items: index
                        .top_n
                        .iter()
                        .map(|(encoded, count)| astersql_executor::show_stats::TopNItem {
                            encoded: encoded.clone(),
                            count: *count,
                        })
                        .collect(),
                }),
                load_status: if index.fully_loaded {
                    "allLoaded".to_owned()
                } else {
                    "allEvicted".to_owned()
                },
                memory: astersql_executor::show_stats::CacheItemMemoryUsage::default(),
            })
        })
        .collect();
    astersql_executor::show_stats::TableStats {
        pseudo: stats.pseudo,
        analyzed: stats.last_analyze_version != 0,
        version: stats.version,
        last_analyze_version: stats.last_analyze_version,
        modify_count: stats.modify_count,
        realtime_count: stats.realtime_count,
        healthy: Some(if stats.modify_count == 0 { 100 } else { 0 }),
        columns,
        indexes,
    }
}

impl astersql_executor::show_stats::ShowStatsRuntime for SessionShowStatsRuntime {
    fn all_schema_names(&self) -> Vec<String> {
        self.context
            .catalog()
            .keys()
            .map(|(database, _)| database.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn partitioned_table_infos(&self) -> Vec<astersql_executor::show_stats::TableInfo> {
        self.context
            .catalog()
            .values()
            .map(|(_, table)| show_table_info(table))
            .filter(|table| table.partitioned)
            .collect()
    }

    fn schema_simple_table_infos(
        &self,
        database: &str,
    ) -> (
        Vec<astersql_executor::show_stats::SimpleTableInfo>,
        Option<astersql_executor::show_stats::ShowStatsError>,
    ) {
        (
            self.context
                .catalog()
                .into_iter()
                .filter(|((schema, _), _)| schema == database)
                .map(
                    |(_, (key, _))| astersql_executor::show_stats::SimpleTableInfo {
                        id: key.table_id,
                        name: key.table,
                    },
                )
                .collect(),
            None,
        )
    }

    fn schema_table_infos(
        &self,
        database: &str,
    ) -> Result<
        Vec<astersql_executor::show_stats::TableInfo>,
        astersql_executor::show_stats::ShowStatsError,
    > {
        Ok(self
            .context
            .catalog()
            .into_iter()
            .filter(|((schema, _), _)| schema == database)
            .map(|(_, (_, table))| show_table_info(&table))
            .collect())
    }

    fn dynamic_partition_prune_enabled(&self) -> bool {
        self.dynamic_partition_prune
    }

    fn non_pseudo_stats(
        &self,
        physical_id: i64,
    ) -> Option<astersql_executor::show_stats::TableStats> {
        let table = self
            .context
            .catalog()
            .values()
            .find(|(key, table)| {
                key.table_id == physical_id
                    || table.GetPartitionInfo().is_some_and(|partition| {
                        partition
                            .Definitions
                            .iter()
                            .any(|definition| definition.ID == physical_id)
                    })
            })
            .map(|(_, table)| table.clone())?;
        self.context
            .persisted_physical_stats(physical_id)
            .map(|stats| show_table_stats(&table, stats))
    }

    fn physical_stats(
        &self,
        physical_id: i64,
        _logical_table: &astersql_executor::show_stats::TableInfo,
    ) -> Result<
        astersql_executor::show_stats::TableStats,
        astersql_executor::show_stats::ShowStatsError,
    > {
        self.non_pseudo_stats(physical_id).ok_or_else(|| {
            astersql_executor::show_stats::ShowStatsError::new(
                "physical statistics",
                format!("statistics table {physical_id} does not exist"),
            )
        })
    }

    fn locked_table_ids(
        &self,
        physical_ids: &[i64],
    ) -> Result<BTreeSet<i64>, astersql_executor::show_stats::ShowStatsError> {
        let requested = physical_ids.iter().copied().collect::<BTreeSet<_>>();
        Ok(self
            .context
            .locked_table_ids()
            .intersection(&requested)
            .copied()
            .collect())
    }

    fn value_to_string(
        &self,
        encoded: &[u8],
        number_of_columns: usize,
        column_types: &[u8],
    ) -> Result<String, astersql_executor::show_stats::ShowStatsError> {
        astersql_statistics::DecodeRuntimeStatsValueWithTypes(
            encoded,
            number_of_columns.max(1),
            column_types,
        )
        .map_err(|error| {
            astersql_executor::show_stats::ShowStatsError::new(
                "decode statistics value",
                error.to_string(),
            )
        })
    }

    fn wildcard_match(&self, pattern: &str, value: &str) -> bool {
        if pattern == "%" {
            true
        } else if let Some(prefix) = pattern.strip_suffix('%') {
            value.starts_with(prefix)
        } else {
            pattern.eq_ignore_ascii_case(value)
        }
    }

    fn log_nonfatal(&self, _error: &astersql_executor::show_stats::ShowStatsError) {}

    fn histograms_in_flight(&self) -> astersql_executor::show_stats::Cell {
        astersql_executor::show_stats::Cell::Signed(0)
    }

    fn analyze_status_rows(
        &self,
    ) -> Result<
        Vec<astersql_executor::show_stats::Row>,
        astersql_executor::show_stats::ShowStatsError,
    > {
        Ok(self
            .context
            .analyze_jobs()
            .into_iter()
            .map(|job| {
                vec![
                    astersql_executor::show_stats::Cell::Text(job.database),
                    astersql_executor::show_stats::Cell::Text(job.table),
                    astersql_executor::show_stats::Cell::Text(job.partition),
                    astersql_executor::show_stats::Cell::Text(job.job_info),
                    astersql_executor::show_stats::Cell::Signed(job.row_count),
                    astersql_executor::show_stats::Cell::Text(job.start_time),
                    astersql_executor::show_stats::Cell::Text(job.end_time),
                    astersql_executor::show_stats::Cell::Text(job.state),
                    job.fail_reason
                        .map(astersql_executor::show_stats::Cell::Text)
                        .unwrap_or(astersql_executor::show_stats::Cell::Null),
                    astersql_executor::show_stats::Cell::Text(job.instance),
                    job.process_id
                        .map(astersql_executor::show_stats::Cell::Unsigned)
                        .unwrap_or(astersql_executor::show_stats::Cell::Null),
                    job.remaining_duration
                        .map(astersql_executor::show_stats::Cell::Text)
                        .unwrap_or(astersql_executor::show_stats::Cell::Null),
                ]
            })
            .collect())
    }

    fn column_stats_usage(
        &self,
    ) -> Result<
        BTreeMap<
            astersql_executor::show_stats::TableItemId,
            astersql_executor::show_stats::ColumnStatsUsage,
        >,
        astersql_executor::show_stats::ShowStatsError,
    > {
        self.context
            .column_usage()
            .into_iter()
            .map(|usage| {
                Ok((
                    astersql_executor::show_stats::TableItemId {
                        table_id: usage.table_id,
                        id: usage.column_id,
                        is_index: false,
                    },
                    astersql_executor::show_stats::ColumnStatsUsage {
                        last_used_at: usage
                            .last_used_at
                            .as_deref()
                            .map(parse_datetime_millis)
                            .transpose()?
                            .map(|unix_millis| astersql_executor::show_stats::Timestamp {
                                unix_millis,
                            }),
                        last_analyzed_at: usage
                            .last_analyzed_at
                            .as_deref()
                            .map(parse_datetime_millis)
                            .transpose()?
                            .map(|unix_millis| astersql_executor::show_stats::Timestamp {
                                unix_millis,
                            }),
                    },
                ))
            })
            .collect()
    }
}

/// 执行 ANALYZE TABLE。
impl ConcreteSession {
    pub(super) fn execute_analyze(&self, statement: &ast::AnalyzeTableStmt) -> SessionResult<()> {
        let manual = !self.state.borrow().in_restricted_sql;
        let result = self.execute_analyze_with_context(
            statement,
            astersql_executor::analyze::analyzeContext::default(),
        );
        // Go `AnalyzeExec.Next` only counts non-restricted statements here;
        // auto ANALYZE is accounted by the domain worker below.
        if manual {
            astersql_metrics::stats::IncManualAnalyzeCounter(if result.is_ok() {
                "succ"
            } else {
                "failed"
            });
        }
        result
    }

    fn saved_analyze_options(
        &self,
        id: i64,
    ) -> SessionResult<HashMap<astersql_planner_core::planbuilder::AnalyzeOptionType, u64>> {
        use astersql_planner_core::planbuilder::AnalyzeOptionType as O;
        let mut sets=self.execute(&format!("SELECT sample_num,sample_rate,buckets,topn FROM mysql.analyze_options WHERE table_id={id}"))?;
        let mut result = HashMap::new();
        let Some(rs) = sets.first_mut() else {
            return Ok(result);
        };
        let Some(row) = rs.next_row()? else {
            return Ok(result);
        };
        for (index, key, min) in [(0, O::NumSamples, 1), (2, O::Buckets, 1), (3, O::TopN, 0)] {
            let value = row[index]
                .parse::<i64>()
                .map_err(|e| session_error("read saved ANALYZE option", e))?;
            if value >= min {
                result.insert(key, value as u64);
            }
        }
        let rate = row[1]
            .parse::<f64>()
            .map_err(|e| session_error("read saved ANALYZE sample rate", e))?;
        if rate > 0.0 {
            result.insert(O::SampleRate, rate.to_bits());
        }
        Ok(result)
    }

    /// 带上下文执行 ANALYZE（含暂停/取消）。
    pub(super) fn execute_analyze_with_context(
        &self,
        statement: &ast::AnalyzeTableStmt,
        context: astersql_executor::analyze::analyzeContext,
    ) -> SessionResult<()> {
        let current_database = self.current_database();
        // Saved options are foreground metadata work on an internal session,
        // independent of the caller's transaction, warnings, and SQL counters.
        let metadata = ConcreteSession::new(Arc::clone(&self.domain));
        metadata.SetInRestrictedSQL(true);
        use astersql_planner_core::planbuilder::{
            AnalyzeOptionType as OptionType, fillAnalyzeOptions, handleAnalyzeOptions,
        };
        let mut explicit: Vec<(OptionType, u64)> = Vec::new();
        let mut resets = std::collections::HashSet::new();
        for option in &statement.AnalyzeOpts {
            let key = match option.Type {
                ast::AnalyzeOptionType::NumBuckets => OptionType::Buckets,
                ast::AnalyzeOptionType::NumTopN => OptionType::TopN,
                ast::AnalyzeOptionType::CMSketchDepth => OptionType::CmsketchDepth,
                ast::AnalyzeOptionType::CMSketchWidth => OptionType::CmsketchWidth,
                ast::AnalyzeOptionType::NumSamples => OptionType::NumSamples,
                ast::AnalyzeOptionType::SampleRate => OptionType::SampleRate,
                _ => continue,
            };
            // The existing parser represents DEFAULT as None (#69956). Keep
            // that fallback when adding saved-option reads for this task.
            let Some(value) = option.Value.as_ref() else {
                resets.insert(key);
                explicit.retain(|(previous, _)| *previous != key);
                continue;
            };
            resets.remove(&key);
            let text = literal(value)?;
            let value = if key == OptionType::SampleRate {
                text.parse::<f64>()
                    .map_err(|error| session_error("parse ANALYZE SAMPLERATE", error))?
                    .to_bits()
            } else {
                text.parse::<u64>()
                    .map_err(|error| session_error("parse ANALYZE option", error))?
            };
            handleAnalyzeOptions(&[(key, value)]).map_err(|error| SessionError::new(error.0))?;
            explicit.push((key, value));
        }
        let raw_options =
            handleAnalyzeOptions(&explicit).map_err(|error| SessionError::new(error.0))?;
        let defaults = fillAnalyzeOptions(raw_options.clone());
        let topn = defaults[&OptionType::TopN] as usize;
        let buckets = defaults[&OptionType::Buckets] as usize;
        let dynamic_partition_prune = self.state.borrow().dynamic_partition_prune;
        let injected_snapshot = astersql_testkit_testfailpoint::eval_string(
            "github.com/pingcap/tidb/pkg/executor/injectAnalyzeSnapshot",
        )
        .is_some();
        let injected_base_count = astersql_testkit_testfailpoint::eval_string(
            "github.com/pingcap/tidb/pkg/executor/injectBaseCount",
        )
        .and_then(|value| value.parse::<i64>().ok());
        let injected_base_modify_count = astersql_testkit_testfailpoint::eval_string(
            "github.com/pingcap/tidb/pkg/executor/injectBaseModifyCount",
        )
        .and_then(|value| value.parse::<i64>().ok());
        let analyze_snapshot = self
            .session_vars
            .GetSystemVar(astersql_sessionctx_vardef::TiDBEnableAnalyzeSnapshot)
            .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true"))
            .unwrap_or_else(|| self.domain.stats_session_vars().analyze_snapshot);
        let persist_options = astersql_sessionctx_vardef::PersistAnalyzeOptions.Load()
            && self.state.borrow().analyze_version == 2;
        let mut options_by_physical_id = HashMap::new();
        let mut options_to_save = HashMap::new();
        let mut dynamic_partition_resets = Vec::new();
        let mut inputs = Vec::new();
        let locked = self.domain.stats_context().locked_table_ids();
        let mut skipped = Vec::new();
        // A named index remains targeted for a selected partition under
        // dynamic pruning. Ordinary version-2 index ANALYZE collects the full
        // table and emits the compatibility warning below.
        let index_only = statement.IndexFlag
            && (self.state.borrow().analyze_version < 2
                || (dynamic_partition_prune && !statement.PartitionNames.is_empty()));
        if statement.IndexFlag && !index_only {
            self.set_warning(
                "The version 2 would collect all statistics not only the selected indexes"
                    .to_owned(),
            );
        }
        for table in &statement.TableNames {
            let database = if table.Schema.L.is_empty() {
                current_database.as_str()
            } else {
                table.Schema.L.as_str()
            };
            let info = self
                .resolve_runtime_table(database, &table.Name.L)
                .ok_or_else(|| {
                    SessionError::new(format!("unknown table {database}.{}", table.Name.L))
                })?;
            let key =
                astersql_statistics_handle::StatsTableKey::new(database, &info.Name.L, info.ID);
            if info.IsView() {
                return Err(SessionError::new(format!(
                    "analyze view {} is not supported now",
                    table.Name.O
                )));
            }
            if info.IsSequence() {
                return Err(SessionError::new(format!(
                    "analyze sequence {} is not supported now",
                    table.Name.O
                )));
            }
            // Temporary tables stay out of the statistics cache until ANALYZE
            // writes their `mysql.stats_meta` row, exactly as in Go.
            self.domain
                .register_temporary_stats_table(&info)
                .map_err(|error| session_error("register temporary table statistics", error))?;
            let requested_partitions = statement
                .PartitionNames
                .iter()
                .map(|name| name.L.clone())
                .collect::<BTreeSet<_>>();
            let mut partition_names = requested_partitions.clone();
            if locked.contains(&key.table_id) {
                if let Some(partition) = info.GetPartitionInfo() {
                    skipped.extend(
                        partition
                            .Definitions
                            .iter()
                            .filter(|definition| {
                                requested_partitions.is_empty()
                                    || requested_partitions.contains(&definition.Name.L)
                            })
                            .map(|definition| {
                                format!(
                                    "{database}.{} partition ({})",
                                    table.Name.L, definition.Name.L
                                )
                            }),
                    );
                } else {
                    skipped.push(format!("{database}.{}", table.Name.L));
                }
                continue;
            }
            if let Some(partition) = info.GetPartitionInfo() {
                let selected = partition
                    .Definitions
                    .iter()
                    .filter(|definition| {
                        requested_partitions.is_empty()
                            || requested_partitions.contains(&definition.Name.L)
                    })
                    .collect::<Vec<_>>();
                let unlocked_names = selected
                    .iter()
                    .filter(|definition| !locked.contains(&definition.ID))
                    .map(|definition| definition.Name.L.clone())
                    .collect::<BTreeSet<_>>();
                skipped.extend(
                    selected
                        .iter()
                        .filter(|definition| locked.contains(&definition.ID))
                        .map(|definition| {
                            format!(
                                "{database}.{} partition ({})",
                                table.Name.L, definition.Name.L
                            )
                        }),
                );
                if unlocked_names.is_empty() && !selected.is_empty() {
                    continue;
                }
                if unlocked_names.len() != selected.len() || !requested_partitions.is_empty() {
                    partition_names = unlocked_names;
                }
            }
            use astersql_planner_core::planbuilder::mergeAnalyzeOptions;
            let table_saved = if persist_options {
                metadata.saved_analyze_options(info.ID)?
            } else {
                HashMap::new()
            };
            let is_analyze_table = requested_partitions.is_empty();
            let stmt_options = if persist_options && dynamic_partition_prune && !is_analyze_table {
                HashMap::new()
            } else {
                raw_options.clone()
            };
            let mut table_raw = if is_analyze_table {
                mergeAnalyzeOptions(stmt_options.clone(), &table_saved)
            } else {
                table_saved.clone()
            };
            if is_analyze_table {
                for key in &resets {
                    table_raw.remove(key);
                }
            }
            options_by_physical_id.insert(
                info.ID,
                fillAnalyzeOptions(if persist_options {
                    table_raw.clone()
                } else {
                    raw_options.clone()
                }),
            );
            if persist_options {
                options_to_save.insert(info.ID, table_raw.clone());
            }
            if let Some(partition) = info.GetPartitionInfo() {
                if persist_options
                    && dynamic_partition_prune
                    && is_analyze_table
                    && !resets.is_empty()
                {
                    dynamic_partition_resets.push((
                        partition
                            .Definitions
                            .iter()
                            .map(|definition| definition.ID)
                            .collect::<Vec<_>>(),
                        resets.clone(),
                    ));
                }
                for definition in &partition.Definitions {
                    if !partition_names.is_empty() && !partition_names.contains(&definition.Name.L)
                    {
                        continue;
                    }
                    let raw = if !persist_options {
                        raw_options.clone()
                    } else if dynamic_partition_prune {
                        table_raw.clone()
                    } else {
                        let mut partition_saved = metadata.saved_analyze_options(definition.ID)?;
                        if !is_analyze_table {
                            for key in &resets {
                                partition_saved.remove(key);
                            }
                        }
                        let mut saved = mergeAnalyzeOptions(partition_saved, &table_saved);
                        if is_analyze_table {
                            for key in &resets {
                                saved.remove(key);
                            }
                        }
                        mergeAnalyzeOptions(stmt_options.clone(), &saved)
                    };
                    options_by_physical_id.insert(definition.ID, fillAnalyzeOptions(raw.clone()));
                    if persist_options && !dynamic_partition_prune {
                        options_to_save.insert(definition.ID, raw);
                    }
                }
            }
            let (analyzed_columns, missing_columns, invalid_columns) =
                self.analyze_columns_info(statement, &key, &info);
            if !invalid_columns.is_empty() {
                return Err(SessionError::new(format!(
                    "unknown column {} in ANALYZE TABLE",
                    invalid_columns.join(", ")
                )));
            }
            if !missing_columns.is_empty() {
                self.set_warning(format!(
                    "Columns {} are missing in ANALYZE but their stats are needed for calculating \
                     stats for indexes/primary key/extended stats",
                    missing_columns.join(",")
                ));
            }
            // Go warns that a dynamic-pruning partition ANALYZE cannot honor
            // its column/index selection or explicit options. Keep the warning
            // even where the simplified runtime retains compatible options.
            if dynamic_partition_prune
                && !requested_partitions.is_empty()
                && (!statement.AnalyzeOpts.is_empty() || statement.IndexFlag)
            {
                self.set_warning(
                    "Ignore columns and options when analyze partition in dynamic mode".to_owned(),
                );
            }
            if let Some(partition) = info.GetPartitionInfo() {
                for definition in partition.Definitions.iter().filter(|definition| {
                    requested_partitions.is_empty()
                        || requested_partitions.contains(&definition.Name.L)
                }) {
                    self.set_note(format!(
                        "Analyze use auto adjusted sample rate 1.000000 for table \
                         {database}.{}'s partition {}, reason to use this rate is \
                         \"use min(1, 110000/10000) as the sample-rate=1\"",
                        table.Name.L, definition.Name.L
                    ));
                }
            } else {
                let sample_base = if statement.IndexFlag && !index_only {
                    1
                } else {
                    10_000
                };
                self.set_note(format!(
                    "Analyze use auto adjusted sample rate 1.000000 for table \
                     {database}.{}, reason to use this rate is \
                     \"use min(1, 110000/{sample_base}) as the sample-rate=1\"",
                    table.Name.L,
                ));
            }
            for index in info.Indices.iter().filter(|index| index.IsColumnarIndex()) {
                self.set_warning(format!(
                    "analyzing columnar index is not supported, skip {}",
                    index.Name.L
                ));
            }
            let analyzed_index_names =
                analyze_indexes_info(&info, self.session_vars.EnableDDLAnalyzeExecOpt);
            let mut rows = self.read_dml_rows_in_database(database, &table.Name.L)?;
            if analyze_snapshot
                && injected_snapshot
                && let Some(base_count) = injected_base_count
            {
                rows.truncate(usize::try_from(base_count.max(0)).unwrap_or(usize::MAX));
            }
            inputs.push(SessionAnalyzeInput {
                database: database.to_owned(),
                key,
                rows,
                partition_names: partition_names.into_iter().collect(),
                analyzed_indexes: index_only.then(|| {
                    statement
                        .IndexNames
                        .iter()
                        .map(|name| name.L.clone())
                        .collect()
                }),
                analyzed_columns,
                analyzed_index_names,
                info,
            });
        }
        if !skipped.is_empty() {
            skipped.sort();
            self.set_warning(format!(
                "skip analyze locked {}: {}",
                if skipped.len() == 1 {
                    "table"
                } else {
                    "tables"
                },
                skipped.join(", ")
            ));
        }
        if inputs.is_empty() {
            return Ok(());
        }
        if dynamic_partition_prune && statement.IndexFlag && !statement.PartitionNames.is_empty() {
            // Preserve the existing merged index-partition histogram shape.
            for opts in options_by_physical_id.values_mut() {
                opts.insert(OptionType::Buckets, 1);
            }
        }
        let runtime = SessionAnalyzeRuntime {
            domain: Arc::clone(&self.domain),
            killer: Arc::clone(&self.sql_killer),
            inputs,
            topn,
            // The dynamic global merge for a partition-index refresh is a
            // single merged histogram.  Building more local buckets leaks the
            // pre-merge partition shape into `SHOW STATS_BUCKETS`.
            buckets: if dynamic_partition_prune
                && statement.IndexFlag
                && !statement.PartitionNames.is_empty()
            {
                1
            } else {
                buckets
            },
            options_by_physical_id,
            dynamic_partition_prune,
            stats_time_zone: self.session_vars.StmtCtx.TimeZone().name().to_owned(),
            start_time: format_system_time(SystemTime::now()),
            concurrency: self.state.borrow().analyze_concurrency,
            active_workers: AtomicUsize::new(0),
            max_active_workers: AtomicUsize::new(1),
            restricted: self.state.borrow().in_restricted_sql,
            analyze_memory_quota: self.state.borrow().analyze_memory_quota,
            injected_base_count,
            injected_base_modify_count,
            analyze_snapshot,
        };
        astersql_executor::analyze::AnalyzeExec::RunCanonicalWithContext(context, &runtime)
            .map_err(|error| session_error("execute ANALYZE pipeline", error))?;
        // Persist raw values only after successful analysis. Absent keys retain
        // SQL sentinels, so changing a global default affects future plans.
        for (id, opts) in options_to_save {
            use astersql_planner_core::planbuilder::AnalyzeOptionType as O;
            let buckets = opts.get(&O::Buckets).copied().unwrap_or(0);
            let topn = opts.get(&O::TopN).map_or(-1, |value| *value as i64);
            let samples = opts.get(&O::NumSamples).copied().unwrap_or(0);
            let rate = opts
                .get(&O::SampleRate)
                .copied()
                .map(f64::from_bits)
                .unwrap_or(-1.0);
            metadata.execute(&format!("INSERT INTO mysql.analyze_options (table_id,sample_num,sample_rate,buckets,topn) VALUES ({id},{samples},{rate},{buckets},{topn}) ON DUPLICATE KEY UPDATE sample_num={samples},sample_rate={rate},buckets={buckets},topn={topn}"))?;
        }
        for (partition_ids, resets) in dynamic_partition_resets {
            use astersql_planner_core::planbuilder::AnalyzeOptionType as O;
            let assignments = [
                (O::NumSamples, "sample_num=0"),
                (O::SampleRate, "sample_rate=-1"),
                (O::Buckets, "buckets=0"),
                (O::TopN, "topn=-1"),
            ]
            .into_iter()
            .filter(|(option, _)| resets.contains(option))
            .map(|(_, assignment)| assignment)
            .collect::<Vec<_>>()
            .join(",");
            let ids = partition_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            if !assignments.is_empty() && !ids.is_empty() {
                metadata.execute(&format!(
                    "UPDATE mysql.analyze_options SET {assignments} WHERE table_id IN ({ids})"
                ))?;
            }
        }
        Ok(())
    }

    /// SHOW STATS_META 结果。
    fn show_stats_meta(&self) -> ConcreteRecordSet {
        ConcreteRecordSet::new(
            vec![
                "Db_name".to_owned(),
                "Table_name".to_owned(),
                "Partition_name".to_owned(),
                "Modify_count".to_owned(),
                "Row_count".to_owned(),
            ],
            self.domain
                .stats_meta_rows()
                .into_iter()
                .map(|stats| {
                    vec![
                        stats.database,
                        stats.table,
                        String::new(),
                        stats.modify_count.to_string(),
                        stats.row_count.to_string(),
                    ]
                })
                .collect(),
        )
    }

    /// 从表达式收集等值条件映射。
    fn show_equalities(expression: Option<&ast::ExprNode>, output: &mut HashMap<String, String>) {
        let Some(expression) = expression else {
            return;
        };
        if let ast::ExprKind::Binary { Op, L, R } = &expression.Kind {
            if Op.eq_ignore_ascii_case("and") {
                Self::show_equalities(Some(L), output);
                Self::show_equalities(Some(R), output);
            } else if Op == "=" {
                match (&L.Kind, &R.Kind) {
                    (ast::ExprKind::Column(column), _) => {
                        if let Ok(value) = literal(R) {
                            output.insert(column.Name.L.clone(), value);
                        }
                    }
                    (_, ast::ExprKind::Column(column)) => {
                        if let Ok(value) = literal(L) {
                            output.insert(column.Name.L.clone(), value);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// 将 SHOW STATS 单元格格式化为字符串。
    fn show_cell(cell: astersql_executor::show_stats::Cell) -> String {
        match cell {
            astersql_executor::show_stats::Cell::Null => SHOW_NULL_CELL.to_owned(),
            astersql_executor::show_stats::Cell::Text(value) => value,
            astersql_executor::show_stats::Cell::Signed(value) => value.to_string(),
            astersql_executor::show_stats::Cell::Unsigned(value) => value.to_string(),
            astersql_executor::show_stats::Cell::Float(value) => value.to_string(),
            astersql_executor::show_stats::Cell::Timestamp(value) => format_system_time(
                UNIX_EPOCH + std::time::Duration::from_millis(value.unix_millis.max(0) as u64),
            ),
        }
    }

    /// 求值 SHOW 过滤表达式。
    fn show_expr_value(
        expression: &ast::ExprNode,
        columns: &[&str],
        row: &[String],
    ) -> SessionResult<String> {
        if let ast::ExprKind::Column(column) = &expression.Kind {
            let index = columns
                .iter()
                .position(|name| name.eq_ignore_ascii_case(&column.Name.L))
                .ok_or_else(|| {
                    SessionError::new(format!("unknown SHOW column {}", column.Name.O))
                })?;
            return Ok(row.get(index).cloned().unwrap_or_default());
        }
        literal(expression)
    }

    /// LIKE 模式匹配（SHOW 过滤）。
    fn show_like(pattern: &str, value: &str) -> bool {
        // Go evaluates SHOW predicates through the SQL expression engine,
        // where `_` consumes one character rather than one UTF-8 byte.
        let pattern = pattern.to_lowercase().chars().collect::<Vec<_>>();
        let value = value.to_lowercase().chars().collect::<Vec<_>>();
        let mut current = vec![false; value.len() + 1];
        current[0] = true;
        for token in pattern {
            let mut next = vec![false; value.len() + 1];
            match token {
                '%' => {
                    next[0] = current[0];
                    for index in 1..=value.len() {
                        next[index] = current[index] || next[index - 1];
                    }
                }
                '_' => {
                    for index in 1..=value.len() {
                        next[index] = current[index - 1];
                    }
                }
                literal => {
                    for index in 1..=value.len() {
                        next[index] = current[index - 1] && value[index - 1] == literal;
                    }
                }
            }
            current = next;
        }
        current[value.len()]
    }

    /// 对 SHOW 行应用 WHERE 谓词。
    pub(super) fn show_predicate(
        expression: &ast::ExprNode,
        columns: &[&str],
        row: &[String],
    ) -> SessionResult<bool> {
        match &expression.Kind {
            ast::ExprKind::Binary { Op, L, R } if Op.eq_ignore_ascii_case("and") => Ok(
                Self::show_predicate(L, columns, row)? && Self::show_predicate(R, columns, row)?,
            ),
            ast::ExprKind::Binary { Op, L, R } if Op.eq_ignore_ascii_case("or") => Ok(
                Self::show_predicate(L, columns, row)? || Self::show_predicate(R, columns, row)?,
            ),
            ast::ExprKind::Binary { Op, L, R } if Op == "=" || Op == "==" => {
                Ok(Self::show_expr_value(L, columns, row)?
                    .eq_ignore_ascii_case(&Self::show_expr_value(R, columns, row)?))
            }
            ast::ExprKind::InList {
                Expr, List, Not, ..
            } => {
                let value = Self::show_expr_value(Expr, columns, row)?;
                let matched = List.iter().any(|candidate| {
                    Self::show_expr_value(candidate, columns, row)
                        .is_ok_and(|candidate| value.eq_ignore_ascii_case(&candidate))
                });
                Ok(if *Not { !matched } else { matched })
            }
            ast::ExprKind::Like {
                Expr, Pattern, Not, ..
            } => {
                let matched = Self::show_like(
                    &Self::show_expr_value(Pattern, columns, row)?,
                    &Self::show_expr_value(Expr, columns, row)?,
                );
                Ok(if *Not { !matched } else { matched })
            }
            ast::ExprKind::IsNull { Expr, Not } => {
                let is_null = Self::show_expr_value(Expr, columns, row)? == SHOW_NULL_CELL;
                Ok(if *Not { !is_null } else { is_null })
            }
            ast::ExprKind::Value(_) => Ok(literal(expression)?.parse::<i64>().unwrap_or(0) != 0),
            _ => Err(SessionError::new("unsupported SHOW predicate")),
        }
    }

    /// 执行各类 SHOW STATS_* 语句。
    pub(super) fn execute_show_stats(
        &self,
        statement: &ast::ShowStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        use astersql_executor::show_stats::{ShowExec, ShowFilters};

        let filters = ShowFilters::default();
        let dynamic_partition_prune = self.state.borrow().dynamic_partition_prune;
        let runtime = SessionShowStatsRuntime {
            context: self.domain.stats_context(),
            dynamic_partition_prune,
        };
        let mut executor = ShowExec::new(runtime, filters);
        let columns = match statement.Tp {
            ast::ShowStmtType::StatsMeta => {
                executor
                    .fetchShowStatsMeta()
                    .map_err(|error| session_error("SHOW STATS_META", error))?;
                vec![
                    "Db_name",
                    "Table_name",
                    "Partition_name",
                    "Update_time",
                    "Modify_count",
                    "Row_count",
                    "Last_analyze_time",
                ]
            }
            ast::ShowStmtType::StatsHistograms => {
                executor
                    .fetchShowStatsHistogram()
                    .map_err(|error| session_error("SHOW STATS_HISTOGRAMS", error))?;
                vec![
                    "Db_name",
                    "Table_name",
                    "Partition_name",
                    "Column_name",
                    "Is_index",
                    "Update_time",
                    "Distinct_count",
                    "Null_count",
                    "Avg_col_size",
                    "Correlation",
                    "Load_status",
                    "Total_mem_size",
                    "Hist_mem_size",
                    "Topn_mem_size",
                    "Cms_mem_size",
                ]
            }
            ast::ShowStmtType::StatsTopN => {
                executor
                    .fetchShowStatsTopN()
                    .map_err(|error| session_error("SHOW STATS_TOPN", error))?;
                vec![
                    "Db_name",
                    "Table_name",
                    "Partition_name",
                    "Column_name",
                    "Is_index",
                    "Value",
                    "Count",
                ]
            }
            ast::ShowStmtType::StatsBuckets => {
                executor
                    .fetchShowStatsBuckets()
                    .map_err(|error| session_error("SHOW STATS_BUCKETS", error))?;
                vec![
                    "Db_name",
                    "Table_name",
                    "Partition_name",
                    "Column_name",
                    "Is_index",
                    "Bucket_id",
                    "Count",
                    "Repeats",
                    "Lower_bound",
                    "Upper_bound",
                    "Ndv",
                ]
            }
            ast::ShowStmtType::StatsHealthy => {
                executor.fetchShowStatsHealthy();
                vec!["Db_name", "Table_name", "Partition_name", "Healthy"]
            }
            ast::ShowStmtType::StatsLocked => {
                executor
                    .fetchShowStatsLocked()
                    .map_err(|error| session_error("SHOW STATS_LOCKED", error))?;
                vec!["Db_name", "Table_name", "Partition_name", "Status"]
            }
            ast::ShowStmtType::HistogramsInFlight => {
                executor.fetchShowHistogramsInFlight();
                vec!["Histograms_in_flight"]
            }
            ast::ShowStmtType::AnalyzeStatus => {
                executor
                    .fetchShowAnalyzeStatus()
                    .map_err(|error| session_error("SHOW ANALYZE STATUS", error))?;
                vec![
                    "Table_schema",
                    "Table_name",
                    "Partition_name",
                    "Job_info",
                    "Processed_rows",
                    "Start_time",
                    "End_time",
                    "State",
                    "Fail_reason",
                    "Instance",
                    "Process_id",
                    "Remaining_duration",
                ]
            }
            ast::ShowStmtType::ColumnStatsUsage => {
                executor
                    .fetchShowColumnStatsUsage()
                    .map_err(|error| session_error("SHOW COLUMN_STATS_USAGE", error))?;
                vec![
                    "Db_name",
                    "Table_name",
                    "Partition_name",
                    "Column_name",
                    "Last_used_at",
                    "Last_analyzed_at",
                ]
            }
            _ => return Err(SessionError::new("unsupported SHOW STATS statement")),
        };
        let rows = executor
            .rows
            .into_iter()
            .map(|row| row.into_iter().map(Self::show_cell).collect::<Vec<_>>())
            .filter(|row| {
                statement.Pattern.as_ref().is_none_or(|pattern| {
                    literal(pattern).is_ok_and(|pattern| {
                        row.first()
                            .is_some_and(|database| Self::show_like(&pattern, database))
                    })
                })
            })
            .filter(|row| {
                statement.Where.as_ref().is_none_or(|predicate| {
                    Self::show_predicate(predicate, &columns, row).unwrap_or(false)
                })
            })
            .collect();
        Ok(ConcreteRecordSet::new(
            columns.into_iter().map(str::to_owned).collect(),
            rows,
        ))
    }

    /// SHOW COLUMN_STATS_USAGE。
    /// execute_show_column_usage：ConcreteSession 内部执行辅助。
    pub(super) fn execute_show_column_usage(
        &self,
        statement: &ast::ShowStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        let context = self.domain.stats_context();
        let catalog = context.catalog();
        let mut equalities = HashMap::new();
        Self::show_equalities(statement.Where.as_ref(), &mut equalities);
        let mut rows = Vec::new();
        for usage in context.column_usage() {
            let Some(((database, table_name), (key, table))) =
                catalog.iter().find(|(_, (key, info))| {
                    key.table_id == usage.table_id
                        || info.GetPartitionInfo().is_some_and(|partition| {
                            partition
                                .Definitions
                                .iter()
                                .any(|definition| definition.ID == usage.table_id)
                        })
                })
            else {
                continue;
            };
            if equalities
                .get("db_name")
                .is_some_and(|value| value != database)
                || equalities
                    .get("table_name")
                    .is_some_and(|value| !value.eq_ignore_ascii_case(table_name))
            {
                continue;
            }
            let Some(column) = table
                .Columns
                .iter()
                .find(|column| column.ID == usage.column_id)
            else {
                continue;
            };
            let partition_name = if key.table_id == usage.table_id {
                if table.GetPartitionInfo().is_some() {
                    "global".to_owned()
                } else {
                    String::new()
                }
            } else {
                table
                    .GetPartitionInfo()
                    .and_then(|partition| {
                        partition
                            .Definitions
                            .iter()
                            .find(|definition| definition.ID == usage.table_id)
                    })
                    .map(|definition| definition.Name.L.clone())
                    .unwrap_or_default()
            };
            rows.push(vec![
                database.clone(),
                table_name.clone(),
                partition_name,
                column.Name.L.clone(),
                usage.last_used_at.unwrap_or_else(|| "<nil>".to_owned()),
                usage.last_analyzed_at.unwrap_or_else(|| "<nil>".to_owned()),
            ]);
        }
        rows.sort();
        Ok(ConcreteRecordSet::new(
            vec![
                "Db_name",
                "Table_name",
                "Partition_name",
                "Column_name",
                "Last_used_at",
                "Last_analyzed_at",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rows,
        ))
    }

    fn show_table_info(
        &self,
        statement: &ast::ShowStmt,
    ) -> SessionResult<(String, String, astersql_meta_model::TableInfo)> {
        let table = statement
            .Table
            .as_ref()
            .ok_or_else(|| SessionError::new("SHOW metadata requires a table"))?;
        let database = if table.Schema.L.is_empty() {
            self.current_database()
        } else {
            table.Schema.L.clone()
        };
        let table_name = table.Name.L.clone();
        let table = self
            .resolve_runtime_table(&database, &table_name)
            .ok_or_else(|| SessionError::new(format!("unknown table {database}.{table_name}")))?;
        Ok((database, table_name, table))
    }

    pub(super) fn execute_show_columns(
        &self,
        statement: &ast::ShowStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        let (_, _, table) = self.show_table_info(statement)?;
        let rows = table
            .Cols()
            .into_iter()
            .flatten()
            .map(|column| {
                let not_null = astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag());
                let auto_increment =
                    astersql_parser_mysql::r#type::HasAutoIncrementFlag(column.GetFlag());
                let generated = column.IsGenerated();
                let mut row = vec![column.Name.O.clone(), column.GetTypeDesc()];
                if statement.Full {
                    row.push(
                        if column.GetCollate().is_empty()
                            || column.GetCharset().eq_ignore_ascii_case("binary")
                        {
                            String::new()
                        } else {
                            column.GetCollate().to_owned()
                        },
                    );
                }
                row.extend([
                    if not_null { "NO" } else { "YES" }.to_owned(),
                    metadata_column_key(&table, column).to_owned(),
                    metadata_default_value(column).unwrap_or_else(|| "<nil>".to_owned()),
                    if auto_increment {
                        "auto_increment".to_owned()
                    } else if generated {
                        if column.GeneratedStored {
                            "STORED GENERATED".to_owned()
                        } else {
                            "VIRTUAL GENERATED".to_owned()
                        }
                    } else {
                        String::new()
                    },
                ]);
                if statement.Full {
                    row.extend([
                        "select,insert,update,references".to_owned(),
                        column.Comment.clone(),
                    ]);
                }
                row
            })
            .collect();
        let columns = if statement.Full {
            vec![
                "Field",
                "Type",
                "Collation",
                "Null",
                "Key",
                "Default",
                "Extra",
                "Privileges",
                "Comment",
            ]
        } else {
            vec!["Field", "Type", "Null", "Key", "Default", "Extra"]
        };
        Ok(ConcreteRecordSet::new(
            columns.into_iter().map(str::to_owned).collect(),
            rows,
        ))
    }

    pub(super) fn execute_show_create_table(
        &self,
        statement: &ast::ShowStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        let (_, table_name, table) = self.show_table_info(statement)?;
        let ansi_quotes = astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
            .map_err(|error| session_error("parse sql_mode", error))?
            .HasANSIQuotesMode();
        let mut definitions = Vec::new();
        for column in table.Columns.iter().filter(|column| !column.Hidden) {
            let mut definition = format!(
                "  {} {}",
                quote_show_identifier(&column.Name.O, ansi_quotes),
                column.GetTypeDesc()
            );
            if astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag()) {
                definition.push_str(" NOT NULL");
            }
            if column.DefaultIsExpr {
                if let Some(astersql_meta_model::DefaultValue::String(value)) =
                    column.GetDefaultValue()
                {
                    definition.push_str(" DEFAULT (");
                    definition.push_str(&String::from_utf8_lossy(&value));
                    definition.push(')');
                }
            } else if let Some(value) = metadata_default_value(column) {
                definition.push_str(" DEFAULT '");
                definition.push_str(&value.replace('\'', "''"));
                definition.push('\'');
            } else if !astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag()) {
                definition.push_str(" DEFAULT NULL");
            }
            if astersql_parser_mysql::r#type::HasAutoIncrementFlag(column.GetFlag()) {
                definition.push_str(" AUTO_INCREMENT");
            }
            definitions.push(definition);
        }
        if !table
            .Indices
            .iter()
            .any(|index| index.Primary || index.Name.L == "primary")
        {
            let primary_columns = table
                .Columns
                .iter()
                .filter(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
                .map(|column| quote_show_identifier(&column.Name.O, ansi_quotes))
                .collect::<Vec<_>>();
            if !primary_columns.is_empty() {
                definitions.push(format!(
                    "  PRIMARY KEY ({}) /*T![clustered_index] {} */",
                    primary_columns.join(","),
                    if table.HasClusteredIndex() {
                        "CLUSTERED"
                    } else {
                        "NONCLUSTERED"
                    }
                ));
            }
        }
        for index in &table.Indices {
            let columns = index
                .Columns
                .iter()
                .map(|column| quote_show_identifier(&column.Name.O, ansi_quotes))
                .collect::<Vec<_>>()
                .join(",");
            let definition = if index.Primary || index.Name.L == "primary" {
                format!(
                    "  PRIMARY KEY ({columns}) /*T![clustered_index] {} */",
                    if table.HasClusteredIndex() {
                        "CLUSTERED"
                    } else {
                        "NONCLUSTERED"
                    }
                )
            } else if index.VectorInfo.is_some() {
                format!(
                    "  VECTOR INDEX {} ({columns})",
                    quote_show_identifier(&index.Name.O, ansi_quotes)
                )
            } else if index.FullTextInfo.is_some() {
                format!(
                    "  FULLTEXT INDEX {}({columns}) WITH PARSER STANDARD",
                    quote_show_identifier(&index.Name.O, ansi_quotes)
                )
            } else if index.Unique {
                format!(
                    "  UNIQUE KEY {} ({columns})",
                    quote_show_identifier(&index.Name.O, ansi_quotes)
                )
            } else {
                format!(
                    "  KEY {} ({columns})",
                    quote_show_identifier(&index.Name.O, ansi_quotes)
                )
            };
            definitions.push(definition);
        }
        let charset = if table.Charset.is_empty() {
            "utf8mb4"
        } else {
            table.Charset.as_str()
        };
        let collate = if table.Collate.is_empty() {
            "utf8mb4_bin"
        } else {
            table.Collate.as_str()
        };
        let create_kind = if table.TempTableType == astersql_meta_model::TempTableGlobal {
            "CREATE GLOBAL TEMPORARY TABLE"
        } else {
            "CREATE TABLE"
        };
        let storage_option = if table.EngineAttribute.is_empty() {
            String::new()
        } else {
            match astersql_ddl::storage_class::GetSimpleTableStorageClassForShowCreate(&table)
                .map_err(SessionError::new)?
            {
                Some(tier) => format!(" STORAGE_CLASS='{tier}'"),
                None => format!(
                    " ENGINE_ATTRIBUTE='{}'",
                    table
                        .EngineAttribute
                        .replace('\0', "\\0")
                        .replace('\'', "''")
                        .replace('\n', "\\n")
                        .replace('\r', "\\r")
                ),
            }
        };
        let mut create = format!(
            "{create_kind} {} (\n{}\n) ENGINE=InnoDB{} DEFAULT CHARSET={} COLLATE={}",
            quote_show_identifier(&table.Name.O, ansi_quotes),
            definitions.join(",\n"),
            storage_option,
            charset,
            collate
        );
        if table.GetAutoIncrementColInfo().is_some() {
            let base = self
                .domain
                .stats_auto_id_base(table.ID, 0)
                .unwrap_or_else(|| u64::try_from(table.AutoIncID).unwrap_or_default());
            if base > 0 {
                create.push_str(&format!(" AUTO_INCREMENT={base}"));
            }
        }
        if table.AutoRandID > 0 {
            create.push_str(&format!(
                " /*T![auto_rand_base] AUTO_RANDOM_BASE={} */",
                table.AutoRandID
            ));
        }
        if table.ShardRowIDBits != 0 {
            create.push_str(&format!(" /*T! SHARD_ROW_ID_BITS={}", table.ShardRowIDBits));
            if table.PreSplitRegions != 0 {
                create.push_str(&format!(" PRE_SPLIT_REGIONS={}", table.PreSplitRegions));
            }
            create.push_str(" */");
        }
        if table.TempTableType == astersql_meta_model::TempTableGlobal {
            create.push_str(" ON COMMIT DELETE ROWS");
        }
        if let Some(ttl) = table.TTLInfo.as_ref() {
            let interval_unit = match ttl.IntervalTimeUnit {
                value if value == ast::TimeUnitType::Microsecond as i32 => "MICROSECOND",
                value if value == ast::TimeUnitType::Second as i32 => "SECOND",
                value if value == ast::TimeUnitType::Minute as i32 => "MINUTE",
                value if value == ast::TimeUnitType::Hour as i32 => "HOUR",
                value if value == ast::TimeUnitType::Day as i32 => "DAY",
                value if value == ast::TimeUnitType::Week as i32 => "WEEK",
                value if value == ast::TimeUnitType::Month as i32 => "MONTH",
                value if value == ast::TimeUnitType::Quarter as i32 => "QUARTER",
                value if value == ast::TimeUnitType::Year as i32 => "YEAR",
                value if value == ast::TimeUnitType::SecondMicrosecond as i32 => {
                    "SECOND_MICROSECOND"
                }
                value if value == ast::TimeUnitType::MinuteMicrosecond as i32 => {
                    "MINUTE_MICROSECOND"
                }
                value if value == ast::TimeUnitType::MinuteSecond as i32 => "MINUTE_SECOND",
                value if value == ast::TimeUnitType::HourMicrosecond as i32 => "HOUR_MICROSECOND",
                value if value == ast::TimeUnitType::HourSecond as i32 => "HOUR_SECOND",
                value if value == ast::TimeUnitType::HourMinute as i32 => "HOUR_MINUTE",
                value if value == ast::TimeUnitType::DayMicrosecond as i32 => "DAY_MICROSECOND",
                value if value == ast::TimeUnitType::DaySecond as i32 => "DAY_SECOND",
                value if value == ast::TimeUnitType::DayMinute as i32 => "DAY_MINUTE",
                value if value == ast::TimeUnitType::DayHour as i32 => "DAY_HOUR",
                value if value == ast::TimeUnitType::YearMonth as i32 => "YEAR_MONTH",
                _ => "",
            };
            create.push_str(&format!(
                " /*T![ttl] TTL={} + INTERVAL {} {} */ /*T![ttl] TTL_ENABLE='{}' */ /*T![ttl] TTL_JOB_INTERVAL='{}' */",
                quote_show_identifier(&ttl.ColumnName.O, ansi_quotes),
                ttl.IntervalExprStr,
                interval_unit,
                if ttl.Enable { "ON" } else { "OFF" },
                ttl.JobInterval
            ));
        }
        if let Some(policy) = table.PlacementPolicyRef.as_ref() {
            create.push_str(&format!(
                " /*T![placement] PLACEMENT POLICY={} */",
                quote_show_identifier(&policy.Name.O, ansi_quotes)
            ));
        }
        if let Some(partition) = table.Partition.as_ref() {
            let partition_type = match partition.Type {
                astersql_meta_model::ast::model::PartitionTypeRange => "RANGE",
                astersql_meta_model::ast::model::PartitionTypeList => "LIST",
                astersql_meta_model::ast::model::PartitionTypeHash => "HASH",
                _ => "",
            };
            if !partition_type.is_empty() {
                create.push_str("\nPARTITION BY ");
                create.push_str(partition_type);
                if partition.Columns.is_empty() {
                    create.push_str(" (");
                    create.push_str(&partition.Expr);
                    create.push(')');
                } else {
                    create.push_str(" COLUMNS(");
                    create.push_str(
                        &partition
                            .Columns
                            .iter()
                            .map(|column| quote_show_identifier(&column.O, ansi_quotes))
                            .collect::<Vec<_>>()
                            .join(","),
                    );
                    create.push(')');
                }
                if !partition.Definitions.is_empty() {
                    create.push_str("\n(");
                    for (index, definition) in partition.Definitions.iter().enumerate() {
                        if index != 0 {
                            create.push_str(",\n ");
                        }
                        create.push_str("PARTITION ");
                        create.push_str(&quote_show_identifier(&definition.Name.O, ansi_quotes));
                        if partition.Type == astersql_meta_model::ast::model::PartitionTypeRange {
                            create.push_str(" VALUES LESS THAN (");
                            create.push_str(&definition.LessThan.join(","));
                            create.push(')');
                        } else if partition.Type
                            == astersql_meta_model::ast::model::PartitionTypeList
                        {
                            create.push_str(" VALUES IN (");
                            create.push_str(
                                &definition
                                    .InValues
                                    .iter()
                                    .map(|values| {
                                        if values.len() == 1 {
                                            values[0].clone()
                                        } else {
                                            format!("({})", values.join(","))
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join(","),
                            );
                            create.push(')');
                        }
                        if !definition.Comment.is_empty() {
                            create.push_str(" COMMENT '");
                            create.push_str(&definition.Comment.replace('\'', "''"));
                            create.push('\'');
                        }
                        if let Some(policy) = definition.PlacementPolicyRef.as_ref() {
                            create.push_str(&format!(
                                " /*T![placement] PLACEMENT POLICY={} */",
                                quote_show_identifier(&policy.Name.O, ansi_quotes)
                            ));
                        }
                    }
                    create.push(')');
                } else if partition.Num != 0 {
                    create.push_str(&format!(" PARTITIONS {}", partition.Num));
                }
            }
        }
        Ok(ConcreteRecordSet::new(
            vec!["Table".to_owned(), "Create Table".to_owned()],
            vec![vec![table_name, create]],
        ))
    }

    pub(super) fn execute_show_index(
        &self,
        statement: &ast::ShowStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        let (_, table_name, table) = self.show_table_info(statement)?;
        let mut rows = Vec::new();
        let primary_columns = table
            .Columns
            .iter()
            .filter(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
            .collect::<Vec<_>>();
        for (position, column) in primary_columns.into_iter().enumerate() {
            rows.push(vec![
                table_name.clone(),
                "0".to_owned(),
                "PRIMARY".to_owned(),
                (position + 1).to_string(),
                column.Name.O.clone(),
                "A".to_owned(),
                "0".to_owned(),
                String::new(),
                String::new(),
                if astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag()) {
                    String::new()
                } else {
                    "YES".to_owned()
                },
                "BTREE".to_owned(),
                String::new(),
                String::new(),
                "YES".to_owned(),
                String::new(),
            ]);
        }
        for index in &table.Indices {
            if index.Name.L == "primary" {
                continue;
            }
            for (position, indexed) in index.Columns.iter().enumerate() {
                let column = table.Columns.get(indexed.Offset.max(0) as usize);
                rows.push(vec![
                    table_name.clone(),
                    if index.Unique { "0" } else { "1" }.to_owned(),
                    index.Name.O.clone(),
                    (position + 1).to_string(),
                    indexed.Name.O.clone(),
                    "A".to_owned(),
                    "0".to_owned(),
                    (indexed.Length >= 0)
                        .then(|| indexed.Length.to_string())
                        .unwrap_or_default(),
                    String::new(),
                    column
                        .map_or("", |column| {
                            if astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag()) {
                                ""
                            } else {
                                "YES"
                            }
                        })
                        .to_owned(),
                    "BTREE".to_owned(),
                    String::new(),
                    index.Comment.clone(),
                    if index.Invisible { "NO" } else { "YES" }.to_owned(),
                    String::new(),
                ]);
            }
        }
        rows.sort_by(|left, right| left[2].cmp(&right[2]).then_with(|| left[3].cmp(&right[3])));
        Ok(ConcreteRecordSet::new(
            [
                "Table",
                "Non_unique",
                "Key_name",
                "Seq_in_index",
                "Column_name",
                "Collation",
                "Cardinality",
                "Sub_part",
                "Packed",
                "Null",
                "Index_type",
                "Comment",
                "Index_comment",
                "Visible",
                "Expression",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rows,
        ))
    }

    /// SHOW TABLE STATUS。
    /// execute_show_table_status：ConcreteSession 内部执行辅助。
    pub(super) fn execute_show_table_status(&self, statement: &ast::ShowStmt) -> ConcreteRecordSet {
        let current_database = if statement.DBName.is_empty() {
            self.current_database()
        } else {
            statement.DBName.to_ascii_lowercase()
        };
        let pattern = statement
            .Pattern
            .as_ref()
            .and_then(|expression| literal(expression).ok())
            .map(|value| value.trim_matches(['\'', '"']).to_owned());
        let time_zone = *self.time_zone.borrow();
        let context = self.domain.stats_context();
        let catalog = self
            .state
            .borrow()
            .snapshot_catalog_version
            .map(|version| context.catalog_at(version))
            .unwrap_or_else(|| context.catalog());
        let mut rows = catalog
            .into_iter()
            .filter(|((database, _), (_, table))| {
                let visible = match (
                    self.login_user.as_deref(),
                    self.authenticated_host.as_deref(),
                ) {
                    (Some(user), Some(host)) => runtime_privilege_handle(&self.domain)
                        .Get()
                        .RequestVerification(
                            &self.active_roles.borrow(),
                            user,
                            host,
                            database,
                            &table.Name.L,
                            "",
                            astersql_privilege_privileges::SelectPriv,
                        ),
                    _ => true,
                };
                database == &current_database
                    && visible
                    && pattern
                        .as_deref()
                        .is_none_or(|pattern| Self::show_like(pattern, &table.Name.O))
            })
            .map(|((_, _), (_, table))| {
                let table_name = table.Name.O.clone();
                let row_count = context
                    .table(&current_database, &table_name)
                    .map_or(0, |stats| stats.row_count.max(0));
                let timestamp = format_table_update_time(table.UpdateTS, time_zone)
                    .unwrap_or_else(|| CONCRETE_NULL_VALUE.to_owned());
                vec![
                    table_name,
                    "InnoDB".to_owned(),
                    "10".to_owned(),
                    "Compact".to_owned(),
                    row_count.to_string(),
                    "0".to_owned(),
                    "0".to_owned(),
                    "0".to_owned(),
                    "0".to_owned(),
                    "0".to_owned(),
                    (table.AutoIncID > 0)
                        .then(|| table.AutoIncID.to_string())
                        .unwrap_or_else(|| CONCRETE_NULL_VALUE.to_owned()),
                    timestamp.clone(),
                    timestamp,
                    CONCRETE_NULL_VALUE.to_owned(),
                    if table.Collate.is_empty() {
                        "utf8mb4_bin".to_owned()
                    } else {
                        table.Collate.clone()
                    },
                    CONCRETE_NULL_VALUE.to_owned(),
                    if table.GetPartitionInfo().is_some() {
                        "partitioned".to_owned()
                    } else {
                        String::new()
                    },
                    table.Comment.clone(),
                ]
            })
            .collect::<Vec<_>>();
        rows.sort();
        ConcreteRecordSet::new(
            [
                "Name",
                "Engine",
                "Version",
                "Row_format",
                "Rows",
                "Avg_row_length",
                "Data_length",
                "Max_data_length",
                "Index_length",
                "Data_free",
                "Auto_increment",
                "Create_time",
                "Update_time",
                "Check_time",
                "Collation",
                "Checksum",
                "Create_options",
                "Comment",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rows,
        )
    }

    /// 写入列统计使用情况。
    pub(super) fn execute_column_usage_insert(
        &self,
        statement: &ast::InsertStmt,
    ) -> SessionResult<bool> {
        let Some(table_refs) = statement.Table.as_ref() else {
            return Ok(false);
        };
        let Some(ast::ResultSetNode::TableSource(source)) = table_refs.TableRefs.Left.as_deref()
        else {
            return Ok(false);
        };
        if source.Source.Schema.L != "mysql" || source.Source.Name.L != "column_stats_usage" {
            return Ok(false);
        }
        for values in &statement.Lists {
            if values.len() < 4 {
                return Err(SessionError::new(
                    "mysql.column_stats_usage requires four values",
                ));
            }
            let table_id = literal(&values[0])?
                .parse::<i64>()
                .map_err(|error| session_error("parse column usage table ID", error))?;
            let column_id = literal(&values[1])?
                .parse::<i64>()
                .map_err(|error| session_error("parse column usage column ID", error))?;
            let last_used_at = literal(&values[2]).ok();
            let last_analyzed_at = literal(&values[3]).ok();
            self.domain
                .stats_handle()
                .lock()
                .expect("domain stats handle lock poisoned")
                .record_column_usage(astersql_statistics_handle::RuntimeColumnUsage {
                    table_id,
                    column_id,
                    last_used_at,
                    last_analyzed_at,
                });
        }
        Ok(true)
    }

    /// 写入 ANALYZE 任务记录。
    pub(super) fn execute_analyze_job_insert(
        &self,
        statement: &ast::InsertStmt,
    ) -> SessionResult<bool> {
        let Some(table_refs) = statement.Table.as_ref() else {
            return Ok(false);
        };
        let Some(ast::ResultSetNode::TableSource(source)) = table_refs.TableRefs.Left.as_deref()
        else {
            return Ok(false);
        };
        if source.Source.Schema.L != "mysql" || source.Source.Name.L != "analyze_jobs" {
            return Ok(false);
        }
        let columns = statement
            .Columns
            .iter()
            .map(|column| column.Name.L.clone())
            .collect::<Vec<_>>();
        let nullable_literal = |expression: &ast::ExprNode| -> SessionResult<Option<String>> {
            if matches!(
                &expression.Kind,
                ast::ExprKind::Value(value) if matches!(value.Datum, ast::ValueDatum::Null)
            ) {
                Ok(None)
            } else {
                literal(expression).map(Some)
            }
        };
        let mut jobs = Vec::new();
        for values in &statement.Lists {
            if values.len() != columns.len() {
                return Err(SessionError::new(
                    "mysql.analyze_jobs column and value counts differ",
                ));
            }
            let fields = columns
                .iter()
                .zip(values)
                .map(|(column, value)| Ok((column.as_str(), nullable_literal(value)?)))
                .collect::<SessionResult<HashMap<_, _>>>()?;
            let required = |name: &str| -> SessionResult<String> {
                fields
                    .get(name)
                    .and_then(Clone::clone)
                    .ok_or_else(|| SessionError::new(format!("mysql.analyze_jobs requires {name}")))
            };
            let database = required("table_schema")?;
            let table = required("table_name")?;
            let (key, _) = self.domain.stats_table(&database, &table).ok_or_else(|| {
                SessionError::new(format!("unknown analyze job table {database}.{table}"))
            })?;
            let state = required("state")?;
            jobs.push(astersql_statistics_handle::RuntimeAnalyzeJob {
                physical_ids: vec![key.table_id],
                database,
                table,
                partition: fields
                    .get("partition_name")
                    .and_then(Clone::clone)
                    .unwrap_or_default(),
                job_info: required("job_info")?,
                row_count: required("processed_rows")?
                    .parse()
                    .map_err(|error| session_error("parse analyze processed_rows", error))?,
                start_time: required("start_time")?,
                end_time: fields
                    .get("end_time")
                    .and_then(Clone::clone)
                    .unwrap_or_default(),
                state: state.clone(),
                fail_reason: fields.get("fail_reason").and_then(Clone::clone),
                instance: required("instance")?,
                process_id: fields
                    .get("process_id")
                    .and_then(Clone::clone)
                    .map(|value| {
                        value
                            .parse()
                            .map_err(|error| session_error("parse analyze process_id", error))
                    })
                    .transpose()?,
                remaining_duration: (state == "running").then(|| "0s".to_owned()),
                ..Default::default()
            });
        }
        self.domain
            .stats_handle()
            .lock()
            .expect("domain stats handle lock poisoned")
            .record_analyze_jobs(jobs);
        Ok(true)
    }

    /// 执行 FLUSH 刷出统计增量。
    pub(super) fn execute_flush_stats_delta(
        &self,
        statement: &ast::FlushStmt,
    ) -> SessionResult<()> {
        let current_database = self.current_database();
        let catalog = self.domain.stats_context().catalog();
        let mut physical_ids = BTreeSet::new();
        for object in &statement.FlushObjects {
            for ((database, table_name), (key, info)) in &catalog {
                let selected = match object.StatsObjectScope {
                    ast::StatsObjectScope::Global => true,
                    ast::StatsObjectScope::Database => {
                        database.eq_ignore_ascii_case(&object.DBName.L)
                    }
                    ast::StatsObjectScope::Table => {
                        let selected_database = if object.DBName.L.is_empty() {
                            current_database.as_str()
                        } else {
                            object.DBName.L.as_str()
                        };
                        database.eq_ignore_ascii_case(selected_database)
                            && table_name.eq_ignore_ascii_case(&object.TableName.L)
                    }
                };
                if !selected {
                    continue;
                }
                physical_ids.insert(key.table_id);
                if let Some(partition) = info.GetPartitionInfo() {
                    physical_ids.extend(partition.Definitions.iter().map(|item| item.ID));
                }
            }
        }
        self.domain
            .flush_stats_delta_history(&physical_ids.into_iter().collect::<Vec<_>>())
            .map_err(|error| session_error("FLUSH STATS_DELTA", error))?;
        Ok(())
    }

    /// Routes `DELETE FROM mysql.stats_*` (with optional WHERE) through the
    /// restricted statistics SQL backend, matching Go's system-table DML path.
    /// 识别对 mysql 统计系统表的 DELETE。
    pub(super) fn mysql_stats_system_delete_sql(
        &self,
        statement: &ast::DeleteStmt,
    ) -> Option<String> {
        let table = statement.TableRefs.as_ref()?;
        let ast::ResultSetNode::TableSource(source) = table.TableRefs.Left.as_deref()? else {
            return None;
        };
        if source.Source.Schema.L != "mysql" {
            return None;
        }
        match source.Source.Name.L.as_str() {
            "stats_table_locked" | "stats_meta" | "stats_histograms" | "stats_meta_history"
            | "stats_history" | "stats_fm_sketch" | "column_stats_usage" => {}
            _ => return None,
        }
        let mut sql = format!("delete from mysql.{}", source.Source.Name.L);
        if let Some(where_clause) = statement.Where.as_ref() {
            // Preserve a narrow equality filter when present; unrestricted deletes
            // clear the whole stats system table as in Go TestNotDumpSysTable.
            if let ast::ExprKind::Binary { Op, L, R } = &where_clause.Kind {
                if Op == "=" {
                    if let (ast::ExprKind::Column(column), Ok(value)) = (&L.Kind, literal(R)) {
                        sql.push_str(&format!(" where {} = {}", column.Name.L, value));
                    } else if let (ast::ExprKind::Column(column), Ok(value)) = (&R.Kind, literal(L))
                    {
                        sql.push_str(&format!(" where {} = {}", column.Name.L, value));
                    }
                }
            }
        }
        Some(sql)
    }

    /// Routes `UPDATE mysql.stats_*` through the restricted statistics SQL
    /// backend. Go tests reach for this to corrupt persisted histograms
    /// (`UPDATE mysql.stats_buckets SET upper_bound = ...`).
    /// 识别对 mysql 统计系统表的 UPDATE。
    pub(super) fn mysql_stats_system_update_sql(
        &self,
        statement: &ast::UpdateStmt,
    ) -> Option<String> {
        let table = statement.TableRefs.as_ref()?;
        let ast::ResultSetNode::TableSource(source) = table.TableRefs.Left.as_deref()? else {
            return None;
        };
        if source.Source.Schema.L != "mysql"
            || !matches!(
                source.Source.Name.L.as_str(),
                "stats_buckets" | "stats_histograms"
            )
        {
            return None;
        }
        let assignments = statement
            .List
            .iter()
            .map(|assignment| {
                stats_system_literal(&assignment.Expr)
                    .map(|value| format!("{} = {}", assignment.Column.Name.L, value))
            })
            .collect::<SessionResult<Vec<_>>>()
            .ok()?;
        if assignments.is_empty() {
            return None;
        }
        let mut sql = format!(
            "update mysql.{} set {}",
            source.Source.Name.L,
            assignments.join(", ")
        );
        if let Some(where_clause) = statement.Where.as_ref() {
            let ast::ExprKind::Binary { Op, L, R } = &where_clause.Kind else {
                return None;
            };
            if Op != "=" {
                return None;
            }
            let filter = match (&L.Kind, &R.Kind) {
                (ast::ExprKind::Column(column), _) => {
                    Some((column.Name.L.clone(), stats_system_literal(R).ok()?))
                }
                (_, ast::ExprKind::Column(column)) => {
                    Some((column.Name.L.clone(), stats_system_literal(L).ok()?))
                }
                _ => None,
            }?;
            sql.push_str(&format!(" where {} = {}", filter.0, filter.1));
        }
        Some(sql)
    }

    /// Serves `SELECT ... FROM mysql.analyze_jobs`. Go persists analyze jobs in
    /// a real system table; this runtime keeps them inside the statistics
    /// handle, so the rows are materialised here with the Go column layout of
    /// `metadef.CreateAnalyzeJobsTable`.
    /// 查询 ANALYZE 任务列表。
    fn execute_analyze_jobs_select(
        &self,
        statement: &ast::SelectStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        const COLUMNS: [&str; 13] = [
            "id",
            "update_time",
            "table_schema",
            "table_name",
            "partition_name",
            "job_info",
            "processed_rows",
            "start_time",
            "end_time",
            "state",
            "fail_reason",
            "instance",
            "process_id",
        ];
        let mut rows = self
            .domain
            .stats_context()
            .analyze_jobs()
            .into_iter()
            .enumerate()
            .map(|(offset, job)| {
                let update_time = if job.end_time.is_empty() {
                    job.start_time.clone()
                } else {
                    job.end_time.clone()
                };
                vec![
                    (offset + 1).to_string(),
                    update_time,
                    job.database,
                    job.table,
                    job.partition,
                    job.job_info,
                    job.row_count.to_string(),
                    job.start_time,
                    job.end_time,
                    job.state,
                    job.fail_reason.unwrap_or_else(|| SHOW_NULL_CELL.to_owned()),
                    job.instance,
                    job.process_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| SHOW_NULL_CELL.to_owned()),
                ]
            })
            .collect::<Vec<_>>();
        if let Some(predicate) = statement.Where.as_ref() {
            let mut kept = Vec::with_capacity(rows.len());
            for row in rows {
                if Self::show_predicate(predicate, &COLUMNS, &row)? {
                    kept.push(row);
                }
            }
            rows = kept;
        }
        for item in statement.OrderBy.iter().rev() {
            let ast::ExprKind::Column(column) = &item.Expr.Kind else {
                return Err(SessionError::new(
                    "mysql.analyze_jobs ORDER BY requires a column",
                ));
            };
            let index = COLUMNS
                .iter()
                .position(|name| name.eq_ignore_ascii_case(&column.Name.L))
                .ok_or_else(|| {
                    SessionError::new(format!("unknown analyze job column {}", column.Name.O))
                })?;
            rows.sort_by(|left, right| {
                let ordering = match (left[index].parse::<i64>(), right[index].parse::<i64>()) {
                    (Ok(left), Ok(right)) => left.cmp(&right),
                    _ => left[index].cmp(&right[index]),
                };
                if item.Desc {
                    ordering.reverse()
                } else {
                    ordering
                }
            });
        }
        if let Some(limit) = statement.Limit.as_ref() {
            let count = limit
                .Count
                .as_ref()
                .map(|expression| {
                    literal(expression)?
                        .parse::<usize>()
                        .map_err(|error| session_error("parse analyze job LIMIT", error))
                })
                .transpose()?
                .unwrap_or(rows.len());
            let offset = limit
                .Offset
                .as_ref()
                .map(|expression| {
                    literal(expression)?
                        .parse::<usize>()
                        .map_err(|error| session_error("parse analyze job OFFSET", error))
                })
                .transpose()?
                .unwrap_or(0);
            rows = rows.into_iter().skip(offset).take(count).collect();
        }
        let mut headers = Vec::new();
        let mut projection = Vec::new();
        for field in &statement.Fields.Fields {
            if field.WildCard.is_some() {
                headers.extend(COLUMNS.iter().map(|name| (*name).to_owned()));
                projection.extend(0..COLUMNS.len());
                continue;
            }
            let expression = field
                .Expr
                .as_ref()
                .ok_or_else(|| SessionError::new("mysql.analyze_jobs SELECT requires columns"))?;
            let ast::ExprKind::Column(column) = &expression.Kind else {
                return Err(SessionError::new(
                    "mysql.analyze_jobs SELECT projection is unsupported",
                ));
            };
            let index = COLUMNS
                .iter()
                .position(|name| name.eq_ignore_ascii_case(&column.Name.L))
                .ok_or_else(|| {
                    SessionError::new(format!("unknown analyze job column {}", column.Name.O))
                })?;
            headers.push(if field.AsName.O.is_empty() {
                column.Name.O.clone()
            } else {
                field.AsName.O.clone()
            });
            projection.push(index);
        }
        let projected = rows
            .into_iter()
            .map(|row| {
                projection
                    .iter()
                    .map(|index| row[*index].clone())
                    .collect::<Vec<_>>()
            })
            .collect();
        Ok(ConcreteRecordSet::new(headers, projected))
    }

    /// 查询统计系统表。
    pub(super) fn execute_stats_system_select(
        &self,
        statement: &ast::SelectStmt,
        sql: &str,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        let Some(from) = statement.From.as_ref() else {
            return Ok(None);
        };
        let Some(ast::ResultSetNode::TableSource(source)) = from.TableRefs.Left.as_deref() else {
            return Ok(None);
        };
        if source.Source.Schema.L != "mysql" {
            return Ok(None);
        }
        match source.Source.Name.L.as_str() {
            "analyze_jobs" => return self.execute_analyze_jobs_select(statement).map(Some),
            "stats_table_locked" | "stats_meta" | "stats_histograms" | "stats_meta_history"
            | "stats_history" | "stats_fm_sketch" | "stats_buckets" | "tidb" => {}
            _ => return Ok(None),
        }
        let catalog = self.metadata_catalog()?;
        let table = catalog
            .get(&(source.Source.Schema.L.clone(), source.Source.Name.L.clone()))
            .ok_or_else(|| {
                SessionError::new(format!(
                    "Table '{}.{}' doesn't exist",
                    source.Source.Schema.O, source.Source.Name.O
                ))
            })?;
        let mut columns = Vec::new();
        for field in &statement.Fields.Fields {
            if field.WildCard.is_some() {
                columns.extend(table.Columns.iter().map(|column| column.Name.L.clone()));
                continue;
            }
            columns.push({
                let expression = field.Expr.as_ref().ok_or_else(|| {
                    SessionError::new("system statistics SELECT requires columns")
                })?;
                match &expression.Kind {
                    ast::ExprKind::Column(column) => Ok(column.Name.L.clone()),
                    ast::ExprKind::AggregateFunction { Name, .. }
                        if Name.eq_ignore_ascii_case("count") =>
                    {
                        Ok("count(*)".to_owned())
                    }
                    ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "hex" => {
                        let column = Args
                            .first()
                            .and_then(|arg| match &arg.Kind {
                                ast::ExprKind::Column(column) => Some(column.Name.L.as_str()),
                                _ => None,
                            })
                            .unwrap_or_default();
                        Ok(format!("hex({column})"))
                    }
                    ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "truncate" => {
                        let column = Args
                            .first()
                            .and_then(|argument| match &argument.Kind {
                                ast::ExprKind::Column(column) => Some(column.Name.L.as_str()),
                                _ => None,
                            })
                            .ok_or_else(|| {
                                SessionError::new("system statistics TRUNCATE requires a column")
                            })?;
                        let digits = Args.get(1).ok_or_else(|| {
                            SessionError::new("system statistics TRUNCATE requires precision")
                        })?;
                        Ok(format!("truncate({column},{})", literal(digits)?))
                    }
                    _ => Err(SessionError::new(
                        "system statistics SELECT projection is unsupported",
                    )),
                }
            }?);
        }
        let rows = {
            let mut state = self.state.borrow_mut();
            if source.Source.Name.L == "tidb" {
                if let Some(transaction) = state.transaction.as_mut() {
                    self.domain
                        .restricted_system_sql_in_transaction(transaction, sql, &[])
                } else {
                    self.domain.restricted_stats_query(sql, &[])
                }
            } else {
                self.domain.restricted_stats_query(sql, &[])
            }
        }
        .map_err(|error| session_error("read mysql statistics system table", error))?;
        Ok(Some(ConcreteRecordSet::new(columns, rows)))
    }
}
