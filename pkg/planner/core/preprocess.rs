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
// Copyright 2026 AsterSQL.

// SQL 预处理：语法与语义边界校验。
//
// 在正式计划构建前遍历语句/DDL 节点，检查名称合法性、列与索引选项、
// CTE 消费、锁表上下文，并收集过期读等快照信息，对齐 Go 的 preprocess 流程。

use crate::planbuilder::{BuilderError, Statement, TableInfo};
use crate::task::{PlanKind, PlanNode};
use std::collections::{HashMap, HashSet};

/// 预处理与计划构建共用的 Result 别名。
pub type Result<T> = std::result::Result<T, BuilderError>;
/// 预处理选项：闭包形式配置 preprocessor 标志/返回值。
pub type PreprocessOpt = dyn Fn(&mut preprocessor);
/// 预处理器内部状态位标志类型。
type preprocessorFlag = u64;
/// Unexpected or unsupported statement type.
pub const TypeInvalid: u8 = 0;
/// SELECT statement.
pub const TypeSelect: u8 = 1;
/// Set-operation statement such as UNION.
pub const TypeSetOpr: u8 = 2;
/// DELETE statement.
pub const TypeDelete: u8 = 3;
/// UPDATE statement.
pub const TypeUpdate: u8 = 4;
/// INSERT statement.
pub const TypeInsert: u8 = 5;
/// 标志：当前处于 PREPARE 语句处理。
const inPrepare: preprocessorFlag = 1 << 0;
/// 标志：当前处于事务重试路径。
const inTxnRetry: preprocessorFlag = 1 << 1;
/// 标志：正在处理建表/删表。
const inCreateOrDropTable: preprocessorFlag = 1 << 2;
/// 标志：父节点为 Join。
const parentIsJoin: preprocessorFlag = 1 << 3;
/// 标志：处于 REPAIR TABLE 流程。
const inRepairTable: preprocessorFlag = 1 << 4;
/// 标志：位于序列函数调用内。
const inSequenceFunction: preprocessorFlag = 1 << 5;
/// 标志：需初始化事务上下文提供者。
const initTxnContextProvider: preprocessorFlag = 1 << 6;
/// 标志：处于 IMPORT INTO 语句。
const inImportInto: preprocessorFlag = 1 << 7;
/// 标志：处于 ANALYZE 语句。
const inAnalyze: preprocessorFlag = 1 << 8;

/// 选项：标记预处理器进入 PREPARE 模式。
pub fn InPrepare(p: &mut preprocessor) {
    // 置位 PREPARE 场景标志。
    p.flag |= inPrepare;
}
/// 选项：标记预处理器进入事务重试模式。
pub fn InTxnRetry(p: &mut preprocessor) {
    // 置位事务重试场景标志。
    p.flag |= inTxnRetry;
}
/// 选项：要求初始化事务上下文提供者。
pub fn InitTxnContextProvider(p: &mut preprocessor) {
    p.flag |= initTxnContextProvider;
}
/// 选项：注入预处理器返回结构（过期读等快照信息）。
pub fn WithPreprocessorReturn(ret: PreprocessorReturn) -> impl Fn(&mut preprocessor) {
    move |p| p.PreprocessorReturn = ret.clone()
}

#[derive(Clone, Debug, Default)]
/// 预处理产出：是否过期读、快照 TS、InfoSchema 版本等。
pub struct PreprocessorReturn {
    /// 是否过期读（staleness read）。
    pub IsStaleness: bool,
    /// 最近快照时间戳。
    pub LastSnapshotTS: u64,
    /// InfoSchema 版本。
    pub InfoSchemaVersion: u64,
    /// 是否已初始化快照 TS。
    pub initedLastSnapshotTS: bool,
}

