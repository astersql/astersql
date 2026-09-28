// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// DDL 集成测试模块。
//
// DDL（Data Definition Language，数据定义语言）指 CREATE/ALTER/DROP 等
// 修改表结构的 SQL 语句。本文件由 TiDB 的 db_integration_test.go 机械迁移而来，
// 覆盖以下场景的端到端验证：
// - 建表语句到表元数据（TableInfo）的构建；
// - 列的增加、删除、修改（含位置调整与默认值）；
// - 索引的创建、前缀长度校验与删除；
// - 字符集/排序规则（charset/collation）变更；
// - 自增 ID（auto_increment）的 rebase 与缓存；
// - 临时表（local/global temporary table）的建删与回收；
// - 生成列（Generated Column，即由表达式计算得出的列）的依赖检查。

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::{Arc, Barrier, Mutex};

use astersql_ddl_util::DdlUtilError;
use astersql_meta_metabuild as metabuild;
use astersql_meta_model as model;
use astersql_parser_ast as ast;

use crate::BuildTableInfoFromAST;
use crate::add_column::{AddColumnError, ColumnConstraint, ColumnDefinition, create_new_column};
use crate::column::{
    ColumnError, ColumnInfo as DdlColumn, ColumnKind, ColumnPosition, DefaultValue as DdlDefault,
    FieldType, IndexColumn as DdlIndexColumn, IndexInfo as DdlIndex, SchemaState,
    TableInfo as DdlColumnTable, check_add_column_too_many_columns, init_and_add_column_to_table,
    locate_offset_to_move, remove_column_and_single_indices,
};
use crate::ddl::{Ddl, Job, JobState};
use crate::ddl_algorithm::{AlgorithmType, AlterKind, resolve_alter_algorithm};
use crate::executor::{ExecutorError, validate_optimize_table};
use crate::generated_column::{
    ExpressionNode, GeneratedColumnError, GenerationType, check_depended_columns_exist,
    check_illegal_function_for_generated, find_column_names_in_expr,
    verify_column_generation_single,
};
use crate::index::{
    ColumnInfo as IndexColumnInfo, ColumnType, IndexError, IndexOptions,
    TableInfo as IndexTableInfo, build_index_info, check_index_prefix_length, remove_index_info,
};
use crate::modify_column::{
    ModifyColumnArgs, ModifyColumnContext, ModifyColumnError, ModifyColumnType,
    advance_modify_column, set_default_for_modified_column,
};
use crate::table::{
    GcController, TableCatalog, TableInfo, TableState, alter_auto_id_cache,
    alter_charset_and_collation, rebase_auto_increment,
};

