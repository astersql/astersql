// Copyright 2026 AsterSQL.

// Shared InfoSchema synchronization models retain complete Go table/database
// metadata alongside lookup fields. The loader binds reads to real KV snapshots
// and applies schema diffs through the public InfoSchema Builder.

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]
use std::fmt;
use std::sync::Arc;

/// 同步过程错误：以字符串消息包装，便于与 Go 侧错误文案对齐断言。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncError(pub String);
impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SyncError {}
impl From<&str> for SyncError {
    fn from(s: &str) -> Self {
        SyncError(s.to_string())
    }
}
impl From<String> for SyncError {
    fn from(s: String) -> Self {
        SyncError(s)
    }
}

/// ActionType is a trimmed-down mirror of Go's `model.ActionType`, keeping only
/// the variants exercised by the issyncer loader and Filter (schema diff
/// dispatch, placement-policy and resource-group pass-through actions).
///
/// DDL 动作类型的精简枚举：仅保留 issyncer 测试会派发的几种
/// （建表、建库、建 Placement Policy）。Placement Policy 是表数据存放策略。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActionType {
    #[default]
    None,
    CreateTable,
    CreateSchema,
    CreatePlacementPolicy,
    AlterPlacementPolicy,
    DropPlacementPolicy,
    CreateResourceGroup,
    DropResourceGroup,
    AlterResourceGroup,
    /// Go numeric action, retained for default table-update dispatch.
    TableUpdate(u8),
    DropTable,
    DropSchema,
}

/// DBInfo mirrors the subset of Go's `model.DBInfo` that issyncer needs: an ID
/// plus the original/lowercased name pair that `ast.CIStr` provides in Go
/// (`.O` / `.L`), since Filter implementations and lookups rely on both forms.
///
/// 数据库（schema）元信息：数值 ID，以及原始名 `NameO` / 小写名 `NameL`
///（对应 Go `ast.CIStr` 的 `.O` / `.L`，用于大小写不敏感比较）。
#[derive(Clone, Debug, Default)]
pub struct DBInfo {
    pub Model: Option<Arc<astersql_meta_model::DBInfo>>,
    /// 数据库 ID。
    pub ID: i64,
    /// 原始库名（大小写保留）。
    pub NameO: String,
    /// 小写库名，供不区分大小写的查找。
    pub NameL: String,
}
impl DBInfo {
    /// 由 ID 与名称构造；自动填充小写名。
    pub fn new(id: i64, name: &str) -> Self {
        Self {
            Model: None,
            ID: id,
            NameO: name.to_string(),
            NameL: name.to_lowercase(),
        }
    }
}

/// TableInfo mirrors the subset of Go's `model.TableInfo` needed here.
///
/// 表元信息子集：表 ID、所属库 ID，以及原始/小写表名。
#[derive(Clone, Debug, Default)]
pub struct TableInfo {
    /// Complete Go model for SQL/planner and system-table schema consumers.
    pub Model: Option<Arc<astersql_meta_model::TableInfo>>,
    /// 表 ID。
    pub ID: i64,
    /// 所属数据库 ID。
    pub SchemaID: i64,
    /// 原始表名。
    pub NameO: String,
    /// 小写表名。
    pub NameL: String,
}
impl TableInfo {
    /// 由表 ID、库 ID 与名称构造。
    pub fn new(id: i64, schema_id: i64, name: &str) -> Self {
        Self {
            Model: None,
            ID: id,
            SchemaID: schema_id,
            NameO: name.to_string(),
            NameL: name.to_lowercase(),
        }
    }
}

