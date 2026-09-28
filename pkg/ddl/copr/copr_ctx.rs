// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Coprocessor（协处理器）上下文模块：为 DDL 过程中的索引回填（add index
// backfill）构建下推到存储层的读取上下文。
//
// 背景：在分布式数据库中，"协处理器"指存储节点上就近执行计算的组件，
// 计算下推可以避免把整表数据拉回计算层。DDL 新建索引时需要扫描表数据、
// 组装索引列，本模块负责描述这次扫描需要哪些列、哪些是虚拟生成列、
// handle（行标识，即行在存储中的唯一 key）由哪些列构成等元信息。
//
// 主要内容：
// - `CopContextBase`：单表扫描所需的公共上下文（列信息、类型、偏移等）；
// - `CopContext` trait 及其两个实现：单索引 `CopContextSingleIndex` 与
//   多索引 `CopContextMultiIndex`（一次扫描同时回填多个索引）；
// - 一组辅助函数：解析索引条件涉及的列、去重、计算输出偏移等。
use std::collections::HashSet;
use std::fmt;
/// 隐藏 handle 列（`_tidb_rowid`）的固定列 ID。
/// 当表没有聚簇主键时，系统会自动生成一个隐藏的整型行 ID 作为 handle。
pub const ExtraHandleID: i64 = -1;
/// 隐藏 handle 列的列名。
pub const ExtraHandleName: &str = "_tidb_rowid";
/// 协处理器上下文构建过程中的错误类型，内部为错误描述字符串。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CopError(pub String);
impl fmt::Display for CopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CopError {}
/// 字段类型的简化表示，仅保留类型名称（如 "int64"）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FieldType {
    /// 类型名称字符串。
    pub TypeName: String,
}
/// 列元信息：描述表中一列的 ID、名称、偏移、类型与依赖关系。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnInfo {
    /// 列的全局唯一 ID。
    pub ID: i64,
    /// 列名。
    pub Name: String,
    /// 列在表定义中的位置（从 0 开始的偏移）。
    pub Offset: usize,
    /// 列的字段类型。
    pub FieldType: FieldType,
    /// 该列（生成列）依赖的其他列的列名列表。
    pub Dependences: Vec<String>,
    /// 是否为虚拟生成列（不落盘存储，读取时按表达式计算）。
    pub VirtualExpr: bool,
    /// 是否带有 Go `PriKeyFlag`（主键句柄场景按该标记定位主键列）。
    pub PrimaryKey: bool,
}
/// 索引列：通过偏移指向表中的某一列。
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct IndexColumn {
    /// 对应表列在 `TableInfo::Columns` 中的偏移。
    pub Offset: usize,
}
/// 索引元信息：索引 ID、组成列以及可选的过滤条件表达式。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexInfo {
    /// 索引的全局唯一 ID。
    pub ID: i64,
    /// 索引包含的列（按索引定义顺序）。
    pub Columns: Vec<IndexColumn>,
    /// 条件索引（部分索引）的过滤条件表达式文本，空串表示无条件。
    pub ConditionExprString: String,
}
impl IndexInfo {
    /// 判断索引是否带过滤条件（即是否为条件/部分索引）。
    pub fn HasCondition(&self) -> bool {
        !self.ConditionExprString.is_empty()
    }
}
/// 表元信息：列集合与主键/聚簇索引相关标记。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
    /// 表名。
    pub Name: String,
    /// 表的全部列定义。
    pub Columns: Vec<ColumnInfo>,
    /// 单列整型主键是否直接作为 handle（行标识）使用。
    pub PKIsHandle: bool,
    /// 是否使用 common handle（多列/非整型主键作为聚簇行标识）。
    pub IsCommonHandle: bool,
    /// common handle 时的主键索引信息。
    pub PrimaryIndex: Option<IndexInfo>,
}
impl TableInfo {
    /// 返回带主键标记的列；没有标记时保留旧 surrogate 数据的偏移 0 兜底。
    pub fn GetPkColInfo(&self) -> Option<&ColumnInfo> {
        self.Columns
            .iter()
            .find(|c| c.PrimaryKey)
            .or_else(|| self.Columns.iter().find(|c| c.Offset == 0))
    }
    /// 判断表是否有聚簇索引，保持 Go `TableInfo.HasClusteredIndex` 语义：
    /// 只有整型主键句柄或 common handle 才不需要隐藏行句柄。
    /// 没有聚簇索引的表需要额外的隐藏 `_tidb_rowid` 列充当 handle。
    pub fn HasClusteredIndex(&self) -> bool {
        self.PKIsHandle || self.IsCommonHandle
    }
}
/// 表达式构建上下文的占位类型（迁移基线中暂无实际内容）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BuildContext;
/// 已解析的条件表达式：表达式文本及其引用的列 ID 集合。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Expression {
    /// 表达式文本。
    pub Text: String,
    /// 表达式中引用到的列 ID 列表。
    pub ReferencedColumnIDs: Vec<i64>,
}
/// 表达式层的列表示：用于执行计划/表达式求值时定位列。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExprColumn {
    /// 列 ID。
    pub ID: i64,
    /// 列在输出行（chunk）中的下标。
    pub Index: usize,
    /// 是否为虚拟生成列。
    pub VirtualExpr: bool,
    /// 列类型。
    pub FieldType: FieldType,
}
/// 输出列的名称信息（表名 + 列名）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FieldName {
    /// 所属表名。
    pub TblName: String,
    /// 列名。
    pub ColName: String,
}
/// 列名列表的类型别名。
pub type NameSlice = Vec<FieldName>;
/// 输出模式（schema）：一次扫描输出的列集合。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Schema {
    /// 输出列。
    pub Columns: Vec<ExprColumn>,
}
/// 协处理器扫描的公共上下文：描述索引回填时表扫描所需的全部元信息。
#[derive(Clone, Debug)]
pub struct CopContextBase {
    /// 目标表的元信息。
    pub TableInfo: TableInfo,
    /// common handle 场景下的主键索引信息。
    pub PrimaryKeyInfo: Option<IndexInfo>,
    /// 表达式构建上下文。
    pub ExprCtx: BuildContext,
    /// 计算下推的标志位（控制哪些运算可以下推到存储层）。
    pub PushDownFlags: u64,
    /// 请求来源标识（用于存储层区分流量来源，如 DDL 内部任务）。
    pub RequestSource: String,
    /// 是否使用新排序规则（collation，即字符串比较/排序规则）框架。
    pub UseNewCollate: bool,
    /// 本次扫描实际需要读取的列（含依赖列与隐藏 handle 列）。
    pub ColumnInfos: Vec<ColumnInfo>,
    /// 与 `ColumnInfos` 一一对应的字段类型。
    pub FieldTypes: Vec<FieldType>,
    /// 表达式层的列描述（与 `ColumnInfos` 对应）。
    pub ExprColumnInfos: Vec<ExprColumn>,
    /// handle 各组成列在输出行中的偏移。
    pub HandleOutputOffsets: Vec<usize>,
    /// 各虚拟生成列在输出行中的偏移。
    pub VirtualColumnsOutputOffsets: Vec<usize>,
    /// 各虚拟生成列的字段类型。
    pub VirtualColumnsFieldTypes: Vec<FieldType>,
}
/// 协处理器上下文的统一接口，屏蔽单索引与多索引回填的差异。
pub trait CopContext {
    /// 返回公共上下文。
    fn GetBase(&self) -> &CopContextBase;
    /// 返回指定索引各列在扫描输出行中的偏移。
    fn IndexColumnOutputOffsets(&self, index_id: i64) -> Vec<usize>;
    /// 按索引 ID 查找索引元信息。
    fn IndexInfo(&self, index_id: i64) -> Option<&IndexInfo>;
    /// 解析索引的过滤条件表达式（条件索引场景），无条件时返回 `None`。
    fn GetCondition(&self) -> Result<Option<Expression>, CopError>;
}
/// 单索引回填的协处理器上下文。
pub struct CopContextSingleIndex {
    /// 公共上下文。
    pub CopContextBase: CopContextBase,
    /// 目标索引的元信息。
    idxInfo: IndexInfo,
    /// 索引各列在扫描输出行中的偏移。
    idxColOutputOffsets: Vec<usize>,
}
/// 多索引回填的协处理器上下文：一次表扫描同时为多个索引提供数据。
pub struct CopContextMultiIndex {
    /// 公共上下文。
    pub CopContextBase: CopContextBase,
    /// 全部目标索引的元信息。
    allIndexInfos: Vec<IndexInfo>,
    /// 每个索引对应的输出偏移列表（与 `allIndexInfos` 一一对应）。
    idxColOutputOffsets: Vec<Vec<usize>>,
}
/// 从条件表达式中提取列引用，并执行构造 CopContext 所需的最小语法校验。
///
/// Go 版本通过 SQL parser + expression rewriter 完成这项工作。这个 crate 的
/// 轻量 Rust 类型不依赖完整 parser，因此这里保留同样重要的契约：字符串常量
/// 不能被当作列名、限定列名只解析最后一段、未知列与未闭合表达式必须报错，且
/// 返回的列按表达式首次出现顺序去重。
fn condition_columns(condition: &str, table: &TableInfo) -> Result<Vec<IndexColumn>, CopError> {
    if condition.trim().is_empty() {
        return Err(CopError("condition expression is empty".into()));
    }

    let chars: Vec<char> = condition.chars().collect();
    let mut columns = Vec::new();
    let mut seen_offsets = HashSet::new();
    let mut parens = Vec::new();
    let mut i = 0;
    let mut saw_token = false;
    let mut expect_operand = true;

    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if chars[i] == '#' {
            i = skip_line_comment(&chars, i + 1);
            continue;
        }
        if chars[i] == '-' && chars.get(i + 1) == Some(&'-') {
            i = skip_line_comment(&chars, i + 2);
            continue;
        }
        if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            let mut closed = false;
            while i + 1 < chars.len() {
                if chars[i] == '*' && chars[i + 1] == '/' {
                    i += 2;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                return Err(CopError("unterminated condition comment".into()));
            }
            continue;
        }
        if matches!(chars[i], '\'' | '"') {
            i = skip_quoted(&chars, i, chars[i])?;
            saw_token = true;
            expect_operand = false;
            continue;
        }
        if chars[i] == '`' {
            let (name, next) = read_backtick_identifier(&chars, i)?;
            let (qualified, next) = read_qualified_tail(&chars, next, name);
            let is_function =
                next_non_whitespace(&chars, next).is_some_and(|position| chars[position] == '(');
            if !is_function {
                add_condition_column(&qualified, table, &mut columns, &mut seen_offsets)?;
                expect_operand = false;
            }
            saw_token = true;
            i = next;
            continue;
        }
        if is_identifier_start(chars[i]) {
            let (name, next) = read_identifier(&chars, i);
            let (qualified, next) = read_qualified_tail(&chars, next, name);
            let last_name = qualified.last().map(String::as_str).unwrap_or_default();
            let is_function =
                next_non_whitespace(&chars, next).is_some_and(|position| chars[position] == '(');
            if is_function || is_sql_keyword(last_name) {
                if is_sql_operator(last_name) {
                    expect_operand = true;
                } else if is_sql_literal(last_name) {
                    expect_operand = false;
                }
            } else if chars.get(i.wrapping_sub(1)) == Some(&'@') {
                expect_operand = false;
            } else {
                add_condition_column(&qualified, table, &mut columns, &mut seen_offsets)?;
                expect_operand = false;
            }
            saw_token = true;
            i = next;
            continue;
        }
        if chars[i].is_ascii_digit() {
            i = skip_number(&chars, i);
            saw_token = true;
            expect_operand = false;
            continue;
        }

        match chars[i] {
            '(' => {
                parens.push('(');
                expect_operand = true;
                saw_token = true;
                i += 1;
            }
            ')' => {
                if parens.pop().is_none() || expect_operand {
                    return Err(CopError("invalid condition expression".into()));
                }
                expect_operand = false;
                saw_token = true;
                i += 1;
            }
            ',' => {
                if expect_operand {
                    return Err(CopError("invalid condition expression".into()));
                }
                expect_operand = true;
                i += 1;
            }
            '+' | '-' | '*' | '/' | '%' | '=' | '<' | '>' | '!' | '&' | '|' | '^' | '~' | '?'
            | ':' => {
                if expect_operand && !matches!(chars[i], '+' | '-' | '~' | '*') {
                    return Err(CopError("invalid condition expression".into()));
                }
                expect_operand = true;
                saw_token = true;
                i += 1;
            }
            '[' | ']' => {
                saw_token = true;
                i += 1;
            }
            '.' | ';' => {
                return Err(CopError("invalid condition expression".into()));
            }
            _ => {
                return Err(CopError(format!(
                    "unsupported character {:?} in condition expression",
                    chars[i]
                )));
            }
        }
    }

    if !parens.is_empty() || !saw_token || expect_operand {
        return Err(CopError("invalid condition expression".into()));
    }
    Ok(columns)
}

