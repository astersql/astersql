// Copyright 2026 AsterSQL.
//! 本文件集中放置 `cmd/importer` 在迁移期依赖的本地桩。
//! 目标是以最小成本保留 Go 命令的可观察行为，而不是复制全部 TiDB 能力。
//! 因此这里的结构普遍只有 importer 会读到的字段和方法。
//! 错误、日志、退出语义先被抽成轻量门面，保证调用层不用改控制流。
//! SQL AST 与 `TableInfo` 也只保留建表、建索引和统计绑定所需的信息。
//! 时间工具覆盖 civil time 运算与格式化，避免引入额外平台依赖。
//! 统计工具只关心桶、边界和简单 JSON 形状，重点服务随机造数。
//! 数据库桩则强调“记录执行了什么”和“何时提交/关闭”，方便 parity test。
//! 随机数桩使用线程本地全局源，模拟 Go 默认随机源的使用方式。
//! TOML 覆盖器同样只支持 importer 实际出现的键，避免变成通用解析器。
//! 维护这类桩时最重要的是守住外部契约，而不是一味增加功能。
//! 如果未来真实依赖可直接接入，应先确认这些桩暴露的语义已经被覆盖。
//! 注释中会反复强调哪些行为是为了对齐 Go，哪些行为只是本地近似实现。
//! 这样后续读者才能区分“必须一致的契约”和“可替换的实现细节”。
//! 进程退出适配在生产构建中必须保留 Go `os.Exit` 的真实状态码，
//! 测试构建则提供隔离探针，避免直接终止整个测试进程。
//! 阅读本文件时，可以把它理解为 importer 的迁移兼容层。
//! 上层命令借助它运行在 arm64 本地与测试环境，下层真实依赖则被有意隔离。
//! 接下来的逐符号中文注释会围绕这种“最小可用兼容层”视角展开。

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// --- Error / Result / logging (terror / pingcap/log / zap) ---

#[derive(Clone, Debug, PartialEq, Eq)]
/// `Error` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct Error {
    pub msg: String,
    pub is_help: bool,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Error {
    /// `new` 用统一入口构造普通错误，便于调用层保持与 Go 类似的错误传递方式。
    /// 这里默认不标记帮助态，让帮助分支仍由 `help` 显式表达。
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            is_help: false,
        }
    }

    /// `help` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn help() -> Self {
        Self {
            msg: "help requested".into(),
            is_help: true,
        }
    }

    /// `Error` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn Error(&self) -> &str {
        &self.msg
    }
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// `cause` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn cause(err: &Error) -> &Error {
    err
}

/// `trace` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn trace(err: Error) -> Error {
    err
}

/// `errorf` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn errorf(msg: impl Into<String>) -> Error {
    Error::new(msg)
}

/// Go `log.Fatal` — process exit semantics via panic (catchable in tests).
/// `fatal` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn fatal(msg: impl AsRef<str>) -> ! {
    panic!("{}", msg.as_ref());
}

/// `log_error` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn log_error(msg: impl AsRef<str>) {
    let _ = writeln!(io::stderr(), "[ERROR] {}", msg.as_ref());
}

/// `log_warn` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn log_warn(msg: impl AsRef<str>) {
    let _ = writeln!(io::stderr(), "[WARN] {}", msg.as_ref());
}

static PROCESS_EXIT_CODE: OnceLock<Mutex<Option<i32>>> = OnceLock::new();

// `exit_slot` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn exit_slot() -> &'static Mutex<Option<i32>> {
    PROCESS_EXIT_CODE.get_or_init(|| Mutex::new(None))
}

/// Go `os.Exit` — exits with the requested status in production.
///
/// Unit tests retain a catchable panic because terminating the test harness
/// would make the entrypoint error branches impossible to exercise in-process.
/// `os_exit` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn os_exit(code: i32) -> ! {
    #[cfg(test)]
    {
        if let Ok(mut g) = exit_slot().lock() {
            *g = Some(code);
        }
        panic!("os.Exit({code})");
    }

    #[cfg(not(test))]
    {
        exit_process(code);
    }
}

fn exit_process(code: i32) -> ! {
    std::process::exit(code);
}

#[cfg(test)]
pub fn exit_process_for_test(code: i32) -> ! {
    exit_process(code);
}

/// `take_exit_code` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn take_exit_code() -> Option<i32> {
    exit_slot().lock().ok().and_then(|mut g| g.take())
}

/// `args_from_env` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn args_from_env() -> Vec<String> {
    std::env::args().skip(1).collect()
}

// --- MySQL type constants (pkg/parser/mysql) ---

// `TypeTiny` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const TypeTiny: u8 = 1;
pub const TypeShort: u8 = 2;
// `TypeLong` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const TypeLong: u8 = 3;
pub const TypeFloat: u8 = 4;
// `TypeDouble` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const TypeDouble: u8 = 5;
pub const TypeTimestamp: u8 = 7;
// `TypeLonglong` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const TypeLonglong: u8 = 8;
pub const TypeInt24: u8 = 9;
pub const TypeDate: u8 = 10;
// `TypeDuration` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const TypeDuration: u8 = 11;
pub const TypeDatetime: u8 = 12;
// `TypeYear` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const TypeYear: u8 = 13;
pub const TypeVarchar: u8 = 15;
// `TypeNewDecimal` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const TypeNewDecimal: u8 = 0xf6;
pub const TypeTinyBlob: u8 = 0xf9;
// `TypeMediumBlob` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const TypeMediumBlob: u8 = 0xfa;
pub const TypeLongBlob: u8 = 0xfb;
// `TypeBlob` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const TypeBlob: u8 = 0xfc;
pub const TypeString: u8 = 0xfe;

// `UnsignedFlag` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const UnsignedFlag: usize = 1 << 5;

/// `HasUnsignedFlag` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn HasUnsignedFlag(flag: usize) -> bool {
    flag & UnsignedFlag != 0
}

/// Go `types.FieldType` subset used by importer.
#[derive(Clone, Debug, Default)]
/// `FieldType` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct FieldType {
    pub tp: u8,
    pub flag: usize,
    pub flen: i32,
    pub decimal: i32,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl FieldType {
    /// `GetType` 返回 MySQL 类型编号，主要服务 importer 后续按 Go 约定做类型分派。
    /// 这里只暴露读取语义，不引入额外转换，避免桩层擅自改变类型判断结果。
    pub fn GetType(&self) -> u8 {
        self.tp
    }
    /// `GetFlag` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn GetFlag(&self) -> usize {
        self.flag
    }
    /// `GetFlen` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn GetFlen(&self) -> i32 {
        self.flen
    }
    /// `GetDecimal` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn GetDecimal(&self) -> i32 {
        self.decimal
    }
}

// --- model.TableInfo subset ---

#[derive(Clone, Debug, Default)]
/// `ColumnInfo` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct ColumnInfo {
    pub ID: i64,
    pub Name: String,
    pub Offset: usize,
    pub FieldType: FieldType,
}

#[derive(Clone, Debug, Default)]
/// `IndexColumn` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct IndexColumn {
    pub Name: String,
    pub Offset: usize,
}

#[derive(Clone, Debug, Default)]
/// `IndexInfo` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct IndexInfo {
    pub ID: i64,
    pub Name: String,
    pub Columns: Vec<IndexColumn>,
}

