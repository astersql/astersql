// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `SHOW PLACEMENT` / `SHOW PLACEMENT LABELS` 语句执行器。
//
// Placement（放置策略）描述数据副本在 TiKV Store 上的分布规则，例如主 Region（键值空间分片单元）
// 所在可用区、follower 副本数等。本模块汇总库/表/分区与全局/元数据 range 的策略与调度状态，
// 并通过 `ShowPlacementBackend` 访问权限、InfoSchema 与 PD（Placement Driver）复制状态。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::collections::{HashMap, HashSet};
use std::fmt::Display;

/// 全局 range bundle 在 PD 中的前缀标识（对应整集群默认放置范围）。
pub const TiDBBundleRangePrefixForGlobal: &str = "TiDB_GLOBAL";
/// 元数据 range bundle 前缀（覆盖 meta 相关键空间）。
pub const TiDBBundleRangePrefixForMeta: &str = "TiDB_META";

#[derive(Clone, Debug, Eq, PartialEq)]
/// 大小写不敏感字符串：保留原始写法，同时缓存小写形式用于比较。
pub struct CiString {
    pub original: String,
    pub lower: String,
}

impl CiString {
    /// 由任意可转字符串的值构造，自动生成小写缓存。
    pub fn new(value: impl Into<String>) -> Self {
        let original = value.into();
        let lower = original.to_lowercase();
        Self { original, lower }
    }

