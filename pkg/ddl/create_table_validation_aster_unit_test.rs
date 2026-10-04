// Copyright 2026 AsterSQL.

// CREATE TABLE 语句校验的单元测试。
//
// 本模块通过 `BuildTableInfoFromAST`（由 CREATE TABLE 的 AST 抽象语法树
// 构建表元数据 TableInfo 的入口函数）验证 DDL（数据定义语言，即建表、
// 改表等修改库表结构的语句）建表流程中的各类校验规则，包括：
// - 非法默认值与时间类型小数秒精度（fsp）越界时应报错；
// - 生成列（generated column，值由表达式计算得出的列）引用带库名/表名
//   限定的列时应报错；
// - YEAR 类型自动补无符号标志、生成列依赖列集合的正确填充；
// - 生成列表达式中 CAST 的元数据记录；
// - `current_timestamp` 默认值不应被当作一般默认表达式处理。

// metabuild：构建表元数据时使用的上下文与工具 crate。
use astersql_meta_metabuild as metabuild;
// model：表结构元数据（TableInfo、ColumnInfo 等）的模型定义 crate。
use astersql_meta_model as model;
// ast：SQL 解析器产生的抽象语法树节点定义 crate。
use astersql_parser_ast as ast;

use crate::BuildTableInfoFromAST;

/// 将一条 CREATE TABLE SQL 文本解析为 `ast::CreateTableStmt` 节点。
///
/// 解析器返回的是动态类型的语句对象，这里先向下转型（downcast）为
/// `CreateTableStmt`，再逐字段克隆重建一个独立的 `Box` 值返回，
/// 以避免借用解析器内部数据；其中 `Select` 字段固定置空，
/// 表示不测试 CREATE TABLE ... SELECT 形式。
fn parse_create(sql: &str) -> Box<ast::CreateTableStmt> {
    let mut parser = astersql_parser::New();
    // ParseOneStmt 只解析单条语句，后两个参数为字符集与排序规则（此处留空用默认值）。
    let statement = parser
        .ParseOneStmt(sql, "", "")
        .expect("parse CREATE TABLE");
    // 将通用语句节点向下转型为 CreateTableStmt，测试输入保证必然成功。
    let create = statement
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .unwrap();
    Box::new(ast::CreateTableStmt {
        node_text: Default::default(),
        IfNotExists: create.IfNotExists,
        TemporaryKeyword: create.TemporaryKeyword,
        OnCommitDelete: create.OnCommitDelete,
        Table: create.Table.clone(),
        ReferTable: create.ReferTable.clone(),
        Cols: create.Cols.clone(),
        Constraints: create.Constraints.clone(),
        Options: create.Options.clone(),
        Partition: create.Partition.clone(),
        SplitIndex: create.SplitIndex.clone(),
        OnDuplicate: create.OnDuplicate,
        Select: None,
    })
}

/// 测试辅助函数：解析 SQL 并调用 `BuildTableInfoFromAST` 构建表元数据。
///
/// 返回 `Result`，成功时得到 `TableInfo`（表的完整元数据描述），
/// 失败时得到解析/校验错误，供各测试断言校验规则是否生效。
fn result(sql: &str) -> Result<model::TableInfo, astersql_parser::errors::Error> {
    // NewContext 创建一个空的元数据构建上下文，泛型参数表示无额外扩展与不可能出错的类型。
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    BuildTableInfoFromAST(&context, &parse_create(sql))
}

/// 校验两类非法定义应被拒绝：
/// 1. 整型列的默认值为无法转换的字符串 'a'；
/// 2. 时间类型（time/datetime/timestamp）的小数秒精度 fsp 超过上限 6。
#[test]
fn rejects_invalid_numeric_default_and_fractional_seconds_precision() {
    assert!(result("create table t(a int not null default 'a')").is_err());
    for sql in [
        "create table t(a time(7))",
        "create table t(a datetime(7))",
        "create table t(a timestamp(7))",
    ] {
        assert!(result(sql).is_err(), "{sql}");
    }
}

/// 对应 Go `TestSchemaNameAndTableNameInGeneratedExpr`：
/// 当前库名/表名限定的生成列引用合法，错误库名或表名才应被拒绝。
#[test]
fn validates_qualified_generated_column_references_like_go() {
    let schema_qualified = result("create table test.t(a int, b int as(lower(test.t.a)))").unwrap();
    assert_eq!(
        schema_qualified.Columns[1].GeneratedExprString,
        "lower(`a`)"
    );
    assert!(schema_qualified.Columns[1].Dependences.contains_key("a"));

    let table_qualified = result("create table t(a int, b int as(t.a+1))").unwrap();
    assert!(table_qualified.Columns[1].Dependences.contains_key("a"));

    assert!(result("create table test.t(a int, b int as(lower(test1.t.a)))").is_err());
    assert!(result("create table test.t(a int, b int as(lower(test.t1.a)))").is_err());
}

/// 校验元数据自动填充逻辑：
/// - YEAR 类型列应自动带上 unsigned（无符号）标志；
/// - 生成列 `b int as(a+1)` 的依赖集合 Dependences 中应记录被引用列 a。
#[test]
fn fills_year_unsigned_and_generated_dependencies() {
    let table = result("create table t(y year, a int, b int as(a+1))").unwrap();
    assert!(model::mysql::HasUnsignedFlag(table.Columns[0].GetFlag()));
    assert!(table.Columns[2].Dependences.contains_key("a"));
}

/// 校验生成列表达式中包含 CAST（类型转换）时的元数据记录：
/// GeneratedExprString（生成列表达式的文本形式）应保留 cast 关键字，
/// 且依赖集合仍能正确解析出被引用列 a。
#[test]
fn supports_cast_in_generated_column_metadata() {
    let table = result("create table t(a int, b varchar(20) as(cast(a as char)))").unwrap();
    assert!(
        table.Columns[1]
            .GeneratedExprString
            .to_ascii_lowercase()
            .contains("cast")
    );
    assert!(table.Columns[1].Dependences.contains_key("a"));
}

/// 校验 `default current_timestamp` 的特殊处理：
/// 它是 MySQL 语义中的内建时间默认值，不应标记为一般默认表达式
/// （DefaultIsExpr 为 false），但默认值本身仍应被记录下来。
#[test]
fn current_timestamp_is_not_a_general_default_expression() {
    let table = result("create table t(a timestamp default current_timestamp)").unwrap();
    assert!(!table.Columns[0].DefaultIsExpr);
    assert!(table.Columns[0].GetDefaultValue().is_some());
}

#[test]
fn fractional_timestamp_default_keeps_function_expression() {
    let table = result("create table t(a timestamp(6) default current_timestamp(6))").unwrap();
    let column = &table.Columns[0];
    assert!(!column.DefaultIsExpr);
    assert_eq!(column.GetDefaultValue(), column.GetOriginDefaultValue());
    let Some(model::DefaultValue::String(value)) = column.GetDefaultValue() else {
        panic!("timestamp function default must be retained");
    };
    assert!(String::from_utf8(value).unwrap().ends_with(')'));
}

#[test]
fn embedding_generated_column_rejects_non_starter_before_shape_validation() {
    let error = result("create table embedding_reject (text text, vec vector(3) generated always as (embed_text('mock/json', text)) stored)")
        .expect_err("embedding DDL must reject unsupported deployment");
    assert!(
        error.to_string().contains("starter deployment mode"),
        "{error}"
    );
}