/// 将 SQL 文本解析为 CREATE TABLE 语句的 AST（抽象语法树）节点。
///
/// 先通过解析器解析出单条语句，再向下转型为 `CreateTableStmt`，
/// 最后手工克隆各字段构造新的 Box（`Select` 置空），供后续构建表元数据使用。
fn parse_create(sql: &str) -> Box<ast::CreateTableStmt> {
    let mut parser = astersql_parser::New();
    let statement = parser
        .ParseOneStmt(sql, "", "")
        .expect("parse CREATE TABLE");
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

/// 从 CREATE TABLE 语句构建表元数据（`model::TableInfo`），失败时 panic。
fn build(sql: &str) -> model::TableInfo {
    let statement = parse_create(sql);
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    BuildTableInfoFromAST(&context, &statement).expect("build table metadata")
}

/// 判断给定 CREATE TABLE 语句在构建表元数据时是否报错（用于负面用例）。
fn build_error(sql: &str) -> bool {
    let statement = parse_create(sql);
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    BuildTableInfoFromAST(&context, &statement).is_err()
}

/// 构造一个处于 Public 状态的 DDL 列对象。
///
/// Schema 状态（SchemaState）是在线 DDL 的核心概念：列/索引会经历
/// None -> DeleteOnly -> WriteOnly -> Public 等状态渐进演化，
/// Public 表示对所有事务完全可见。字符串类列会附带 utf8mb4 字符集。
fn ddl_column(name: &str, kind: ColumnKind, flen: usize) -> DdlColumn {
    let mut field_type = FieldType::integer();
    field_type.kind = kind;
    field_type.flen = flen;
    if matches!(kind, ColumnKind::Varchar | ColumnKind::String) {
        field_type.charset = "utf8mb4".into();
        field_type.collation = "utf8mb4_bin".into();
    }
    let mut column = DdlColumn::new(name, field_type);
    column.state = SchemaState::Public;
    column
}

/// 用给定列名列表构造一张测试表，每列均为 int(11) 且置为 Public 状态。
fn column_table(names: &[&str]) -> DdlColumnTable {
    let mut table = DdlColumnTable::new(1, "t");
    for name in names {
        let id =
            init_and_add_column_to_table(&mut table, ddl_column(name, ColumnKind::Integer, 11));
        table
            .columns
            .iter_mut()
            .find(|column| column.id == id)
            .unwrap()
            .state = SchemaState::Public;
    }
    table
}

/// 构造索引模块使用的列信息；`charset_max_bytes` 为该字符集下单字符最大字节数，
/// 用于计算索引键长度上限（如 utf8mb4 为 4 字节）。
fn index_column(name: &str, column_type: ColumnType, charset_max_bytes: usize) -> IndexColumnInfo {
    IndexColumnInfo {
        id: 1,
        name: name.into(),
        column_type,
        charset_max_bytes,
        generated: false,
        stored: false,
        hidden: false,
        nullable: true,
        primary_key: false,
        index_flags: 0,
        generated_dependencies: BTreeSet::new(),
    }
}

/// 用给定列构造索引模块使用的表信息（初始无索引、不分区）。
fn index_table(columns: Vec<IndexColumnInfo>) -> IndexTableInfo {
    IndexTableInfo {
        id: 1,
        columns,
        indices: Vec::new(),
        max_index_id: 0,
        partitioned: false,
    }
}

/// 构造表目录（catalog）中的表元数据，默认 utf8mb4 字符集、Public 状态。
/// catalog 是数据库中记录所有 schema/表定义的元数据集合。
fn catalog_table(id: i64, schema_id: i64, name: &str) -> TableInfo {
    TableInfo {
        id,
        schema_id,
        name: name.into(),
        state: TableState::Public,
        partition_ids: Vec::new(),
        auto_increment_id: 0,
        auto_random_id: 0,
        auto_id_cache: 0,
        auto_id_schema_id: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        charset: "utf8mb4".into(),
        collation: "utf8mb4_bin".into(),
        version: 0,
        foreign_keys: Vec::new(),
        tiflash_replica: None,
        placement_policy: None,
        attributes: BTreeMap::new(),
        cached: false,
        affinity: None,
        split_policy: None,
    }
}

/// 构造修改列（MODIFY COLUMN）的默认上下文：启用严格 SQL 模式、
/// 不禁用有损优化，并给出 auto_random 的默认范围位与最大分片位。
fn modify_context() -> ModifyColumnContext {
    ModifyColumnContext {
        strict_sql_mode: true,
        partition: None,
        disable_lossy_optimization: false,
        auto_random_range_bits_default: 64,
        auto_random_shard_bits_max: 15,
    }
}

/// 构造由旧列改为新列的 MODIFY COLUMN 参数（位置与类型均取默认值）。
fn modify_args(old: &DdlColumn, new: DdlColumn) -> ModifyColumnArgs {
    ModifyColumnArgs {
        old_column_name: old.name.clone(),
        old_column_id: old.id,
        column: new,
        position: ColumnPosition::None,
        modify_type: ModifyColumnType::None,
        changing_column_id: None,
        changing_index_ids: Vec::new(),
        redundant_index_ids: Vec::new(),
        old_elements: Vec::new(),
        new_elements: Vec::new(),
        new_shard_bits: 0,
        new_range_bits: 0,
    }
}

/// 验证 CREATE TABLE IF NOT EXISTS ... LIKE 语法：正确解析出
/// IfNotExists 标记与被引用表（ReferTable）名。
#[test]
fn test_create_table_if_not_exists_like() {
    let statement = parse_create("create table if not exists ct like ct1");
    assert!(statement.IfNotExists);
    assert_eq!(statement.ReferTable.as_ref().unwrap().Name.L, "ct1");
    assert!(
        build("create table if not exists ct(b bigint, c varchar(60))")
            .Columns
            .len()
            == 2
    );
}

/// 验证使用非保留关键字（pump、drainer 等）作为列名可正常建表。
#[test]
fn test_create_table_with_key_word() {
    let table = build(
        "create table t1(pump varchar(20), drainer varchar(20), node_id varchar(20), node_state varchar(20))",
    );
    assert_eq!(
        table
            .Columns
            .iter()
            .map(|column| column.Name.L.as_str())
            .collect::<Vec<_>>(),
        ["pump", "drainer", "node_id", "node_state"]
    );
}

/// 验证唯一键（UNIQUE KEY）列不会被强制加 NOT NULL 标记：
/// 与主键不同，唯一索引允许 NULL 值。
#[test]
fn test_unique_key_null_value() {
    let table = build("create table t(a int primary key, b varchar(255), unique key b(b))");
    assert_eq!(table.Indices.len(), 1);
    assert!(!model::mysql::HasNotNullFlag(table.Columns[1].GetFlag()));
    assert!(table.Indices[0].Unique);
}

/// 验证聚簇索引（clustered index，数据按主键顺序存储）场景：
/// 非整数复合主键使表使用公共句柄（IsCommonHandle），
/// 唯一键列仍可为 NULL。
#[test]
fn test_unique_key_null_value_cluster_index() {
    let table = build(
        "create table t(a varchar(10), b float, c varchar(255), primary key(a,b), unique key c(c))",
    );
    assert!(table.IsCommonHandle);
    assert!(
        table
            .Indices
            .iter()
            .any(|index| index.Unique && index.Name.L == "c")
    );
    assert!(!model::mysql::HasNotNullFlag(table.Columns[2].GetFlag()));
}

/// 验证在已有前缀索引的列上执行 MODIFY COLUMN（int -> varchar(50)）：
/// 通过循环推进在线修改列状态机直至完成，最终列长度应为 50。
#[test]
fn test_modify_column_after_add_index() {
    let mut table = column_table(&["city"]);
    table.indices.push(DdlIndex {
        id: 1,
        name: "city".into(),
        state: SchemaState::Public,
        columns: vec![DdlIndexColumn {
            name: "city".into(),
            offset: 0,
            length: Some(2),
            use_changing_type: false,
        }],
        primary: false,
        columnar: false,
    });
    let old = table.columns[0].clone();
    let new = ddl_column("city", ColumnKind::Varchar, 50);
    let mut args = modify_args(&old, new);
    let mut version = 0;
    // 逐步推进修改列的状态机，每次调用对应一次 schema 版本变更，直到 finished。
    loop {
        let outcome = advance_modify_column(
            &mut table,
            &mut args,
            &modify_context(),
            true,
            &mut version,
            false,
        )
        .unwrap();
        if outcome.finished {
            break;
        }
    }
    assert_eq!(table.columns[0].field_type.flen, 50);
}

/// 验证 MODIFY COLUMN 引用了不存在的旧列 ID 时返回 ColumnNotFound 错误。
#[test]
fn test_modify_column_old_column_id_not_found() {
    let mut table = column_table(&["a", "b"]);
    let old = table.columns[0].clone();
    let mut args = modify_args(&old, ddl_column("a", ColumnKind::Varchar, 16));
    args.old_column_id = 999;
    let mut version = 0;
    assert_eq!(
        advance_modify_column(
            &mut table,
            &mut args,
            &modify_context(),
            true,
            &mut version,
            false
        ),
        Err(ModifyColumnError::ColumnNotFound("a".into()))
    );
}

/// 回归 issue #2293：int 列使用非法字符串默认值应报错，合法建表不受影响。
#[test]
fn test_issue2293() {
    assert!(build_error(
        "create table t_issue_2293(a int, b int not null default 'a')"
    ));
    assert_eq!(build("create table t_issue_2293(a int)").Columns.len(), 1);
}

/// 回归 issue #19229：enum/set 类型的枚举元素应正确保存在列元数据中。
#[test]
fn test_issue19229() {
    let enum_table = build("create table enumt(type enum('a','b'))");
    let set_table = build("create table sett(type set('a','b'))");
    assert_eq!(enum_table.Columns[0].GetElems(), ["a", "b"]);
    assert_eq!(set_table.Columns[0].GetElems(), ["a", "b"]);
}

/// 验证索引键长度计算与上限校验：
/// int 列索引长度为 4 字节；text 列前缀索引 768 字符 * 4 字节 = 3072，
/// 恰好等于默认最大索引长度 3072。
#[test]
fn test_index_length() {
    let columns = vec![
        index_column("a", ColumnType::Int, 1),
        index_column("b", ColumnType::Timestamp, 1),
        index_column("c", ColumnType::Text, 4),
    ];
    let mut table = index_table(columns);
    let a = build_index_info(
        &mut table,
        "a",
        &[("a".into(), None)],
        IndexOptions::default(),
    )
    .unwrap();
    assert_eq!(
        check_index_prefix_length(
            &table.columns,
            &a.columns,
            crate::index::ColumnarIndexType::None,
            3072
        ),
        Ok(4)
    );
    let text = build_index_info(
        &mut table,
        "c",
        &[("c".into(), Some(768))],
        IndexOptions::default(),
    )
    .unwrap();
    assert_eq!(
        check_index_prefix_length(
            &table.columns,
            &text.columns,
            crate::index::ColumnarIndexType::None,
            3072
        ),
        Ok(3072)
    );
}

/// 回归 issue #2858/#2717：bit 列的二进制字面量默认值与
/// int 列的十六进制字面量默认值均应被接受。
#[test]
fn test_issue2858_and2717() {
    let bit = build("create table t_bit(a bit(64) default b'0')");
    let hex = build("create table t_hex(a int default 0x123)");
    assert!(bit.Columns[0].GetDefaultValue().is_some());
    assert!(hex.Columns[0].GetDefaultValue().is_some());
}

/// 回归 issue #4432：bit 列默认值支持字符串、十六进制、十进制、二进制多种写法。
#[test]
fn test_issue4432() {
    for default in ["'a'", "0x61", "97", "0b1100001"] {
        let table = build(&format!("create table tx(col bit(10) default {default})"));
        assert!(table.Columns[0].GetDefaultValue().is_some());
    }
}

/// 回归 issue #5092：新增列后按 AFTER 语义移动列位置，再删除列应成功
/// 且不影响其余列的顺序。
#[test]
fn test_issue5092() {
    let mut table = column_table(&["a", "b", "c"]);
    let d = init_and_add_column_to_table(&mut table, ddl_column("d", ColumnKind::Integer, 11));
    table.move_column_info((d - 1) as usize, 2).unwrap();
    assert_eq!(
        table
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "d", "c"]
    );
    assert_eq!(
        remove_column_and_single_indices(&mut table, 2),
        Ok(Vec::new())
    );
}

