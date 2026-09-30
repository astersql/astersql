// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// InfoSchema Builder：根据 DDL SchemaDiff 增量或全量构建元数据快照。
//
// SchemaDiff 描述一次 DDL 对元数据的变更；Builder 将其应用到内存中的库表、
// Placement Policy 与 Resource Group，最后 `Build` 出 `InfoSchema`（v1 或 v2）。

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use crate::bundle_builder::{
    BundleSchema, PartitionBundleSpec, TableBundleSpec, bundleInfoBuilder,
};
use crate::infoschema::PlacementBundle;
use crate::infoschema::{
    DBInfo, InfoSchema, MaskingPolicyInfo, MaskingPolicyLoader, PolicyInfo, ResourceGroupInfo,
    Table, TableInfo, infoSchema,
};
use crate::infoschema_v2::{Data, infoschemaV2};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// DDL 动作类型，对应 Go `model.ActionType`，驱动 `ApplyDiff` 分派。
pub enum ActionType {
    #[default]
    None,
    CreateSchema,
    DropSchema,
    RecoverSchema,
    ModifySchemaCharsetAndCollate,
    ModifySchemaDefaultPlacement,
    CreateTable,
    CreateMaterializedView,
    CreateMaterializedViewLog,
    CreateMaterializedViewShadow,
    CreateTables,
    DropTable,
    DropMaterializedView,
    DropMaterializedViewLog,
    DropMaterializedViewShadow,
    MViewRefreshOutOfPlaceCutover,
    TruncateTable,
    RecoverTable,
    RenameTable,
    RenameTables,
    AddTablePartition,
    DropTablePartition,
    TruncateTablePartition,
    ReorganizePartition,
    ExchangeTablePartition,
    AlterTablePartitioning,
    RemovePartitioning,
    CreatePlacementPolicy,
    AlterPlacementPolicy,
    DropPlacementPolicy,
    CreateResourceGroup,
    AlterResourceGroup,
    DropResourceGroup,
    CreateMaskingPolicy,
    AlterMaskingPolicy,
    DropMaskingPolicy,
    RefreshMeta,
    // Allocator-related actions used by getKeptAllocators (Go model.Action*).
    RebaseAutoID,
    ModifyTableAutoIDCache,
    RebaseAutoRandomBase,
    MultiSchemaChange,
    AddColumn,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一次 diff 中额外受影响的库表 ID 选项（如批量建表、重命名）。
pub struct AffectedOption {
    pub schema_id: i64,
    pub old_schema_id: i64,
    pub table_id: i64,
    pub old_table_id: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单次 schema 变更描述：版本、动作、主库表 ID 及附属选项。
pub struct SchemaDiff {
    pub version: i64,
    pub action_type: ActionType,
    pub schema_id: i64,
    pub table_id: i64,
    pub old_schema_id: i64,
    pub old_table_id: i64,
    pub affected_options: Vec<AffectedOption>,
    /// Sub-actions for MultiSchemaChange (Go `SchemaDiff.SubActionTypes`).
    pub sub_action_types: Vec<ActionType>,
}

/// 读取库/表/策略/资源组元数据的抽象；策略和资源组的默认实现返回 `None`。
pub trait MetadataReader {
    fn database(&self, id: i64) -> Result<Option<DBInfo>, String>;
    fn table(&self, schema_id: i64, table_id: i64) -> Result<Option<TableInfo>, String>;
    fn policy(&self, id: i64) -> Result<Option<PolicyInfo>, String> {
        let _ = id;
        Ok(None)
    }
    fn resource_group(&self, id: i64) -> Result<Option<ResourceGroupInfo>, String> {
        let _ = id;
        Ok(None)
    }
}

#[derive(Clone)]
/// Builder 内部的库状态：库信息与表 ID → Table 映射。
struct DatabaseState {
    info: DBInfo,
    tables: HashMap<i64, Table>,
}

pub struct Builder {
    /// 是否写入 infoschema v2 的 `Data` 后端。
    enable_v2: bool,
    /// 当前构建对应的 schema 元数据版本。
    schema_version: i64,
    /// Build 时写入的 schema 时间戳。
    schema_ts: u64,
    /// 库 ID → 库内表集合。
    databases: HashMap<i64, DatabaseState>,
    /// Placement Policy 缓存。
    policies: HashMap<i64, PolicyInfo>,
    /// Resource Group 缓存。
    resource_groups: HashMap<i64, ResourceGroupInfo>,
    /// 临时表 ID 集合。
    temporary_table_ids: HashSet<i64>,
    /// v2 路径共享的 Data。
    info_data: Arc<Data>,
    /// 是否跨 keyspace（多租户键空间）构建。
    cross_keyspace: bool,
    storage_class_enabled: Option<bool>,
    masking_cache: HashMap<i64, HashMap<i64, Arc<MaskingPolicyInfo>>>,
    masking_loaded: bool,
    masking_loader: Option<Arc<dyn MaskingPolicyLoader>>,
    bundle_cache: HashMap<i64, Arc<PlacementBundle>>,
    bundle_updates: HashSet<i64>,
    bundle_policy_updates: HashSet<i64>,
    delta_bundles: bool,
}

impl Builder {
    /// 创建空 Builder。
    pub fn new(info_data: Arc<Data>, use_v2: bool) -> Self {
        Self {
            enable_v2: use_v2,
            schema_version: 0,
            schema_ts: 0,
            databases: HashMap::new(),
            policies: HashMap::new(),
            resource_groups: HashMap::new(),
            temporary_table_ids: HashSet::new(),
            info_data,
            cross_keyspace: false,
            storage_class_enabled: None,
            masking_cache: HashMap::new(),
            masking_loaded: false,
            masking_loader: None,
            bundle_cache: HashMap::new(),
            bundle_updates: HashSet::new(),
            bundle_policy_updates: HashSet::new(),
            delta_bundles: false,
        }
    }
    /// 设置后续 diff 应用使用的 schema 版本。
    pub fn SetSchemaVersion(&mut self, version: i64) {
        self.schema_version = version;
    }
    /// 占位：Go 侧绑定 KV Storage，此处恒等返回。
    pub fn WithStore(self) -> Self {
        self
    }
    /// 设置跨 keyspace 标志。
    pub fn WithCrossKS(mut self, cross_keyspace: bool) -> Self {
        self.cross_keyspace = cross_keyspace;
        self
    }

