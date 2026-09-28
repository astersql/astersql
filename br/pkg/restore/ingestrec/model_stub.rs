// Copyright 2026 AsterSQL.
//! Local stand-ins for meta/model, infoschema, ast, mysql, types used by ingestrec.
//! Keeps this crate off the grpcio rebuild path while preserving Go field/API shapes.
//!
//! 中文模块概述：本文件是 ingestrec 的本地模型替身，镜像 Go meta/model、infoschema、
//! ast.CIStr、mysql.PriKeyFlag 与 types.UnspecifiedLength 的字段形状与关键判定，
//! 使录制器/外键逻辑可单测且不拉起 kvproto。桩服务于验证与编译隔离，不代表生产
//! 路径依赖这些简化类型。前缀覆盖与部分索引谓词判定对齐 `model.IsIndexPrefixCovered*`。
//! Job/ActionType/ReorgType 仅覆盖录制器需要的子集，数值与 Go iota 保持一致。
//! InfoSchema trait 是 FK 与 UpdateIndexInfo 的唯一外部依赖面。
//! annotatef/trace 统一错误包装，避免各调用点直接依赖 astersql_errors 细节。

use astersql_errors::{Annotate, SharedError, Trace};

/// Unspecified column length (types.UnspecifiedLength).
/// 未指定前缀长度时为 -1；与 Go types.UnspecifiedLength 相同。
pub const UnspecifiedLength: isize = -1;

/// 大小写不敏感标识：O 保留原文，L 为小写，查找一律用 L。
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct CIStr {
    pub O: String,
    pub L: String,
}

impl CIStr {
    // 构造时同步生成小写形式，避免调用方遗漏规范化。
    pub fn new(o: impl Into<String>) -> Self {
        let O = o.into();
        let L = O.to_lowercase();
        Self { O, L }
    }
}

/// 外键元数据片段：名称、子表列与引用列（CIStr 向量）。
#[derive(Clone, Debug, Default)]
pub struct FKInfo {
    pub Name: CIStr,
    pub Cols: Vec<CIStr>,
    pub RefCols: Vec<CIStr>,
}

/// 列信息：Offset 对应表.Columns 下标；Hidden 表示生成列。
#[derive(Clone, Debug, Default)]
pub struct ColumnInfo {
    pub Name: CIStr,
    pub Offset: usize,
    pub Hidden: bool,
    pub GeneratedExprString: String,
    pub Flag: u32,
    pub Flen: isize,
}

impl ColumnInfo {
    // 与 Go ColumnInfo.GetFlag 对齐，供主键标志检测。
    pub fn GetFlag(&self) -> u32 {
        self.Flag
    }
    // 列定义长度，用于前缀索引覆盖判定。
    pub fn GetFlen(&self) -> isize {
        self.Flen
    }
}

// mysql.PriKeyFlag：位 1，与 HasPriKeyFlag 配合使用。
pub const PriKeyFlag: u32 = 1 << 1; // mysql.PriKeyFlag

/// 判断列 flag 是否含主键位。
pub fn HasPriKeyFlag(flag: u32) -> bool {
    flag & PriKeyFlag != 0
}

/// 按小写列名在切片中查找列；找不到返回 None。
pub fn FindColumnInfo<'a>(cols: &'a [ColumnInfo], name_l: &str) -> Option<&'a ColumnInfo> {
    cols.iter().find(|c| c.Name.L == name_l)
}

/// 索引列：Offset 指向表列；Length 为前缀长度或 UnspecifiedLength。
#[derive(Clone, Debug, Default)]
pub struct IndexColumn {
    pub Name: CIStr,
    pub Offset: i32,
    pub Length: isize,
}

/// 索引元数据；ConditionExprString 非空表示部分索引谓词。
#[derive(Clone, Debug, Default)]
pub struct IndexInfo {
    pub ID: i64,
    pub Name: CIStr,
    pub Table: CIStr,
    pub Columns: Vec<IndexColumn>,
    /// Partial index predicate string (model.IndexInfo.ConditionExprString).
    pub ConditionExprString: String,
}

impl IndexInfo {
    /// 是否带部分索引条件（非空谓词串）。
    pub fn HasCondition(&self) -> bool {
        !self.ConditionExprString.is_empty()
    }
}