#[derive(Clone, Debug, Default)]
/// `TableInfo` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct TableInfo {
    pub ID: i64,
    pub Name: String,
    pub Columns: Vec<ColumnInfo>,
    pub Indices: Vec<IndexInfo>,
}

// --- AST subset for CREATE TABLE / CREATE INDEX ---

#[derive(Clone, Debug)]
/// `CIStr` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct CIStr {
    pub O: String,
    pub L: String,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl CIStr {
    /// `new` 同时保存原始大小写和值的小写副本，模拟 Go 侧 `model.CIStr` 的常见读取模式。
    /// 后续表名、列名匹配通常走小写字段，因此这里集中完成规范化。
    pub fn new(s: &str) -> Self {
        Self {
            O: s.to_string(),
            L: s.to_lowercase(),
        }
    }
}

#[derive(Clone, Debug)]
/// `ColumnOptionTp` 汇总当前场景下需要区分的有限状态。
/// 枚举成员的划分优先服务 Go 兼容分支，而不是追求更泛化的抽象。
/// 因此新增成员前应先确认上层是否真的需要新的可观察行为。
pub enum ColumnOptionTp {
    PrimaryKey,
    UniqKey,
    AutoIncrement,
    Comment,
    Other,
}

#[derive(Clone, Debug)]
/// `ColumnOption` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct ColumnOption {
    pub Tp: ColumnOptionTp,
    pub Comment: String,
}

#[derive(Clone, Debug)]
/// `ColumnDef` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct ColumnDef {
    pub Name: CIStr,
    pub Tp: FieldType,
    pub Options: Vec<ColumnOption>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// `ConstraintTp` 汇总当前场景下需要区分的有限状态。
/// 枚举成员的划分优先服务 Go 兼容分支，而不是追求更泛化的抽象。
/// 因此新增成员前应先确认上层是否真的需要新的可观察行为。
pub enum ConstraintTp {
    PrimaryKey,
    Key,
    Uniq,
    UniqKey,
    UniqIndex,
    Index,
    Other,
}

#[derive(Clone, Debug)]
/// `IndexPartSpecification` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct IndexPartSpecification {
    pub Column: CIStr,
}

#[derive(Clone, Debug)]
/// `Constraint` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct Constraint {
    pub Tp: ConstraintTp,
    pub Keys: Vec<IndexPartSpecification>,
}

#[derive(Clone, Debug)]
/// `CreateTableStmt` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct CreateTableStmt {
    pub Table: CIStr,
    pub Cols: Vec<ColumnDef>,
    pub Constraints: Vec<Constraint>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// `IndexKeyType` 汇总当前场景下需要区分的有限状态。
/// 枚举成员的划分优先服务 Go 兼容分支，而不是追求更泛化的抽象。
/// 因此新增成员前应先确认上层是否真的需要新的可观察行为。
pub enum IndexKeyType {
    None,
    Unique,
    Other,
}

#[derive(Clone, Debug)]
/// `CreateIndexStmt` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct CreateIndexStmt {
    pub Table: CIStr,
    pub KeyType: IndexKeyType,
    pub IndexPartSpecifications: Vec<IndexPartSpecification>,
    pub text: String,
}

#[derive(Clone, Debug)]
/// `StmtNode` 汇总当前场景下需要区分的有限状态。
/// 枚举成员的划分优先服务 Go 兼容分支，而不是追求更泛化的抽象。
/// 因此新增成员前应先确认上层是否真的需要新的可观察行为。
pub enum StmtNode {
    CreateTable(CreateTableStmt),
    CreateIndex(CreateIndexStmt),
    Other(String),
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl StmtNode {
    /// `Text` 保留解析前后的原 SQL 文本，方便上层在日志、错误或回放场景继续沿用输入字符串。
    /// 该接口不试图重新格式化 SQL，只返回创建节点时保存的内容。
    pub fn Text(&self) -> &str {
        match self {
            StmtNode::CreateTable(s) => &s.text,
            StmtNode::CreateIndex(s) => &s.text,
            StmtNode::Other(s) => s,
        }
    }
}

/// Minimal SQL tokenizer / parser for the CREATE TABLE / INDEX forms importer uses.
/// `parse_one_stmt` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn parse_one_stmt(sql: &str) -> Result<StmtNode> {
    let text = sql.trim().to_string();
    let lower = text.to_lowercase();
    let mut words = lower.split_whitespace();
    let first = words.next();
    let second = words.next();
    let third = words.next();
    if first == Some("create")
        && (second == Some("table") || (second == Some("temporary") && third == Some("table")))
    {
        parse_create_table(&text)
    } else if first == Some("create")
        && (second == Some("index") || (second == Some("unique") && third == Some("index")))
    {
        parse_create_index(&text)
    } else {
        Ok(StmtNode::Other(text))
    }
}

// `strip_trailing_semi` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn strip_trailing_semi(s: &str) -> &str {
    s.trim().trim_end_matches(';').trim()
}

// `parse_create_table` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn parse_create_table(sql: &str) -> Result<StmtNode> {
    let raw = strip_trailing_semi(sql);
    let lower = raw.to_lowercase();
    let table_pos = lower
        .find("table")
        .ok_or_else(|| Error::new("invalid create table"))?;
    let mut after_table = raw[table_pos + 5..].trim_start();
    if let Some(rest) = consume_keyword(after_table, "if") {
        let rest =
            consume_keyword(rest, "not").ok_or_else(|| Error::new("expected NOT after IF"))?;
        after_table = consume_keyword(rest, "exists")
            .ok_or_else(|| Error::new("expected EXISTS after IF NOT"))?;
    }
    let (name, rest) = split_ident(after_table)?;
    let rest = rest.trim_start();
    if !rest.starts_with('(') {
        return Err(Error::new("expected '(' after table name"));
    }
    let inner = extract_paren_list(rest)?;
    let mut cols = Vec::new();
    let mut constraints = Vec::new();
    for part in split_top_level_commas(&inner) {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        let pl = p.to_lowercase();
        if pl.starts_with("primary")
            || pl.starts_with("unique")
            || pl.starts_with("key")
            || pl.starts_with("index")
            || pl.starts_with("constraint")
        {
            constraints.push(parse_table_constraint(p)?);
        } else {
            cols.push(parse_column_def(p)?);
        }
    }
    Ok(StmtNode::CreateTable(CreateTableStmt {
        Table: CIStr::new(&name),
        Cols: cols,
        Constraints: constraints,
        text: sql.to_string(),
    }))
}

fn consume_keyword<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    let s = s.trim_start();
    let prefix = s.get(..keyword.len())?;
    if !prefix.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &s[keyword.len()..];
    if rest
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some(rest.trim_start())
}