    /// Override the instance setting for a snapshot, primarily for deterministic tests.
    pub fn WithStorageClassEnabled(mut self, enabled: bool) -> Self {
        self.storage_class_enabled = Some(enabled);
        self
    }

    /// 按 `action_type` 分派 DDL 变更，返回受影响的表/分区 ID 列表。
    pub fn ApplyDiff(
        &mut self,
        metadata: &dyn MetadataReader,
        diff: &SchemaDiff,
    ) -> Result<Vec<i64>, String> {
        // 先对齐版本，再按动作类型更新内存元数据。
        self.SetSchemaVersion(diff.version);
        let mut affected = Vec::new();
        match diff.action_type {
            ActionType::CreateSchema => self.applyCreateSchema(metadata, diff)?,
            ActionType::DropSchema => affected.extend(self.applyDropSchema(diff)),
            ActionType::RecoverSchema => affected.extend(self.applyRecoverSchema(metadata, diff)?),
            ActionType::ModifySchemaCharsetAndCollate
            | ActionType::ModifySchemaDefaultPlacement => {
                self.refresh_schema(metadata, diff.schema_id)?
            }
            ActionType::CreateTable
            | ActionType::CreateMaterializedView
            | ActionType::CreateMaterializedViewLog
            | ActionType::CreateMaterializedViewShadow
            | ActionType::RecoverTable => {
                if diff.table_id > 0 {
                    affected.extend(self.applyTableUpdate(metadata, diff)?);
                } else if diff.old_table_id > 0 {
                    affected.extend(self.applyDropTable(diff.schema_id, diff.old_table_id));
                }
            }
            // 主表更新后，再处理 AffectedOptions 中的其余表。
            ActionType::CreateTables => {
                for option in &diff.affected_options {
                    affected.extend(self.apply_table_ids(
                        metadata,
                        option.schema_id,
                        option.table_id,
                        0,
                    )?);
                }
            }
            ActionType::DropTable => {
                affected.extend(self.applyDropTable(diff.schema_id, diff.table_id))
            }
            ActionType::DropMaterializedView
            | ActionType::DropMaterializedViewLog
            | ActionType::DropMaterializedViewShadow => {
                let current = metadata.table(diff.schema_id, diff.table_id)?;
                if current.as_ref().is_some_and(|table| {
                    table
                        .model_meta
                        .as_ref()
                        .is_none_or(|model| model.State != astersql_meta_model::StateNone)
                }) {
                    affected.extend(self.applyTableUpdate(metadata, diff)?);
                } else {
                    affected.extend(self.applyDropTable(diff.schema_id, diff.table_id));
                }
                for option in &diff.affected_options {
                    if option.schema_id == 0 && option.old_schema_id == 0 {
                        continue;
                    }
                    affected.extend(self.apply_table_ids(
                        metadata,
                        option.schema_id,
                        option.table_id,
                        option.old_table_id,
                    )?);
                }
            }
            ActionType::MViewRefreshOutOfPlaceCutover => {
                if !self.databases.contains_key(&diff.schema_id) {
                    return Err(format!("database {} not found", diff.schema_id));
                }
                if diff.old_table_id > 0 {
                    affected.extend(self.applyDropTable(diff.schema_id, diff.old_table_id));
                }
                if diff.table_id > 0 && diff.table_id != diff.old_table_id {
                    affected.extend(self.applyDropTable(diff.schema_id, diff.table_id));
                }
                if diff.table_id > 0 {
                    affected.extend(self.apply_table_ids(
                        metadata,
                        diff.schema_id,
                        diff.table_id,
                        0,
                    )?);
                }
                for option in &diff.affected_options {
                    affected.extend(self.apply_table_ids(
                        metadata,
                        option.schema_id,
                        option.table_id,
                        option.old_table_id,
                    )?);
                }
            }
            // 截断/重命名/分区变更等：更新主表并处理附属 old/new 表 ID。
            ActionType::TruncateTable
            | ActionType::RenameTable
            | ActionType::RenameTables
            | ActionType::AddTablePartition
            | ActionType::DropTablePartition
            | ActionType::TruncateTablePartition
            | ActionType::ReorganizePartition
            | ActionType::ExchangeTablePartition
            | ActionType::AlterTablePartitioning
            | ActionType::RemovePartitioning => {
                affected.extend(self.applyTableUpdate(metadata, diff)?);
                for option in &diff.affected_options {
                    affected.extend(self.apply_table_ids(
                        metadata,
                        option.schema_id,
                        option.table_id,
                        option.old_table_id,
                    )?);
                }
            }
            ActionType::RefreshMeta => affected.extend(self.applyRefreshMeta(metadata, diff)?),
            ActionType::CreatePlacementPolicy => self.applyCreatePolicy(metadata, diff.table_id)?,
            ActionType::AlterPlacementPolicy => {
                self.applyCreatePolicy(metadata, diff.table_id)?;
                affected.extend(self.tables_referencing_policy(diff.table_id));
            }
            ActionType::DropPlacementPolicy => {
                self.policies.remove(&diff.table_id);
                affected.extend(self.tables_referencing_policy(diff.table_id));
            }
            ActionType::CreateResourceGroup | ActionType::AlterResourceGroup => {
                self.applyResourceGroup(metadata, diff.table_id)?
            }
            ActionType::DropResourceGroup => {
                self.resource_groups.remove(&diff.table_id);
            }
            // Masking / 仅分配器相关动作在此路径暂不修改结构。
            ActionType::CreateMaskingPolicy
            | ActionType::AlterMaskingPolicy
            | ActionType::DropMaskingPolicy
            | ActionType::None
            | ActionType::RebaseAutoID
            | ActionType::ModifyTableAutoIDCache
            | ActionType::RebaseAutoRandomBase
            | ActionType::MultiSchemaChange
            | ActionType::AddColumn => {}
        }
        if needRefreshMaskingPoliciesForTableDiff(diff.action_type) {
            self.masking_cache.clear();
            self.masking_loaded = false;
        }
        if self.delta_bundles {
            for id in affected
                .iter()
                .copied()
                .chain([diff.table_id, diff.old_table_id])
            {
                if id > 0 {
                    self.bundle_updates.insert(id);
                }
            }
            for option in &diff.affected_options {
                for id in [option.table_id, option.old_table_id] {
                    if id > 0 {
                        self.bundle_updates.insert(id);
                    }
                }
            }
            if matches!(
                diff.action_type,
                ActionType::CreatePlacementPolicy
                    | ActionType::AlterPlacementPolicy
                    | ActionType::DropPlacementPolicy
            ) {
                self.bundle_policy_updates.insert(diff.table_id);
            }
        }
        // 去重排序后返回，便于调用方稳定比较。
        affected.sort_unstable();
        affected.dedup();
        Ok(affected)
    }