/// 验证时间类型的小数秒精度（fsp）限制：最大为 6，7 应报错；0 合法。
#[test]
fn test_table_ddl_with_time_type() {
    assert!(build_error("create table t(a time(7))"));
    assert!(build_error("create table t(a datetime(7))"));
    assert!(build_error("create table t(a timestamp(7))"));
    let table = build("create table t(a datetime(0))");
    assert_eq!(table.Columns[0].GetDecimal(), 0);
}

/// 验证向一张表新增带默认值的列不会影响另一张同构表的列数。
#[test]
fn test_update_multiple_table() {
    let mut left = column_table(&["c1", "c2"]);
    let right = column_table(&["c1", "c2"]);
    let id = init_and_add_column_to_table(&mut left, ddl_column("c3", ColumnKind::Integer, 20));
    left.columns
        .iter_mut()
        .find(|column| column.id == id)
        .unwrap()
        .default_value = Some(DdlDefault::Integer(9));
    assert_eq!(right.columns.len(), 2);
    assert_eq!(left.columns[2].default_value, Some(DdlDefault::Integer(9)));
}

/// 验证虚拟生成列（VIRTUAL generated column，值在读取时实时计算）
/// 可以被标记为 IsGenerated 并作为索引列。
#[test]
fn test_null_generated_column() {
    let table = build(
        "create table t(a int, b int, c int generated always as (a+b) virtual, key idx_c(c))",
    );
    assert!(table.Columns[2].IsGenerated());
    assert_eq!(table.Indices[0].Columns[0].Name.L, "c");
}

/// 验证生成列的依赖检查：
/// - 依赖不存在的列报 UnknownColumn；
/// - 新列被移到被依赖生成列之前（FIRST）时报 NonPriorColumn，
///   因为生成列只能引用定义在其之前的列。
#[test]
fn test_depended_generated_column_prior2_generated_column() {
    let mut dependencies = HashSet::from(["c".to_owned(), "f".to_owned()]);
    let columns = vec![
        ddl_column("a", ColumnKind::Integer, 11),
        ddl_column("c", ColumnKind::Integer, 11),
    ];
    assert_eq!(
        check_depended_columns_exist(&mut dependencies, &columns),
        Err(GeneratedColumnError::UnknownColumn("f".into()))
    );
    let mut generated = columns;
    generated[1].generated = true;
    assert_eq!(
        verify_column_generation_single(
            &HashSet::from(["c".into()]),
            &generated,
            &ColumnPosition::First
        ),
        Err(GeneratedColumnError::NonPriorColumn("c".into()))
    );
    assert!(
        verify_column_generation_single(
            &HashSet::from(["c".into()]),
            &generated,
            &ColumnPosition::None
        )
        .is_ok()
    );
}

/// 验证修改表字符集：latin1 -> utf8mb4 合法；
/// 字符集与排序规则不匹配（utf8 + latin1_bin）时报错。
#[test]
fn test_changing_table_charset() {
    let mut table = catalog_table(1, 1, "t");
    table.charset = "latin1".into();
    table.collation = "latin1_bin".into();
    assert!(alter_charset_and_collation(&mut table, "utf8mb4", "utf8mb4_bin").unwrap());
    assert_eq!(
        alter_charset_and_collation(&mut table, "utf8", "latin1_bin"),
        Err(crate::table::TableError::InvalidCharsetCollation)
    );
}