// `parse_create_index` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn parse_create_index(sql: &str) -> Result<StmtNode> {
    let raw = strip_trailing_semi(sql);
    let lower = raw.to_lowercase();
    let mut key_type = IndexKeyType::None;
    let mut s = raw;
    // create [unique] index name on table (cols)
    let mut after_create = raw;
    if let Some(pos) = lower.find("create") {
        after_create = raw[pos + 6..].trim_start();
    }
    let al = after_create.to_lowercase();
    if al.starts_with("unique") {
        key_type = IndexKeyType::Unique;
        after_create = after_create[6..].trim_start();
    }
    let al = after_create.to_lowercase();
    if !al.starts_with("index") && !al.starts_with("key") {
        return Err(Error::new("expected INDEX"));
    }
    // skip index/key
    let sp = after_create
        .find(char::is_whitespace)
        .ok_or_else(|| Error::new("bad create index"))?;
    after_create = after_create[sp..].trim_start();
    // index name
    let (_idx_name, rest) = split_ident(after_create)?;
    let rest = rest.trim_start();
    let rl = rest.to_lowercase();
    if !rl.starts_with("on") {
        return Err(Error::new("expected ON"));
    }
    let rest = rest[2..].trim_start();
    let (table, rest) = split_ident(rest)?;
    let rest = rest.trim_start();
    if !rest.starts_with('(') {
        return Err(Error::new("expected column list"));
    }
    let inner = extract_paren_list(rest)?;
    let mut specs = Vec::new();
    for part in split_top_level_commas(&inner) {
        let name = part.trim().trim_matches('`');
        if name.is_empty() {
            continue;
        }
        // col (len) optional — take first token
        let col = name.split_whitespace().next().unwrap_or(name);
        let col = col.trim_matches('`');
        specs.push(IndexPartSpecification {
            Column: CIStr::new(col),
        });
    }
    let _ = s;
    Ok(StmtNode::CreateIndex(CreateIndexStmt {
        Table: CIStr::new(&table),
        KeyType: key_type,
        IndexPartSpecifications: specs,
        text: sql.to_string(),
    }))
}

// `parse_table_constraint` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn parse_table_constraint(p: &str) -> Result<Constraint> {
    let mut s = p.trim();
    let sl = s.to_lowercase();
    if sl.starts_with("constraint") {
        let rest = s[10..].trim_start();
        let (_n, r) = split_ident(rest)?;
        s = r.trim_start();
    }
    let sl = s.to_lowercase();
    let (tp, after) = if sl.starts_with("primary key") {
        (ConstraintTp::PrimaryKey, s[11..].trim_start())
    } else if sl.starts_with("unique index") {
        (ConstraintTp::UniqIndex, s[12..].trim_start())
    } else if sl.starts_with("unique key") {
        (ConstraintTp::UniqKey, s[10..].trim_start())
    } else if sl.starts_with("unique") {
        (ConstraintTp::Uniq, s[6..].trim_start())
    } else if sl.starts_with("index") {
        (ConstraintTp::Index, s[5..].trim_start())
    } else if sl.starts_with("key") {
        (ConstraintTp::Key, s[3..].trim_start())
    } else {
        return Ok(Constraint {
            Tp: ConstraintTp::Other,
            Keys: vec![],
        });
    };
    // optional constraint name before (
    let mut rest = after;
    if !rest.starts_with('(') {
        let (_n, r) = split_ident(rest)?;
        rest = r.trim_start();
    }
    let inner = extract_paren_list(rest)?;
    let mut keys = Vec::new();
    for part in split_top_level_commas(&inner) {
        let name = part
            .trim()
            .trim_matches('`')
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches('`');
        if !name.is_empty() {
            keys.push(IndexPartSpecification {
                Column: CIStr::new(name),
            });
        }
    }
    Ok(Constraint { Tp: tp, Keys: keys })
}

// `parse_column_def` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn parse_column_def(p: &str) -> Result<ColumnDef> {
    let (name, rest) = split_ident(p.trim())?;
    let rest = rest.trim_start();
    let (tp, mut rest) = parse_type(rest)?;
    let mut options = Vec::new();
    while !rest.is_empty() {
        let rl = rest.to_lowercase();
        if rl.starts_with("primary") && rl[7..].trim_start().starts_with("key") {
            options.push(ColumnOption {
                Tp: ColumnOptionTp::PrimaryKey,
                Comment: String::new(),
            });
            // skip "primary key"
            rest = skip_keywords(rest, &["primary", "key"]);
        } else if rl.starts_with("unique") {
            options.push(ColumnOption {
                Tp: ColumnOptionTp::UniqKey,
                Comment: String::new(),
            });
            rest = skip_keywords(rest, &["unique"]);
            let rl2 = rest.to_lowercase();
            if rl2.starts_with("key") || rl2.starts_with("index") {
                rest = skip_keywords(rest, &["key"]);
                let rl3 = rest.to_lowercase();
                if rl3.starts_with("index") {
                    rest = skip_keywords(rest, &["index"]);
                }
            }
        } else if rl.starts_with("auto_increment") {
            options.push(ColumnOption {
                Tp: ColumnOptionTp::AutoIncrement,
                Comment: String::new(),
            });
            rest = skip_keywords(rest, &["auto_increment"]);
        } else if rl.starts_with("comment") {
            rest = skip_keywords(rest, &["comment"]);
            let (cmt, r) = parse_string_literal(rest)?;
            options.push(ColumnOption {
                Tp: ColumnOptionTp::Comment,
                Comment: cmt,
            });
            rest = r.trim_start();
        } else if rl.starts_with("not") && rl[3..].trim_start().starts_with("null") {
            rest = skip_keywords(rest, &["not", "null"]);
        } else if rl.starts_with("null") {
            rest = skip_keywords(rest, &["null"]);
        } else if rl.starts_with("default") {
            rest = skip_keywords(rest, &["default"]);
            // skip next token / literal
            if rest.starts_with('\'') || rest.starts_with('"') {
                let (_, r) = parse_string_literal(rest)?;
                rest = r.trim_start();
            } else {
                let (_t, r) = split_ident(rest)?;
                rest = r.trim_start();
            }
        } else {
            // unknown token — skip one ident
            let (_t, r) = split_ident(rest)?;
            if r.len() == rest.len() {
                break;
            }
            rest = r.trim_start();
            options.push(ColumnOption {
                Tp: ColumnOptionTp::Other,
                Comment: String::new(),
            });
        }
    }
    let _ = tp;
    Ok(ColumnDef {
        Name: CIStr::new(&name),
        Tp: tp,
        Options: options,
    })
}