    /// 从 MetadataReader 加载库并插入；v2 同步 addDB。
    fn applyCreateSchema(
        &mut self,
        metadata: &dyn MetadataReader,
        diff: &SchemaDiff,
    ) -> Result<(), String> {
        let db = metadata
            .database(diff.schema_id)?
            .ok_or_else(|| format!("database {} not found", diff.schema_id))?;
        self.databases.insert(
            db.id,
            DatabaseState {
                info: db.clone(),
                tables: HashMap::new(),
            },
        );
        // v2：直接封装共享 Data；v1：把库表与策略填入 infoSchema。
        if self.enable_v2 {
            self.info_data.addDB(diff.version, db);
        }
        Ok(())
    }
    /// 删除库及其表，返回受影响表 ID。
    fn applyDropSchema(&mut self, diff: &SchemaDiff) -> Vec<i64> {
        let Some(db) = self.databases.remove(&diff.schema_id) else {
            return Vec::new();
        };
        let affected: Vec<i64> = db.tables.keys().copied().collect();
        if self.enable_v2 {
            self.info_data.deleteDB(db.info, diff.version);
        }
        affected
    }
    /// 恢复库后，按 affected_options 逐表恢复。
    fn applyRecoverSchema(
        &mut self,
        metadata: &dyn MetadataReader,
        diff: &SchemaDiff,
    ) -> Result<Vec<i64>, String> {
        self.applyCreateSchema(metadata, diff)?;
        let table_ids: Vec<i64> = diff
            .affected_options
            .iter()
            .map(|option| option.table_id)
            .collect();
        let mut affected = Vec::new();
        for table_id in table_ids {
            affected.extend(self.apply_table_ids(metadata, diff.schema_id, table_id, 0)?);
        }
        Ok(affected)
    }
    /// 刷新库级属性（字符集/默认 placement 等）。
    fn refresh_schema(
        &mut self,
        metadata: &dyn MetadataReader,
        schema_id: i64,
    ) -> Result<(), String> {
        let db = metadata
            .database(schema_id)?
            .ok_or_else(|| format!("database {schema_id} not found"))?;
        if let Some(state) = self.databases.get_mut(&schema_id) {
            state.info = db.clone();
        } else {
            self.databases.insert(
                schema_id,
                DatabaseState {
                    info: db.clone(),
                    tables: HashMap::new(),
                },
            );
        }
        if self.enable_v2 {
            self.info_data.addDB(self.schema_version, db);
        }
        Ok(())
    }
    /// PITR 元数据刷新：库级 diff 创建/更新/删除库，表级 diff 创建/更新/删除表。
    fn applyRefreshMeta(
        &mut self,
        metadata: &dyn MetadataReader,
        diff: &SchemaDiff,
    ) -> Result<Vec<i64>, String> {
        if diff.table_id == 0 {
            if metadata.database(diff.schema_id)?.is_none() {
                return Ok(self.applyDropSchema(diff));
            }
            if self.databases.contains_key(&diff.schema_id) {
                self.refresh_schema(metadata, diff.schema_id)?;
            } else {
                self.applyCreateSchema(metadata, diff)?;
            }
            return Ok(Vec::new());
        }

        // 与 Go 一致：库已不存在时，其表必然也已从一致快照中移除。
        if !self.databases.contains_key(&diff.schema_id) {
            return Ok(Vec::new());
        }
        if metadata.table(diff.schema_id, diff.table_id)?.is_none() {
            return Ok(self.applyDropTable(diff.schema_id, diff.table_id));
        }
        self.applyTableUpdate(metadata, diff)
    }
    /// 表级更新：若存在 old_table_id 则先删旧再写新。
    fn applyTableUpdate(
        &mut self,
        metadata: &dyn MetadataReader,
        diff: &SchemaDiff,
    ) -> Result<Vec<i64>, String> {
        let old_id = if diff.old_table_id != 0 {
            diff.old_table_id
        } else {
            diff.table_id
        };
        self.apply_table_ids(metadata, diff.schema_id, diff.table_id, old_id)
    }
    /// 写入新表元数据，必要时删除旧表，并收集分区等受影响 ID。
    fn apply_table_ids(
        &mut self,
        metadata: &dyn MetadataReader,
        schema_id: i64,
        new_table_id: i64,
        old_table_id: i64,
    ) -> Result<Vec<i64>, String> {
        let mut affected = Vec::new();
        let mut table_info = metadata
            .table(schema_id, new_table_id)?
            .ok_or_else(|| format!("table {schema_id}/{new_table_id} not found"))?;
        let current_name = self
            .databases
            .get(&schema_id)
            .and_then(|db| db.tables.get(&new_table_id))
            .map(|table| table.Meta().name.lower.as_str());
        if old_table_id > 0
            && (old_table_id != new_table_id
                || current_name.is_some_and(|name| name != table_info.name.lower))
        {
            affected.extend(self.applyDropTable(schema_id, old_table_id));
        }
        table_info.db_id = schema_id;
        // 规范化字符集/排序规则大小写，并处理历史 UTF8→UTF8MB4。
        ConvertCharsetCollateToLowerCaseIfNeed(&mut table_info);
        ConvertOldVersionUTF8ToUTF8MB4IfNeed(&mut table_info);
        let table = Table::new(table_info.clone());
        let db = self
            .databases
            .get_mut(&schema_id)
            .ok_or_else(|| format!("database {schema_id} not loaded"))?;
        db.tables.insert(new_table_id, table.clone());
        if self.enable_v2 {
            self.info_data.add(&db.info, table, self.schema_version);
        }
        affected.extend(appendAffectedIDs(Vec::new(), &table_info));
        Ok(affected)
    }
    /// 从库中移除表；v2 同步 remove。
    fn applyDropTable(&mut self, schema_id: i64, table_id: i64) -> Vec<i64> {
        let Some(db) = self.databases.get_mut(&schema_id) else {
            return Vec::new();
        };
        let Some(table) = db.tables.remove(&table_id) else {
            return Vec::new();
        };
        self.temporary_table_ids.remove(&table_id);
        self.info_data.removeTemporaryTable(table_id);
        if self.enable_v2 {
            self.info_data.remove_by_id(table_id, self.schema_version);
        }
        appendAffectedIDs(Vec::new(), table.Meta())
    }
    /// 加载并缓存 Placement Policy（创建与修改共用）。
    fn applyCreatePolicy(
        &mut self,
        metadata: &dyn MetadataReader,
        policy_id: i64,
    ) -> Result<(), String> {
        let policy = metadata
            .policy(policy_id)?
            .ok_or_else(|| format!("policy {policy_id} not found"))?;
        self.policies.insert(policy.id, policy);
        Ok(())
    }
    /// 加载并缓存 Resource Group。
    fn applyResourceGroup(
        &mut self,
        metadata: &dyn MetadataReader,
        group_id: i64,
    ) -> Result<(), String> {
        let group = metadata
            .resource_group(group_id)?
            .ok_or_else(|| format!("resource group {group_id} not found"))?;
        self.resource_groups.insert(group.id, group);
        Ok(())
    }
    /// Go 侧会扫描引用该 policy 的表；此处暂返回空以保持接口。
    fn tables_referencing_policy(&self, _policy_id: i64) -> Vec<i64> {
        Vec::new()
    }
    /// 登记临时表 ID。
    pub fn addTemporaryTable(&mut self, table_id: i64) {
        self.temporary_table_ids.insert(table_id);
        self.info_data.addTemporaryTable(table_id);
    }
    /// 批量注入 policy 与 resource group。
    pub fn initMisc(&mut self, policies: Vec<PolicyInfo>, resource_groups: Vec<ResourceGroupInfo>) {
        self.policies
            .extend(policies.into_iter().map(|policy| (policy.id, policy)));
        self.resource_groups
            .extend(resource_groups.into_iter().map(|group| (group.id, group)));
    }