/// 验证修改列默认值时同步更新 origin_default_value（原始默认值，
/// 用于为旧数据行回填该列的值）。
#[test]
fn test_modify_column_option() {
    let mut column = ddl_column("a", ColumnKind::Integer, 11);
    set_default_for_modified_column(&mut column, Some(DdlDefault::Integer(7)));
    assert_eq!(column.default_value, Some(DdlDefault::Integer(7)));
    assert_eq!(column.origin_default_value, column.default_value);
}

/// 构建含生成列与指定索引的表，并断言索引与生成列均存在。
fn generated_index_table(sql: &str, index_name: &str) -> model::TableInfo {
    let table = build(sql);
    assert!(table.Indices.iter().any(|index| index.Name.L == index_name));
    assert!(table.Columns.iter().any(|column| column.IsGenerated()));
    table
}

/// 验证在链式依赖的生成列（c 依赖 b，b 依赖 a）上建索引，依赖关系被记录。
#[test]
fn test_index_on_multiple_generated_column() {
    let table = generated_index_table(
        "create table t(a int, b int as(a+1), c int as(b+1), index idx(c))",
        "idx",
    );
    assert!(table.Columns[2].Dependences.contains_key("b"));
}

/// 验证跨类型（bigint -> decimal -> varchar）的生成列链上建索引。
#[test]
fn test_index_on_multiple_generated_column1() {
    let table = generated_index_table(
        "create table t(a bigint, b decimal as(a+1), c varchar(20) as(b*2), index idx(c))",
        "idx",
    );
    assert_eq!(table.Columns.len(), 3);
}

/// 验证生成列表达式同时依赖普通列与其他生成列（含函数调用）的场景。
#[test]
fn test_index_on_multiple_generated_column2() {
    let table = generated_index_table(
        "create table t(a bigint, b decimal as(a+1), c varchar(20) as(b*2), d float as(a*23+b-1+length(c)), index idx(d))",
        "idx",
    );
    assert!(table.Columns[3].Dependences.contains_key("c"));
}

/// 验证含字符串函数（length/right/ascii）的多级生成列上建索引。
#[test]
fn test_index_on_multiple_generated_column3() {
    let table = generated_index_table(
        "create table t(a varchar(10), b float as(length(a)+123), c varchar(20) as(right(a,2)), d float as(b+b-7+1-3+3*ascii(c)), index idx(d))",
        "idx",
    );
    assert_eq!(table.Indices[0].Columns[0].Name.L, "d");
}

/// 验证生成列 e 依赖前面全部 4 列时依赖集合大小正确。
#[test]
fn test_index_on_multiple_generated_column4() {
    let table = generated_index_table(
        "create table t(a bigint, b decimal as(a), c int as(a+b), d float as(a+b+c), e decimal as(a+b+c+d), index idx(d))",
        "idx",
    );
    assert_eq!(table.Columns[4].Dependences.len(), 4);
}

/// 验证为多个链式生成列各自建索引，共产生 3 个索引。
#[test]
fn test_index_on_multiple_generated_column5() {
    let table = generated_index_table(
        "create table t(a bigint, b bigint as(a+1), c bigint as(b+1), d bigint as(c+1), index idx_b(b), index idx_c(c), index idx_d(d))",
        "idx_d",
    );
    assert_eq!(table.Indices.len(), 3);
}

/// 验证字符集与排序规则名不区分大小写，统一规范化为小写存储。
#[test]
fn test_case_insensitive_charset_and_collate() {
    let table = build("create table t(id int) default charset=Utf8mb4 collate=UTF8MB4_GENERAL_CI");
    assert_eq!(table.Charset, "utf8mb4");
    assert_eq!(table.Collate, "utf8mb4_general_ci");
}

/// 验证 year 类型隐含 unsigned，zerofill 修饰同时隐含 unsigned 与 zerofill 标记。
#[test]
fn test_zero_fill_create_table() {
    let table = build("create table abc(y year, z tinyint(10) zerofill, primary key(y))");
    assert!(model::mysql::HasUnsignedFlag(table.Columns[0].GetFlag()));
    assert!(model::mysql::HasUnsignedFlag(table.Columns[1].GetFlag()));
    assert!(model::mysql::HasZerofillFlag(table.Columns[1].GetFlag()));
}

/// 验证 bit 列的十进制/二进制字面量默认值可保存，default null 则无默认值。
#[test]
fn test_bit_default_value() {
    let table = build(
        "create table t_bit(c1 bit(10) default 250, c2 bit(16) default b'1100110111001', c3 bit default null)",
    );
    assert!(table.Columns[0].GetDefaultValue().is_some());
    assert!(table.Columns[1].GetDefaultValue().is_some());
    assert!(table.Columns[2].GetDefaultValue().is_none());
}

/// 验证列数上限检查：超过 4096 列报 TooManyColumns，恰好等于上限则通过。
#[test]
fn test_create_table_too_large() {
    assert_eq!(
        check_add_column_too_many_columns(4097, 4096),
        Err(ColumnError::TooManyColumns {
            count: 4097,
            limit: 4096
        })
    );
    assert_eq!(check_add_column_too_many_columns(4096, 4096), Ok(()));
}

/// 验证 ALTER ... AFTER 语义下的列偏移定位与移动；
/// 引用不存在的锚点列时报 ColumnNotFound。
#[test]
fn test_change_column_position() {
    let mut table = column_table(&["a", "b", "c", "d"]);
    assert_eq!(
        locate_offset_to_move(3, &ColumnPosition::After("a".into()), &table),
        Ok(1)
    );
    table.move_column_info(3, 1).unwrap();
    assert_eq!(
        table
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "d", "b", "c"]
    );
    assert_eq!(
        locate_offset_to_move(2, &ColumnPosition::After("missing".into()), &table),
        Err(ColumnError::ColumnNotFound("missing".into()))
    );
}