// `parse_type` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn parse_type(s: &str) -> Result<(FieldType, &str)> {
    let (type_name, mut rest) = split_ident(s)?;
    let mut ft = FieldType::default();
    let tn = type_name.to_lowercase();
    // optional (flen, decimal)
    let mut flen = -1i32;
    let mut decimal = 0i32;
    rest = rest.trim_start();
    if rest.starts_with('(') {
        let inner = extract_paren_list(rest)?;
        let close = rest.find(')').unwrap_or(0);
        rest = rest[close + 1..].trim_start();
        let parts: Vec<&str> = inner.split(',').map(|x| x.trim()).collect();
        if !parts.is_empty() && !parts[0].is_empty() {
            flen = parts[0].parse().unwrap_or(-1);
        }
        if parts.len() > 1 {
            decimal = parts[1].parse().unwrap_or(0);
        }
    }
    // unsigned
    let rl = rest.to_lowercase();
    if rl.starts_with("unsigned") {
        ft.flag |= UnsignedFlag;
        rest = skip_keywords(rest, &["unsigned"]);
    }
    match tn.as_str() {
        "tinyint" => ft.tp = TypeTiny,
        "smallint" => ft.tp = TypeShort,
        "int" | "integer" => ft.tp = TypeLong,
        "mediumint" => ft.tp = TypeInt24,
        "bigint" => ft.tp = TypeLonglong,
        "float" => ft.tp = TypeFloat,
        "double" | "real" => ft.tp = TypeDouble,
        "varchar" => {
            ft.tp = TypeVarchar;
            ft.flen = if flen < 0 { 1 } else { flen };
        }
        "char" => {
            ft.tp = TypeString;
            ft.flen = if flen < 0 { 1 } else { flen };
        }
        "tinytext" | "tinyblob" => {
            ft.tp = TypeTinyBlob;
            ft.flen = if flen < 0 { 255 } else { flen };
        }
        "text" | "blob" => {
            ft.tp = TypeBlob;
            ft.flen = if flen < 0 { 65_535 } else { flen };
        }
        "mediumblob" | "mediumtext" => {
            ft.tp = TypeMediumBlob;
            ft.flen = if flen < 0 { 16_777_215 } else { flen };
        }
        "longblob" | "longtext" => {
            ft.tp = TypeLongBlob;
            ft.flen = if flen < 0 { 16777215 } else { flen };
        }
        "date" => ft.tp = TypeDate,
        "time" => ft.tp = TypeDuration,
        "datetime" => ft.tp = TypeDatetime,
        "timestamp" => ft.tp = TypeTimestamp,
        "year" => ft.tp = TypeYear,
        "decimal" | "numeric" => {
            ft.tp = TypeNewDecimal;
            ft.flen = if flen < 0 { 10 } else { flen };
            ft.decimal = decimal;
        }
        _ => {
            return Err(Error::new(format!("unsupported type {type_name}")));
        }
    }
    if ft.flen < 0 && matches!(ft.tp, TypeVarchar | TypeString) {
        ft.flen = 1;
    }
    Ok((ft, rest))
}

// `split_ident` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn split_ident(s: &str) -> Result<(String, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return Err(Error::new("expected identifier"));
    }
    if s.starts_with('`') {
        let end = s[1..]
            .find('`')
            .ok_or_else(|| Error::new("unclosed ident"))?;
        let name = s[1..1 + end].to_string();
        Ok((name, &s[2 + end..]))
    } else {
        let mut end = 0;
        for (i, c) in s.char_indices() {
            if c.is_ascii_alphanumeric() || c == '_' {
                end = i + c.len_utf8();
            } else {
                break;
            }
        }
        if end == 0 {
            return Err(Error::new(format!("expected identifier at {s}")));
        }
        Ok((s[..end].to_string(), &s[end..]))
    }
}

// `skip_keywords<'a>` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn skip_keywords<'a>(s: &'a str, words: &[&str]) -> &'a str {
    let mut rest = s.trim_start();
    for w in words {
        let rl = rest.to_lowercase();
        if rl.starts_with(w) {
            rest = rest[w.len()..].trim_start();
        }
    }
    rest
}

// `parse_string_literal` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn parse_string_literal(s: &str) -> Result<(String, &str)> {
    let s = s.trim_start();
    let quote = s
        .chars()
        .next()
        .ok_or_else(|| Error::new("expected string"))?;
    if quote != '\'' && quote != '"' {
        return Err(Error::new("expected quoted string"));
    }
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == quote as u8 {
            if i + 1 < bytes.len() && bytes[i + 1] == quote as u8 {
                out.push(quote);
                i += 2;
                continue;
            }
            return Ok((out, &s[i + 1..]));
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    Err(Error::new("unclosed string"))
}

// `extract_paren_list` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn extract_paren_list(s: &str) -> Result<String> {
    let s = s.trim_start();
    if !s.starts_with('(') {
        return Err(Error::new("expected '('"));
    }
    let mut depth = 0;
    let mut in_str: Option<u8> = None;
    for (i, b) in s.bytes().enumerate() {
        if let Some(q) = in_str {
            if b == q {
                in_str = None;
            }
            continue;
        }
        match b {
            b'\'' | b'"' => in_str = Some(b),
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(s[1..i].to_string());
                }
            }
            _ => {}
        }
    }
    Err(Error::new("unclosed '('"))
}

