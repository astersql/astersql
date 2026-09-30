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

// InfoSchema（信息模式）核心实现：内存中的库/表/策略元数据视图。
//
// 对应 Go `pkg/infoschema`：提供按名/ID 查找 schema 与表、放置策略、资源组、
// 脱敏策略（Masking Policy）、外键反向引用，以及会话级临时表。

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, RwLock};

use crate::error::ErrTableNotExists;
use astersql_infoschema_context as context_dependency;
use astersql_meta_model as model_dependency;

/// 按表 ID 分桶存放排序表列表时的桶数量。
pub const bucketCount: usize = 512;

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
/// 大小写不敏感字符串：保留原文与小写形式，用于库表名比较。
pub struct CiString {
    pub original: String,
    pub lower: String,
}

impl CiString {
    /// 由任意可转 String 的值构造，同时缓存小写形式。
    pub fn new(value: impl Into<String>) -> Self {
        let original = value.into();
        let lower = original.to_lowercase();
        Self { original, lower }
    }
}

impl From<&str> for CiString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 列的精简缓存索引：ID、名称、是否自增。
pub struct ColumnInfo {
    pub id: i64,
    pub name: CiString,
    pub auto_increment: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 索引的精简缓存索引：ID 与名称。
pub struct IndexInfo {
    pub id: i64,
    pub name: CiString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 分区定义的精简表示：物理分区 ID 与名称。
pub struct PartitionDefinition {
    pub id: i64,
    pub name: CiString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表的分区信息：分区定义列表。
pub struct PartitionInfo {
    pub definitions: Vec<PartitionDefinition>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 外键：名称及引用的父库/父表。
pub struct ForeignKeyInfo {
    pub name: CiString,
    pub ref_schema: CiString,
    pub ref_table: CiString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 被引用外键：记录子表侧的 schema/表/外键名（反向索引）。
pub struct ReferredFKInfo {
    pub child_schema: CiString,
    pub child_table: CiString,
    pub child_fk_name: CiString,
}

#[derive(Clone, Debug, Default)]
/// 表元数据的缓存索引视图；完整 Go 模型可选放在 `model_meta`。
pub struct TableInfo {
    pub id: i64,
    pub db_id: i64,
    pub name: CiString,
    pub columns: Vec<ColumnInfo>,
    pub indices: Vec<IndexInfo>,
    pub partition: Option<PartitionInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    pub is_view: bool,
    pub is_sequence: bool,
    /// Complete Go table metadata retained for planner/executor consumers.
    ///
    /// The compact fields above are the cache index used by this migration,
    /// but they cannot represent column defaults, field types, FULLTEXT index
    // / state, or TiFlash replica metadata. Keeping the canonical model avoids
    /// lossy reconstruction at InfoSchema call boundaries.
    pub model_meta: Option<Arc<model_dependency::TableInfo>>,
}

impl PartialEq for TableInfo {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.db_id == other.db_id
            && self.name == other.name
            && self.columns == other.columns
            && self.indices == other.indices
            && self.partition == other.partition
            && self.foreign_keys == other.foreign_keys
            && self.is_view == other.is_view
            && self.is_sequence == other.is_sequence
    }
}

impl Eq for TableInfo {}

#[derive(Clone, Debug)]
/// 共享所有权的表句柄（`Arc<TableInfo>`）。
pub struct Table(pub Arc<TableInfo>);

impl Table {
    /// 包装一张表的元数据。
    pub fn new(info: TableInfo) -> Self {
        Self(Arc::new(info))
    }
    /// 返回内部 `TableInfo` 引用。
    pub fn Meta(&self) -> &TableInfo {
        &self.0
    }

    /// 从完整 meta_model::TableInfo 投影出缓存索引字段，并保留完整模型。
    pub fn from_model(info: model_dependency::TableInfo) -> Self {
        let info = Arc::new(info);
        let auto_increment_id = info.GetAutoIncrementColInfo().map(|column| column.ID);
        let partition = info.Partition.as_ref().map(|partition| PartitionInfo {
            definitions: partition
                .Definitions
                .iter()
                .map(|definition| PartitionDefinition {
                    id: definition.ID,
                    name: CiString::new(definition.Name.O.clone()),
                })
                .collect(),
        });
        Self::new(TableInfo {
            id: info.ID,
            db_id: info.DBID,
            name: CiString::new(info.Name.O.clone()),
            columns: info
                .Columns
                .iter()
                .map(|column| ColumnInfo {
                    id: column.ID,
                    name: CiString::new(column.Name.O.clone()),
                    auto_increment: auto_increment_id == Some(column.ID),
                })
                .collect(),
            indices: info
                .Indices
                .iter()
                .map(|index| IndexInfo {
                    id: index.ID,
                    name: CiString::new(index.Name.O.clone()),
                })
                .collect(),
            partition,
            foreign_keys: info
                .ForeignKeys
                .iter()
                .map(|foreign_key| ForeignKeyInfo {
                    name: CiString::new(foreign_key.Name.O.clone()),
                    ref_schema: CiString::new(foreign_key.RefSchema.O.clone()),
                    ref_table: CiString::new(foreign_key.RefTable.O.clone()),
                })
                .collect(),
            is_view: info.View.is_some(),
            is_sequence: info.Sequence.is_some(),
            model_meta: Some(info),
        })
    }