#[derive(Clone, Debug, Default)]
/// WITH 子句中 CTE 定义及其消费计数。
pub struct CteDefinition {
    /// 名称。
    pub name: String,
    /// CTE 被消费次数。
    pub consumer_count: usize,
}
#[derive(Clone, Debug, Default)]
/// WITH/CTE 预处理栈：可用 CTE、偏移与嵌套定义。
pub struct preprocessWith {
    /// 当前可见可用的 CTE 名。
    pub cteCanUsed: Vec<String>,
    /// CTE 偏移记录。
    pub cteBeforeOffset: Vec<usize>,
    /// 嵌套 WITH 的 CTE 定义栈。
    pub cteStack: Vec<Vec<CteDefinition>>,
}
impl preprocessWith {
    /// 按表名自内向外累加 CTE 消费次数。
    pub fn UpdateCTEConsumerCount(&mut self, tableName: &str) {
        for layer in self.cteStack.iter_mut().rev() {
            if let Some(cte) = layer
                .iter_mut()
                .find(|c| c.name.eq_ignore_ascii_case(tableName))
            {
                cte.consumer_count += 1;
                return;
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 列级选项：空值、自增、主键、默认值、生成列等。
pub enum ColumnOption {
    NotNull,
    Null,
    AutoIncrement,
    PrimaryKey,
    UniqueKey,
    Default(String),
    Generated,
    AutoRandom,
    Comment,
}
#[derive(Clone, Debug)]
/// 列定义：名称、类型与选项。
pub struct ColumnDef {
    /// 名称。
    pub name: String,
    /// 数据类型名。
    pub data_type: String,
    /// 选项列表。
    pub options: Vec<ColumnOption>,
    /// 是否生成列。
    pub generated: bool,
    /// 生成列是否 STORED。
    pub stored: bool,
}
#[derive(Clone, Debug, Default)]
/// 索引选项：类型、解析器、可见性、全局索引等。
pub struct IndexOption {
    /// 索引类型。
    pub index_type: String,
    /// 全文解析器名。
    pub parser_name: Option<String>,
    /// 注释。
    pub comment: String,
    /// 是否不可见索引。
    pub invisible: bool,
    /// 是否全局索引。
    pub global: bool,
}
#[derive(Clone, Debug)]
/// 索引键部件：列名、前缀长度或表达式。
pub struct IndexPart {
    /// 列名。
    pub column: String,
    /// 前缀长度。
    pub length: Option<usize>,
    /// 表达式文本。
    pub expression: Option<String>,
}
#[derive(Clone, Debug)]
/// 表约束（主键/唯一等）及其索引部件。
pub struct Constraint {
    /// 名称。
    pub name: String,
    /// 是否主键约束。
    pub primary: bool,
    /// 是否唯一索引。
    pub unique: bool,
    /// 索引/约束部件。
    pub parts: Vec<IndexPart>,
    /// 关联选项。
    pub option: IndexOption,
}
#[derive(Clone, Debug)]
/// 表级选项：引擎、字符集、注释等。
pub enum TableOption {
    Engine(String),
    Charset(String),
    Collate(String),
    Comment(String),
    AutoIncrement(u64),
    Unsupported(String),
}
#[derive(Clone, Debug, Default)]
/// 带 schema/别名/临时表标记的表名引用。
pub struct TableName {
    /// 输出 Schema。
    pub schema: String,
    /// 名称。
    pub name: String,
    /// 别名。
    pub alias: String,
    /// 是否临时表。
    pub temporary: bool,
    /// 是否序列对象。
    pub sequence: bool,
}
impl TableName {
    /// 生成 schema.table 小写键，用于去重与查找。
    fn key(&self) -> String {
        format!(
            "{}.{}",
            self.schema.to_ascii_lowercase(),
            self.name.to_ascii_lowercase()
        )
    }
    /// 对外可见名：优先别名，否则表名（小写）。
    fn visible_name(&self) -> String {
        if self.alias.is_empty() {
            self.name.to_ascii_lowercase()
        } else {
            self.alias.to_ascii_lowercase()
        }
    }
}

#[derive(Clone, Debug)]
/// 预处理可遍历的语句/DDL 节点枚举。
pub enum PreprocessNode {
    Statement(Statement),
    CreateDatabase {
        name: String,
    },
    AlterDatabase {
        name: String,
    },
    DropDatabase {
        name: String,
    },
    CreateTable {
        table: TableName,
        columns: Vec<ColumnDef>,
        constraints: Vec<Constraint>,
        options: Vec<TableOption>,
    },
    CreateView {
        name: TableName,
        columns: Vec<String>,
        select_fields: usize,
    },
    CreateIndex {
        table: TableName,
        name: String,
        parts: Vec<IndexPart>,
        option: IndexOption,
    },
    DropTables {
        tables: Vec<TableName>,
        temporary_only: bool,
    },
    DropSequences(Vec<TableName>),
    RenameTables(Vec<(TableName, TableName)>),
    RepairTable {
        table: TableName,
        create: Box<PreprocessNode>,
    },
    AlterTable {
        table: TableName,
        operations: Vec<AlterTableOperation>,
    },
    Select {
        tables: Vec<TableName>,
        lock_tables: Vec<TableName>,
        aliases: Vec<String>,
        noop_functions: Vec<String>,
        group_by_positions: Vec<i64>,
    },
    Binding {
        origin_type: u8,
        hinted_type: u8,
        origin_db: String,
        hinted_db: String,
    },
    Show {
        table: Option<TableName>,
    },
    Execute {
        name: String,
    },
    CreateSequence(TableName),
    Cast {
        target: String,
        flen: i32,
        decimal: i32,
    },
    UserVariable {
        name: String,
        assigned: bool,
    },
}

#[derive(Clone, Debug)]
/// ALTER TABLE 子操作种类。
pub enum AlterTableOperation {
    AddColumn(ColumnDef),
    AddConstraint(Constraint),
    RenameTo(TableName),
    Engine(String),
    DropColumn(String),
    ModifyColumn(ColumnDef),
    Unsupported(String),
}

#[derive(Clone, Debug, Default)]
/// LOCK TABLES 中的表引用记录。
pub struct lockRef {
    pub table_name: String,
    /// 别名。
    pub alias: String,
}
#[derive(Clone, Debug, Default)]
/// SELECT ... FOR UPDATE 等锁表上下文。
pub struct lockSelectCtx {
    pub lock_tables: HashMap<String, lockRef>,
    pub used_tables: HashSet<String>,
}
/// 由 LOCK TABLES 列表构造锁选择上下文。
pub fn newLockSelectCtx(lockTables: &[TableName]) -> lockSelectCtx {
    let lock_tables = lockTables
        .iter()
        .map(|t| {
            (
                t.visible_name(),
                lockRef {
                    table_name: t.key(),
                    alias: t.alias.clone(),
                },
            )
        })
        .collect();
    lockSelectCtx {
        lock_tables,
        used_tables: HashSet::new(),
    }
}
impl lockSelectCtx {
    /// 收集锁上下文中的表名与别名。
    pub fn collect(&mut self, tableName: &TableName, asName: &str) {
        self.used_tables.insert(if asName.is_empty() {
            tableName.visible_name()
        } else {
            asName.to_ascii_lowercase()
        });
    }
}

#[derive(Clone, Debug, Default)]
/// 语句预处理器：语法检查、标志位、锁与 CTE 状态。
pub struct preprocessor {
    /// 列类型标志位。
    pub flag: preprocessorFlag,
    pub stmtTp: u8,
    pub showTp: u8,
    pub PreprocessorReturn: PreprocessorReturn,
    pub tableAliasInJoin: Vec<HashMap<String, String>>,
    pub preprocessWith: preprocessWith,
    pub lockSelectCtxStack: Vec<lockSelectCtx>,
    pub current_db: String,
    pub known_tables: HashMap<String, TableInfo>,
    pub varsMutable: HashSet<String>,
    pub varsReadonly: HashSet<String>,
    pub warnings: Vec<String>,
    pub err: Option<BuilderError>,
}

/// 在非 FOR UPDATE 的 SELECT 外包一层 Limit（如资源管控）。
pub fn TryAddExtraLimit(
    statement: &Statement,
    select_limit: u64,
    restricted_sql: bool,
) -> Statement {
    if restricted_sql || select_limit == u64::MAX {
        return statement.clone();
    }
    match statement {
        Statement::Select { plan, for_update } if !matches!(plan.kind, PlanKind::Limit) => {
            let mut limit = PlanNode::new(PlanKind::Limit);
            limit.count = select_limit;
            limit.children.push(plan.clone());
            Statement::Select {
                plan: limit,
                for_update: *for_update,
            }
        }
        Statement::Explain {
            format,
            analyze,
            explore,
            stmt,
        } => Statement::Explain {
            format: format.clone(),
            analyze: *analyze,
            explore: *explore,
            stmt: Box::new(TryAddExtraLimit(stmt, select_limit, restricted_sql)),
        },
        _ => statement.clone(),
    }
}

/// 对节点执行预处理：语法校验与上下文解析入口。
pub fn Preprocess(
    node: &PreprocessNode,
    known_tables: HashMap<String, TableInfo>,
    options: &[&PreprocessOpt],
) -> Result<PreprocessorReturn> {
    let mut p = preprocessor {
        known_tables,
        ..preprocessor::default()
    };
    for option in options {
        option(&mut p);
    }
    p.Enter(node);
    if let Some(error) = p.err.take() {
        return Err(error);
    }
    p.Leave(node);
    if let Some(error) = p.err.take() {
        return Err(error);
    }
    p.ensureInfoSchema();
    Ok(p.PreprocessorReturn)
}

impl preprocessor {
    /// Record a user-variable visit using the same case-insensitive state machine as Go.
    fn recordUserVariable(&mut self, name: &str, assigned: bool) {
        let name = name.to_ascii_lowercase();
        if assigned {
            self.varsMutable.insert(name.clone());
            self.varsReadonly.remove(&name);
        } else if matches!(
            self.stmtTp,
            TypeSelect | TypeUpdate | TypeInsert | TypeDelete
        ) && !self.varsMutable.contains(&name)
        {
            self.varsReadonly.insert(name);
        }
    }

    /// 记录失败结果并返回是否应中止遍历。
    fn fail(&mut self, result: Result<()>) {
        if self.err.is_none() {
            if let Err(error) = result {
                self.err = Some(error);
            }
        }
    }
    /// 进入节点时的预处理钩子（对应 AST visitor Enter）。
    pub fn Enter(&mut self, node: &PreprocessNode) -> bool {
        if self.err.is_some() {
            return true;
        }
        match node {
            PreprocessNode::UserVariable { name, assigned } => {
                self.recordUserVariable(name, *assigned)
            }
            PreprocessNode::CreateDatabase { name } => {
                let r = self.checkCreateDatabaseGrammar(name);
                self.fail(r);
            }
            PreprocessNode::AlterDatabase { name } => {
                let r = self.checkAlterDatabaseGrammar(name);
                self.fail(r);
            }
            PreprocessNode::DropDatabase { name } => {
                let r = self.checkDropDatabaseGrammar(name);
                self.fail(r);
            }
            PreprocessNode::CreateTable { .. }
            | PreprocessNode::CreateView { .. }
            | PreprocessNode::DropTables { .. }
            | PreprocessNode::DropSequences(_)
            | PreprocessNode::RenameTables(_)
            | PreprocessNode::AlterTable { .. } => self.flag |= inCreateOrDropTable,
            PreprocessNode::RepairTable { .. } => self.flag |= inRepairTable,
            PreprocessNode::Statement(Statement::ImportInto { .. }) => self.flag |= inImportInto,
            PreprocessNode::Statement(Statement::Analyze(_)) => self.flag |= inAnalyze,
            PreprocessNode::CreateSequence(_) => self.flag |= inSequenceFunction,
            PreprocessNode::Select { lock_tables, .. } => {
                self.flag |= parentIsJoin;
                self.pushLockSelectCtx(lock_tables);
            }
            _ => {}
        }
        false
    }
    /// 离开节点时的预处理钩子（对应 AST visitor Leave）。
    pub fn Leave(&mut self, node: &PreprocessNode) -> bool {
        if self.err.is_some() {
            return false;
        }
        let result = match node {
            PreprocessNode::CreateTable {
                table,
                columns,
                constraints,
                options,
            } => self.checkCreateTableGrammar(table, columns, constraints, options),
            PreprocessNode::CreateView {
                name,
                columns,
                select_fields,
            } => self.checkCreateViewGrammar(name, columns, *select_fields),
            PreprocessNode::CreateIndex {
                table,
                name,
                parts,
                option,
            } => self.checkCreateIndexGrammar(table, name, parts, option),
            PreprocessNode::DropTables {
                tables,
                temporary_only,
            } => self.checkDropTableGrammar(tables, *temporary_only),
            PreprocessNode::DropSequences(tables) => self.checkDropSequenceGrammar(tables),
            PreprocessNode::RenameTables(pairs) => self.checkRenameTableGrammar(pairs),
            PreprocessNode::RepairTable { table, create } => {
                self.checkRepairTableGrammar(table, create)
            }
            PreprocessNode::AlterTable { table, operations } => {
                self.checkAlterTableGrammar(table, operations)
            }
            PreprocessNode::Select {
                tables,
                lock_tables,
                aliases,
                noop_functions,
                group_by_positions,
            } => {
                let result = self.checkSelectGrammar(
                    tables,
                    lock_tables,
                    aliases,
                    noop_functions,
                    group_by_positions,
                );
                self.popLockSelectCtx();
                self.flag &= !parentIsJoin;
                result
            }
            PreprocessNode::Binding {
                origin_type,
                hinted_type,
                origin_db,
                hinted_db,
            } => self.checkBindGrammar(*origin_type, *hinted_type, origin_db, hinted_db),
            PreprocessNode::Show { table } => {
                self.resolveShowStmt(table.as_ref());
                Ok(())
            }
            PreprocessNode::Execute { name } => self.resolveExecuteStmt(name),
            PreprocessNode::CreateSequence(table) => self.resolveCreateSequenceStmt(table),
            PreprocessNode::Cast {
                target,
                flen,
                decimal,
            } => self.checkFuncCastExpr(target, *flen, *decimal),
            _ => Ok(()),
        };
        self.fail(result);
        self.err.is_none()
    }
    /// 按名在当前 InfoSchema 中查找表元数据。
    pub fn tableByName(&self, tn: &TableName) -> Result<&TableInfo> {
        self.known_tables
            .get(&tn.key())
            .or_else(|| self.known_tables.get(&tn.name.to_ascii_lowercase()))
            .ok_or_else(|| BuilderError(format!("table {} does not exist", tn.key())))
    }
    /// 校验 SQL BINDING 相关语法。
    pub fn checkBindGrammar(
        &self,
        origin: u8,
        hinted: u8,
        origin_db: &str,
        hinted_db: &str,
    ) -> Result<()> {
        if origin == 0 || origin != hinted {
            return Err(BuilderError(
                "binding SQL statement types do not match".into(),
            ));
        }
        if !origin_db.is_empty()
            && !hinted_db.is_empty()
            && !origin_db.eq_ignore_ascii_case(hinted_db)
        {
            return Err(BuilderError(
                "binding SQL default databases do not match".into(),
            ));
        }
        Ok(())
    }
    /// 校验 CREATE DATABASE 名称合法性。
    pub fn checkCreateDatabaseGrammar(&self, name: &str) -> Result<()> {
        checkObjectName("database", name)
    }
    /// 校验 ALTER DATABASE 名称合法性。
    pub fn checkAlterDatabaseGrammar(&self, name: &str) -> Result<()> {
        checkObjectName("database", name)
    }
    /// 校验 DROP DATABASE 名称合法性。
    pub fn checkDropDatabaseGrammar(&self, name: &str) -> Result<()> {
        checkObjectName("database", name)
    }
    /// 校验 FLASHBACK TABLE 源/目标名。
    pub fn checkFlashbackTableGrammar(&self, source: &TableName, target: &TableName) -> Result<()> {
        checkObjectName("table", &source.name)?;
        checkObjectName("table", &target.name)?;
        if source.key() == target.key() {
            return Err(BuilderError(
                "flashback source and target must differ".into(),
            ));
        }
        Ok(())
    }
    /// 校验 FLASHBACK DATABASE 源/目标名。
    pub fn checkFlashbackDatabaseGrammar(&self, source: &str, target: &str) -> Result<()> {
        checkObjectName("database", source)?;
        checkObjectName("database", target)?;
        if source.eq_ignore_ascii_case(target) {
            return Err(BuilderError(
                "flashback source and target must differ".into(),
            ));
        }
        Ok(())
    }
    /// 校验 ADMIN CHECK TABLE 表名列表。
    pub fn checkAdminCheckTableGrammar(&self, tables: &[TableName]) -> Result<()> {
        if tables.is_empty() {
            return Err(BuilderError("ADMIN CHECK TABLE requires a table".into()));
        }
        for table in tables {
            self.tableByName(table)?;
        }
        Ok(())
    }
    /// 校验 CREATE TABLE 列、约束与选项。
    pub fn checkCreateTableGrammar(
        &self,
        table: &TableName,
        columns: &[ColumnDef],
        constraints: &[Constraint],
        options: &[TableOption],
    ) -> Result<()> {
        checkObjectName("table", &table.name)?;
        if columns.is_empty() {
            return Err(BuilderError(
                "table must contain at least one column".into(),
            ));
        }
        let mut names = HashSet::new();
        for column in columns {
            checkColumn(column)?;
            if !names.insert(column.name.to_ascii_lowercase()) {
                return Err(BuilderError(format!("duplicate column {}", column.name)));
            }
        }
        self.checkAutoIncrement(columns, constraints)?;
        for constraint in constraints {
            self.checkConstraintGrammar(constraint, &names)?;
        }
        checkUnsupportedTableOptions(options)?;
        for option in options {
            if let TableOption::Engine(engine) = option {
                checkTableEngine(engine)?;
            }
        }
        Ok(())
    }
    /// 校验 CREATE VIEW 语法。
    pub fn checkCreateViewGrammar(
        &self,
        name: &TableName,
        columns: &[String],
        select_fields: usize,
    ) -> Result<()> {
        checkObjectName("view", &name.name)?;
        let mut seen = HashSet::new();
        for column in columns {
            checkObjectName("view column", column)?;
            if !seen.insert(column.to_ascii_lowercase()) {
                return Err(BuilderError(format!("duplicate view column {column}")));
            }
        }
        if !columns.is_empty() && columns.len() != select_fields {
            return Err(BuilderError(
                "view column count does not match SELECT list".into(),
            ));
        }
        Ok(())
    }
    /// 校验带 SELECT 的 CREATE VIEW。
    pub fn checkCreateViewWithSelect(&self, node: &PreprocessNode) -> Result<()> {
        if matches!(node, PreprocessNode::Select { .. }) {
            Ok(())
        } else {
            Err(BuilderError("view definition must be a SELECT".into()))
        }
    }
    /// 校验 CREATE VIEW 的 SELECT 语法细节。
    pub fn checkCreateViewWithSelectGrammar(&self, node: &PreprocessNode) -> Result<()> {
        self.checkCreateViewWithSelect(node)
    }
    /// 校验 DROP SEQUENCE 表名列表。
    pub fn checkDropSequenceGrammar(&self, tables: &[TableName]) -> Result<()> {
        if tables.is_empty() {
            return Err(BuilderError("DROP SEQUENCE requires a name".into()));
        }
        for t in tables {
            if !t.sequence {
                return Err(BuilderError(format!("{} is not a sequence", t.name)));
            }
        }
        Ok(())
    }
    /// 校验 DROP TABLE（可限制仅临时表）。
    pub fn checkDropTableGrammar(&self, tables: &[TableName], temporary_only: bool) -> Result<()> {
        self.checkDropTableNames(tables)?;
        if temporary_only && tables.iter().any(|t| !t.temporary) {
            return Err(BuilderError(
                "DROP TEMPORARY TABLE contains a non-temporary table".into(),
            ));
        }
        Ok(())
    }
    /// 校验 DROP TEMPORARY TABLE。
    pub fn checkDropTemporaryTableGrammar(&self, tables: &[TableName]) -> Result<()> {
        self.checkDropTableGrammar(tables, true)
    }
    /// 校验待删除表名非空且合法。
    pub fn checkDropTableNames(&self, tables: &[TableName]) -> Result<()> {
        let mut seen = HashSet::new();
        for t in tables {
            if !seen.insert(t.key()) {
                return Err(BuilderError(format!("table {} specified twice", t.name)));
            }
        }
        if tables.is_empty() {
            Err(BuilderError("DROP TABLE requires a name".into()))
        } else {
            Ok(())
        }
    }
    /// 检查 FROM 中表别名是否唯一。
    pub fn checkNonUniqTableAlias(&self, tables: &[TableName]) -> Result<()> {
        let mut aliases = HashMap::new();
        for table in tables {
            let alias = table.visible_name();
            if let Some(previous) = aliases.insert(alias.clone(), table.key()) {
                return Err(BuilderError(format!(
                    "non-unique table alias {alias}: {previous}"
                )));
            }
        }
        Ok(())
    }
    /// 校验表中自增列定义是否合法。
    pub fn checkAutoIncrement(
        &self,
        columns: &[ColumnDef],
        constraints: &[Constraint],
    ) -> Result<()> {
        let mut found = None;
        for (index, column) in columns.iter().enumerate() {
            if checkAutoIncrementOp(column, index)? {
                if found.replace(index).is_some() {
                    return Err(BuilderError(
                        "there can be only one auto-increment column".into(),
                    ));
                }
            }
        }
        if let Some(index) = found {
            let name = &columns[index].name;
            let indexed = columns[index].options.contains(&ColumnOption::PrimaryKey)
                || columns[index].options.contains(&ColumnOption::UniqueKey)
                || constraints.iter().any(|c| {
                    (c.primary || c.unique)
                        && c.parts
                            .first()
                            .is_some_and(|p| p.column.eq_ignore_ascii_case(name))
                });
            if !indexed {
                return Err(BuilderError(
                    "auto-increment column must be indexed as the first key part".into(),
                ));
            }
        }
        Ok(())
    }
    /// 校验 UNION 等集合操作各分支字段数一致。
    pub fn checkSetOprSelectList(&self, branch_field_counts: &[usize]) -> Result<()> {
        if let Some(first) = branch_field_counts.first() {
            if branch_field_counts.iter().any(|count| count != first) {
                return Err(BuilderError(
                    "set-operation branches have different column counts".into(),
                ));
            }
        }
        Ok(())
    }
    /// 校验 CREATE INDEX 语法。
    pub fn checkCreateIndexGrammar(
        &self,
        table: &TableName,
        name: &str,
        parts: &[IndexPart],
        option: &IndexOption,
    ) -> Result<()> {
        self.tableByName(table)?;
        checkIndexInfo(name, parts)?;
        checkIndexOptions(false, option)?;
        checkIndexSpecs(option, parts)
    }
    /// 校验约束定义语法。
    pub fn checkConstraintGrammar(
        &self,
        constraint: &Constraint,
        columns: &HashSet<String>,
    ) -> Result<()> {
        checkIndexInfo(&constraint.name, &constraint.parts)?;
        checkIndexOptions(false, &constraint.option)?;
        for part in &constraint.parts {
            if part.expression.is_none() && !columns.contains(&part.column.to_ascii_lowercase()) {
                return Err(BuilderError(format!(
                    "key column {} does not exist",
                    part.column
                )));
            }
        }
        Ok(())
    }
    /// 检查 SELECT 中的空操作/禁用函数。
    pub fn checkSelectNoopFuncs(&mut self, functions: &[String]) -> Result<()> {
        for function in functions {
            let f = function.to_ascii_lowercase();
            if ["get_lock", "release_lock", "sleep"].contains(&f.as_str()) {
                self.warnings
                    .push(format!("function {function} has no effect in this context"));
            }
        }
        Ok(())
    }
    /// 校验 GROUP BY 位置引用合法性。
    pub fn checkGroupBy(&self, positions: &[i64]) -> Result<()> {
        if let Some(position) = positions.iter().find(|p| **p <= 0) {
            return Err(BuilderError(format!(
                "invalid GROUP BY position {position}"
            )));
        }
        Ok(())
    }
    /// 校验 RENAME TABLE 成对旧新名。
    pub fn checkRenameTableGrammar(&self, pairs: &[(TableName, TableName)]) -> Result<()> {
        if pairs.is_empty() {
            return Err(BuilderError("RENAME TABLE requires a pair".into()));
        }
        let mut targets = HashSet::new();
        for (old, new) in pairs {
            self.checkRenameTable(old, new)?;
            if !targets.insert(new.key()) {
                return Err(BuilderError(format!(
                    "duplicate rename target {}",
                    new.name
                )));
            }
        }
        Ok(())
    }
    /// 校验单次 RENAME 的旧表存在且新名合法。
    pub fn checkRenameTable(&self, old: &TableName, new: &TableName) -> Result<()> {
        checkObjectName("table", &old.name)?;
        checkObjectName("table", &new.name)?;
        if old.key() == new.key() {
            return Err(BuilderError("old and new table names are identical".into()));
        }
        Ok(())
    }
    /// 校验 REPAIR TABLE 语法。
    pub fn checkRepairTableGrammar(
        &self,
        table: &TableName,
        create: &PreprocessNode,
    ) -> Result<()> {
        let PreprocessNode::CreateTable { table: rebuilt, .. } = create else {
            return Err(BuilderError(
                "REPAIR TABLE requires CREATE TABLE definition".into(),
            ));
        };
        if !table.name.eq_ignore_ascii_case(&rebuilt.name) {
            return Err(BuilderError(
                "repair table name differs from CREATE TABLE".into(),
            ));
        }
        Ok(())
    }
    /// 校验 ALTER TABLE 操作列表。
    pub fn checkAlterTableGrammar(
        &self,
        table: &TableName,
        operations: &[AlterTableOperation],
    ) -> Result<()> {
        checkObjectName("table", &table.name)?;
        if operations.is_empty() {
            return Err(BuilderError("ALTER TABLE requires an operation".into()));
        }
        for op in operations {
            match op {
                AlterTableOperation::AddColumn(c) | AlterTableOperation::ModifyColumn(c) => {
                    checkColumn(c)?
                }
                AlterTableOperation::AddConstraint(c) => {
                    checkIndexInfo(&c.name, &c.parts)?;
                    checkIndexOptions(false, &c.option)?;
                }
                AlterTableOperation::RenameTo(t) => self.checkRenameTable(table, t)?,
                AlterTableOperation::Engine(e) => checkTableEngine(e)?,
                AlterTableOperation::DropColumn(c) => checkObjectName("column", c)?,
                AlterTableOperation::Unsupported(op) => {
                    return Err(BuilderError(format!(
                        "unsupported ALTER TABLE operation {op}"
                    )));
                }
            }
        }
        Ok(())
    }
    /// 校验 SELECT 基本语法约束。
    pub fn checkSelectGrammar(
        &mut self,
        tables: &[TableName],
        lock_tables: &[TableName],
        aliases: &[String],
        functions: &[String],
        positions: &[i64],
    ) -> Result<()> {
        self.checkNonUniqTableAlias(tables)?;
        self.checkGroupBy(positions)?;
        self.checkSelectNoopFuncs(functions)?;
        let mut all = tables.to_vec();
        for (table, alias) in all.iter_mut().zip(aliases) {
            table.alias = alias.clone();
        }
        let mut lock_ctx = newLockSelectCtx(lock_tables);
        for table in &all {
            lock_ctx.collect(table, &table.alias);
        }
        self.checkLockClauseTables(&lock_ctx)
    }
    /// 校验锁子句中的表均已出现在 FROM。
    pub fn checkLockClauseTables(&self, ctx: &lockSelectCtx) -> Result<()> {
        for name in ctx.lock_tables.keys() {
            if !ctx.used_tables.contains(name) {
                return Err(BuilderError(format!(
                    "table {name} in locking clause is not used"
                )));
            }
        }
        Ok(())
    }
    /// 压入一层锁选择上下文。
    pub fn pushLockSelectCtx(&mut self, lockTables: &[TableName]) {
        self.lockSelectCtxStack.push(newLockSelectCtx(lockTables));
    }
    /// 取锁选择上下文栈顶。
    pub fn getLockSelectCtxStackTop(&mut self) -> Option<&mut lockSelectCtx> {
        self.lockSelectCtxStack.last_mut()
    }
    /// 弹出锁选择上下文。
    pub fn popLockSelectCtx(&mut self) -> Option<lockSelectCtx> {
        self.lockSelectCtxStack.pop()
    }
    /// 确保表当前不在 repair 列表冲突中。
    pub fn checkNotInRepair(&self, tn: &TableName) -> Result<()> {
        if self.flag & inRepairTable != 0 && self.known_tables.contains_key(&tn.key()) {
            return Err(BuilderError(
                "table is already present while repairing".into(),
            ));
        }
        Ok(())
    }
    /// 处理 REPAIR 场景下的表名映射。
    pub fn handleRepairName(&mut self, tn: &TableName) {
        if self.current_db.is_empty() {
            self.current_db = tn.schema.clone();
        }
    }
    /// 解析并规范化表名引用。
    pub fn handleTableName(&mut self, tn: &TableName) -> Result<()> {
        if tn.schema.is_empty() && self.current_db.is_empty() {
            return Err(BuilderError(format!(
                "no database selected for {}",
                tn.name
            )));
        }
        self.preprocessWith.UpdateCTEConsumerCount(&tn.name);
        if self.flag & inCreateOrDropTable == 0 {
            self.tableByName(tn)?;
        }
        Ok(())
    }
    /// 解析 SHOW 语句关联的表（若有）。
    pub fn resolveShowStmt(&mut self, table: Option<&TableName>) {
        if let Some(table) = table {
            if self.current_db.is_empty() {
                self.current_db = table.schema.clone();
            }
        }
    }
    /// 解析 EXECUTE 预处理语句名。
    pub fn resolveExecuteStmt(&self, name: &str) -> Result<()> {
        checkObjectName("prepared statement", name)
    }
    /// 解析 CREATE TABLE 目标表名。
    pub fn resolveCreateTableStmt(&self, table: &TableName) -> Result<()> {
        checkObjectName("table", &table.name)
    }
    /// 解析 ALTER TABLE 目标表名。
    pub fn resolveAlterTableStmt(&self, table: &TableName) -> Result<()> {
        self.tableByName(table).map(|_| ())
    }
    /// 解析 CREATE SEQUENCE 目标名。
    pub fn resolveCreateSequenceStmt(&self, table: &TableName) -> Result<()> {
        if !table.sequence {
            return Err(BuilderError(
                "CREATE SEQUENCE target is not marked as sequence".into(),
            ));
        }
        checkObjectName("sequence", &table.name)
    }
    /// 校验 CAST 目标类型长度与精度。
    pub fn checkFuncCastExpr(&self, target: &str, flen: i32, decimal: i32) -> Result<()> {
        let target = target.to_ascii_lowercase();
        if ![
            "binary", "char", "date", "datetime", "decimal", "signed", "time", "unsigned", "json",
            "vector",
        ]
        .contains(&target.as_str())
        {
            return Err(BuilderError(format!("unsupported cast target {target}")));
        }
        if flen < -1 || decimal < -1 || (flen >= 0 && decimal > flen) {
            return Err(BuilderError("invalid CAST length or scale".into()));
        }
        Ok(())
    }
    /// 根据过期读处理器更新快照 TS 状态。
    pub fn updateStateFromStaleReadProcessor(&mut self, snapshot_ts: Option<u64>) -> Result<()> {
        if let Some(ts) = snapshot_ts {
            if ts == 0 {
                return Err(BuilderError("stale-read timestamp must be positive".into()));
            }
            self.PreprocessorReturn.IsStaleness = true;
            self.PreprocessorReturn.LastSnapshotTS = ts;
            self.PreprocessorReturn.initedLastSnapshotTS = true;
        }
        Ok(())
    }
    /// 确保已绑定 InfoSchema，返回其版本号。
    pub fn ensureInfoSchema(&mut self) -> u64 {
        if self.PreprocessorReturn.InfoSchemaVersion == 0 {
            self.PreprocessorReturn.InfoSchemaVersion = self
                .known_tables
                .values()
                .map(|t| t.id.unsigned_abs())
                .max()
                .unwrap_or(1);
        }
        self.PreprocessorReturn.InfoSchemaVersion
    }
    /// 判断列是否触发自动类型转换警告。
    pub fn hasAutoConvertWarning(&self, column: &ColumnDef) -> bool {
        column.data_type.eq_ignore_ascii_case("year")
            && column
                .options
                .iter()
                .any(|o| matches!(o, ColumnOption::Default(v) if v == "0"))
    }
    /// 是否跳过 MDL（元数据锁）加锁。
    pub fn skipLockMDL(&self) -> bool {
        self.flag & (inCreateOrDropTable | inTxnRetry | inPrepare) != 0
            || self.PreprocessorReturn.IsStaleness
    }
    /// 返回当前语句类型标签字符串。
    pub fn stmtType(&self) -> &'static str {
        match self.stmtTp {
            1 => "select",
            2 => "insert",
            3 => "ddl",
            _ => "unknown",
        }
    }
}

/// 就地去掉 SQL 末尾分号。
pub fn EraseLastSemicolon(sql: &mut String) {
    *sql = EraseLastSemicolonInSQL(sql);
}
/// 返回去掉末尾分号后的 SQL 副本。
pub fn EraseLastSemicolonInSQL(sql: &str) -> String {
    sql.strip_suffix(';').unwrap_or(sql).to_string()
}
/// 将语句映射为可绑定类型编码。
pub fn bindableStmtType(statement: &Statement) -> u8 {
    match statement {
        Statement::Select { .. } => TypeSelect,
        Statement::Insert(_) => TypeInsert,
        _ => TypeInvalid,
    }
}

/// 校验自增列选项组合是否合法，返回是否含自增。
pub fn checkAutoIncrementOp(column: &ColumnDef, _index: usize) -> Result<bool> {
    if !column.options.contains(&ColumnOption::AutoIncrement) {
        return Ok(false);
    }
    if column.generated {
        return Err(BuilderError(
            "generated column cannot be auto-increment".into(),
        ));
    }
    let ty = column.data_type.to_ascii_lowercase();
    if ![
        "tinyint",
        "smallint",
        "mediumint",
        "int",
        "integer",
        "bigint",
        "float",
        "double",
    ]
    .contains(&ty.as_str())
    {
        return Err(BuilderError(
            "auto-increment column must have an integer type".into(),
        ));
    }
    if column.options.iter().any(
        |option| matches!(option, ColumnOption::Default(value) if !value.eq_ignore_ascii_case("null")),
    ) {
        return Err(BuilderError(
            "auto-increment column cannot have a default".into(),
        ));
    }
    Ok(true)
}
/// 校验列选项冲突（如 NULL+NOT NULL），返回自增下标类信息。
pub fn checkColumnOptions(isTempTable: bool, options: &[ColumnOption]) -> Result<usize> {
    let mut seen = HashSet::new();
    let mut auto = 0;
    for option in options {
        let tag = std::mem::discriminant(option);
        if !seen.insert(tag) && !matches!(option, ColumnOption::Comment) {
            return Err(BuilderError("duplicate column option".into()));
        }
        if matches!(option, ColumnOption::AutoIncrement) {
            auto += 1;
        }
        if isTempTable && matches!(option, ColumnOption::AutoRandom) {
            return Err(BuilderError(
                "temporary table cannot use AUTO_RANDOM".into(),
            ));
        }
    }
    if options.contains(&ColumnOption::Null) && options.contains(&ColumnOption::NotNull) {
        return Err(BuilderError("NULL and NOT NULL conflict".into()));
    }
    Ok(auto)
}
/// 校验索引选项（列存索引不可为全局等）。
pub fn checkIndexOptions(isColumnar: bool, option: &IndexOption) -> Result<()> {
    if option.comment.len() > 1024 {
        return Err(BuilderError("index comment is too long".into()));
    }
    let ty = option.index_type.to_ascii_lowercase();
    if isColumnar {
        if ty.is_empty() {
            return Err(BuilderError(
                "columnar index must specify an index type".into(),
            ));
        }
        if !["vector", "inverted", "fulltext"].contains(&ty.as_str()) {
            return Err(BuilderError(format!(
                "unsupported index type {ty} for columnar index"
            )));
        }
        if option.invisible {
            return Err(BuilderError("columnar index cannot be invisible".into()));
        }
        if ty == "fulltext"
            && option
                .parser_name
                .as_deref()
                .is_some_and(|parser| !parser.is_empty() && parser != "standard")
        {
            return Err(BuilderError("unsupported fulltext parser".into()));
        }
    } else if ["hnsw", "vector", "inverted", "fulltext"].contains(&ty.as_str()) {
        return Err(BuilderError(format!(
            "index type {ty} requires the matching columnar index kind"
        )));
    } else if !ty.is_empty() && !["btree", "hash", "rtree", "hypo"].contains(&ty.as_str()) {
        return Err(BuilderError(format!("unsupported index type {ty}")));
    }
    Ok(())
}
/// 校验索引键部件规格。
pub fn checkIndexSpecs(_option: &IndexOption, parts: &[IndexPart]) -> Result<()> {
    if parts.is_empty() {
        return Err(BuilderError("index must contain a key part".into()));
    }
    checkDuplicateColumnName(parts)?;
    let ty = _option.index_type.to_ascii_lowercase();
    match ty.as_str() {
        "vector" if parts.len() != 1 || parts[0].expression.is_none() => {
            return Err(BuilderError(
                "vector index must specify exactly one expression".into(),
            ));
        }
        "inverted" | "fulltext" if parts.len() != 1 || parts[0].column.is_empty() => {
            return Err(BuilderError(format!(
                "{ty} index must specify exactly one column"
            )));
        }
        _ => {}
    }
    for part in parts {
        if part.column.is_empty() == part.expression.is_none() {
            return Err(BuilderError(
                "index part must contain exactly one column or expression".into(),
            ));
        }
        if part.length == Some(0) {
            return Err(BuilderError("index prefix length must be positive".into()));
        }
    }
    Ok(())
}
/// 检查索引部件中是否有重复列名。
pub fn checkDuplicateColumnName(parts: &[IndexPart]) -> Result<()> {
    let mut seen = HashSet::new();
    for part in parts {
        if !part.column.is_empty() && !seen.insert(part.column.to_ascii_lowercase()) {
            return Err(BuilderError(format!(
                "duplicate index column {}",
                part.column
            )));
        }
    }
    Ok(())
}
/// 综合校验索引名与部件。
pub fn checkIndexInfo(indexName: &str, parts: &[IndexPart]) -> Result<()> {
    checkObjectName("index", indexName)?;
    if parts.len() > 16 {
        return Err(BuilderError("too many key parts".into()));
    }
    checkDuplicateColumnName(parts)
}
/// 拒绝尚未支持的表选项。
pub fn checkUnsupportedTableOptions(options: &[TableOption]) -> Result<()> {
    for option in options {
        if let TableOption::Unsupported(name) = option {
            return Err(BuilderError(format!("unsupported table option {name}")));
        }
    }
    Ok(())
}
/// 校验存储引擎名是否允许。
pub fn checkTableEngine(engineName: &str) -> Result<()> {
    if [
        "archive",
        "blackhole",
        "csv",
        "example",
        "federated",
        "innodb",
        "memory",
        "merge",
        "mgr_myisam",
        "myisam",
        "ndb",
        "heap",
        "aria",
        "myrocks",
        "tokudb",
    ]
    .iter()
    .any(|e| e.eq_ignore_ascii_case(engineName))
    {
        Ok(())
    } else {
        Err(BuilderError(format!(
            "unsupported storage engine {engineName}"
        )))
    }
}
/// 临时表不得带外键等引用信息。
pub fn checkReferInfoForTemporaryTable(table: &TableInfo) -> Result<()> {
    if table.temporary && table.indices.iter().any(|i| i.global) {
        return Err(BuilderError(
            "temporary table cannot contain global indexes".into(),
        ));
    }
    Ok(())
}
/// 校验单列定义的完整性。
pub fn checkColumn(column: &ColumnDef) -> Result<()> {
    checkObjectName("column", &column.name)?;
    checkColumnOptions(false, &column.options)?;
    if column.data_type.is_empty() {
        return Err(BuilderError(format!("column {} has no type", column.name)));
    }
    if column.generated != column.options.contains(&ColumnOption::Generated) {
        return Err(BuilderError(
            "generated column marker and option disagree".into(),
        ));
    }
    if column.generated && !column.stored && column.options.contains(&ColumnOption::PrimaryKey) {
        return Err(BuilderError(
            "virtual generated column cannot be a primary key".into(),
        ));
    }
    if isInvalidDefaultValue(column) {
        return Err(BuilderError(format!(
            "invalid default value for {}",
            column.name
        )));
    }
    Ok(())
}
/// 默认值是否为 NOW 类符号函数。
pub fn isDefaultValNowSymFunc(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "current_timestamp" | "current_timestamp()" | "now()" | "localtime" | "localtimestamp"
    )
}
/// 判断列默认值是否非法。
pub fn isInvalidDefaultValue(column: &ColumnDef) -> bool {
    column.options.iter().any(|o| matches!(o, ColumnOption::Default(v) if v.eq_ignore_ascii_case("null") && column.options.contains(&ColumnOption::NotNull)))
}
/// 禁止列名含库.表.列形式的点号限定。
pub fn checkContainDotColumn(columns: &[ColumnDef]) -> Result<()> {
    if let Some(column) = columns.iter().find(|c| c.name.contains('.')) {
        Err(BuilderError(format!(
            "column name {} cannot contain a dot",
            column.name
        )))
    } else {
        Ok(())
    }
}
/// 检测表别名集合是否有冲突。
pub fn isTableAliasDuplicate(
    tables: &[TableName],
    tableAliases: &mut HashMap<String, String>,
) -> Result<()> {
    for table in tables {
        let alias = table.visible_name();
        if let Some(previous) = tableAliases.insert(alias.clone(), table.key()) {
            return Err(BuilderError(format!(
                "non-unique table alias {alias}: {previous}"
            )));
        }
    }
    Ok(())
}
/// 尝试加 MDL 并在需要时刷新 schema。
pub fn tryLockMDLAndUpdateSchemaIfNecessary(
    table: &TableInfo,
    schema_version: &mut u64,
    skip: bool,
) -> Result<TableInfo> {
    if table.id <= 0 {
        return Err(BuilderError("invalid table metadata id".into()));
    }
    if !skip {
        *schema_version = (*schema_version).max(table.id as u64);
    }
    Ok(table.clone())
}

/// 校验数据库对象名称合法性。
fn checkObjectName(kind: &str, name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 || name.chars().any(|c| c == '\0') {
        Err(BuilderError(format!("invalid {kind} name {name:?}")))
    } else {
        Ok(())
    }
}

/// 表别名冲突检查器。
pub struct aliasChecker;
impl aliasChecker {
    /// 进入节点时的预处理钩子（对应 AST visitor Enter）。
    pub fn Enter(&self, tables: &[TableName]) -> Result<()> {
        let mut seen = HashSet::new();
        for table in tables {
            if !seen.insert(table.visible_name()) {
                return Err(BuilderError(format!(
                    "duplicate table alias {}",
                    table.visible_name()
                )));
            }
        }
        Ok(())
    }
    /// 离开节点时的预处理钩子（对应 AST visitor Leave）。
    pub fn Leave(&self) -> bool {
        true
    }
}
/// 取表引用对外别名（无别名则用表名）。
pub fn getTableRefsAlias(table: &TableName) -> String {
    table.visible_name()
}