    /// 返回原始字符串（保持用户输入的大小写）。
    pub fn String(&self) -> String {
        self.original.clone()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// TiKV Store 标签（如 zone/rack），用于 Placement 规则匹配。
pub struct StoreLabel {
    pub key: String,
    pub value: String,
}

/// Parsed representation of TIKV_STORE_STATUS.LABEL. JSON decoding belongs to
/// the backend so malformed JSON and storage errors retain their native type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreLabelsJson {
    Null,
    Array(Vec<StoreLabel>),
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// SHOW 结果单元格：普通字符串或 JSON 字符串数组。
pub enum PlacementValue {
    String(String),
    JsonStringArray(Vec<String>),
}

/// 一行 SHOW PLACEMENT 结果。
pub type PlacementRow = Vec<PlacementValue>;

/// 将输入包装为字符串单元格。
fn string_value(value: impl Into<String>) -> PlacementValue {
    PlacementValue::String(value.into())
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Placement 策略的可展示文本（如 `PRIMARY_REGION="us-east-1"`）。
pub struct PlacementSettings {
    pub display: String,
}

impl PlacementSettings {
    pub fn String(&self) -> String {
        self.display.clone()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 对已定义 Placement Policy 的引用（按名称）。
pub struct PolicyRefInfo {
    pub name: CiString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Placement Policy 元信息：名称及其 settings 展示串。
pub struct PolicyInfo {
    pub name: CiString,
    pub placement_settings: PlacementSettings,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 库级 Placement 信息（库名与可选策略引用）。
pub struct DatabaseInfo {
    pub name: CiString,
    pub placement_policy_ref: Option<PolicyRefInfo>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 分区定义中的 Placement 相关字段。
pub struct PartitionDefinition {
    pub id: i64,
    pub name: CiString,
    pub placement_policy_ref: Option<PolicyRefInfo>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表级 Placement 信息，可带分区列表。
pub struct TableInfo {
    pub id: i64,
    pub name: CiString,
    pub placement_policy_ref: Option<PolicyRefInfo>,
    pub partitions: Option<Vec<PartitionDefinition>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 带特殊属性（含 Placement）的库及其表集合。
pub struct SpecialAttributeDatabase {
    pub database_name: CiString,
    pub table_infos: Vec<TableInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
/// Placement 调度进度：Pending（待调度）→ InProgress（进行中）→ Scheduled（已完成）。
pub enum PlacementScheduleState {
    Pending = 0,
    InProgress = 1,
    Scheduled = 2,
}

impl PlacementScheduleState {
    /// 转为 SHOW 输出使用的大写状态名。
    pub fn String(self) -> &'static str {
        match self {
            Self::Scheduled => "SCHEDULED",
            Self::InProgress => "INPROGRESS",
            Self::Pending => "PENDING",
        }
    }
}

#[derive(Debug)]
/// 查询调度状态失败时携带当时已累积的状态与底层错误。
pub struct ScheduleError<E> {
    pub state: PlacementScheduleState,
    pub error: E,
}

/// Production boundary for restricted SQL, privileges, InfoSchema, DDL range
/// policies, table-key encoding, and PD replication-state queries.
/// 生产边界：受限 SQL、权限、InfoSchema、DDL range 策略、表键编码与 PD 复制状态查询。
pub trait ShowPlacementBackend {
    type Context: Clone;
    type Error: Display;

    fn error(&self, message: String) -> Self::Error;
    fn database_access_denied(&self) -> Self::Error;
    fn database_not_exists(&self, database: &str) -> Self::Error;
    fn unknown_partition(&self, partition: &str, table: &str) -> Self::Error;

    fn restricted_store_labels(
        &self,
        context: &Self::Context,
    ) -> Result<Vec<StoreLabelsJson>, Self::Error>;

    /// True only when both a privilege manager and a session user exist.
    fn database_visibility_check_enabled(&self) -> bool;
    fn database_is_visible(&self, database: &str) -> bool;
    /// True whenever a privilege manager exists.
    fn table_privilege_check_enabled(&self) -> bool;
    fn table_is_visible(&self, database: &str, table: &str) -> bool;

    fn schema_by_name(&self, name: &CiString) -> Option<DatabaseInfo>;
    fn selected_table(&self, schema: &CiString, table: &CiString)
    -> Result<TableInfo, Self::Error>;
    fn all_placement_policies(&self) -> Vec<PolicyInfo>;
    fn all_schema_names(&self) -> Vec<CiString>;
    fn schema_simple_table_ids(
        &self,
        context: &Self::Context,
        database: &CiString,
    ) -> Result<Vec<i64>, Self::Error>;
    fn tables_with_special_attributes(&self) -> Vec<SpecialAttributeDatabase>;
    fn policy_by_name(&self, name: &CiString) -> Option<PolicyInfo>;
    fn table_by_id(&self, context: &Self::Context, id: i64) -> Option<TableInfo>;

    fn range_policy_name(
        &self,
        context: &Self::Context,
        range_bundle_id: &str,
    ) -> Result<String, Self::Error>;
    fn range_key_hex(&self, range_bundle_id: &str) -> (String, String);
    fn encoded_table_range(&self, id: i64) -> (Vec<u8>, Vec<u8>);
    fn replication_state(
        &self,
        context: &Self::Context,
        start_key: &[u8],
        end_key: &[u8],
    ) -> Result<PlacementScheduleState, Self::Error>;
}

#[derive(Default)]
/// 聚合各 Store 标签键到取值集合，最终输出去重排序后的 LABELS 行。
pub struct showPlacementLabelsResultBuilder {
    pub labelKey2values: HashMap<String, HashSet<String>>,
}

impl showPlacementLabelsResultBuilder {
    /// 解析并并入一组 Store 标签；非法 JSON 形态返回错误。
    pub fn AppendStoreLabels(&mut self, labels_json: StoreLabelsJson) -> Result<(), String> {
        let labels = match labels_json {
            StoreLabelsJson::Null => return Ok(()),
            StoreLabelsJson::Array(labels) => labels,
            StoreLabelsJson::Other => {
                return Err("only array or null type is allowed".to_owned());
            }
        };

        for label in labels {
            self.labelKey2values
                .entry(label.key)
                .or_default()
                .insert(label.value);
        }
        Ok(())
    }

    /// 按标签键排序生成结果行，每行的值为该键下取值的排序 JSON 数组。
    pub fn BuildRows(&self) -> Vec<PlacementRow> {
        self.sortMapKeys(&self.labelKey2values)
            .into_iter()
            .map(|key| {
                let mut values: Vec<_> = self.labelKey2values[&key].iter().cloned().collect();
                values.sort();
                vec![string_value(key), PlacementValue::JsonStringArray(values)]
            })
            .collect()
    }

    /// 返回 map 键的排序副本。
    pub fn sortMapKeys<T>(&self, values: &HashMap<String, T>) -> Vec<String> {
        let mut keys: Vec<_> = values.keys().cloned().collect();
        keys.sort();
        keys
    }
}

/// 某一对象（库/表/分区）名称及其 Placement 结果行集合。
pub struct tableRowSet {
    pub name: String,
    pub rows: Vec<PlacementRow>,
}

/// `SHOW PLACEMENT` 执行器：按 DB/表/分区过滤或拉取全集，填充 `rows`。
pub struct ShowPlacementExec<B: ShowPlacementBackend> {
    pub backend: B,
    pub DBName: CiString,
    pub TableSchema: CiString,
    pub TableName: CiString,
    pub Partition: CiString,
    pub rows: Vec<PlacementRow>,
}

impl<B: ShowPlacementBackend> ShowPlacementExec<B> {
    /// 追加一行结果。
    fn appendRow(&mut self, row: PlacementRow) {
        self.rows.push(row);
    }

    /// 拉取所有 Store 标签并构建 `SHOW PLACEMENT LABELS` 结果。
    pub fn fetchShowPlacementLabels(&mut self, context: &B::Context) -> Result<(), B::Error> {
        let labels = self.backend.restricted_store_labels(context)?;
        // 聚合全部 Store 标签后一次性 BuildRows。
        let mut builder = showPlacementLabelsResultBuilder::default();
        for label in labels {
            builder
                .AppendStoreLabels(label)
                .map_err(|error| self.backend.error(error))?;
        }
        for row in builder.BuildRows() {
            self.appendRow(row);
        }
        Ok(())
    }

    /// 仅展示指定数据库的 Placement 与调度状态。
    pub fn fetchShowPlacementForDB(&mut self, context: &B::Context) -> Result<(), B::Error> {
        if self.backend.database_visibility_check_enabled()
            && !self.backend.database_is_visible(&self.DBName.String())
        {
            return Err(self.backend.database_access_denied());
        }

        let database = self
            .backend
            .schema_by_name(&self.DBName)
            .ok_or_else(|| self.backend.database_not_exists(&self.DBName.original))?;
        if let Some(placement) = self.getDBPlacement(&database)? {
            let state = self
                .fetchDBScheduleState(context, None, &database)
                .map_err(|failure| failure.error)?;
            self.appendRow(vec![
                string_value(format!("DATABASE {}", database.name.String())),
                string_value(placement.String()),
                string_value(state.String()),
            ]);
        }
        Ok(())
    }

    /// 仅展示指定表（及分区）的 Placement。
    pub fn fetchShowPlacementForTable(&mut self, context: &B::Context) -> Result<(), B::Error> {
        let table = self
            .backend
            .selected_table(&self.TableSchema, &self.TableName)?;
        if let Some(placement) = self.getTablePlacement(&table)? {
            let state = fetchTableScheduleState(&self.backend, context, None, &table)
                .map_err(|failure| failure.error)?;
            self.appendRow(vec![
                string_value(format!(
                    "TABLE {}",
                    format_identifier(&self.TableSchema, &table.name)
                )),
                string_value(placement.String()),
                string_value(state.String()),
            ]);
        }
        Ok(())
    }

    /// 仅展示指定分区的 Placement。
    pub fn fetchShowPlacementForPartition(&mut self, context: &B::Context) -> Result<(), B::Error> {
        let table = self
            .backend
            .selected_table(&self.TableSchema, &self.TableName)?;
        let partition = table
            .partitions
            .as_ref()
            .and_then(|partitions| {
                partitions
                    .iter()
                    .find(|partition| partition.name.lower == self.Partition.lower)
            })
            .cloned()
            .ok_or_else(|| {
                self.backend
                    .unknown_partition(&self.Partition.original, &table.name.original)
            })?;

        let table_placement = self.getTablePlacement(&table)?;
        if let Some(placement) = self.getPartitionPlacement(table_placement, &partition)? {
            let state = fetchPartitionScheduleState(&self.backend, context, None, &partition)
                .map_err(|failure| failure.error)?;
            self.appendRow(vec![
                string_value(format!(
                    "TABLE {} PARTITION {}",
                    format_identifier(&self.TableSchema, &table.name),
                    partition.name.String()
                )),
                string_value(placement.String()),
                string_value(state.String()),
            ]);
        }
        Ok(())
    }

    /// 拉取全部策略、range、库表 Placement（完整 SHOW）。
    pub fn fetchShowPlacement(&mut self, context: &B::Context) -> Result<(), B::Error> {
        self.fetchAllPlacementPolicies()?;
        let mut scheduled = HashMap::new();
        self.fetchAllDBPlacements(context, Some(&mut scheduled))?;
        self.fetchAllTablePlacements(context, Some(&mut scheduled))?;
        self.fetchRangesPlacementPlocy(context)
    }

    /// 枚举所有命名 Placement Policy 并写入结果行。
    pub fn fetchAllPlacementPolicies(&mut self) -> Result<(), B::Error> {
        let mut policies = self.backend.all_placement_policies();
        policies.sort_by(|left, right| left.name.original.cmp(&right.name.original));
        for policy in policies {
            self.appendRow(vec![
                string_value(format!("POLICY {}", policy.name.String())),
                string_value(policy.placement_settings.String()),
                string_value("NULL"),
            ]);
        }
        Ok(())
    }

    /// 查询全局/元数据 range 绑定的策略与调度状态（函数名沿用 Go 拼写）。
    pub fn fetchRangesPlacementPlocy(&mut self, context: &B::Context) -> Result<(), B::Error> {
        for range_bundle_id in [TiDBBundleRangePrefixForGlobal, TiDBBundleRangePrefixForMeta] {
            let policy_name = self.backend.range_policy_name(context, range_bundle_id)?;
            if policy_name.is_empty() {
                continue;
            }

            let (start_hex, end_hex) = self.backend.range_key_hex(range_bundle_id);
            let start_key = decode_hex_ignoring_error(&start_hex);
            let end_key = decode_hex_ignoring_error(&end_hex);
            let state = self
                .backend
                .replication_state(context, &start_key, &end_key)?;
            let policy_key = CiString::new(policy_name.clone());
            let policy = self.backend.policy_by_name(&policy_key).ok_or_else(|| {
                self.backend
                    .error(format!("Policy with name '{}' not found", policy_name))
            })?;
            self.appendRow(vec![
                string_value(format!("RANGE {range_bundle_id}")),
                string_value(policy.placement_settings.String()),
                string_value(state.String()),
            ]);
        }
        Ok(())
    }

    /// 遍历可见库，收集库级 Placement 行。
    pub fn fetchAllDBPlacements(
        &mut self,
        context: &B::Context,
        mut schedule_state: Option<&mut HashMap<i64, PlacementScheduleState>>,
    ) -> Result<(), B::Error> {
        let mut databases = self.backend.all_schema_names();
        databases.sort_by(|left, right| left.original.cmp(&right.original));

        for database_name in databases {
            if self.backend.database_visibility_check_enabled()
                && !self.backend.database_is_visible(&database_name.original)
            {
                continue;
            }
            let database = self
                .backend
                .schema_by_name(&database_name)
                .ok_or_else(|| self.backend.database_not_exists(&database_name.original))?;
            if let Some(placement) = self.getDBPlacement(&database)? {
                let state = self
                    .fetchDBScheduleState(context, schedule_state.as_deref_mut(), &database)
                    .map_err(|failure| failure.error)?;
                self.appendRow(vec![
                    string_value(format!("DATABASE {}", database.name.String())),
                    string_value(placement.String()),
                    string_value(state.String()),
                ]);
            }
        }
        Ok(())
    }

    /// 汇总库内所有表的调度状态，取最落后的状态（见 `accumulateState`）。
    pub fn fetchDBScheduleState(
        &self,
        context: &B::Context,
        mut schedule_state: Option<&mut HashMap<i64, PlacementScheduleState>>,
        database: &DatabaseInfo,
    ) -> Result<PlacementScheduleState, ScheduleError<B::Error>> {
        let mut state = PlacementScheduleState::Scheduled;
        let table_ids = self
            .backend
            .schema_simple_table_ids(context, &database.name)
            .map_err(|error| ScheduleError { state, error })?;
        for table_id in table_ids {
            let schedule = self.fetchTableScheduleStateByTableID(
                context,
                schedule_state.as_deref_mut(),
                table_id,
            )?;
            state = accumulateState(state, schedule);
            if state != PlacementScheduleState::Scheduled {
                break;
            }
        }
        Ok(state)
    }

    /// 遍历库内表/分区，按权限过滤后输出 Placement 行。
    pub fn fetchAllTablePlacements(
        &mut self,
        context: &B::Context,
        mut schedule_state: Option<&mut HashMap<i64, PlacementScheduleState>>,
    ) -> Result<(), B::Error> {
        // The Go implementation computes a sorted schema list here but consumes
        // the InfoSchema special-attribute grouping order, so only table rows
        // within each returned database group are explicitly sorted.
        let mut databases = self.backend.all_schema_names();
        databases.sort_by(|left, right| left.original.cmp(&right.original));

        for database in self.backend.tables_with_special_attributes() {
            let mut table_row_sets = Vec::new();
            for table in database.table_infos {
                if self.backend.table_privilege_check_enabled()
                    && !self
                        .backend
                        .table_is_visible(&database.database_name.original, &table.name.original)
                {
                    continue;
                }

                let identifier = format_identifier(&database.database_name, &table.name);
                let table_placement = self.getTablePlacement(&table)?;
                let mut rows = Vec::new();
                if let Some(placement) = table_placement.as_ref() {
                    let state = fetchTableScheduleState(
                        &self.backend,
                        context,
                        schedule_state.as_deref_mut(),
                        &table,
                    )
                    .map_err(|failure| failure.error)?;
                    rows.push(vec![
                        string_value(format!("TABLE {identifier}")),
                        string_value(placement.String()),
                        string_value(state.String()),
                    ]);
                }

                if let Some(partitions) = table.partitions.as_ref() {
                    for partition in partitions {
                        let partition_placement =
                            self.getPartitionPlacement(table_placement.clone(), partition)?;
                        if let Some(placement) = partition_placement {
                            let state = fetchPartitionScheduleState(
                                &self.backend,
                                context,
                                schedule_state.as_deref_mut(),
                                partition,
                            )
                            .map_err(|failure| failure.error)?;
                            rows.push(vec![
                                string_value(format!(
                                    "TABLE {identifier} PARTITION {}",
                                    partition.name.String()
                                )),
                                string_value(placement.String()),
                                string_value(state.String()),
                            ]);
                        }
                    }
                }

                if !rows.is_empty() {
                    table_row_sets.push(tableRowSet {
                        name: table.name.String(),
                        rows,
                    });
                }
            }

            table_row_sets.sort_by(|left, right| left.name.cmp(&right.name));
            for row_set in table_row_sets {
                for row in row_set.rows {
                    self.appendRow(row);
                }
            }
        }
        Ok(())
    }

    /// 解析库上的 Placement Policy 展示文本。
    pub fn getDBPlacement(
        &self,
        database: &DatabaseInfo,
    ) -> Result<Option<PlacementSettings>, B::Error> {
        self.getPolicyPlacement(database.placement_policy_ref.as_ref())
    }

    /// 解析表上的 Placement Policy 展示文本。
    pub fn getTablePlacement(
        &self,
        table: &TableInfo,
    ) -> Result<Option<PlacementSettings>, B::Error> {
        self.getPolicyPlacement(table.placement_policy_ref.as_ref())
    }

    /// 解析分区上的 Placement Policy 展示文本。
    pub fn getPartitionPlacement(
        &self,
        table_placement: Option<PlacementSettings>,
        partition: &PartitionDefinition,
    ) -> Result<Option<PlacementSettings>, B::Error> {
        let partition_placement =
            self.getPolicyPlacement(partition.placement_policy_ref.as_ref())?;
        Ok(partition_placement.or(table_placement))
    }

    /// 按策略引用名查找策略展示串。
    pub fn getPolicyPlacement(
        &self,
        policy_ref: Option<&PolicyRefInfo>,
    ) -> Result<Option<PlacementSettings>, B::Error> {
        let Some(policy_ref) = policy_ref else {
            return Ok(None);
        };
        let policy = self
            .backend
            .policy_by_name(&policy_ref.name)
            .ok_or_else(|| {
                self.backend.error(format!(
                    "Policy with name '{}' not found",
                    policy_ref.name.String()
                ))
            })?;
        Ok(Some(policy.placement_settings))
    }

    /// 按物理表 ID 查询 PD 复制/调度状态。
    pub fn fetchTableScheduleStateByTableID(
        &self,
        context: &B::Context,
        mut schedule_state: Option<&mut HashMap<i64, PlacementScheduleState>>,
        id: i64,
    ) -> Result<PlacementScheduleState, ScheduleError<B::Error>> {
        let mut state = PlacementScheduleState::Scheduled;
        let schedule =
            fetchScheduleState(&self.backend, context, schedule_state.as_deref_mut(), id).map_err(
                |failure| ScheduleError {
                    state,
                    error: failure.error,
                },
            )?;
        state = accumulateState(state, schedule);
        if state != PlacementScheduleState::Scheduled {
            return Ok(state);
        }

        let table = self
            .backend
            .table_by_id(context, id)
            .ok_or_else(|| ScheduleError {
                state,
                error: self
                    .backend
                    .error(format!("Table with ID '{}' not found", id)),
            })?;
        fetchTablePartitionScheduleState(&self.backend, context, schedule_state, &table, state)
    }
}

/// 对给定 key range 查询 PD 复制状态；失败时包装为 `ScheduleError`。
pub fn fetchScheduleState<B: ShowPlacementBackend>(
    backend: &B,
    context: &B::Context,
    schedule_state: Option<&mut HashMap<i64, PlacementScheduleState>>,
    id: i64,
) -> Result<PlacementScheduleState, ScheduleError<B::Error>> {
    if let Some(state) = schedule_state
        .as_ref()
        .and_then(|schedule_state| schedule_state.get(&id))
    {
        return Ok(*state);
    }

    let (start_key, end_key) = backend.encoded_table_range(id);
    let schedule = backend
        .replication_state(context, &start_key, &end_key)
        .map_err(|error| ScheduleError {
            state: PlacementScheduleState::Pending,
            error,
        })?;
    if let Some(schedule_state) = schedule_state {
        schedule_state.insert(id, schedule);
    }
    Ok(schedule)
}

/// 查询单个分区的调度状态。
pub fn fetchPartitionScheduleState<B: ShowPlacementBackend>(
    backend: &B,
    context: &B::Context,
    schedule_state: Option<&mut HashMap<i64, PlacementScheduleState>>,
    partition: &PartitionDefinition,
) -> Result<PlacementScheduleState, ScheduleError<B::Error>> {
    fetchScheduleState(backend, context, schedule_state, partition.id)
}

/// 聚合表及其全部分区的调度状态，取最落后者。
pub fn fetchTablePartitionScheduleState<B: ShowPlacementBackend>(
    backend: &B,
    context: &B::Context,
    mut schedule_state: Option<&mut HashMap<i64, PlacementScheduleState>>,
    table: &TableInfo,
    mut state: PlacementScheduleState,
) -> Result<PlacementScheduleState, ScheduleError<B::Error>> {
    if let Some(partitions) = table.partitions.as_ref() {
        for partition in partitions {
            let schedule = fetchScheduleState(
                backend,
                context,
                schedule_state.as_deref_mut(),
                partition.id,
            )
            .map_err(|failure| ScheduleError {
                state: PlacementScheduleState::Pending,
                error: failure.error,
            })?;
            state = accumulateState(state, schedule);
            if state != PlacementScheduleState::Scheduled {
                break;
            }
        }
    }
    Ok(state)
}

/// 查询整表（非分区或表级）key range 的调度状态。
pub fn fetchTableScheduleState<B: ShowPlacementBackend>(
    backend: &B,
    context: &B::Context,
    mut schedule_state: Option<&mut HashMap<i64, PlacementScheduleState>>,
    table: &TableInfo,
) -> Result<PlacementScheduleState, ScheduleError<B::Error>> {
    let mut state = PlacementScheduleState::Scheduled;
    let schedule = fetchScheduleState(backend, context, schedule_state.as_deref_mut(), table.id)
        .map_err(|failure| ScheduleError {
            state,
            error: failure.error,
        })?;
    state = accumulateState(state, schedule);
    if state != PlacementScheduleState::Scheduled {
        return Ok(state);
    }
    fetchTablePartitionScheduleState(backend, context, schedule_state, table, state)
}

/// 合并两个调度状态：数值更小（更落后）的优先保留。
pub fn accumulateState(
    current: PlacementScheduleState,
    new: PlacementScheduleState,
) -> PlacementScheduleState {
    if (current as i32) > (new as i32) {
        new
    } else {
        current
    }
}

/// 格式化为 `schema.table` 标识串。
fn format_identifier(schema: &CiString, table: &CiString) -> String {
    format!(
        "`{}`.`{}`",
        schema.original.replace('`', "``"),
        table.original.replace('`', "``")
    )
}

/// encoding/hex.DecodeString returns the successfully decoded prefix together
/// with an error. SHOW PLACEMENT intentionally ignores that error.
/// 将十六进制字符串解码为字节；非法字符处停止（忽略后续错误）。
fn decode_hex_ignoring_error(value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let Some(high) = hex_digit(pair[0]) else {
            break;
        };
        let Some(low) = hex_digit(pair[1]) else {
            break;
        };
        decoded.push((high << 4) | low);
    }
    decoded
}

/// 将单个 ASCII 十六进制字符转为 0..=15。
fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