/// SchemaDiff mirrors the subset of Go's `model.SchemaDiff` needed by
/// `Loader::skipLoadingDiff` and diff-based loading.
///
/// 一次 schema 版本变更的增量描述：版本号、DDL 类型，以及涉及的新旧库/表 ID。
/// 增量加载时按版本依次应用这些 diff。
#[derive(Clone, Debug, Default)]
pub struct SchemaDiff {
    pub RegenerateSchemaMap: bool,
    pub ReadTableFromMeta: bool,
    pub AffectedOptions: Vec<astersql_infoschema::builder::AffectedOption>,
    pub SubActionTypes: Vec<u8>,
    /// schema 元版本号。
    pub Version: i64,
    /// DDL 动作类型。
    pub Type: ActionType,
    /// 目标数据库 ID。
    pub SchemaID: i64,
    /// 变更前数据库 ID（跨库 rename 等场景）。
    pub OldSchemaID: i64,
    /// 目标表 ID。
    pub TableID: i64,
    /// 变更前表 ID。
    pub OldTableID: i64,
}

/// RelatedSchemaChange mirrors `transaction.RelatedSchemaChange`.
///
/// 增量加载后汇报的“相关表变更”：物理表 ID 列表及对应动作类型，
/// 供 SchemaValidator（校验事务是否仍可使用旧 schema）更新。
#[derive(Clone, Debug, Default)]
pub struct RelatedSchemaChange {
    /// 发生变更的物理表 ID 列表。
    pub PhyTblIDS: Vec<i64>,
    /// 与各物理表对应的 DDL 动作类型。
    pub ActionTypes: Vec<ActionType>,
}