// `split_top_level_commas` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    let mut in_str: Option<u8> = None;
    for b in s.bytes() {
        if let Some(q) = in_str {
            cur.push(b as char);
            if b == q {
                in_str = None;
            }
            continue;
        }
        match b {
            b'\'' | b'"' => {
                in_str = Some(b);
                cur.push(b as char);
            }
            b'(' => {
                depth += 1;
                cur.push('(');
            }
            b')' => {
                depth -= 1;
                cur.push(')');
            }
            b',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(b as char),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Go `ddl.BuildTableInfoFromAST` + assign IDs.
/// `build_table_info_from_ast` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn build_table_info_from_ast(stmt: &CreateTableStmt) -> Result<TableInfo> {
    let mut cols = Vec::new();
    for (i, c) in stmt.Cols.iter().enumerate() {
        cols.push(ColumnInfo {
            ID: (i as i64) + 1,
            Name: c.Name.L.clone(),
            Offset: i,
            FieldType: c.Tp.clone(),
        });
    }
    let mut indices = Vec::new();
    let mut idx_id = 1i64;
    // column-level unique/pk
    for c in &stmt.Cols {
        let uniq = c.Options.iter().any(|o| {
            matches!(
                o.Tp,
                ColumnOptionTp::PrimaryKey
                    | ColumnOptionTp::UniqKey
                    | ColumnOptionTp::AutoIncrement
            )
        });
        if uniq {
            if let Some(ci) = cols.iter().find(|x| x.Name == c.Name.L) {
                indices.push(IndexInfo {
                    ID: idx_id,
                    Name: format!("idx_{}", c.Name.L),
                    Columns: vec![IndexColumn {
                        Name: c.Name.L.clone(),
                        Offset: ci.Offset,
                    }],
                });
                idx_id += 1;
            }
        }
    }
    for cons in &stmt.Constraints {
        match cons.Tp {
            ConstraintTp::PrimaryKey
            | ConstraintTp::Key
            | ConstraintTp::Uniq
            | ConstraintTp::UniqKey
            | ConstraintTp::UniqIndex
            | ConstraintTp::Index => {
                let mut icols = Vec::new();
                for k in &cons.Keys {
                    if let Some(ci) = cols.iter().find(|x| x.Name == k.Column.L) {
                        icols.push(IndexColumn {
                            Name: k.Column.L.clone(),
                            Offset: ci.Offset,
                        });
                    }
                }
                if !icols.is_empty() {
                    indices.push(IndexInfo {
                        ID: idx_id,
                        Name: format!("idx_{idx_id}"),
                        Columns: icols,
                    });
                    idx_id += 1;
                }
            }
            ConstraintTp::Other => {}
        }
    }
    Ok(TableInfo {
        ID: 1,
        Name: stmt.Table.L.clone(),
        Columns: cols,
        Indices: indices,
    })
}

// --- Time helpers used by rand/stats ---

#[derive(Clone, Debug, PartialEq, Eq)]
/// `CivilTime` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct CivilTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl CivilTime {
    /// `now` 以当前 Unix 秒数构造一个近似本地墙上时间的 civil time。
    /// 这里优先维持 importer 需要的格式形状与可重复性，不承担完整时区语义。
    pub fn now() -> Self {
        let dur = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        // Approximate local wall clock via UTC for deterministic formatting shape.
        let secs = dur.as_secs() as i64;
        civil_from_unix(secs)
    }

    /// `is_zero` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn is_zero(&self) -> bool {
        *self == CivilTime::default()
    }

    /// `add_days` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn add_days(&self, days: i32) -> Self {
        let mut d = days_from_civil(self.year, self.month, self.day) + days as i64;
        let (y, m, day) = civil_from_days(d);
        Self {
            year: y,
            month: m,
            day,
            hour: self.hour,
            minute: self.minute,
            second: self.second,
        }
    }

    /// `add_seconds` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn add_seconds(&self, secs: i64) -> Self {
        let base = days_from_civil(self.year, self.month, self.day) * 86400
            + self.hour as i64 * 3600
            + self.minute as i64 * 60
            + self.second as i64;
        civil_from_unix(base + secs)
    }

    /// `add_years` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn add_years(&self, years: i32) -> Self {
        Self {
            year: self.year + years,
            ..self.clone()
        }
    }

    /// `format_date` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn format_date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// `format_time` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn format_time(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }

    /// `format_datetime` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn format_datetime(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// `format_year` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn format_year(&self) -> String {
        format!("{:04}", self.year)
    }

    /// `DateFormat` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn DateFormat(&self, mysql_fmt: &str) -> Result<String> {
        // Supports the formats importer uses.
        match mysql_fmt {
            "%Y-%m-%d" => Ok(self.format_date()),
            "%H:%i:%s" => Ok(self.format_time()),
            "%Y-%m-%d %H:%i:%s" => Ok(self.format_datetime()),
            "%Y" => Ok(self.format_year()),
            _ => Ok(self.format_datetime()),
        }
    }

    /// `GoTime` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn GoTime(&self) -> Result<CivilTime> {
        Ok(self.clone())
    }

    /// `Format` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn Format(&self, date_fmt: &str) -> String {
        // Go layouts: DateOnly, TimeOnly, DateTime, "2006"
        if date_fmt == "2006-01-02" || date_fmt == date_format_go() {
            self.format_date()
        } else if date_fmt == "15:04:05" || date_fmt == time_format_go() {
            self.format_time()
        } else if date_fmt == "2006-01-02 15:04:05" || date_fmt == datetime_format_go() {
            self.format_datetime()
        } else if date_fmt == "2006" {
            self.format_year()
        } else if date_fmt.contains("%Y") {
            self.DateFormat(date_fmt)
                .unwrap_or_else(|_| self.format_date())
        } else if date_fmt.contains('-') && date_fmt.contains(':') {
            self.format_datetime()
        } else if date_fmt.contains('-') {
            self.format_date()
        } else if date_fmt.contains(':') {
            self.format_time()
        } else {
            self.format_datetime()
        }
    }
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Default for CivilTime {
    fn default() -> Self {
        Self {
            year: 0,
            month: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }
}

// `fn` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const fn date_format_go() -> &'static str {
    "2006-01-02"
}
// `fn` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const fn time_format_go() -> &'static str {
    "15:04:05"
}
// `fn` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const fn datetime_format_go() -> &'static str {
    "2006-01-02 15:04:05"
}
// `YEAR_FORMAT` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const YEAR_FORMAT: &str = "2006";

/// `parse_date` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn parse_date(s: &str) -> Result<CivilTime> {
    // YYYY-MM-DD
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return Err(Error::new(format!("parse date: {s}")));
    }
    Ok(CivilTime {
        year: parts[0].parse().map_err(|e| Error::new(format!("{e}")))?,
        month: parts[1].parse().map_err(|e| Error::new(format!("{e}")))?,
        day: parts[2].parse().map_err(|e| Error::new(format!("{e}")))?,
        ..CivilTime::default()
    })
}

/// `parse_time_of_day` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn parse_time_of_day(s: &str) -> Result<CivilTime> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return Err(Error::new(format!("parse time: {s}")));
    }
    Ok(CivilTime {
        hour: parts[0].parse().map_err(|e| Error::new(format!("{e}")))?,
        minute: parts[1].parse().map_err(|e| Error::new(format!("{e}")))?,
        second: parts[2].parse().map_err(|e| Error::new(format!("{e}")))?,
        year: 0,
        month: 1,
        day: 1,
    })
}

/// `parse_datetime` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn parse_datetime(s: &str) -> Result<CivilTime> {
    let mut sp = s.splitn(2, ' ');
    let d = sp.next().unwrap_or("");
    let t = sp.next().unwrap_or("00:00:00");
    let mut dt = parse_date(d)?;
    let tm = parse_time_of_day(t)?;
    dt.hour = tm.hour;
    dt.minute = tm.minute;
    dt.second = tm.second;
    Ok(dt)
}

/// `parse_year` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn parse_year(s: &str) -> Result<CivilTime> {
    let y: i32 = s.parse().map_err(|e| Error::new(format!("{e}")))?;
    Ok(CivilTime {
        year: y,
        month: 1,
        day: 1,
        ..CivilTime::default()
    })
}

/// `timestamp_diff` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn timestamp_diff(unit: &str, lower: &CivilTime, upper: &CivilTime) -> i64 {
    match unit {
        "DAY" => {
            days_from_civil(upper.year, upper.month, upper.day)
                - days_from_civil(lower.year, lower.month, lower.day)
        }
        "SECOND" => {
            let lu = days_from_civil(lower.year, lower.month, lower.day) * 86400
                + lower.hour as i64 * 3600
                + lower.minute as i64 * 60
                + lower.second as i64;
            let uu = days_from_civil(upper.year, upper.month, upper.day) * 86400
                + upper.hour as i64 * 3600
                + upper.minute as i64 * 60
                + upper.second as i64;
            uu - lu
        }
        "YEAR" => (upper.year - lower.year) as i64,
        _ => 0,
    }
}

// `civil_from_unix` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn civil_from_unix(secs: i64) -> CivilTime {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400) as u32;
    let (y, m, d) = civil_from_days(days);
    CivilTime {
        year: y,
        month: m,
        day: d,
        hour: rem / 3600,
        minute: (rem % 3600) / 60,
        second: rem % 60,
    }
}

// `days_from_civil` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    // Howard Hinnant algorithm
    let mut y = y as i64;
    let m = m as i64;
    let d = d as i64;
    y -= if m <= 2 { 1 } else { 0 };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

// `civil_from_days` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

// --- Stats histogram stubs ---

#[derive(Clone, Debug)]
/// `BoundDatum` 汇总当前场景下需要区分的有限状态。
/// 枚举成员的划分优先服务 Go 兼容分支，而不是追求更泛化的抽象。
/// 因此新增成员前应先确认上层是否真的需要新的可观察行为。
pub enum BoundDatum {
    Int(i64),
    Str(String),
    Time(CivilTime),
}

#[derive(Clone, Debug, Default)]
/// `Bounds` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct Bounds {
    pub rows: Vec<BoundDatum>,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Bounds {
    /// `NumRows` 暴露边界行数，供上层按 Go 习惯遍历直方图上下界。
    /// 返回值保持 `i32` 形状，避免调用点额外适配整数类型。
    pub fn NumRows(&self) -> i32 {
        self.rows.len() as i32
    }
    /// `GetRow` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn GetRow(&self, idx: i32) -> BoundRow<'_> {
        BoundRow {
            datum: self.rows.get(idx as usize),
        }
    }
}