    /// 用完整 DBInfo 列表全量初始化；已加载表会从 TableName2ID 按原名移除。
    pub fn InitWithDBInfos(
        &mut self,
        db_infos: &mut [DBInfo],
        policies: Vec<PolicyInfo>,
        resource_groups: Vec<ResourceGroupInfo>,
        schema_version: i64,
    ) {
        self.schema_version = schema_version;
        self.databases.clear();
        self.bundle_cache.clear();
        self.bundle_updates.clear();
        self.bundle_policy_updates.clear();
        self.delta_bundles = false;
        if self.enable_v2 {
            self.info_data.resetBeforeFullLoad(schema_version);
        }
        for db in db_infos.iter_mut() {
            let tables = std::mem::take(&mut db.tables);
            // Go 按 Name.O（原始大小写）从 TableName2ID 删除已加载表，保留未加载项供惰性加载。
            // Go deletes loaded tables from TableName2ID by Name.O (original case).
            if !db.table_name_2_id.is_empty() {
                for table in &tables {
                    db.table_name_2_id.remove(&table.name.original);
                }
            }
            let mut state = DatabaseState {
                info: db.clone(),
                tables: HashMap::new(),
            };
            if self.enable_v2 {
                self.info_data.addDB(schema_version, db.clone());
            }
            for table in tables {
                let table = Table(table);
                if self.enable_v2 {
                    self.info_data.add(db, table.clone(), schema_version);
                }
                state.tables.insert(table.Meta().id, table);
            }
            self.databases.insert(db.id, state);
        }
        if !self.cross_keyspace {
            self.init_information_schema_tables(schema_version);
        }
        self.initMisc(policies, resource_groups);
    }