/// 验证新增列后创建唯一索引，以及索引列数超过 16 的上限检查。
#[test]
fn test_add_index_after_add_column() {
    let mut table = index_table(vec![index_column("c", ColumnType::Int, 1)]);
    let index = build_index_info(
        &mut table,
        "cc",
        &[("c".into(), None)],
        IndexOptions {
            unique: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(index.unique);
    let too_many = (0..17).map(|i| (format!("f{i}"), None)).collect::<Vec<_>>();
    assert_eq!(
        crate::index::build_index_columns(
            &table.columns,
            &too_many,
            crate::index::ColumnarIndexType::None
        ),
        Err(IndexError::TooManyKeyParts {
            actual: 17,
            maximum: 16
        })
    );
}

/// 验证列级字符集缺省时继承表级默认字符集（latin1）。
#[test]
fn test_resolve_charset() {
    let table = build("create table resolve_charset(a varchar(255)) default charset=latin1");
    assert_eq!(table.Charset, "latin1");
    assert_eq!(table.Columns[0].GetCharset(), "latin1");
}

/// 验证 default now(6) 与 on update now(6)：所有列都有默认值，
/// 且第二列带 ON UPDATE NOW（更新行时自动刷新时间戳）标记。
#[test]
fn test_add_column_default_now() {
    let table = build(
        "create table t(c1 timestamp(6) default now(6), c2 timestamp(6) default now(6) on update now(6), c3 datetime(6) default now(6))",
    );
    assert!(
        table
            .Columns
            .iter()
            .all(|column| column.GetDefaultValue().is_some())
    );
    assert!(model::mysql::HasOnUpdateNowFlag(table.Columns[1].GetFlag()));
}

/// 验证 ALTER COLUMN SET DEFAULT / DROP DEFAULT：默认值可反复覆盖并清除。
#[test]
fn test_alter_column() {
    let mut column = ddl_column("a", ColumnKind::Integer, 11);
    set_default_for_modified_column(&mut column, Some(DdlDefault::Integer(111)));
    set_default_for_modified_column(&mut column, Some(DdlDefault::Integer(222)));
    assert_eq!(column.default_value, Some(DdlDefault::Integer(222)));
    set_default_for_modified_column(&mut column, None);
    assert_eq!(column.default_value, None);
}

/// 验证 ALTER 算法协商：普通变更默认选 INSTANT（仅改元数据的即时算法）；
/// 加约束请求更优的 INSTANT 时无可用算法，返回 DEFAULT 并给出错误。
#[test]
fn test_alter_algorithm() {
    assert_eq!(
        resolve_alter_algorithm(AlterKind::Other, AlgorithmType::Default),
        (AlgorithmType::Instant, None)
    );
    let (selected, error) =
        resolve_alter_algorithm(AlterKind::AddConstraint, AlgorithmType::Instant);
    assert_eq!(selected, AlgorithmType::Default);
    assert!(error.is_some());
}

/// 验证旧版 utf8（最多 3 字节）表可升级为 utf8mb4（完整 4 字节 UTF-8）。
#[test]
fn test_treat_old_version_utf8_as_utf8mb4() {
    let mut table = catalog_table(1, 1, "t");
    table.charset = "utf8".into();
    table.collation = "utf8_bin".into();
    assert!(alter_charset_and_collation(&mut table, "utf8mb4", "utf8mb4_bin").unwrap());
    assert_eq!(
        (&table.charset, &table.collation),
        (&"utf8mb4".to_owned(), &"utf8mb4_bin".to_owned())
    );
}

/// 验证表达式默认值 rand()：建表时允许（记为 DefaultIsExpr），
/// 但 ADD COLUMN 时因需为旧行回填确定值而拒绝非确定性函数。
#[test]
fn test_default_column_with_rand() {
    for sql in [
        "create table t(a double default (rand()))",
        "create table t(a double default (rand(1)))",
    ] {
        let table = build(sql);
        assert!(table.Columns[0].DefaultIsExpr, "{sql}");
        assert!(table.Columns[0].GetDefaultValue().is_some(), "{sql}");
    }
    let definition = ColumnDefinition {
        name: "b".into(),
        field_type: FieldType::integer(),
        constraints: Vec::new(),
        default_value: Some(DdlDefault::Expression("rand(2)".into())),
        comment: String::new(),
        generated: None,
    };
    assert_eq!(
        create_new_column(
            &column_table(&["a"]),
            &definition,
            &ColumnPosition::None,
            false,
            true,
        ),
        Err(AddColumnError::UnsafeDefaultFunction("rand".into()))
    );
}

/// 验证表达式默认值标记：uuid()/abs() 记为表达式，
/// current_timestamp 是内建时间默认值而非普通表达式。
#[test]
fn test_default_value_as_expressions() {
    let table = build(
        "create table t(a varchar(64) default (uuid()), b int default (abs(-2)), c timestamp default current_timestamp)",
    );
    assert!(table.Columns[0].DefaultIsExpr);
    assert!(table.Columns[1].DefaultIsExpr);
    assert!(!table.Columns[2].DefaultIsExpr);
}

/// 验证同库不同表可分别修改为不同字符集，互不影响。
#[test]
fn test_changing_db_charset() {
    let mut first = catalog_table(1, 1, "t1");
    let mut second = catalog_table(2, 1, "t2");
    assert!(alter_charset_and_collation(&mut first, "latin1", "latin1_bin").unwrap());
    assert!(alter_charset_and_collation(&mut second, "utf8mb4", "utf8mb4_general_ci").unwrap());
    assert_ne!(first.charset, second.charset);
}

/// 验证生成列表达式中的函数合法性：非确定性函数 rand 被拒绝，
/// lower 等确定性函数允许；同时验证表达式中列名的提取（统一小写）。
#[test]
fn test_sql_functions_in_generated_columns() {
    let unsupported = ExpressionNode::Function {
        name: "rand".into(),
        supported: false,
        guaranteed_available: false,
        arguments: Vec::new(),
    };
    assert_eq!(
        check_illegal_function_for_generated("g", GenerationType::Column, &unsupported, true),
        Err(GeneratedColumnError::IllegalFunction("rand".into()))
    );
    let supported = ExpressionNode::Function {
        name: "lower".into(),
        supported: true,
        guaranteed_available: true,
        arguments: vec![ExpressionNode::Column("A".into())],
    };
    assert!(
        check_illegal_function_for_generated("g", GenerationType::Column, &supported, true).is_ok()
    );
    assert_eq!(
        find_column_names_in_expr(&supported),
        HashSet::from(["a".into()])
    );
}

/// 对应 Go `TestSchemaNameAndTableNameInGeneratedExpr`：当前库表限定合法，
/// 保存元数据时移除限定名。
#[test]
fn test_schema_name_and_table_name_in_generated_expr() {
    let table = build("create table test.t(a int, b int as(lower(test.t.a)))");
    assert_eq!(table.Columns[1].GeneratedExprString, "lower(`a`)");
    assert!(build_error(
        "create table test.t(a int, b int as(lower(test1.t.a)))"
    ));
    assert!(build_error(
        "create table test.t(a int, b int as(lower(test.t1.a)))"
    ));
}

/// 回归 parser issue #284：带命名约束的外键（FOREIGN KEY）应正确
/// 解析出约束名、引用表与引用列。
#[test]
fn test_parser_issue284() {
    let table = build(
        "create table t2(id int primary key, c1 int not null, constraint fk foreign key(c1) references t1(c1))",
    );
    assert_eq!(table.ForeignKeys.len(), 1);
    assert_eq!(table.ForeignKeys[0].Name.L, "fk");
    assert_eq!(table.ForeignKeys[0].RefTable.L, "t1");
    assert_eq!(table.ForeignKeys[0].RefCols[0].L, "c1");
}

/// 验证表达式索引（对 lower(a) 等表达式建索引）：底层实现为
/// 自动生成的隐藏（Hidden）虚拟列，索引指向该隐藏列。
#[test]
fn test_add_expression_index() {
    let table = build("create table t(a varchar(20), index idx((lower(a))))");
    let hidden = table.Columns.iter().find(|column| column.Hidden).unwrap();
    assert!(hidden.GeneratedExprString.contains("lower(`a`)"));
    assert_eq!(table.Indices[0].Columns[0].Name, hidden.Name);
}

/// 验证列被复合索引（多列索引）引用时不允许直接删除该列。
#[test]
fn test_drop_column_with_composite_index() {
    let mut table = column_table(&["a", "b"]);
    table.indices.push(DdlIndex {
        id: 1,
        name: "ab".into(),
        state: SchemaState::Public,
        columns: vec![
            DdlIndexColumn {
                name: "a".into(),
                offset: 0,
                length: None,
                use_changing_type: false,
            },
            DdlIndexColumn {
                name: "b".into(),
                offset: 1,
                length: None,
                use_changing_type: false,
            },
        ],
        primary: false,
        columnar: false,
    });
    assert_eq!(
        remove_column_and_single_indices(&mut table, 1),
        Err(ColumnError::CannotDropIndexedColumn("a".into()))
    );
}

/// 验证删除列时其单列索引会被一并删除，并返回被删索引的 ID。
#[test]
fn test_drop_column_with_index() {
    let mut table = column_table(&["a", "b"]);
    table.indices.push(DdlIndex {
        id: 9,
        name: "a".into(),
        state: SchemaState::Public,
        columns: vec![DdlIndexColumn {
            name: "a".into(),
            offset: 0,
            length: None,
            use_changing_type: false,
        }],
        primary: false,
        columnar: false,
    });
    assert_eq!(remove_column_and_single_indices(&mut table, 1), Ok(vec![9]));
    assert!(table.indices.is_empty());
}

/// 验证删除自增（auto_increment）列本身是允许的，剩余列顺序正确。
#[test]
fn test_drop_column_with_auto_inc() {
    let mut table = column_table(&["id", "v"]);
    table.columns[0].auto_increment = true;
    assert_eq!(
        remove_column_and_single_indices(&mut table, 1),
        Ok(Vec::new())
    );
    assert_eq!(table.columns[0].name, "v");
}

/// 验证一列上存在多个单列索引时，删除该列会同时删除全部相关索引。
#[test]
fn test_drop_column_with_multi_index() {
    let mut table = column_table(&["a", "b", "c"]);
    for (id, name) in [(1, "ia"), (2, "ia2")] {
        table.indices.push(DdlIndex {
            id,
            name: name.into(),
            state: SchemaState::Public,
            columns: vec![DdlIndexColumn {
                name: "a".into(),
                offset: 0,
                length: None,
                use_changing_type: false,
            }],
            primary: false,
            columnar: false,
        });
    }
    assert_eq!(
        remove_column_and_single_indices(&mut table, 1),
        Ok(vec![1, 2])
    );
}

/// 验证依次删除多列时，各自的单列索引分别被正确删除。
#[test]
fn test_drop_columns_with_multi_index() {
    let mut table = column_table(&["a", "b", "c"]);
    table.indices.push(DdlIndex {
        id: 1,
        name: "a".into(),
        state: SchemaState::Public,
        columns: vec![DdlIndexColumn {
            name: "a".into(),
            offset: 0,
            length: None,
            use_changing_type: false,
        }],
        primary: false,
        columnar: false,
    });
    table.indices.push(DdlIndex {
        id: 2,
        name: "b".into(),
        state: SchemaState::Public,
        columns: vec![DdlIndexColumn {
            name: "b".into(),
            offset: 1,
            length: None,
            use_changing_type: false,
        }],
        primary: false,
        columnar: false,
    });
    assert_eq!(remove_column_and_single_indices(&mut table, 1), Ok(vec![1]));
    assert_eq!(remove_column_and_single_indices(&mut table, 2), Ok(vec![2]));
}

/// 验证表级 auto_increment=100 选项：起始自增值与列上的自增标记均正确。
#[test]
fn test_auto_increment_table_option() {
    let table = build("create table t(id bigint auto_increment primary key) auto_increment=100");
    assert_eq!(table.AutoIncID, 100);
    assert!(model::mysql::HasAutoIncrementFlag(
        table.Columns[0].GetFlag()
    ));
}

/// 验证自增 ID rebase（重设基准值）：普通模式不允许回退，
/// force 模式可强制回退到更小的值。
#[test]
fn test_auto_increment_force() {
    let mut table = catalog_table(1, 1, "t");
    assert!(rebase_auto_increment(&mut table, 100, false).unwrap());
    assert!(!rebase_auto_increment(&mut table, 50, false).unwrap());
    assert!(rebase_auto_increment(&mut table, 50, true).unwrap());
    assert_eq!(table.auto_increment_id, 50);
}

/// 验证设置 auto_id_cache（自增 ID 批量缓存大小）后强制 rebase 仍生效。
#[test]
fn test_auto_increment_force_auto_id_cache() {
    let mut table = catalog_table(1, 1, "t");
    assert!(alter_auto_id_cache(&mut table, 100));
    assert!(rebase_auto_increment(&mut table, 10, true).unwrap());
    assert_eq!((table.auto_id_cache, table.auto_increment_id), (100, 10));
}

/// 回归 issue #20490：先添加 NOT NULL 且有默认值的列，
/// 再通过 MODIFY COLUMN 状态机将其改为可空并清除默认值。
#[test]
fn test_issue20490() {
    let mut table = column_table(&["a"]);
    let definition = ColumnDefinition {
        name: "b".into(),
        field_type: FieldType::integer(),
        constraints: vec![ColumnConstraint::NotNull],
        default_value: Some(DdlDefault::Integer(1)),
        comment: String::new(),
        generated: None,
    };
    let added = create_new_column(&table, &definition, &ColumnPosition::None, false, true).unwrap();
    let added_id = init_and_add_column_to_table(&mut table, added);
    let added = table
        .columns
        .iter_mut()
        .find(|column| column.id == added_id)
        .unwrap();
    added.state = SchemaState::Public;
    assert!(added.not_null);
    assert_eq!(added.default_value, Some(DdlDefault::Integer(1)));

    let old = added.clone();
    let mut nullable = old.clone();
    nullable.not_null = false;
    nullable.default_value = None;
    nullable.origin_default_value = None;
    let mut args = modify_args(&old, nullable);
    let mut version = 0;
    // 循环推进在线修改列状态机直至完成。
    loop {
        let outcome = advance_modify_column(
            &mut table,
            &mut args,
            &modify_context(),
            true,
            &mut version,
            false,
        )
        .unwrap();
        if outcome.finished {
            break;
        }
    }
    let modified = table
        .columns
        .iter()
        .find(|column| column.name == "b")
        .unwrap();
    assert!(!modified.not_null);
    assert_eq!(modified.default_value, None);
}

/// 回归 issue #20741：enum 列带 NOT NULL、默认值与索引时元数据正确。
#[test]
fn test_issue20741_with_enum_field() {
    let table = build("create table t(a enum('x','y') not null default 'x', key(a))");
    assert_eq!(table.Columns[0].GetElems(), ["x", "y"]);
    assert!(table.Columns[0].GetDefaultValue().is_some());
}

/// 验证 enum 与 set 类型的默认值（set 支持逗号分隔的多值组合）。
#[test]
fn test_enum_and_set_default_value() {
    let table = build("create table t(a enum('x','y') default 'y', b set('a','b') default 'a,b')");
    assert!(
        table
            .Columns
            .iter()
            .all(|column| column.GetDefaultValue().is_some())
    );
    assert_eq!(table.Columns[1].GetElems(), ["a", "b"]);
}

/// 验证唯一键冲突错误消息的格式与 MySQL 兼容：
/// "Duplicate entry '值' for key '索引名'"。
#[test]
fn test_duplicate_error_message() {
    for value in ["1-a", "1-1", "1-1.1"] {
        let error = DdlUtilError::KeyExists {
            value: value.into(),
            index: "test.t_idx".into(),
        };
        assert_eq!(
            error.to_string(),
            format!("Duplicate entry '{value}' for key 'test.t_idx'")
        );
    }
}

/// 回归 issue #22028：double(0,0) 精度非法应报错，double(1,0)/float(0) 合法。
#[test]
fn test_issue22028() {
    assert!(build_error("create table t(a double(0,0))"));
    let valid = build("create table t(a double(1,0), b float(0))");
    assert_eq!(valid.Columns.len(), 2);
}

/// 验证临时表类型标记：本地临时表（会话内可见）与全局临时表
/// （表结构全局共享、数据按事务隔离，提交时清空）。
#[test]
fn test_create_temporary_table() {
    let local = build("create temporary table lt(a int)");
    let global = build("create global temporary table gt(a int) on commit delete rows");
    assert_eq!(local.TempTableType, model::TempTableLocal);
    assert_eq!(global.TempTableType, model::TempTableGlobal);
}

/// 验证 DROP DATABASE 时本地临时表随 schema 一起被删除，
/// 且其 "temporary" 属性保持为 local。
#[test]
fn test_access_local_tmp_table_after_drop_db() {
    let metadata = build("create temporary table t(a int)");
    let mut catalog = TableCatalog::default();
    catalog.create_schema(1);
    let mut temporary = catalog_table(1, 1, "t");
    temporary.attributes.insert(
        "temporary".into(),
        metadata.TempTableType.String().to_owned(),
    );
    catalog.insert(temporary).unwrap();
    let dropped = catalog.drop_schema(1).unwrap();
    assert_eq!(dropped[0].attributes.get("temporary").unwrap(), "local");
}

/// 验证本地临时表不是持久表，因此不允许在其上创建视图。
#[test]
fn test_avoid_create_view_on_local_temporary_table() {
    let temporary = build("create temporary table t(a int)");
    assert_eq!(temporary.TempTableType, model::TempTableLocal);
    let view_source_is_persistent = temporary.TempTableType == model::TempTableNone;
    assert!(!view_source_is_persistent);
}

/// 验证删表按在线 DDL 三步走：WriteOnly -> DeleteOnly -> None（彻底删除）。
#[test]
fn test_drop_temporary_table() {
    let metadata = build("create temporary table t(a int)");
    let mut catalog = TableCatalog::default();
    catalog
        .insert(catalog_table(1, 1, &metadata.Name.L))
        .unwrap();
    for expected in [
        TableState::WriteOnly,
        TableState::DeleteOnly,
        TableState::None,
    ] {
        assert_eq!(catalog.drop_table_step(1, "t", 10).unwrap(), expected);
    }
}

/// 验证 TRUNCATE 本地临时表：返回被替换的旧表 ID，并重置自增 ID。
#[test]
fn test_truncate_local_temporary_table() {
    let metadata = build("create temporary table t(a int)");
    let mut catalog = TableCatalog::default();
    let mut table = catalog_table(1, 1, &metadata.Name.L);
    table.auto_increment_id = 9;
    catalog.insert(table).unwrap();
    assert_eq!(
        catalog.truncate_table(1, "t", 2, Vec::new()).unwrap(),
        vec![1]
    );
    assert_eq!(catalog.get(1, "t").unwrap().auto_increment_id, 0);
}

/// 回归 issue #29282：模拟 SELECT ... FOR UPDATE 对目录的互斥访问。
/// 用 Mutex 模拟悲观锁、Barrier 协调两个线程的时序，
/// 验证锁被持有期间其他事务无法获得锁。
#[test]
fn test_issue29282() {
    let temporary = build("create temporary table issue29828_tmp(id int)");
    assert_eq!(temporary.TempTableType, model::TempTableLocal);
    let catalog = Arc::new(Mutex::new(TableCatalog::default()));
    {
        let mut guard = catalog.lock().unwrap();
        guard.insert(catalog_table(1, 1, "issue29828_t")).unwrap();
        let mut temp = catalog_table(2, 1, "issue29828_tmp");
        temp.attributes.insert("rows".into(), "1".into());
        guard.insert(temp).unwrap();
        let rows = guard.get(1, "issue29828_tmp").unwrap().attributes["rows"].clone();
        guard
            .get_mut(1, "issue29828_t")
            .unwrap()
            .attributes
            .insert("rows".into(), rows);
    }
    // locked：工作线程持锁后通知主线程；release：主线程校验完后允许释放锁。
    let locked = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        let worker_catalog = Arc::clone(&catalog);
        let worker_locked = Arc::clone(&locked);
        let worker_release = Arc::clone(&release);
        scope.spawn(move || {
            let _guard = worker_catalog.lock().unwrap();
            worker_locked.wait();
            worker_release.wait();
        });
        locked.wait();
        assert!(catalog.try_lock().is_err(), "FOR UPDATE remains blocked");
        release.wait();
    });
    assert_eq!(
        catalog
            .lock()
            .unwrap()
            .get(1, "issue29828_t")
            .unwrap()
            .attributes["rows"],
        "1"
    );
}