/// `BoundRow<'a>` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct BoundRow<'a> {
    datum: Option<&'a BoundDatum>,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl BoundRow<'_> {
    /// `GetInt64` 只在当前边界本身是整数时返回真实值，其余情况回退为零值。
    /// 这种宽松读取方式对应 importer 对统计桩“尽量给默认值继续跑”的预期。
    pub fn GetInt64(&self, _col: i32) -> i64 {
        match self.datum {
            Some(BoundDatum::Int(v)) => *v,
            _ => 0,
        }
    }
    /// `GetString` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn GetString(&self, _col: i32) -> String {
        match self.datum {
            Some(BoundDatum::Str(s)) => s.clone(),
            Some(BoundDatum::Int(v)) => v.to_string(),
            Some(BoundDatum::Time(t)) => t.format_datetime(),
            None => String::new(),
        }
    }
    /// `GetTime` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn GetTime(&self, _col: i32) -> CivilTime {
        match self.datum {
            Some(BoundDatum::Time(t)) => t.clone(),
            _ => CivilTime::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
/// `Bucket` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct Bucket {
    pub Count: i64,
    pub Repeat: i64,
}

#[derive(Clone, Debug, Default)]
/// `HistogramCore` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct HistogramCore {
    pub Buckets: Vec<Bucket>,
    pub Bounds: Bounds,
    pub ID: i64,
}

#[derive(Clone, Debug)]
/// `ColumnStats` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct ColumnStats {
    pub Histogram: HistogramCore,
    pub Info: Option<IndexInfo>,
}

#[derive(Clone, Debug, Default)]
/// `StatsTable` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct StatsTable {
    pub columns: HashMap<i64, ColumnStats>,
    pub indices: HashMap<i64, ColumnStats>,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl StatsTable {
    /// `GetCol` 通过列 ID 取统计信息，维持与 Go 侧 map 读取一致的可选返回语义。
    /// 若没有命中，调用方看到的是 `None`，而不是桩层自行合成空统计。
    pub fn GetCol(&self, id: i64) -> Option<&ColumnStats> {
        self.columns.get(&id)
    }
    /// `GetIdx` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn GetIdx(&self, id: i64) -> Option<&ColumnStats> {
        self.indices.get(&id)
    }
}

/// Simplified stats JSON for tests / local load:
/// `{ "columns": { "1": { "buckets":[{"count":10,"repeat":1}], "bounds":[{"int":1},{"int":10}] } }, "indices": {} }`
/// `table_stats_from_simplified_json` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn table_stats_from_simplified_json(tbl: &TableInfo, data: &str) -> Result<StatsTable> {
    let _ = tbl;
    parse_simplified_stats(data)
}

/// `load_stats_file` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn load_stats_file(tbl: &TableInfo, path: &str) -> Result<StatsTable> {
    let data = fs::read_to_string(path).map_err(|e| Error::new(e.to_string()))?;
    let json: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| Error::new(e.to_string()))?;
    if data.contains("\"bounds\"") {
        return table_stats_from_simplified_json(tbl, &data);
    }
    table_stats_from_tidb_json(tbl, &json)
}

fn table_stats_from_tidb_json(tbl: &TableInfo, json: &serde_json::Value) -> Result<StatsTable> {
    let mut table = StatsTable::default();
    let columns = json
        .get("columns")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| Error::new("statistics JSON has no columns object"))?;
    for column in &tbl.Columns {
        if let Some(value) = columns.get(&column.Name) {
            table.columns.insert(
                column.ID,
                parse_tidb_column(value, column.ID, column.FieldType.GetType(), None)?,
            );
        }
    }
    if let Some(indices) = json.get("indices").and_then(serde_json::Value::as_object) {
        for index in &tbl.Indices {
            let value = indices.get(&index.Name).or_else(|| {
                index
                    .Columns
                    .first()
                    .and_then(|column| indices.get(&column.Name))
            });
            if let Some(value) = value {
                let tp = index
                    .Columns
                    .first()
                    .and_then(|key| tbl.Columns.get(key.Offset))
                    .map(|column| column.FieldType.GetType())
                    .unwrap_or(TypeString);
                table.indices.insert(
                    index.ID,
                    parse_tidb_column(value, index.ID, tp, Some(index.clone()))?,
                );
            }
        }
    }
    Ok(table)
}

fn parse_tidb_column(
    value: &serde_json::Value,
    id: i64,
    tp: u8,
    info: Option<IndexInfo>,
) -> Result<ColumnStats> {
    let mut buckets = Vec::new();
    let mut bounds = Vec::new();
    if let Some(items) = value
        .get("histogram")
        .and_then(|histogram| histogram.get("buckets"))
        .and_then(serde_json::Value::as_array)
    {
        for item in items {
            buckets.push(Bucket {
                Count: item
                    .get("count")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
                Repeat: item
                    .get("repeats")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
            });
            for key in ["lower_bound", "upper_bound"] {
                let encoded = item
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| Error::new(format!("statistics bucket has no {key}")))?;
                bounds.push(decode_tidb_bound(encoded, tp)?);
            }
        }
    }
    Ok(ColumnStats {
        Histogram: HistogramCore {
            Buckets: buckets,
            Bounds: Bounds { rows: bounds },
            ID: id,
        },
        Info: info,
    })
}

fn decode_tidb_bound(encoded: &str, tp: u8) -> Result<BoundDatum> {
    let bytes = decode_base64(encoded)?;
    if matches!(tp, TypeDate | TypeDatetime | TypeTimestamp) && bytes.len() == 9 && bytes[0] == 4 {
        let packed = u64::from_be_bytes(bytes[1..].try_into().unwrap());
        let ymdhms = packed >> 24;
        let ymd = ymdhms >> 17;
        let hms = ymdhms & ((1 << 17) - 1);
        let ym = ymd >> 5;
        return Ok(BoundDatum::Time(CivilTime {
            year: (ym / 13) as i32,
            month: (ym % 13) as u32,
            day: (ymd & 31) as u32,
            hour: (hms >> 12) as u32,
            minute: ((hms >> 6) & 63) as u32,
            second: (hms & 63) as u32,
        }));
    }
    let text = String::from_utf8(bytes.clone()).ok();
    if matches!(
        tp,
        TypeTiny | TypeShort | TypeLong | TypeLonglong | TypeYear
    ) {
        if let Some(value) = text.as_deref().and_then(|text| text.parse::<i64>().ok()) {
            return Ok(BoundDatum::Int(value));
        }
    }
    Ok(BoundDatum::Str(text.unwrap_or_else(|| {
        String::from_utf8_lossy(&bytes).into_owned()
    })))
}

