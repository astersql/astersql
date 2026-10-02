// Copyright 2026 AsterSQL.

// CREATE TABLE 语句构建表元信息（TableInfo）的单元测试。
//
// 本文件验证 DDL（数据定义语言）层将 `CREATE TABLE` 的 AST（抽象语法树）
// 转换为内部表元数据的核心入口 `BuildTableInfoFromAST` / `BuildTableInfoWithStmt`，
// 覆盖以下场景：
// - 列、索引、生成列（generated column）、CHECK 约束与外键的构建；
// - RANGE/LIST 分区表的构建以及数据库级默认字符集/排序规则的继承；
// - 默认值表达式、表达式索引（expression index）与 AUTO_RANDOM 的支持；
// - 与 Go(TiDB) 行为对齐的子分区（subpartition）降级处理；
// - 非法 AUTO_RANDOM 定义与不支持的临时表模式的报错。

use astersql_meta_metabuild as metabuild;
use astersql_meta_model as model;
use astersql_parser_ast as ast;

use crate::{BuildTableInfoFromAST, BuildTableInfoWithStmt};

/// 解析一条 `CREATE TABLE` SQL 并返回其 AST 节点的独立副本。
///
/// 解析器返回的是通用语句 trait 对象，这里先通过 `downcast_ref`
/// 向下转型为 `CreateTableStmt`，再逐字段克隆出一个新的 Box 节点，
/// 避免测试代码持有解析器内部的借用；其中 `Select` 字段被显式置空，
/// 因为这些测试不涉及 `CREATE TABLE ... SELECT` 形式。
fn parse_create(sql: &str) -> Box<ast::CreateTableStmt> {
    astersql_planner_core::InstallPlannerExpressionFactory().expect("planner expression factory");
    let mut parser = astersql_parser::New();
    let statement = parser
        .ParseOneStmt(sql, "", "")
        .expect("parse CREATE TABLE");
    // 将通用语句节点向下转型为 CREATE TABLE 语句节点。
    let create = statement
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .expect("CREATE TABLE AST");
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

/// 验证正式构建器能正确生成列、索引、生成列、CHECK 约束与外键元数据。
///
/// 涉及术语：
/// - clustered 主键：主键即行的存储句柄（handle），对应 `PKIsHandle`；
/// - 生成列（generated column）：值由表达式计算得出，`stored` 表示物化存储；
/// - 外键（foreign key）：引用父表列并带 `ON DELETE CASCADE` 级联删除动作。
#[test]
fn formal_builder_constructs_columns_indexes_generated_checks_and_foreign_keys() {
    let statement = parse_create(
        "create table child (id bigint primary key clustered, parent_id bigint, \
         g bigint generated always as (parent_id + 1) stored, \
         unique key uk_parent(parent_id), \
         constraint fk_parent foreign key(parent_id) references parent(id) on delete cascade, \
         constraint chk_parent check(parent_id > 0)) comment='child'",
    );
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table = BuildTableInfoFromAST(&context, &statement).expect("build table metadata");

    // 主键为 clustered bigint，应直接作为行句柄（PKIsHandle）。
    assert!(table.PKIsHandle);
    assert_eq!(table.Columns.len(), 3);
    assert_eq!(table.Columns[2].GeneratedExprString, "`parent_id` + 1");
    assert!(table.Columns[2].GeneratedStored);
    assert_eq!(table.Indices.len(), 1);
    assert_eq!(table.ForeignKeys.len(), 1);
    assert_eq!(table.ForeignKeys[0].RefTable.O, "parent");
    assert_eq!(table.Constraints.len(), 1);
    assert_eq!(table.Comment, "child");
}

#[test]
fn formal_builder_sets_go_compatible_missing_default_flags() {
    let statement = parse_create(
        "create table flag_defaults (\
         required int not null, \
         explicit_default int not null default 1, \
         auto_id int auto_increment, \
         primary_id int, key auto_idx(auto_id), primary key(primary_id))",
    );
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table = BuildTableInfoFromAST(&context, &statement).expect("build table metadata");

    assert!(model::mysql::HasNoDefaultValueFlag(
        table.Columns[0].GetFlag()
    ));
    assert_eq!(table.Columns[0].GetFlen(), 11);
    assert!(!model::mysql::HasNoDefaultValueFlag(
        table.Columns[1].GetFlag()
    ));
    assert!(model::mysql::HasNotNullFlag(table.Columns[2].GetFlag()));
    assert!(model::mysql::HasMultipleKeyFlag(table.Columns[2].GetFlag()));
    assert!(!model::mysql::HasNoDefaultValueFlag(
        table.Columns[2].GetFlag()
    ));
    assert!(model::mysql::HasNoDefaultValueFlag(
        table.Columns[3].GetFlag()
    ));
}

#[test]
fn formal_builder_resolves_explicit_column_charset_and_collation() {
    let statement = parse_create(
        "create table charset_columns (\
         utf8_col varchar(10) character set utf8, \
         latin1_col varchar(10) collate latin1_bin, \
         binary_col binary(4) default '', \
         temporal_col datetime(3) default '2020-01-01 00:00:00')",
    );
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table = BuildTableInfoFromAST(&context, &statement).expect("build table metadata");

    assert_eq!(table.Columns[0].GetCharset(), "utf8");
    assert_eq!(table.Columns[0].GetCollate(), "utf8_bin");
    assert_eq!(table.Columns[1].GetCharset(), "latin1");
    assert_eq!(table.Columns[1].GetCollate(), "latin1_bin");
    assert_eq!(
        table.Columns[2].DefaultValue,
        Some(model::DefaultValue::String(vec![0; 4]))
    );
    assert_eq!(
        table.Columns[3].DefaultValue,
        Some(model::DefaultValue::String(
            b"2020-01-01 00:00:00.000".to_vec()
        ))
    );
}

#[test]
fn formal_builder_derives_binary_charset_from_bare_table_collation() {
    let statement = parse_create("create table binary_table (value varchar(20)) collate=binary");
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table = BuildTableInfoFromAST(&context, &statement).expect("build table metadata");

    assert_eq!(table.Charset, "binary");
    assert_eq!(table.Collate, "binary");
    assert_eq!(table.Columns[0].GetCharset(), "binary");
    assert_eq!(table.Columns[0].GetCollate(), "binary");
}

/// 验证 RANGE 分区元数据的构建，并确认表继承数据库级默认字符集与排序规则。
///
/// RANGE 分区按分区键的取值区间划分数据；`MAXVALUE` 表示无上界的兜底分区。
/// `BuildTableInfoWithStmt` 额外接收数据库默认 charset/collate（此处为 latin1），
/// 建表语句未显式指定时应沿用这些默认值。
#[test]
fn formal_builder_constructs_partition_and_honors_database_defaults() {
    let statement = parse_create(
        "create table events (id bigint, created int, key(created)) \
         partition by range(created) (partition p0 values less than (10), \
         partition pmax values less than (maxvalue))",
    );
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table = BuildTableInfoWithStmt(&context, &statement, "latin1", "latin1_bin", None)
        .expect("build partitioned table metadata");

    assert_eq!(table.Charset, "latin1");
    assert_eq!(table.Collate, "latin1_bin");
    let partition = table.Partition.expect("partition metadata");
    assert_eq!(
        partition.Type,
        astersql_meta_model::ast::model::PartitionTypeRange
    );
    assert_eq!(partition.Definitions.len(), 2);
    assert_eq!(partition.Definitions[1].LessThan, vec!["MAXVALUE"]);
}

/// 验证 LIST 分区的 DEFAULT 兜底分区按 Go 元数据格式保存。
#[test]
fn formal_builder_constructs_list_default_partition() {
    let statement = parse_create(
        "create table list_events (category int) partition by list (category) (\
         partition p_values values in (1, 2, 3), partition p_default default)",
    );
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table =
        BuildTableInfoFromAST(&context, &statement).expect("build LIST DEFAULT partition metadata");

    let partition = table.Partition.expect("partition metadata");
    assert_eq!(
        partition.Type,
        astersql_meta_model::ast::model::PartitionTypeList
    );
    assert_eq!(partition.Definitions.len(), 2);
    assert_eq!(
        partition.Definitions[0].InValues,
        vec![vec!["1"], vec!["2"], vec!["3"]]
    );
    assert_eq!(partition.Definitions[1].InValues, vec![vec!["DEFAULT"]]);
}

/// 验证默认值表达式、表达式索引与 AUTO_RANDOM 属性的构建。
///
/// 涉及术语：
/// - AUTO_RANDOM(6, 40)：主键高位注入 6 个随机位、总范围 40 位，
///   用于打散自增主键写入热点；
/// - 默认值表达式：如 `default (uuid())`，默认值由表达式而非常量给出；
/// - 表达式索引：对表达式（如 `lower(name)`）建索引，内部通过
///   隐藏的虚拟生成列（Hidden 列）实现。
#[test]
fn formal_builder_supports_default_expressions_expression_indexes_and_auto_random() {
    let statement = parse_create(
        "create table generated_defaults (\
         id bigint primary key clustered auto_random(6, 40), \
         created timestamp default current_timestamp, \
         token varchar(64) default (uuid()), name varchar(32), \
         key idx_lower ((lower(name))))",
    );
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table = BuildTableInfoFromAST(&context, &statement).expect("build full table metadata");

    assert_eq!(table.AutoRandomBits, 6);
    assert_eq!(table.AutoRandomRangeBits, 40);
    // CURRENT_TIMESTAMP 默认值应以字符串形式记录（忽略大小写比较）。
    let Some(model::DefaultValue::String(current_timestamp)) =
        table.Columns[1].DefaultValue.as_ref()
    else {
        panic!("CURRENT_TIMESTAMP expression default");
    };
    assert_eq!(
        String::from_utf8_lossy(current_timestamp).to_ascii_lowercase(),
        "current_timestamp()"
    );
    assert!(table.Columns[2].DefaultIsExpr);
    // 表达式索引会额外生成一个隐藏的虚拟生成列，索引实际引用该隐藏列。
    let hidden = table
        .Columns
        .iter()
        .find(|column| column.Hidden)
        .expect("expression index hidden column");
    assert_eq!(hidden.Name.O, "_V$_idx_lower_0");
    assert!(hidden.GeneratedExprString.contains("lower(`name`)"));
    assert_eq!(table.Indices[0].Columns[0].Name, hidden.Name);
}

#[test]
fn generated_expression_metadata_preserves_string_literals() {
    let statement = parse_create(
        "create table generated_strings (\
         id varchar(16), raw varchar(16), \
         combined varchar(40) generated always as (concat(id, ':', raw)) virtual, \
         prefix varchar(16) generated always as (substr(raw, 1, 2)) virtual)",
    );
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table =
        BuildTableInfoFromAST(&context, &statement).expect("build generated string metadata");

    assert_eq!(
        table.Columns[2].GeneratedExprString,
        "concat(`id`,':',`raw`)"
    );
    assert_eq!(table.Columns[3].GeneratedExprString, "substr(`raw`,1,2)");
}

#[test]
fn generated_expression_metadata_preserves_charset_introduced_literals() {
    let statement = parse_create(
        "create table t (id int, \
         deleted_at datetime(3) not null default '1970-01-01 01:00:01.000', \
         is_deleted tinyint(1) generated always as \
         ((deleted_at > _utf8mb4'1970-01-01 01:00:01.000')) virtual not null, \
         key k(id, is_deleted))",
    );
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table = BuildTableInfoFromAST(&context, &statement)
        .expect("build generated column with charset-introduced literal");

    assert!(
        table.Columns[2]
            .GeneratedExprString
            .contains("_utf8mb4'1970-01-01 01:00:01.000'")
    );
    assert!(table.Columns[2].Dependences.contains_key("deleted_at"));
    assert!(
        table.Indices[0]
            .Columns
            .iter()
            .any(|column| column.Name.L == "is_deleted")
    );
}

/// 验证与 Go(TiDB) 一致的子分区降级行为：RANGE 分区表上不受支持的
/// HASH 子分区定义会被静默忽略，仅保留顶层 RANGE 分区元数据。
#[test]
fn formal_builder_matches_go_subpartition_fallback_for_range_tables() {
    let statement = parse_create(
        "create table subpart (id bigint primary key) \
         partition by range(id) subpartition by hash(id) subpartitions 2 (\
         partition p0 values less than (10), \
         partition p1 values less than (maxvalue))",
    );
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table = BuildTableInfoFromAST(&context, &statement)
        .expect("Go ignores unsupported RANGE subpartition metadata");

    let partition = table.Partition.expect("top-level partition metadata");
    assert_eq!(
        partition.Type,
        astersql_meta_model::ast::model::PartitionTypeRange
    );
    assert_eq!(partition.Definitions.len(), 2);
}

/// 验证非法定义会被构建器拒绝：
/// - `int` 列上使用 AUTO_RANDOM（该属性要求 bigint 主键）；
/// - 全局临时表使用 `ON COMMIT PRESERVE ROWS`（仅支持事务提交即删除行的模式）。
#[test]
fn formal_builder_rejects_invalid_auto_random_and_temporary_modes() {
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let invalid_auto_random =
        parse_create("create table bad_auto_random (id int primary key clustered auto_random)");
    assert!(BuildTableInfoFromAST(&context, &invalid_auto_random).is_err());

    let preserve_rows = parse_create(
        "create global temporary table bad_temporary (id bigint) on commit preserve rows",
    );
    assert!(BuildTableInfoFromAST(&context, &preserve_rows).is_err());
}

// The storage-class ADD/REORGANIZE tests consume checked, normalized boundaries.
// Isolate that prerequisite before adding storage-class matching to this path.
#[test]
fn formal_builder_normalizes_range_boundary_for_storage_class_matching() {
    let statement = parse_create(
        "create table t (id int) partition by range (id) \
         (partition p0 values less than (100), partition p1 values less than (100 + 200))",
    );
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table = BuildTableInfoFromAST(&context, &statement).expect("build checked metadata");
    let partition = table.Partition.as_ref().expect("range partition metadata");
    assert_eq!(partition.Definitions[1].LessThan, vec!["300"]);
}

#[test]
fn formal_builder_applies_storage_class_engine_attribute() {
    let statement =
        parse_create("create table t (id int) engine_attribute='{\"storage_class\":\"IA\"}'");
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table = BuildTableInfoFromAST(&context, &statement).expect("storage class metadata");
    assert_eq!(table.StorageClassTier, "IA");
}