fn is_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}

fn is_identifier_continue(ch: char) -> bool {
    is_identifier_start(ch) || ch.is_ascii_digit() || ch == '$'
}

fn read_identifier(chars: &[char], start: usize) -> (String, usize) {
    let mut end = start + 1;
    while end < chars.len() && is_identifier_continue(chars[end]) {
        end += 1;
    }
    (chars[start..end].iter().collect(), end)
}

fn read_backtick_identifier(chars: &[char], start: usize) -> Result<(String, usize), CopError> {
    let mut end = start + 1;
    let mut name = String::new();
    while end < chars.len() {
        if chars[end] == '`' {
            if chars.get(end + 1) == Some(&'`') {
                name.push('`');
                end += 2;
                continue;
            }
            return Ok((name, end + 1));
        }
        name.push(chars[end]);
        end += 1;
    }
    Err(CopError("unterminated quoted identifier".into()))
}

fn read_qualified_tail(chars: &[char], mut next: usize, first: String) -> (Vec<String>, usize) {
    let mut names = vec![first];
    loop {
        let Some(dot) = next_non_whitespace(chars, next) else {
            return (names, next);
        };
        if chars[dot] != '.' {
            return (names, next);
        }
        let Some(start) = next_non_whitespace(chars, dot + 1) else {
            return (names, next);
        };
        if chars[start] == '`' {
            let Ok((name, end)) = read_backtick_identifier(chars, start) else {
                return (names, next);
            };
            names.push(name);
            next = end;
        } else if is_identifier_start(chars[start]) {
            let (name, end) = read_identifier(chars, start);
            names.push(name);
            next = end;
        } else {
            return (names, next);
        }
    }
}