/// SchemaInfo is a trimmed-down stand-in for Go's `infoschema.InfoSchema`,
/// exposing only the read APIs the issyncer tests rely on.
///
/// 精简版 Information Schema：当前版本号及库/表列表，提供按 ID/名查询接口。
#[derive(Clone, Debug, Default)]
pub struct SchemaInfo {
    /// 当前 schema 元版本。
    pub Version: i64,
    /// 已加载的数据库列表。
    pub Databases: Vec<DBInfo>,
    /// 已加载的表列表。
    pub Tables: Vec<TableInfo>,
}
impl SchemaInfo {
    /// 返回全部数据库元信息。
    pub fn AllSchemas(&self) -> &[DBInfo] {
        &self.Databases
    }
    /// 返回当前 schema 元版本号。
    pub fn SchemaMetaVersion(&self) -> i64 {
        self.Version
    }
    /// 按数据库 ID 查找。
    pub fn SchemaByID(&self, id: i64) -> Option<&DBInfo> {
        self.Databases.iter().find(|d| d.ID == id)
    }
    /// 按库名（大小写不敏感）查找。
    pub fn SchemaByName(&self, name: &str) -> Option<&DBInfo> {
        let l = name.to_lowercase();
        self.Databases.iter().find(|d| d.NameL == l)
    }
    /// 返回指定库下全部表信息；库不存在时返回空向量。
    pub fn SchemaTableInfos(&self, name: &str) -> Vec<TableInfo> {
        match self.SchemaByName(name) {
            Some(db) => self
                .Tables
                .iter()
                .filter(|t| t.SchemaID == db.ID)
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    }
}

/// 延迟回调队列（到期后触发）。
mod deferfn;
/// 自定义加载过滤（跳过某些 diff / schema）。
mod filter;
/// 从 SchemaStore 全量或增量加载 InfoSchema。
mod loader;
/// MDL（Metadata Lock，元数据锁）检查相关表信息。
mod mdl_check;
/// 对外 Syncer：封装 Loader 与 schema 校验器。
mod syncer;
pub use deferfn::*;
pub use filter::*;
pub use loader::*;
pub use mdl_check::*;
pub use syncer::*;

#[cfg(test)]
#[path = "deferfn_test.rs"]
mod deferfn_test;
#[cfg(test)]
#[path = "loader_test.rs"]
mod loader_test;
#[cfg(test)]
#[path = "syncer_test.rs"]
mod syncer_test;

impl PartialEq for DBInfo {
    fn eq(&self, other: &Self) -> bool {
        self.ID == other.ID
            && self.NameO == other.NameO
            && self.NameL == other.NameL
            && self
                .Model
                .as_ref()
                .map(|m| astersql_meta_model::EncodeDBInfo(m))
                == other
                    .Model
                    .as_ref()
                    .map(|m| astersql_meta_model::EncodeDBInfo(m))
    }
}
impl Eq for DBInfo {}
impl PartialEq for TableInfo {
    fn eq(&self, other: &Self) -> bool {
        self.ID == other.ID
            && self.SchemaID == other.SchemaID
            && self.NameO == other.NameO
            && self.NameL == other.NameL
            && self
                .Model
                .as_ref()
                .map(|m| astersql_meta_model::EncodeTableInfo(m))
                == other
                    .Model
                    .as_ref()
                    .map(|m| astersql_meta_model::EncodeTableInfo(m))
    }
}
impl Eq for TableInfo {}

impl DBInfo {
    pub(crate) fn builder_database(&self) -> astersql_infoschema::DBInfo {
        astersql_infoschema::DBInfo {
            id: self.ID,
            name: astersql_infoschema::CiString::new(&self.NameO),
            ..Default::default()
        }
    }
    pub fn from_model(model: astersql_meta_model::DBInfo) -> Self {
        Self {
            ID: model.ID,
            NameO: model.Name.O.clone(),
            NameL: model.Name.L.clone(),
            Model: Some(Arc::new(model)),
        }
    }
}
impl TableInfo {
    pub(crate) fn builder_table(&self) -> astersql_infoschema::TableInfo {
        self.Model
            .as_ref()
            .map(|model| {
                astersql_infoschema::Table::from_model((**model).clone())
                    .Meta()
                    .clone()
            })
            .unwrap_or_else(|| astersql_infoschema::TableInfo {
                id: self.ID,
                db_id: self.SchemaID,
                name: astersql_infoschema::CiString::new(&self.NameO),
                ..Default::default()
            })
    }
    pub fn from_model(mut model: astersql_meta_model::TableInfo, db: i64) -> Self {
        model.DBID = db;
        Self {
            ID: model.ID,
            SchemaID: db,
            NameO: model.Name.O.clone(),
            NameL: model.Name.L.clone(),
            Model: Some(Arc::new(model)),
        }
    }
}
impl SchemaInfo {
    pub(crate) fn builder_databases(&self) -> Vec<astersql_infoschema::DBInfo> {
        self.Databases
            .iter()
            .map(|db| {
                let mut result = db.builder_database();
                result.tables = self
                    .Tables
                    .iter()
                    .filter(|t| t.SchemaID == db.ID)
                    .map(|t| Arc::new(t.builder_table()))
                    .collect();
                result
            })
            .collect()
    }
    /// Build the complete SQL InfoSchema without reconstructing lossy models.
    pub fn CompleteInfoSchema(&self) -> astersql_infoschema::SchemaRef {
        let mut builder = astersql_infoschema::builder::Builder::new(
            astersql_infoschema::infoschema_v2::NewData(),
            false,
        )
        .WithCrossKS(true);
        builder.InitWithDBInfos(&mut self.builder_databases(), vec![], vec![], self.Version);
        builder.Build(0)
    }
    pub(crate) fn from_schema(schema: &dyn astersql_infoschema::InfoSchema) -> Self {
        let mut result = Self {
            Version: schema.SchemaMetaVersion(),
            ..Default::default()
        };
        for db in schema.AllSchemas() {
            result.Databases.push(DBInfo::new(db.id, &db.name.original));
            for table in &db.tables {
                result.Tables.push(TableInfo {
                    ID: table.id,
                    SchemaID: db.id,
                    NameO: table.name.original.clone(),
                    NameL: table.name.lower.clone(),
                    Model: table.model_meta.clone(),
                });
            }
        }
        result
    }
}
impl ActionType {
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => Self::None,
            1 => Self::CreateSchema,
            2 => Self::DropSchema,
            3 => Self::CreateTable,
            4 => Self::DropTable,
            51 => Self::CreatePlacementPolicy,
            52 => Self::AlterPlacementPolicy,
            53 => Self::DropPlacementPolicy,
            68 => Self::CreateResourceGroup,
            69 => Self::AlterResourceGroup,
            70 => Self::DropResourceGroup,
            code => Self::TableUpdate(code),
        }
    }
    pub fn code(self) -> u8 {
        match self {
            Self::None => 0,
            Self::CreateSchema => 1,
            Self::DropSchema => 2,
            Self::CreateTable => 3,
            Self::DropTable => 4,
            Self::CreatePlacementPolicy => 51,
            Self::AlterPlacementPolicy => 52,
            Self::DropPlacementPolicy => 53,
            Self::CreateResourceGroup => 68,
            Self::AlterResourceGroup => 69,
            Self::DropResourceGroup => 70,
            Self::TableUpdate(code) => code,
        }
    }
}
impl SchemaDiff {
    pub(crate) fn builder_diff(&self) -> astersql_infoschema::builder::SchemaDiff {
        use astersql_infoschema::builder::ActionType as A;
        let action = |kind: ActionType| match kind {
            ActionType::None => A::TableUpdate(0),
            ActionType::CreateSchema => A::CreateSchema,
            ActionType::DropSchema => A::DropSchema,
            ActionType::CreateTable => A::CreateTable,
            ActionType::DropTable => A::DropTable,
            ActionType::CreatePlacementPolicy => A::CreatePlacementPolicy,
            ActionType::AlterPlacementPolicy => A::AlterPlacementPolicy,
            ActionType::DropPlacementPolicy => A::DropPlacementPolicy,
            ActionType::CreateResourceGroup => A::CreateResourceGroup,
            ActionType::AlterResourceGroup => A::AlterResourceGroup,
            ActionType::DropResourceGroup => A::DropResourceGroup,
            ActionType::TableUpdate(code) => match code {
                5 => A::AddColumn,
                11 => A::TruncateTable,
                14 => A::RenameTable,
                19 => A::AddTablePartition,
                20 => A::DropTablePartition,
                21 | 32 => A::CreateTable,
                23 => A::TruncateTablePartition,
                24 | 36 => A::DropTable,
                25 => A::RecoverTable,
                26 => A::ModifySchemaCharsetAndCollate,
                13 => A::RebaseAutoID,
                39 => A::ModifyTableAutoIDCache,
                40 => A::RebaseAutoRandomBase,
                42 => A::ExchangeTablePartition,
                47 => A::RenameTables,
                55 => A::ModifySchemaDefaultPlacement,
                60 => A::CreateTables,
                61 => A::MultiSchemaChange,
                63 => A::RecoverSchema,
                64 => A::ReorganizePartition,
                71 => A::AlterTablePartitioning,
                72 => A::RemovePartitioning,
                76 => A::RefreshMeta,
                81 => A::CreateMaskingPolicy,
                82 => A::AlterMaskingPolicy,
                83 => A::DropMaskingPolicy,
                85 => A::CreateMaterializedViewLog,
                86 => A::CreateMaterializedView,
                87 => A::DropMaterializedViewLog,
                88 => A::DropMaterializedView,
                92 => A::MViewRefreshOutOfPlaceCutover,
                93 => A::CreateMaterializedViewShadow,
                94 => A::DropMaterializedViewShadow,
                _ => A::TableUpdate(code),
            },
        };
        astersql_infoschema::builder::SchemaDiff {
            version: self.Version,
            action_type: action(self.Type),
            schema_id: self.SchemaID,
            table_id: self.TableID,
            old_schema_id: self.OldSchemaID,
            old_table_id: self.OldTableID,
            affected_options: self.AffectedOptions.clone(),
            sub_action_types: self
                .SubActionTypes
                .iter()
                .map(|v| action(ActionType::from_code(*v)))
                .collect(),
        }
    }
}