/// 表元数据：列、索引、外键；PKIsHandle 表示整型主键句柄。
#[derive(Clone, Debug, Default)]
pub struct TableInfo {
    pub ID: i64,
    pub DBID: i64,
    pub Name: CIStr,
    pub Columns: Vec<ColumnInfo>,
    pub Indices: Vec<IndexInfo>,
    pub ForeignKeys: Vec<FKInfo>,
    pub PKIsHandle: bool,
}

impl TableInfo {
    /// 按索引名（大小写不敏感）查找；对齐 Go FindIndexByName。
    pub fn FindIndexByName(&self, name: &str) -> Option<&IndexInfo> {
        let lower = name.to_lowercase();
        self.Indices.iter().find(|idx| idx.Name.L == lower)
    }
}

/// Prefix coverage matching `model.IsIndexPrefixCovered`.
/// 索引前缀列名与 FK 列一致，且前缀长度足以覆盖列 Flen；列数不足则失败。
pub fn IsIndexPrefixCovered(table: &TableInfo, index: &IndexInfo, fk_cols: &[CIStr]) -> bool {
    if index.Columns.len() < fk_cols.len() {
        return false;
    }
    for (i, fk_col) in fk_cols.iter().enumerate() {
        if index.Columns[i].Name.L != fk_col.L {
            return false;
        }
        let offset = index.Columns[i].Offset as usize;
        if offset >= table.Columns.len() {
            return false;
        }
        let col_info = &table.Columns[offset];
        // 指定了前缀长度且短于列定义长度 → 不足以支撑 FK。
        if index.Columns[i].Length != UnspecifiedLength
            && index.Columns[i].Length < col_info.GetFlen()
        {
            return false;
        }
    }
    true
}

/// Matching `model.IsIndexPrefixCoveredForForeignKey` including partial-index safety.
/// 在前缀覆盖之上，部分索引谓词必须被 FK 列安全覆盖（MATCH SIMPLE）。
pub fn IsIndexPrefixCoveredForForeignKey(
    table: &TableInfo,
    index: &IndexInfo,
    fk_cols: &[CIStr],
) -> bool {
    if !IsIndexPrefixCovered(table, index, fk_cols) {
        return false;
    }
    isIndexConditionCoveredByForeignKeyCols(index, fk_cols)
}

/// Partial predicate must be `<fk-col> IS NOT NULL` (MATCH SIMPLE).
/// 轻量替代解析器：仅接受 `col is not null` / `` `col` is not null ``。
fn isIndexConditionCoveredByForeignKeyCols(index: &IndexInfo, fk_cols: &[CIStr]) -> bool {
    if !index.HasCondition() {
        return true;
    }
    // Lightweight stand-in for parser-backed ConditionExpr(): accept
    // "`col` is not null" / "col is not null" only.
    let raw = index.ConditionExprString.trim().to_lowercase();
    let Some(col_part) = raw.strip_suffix(" is not null") else {
        // 非 IS NOT NULL 形态视为不安全，拒绝覆盖。
        return false;
    };
    // Go's parser stores the terminal identifier in ColumnName.Name, so a
    // qualified column such as `tbl`.`col` is compared as just `col`.
    let col_name = col_part
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches('`')
        .trim()
        .to_lowercase();
    if col_name.is_empty() {
        return false;
    }
    // 谓词列必须属于 FK 列集合，否则删除索引会破坏约束语义。
    fk_cols.iter().any(|c| c.L == col_name)
}

/// 库元数据：ID 与名称。
#[derive(Clone, Debug, Default)]
pub struct DBInfo {
    pub ID: i64,
    pub Name: CIStr,
}

/// 被引用外键信息：父表侧视角下的子表 FK 定位。
#[derive(Clone, Debug, Default)]
pub struct ReferredFKInfo {
    pub Cols: Vec<CIStr>,
    pub ChildSchema: CIStr,
    pub ChildTable: CIStr,
    pub ChildFKName: CIStr,
}

/// DDL 重组类型；判别值与 Go `model.ReorgType` 的 iota 完全一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i8)]
pub enum ReorgType {
    None = 0,
    Txn = 1,
    Ingest = 2,
    TxnMerge = 3,
}