/// 验证 enum 元素为数字字符串（'0','1','2'）时默认值仍能正确解析。
#[test]
fn test_enum_default_value() {
    let table = build("create table t(a enum('0','1','2') default '0')");
    assert_eq!(table.Columns[0].GetElems(), ["0", "1", "2"]);
    assert!(table.Columns[0].GetDefaultValue().is_some());
}

/// 验证 DDL 作业（Job）的提交与完成流程：提交后可查询到原始 SQL，
/// 完成后状态变为 Synced（已同步到所有节点）。
#[test]
fn test_ddl_last_info() {
    let mut ddl = Ddl::new("last-info", Vec::new());
    ddl.started = true;
    ddl.submit_job(Job {
        id: 1,
        query: "create table t(a int)".into(),
        state: JobState::None,
        version: 0,
        start_ts: 1,
        real_start_ts: 0,
        action_type: crate::ddl::ActionType::Other,
        table_id: 1,
        schema_id: 1,
        paused_by: None,
    })
    .unwrap();
    assert_eq!(ddl.all_jobs()[0].query, "create table t(a int)");
    ddl.finish_job(1).unwrap();
    assert_eq!(ddl.all_jobs()[0].state, JobState::Synced);
}

/// 验证 utf8mb4 的默认排序规则为 utf8mb4_bin（按字节二进制比较）。
#[test]
fn test_default_collation_for_utf8mb4() {
    let table = build("create table t(a varchar(10)) default charset=utf8mb4");
    assert_eq!(table.Charset, "utf8mb4");
    assert_eq!(table.Collate, "utf8mb4_bin");
}