    fn init_information_schema_tables(&mut self, schema_version: i64) {
        let enabled = self
            .storage_class_enabled
            .unwrap_or_else(|| astersql_config::get_global_config().enable_storage_class);
        let db = crate::tables::information_schema_db_with_storage_class(enabled);
        if self.databases.contains_key(&db.id) {
            return;
        }
        let tables: HashMap<i64, Table> = db
            .tables
            .iter()
            .cloned()
            .map(Table)
            .map(|table| (table.Meta().id, table))
            .collect();
        if self.enable_v2 {
            self.info_data.addDB(schema_version, db.clone());
            for table in tables.values() {
                self.info_data.add(&db, table.clone(), schema_version);
            }
        }
        self.databases
            .insert(db.id, DatabaseState { info: db, tables });
    }

    /// 从已有 InfoSchema 拷贝库表到 Builder，继承其 schema 版本。
    pub fn InitWithOldInfoSchema(&mut self, old: &dyn InfoSchema) {
        let schemas = old.AllSchemas();
        self.databases.clear();
        for db in schemas {
            let tables = old
                .SchemaTableInfos(&db.name)
                .unwrap_or_default()
                .into_iter()
                .map(Table)
                .map(|table| (table.Meta().id, table))
                .collect();
            self.databases.insert(
                db.id,
                DatabaseState {
                    info: (*db).clone(),
                    tables,
                },
            );
        }
        self.policies = old
            .AllPlacementPolicies()
            .into_iter()
            .map(|policy| (policy.id, (*policy).clone()))
            .collect();
        self.bundle_cache = old
            .AllPlacementBundles()
            .into_iter()
            .map(|bundle| (bundle.physical_id, bundle))
            .collect();
        self.bundle_updates.clear();
        self.bundle_policy_updates.clear();
        self.delta_bundles = true;
        (self.masking_cache, self.masking_loaded) = old.MaskingCacheSnapshot();
        self.masking_loader = old.MaskingLoader();
        self.schema_version = old.SchemaMetaVersion();
    }