fn next_non_whitespace(chars: &[char], mut position: usize) -> Option<usize> {
    while position < chars.len() && chars[position].is_whitespace() {
        position += 1;
    }
    (position < chars.len()).then_some(position)
}

fn skip_line_comment(chars: &[char], mut position: usize) -> usize {
    while position < chars.len() && chars[position] != '\n' {
        position += 1;
    }
    position
}

fn skip_quoted(chars: &[char], start: usize, quote: char) -> Result<usize, CopError> {
    let mut position = start + 1;
    while position < chars.len() {
        if chars[position] == '\\' {
            position += 2;
            continue;
        }
        if chars[position] == quote {
            if chars.get(position + 1) == Some(&quote) {
                position += 2;
                continue;
            }
            return Ok(position + 1);
        }
        position += 1;
    }
    Err(CopError("unterminated condition string".into()))
}

fn skip_number(chars: &[char], mut position: usize) -> usize {
    while position < chars.len()
        && (chars[position].is_ascii_alphanumeric()
            || matches!(chars[position], '.' | '_' | '+' | '-'))
    {
        position += 1;
    }
    position
}

fn add_condition_column(
    qualified: &[String],
    table: &TableInfo,
    columns: &mut Vec<IndexColumn>,
    seen_offsets: &mut HashSet<usize>,
) -> Result<(), CopError> {
    let name = qualified.last().map(String::as_str).unwrap_or_default();
    let matches: Vec<&ColumnInfo> = table
        .Columns
        .iter()
        .filter(|column| column.Name.eq_ignore_ascii_case(name))
        .collect();
    let Some(column) = matches.first() else {
        return Err(CopError(format!("unknown column {name}")));
    };
    if matches.len() > 1 {
        return Err(CopError(format!("ambiguous column {name}")));
    }
    if seen_offsets.insert(column.Offset) {
        columns.push(IndexColumn {
            Offset: column.Offset,
        });
    }
    Ok(())
}