/// 验证 OPTIMIZE TABLE 语句返回不支持的错误。
#[test]
fn test_optimize_table() {
    assert_eq!(
        validate_optimize_table(),
        Err(ExecutorError::Unsupported(
            "OPTIMIZE TABLE is not supported".into()
        ))
    );
}

/// 回归 issue #52680：删表后在 GC（垃圾回收）安全点内执行 RECOVER TABLE，
/// 恢复的表应保留原表 ID、自增值与 auto_id_cache 设置，GC 保持开启。
#[test]
fn test_issue52680() {
    let mut catalog = TableCatalog::default();
    catalog.create_schema(1);
    let mut table = catalog_table(101, 1, "t");
    table.auto_increment_id = 4000;
    table.auto_id_cache = 1;
    catalog.insert(table).unwrap();
    for expected in [
        TableState::WriteOnly,
        TableState::DeleteOnly,
        TableState::None,
    ] {
        assert_eq!(catalog.drop_table_step(1, "t", 10).unwrap(), expected);
    }
    let mut gc = GcController {
        enabled: true,
        safe_point: 9,
    };
    assert_eq!(catalog.recover_table(101, &mut gc), Ok(true));
    let recovered = catalog.get(1, "t").unwrap();
    assert_eq!(recovered.id, 101);
    assert_eq!(recovered.auto_increment_id, 4000);
    assert_eq!(recovered.auto_id_cache, 1);
    assert!(gc.enabled);
}

/// 验证调整最大索引长度配置的效果：同一前缀索引在上限 3072 下合法，
/// 上限 1024 下报 KeyTooLong；最后删除该索引应成功。
#[test]
fn test_create_index_with_change_max_index_length() {
    let mut table = index_table(vec![index_column("a", ColumnType::VarChar(1024), 4)]);
    let index = build_index_info(
        &mut table,
        "a",
        &[("a".into(), Some(768))],
        IndexOptions::default(),
    )
    .unwrap();
    assert_eq!(
        check_index_prefix_length(
            &table.columns,
            &index.columns,
            crate::index::ColumnarIndexType::None,
            3072
        ),
        Ok(3072)
    );
    assert_eq!(
        check_index_prefix_length(
            &table.columns,
            &index.columns,
            crate::index::ColumnarIndexType::None,
            1024
        ),
        Err(IndexError::KeyTooLong {
            actual: 3072,
            maximum: 1024
        })
    );
    table.indices.push(index);
    assert!(remove_index_info(&mut table, "a").is_ok());
}