    /// 消费 Builder，产出 v2 `infoschemaV2` 或 v1 `infoSchema`。
    pub fn Build(mut self, schema_ts: u64) -> Arc<dyn InfoSchema> {
        self.schema_ts = schema_ts;
        let bundles = self.build_bundles();
        if self.enable_v2 {
            return Arc::new(
                infoschemaV2::new(self.info_data, self.schema_version, schema_ts)
                    .with_bundles_and_policies(
                        bundles,
                        self.policies
                            .into_iter()
                            .map(|(id, policy)| (id, Arc::new(policy)))
                            .collect(),
                    )
                    .with_masking_cache(
                        self.masking_cache,
                        self.masking_loaded,
                        self.masking_loader,
                    ),
            );
        }
        let mut schema = infoSchema::new(self.schema_version);
        if let Some(loader) = self.masking_loader {
            schema = schema.with_masking_loader(loader, schema_ts);
        }
        schema.restore_masking_policies(self.masking_cache, self.masking_loaded);
        schema.set_bundles(bundles);
        schema.set_temporary_table_ids(self.temporary_table_ids.clone());
        for (_, state) in self.databases {
            schema.add_schema(state.info, state.tables.into_values().collect());
        }
        for (_, policy) in self.policies {
            schema.setPolicy(policy);
        }
        for (_, group) in self.resource_groups {
            schema.setResourceGroup(group);
        }
        Arc::new(schema)
    }