fn is_sql_keyword(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "and"
            | "as"
            | "between"
            | "binary"
            | "case"
            | "cast"
            | "collate"
            | "convert"
            | "div"
            | "else"
            | "end"
            | "exists"
            | "false"
            | "in"
            | "interval"
            | "is"
            | "like"
            | "mod"
            | "not"
            | "null"
            | "or"
            | "regexp"
            | "rlike"
            | "then"
            | "true"
            | "when"
            | "xor"
    )
}

fn is_sql_operator(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "and"
            | "between"
            | "div"
            | "in"
            | "is"
            | "like"
            | "mod"
            | "not"
            | "or"
            | "regexp"
            | "rlike"
            | "xor"
    )
}

fn is_sql_literal(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "false"
            | "null"
            | "true"
            | "current_date"
            | "current_time"
            | "current_timestamp"
            | "localtime"
            | "localtimestamp"
            | "now"
    )
}
/// 按列偏移去重，保留首次出现的顺序。
fn dedup(columns: Vec<IndexColumn>) -> Vec<IndexColumn> {
    let mut seen = HashSet::new();
    columns
        .into_iter()
        .filter(|c| seen.insert(c.Offset))
        .collect()
}
/// 收集索引列及其（生成列）依赖列的 ID，填入 `used` 集合。
/// 使用显式栈做传递闭包遍历：生成列可能依赖其他生成列，需要递归展开。
pub fn fillUsedColumns(
    mut used: HashSet<i64>,
    idx_cols: &[IndexColumn],
    table: &TableInfo,
) -> Result<HashSet<i64>, CopError> {
    // 先把索引直接引用的列按偏移解析为列信息，偏移越界视为错误
    let mut pending: Vec<ColumnInfo> = idx_cols
        .iter()
        .map(|c| {
            table
                .Columns
                .get(c.Offset)
                .cloned()
                .ok_or_else(|| CopError(format!("column offset {} out of range", c.Offset)))
        })
        .collect::<Result<_, _>>()?;
    // 深度优先展开依赖：已收录的列跳过，未收录的依赖列压栈继续处理
    while let Some(column) = pending.pop() {
        if !used.insert(column.ID) {
            continue;
        }
        for name in column.Dependences {
            let dependency = table
                .Columns
                .iter()
                .find(|c| c.Name.eq_ignore_ascii_case(&name))
                .cloned()
                .ok_or_else(|| CopError(format!("dependent column {name} not found")))?;
            if !used.contains(&dependency.ID) {
                pending.push(dependency)
            }
        }
    }
    Ok(used)
}
/// 构建公共协处理器上下文：确定需要读取的列集合、handle 组成列、
/// 输出偏移以及虚拟列信息。
pub fn NewCopContextBase(
    expr_ctx: BuildContext,
    flags: u64,
    table: TableInfo,
    idx_cols: &[IndexColumn],
    source: impl Into<String>,
    new_collate: bool,
) -> Result<CopContextBase, CopError> {
    // 收集索引列及其依赖列
    let mut used = fillUsedColumns(HashSet::new(), idx_cols, &table)?;
    let mut handles = vec![];
    // 按表的主键形态确定 handle 组成列：
    // 1) 整型主键即 handle；2) common handle 由主键索引各列组成；3) 其余无显式 handle
    let primary = if table.PKIsHandle {
        let pk = table
            .GetPkColInfo()
            .ok_or_else(|| CopError("primary key column not found".into()))?;
        used.insert(pk.ID);
        handles.push(pk.ID);
        None
    } else if table.IsCommonHandle {
        let index = table
            .PrimaryIndex
            .clone()
            .ok_or_else(|| CopError("primary index not found".into()))?;
        for col in &index.Columns {
            let info = table
                .Columns
                .get(col.Offset)
                .ok_or_else(|| CopError("primary index offset out of range".into()))?;
            handles.push(info.ID)
        }
        used = fillUsedColumns(used, &index.Columns, &table)?;
        Some(index)
    } else {
        None
    };
    // 按表定义顺序保留实际用到的列
    let mut columns: Vec<_> = table
        .Columns
        .iter()
        .filter(|c| used.contains(&c.ID))
        .cloned()
        .collect();
    // 非聚簇表需要追加隐藏的 _tidb_rowid 列作为 handle
    if !table.HasClusteredIndex() {
        columns.push(ColumnInfo {
            ID: ExtraHandleID,
            Name: ExtraHandleName.into(),
            Offset: table.Columns.len(),
            FieldType: FieldType {
                TypeName: "int64".into(),
            },
            ..Default::default()
        });
        handles = vec![ExtraHandleID]
    }
    // 生成表达式层列描述、字段类型以及 handle/虚拟列的输出偏移
    let expr_columns = columns
        .iter()
        .enumerate()
        .map(|(_i, c)| ExprColumn {
            ID: c.ID,
            Index: c.Offset,
            VirtualExpr: c.VirtualExpr,
            FieldType: c.FieldType.clone(),
        })
        .collect::<Vec<_>>();
    let fields = columns.iter().map(|c| c.FieldType.clone()).collect();
    let handle_offsets = resolveIndicesForHandle(&expr_columns, &handles);
    let (virtual_offsets, virtual_types) = collectVirtualColumnOffsetsAndTypes(&expr_columns);
    Ok(CopContextBase {
        TableInfo: table,
        PrimaryKeyInfo: primary,
        ExprCtx: expr_ctx,
        PushDownFlags: flags,
        RequestSource: source.into(),
        UseNewCollate: new_collate,
        ColumnInfos: columns,
        FieldTypes: fields,
        ExprColumnInfos: expr_columns,
        HandleOutputOffsets: handle_offsets,
        VirtualColumnsOutputOffsets: virtual_offsets,
        VirtualColumnsFieldTypes: virtual_types,
    })
}
/// 构建协处理器上下文工厂函数：按索引数量选择单索引或多索引实现。
pub fn NewCopContext(
    expr: BuildContext,
    flags: u64,
    table: TableInfo,
    indexes: Vec<IndexInfo>,
    source: impl Into<String>,
    collate: bool,
) -> Result<Box<dyn CopContext>, CopError> {
    let source = source.into();
    if indexes.len() == 1 {
        Ok(Box::new(NewCopContextSingleIndex(
            expr,
            flags,
            table,
            indexes.into_iter().next().unwrap(),
            source,
            collate,
        )?))
    } else {
        Ok(Box::new(NewCopContextMultiIndex(
            expr, flags, table, indexes, source, collate,
        )?))
    }
}
/// 构建单索引回填上下文：需要读取索引列以及条件表达式引用的列。
pub fn NewCopContextSingleIndex(
    expr: BuildContext,
    flags: u64,
    table: TableInfo,
    index: IndexInfo,
    source: impl Into<String>,
    collate: bool,
) -> Result<CopContextSingleIndex, CopError> {
    // 需要的列 = 索引列 + 条件表达式引用的列（去重后传入基础上下文）
    let mut columns = index.Columns.clone();
    if index.HasCondition() {
        columns.extend(condition_columns(&index.ConditionExprString, &table)?);
    }
    let base = NewCopContextBase(
        expr,
        flags,
        table,
        dedup(columns).as_slice(),
        source,
        collate,
    )?;
    let offsets = resolveIndicesForIndex(&base.ExprColumnInfos, &index, &base.TableInfo);
    Ok(CopContextSingleIndex {
        CopContextBase: base,
        idxInfo: index,
        idxColOutputOffsets: offsets,
    })
}
/// 构建多索引回填上下文：合并所有索引的列与条件引用列，一次扫描共用。
pub fn NewCopContextMultiIndex(
    expr: BuildContext,
    flags: u64,
    table: TableInfo,
    indexes: Vec<IndexInfo>,
    source: impl Into<String>,
    collate: bool,
) -> Result<CopContextMultiIndex, CopError> {
    // 汇总每个索引的索引列与条件列，去重后共同决定扫描列集合
    let mut columns = vec![];
    for index in &indexes {
        columns.extend(index.Columns.clone());
        if index.HasCondition() {
            columns.extend(condition_columns(&index.ConditionExprString, &table)?);
        }
    }
    let base = NewCopContextBase(
        expr,
        flags,
        table,
        dedup(columns).as_slice(),
        source,
        collate,
    )?;
    // 分别计算每个索引各列在输出行中的偏移
    let offsets = indexes
        .iter()
        .map(|index| resolveIndicesForIndex(&base.ExprColumnInfos, index, &base.TableInfo))
        .collect();
    Ok(CopContextMultiIndex {
        CopContextBase: base,
        allIndexInfos: indexes,
        idxColOutputOffsets: offsets,
    })
}
/// 解析条件索引的过滤表达式：
/// - 空表达式返回 `None`；
/// - 引用了虚拟生成列时不下推，返回 `None`（虚拟列存储层无法计算）；
/// - 括号不配对视为非法表达式并报错。
fn parse_condition(base: &CopContextBase, text: &str) -> Result<Option<Expression>, CopError> {
    if text.is_empty() {
        return Ok(None);
    }
    // 解析表达式并收集引用列；一旦涉及虚拟列则放弃下推。
    let referenced_columns = condition_columns(text, &base.TableInfo)?;
    let mut referenced = Vec::with_capacity(referenced_columns.len());
    for index_column in referenced_columns {
        let column = base
            .TableInfo
            .Columns
            .get(index_column.Offset)
            .ok_or_else(|| CopError("condition column offset out of range".into()))?;
        if column.VirtualExpr {
            return Ok(None);
        }
        referenced.push(column.ID);
    }
    Ok(Some(Expression {
        Text: text.into(),
        ReferencedColumnIDs: referenced,
    }))
}
impl CopContext for CopContextSingleIndex {
    fn GetBase(&self) -> &CopContextBase {
        &self.CopContextBase
    }
    // 单索引场景忽略传入的索引 ID，直接返回唯一索引的信息
    fn IndexColumnOutputOffsets(&self, _: i64) -> Vec<usize> {
        self.idxColOutputOffsets.clone()
    }
    fn IndexInfo(&self, _: i64) -> Option<&IndexInfo> {
        Some(&self.idxInfo)
    }
    fn GetCondition(&self) -> Result<Option<Expression>, CopError> {
        parse_condition(&self.CopContextBase, &self.idxInfo.ConditionExprString)
    }
}
impl CopContext for CopContextMultiIndex {
    fn GetBase(&self) -> &CopContextBase {
        &self.CopContextBase
    }
    fn IndexColumnOutputOffsets(&self, id: i64) -> Vec<usize> {
        self.allIndexInfos
            .iter()
            .position(|i| i.ID == id)
            .map(|i| self.idxColOutputOffsets[i].clone())
            .unwrap_or_default()
    }
    fn IndexInfo(&self, id: i64) -> Option<&IndexInfo> {
        self.allIndexInfos.iter().find(|i| i.ID == id)
    }
    // 多索引场景：把各索引的条件用 OR 拼接成一个整体过滤条件；
    // 只要任一索引无条件（或条件不可下推），整体就无法过滤，返回 None
    fn GetCondition(&self) -> Result<Option<Expression>, CopError> {
        let mut expressions = vec![];
        let mut ids = vec![];
        for index in &self.allIndexInfos {
            let Some(expression) =
                parse_condition(&self.CopContextBase, &index.ConditionExprString)?
            else {
                return Ok(None);
            };
            expressions.push(format!("({})", expression.Text));
            ids.extend(expression.ReferencedColumnIDs)
        }
        if expressions.is_empty() {
            Ok(None)
        } else {
            // 引用列 ID 排序去重后返回
            ids.sort_unstable();
            ids.dedup();
            Ok(Some(Expression {
                Text: expressions.join(" OR "),
                ReferencedColumnIDs: ids,
            }))
        }
    }
}
/// 计算索引各列在输出列集合中的位置：
/// 先按索引列偏移找到表列，再按列 ID 在输出列中定位。
pub fn resolveIndicesForIndex(
    output: &[ExprColumn],
    index: &IndexInfo,
    table: &TableInfo,
) -> Vec<usize> {
    index
        .Columns
        .iter()
        .filter_map(|idx| table.Columns.get(idx.Offset))
        .filter_map(|info| output.iter().position(|column| column.ID == info.ID))
        .collect()
}
/// 计算 handle 组成列（按列 ID）在输出列集合中的位置。
pub fn resolveIndicesForHandle(columns: &[ExprColumn], ids: &[i64]) -> Vec<usize> {
    ids.iter()
        .filter_map(|id| columns.iter().position(|column| column.ID == *id))
        .collect()
}
/// 收集所有虚拟生成列在输出中的偏移及其字段类型。
pub fn collectVirtualColumnOffsetsAndTypes(columns: &[ExprColumn]) -> (Vec<usize>, Vec<FieldType>) {
    columns
        .iter()
        .enumerate()
        .filter(|(_, c)| c.VirtualExpr)
        .map(|(i, c)| (i, c.FieldType.clone()))
        .unzip()
}
impl CopContextBase {
    /// 生成扫描输出的 schema 与列名列表：
    /// 列下标重排为输出顺序，隐藏 handle 列使用固定名 `_tidb_rowid`。
    pub fn GetSchemaAndNames(&self) -> (Schema, NameSlice) {
        let mut columns = vec![];
        let mut names = vec![];
        for (i, column) in self.ExprColumnInfos.iter().enumerate() {
            // 输出列的 Index 改写为其在输出行中的实际位置
            let mut cloned = column.clone();
            cloned.Index = i;
            let name = if column.ID == ExtraHandleID {
                ExtraHandleName.into()
            } else {
                self.TableInfo
                    .Columns
                    .get(column.Index)
                    .map(|c| c.Name.clone())
                    .unwrap_or_default()
            };
            columns.push(cloned);
            names.push(FieldName {
                TblName: self.TableInfo.Name.clone(),
                ColName: name,
            });
        }
        (Schema { Columns: columns }, names)
    }
}
