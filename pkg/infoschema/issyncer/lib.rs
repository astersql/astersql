// Copyright 2026 AsterSQL.

// InfoSchema 同步器（issyncer）crate 入口与精简领域类型。
//
// 对应 Go 的 `pkg/infoschema/issyncer`：负责把元数据存储中的 schema 变更
// （DDL 作业产生的 SchemaDiff）同步到本机缓存的 Information Schema。
// Information Schema 是库/表/列等元数据的内存视图；SchemaDiff 是一次 DDL
// 提交后记录的版本增量。
//
// 本文件定义同步过程所需的裁剪版模型类型（`DBInfo`/`TableInfo`/`SchemaDiff`/
// `SchemaInfo` 等），并声明 `loader`/`syncer`/`filter`/`mdl_check`/`deferfn`
// 子模块。完整 Go 依赖（kv.Storage、infoschema.Builder）尚未可编译移植，
// 因此此处仅保留测试与同步逻辑真正用到的字段与 API。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]
use std::fmt;

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
}

/// DBInfo mirrors the subset of Go's `model.DBInfo` that issyncer needs: an ID
/// plus the original/lowercased name pair that `ast.CIStr` provides in Go
/// (`.O` / `.L`), since Filter implementations and lookups rely on both forms.
///
/// 数据库（schema）元信息：数值 ID，以及原始名 `NameO` / 小写名 `NameL`
///（对应 Go `ast.CIStr` 的 `.O` / `.L`，用于大小写不敏感比较）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DBInfo {
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
            ID: id,
            NameO: name.to_string(),
            NameL: name.to_lowercase(),
        }
    }
}

/// TableInfo mirrors the subset of Go's `model.TableInfo` needed here.
///
/// 表元信息子集：表 ID、所属库 ID，以及原始/小写表名。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
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