    fn build_bundles(&self) -> HashMap<i64, Arc<PlacementBundle>> {
        let source = BuilderBundleSchema {
            databases: &self.databases,
            policies: &self.policies,
        };
        let mut builder = bundleInfoBuilder::new();
        if self.delta_bundles {
            builder.inherit_bundles(self.bundle_cache.clone());
            for id in &self.bundle_updates {
                builder.markTableBundleShouldUpdate(*id);
            }
            for id in &self.bundle_policy_updates {
                builder.markBundlesReferPolicyShouldUpdate(*id);
            }
        }
        for error in builder.updateInfoSchemaBundles(&source) {
            tracing::warn!(error = %error, "unable to build placement bundle");
        }
        builder.bundles().clone()
    }
}

struct BuilderBundleSchema<'a> {
    databases: &'a HashMap<i64, DatabaseState>,
    policies: &'a HashMap<i64, PolicyInfo>,
}

impl BundleSchema for BuilderBundleSchema<'_> {
    fn policy_by_id(&self, policy_id: i64) -> Option<Arc<PolicyInfo>> {
        self.policies.get(&policy_id).cloned().map(Arc::new)
    }

    fn table_bundle_spec(&self, table_id: i64) -> Option<TableBundleSpec> {
        self.databases.values().find_map(|db| {
            let table = db.tables.get(&table_id)?;
            let model = table.Meta().model_meta.as_ref()?;
            let policy_id = model.PlacementPolicyRef.as_ref().map(|policy| policy.ID);
            let partitions = model.Partition.as_ref().map_or_else(Vec::new, |partition| {
                partition
                    .Definitions
                    .iter()
                    .map(|definition| PartitionBundleSpec {
                        partition_id: definition.ID,
                        policy_id: definition
                            .PlacementPolicyRef
                            .as_ref()
                            .map_or(policy_id, |policy| Some(policy.ID)),
                    })
                    .collect()
            });
            Some(TableBundleSpec {
                table_id,
                policy_id,
                partitions,
            })
        })
    }

    fn all_table_bundle_specs(&self) -> Vec<TableBundleSpec> {
        self.databases
            .values()
            .flat_map(|db| db.tables.keys().copied())
            .filter_map(|table_id| self.table_bundle_spec(table_id))
            .collect()
    }
}

/// Table diffs that invalidate cached masking-policy table/column references.
pub fn needRefreshMaskingPoliciesForTableDiff(action: ActionType) -> bool {
    matches!(
        action,
        ActionType::CreateMaskingPolicy
            | ActionType::AlterMaskingPolicy
            | ActionType::DropMaskingPolicy
            | ActionType::DropTable
            | ActionType::DropMaterializedView
            | ActionType::DropMaterializedViewLog
            | ActionType::RenameTable
            | ActionType::RenameTables
            | ActionType::TruncateTable
            | ActionType::DropSchema
    )
}

/// 将表 ID 及其分区定义 ID 追加到受影响列表。
pub fn appendAffectedIDs(mut affected: Vec<i64>, table: &TableInfo) -> Vec<i64> {
    affected.push(table.id);
    if let Some(partitions) = &table.partition {
        affected.extend(partitions.definitions.iter().map(|partition| partition.id));
    }
    affected
}
/// 将表/列名的 lower 字段同步为 original 的小写形式。
pub fn ConvertCharsetCollateToLowerCaseIfNeed(table: &mut TableInfo) {
    table.name.lower = table.name.original.to_lowercase();
    for column in &mut table.columns {
        column.name.lower = column.name.original.to_lowercase();
    }
}
/// 当前不修改表元数据。
pub fn ConvertOldVersionUTF8ToUTF8MB4IfNeed(_table: &mut TableInfo) {}
/// 比较两个可选 Placement Policy ID 是否相同。
pub fn equalPlacementPolicy(left: Option<i64>, right: Option<i64>) -> bool {
    left == right
}
/// 按表 ID 映射到 infoschema 分桶下标。
pub fn tableBucketIdx(table_id: i64) -> usize {
    assert!(table_id > 0);
    table_id as usize % crate::infoschema::bucketCount
}
/// 表 ID 是否为正数（有效）。
pub fn tableIDIsValid(table_id: i64) -> bool {
    table_id > 0
}