fn decode_base64(input: &str) -> Result<Vec<u8>> {
    fn digit(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut output = Vec::with_capacity(input.len() / 4 * 3);
    for chunk in input.as_bytes().chunks(4) {
        if chunk.len() != 4 {
            return Err(Error::new("invalid base64 statistics bound"));
        }
        let mut value = 0_u32;
        let mut padding = 0;
        for byte in chunk {
            value <<= 6;
            if *byte == b'=' {
                padding += 1;
            } else {
                value |= digit(*byte)
                    .ok_or_else(|| Error::new("invalid base64 statistics bound"))?
                    as u32;
            }
        }
        output.push((value >> 16) as u8);
        if padding < 2 {
            output.push((value >> 8) as u8);
        }
        if padding == 0 {
            output.push(value as u8);
        }
    }
    Ok(output)
}

fn validate_json_object(data: &str) -> Result<()> {
    let trimmed = data.trim();
    if !trimmed.starts_with('{') {
        return Err(Error::new("invalid JSON: expected object"));
    }
    let body = extract_balanced(trimmed, '{', '}')?;
    if body.len() + 2 != trimmed.len() {
        return Err(Error::new("invalid JSON: trailing data"));
    }
    let body = body.trim();
    if !body.is_empty() && !body.starts_with('"') {
        return Err(Error::new("invalid JSON: object key must be a string"));
    }
    Ok(())
}

// `parse_simplified_stats` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn parse_simplified_stats(data: &str) -> Result<StatsTable> {
    // Extremely small hand parser for our test JSON shape.
    let mut st = StatsTable::default();
    // columns
    if let Some(cols) = extract_json_object_section(data, "columns") {
        for (key, body) in iter_json_map_entries(&cols) {
            let id: i64 = key.parse().unwrap_or(0);
            st.columns.insert(id, parse_col_stats(&body)?);
        }
    }
    if let Some(idxs) = extract_json_object_section(data, "indices") {
        for (key, body) in iter_json_map_entries(&idxs) {
            let id: i64 = key.parse().unwrap_or(0);
            let mut cs = parse_col_stats(&body)?;
            cs.Info = Some(IndexInfo {
                ID: id,
                Name: format!("idx_{id}"),
                Columns: vec![IndexColumn {
                    Name: String::new(),
                    Offset: 0,
                }],
            });
            st.indices.insert(id, cs);
        }
    }
    Ok(st)
}

// `parse_col_stats` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn parse_col_stats(body: &str) -> Result<ColumnStats> {
    let mut buckets = Vec::new();
    let mut bounds = Vec::new();
    if let Some(bkt) = extract_json_array_section(body, "buckets") {
        for item in split_json_array_items(&bkt) {
            let count = extract_json_i64(&item, "count").unwrap_or(0);
            let repeat = extract_json_i64(&item, "repeat").unwrap_or(0);
            buckets.push(Bucket {
                Count: count,
                Repeat: repeat,
            });
        }
    }
    if let Some(bnd) = extract_json_array_section(body, "bounds") {
        for item in split_json_array_items(&bnd) {
            if let Some(v) = extract_json_i64(&item, "int") {
                bounds.push(BoundDatum::Int(v));
            } else if let Some(s) = extract_json_string(&item, "str") {
                bounds.push(BoundDatum::Str(s));
            } else if let Some(s) = extract_json_string(&item, "time") {
                let t = parse_datetime(&s)
                    .or_else(|_| parse_date(&s))
                    .or_else(|_| parse_time_of_day(&s))
                    .unwrap_or_default();
                bounds.push(BoundDatum::Time(t));
            }
        }
    }
    Ok(ColumnStats {
        Histogram: HistogramCore {
            Buckets: buckets,
            Bounds: Bounds { rows: bounds },
            ID: 0,
        },
        Info: None,
    })
}

// `extract_json_object_section` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn extract_json_object_section(data: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let idx = data.find(&pat)?;
    let after = data[idx + pat.len()..].trim_start();
    if !after.starts_with(':') {
        return None;
    }
    let after = after[1..].trim_start();
    if !after.starts_with('{') {
        return None;
    }
    extract_balanced(after, '{', '}').ok()
}

// `extract_json_array_section` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn extract_json_array_section(data: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let idx = data.find(&pat)?;
    let after = data[idx + pat.len()..].trim_start();
    if !after.starts_with(':') {
        return None;
    }
    let after = after[1..].trim_start();
    if !after.starts_with('[') {
        return None;
    }
    extract_balanced(after, '[', ']').ok()
}

// `extract_balanced` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn extract_balanced(s: &str, open: char, close: char) -> Result<String> {
    let bytes = s.as_bytes();
    let mut depth = 0;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            c if c == open as u8 => depth += 1,
            c if c == close as u8 => {
                depth -= 1;
                if depth == 0 {
                    return Ok(s[1..i].to_string());
                }
            }
            _ => {}
        }
    }
    Err(Error::new("unbalanced json"))
}

// `iter_json_map_entries` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn iter_json_map_entries(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = body.trim();
    while !rest.is_empty() {
        rest = rest.trim_start().trim_start_matches(',');
        if rest.is_empty() {
            break;
        }
        if !rest.starts_with('"') {
            break;
        }
        let end = match rest[1..].find('"') {
            Some(e) => e,
            None => break,
        };
        let key = rest[1..1 + end].to_string();
        rest = rest[2 + end..].trim_start();
        if !rest.starts_with(':') {
            break;
        }
        rest = rest[1..].trim_start();
        let (val, next) = if rest.starts_with('{') {
            match extract_balanced(rest, '{', '}') {
                Ok(v) => {
                    let consumed = v.len() + 2;
                    (v, &rest[consumed..])
                }
                Err(_) => break,
            }
        } else {
            break;
        };
        out.push((key, val));
        rest = next;
    }
    out
}

// `split_json_array_items` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn split_json_array_items(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = body.trim();
    while !rest.is_empty() {
        rest = rest.trim_start().trim_start_matches(',');
        if rest.is_empty() {
            break;
        }
        if rest.starts_with('{') {
            match extract_balanced(rest, '{', '}') {
                Ok(v) => {
                    let consumed = v.len() + 2;
                    out.push(v);
                    rest = &rest[consumed..];
                }
                Err(_) => break,
            }
        } else {
            break;
        }
    }
    out
}

// `extract_json_i64` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn extract_json_i64(obj: &str, key: &str) -> Option<i64> {
    let pat = format!("\"{key}\"");
    let idx = obj.find(&pat)?;
    let after = obj[idx + pat.len()..].trim_start();
    if !after.starts_with(':') {
        return None;
    }
    let after = after[1..].trim_start();
    let num: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    num.parse().ok()
}

// `extract_json_string` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn extract_json_string(obj: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let idx = obj.find(&pat)?;
    let after = obj[idx + pat.len()..].trim_start();
    if !after.starts_with(':') {
        return None;
    }
    let after = after[1..].trim_start();
    if !after.starts_with('"') {
        return None;
    }
    let end = after[1..].find('"')?;
    Some(after[1..1 + end].to_string())
}

// --- database/sql recording stub ---

#[derive(Clone, Debug, Default)]
/// `DB` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct DB {
    inner: Arc<Mutex<DbInner>>,
}

#[derive(Default, Debug)]
struct DbInner {
    closed: bool,
    execs: Vec<String>,
    begins: u64,
    commits: u64,
    dsn: String,
    fail_exec: bool,
    fail_begin: bool,
    fail_commit: bool,
}