    /// 取出完整表元数据；缺失时返回 `ErrTableMetadataUnavailable`。
    pub fn ModelMeta(&self) -> Result<Arc<model_dependency::TableInfo>, InfoSchemaError> {
        self.0.model_meta.clone().ok_or_else(|| InfoSchemaError {
            code: "ErrTableMetadataUnavailable",
            message: format!(
                "complete table metadata is unavailable for {}",
                self.0.name.original
            ),
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 数据库（schema）元数据：ID、名称、表列表与名→ID 映射。
pub struct DBInfo {
    pub id: i64,
    pub name: CiString,
    pub tables: Vec<Arc<TableInfo>>,
    /// Original-case table name → id map used by schema-cache lazy loading.
    /// Loaded tables are removed (by `Name.O`) during `InitWithDBInfos`.
    pub table_name_2_id: HashMap<String, i64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 放置策略（Placement Policy）精简信息。
pub struct PolicyInfo {
    pub id: i64,
    pub name: CiString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 资源组精简信息。
pub struct ResourceGroupInfo {
    pub id: i64,
    pub name: CiString,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 物理表对应的 placement rule bundle。
pub struct PlacementBundle {
    pub physical_id: i64,
    pub rules: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 脱敏策略启用状态。
pub enum MaskingPolicyStatus {
    #[default]
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 脱敏策略类型；值域与 Go `model.MaskingPolicyType*` 持久化契约一致。
pub enum MaskingPolicyType {
    #[default]
    Full,
    Partial,
    Null,
    Date,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 脱敏策略限制操作位图。
pub struct MaskingPolicyRestrictOps(pub u8);

impl MaskingPolicyRestrictOps {
    pub const NONE: Self = Self(0);
    pub const INSERT_INTO_SELECT: Self = Self(1);
    pub const UPDATE_SELECT: Self = Self(2);
    pub const DELETE_SELECT: Self = Self(4);
    pub const CTAS: Self = Self(8);
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单条脱敏策略：绑定到表列，含表达式与限制操作。
pub struct MaskingPolicyInfo {
    pub id: i64,
    pub name: CiString,
    pub table_id: i64,
    pub column_id: i64,
    pub status: MaskingPolicyStatus,
    pub policy_type: MaskingPolicyType,
    pub restrict_ops: MaskingPolicyRestrictOps,
    pub expression: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// 小写 schema+table 名对，用作外键反向索引键。
pub struct SchemaAndTableName {
    pub schema: String,
    pub table: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表的轻量定位信息：所属库名与表名。
pub struct TableItem {
    pub DBName: CiString,
    pub TableName: CiString,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// InfoSchema 查询错误：错误码名 + 消息。
pub struct InfoSchemaError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for InfoSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for InfoSchemaError {}

impl InfoSchemaError {
    /// 转为共享错误类型，供跨 crate 传播。
    pub fn into_shared(self) -> astersql_util_dbterror::errors::SharedError {
        astersql_util_dbterror::errors::New(self.to_string())
    }
}

/// InfoSchema 对外查询接口：按名/ID 查库表、分区、特殊属性列表等。
pub trait InfoSchema: Send + Sync {
    fn SchemaMetaVersion(&self) -> i64;
    fn SchemaByName(&self, schema: &CiString) -> Option<Arc<DBInfo>>;
    fn SchemaByID(&self, id: i64) -> Option<Arc<DBInfo>>;
    fn TableByName(&self, schema: &CiString, table: &CiString) -> Result<Table, InfoSchemaError>;
    fn ModelTableInfoByName(
        &self,
        schema: &CiString,
        table: &CiString,
    ) -> Result<Arc<model_dependency::TableInfo>, InfoSchemaError> {
        self.TableByName(schema, table)?.ModelMeta()
    }
    fn TableByID(&self, id: i64) -> Option<Table>;
    fn TableItemByID(&self, id: i64) -> Option<TableItem>;
    /// Return all tables in a schema; a missing schema yields an empty list.
    fn SchemaTableInfos(&self, schema: &CiString) -> Result<Vec<Arc<TableInfo>>, InfoSchemaError>;
    fn SchemaSimpleTableInfos(
        &self,
        schema: &CiString,
    ) -> Result<Vec<Arc<model_dependency::TableNameInfo>>, InfoSchemaError> {
        self.SchemaTableInfos(schema).map(|tables| {
            tables
                .into_iter()
                .map(|table| {
                    Arc::new(model_dependency::TableNameInfo {
                        ID: table.id,
                        Name: astersql_parser_ast::NewCIStr(table.name.original.clone()),
                    })
                })
                .collect()
        })
    }
    /// Whether the snapshot contains a global temporary table.
    fn HasTemporaryTable(&self) -> bool {
        false
    }
    fn FindTableByPartitionID(
        &self,
        partition_id: i64,
    ) -> Option<(Table, Arc<DBInfo>, PartitionDefinition)>;
    fn AllSchemas(&self) -> Vec<Arc<DBInfo>>;
    fn AllPlacementPolicies(&self) -> Vec<Arc<PolicyInfo>> {
        Vec::new()
    }
    fn PlacementBundleByPhysicalTableID(&self, _id: i64) -> Option<Arc<PlacementBundle>> {
        None
    }
    fn AllPlacementBundles(&self) -> Vec<Arc<PlacementBundle>> {
        Vec::new()
    }
    fn MaskingCacheSnapshot(&self) -> (HashMap<i64, HashMap<i64, Arc<MaskingPolicyInfo>>>, bool) {
        (HashMap::new(), false)
    }
    fn MaskingLoader(&self) -> Option<Arc<dyn MaskingPolicyLoader>> {
        None
    }
    fn ListTablesWithSpecialAttribute(
        &self,
        filter: context_dependency::SpecialAttributeFilter,
    ) -> Vec<context_dependency::TableInfoResult> {
        self.AllSchemas()
            .into_iter()
            .map(|schema| {
                let table_infos = schema
                    .tables
                    .iter()
                    .filter_map(|table| table.model_meta.clone())
                    .filter(|table| filter(table))
                    .collect();
                context_dependency::TableInfoResult {
                    DBName: astersql_parser_ast::NewCIStr(&schema.name.original),
                    TableInfos: table_infos,
                }
            })
            .collect()
    }
    fn IsV2(&self) -> bool {
        false
    }
    /// Compact table-history records older than `cut_version`.
    ///
    /// InfoSchema V1 has no shared version history, so the default returns
    /// `None`. InfoSchema V2 overrides this with its production data GC.
    fn GCOldVersion(&self, _cut_version: i64) -> Option<(usize, i64)> {
        None
    }
}

#[derive(Clone)]
/// 单个 schema 下的库信息与按小写表名索引的表映射。
struct schemaTables {
    db_info: Arc<DBInfo>,
    tables: HashMap<String, Table>,
}

/// InfoSchema V1 具体实现：分桶表索引、策略/资源组/脱敏缓存与临时表 ID 集合。
pub struct infoSchema {
    schema_meta_version: i64,
    schema_map: HashMap<String, schemaTables>,
    schema_id_to_name: HashMap<i64, String>,
    sorted_table_buckets: Vec<Vec<Table>>,
    referred_foreign_keys: HashMap<SchemaAndTableName, Vec<ReferredFKInfo>>,
    policies: RwLock<HashMap<String, Arc<PolicyInfo>>>,
    resource_groups: RwLock<HashMap<String, Arc<ResourceGroupInfo>>>,
    bundles: HashMap<i64, Arc<PlacementBundle>>,
    temporary_table_ids: HashSet<i64>,
    masking: RwLock<HashMap<i64, HashMap<i64, Arc<MaskingPolicyInfo>>>>,
    masking_loaded: Mutex<bool>,
    masking_loader: Option<Arc<dyn MaskingPolicyLoader>>,
    snapshot_ts: u64,
}

impl infoSchema {
    /// 创建指定 schema 元版本的空 InfoSchema。
    pub fn new(schema_meta_version: i64) -> Self {
        Self {
            schema_meta_version,
            schema_map: HashMap::new(),
            schema_id_to_name: HashMap::new(),
            sorted_table_buckets: (0..bucketCount).map(|_| Vec::new()).collect(),
            referred_foreign_keys: HashMap::new(),
            policies: RwLock::new(HashMap::new()),
            resource_groups: RwLock::new(HashMap::new()),
            bundles: HashMap::new(),
            temporary_table_ids: HashSet::new(),
            masking: RwLock::new(HashMap::new()),
            masking_loaded: Mutex::new(false),
            masking_loader: None,
            snapshot_ts: 0,
        }
    }
    pub fn set_bundles(&mut self, bundles: HashMap<i64, Arc<PlacementBundle>>) {
        self.bundles = bundles;
    }

    /// 挂载脱敏策略加载器与快照时间戳，供惰性加载使用。
    pub fn with_masking_loader(
        mut self,
        loader: Arc<dyn MaskingPolicyLoader>,
        snapshot_ts: u64,
    ) -> Self {
        self.masking_loader = Some(loader);
        self.snapshot_ts = snapshot_ts;
        self
    }

    /// 注册一个 schema 及其表：写入分桶、外键反向索引与名映射。
    pub fn add_schema(&mut self, mut db: DBInfo, tables: Vec<Table>) {
        let mut by_name = HashMap::new();
        db.tables = tables.iter().map(|table| table.0.clone()).collect();
        for table in &tables {
            by_name.insert(table.Meta().name.lower.clone(), table.clone());
            let bucket = tableBucketIdx(table.Meta().id);
            self.sorted_table_buckets[bucket].push(table.clone());
            self.addReferredForeignKeys(&db.name, table.Meta());
        }
        for bucket in &mut self.sorted_table_buckets {
            bucket.sort_by_key(|table| table.Meta().id);
        }
        let db = Arc::new(db);
        self.schema_id_to_name.insert(db.id, db.name.lower.clone());
        self.schema_map.insert(
            db.name.lower.clone(),
            schemaTables {
                db_info: db,
                tables: by_name,
            },
        );
    }

    /// 删除 schema；成功返回 true。
    pub fn del_schema(&mut self, schema: &CiString) -> bool {
        let Some(old) = self.schema_map.remove(&schema.lower) else {
            return false;
        };
        self.schema_id_to_name.remove(&old.db_info.id);
        let ids: HashSet<i64> = old.tables.values().map(|table| table.Meta().id).collect();
        for bucket in &mut self.sorted_table_buckets {
            bucket.retain(|table| !ids.contains(&table.Meta().id));
        }
        true
    }

    /// 判断 schema 是否存在。
    pub fn SchemaExists(&self, schema: &CiString) -> bool {
        self.schema_map.contains_key(&schema.lower)
    }

    /// 判断库表是否存在。
    pub fn TableExists(&self, schema: &CiString, table: &CiString) -> bool {
        self.schema_map
            .get(&schema.lower)
            .is_some_and(|db| db.tables.contains_key(&table.lower))
    }

    /// 按库表名取 `TableInfo`。
    pub fn TableInfoByName(
        &self,
        schema: &CiString,
        table: &CiString,
    ) -> Result<Arc<TableInfo>, InfoSchemaError> {
        self.TableByName(schema, table).map(|table| table.0)
    }

    /// 按表 ID 取 `TableInfo`。
    pub fn TableInfoByID(&self, id: i64) -> Option<Arc<TableInfo>> {
        self.TableByID(id).map(|table| table.0)
    }

    /// 按分区 ID 找回所属表/库/分区定义。
    pub fn FindTableInfoByPartitionID(
        &self,
        id: i64,
    ) -> Option<(Arc<TableInfo>, Arc<DBInfo>, PartitionDefinition)> {
        self.FindTableByPartitionID(id)
            .map(|(table, db, partition)| (table.0, db, partition))
    }

    /// 列出指定 schema 下全部表的 `TableInfo`。
    pub fn SchemaTableInfos(
        &self,
        schema: &CiString,
    ) -> Result<Vec<Arc<TableInfo>>, InfoSchemaError> {
        Ok(self
            .schema_map
            .get(&schema.lower)
            .map(|tables| tables.db_info.tables.clone())
            .unwrap_or_default())
    }

    /// 返回全部 schema 名。
    pub fn AllSchemaNames(&self) -> Vec<CiString> {
        self.AllSchemas()
            .into_iter()
            .map(|db| db.name.clone())
            .collect()
    }
    /// 是否登记了全局临时表。
    pub fn HasTemporaryTable(&self) -> bool {
        !self.temporary_table_ids.is_empty()
    }

    pub(crate) fn set_temporary_table_ids(&mut self, ids: HashSet<i64>) {
        self.temporary_table_ids = ids;
    }

    /// 按名查找放置策略。
    pub fn PolicyByName(&self, name: &CiString) -> Option<Arc<PolicyInfo>> {
        self.policies
            .read()
            .expect("policy lock poisoned")
            .get(&name.lower)
            .cloned()
    }
    /// 按 ID 查找放置策略。
    pub fn PolicyByID(&self, id: i64) -> Option<Arc<PolicyInfo>> {
        self.policies
            .read()
            .expect("policy lock poisoned")
            .values()
            .find(|policy| policy.id == id)
            .cloned()
    }
    /// 列出全部放置策略。
    pub fn AllPlacementPolicies(&self) -> Vec<Arc<PolicyInfo>> {
        self.policies
            .read()
            .expect("policy lock poisoned")
            .values()
            .cloned()
            .collect()
    }
    /// 写入/覆盖一条放置策略。
    pub fn setPolicy(&self, policy: PolicyInfo) {
        self.policies
            .write()
            .expect("policy lock poisoned")
            .insert(policy.name.lower.clone(), Arc::new(policy));
    }
    /// 按名删除放置策略。
    pub fn deletePolicy(&self, name: &str) {
        self.policies
            .write()
            .expect("policy lock poisoned")
            .remove(&name.to_lowercase());
    }

    /// 按名查找资源组。
    pub fn ResourceGroupByName(&self, name: &CiString) -> Option<Arc<ResourceGroupInfo>> {
        self.resource_groups
            .read()
            .expect("resource group lock poisoned")
            .get(&name.lower)
            .cloned()
    }
    /// 按 ID 查找资源组。
    pub fn ResourceGroupByID(&self, id: i64) -> Option<Arc<ResourceGroupInfo>> {
        self.resource_groups
            .read()
            .expect("resource group lock poisoned")
            .values()
            .find(|group| group.id == id)
            .cloned()
    }
    /// 列出全部资源组。
    pub fn AllResourceGroups(&self) -> Vec<Arc<ResourceGroupInfo>> {
        self.resource_groups
            .read()
            .expect("resource group lock poisoned")
            .values()
            .cloned()
            .collect()
    }
    /// 写入/覆盖一个资源组。
    pub fn setResourceGroup(&self, group: ResourceGroupInfo) {
        self.resource_groups
            .write()
            .expect("resource group lock poisoned")
            .insert(group.name.lower.clone(), Arc::new(group));
    }
    /// 按名删除资源组。
    pub fn deleteResourceGroup(&self, name: &str) {
        self.resource_groups
            .write()
            .expect("resource group lock poisoned")
            .remove(&name.to_lowercase());
    }

    /// 按物理表 ID 取 placement bundle。
    pub fn PlacementBundleByPhysicalTableID(&self, id: i64) -> Option<Arc<PlacementBundle>> {
        self.bundles.get(&id).cloned()
    }
    /// 列出全部 placement bundle。
    pub fn AllPlacementBundles(&self) -> Vec<Arc<PlacementBundle>> {
        self.bundles.values().cloned().collect()
    }

    /// 按表 ID + 列 ID 取脱敏策略（必要时触发惰性加载）。
    pub fn MaskingPolicyByTableColumn(
        &self,
        table_id: i64,
        column_id: i64,
    ) -> Option<Arc<MaskingPolicyInfo>> {
        self.loadMaskingPoliciesIfNeeded();
        self.masking
            .read()
            .expect("masking lock poisoned")
            .get(&table_id)?
            .get(&column_id)
            .cloned()
    }
    /// 按策略 ID 查找脱敏策略。
    pub fn MaskingPolicyByID(&self, id: i64) -> Option<Arc<MaskingPolicyInfo>> {
        self.loadMaskingPoliciesIfNeeded();
        self.masking
            .read()
            .expect("masking lock poisoned")
            .values()
            .flat_map(|columns| columns.values())
            .find(|policy| policy.id == id)
            .cloned()
    }
    /// 按名查找脱敏策略；重名（歧义）时返回 None。
    pub fn MaskingPolicyByName(&self, name: &CiString) -> Option<Arc<MaskingPolicyInfo>> {
        self.loadMaskingPoliciesIfNeeded();
        let mut found: Option<Arc<MaskingPolicyInfo>> = None;
        for policy in self
            .masking
            .read()
            .expect("masking lock poisoned")
            .values()
            .flat_map(|columns| columns.values())
        {
            if policy.name.lower == name.lower {
                if found.is_some() {
                    // Ambiguous name: Go returns (nil, false).
                    return None;
                }
                found = Some(policy.clone());
            }
        }
        found
    }

    /// Whether masking policies have been marked loaded (exported for same-crate tests).
    pub fn masking_policies_loaded(&self) -> bool {
        *self
            .masking_loaded
            .lock()
            .expect("masking state lock poisoned")
    }

    /// Inject a masking policy without going through the loader (test / builder copy path).
    pub fn put_masking_policy(&self, policy: MaskingPolicyInfo) {
        let table_id = policy.table_id;
        let column_id = policy.column_id;
        self.masking
            .write()
            .expect("masking lock poisoned")
            .entry(table_id)
            .or_default()
            .insert(column_id, Arc::new(policy));
    }

    /// 测试/迁移辅助：直接设置脱敏策略已加载标志。
    pub fn set_masking_policies_loaded(&self, loaded: bool) {
        *self
            .masking_loaded
            .lock()
            .expect("masking state lock poisoned") = loaded;
    }

    /// 克隆表→列→策略的脱敏缓存。
    pub fn clone_masking_policies(&self) -> HashMap<i64, HashMap<i64, Arc<MaskingPolicyInfo>>> {
        self.masking.read().expect("masking lock poisoned").clone()
    }

    /// 用给定映射整体替换脱敏缓存。
    pub fn restore_masking_policies(
        &self,
        policies: HashMap<i64, HashMap<i64, Arc<MaskingPolicyInfo>>>,
        loaded: bool,
    ) {
        *self.masking.write().expect("masking lock poisoned") = policies;
        self.set_masking_policies_loaded(loaded);
    }
    /// 列出全部已缓存脱敏策略。
    pub fn AllMaskingPolicies(&self) -> Vec<Arc<MaskingPolicyInfo>> {
        self.loadMaskingPoliciesIfNeeded();
        let mut policies: Vec<_> = self
            .masking
            .read()
            .expect("masking lock poisoned")
            .values()
            .flat_map(|columns| columns.values().cloned())
            .collect();
        policies.sort_by(|left, right| {
            left.name
                .lower
                .cmp(&right.name.lower)
                .then_with(|| left.id.cmp(&right.id))
        });
        policies
    }
    /// 清空脱敏缓存并重置 loaded 标志。
    pub fn resetMaskingPolicyCache(&self) {
        self.masking.write().expect("masking lock poisoned").clear();
        *self
            .masking_loaded
            .lock()
            .expect("masking state lock poisoned") = false;
    }
    /// 若尚未加载则调用 loader；表未就绪则置 loaded 且不再重试。
    fn loadMaskingPoliciesIfNeeded(&self) {
        let mut loaded = self
            .masking_loaded
            .lock()
            .expect("masking state lock poisoned");
        if *loaded {
            return;
        }
        let Some(loader) = &self.masking_loader else {
            // Go: factory == nil → mark loaded and skip.
            *loaded = true;
            return;
        };
        match loader.load(&[], self.snapshot_ts) {
            Ok(policies) => {
                let mut target = self.masking.write().expect("masking lock poisoned");
                for policy in policies {
                    target
                        .entry(policy.table_id)
                        .or_default()
                        .insert(policy.column_id, Arc::new(policy));
                }
                *loaded = true;
            }
            Err(err)
                if err.code == ErrTableNotExists.mysql_name || err.code == "ErrNoSuchTable" =>
            {
                // Table not ready: treat as loaded so we do not retry forever.
                // 对应 Go/生产辅助函数 isMaskingPolicyTableNotReady。
                *loaded = true;
            }
            Err(_) => {
                // Generic error: leave unloaded so the next access retries.
            }
        }
    }

    /// 将表的外键登记到父表的反向引用索引。
    fn addReferredForeignKeys(&mut self, schema: &CiString, table: &TableInfo) {
        for foreign_key in &table.foreign_keys {
            self.referred_foreign_keys
                .entry(SchemaAndTableName {
                    schema: foreign_key.ref_schema.lower.clone(),
                    table: foreign_key.ref_table.lower.clone(),
                })
                .or_default()
                .push(ReferredFKInfo {
                    child_schema: schema.clone(),
                    child_table: table.name.clone(),
                    child_fk_name: foreign_key.name.clone(),
                });
        }
    }
    /// 查询引用指定父表的全部子表外键。
    pub fn GetTableReferredForeignKeys(&self, schema: &str, table: &str) -> Vec<ReferredFKInfo> {
        self.referred_foreign_keys
            .get(&SchemaAndTableName {
                schema: schema.to_lowercase(),
                table: table.to_lowercase(),
            })
            .cloned()
            .unwrap_or_default()
    }
}

impl InfoSchema for infoSchema {
    fn SchemaMetaVersion(&self) -> i64 {
        self.schema_meta_version
    }
    fn SchemaByName(&self, schema: &CiString) -> Option<Arc<DBInfo>> {
        self.schema_map
            .get(&schema.lower)
            .map(|tables| tables.db_info.clone())
    }
    fn SchemaByID(&self, id: i64) -> Option<Arc<DBInfo>> {
        self.schema_id_to_name
            .get(&id)
            .and_then(|name| self.schema_map.get(name))
            .map(|tables| tables.db_info.clone())
    }
    fn TableByName(&self, schema: &CiString, table: &CiString) -> Result<Table, InfoSchemaError> {
        self.schema_map
            .get(&schema.lower)
            .and_then(|db| db.tables.get(&table.lower))
            .cloned()
            .ok_or_else(|| InfoSchemaError {
                code: ErrTableNotExists.mysql_name,
                message: format!("{}.{}", schema.original, table.original),
            })
    }
    fn TableByID(&self, id: i64) -> Option<Table> {
        if id <= 0 {
            return None;
        }
        let bucket = &self.sorted_table_buckets[tableBucketIdx(id)];
        bucket
            .binary_search_by_key(&id, |table| table.Meta().id)
            .ok()
            .map(|index| bucket[index].clone())
    }
    fn SchemaTableInfos(&self, schema: &CiString) -> Result<Vec<Arc<TableInfo>>, InfoSchemaError> {
        self.SchemaTableInfos(schema)
    }
    fn HasTemporaryTable(&self) -> bool {
        self.HasTemporaryTable()
    }
    fn TableItemByID(&self, id: i64) -> Option<TableItem> {
        let table = self.TableByID(id)?;
        let db = self.SchemaByID(table.Meta().db_id)?;
        Some(TableItem {
            DBName: db.name.clone(),
            TableName: table.Meta().name.clone(),
        })
    }
    fn FindTableByPartitionID(
        &self,
        partition_id: i64,
    ) -> Option<(Table, Arc<DBInfo>, PartitionDefinition)> {
        for db in self.schema_map.values() {
            for table in db.tables.values() {
                if let Some(partition) = &table.Meta().partition {
                    if let Some(definition) = partition
                        .definitions
                        .iter()
                        .find(|definition| definition.id == partition_id)
                    {
                        return Some((table.clone(), db.db_info.clone(), definition.clone()));
                    }
                }
            }
        }
        None
    }
    fn AllSchemas(&self) -> Vec<Arc<DBInfo>> {
        self.schema_map
            .values()
            .map(|tables| tables.db_info.clone())
            .collect()
    }
    fn AllPlacementPolicies(&self) -> Vec<Arc<PolicyInfo>> {
        infoSchema::AllPlacementPolicies(self)
    }
    fn PlacementBundleByPhysicalTableID(&self, id: i64) -> Option<Arc<PlacementBundle>> {
        infoSchema::PlacementBundleByPhysicalTableID(self, id)
    }
    fn AllPlacementBundles(&self) -> Vec<Arc<PlacementBundle>> {
        infoSchema::AllPlacementBundles(self)
    }
    fn MaskingCacheSnapshot(&self) -> (HashMap<i64, HashMap<i64, Arc<MaskingPolicyInfo>>>, bool) {
        (
            self.clone_masking_policies(),
            self.masking_policies_loaded(),
        )
    }
    fn MaskingLoader(&self) -> Option<Arc<dyn MaskingPolicyLoader>> {
        self.masking_loader.clone()
    }
}

/// 由表 ID 计算分桶下标。
pub fn tableBucketIdx(id: i64) -> usize {
    id.unsigned_abs() as usize % bucketCount
}

/// 用给定表列表构造测试用 InfoSchema（默认库名 test，版本 0）。
pub fn MockInfoSchema(mut table_infos: Vec<TableInfo>) -> Arc<infoSchema> {
    MockInfoSchemaWithSchemaVer(std::mem::take(&mut table_infos), 0)
}

/// 同 `MockInfoSchema`，可指定 schema 元版本。
pub fn MockInfoSchemaWithSchemaVer(
    mut table_infos: Vec<TableInfo>,
    schema_version: i64,
) -> Arc<infoSchema> {
    let mut schema = infoSchema::new(schema_version);
    for table in &mut table_infos {
        table.db_id = 1;
    }
    let mut tables: Vec<Table> = table_infos.into_iter().map(Table::new).collect();
    let system = Table::new(TableInfo {
        id: 9999,
        db_id: 2,
        name: CiString::new("stats_meta"),
        columns: vec![ColumnInfo {
            id: 1,
            name: CiString::new("a"),
            auto_increment: false,
        }],
        ..TableInfo::default()
    });
    schema.add_schema(
        DBInfo {
            id: 1,
            name: CiString::new("test"),
            tables: Vec::new(),
            table_name_2_id: Default::default(),
        },
        std::mem::take(&mut tables),
    );
    schema.add_schema(
        DBInfo {
            id: 2,
            name: CiString::new("mysql"),
            tables: Vec::new(),
            table_name_2_id: Default::default(),
        },
        vec![system],
    );
    Arc::new(schema)
}

/// 判断指定库表是否为视图。
pub fn TableIsView(schema: &dyn InfoSchema, db: &CiString, table: &CiString) -> bool {
    schema
        .TableByName(db, table)
        .is_ok_and(|table| table.Meta().is_view)
}
/// 判断指定库表是否为序列。
pub fn TableIsSequence(schema: &dyn InfoSchema, db: &CiString, table: &CiString) -> bool {
    schema
        .TableByName(db, table)
        .is_ok_and(|table| table.Meta().is_sequence)
}
/// 根据表元数据找回所属 schema。
pub fn SchemaByTable(schema: &dyn InfoSchema, table: &TableInfo) -> Option<Arc<DBInfo>> {
    if table.db_id > 0 {
        return schema.SchemaByID(table.db_id);
    }
    let table = schema.TableByID(table.id)?;
    schema.SchemaByID(table.Meta().db_id)
}
/// 返回全部 schema 的原文名字符串列表。
pub fn AllSchemaNames(schema: &dyn InfoSchema) -> Vec<String> {
    schema
        .AllSchemas()
        .into_iter()
        .map(|db| db.name.original.clone())
        .collect()
}
/// 若表有自增列则返回其名称。
pub fn HasAutoIncrementColumn(table: &TableInfo) -> Option<String> {
    table
        .columns
        .iter()
        .find(|column| column.auto_increment)
        .map(|column| column.name.original.clone())
}

/// 先按表 ID 再按分区 ID 查找表（及可选分区定义）。
pub fn FindTableByTblOrPartID(
    schema: &dyn InfoSchema,
    id: i64,
) -> (Option<Table>, Option<PartitionDefinition>) {
    if let Some(table) = schema.TableByID(id) {
        return (Some(table), None);
    }
    match schema.FindTableByPartitionID(id) {
        Some((table, _, partition)) => (Some(table), Some(partition)),
        None => (None, None),
    }
}

/// 脱敏策略加载器：按表 ID 列表与快照时间戳从存储拉取策略。
pub trait MaskingPolicyLoader: Send + Sync {
    fn load(
        &self,
        table_ids: &[i64],
        snapshot_ts: u64,
    ) -> Result<Vec<MaskingPolicyInfo>, InfoSchemaError>;
}

/// 加载全部脱敏策略（无表 ID 过滤）。
pub fn LoadMaskingPolicies(
    loader: &dyn MaskingPolicyLoader,
    snapshot_ts: u64,
) -> Result<Vec<MaskingPolicyInfo>, InfoSchemaError> {
    let mut policies = loader.load(&[], snapshot_ts)?;
    policies.sort_by_key(|policy| (policy.table_id, policy.column_id, policy.id));
    Ok(policies)
}

/// 按表 ID 过滤加载脱敏策略。
pub fn loadMaskingPoliciesWithTableIDs(
    loader: &dyn MaskingPolicyLoader,
    table_ids: &[i64],
    snapshot_ts: u64,
) -> Result<Vec<MaskingPolicyInfo>, InfoSchemaError> {
    let (normalized, has_filter) = normalizeMaskingPolicyTableIDs(table_ids);
    if !has_filter {
        // No filter → load all policies.
        return LoadMaskingPolicies(loader, snapshot_ts);
    }
    // Has filter with empty positive ids → match nothing.
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    const MAX_BATCH_SIZE: usize = 1024;
    let mut policies = Vec::new();
    for batch in normalized.chunks(MAX_BATCH_SIZE) {
        policies.extend(loader.load(batch, snapshot_ts)?);
    }
    policies.sort_by_key(|policy| (policy.table_id, policy.column_id, policy.id));
    Ok(policies)
}

/// Normalize table-id filters for masking-policy loads.
///
/// Mirrors Go `normalizeMaskingPolicyTableIDs`:
/// - empty / nil input → `(empty, false)` meaning "no filter" (load all);
/// - non-empty input → `(sorted unique positive ids, true)` even when the
// / filtered id list ends up empty (caller must treat that as "match nothing").
/// 规范化表 ID：去非正数、去重排序；返回 (ids, 是否有过滤条件)。
pub fn normalizeMaskingPolicyTableIDs(table_ids: &[i64]) -> (Vec<i64>, bool) {
    if table_ids.is_empty() {
        return (Vec::new(), false);
    }
    let mut ids: Vec<i64> = table_ids.iter().copied().filter(|id| *id > 0).collect();
    ids.sort_unstable();
    ids.dedup();
    (ids, true)
}

/// 判断错误是否表示脱敏系统表尚不存在/未就绪。
pub fn isMaskingPolicyTableNotReady(err: &InfoSchemaError) -> bool {
    err.code == ErrTableNotExists.mysql_name || err.code == "ErrNoSuchTable"
}

/// 构造加载脱敏策略的 SQL 形状与绑定参数（迁移期仅保留形状）。
pub fn buildLoadMaskingPoliciesQuery(table_ids: &[i64]) -> (String, Vec<i64>) {
    let base = "SELECT policy_id, policy_name, db_name, table_name, table_id, column_name, column_id, expression, status, masking_type, restrict_on, created_at, updated_at, created_by\nFROM mysql.tidb_masking_policy";
    if table_ids.is_empty() {
        return (
            format!("{base} ORDER BY table_id, column_id, policy_id"),
            Vec::new(),
        );
    }
    let placeholders = std::iter::repeat_n("%?", table_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    (
        format!(
            "{base} WHERE table_id IN ({placeholders}) ORDER BY table_id, column_id, policy_id"
        ),
        table_ids.to_vec(),
    )
}

/// 解析脱敏状态字符串。
pub fn maskingPolicyStatusFromString(status: &str) -> Result<MaskingPolicyStatus, InfoSchemaError> {
    match status.trim().to_ascii_lowercase().as_str() {
        "enabled" | "enable" => Ok(MaskingPolicyStatus::Enabled),
        "disabled" | "disable" => Ok(MaskingPolicyStatus::Disabled),
        _ => Err(InfoSchemaError {
            code: "ErrInvalidMaskingPolicyStatus",
            message: status.to_owned(),
        }),
    }
}
/// 解析脱敏类型字符串。
pub fn maskingPolicyTypeFromString(
    policy_type: &str,
) -> Result<MaskingPolicyType, InfoSchemaError> {
    match policy_type.trim().to_ascii_uppercase().as_str() {
        "MASK_FULL" => Ok(MaskingPolicyType::Full),
        "MASK_PARTIAL" => Ok(MaskingPolicyType::Partial),
        "MASK_NULL" => Ok(MaskingPolicyType::Null),
        "MASK_DATE" => Ok(MaskingPolicyType::Date),
        "CUSTOM" => Ok(MaskingPolicyType::Custom),
        _ => Err(InfoSchemaError {
            code: "ErrInvalidMaskingPolicyType",
            message: policy_type.to_owned(),
        }),
    }
}
/// 解析限制操作字符串为位图。
pub fn maskingPolicyRestrictOpsFromString(
    value: &str,
) -> Result<MaskingPolicyRestrictOps, InfoSchemaError> {
    let value = value.trim().to_ascii_uppercase();
    if value.is_empty() || value == "NONE" {
        return Ok(MaskingPolicyRestrictOps::NONE);
    }
    let mut result = MaskingPolicyRestrictOps::NONE;
    for operation in value.split(',').map(str::trim).filter(|op| !op.is_empty()) {
        result.0 |= match operation {
            "INSERT_INTO_SELECT" => MaskingPolicyRestrictOps::INSERT_INTO_SELECT.0,
            "UPDATE_SELECT" => MaskingPolicyRestrictOps::UPDATE_SELECT.0,
            "DELETE_SELECT" => MaskingPolicyRestrictOps::DELETE_SELECT.0,
            "CTAS" => MaskingPolicyRestrictOps::CTAS.0,
            "NONE" => 0,
            _ => {
                return Err(InfoSchemaError {
                    code: "ErrInvalidMaskingPolicyRestrictOps",
                    message: operation.to_owned(),
                });
            }
        };
    }
    Ok(result)
}

#[derive(Default)]
/// 会话级本地临时表容器。
pub struct SessionTables {
    schemas: HashMap<String, schemaTables>,
    table_ids: HashMap<i64, Table>,
}

impl SessionTables {
    /// 创建空的会话临时表集合。
    pub fn new() -> Self {
        Self::default()
    }
    /// 按库表名查找会话临时表。
    pub fn TableByName(&self, schema: &CiString, table: &CiString) -> Option<Table> {
        self.schemas
            .get(&schema.lower)?
            .tables
            .get(&table.lower)
            .cloned()
    }
    /// 判断库表是否存在。
    /// 会话临时表是否存在。
    pub fn TableExists(&self, schema: &CiString, table: &CiString) -> bool {
        self.TableByName(schema, table).is_some()
    }
    /// 按表 ID 查找会话临时表。
    pub fn TableByID(&self, id: i64) -> Option<Table> {
        self.table_ids.get(&id).cloned()
    }
    /// 向会话临时表注册一张表。
    pub fn AddTable(&mut self, db: DBInfo, table: Table) -> Result<(), InfoSchemaError> {
        if self.table_ids.contains_key(&table.Meta().id)
            || self.TableExists(&db.name, &table.Meta().name)
        {
            return Err(InfoSchemaError {
                code: "ErrTableExists",
                message: table.Meta().name.original.clone(),
            });
        }
        assert_eq!(db.id, table.Meta().db_id);
        self.table_ids.insert(table.Meta().id, table.clone());
        self.schemas
            .entry(db.name.lower.clone())
            .or_insert_with(|| schemaTables {
                db_info: Arc::new(db),
                tables: HashMap::new(),
            })
            .tables
            .insert(table.Meta().name.lower.clone(), table);
        Ok(())
    }
    /// 移除会话临时表。
    pub fn RemoveTable(&mut self, schema: &CiString, table: &CiString) -> bool {
        let Some(removed) = self
            .schemas
            .get_mut(&schema.lower)
            .and_then(|db| db.tables.remove(&table.lower))
        else {
            return false;
        };
        self.table_ids.remove(&removed.Meta().id);
        if self
            .schemas
            .get(&schema.lower)
            .is_some_and(|db| db.tables.is_empty())
        {
            self.schemas.remove(&schema.lower);
        }
        true
    }
    /// 会话临时表数量。
    pub fn Count(&self) -> usize {
        self.table_ids.len()
    }
    pub fn SchemaByID(&self, id: i64) -> Option<Arc<DBInfo>> {
        self.schemas
            .values()
            .find(|schema| schema.db_info.id == id)
            .map(|schema| schema.db_info.clone())
    }
}

/// 构造空的 `SessionTables`。
pub fn NewSessionTables() -> SessionTables {
    SessionTables::new()
}

/// 扩展 InfoSchema：在基础视图上叠加会话临时表查找。
pub struct SessionExtendedInfoSchema {
    pub base: Arc<dyn InfoSchema>,
    pub temporary: SessionTables,
}

impl SessionExtendedInfoSchema {
    /// 先查会话临时表，再回落到底层 InfoSchema。
    pub fn TableByName(
        &self,
        schema: &CiString,
        table: &CiString,
    ) -> Result<Table, InfoSchemaError> {
        self.temporary
            .TableByName(schema, table)
            .map(Ok)
            .unwrap_or_else(|| self.base.TableByName(schema, table))
    }
    /// 先按 ID 查会话临时表，再回落底层。
    pub fn TableByID(&self, id: i64) -> Option<Table> {
        self.temporary
            .TableByID(id)
            .or_else(|| self.base.TableByID(id))
    }
    /// 先查会话临时 schema，再回落底层。
    pub fn SchemaByID(&self, id: i64) -> Option<Arc<DBInfo>> {
        self.temporary
            .SchemaByID(id)
            .or_else(|| self.base.SchemaByID(id))
    }
    /// 是否登记了全局临时表。
    pub fn HasTemporaryTable(&self) -> bool {
        self.temporary.Count() != 0
    }
    /// 剥离临时表层，仅返回底层 InfoSchema。
    pub fn DetachTemporaryTableInfoSchema(&self) -> Arc<dyn InfoSchema> {
        self.base.clone()
    }
}