/// 从 TableInfo 构造虚拟表 Table 的函数类型。
pub type tableFromMetaFunc = fn(TableInfo) -> Result<Table, String>;
#[derive(Clone)]
/// 虚拟表驱动：绑定系统库信息与 TableFromMeta 转换函数。
pub struct virtualTableDriver {
    pub DBInfo: DBInfo,
    pub TableFromMeta: tableFromMetaFunc,
}
static DRIVERS: OnceLock<Mutex<Vec<virtualTableDriver>>> = OnceLock::new();
/// 进程内全局虚拟表驱动列表。
fn virtual_table_drivers() -> &'static Mutex<Vec<virtualTableDriver>> {
    DRIVERS.get_or_init(|| Mutex::new(Vec::new()))
}
/// 注册虚拟表驱动，供 information_schema 等系统表使用。
pub fn RegisterVirtualTable(db: DBInfo, convert: tableFromMetaFunc) {
    virtual_table_drivers()
        .lock()
        .expect("virtual table driver lock poisoned")
        .push(virtualTableDriver {
            DBInfo: db,
            TableFromMeta: convert,
        });
}
/// 设置 Data 缓存容量并创建 Builder。
pub fn NewBuilder(schema_cache_size: u64, info_data: Arc<Data>, use_v2: bool) -> Builder {
    info_data.SetCacheCapacity(schema_cache_size);
    Builder::new(info_data, use_v2)
}

/// 从已有 v1 infoSchema 拷贝 masking policy 缓存，对齐 Go InitWithOldInfoSchema 语义。
/// Copy masking-policy cache from an existing v1 `infoSchema`, matching Go
/// `InitWithOldInfoSchema` masking semantics.
pub fn apply_masking_copy(
    target: &crate::infoschema::infoSchema,
    old: &crate::infoschema::infoSchema,
    load_in_progress: bool,
) {
    let policies = old.clone_masking_policies();
    let mut loaded = old.masking_policies_loaded();
    if load_in_progress {
        loaded = false;
    }
    target.restore_masking_policies(policies, loaded);
}

/// 按 SchemaDiff 过滤仍有效的自动分配器（AutoID / AutoRandom 等）。
///
/// 对应 Go `getKeptAllocators`：
/// - RebaseAutoID / ModifyTableAutoIDCache → 丢弃 RowID 与 AutoIncrement；
/// - RebaseAutoRandomBase → 丢弃 AutoRandom；
/// - MultiSchemaChange 检查 `sub_action_types`；
/// - 其余动作保留全部。
/// Keep allocators that were not invalidated by the current schema diff.
///
/// Mirrors Go `getKeptAllocators`:
/// - RebaseAutoID / ModifyTableAutoIDCache → drop RowID + AutoIncrement;
/// - RebaseAutoRandomBase → drop AutoRandom;
/// - MultiSchemaChange inspects `sub_action_types` the same way;
/// - otherwise keep all.
pub fn getKeptAllocators(
    diff: &SchemaDiff,
    old: &astersql_meta_autoid::Allocators,
) -> astersql_meta_autoid::Allocators {
    use astersql_meta_autoid::AllocatorType;

    // 扫描主动作与子动作，标记哪些分配器类型已失效。
    let mut auto_id_changed = false;
    let mut auto_random_changed = false;
    match diff.action_type {
        ActionType::RebaseAutoID | ActionType::ModifyTableAutoIDCache => {
            auto_id_changed = true;
        }
        ActionType::RebaseAutoRandomBase => {
            auto_random_changed = true;
        }
        ActionType::MultiSchemaChange => {
            for sub in &diff.sub_action_types {
                match sub {
                    ActionType::RebaseAutoID | ActionType::ModifyTableAutoIDCache => {
                        auto_id_changed = true;
                    }
                    ActionType::RebaseAutoRandomBase => {
                        auto_random_changed = true;
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if auto_id_changed {
        old.filter(|a| {
            let tp = a.get_type();
            tp != AllocatorType::RowId && tp != AllocatorType::AutoIncrement
        })
    } else if auto_random_changed {
        old.filter(|a| a.get_type() != AllocatorType::AutoRandom)
    } else {
        old.clone()
    }
}