#[derive(Clone, Debug)]
/// `Tx` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct Tx {
    db: DB,
    execs: Arc<Mutex<Vec<String>>>,
    done: Arc<Mutex<bool>>,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl DB {
    /// `new` 只负责初始化记录型数据库桩并记住 DSN，不在此处建立真实连接。
    /// 这样 importer 的连接生命周期仍可被观察，同时避免引入外部依赖。
    pub fn new(dsn: String) -> Self {
        let db = DB::default();
        if let Ok(mut g) = db.inner.lock() {
            g.dsn = dsn;
        }
        db
    }

    /// `Exec` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn Exec(&self, sql: &str) -> Result<()> {
        let mut g = self.inner.lock().map_err(|e| Error::new(e.to_string()))?;
        if g.closed {
            return Err(Error::new("sql: database is closed"));
        }
        if g.fail_exec {
            return Err(Error::new("exec failed"));
        }
        g.execs.push(sql.to_string());
        Ok(())
    }

    /// `Begin` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn Begin(&self) -> Result<Tx> {
        let mut g = self.inner.lock().map_err(|e| Error::new(e.to_string()))?;
        if g.closed {
            return Err(Error::new("sql: database is closed"));
        }
        if g.fail_begin {
            return Err(Error::new("begin failed"));
        }
        g.begins += 1;
        Ok(Tx {
            db: self.clone(),
            execs: Arc::new(Mutex::new(Vec::new())),
            done: Arc::new(Mutex::new(false)),
        })
    }

    /// `Close` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn Close(&self) -> Result<()> {
        let mut g = self.inner.lock().map_err(|e| Error::new(e.to_string()))?;
        g.closed = true;
        Ok(())
    }

    /// `execs` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn execs(&self) -> Vec<String> {
        self.inner
            .lock()
            .map(|g| g.execs.clone())
            .unwrap_or_default()
    }

    /// `begins` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn begins(&self) -> u64 {
        self.inner.lock().map(|g| g.begins).unwrap_or(0)
    }

    /// `commits` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn commits(&self) -> u64 {
        self.inner.lock().map(|g| g.commits).unwrap_or(0)
    }

    /// `is_closed` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn is_closed(&self) -> bool {
        self.inner.lock().map(|g| g.closed).unwrap_or(true)
    }

    /// `dsn` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn dsn(&self) -> String {
        self.inner.lock().map(|g| g.dsn.clone()).unwrap_or_default()
    }

    /// `set_fail_exec` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn set_fail_exec(&self, v: bool) {
        if let Ok(mut g) = self.inner.lock() {
            g.fail_exec = v;
        }
    }

    /// `set_fail_begin` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn set_fail_begin(&self, v: bool) {
        if let Ok(mut g) = self.inner.lock() {
            g.fail_begin = v;
        }
    }

    /// `set_fail_commit` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn set_fail_commit(&self, v: bool) {
        if let Ok(mut g) = self.inner.lock() {
            g.fail_commit = v;
        }
    }
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Tx {
    /// `Exec` 将事务内执行同时镜像到事务日志和数据库总日志，方便 parity test 观察执行轨迹。
    /// 若底层被配置为失败，这里仍按 Go 语义在真正记录前返回错误。
    pub fn Exec(&self, sql: &str) -> Result<()> {
        if *self.done.lock().unwrap() {
            return Err(Error::new(
                "sql: transaction has already been committed or rolled back",
            ));
        }
        self.execs.lock().unwrap().push(sql.to_string());
        // Also mirror onto DB exec log for observability.
        let mut g = self
            .db
            .inner
            .lock()
            .map_err(|e| Error::new(e.to_string()))?;
        if g.fail_exec {
            return Err(Error::new("exec failed"));
        }
        g.execs.push(sql.to_string());
        Ok(())
    }

    /// `Commit` 承担该类型上的一个局部行为。
    /// 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    /// 若未来重构此方法，应先确认外层可观察语义没有漂移。
    pub fn Commit(&self) -> Result<()> {
        let mut done = self.done.lock().unwrap();
        if *done {
            return Err(Error::new(
                "sql: transaction has already been committed or rolled back",
            ));
        }
        let mut g = self
            .db
            .inner
            .lock()
            .map_err(|e| Error::new(e.to_string()))?;
        if g.fail_commit {
            return Err(Error::new("commit failed"));
        }
        g.commits += 1;
        *done = true;
        Ok(())
    }
}

/// `open_db` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn open_db(user: &str, password: &str, host: &str, port: isize, name: &str) -> Result<DB> {
    let dsn = format!("{user}:{password}@tcp({host}:{port})/{name}");
    Ok(DB::new(dsn))
}

// --- RNG (math/rand global source stand-in) ---

use std::cell::Cell;

thread_local! {
    static RNG: Cell<u64> = Cell::new(0x1234_5678_9abc_def0);
}

/// `seed_rng` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn seed_rng(seed: u64) {
    RNG.with(|r| r.set(seed));
}

// `next_u64` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn next_u64() -> u64 {
    RNG.with(|r| {
        // xorshift64*
        let mut x = r.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        r.set(x);
        x
    })
}

/// Go `rand.Intn(n)` for n > 0.
/// `rand_intn` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn rand_intn(n: i32) -> i32 {
    assert!(n > 0);
    (next_u64() % n as u64) as i32
}

/// Go `rand.Int63()`.
/// `rand_int63` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn rand_int63() -> i64 {
    (next_u64() & 0x7fff_ffff_ffff_ffff) as i64
}

/// Go `rand.Int63n(n)`.
/// `rand_int63n` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn rand_int63n(n: i64) -> i64 {
    assert!(n > 0);
    rand_int63().rem_euclid(n)
}

/// Go `rand.Int31n(n)`.
/// `rand_int31n` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn rand_int31n(n: i32) -> i32 {
    assert!(n > 0);
    (rand_int63() % n as i64) as i32
}

// --- Minimal TOML overlay for Config ---

/// `apply_toml_to_config_fields` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn apply_toml_to_config_fields(
    text: &str,
    db_host: &mut String,
    db_user: &mut String,
    db_password: &mut String,
    db_name: &mut String,
    db_port: &mut i32,
    table_sql: &mut String,
    index_sql: &mut String,
    stats_path: &mut String,
    log_level: &mut String,
    worker_count: &mut i32,
    job_count: &mut i32,
    batch: &mut i32,
) -> Result<()> {
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let v = trim_toml_value(v.trim());
        match (section.as_str(), k) {
            ("db", "host") => *db_host = v,
            ("db", "user") => *db_user = v,
            ("db", "password") => *db_password = v,
            ("db", "name") => *db_name = v,
            ("db", "port") => {
                *db_port = parse_toml_i32(&v, "db.port")?;
            }
            ("ddl", "table-sql") => *table_sql = v,
            ("ddl", "index-sql") => *index_sql = v,
            ("stats", "stats-file-path") => *stats_path = v,
            ("sys", "log-level") => *log_level = v,
            ("sys", "worker-count") => {
                *worker_count = parse_toml_i32(&v, "sys.worker-count")?;
            }
            ("sys", "job-count") => {
                *job_count = parse_toml_i32(&v, "sys.job-count")?;
            }
            ("sys", "batch") => {
                *batch = parse_toml_i32(&v, "sys.batch")?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_toml_i32(value: &str, field: &str) -> Result<i32> {
    value
        .parse::<i32>()
        .map_err(|err| Error::new(format!("invalid TOML value for {field}: {err}")))
}

// `trim_toml_value` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn trim_toml_value(v: &str) -> String {
    let v = v.trim();
    if (v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')) {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}