impl ReorgType {
    /// Backward-compatible name used by older local fixtures.
    pub const LitMerge: Self = Self::TxnMerge;
}

/// DDL 重组元数据，录制器只关心 ReorgTp 是否为 Ingest。
#[derive(Clone, Debug, Default)]
pub struct DDLReorgMeta {
    pub ReorgTp: ReorgType,
}

impl Default for ReorgType {
    // Go 零值是 ReorgTypeNone。
    fn default() -> Self {
        ReorgType::None
    }
}

/// DDL Action 子集：录制器仅识别加索引/主键与改列。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ActionType {
    Other = 0,
    AddIndex = 7,
    DropIndex = 8,
    ModifyColumn = 12,
    AddPrimaryKey = 32,
}

/// Job 状态：Synced/Done 决定是否被 TryAddJob 接受。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum JobState {
    None = 0,
    RollbackDone = 3,
    Done = 4,
    Synced = 6,
}

/// DDL Job 替身：携带 finished_*_args 供 GetFinished* 解析。
#[derive(Clone, Debug, Default)]
pub struct Job {
    pub TableID: i64,
    pub Type: ActionType,
    pub State: JobState,
    pub ReorgMeta: Option<DDLReorgMeta>,
    pub finished_index_args: Option<FinishedModifyIndexArgs>,
    pub finished_column_args: Option<FinishedModifyColumnArgs>,
}

impl Default for ActionType {
    fn default() -> Self {
        ActionType::Other
    }
}

impl Default for JobState {
    fn default() -> Self {
        JobState::None
    }
}

/// 单个索引参数：仅 IndexID 参与录制。
#[derive(Clone, Debug, Default)]
pub struct IndexArg {
    pub IndexID: i64,
}

/// 加索引类 job 完成后的参数列表。
#[derive(Clone, Debug, Default)]
pub struct FinishedModifyIndexArgs {
    pub IndexArgs: Vec<IndexArg>,
}

/// 改列 job 产生的新索引 ID 列表。
#[derive(Clone, Debug, Default)]
pub struct FinishedModifyColumnArgs {
    pub NewIndexIDs: Vec<i64>,
}

/// 缺失 finished_index_args 时返回错误（对齐 Go 解码失败路径）。
pub fn GetFinishedModifyIndexArgs(job: &Job) -> Result<FinishedModifyIndexArgs, SharedError> {
    job.finished_index_args
        .clone()
        .ok_or_else(|| astersql_errors::New("missing finished modify index args"))
}

/// 缺失 finished_column_args 时返回错误。
pub fn GetFinishedModifyColumnArgs(job: &Job) -> Result<FinishedModifyColumnArgs, SharedError> {
    job.finished_column_args
        .clone()
        .ok_or_else(|| astersql_errors::New("missing finished modify column args"))
}

/// InfoSchema boundary used by ingestrec.
/// 录制器/FK 管理器依赖的最小 infoschema 面：按名取表、按 ID 取表/库、被引用 FK。
pub trait InfoSchema: Send + Sync {
    fn GetTableReferredForeignKeys(&self, schema_l: &str, table_l: &str) -> Vec<ReferredFKInfo>;
    fn TableByName(
        &self,
        child_schema: &CIStr,
        child_table: &CIStr,
    ) -> Result<TableInfo, SharedError>;
    fn TableInfoByID(&self, table_id: i64) -> Option<TableInfo>;
    fn SchemaByID(&self, db_id: i64) -> Option<DBInfo>;
}

/// 包装 annotate：为错误附加上下文消息。
pub fn annotatef(err: SharedError, msg: impl Into<String>) -> SharedError {
    Annotate(Some(err), msg).expect("annotate")
}

/// 包装 trace：保留错误栈信息。
pub fn trace(err: SharedError) -> SharedError {
    Trace(Some(err)).expect("trace")
}

/// Context stand-in.
/// 占位上下文；当前录制路径不读取其字段，仅保持 API 形状。
#[derive(Clone, Debug, Default)]
pub struct Context;

/// 列参数在 Rust 侧简化为 String（Go 为 any，实际为列名）。
pub type ColumnArg = String;
