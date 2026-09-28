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

// 规划器测试用的 Mock 表元信息与会话上下文。
//
// 提供有符号/无符号表、视图、分区表（Range/Hash/List）及全局索引等构造器，
// 供执行计划（execution plan）相关单测使用。下方大段注释保留 Go 完整语义。

// Go imports are retained below as parity-reference documentation.
// - context, fmt
// - pkg/domain, infoschema, meta/model, parser/ast, parser/auth, parser/mysql
// - sessionctx/vardef, types, util/mock
//
// pub fn newLongType() -> types::FieldType {
//     return *(types.NewFieldType(mysql.TypeLong))
// }
//
// pub fn newStringType() -> types::FieldType {
//     ft := types.NewFieldType(mysql.TypeVarchar)
//     charset, collate := types.DefaultCharsetForType(mysql.TypeVarchar)
//     ft.SetCharset(charset)
//     ft.SetCollate(collate)
//     return *ft
// }
//
// pub fn newDateType() -> types::FieldType {
//     ft := types.NewFieldType(mysql.TypeDate)
//     return *ft
// }
//
// Go 语义：MockSignedTable is only used for plan related tests.
// pub fn MockSignedTable() -> model::TableInfo {
// column: a, b, c, d, e, c_str, d_str, e_str, f, g, h, i_date
// PK: a
// indices: c_d_e, e, f, g, f_g, c_d_e_str, e_d_c_str_prefix
//     indices := []*model.IndexInfo{
//         {
//             ID:   1,
//             Name: ast.NewCIStr("c_d_e"),
//             Columns: []*model.IndexColumn{
//                 {
//                     Name:   ast.NewCIStr("c"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 2,
//                 },
//                 {
//                     Name:   ast.NewCIStr("d"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 3,
//                 },
//                 {
//                     Name:   ast.NewCIStr("e"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 4,
//                 },
//             },
//             State:  model.StatePublic,
//             Unique: true,
//         },
//         {
//             ID:   2,
//             Name: ast.NewCIStr("x"),
//             Columns: []*model.IndexColumn{
//                 {
//                     Name:   ast.NewCIStr("e"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 4,
//                 },
//             },
//             State:  model.StateWriteOnly,
//             Unique: true,
//         },
//         {
//             ID:   3,
//             Name: ast.NewCIStr("f"),
//             Columns: []*model.IndexColumn{
//                 {
//                     Name:   ast.NewCIStr("f"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 8,
//                 },
//             },
//             State:  model.StatePublic,
//             Unique: true,
//         },
//         {
//             ID:   4,
//             Name: ast.NewCIStr("g"),
//             Columns: []*model.IndexColumn{
//                 {
//                     Name:   ast.NewCIStr("g"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 9,
//                 },
//             },
//             State: model.StatePublic,
//         },
//         {
//             ID:   5,
//             Name: ast.NewCIStr("f_g"),
//             Columns: []*model.IndexColumn{
//                 {
//                     Name:   ast.NewCIStr("f"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 8,
//                 },
//                 {
//                     Name:   ast.NewCIStr("g"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 9,
//                 },
//             },
//             State:  model.StatePublic,
//             Unique: true,
//         },
//         {
//             ID:   6,
//             Name: ast.NewCIStr("c_d_e_str"),
//             Columns: []*model.IndexColumn{
//                 {
//                     Name:   ast.NewCIStr("c_str"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 5,
//                 },
//                 {
//                     Name:   ast.NewCIStr("d_str"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 6,
//                 },
//                 {
//                     Name:   ast.NewCIStr("e_str"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 7,
//                 },
//             },
//             State: model.StatePublic,
//         },
//         {
//             ID:   7,
//             Name: ast.NewCIStr("e_d_c_str_prefix"),
//             Columns: []*model.IndexColumn{
//                 {
//                     Name:   ast.NewCIStr("e_str"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 7,
//                 },
//                 {
//                     Name:   ast.NewCIStr("d_str"),
//                     Length: types.UnspecifiedLength,
//                     Offset: 6,
//                 },
//                 {
//                     Name:   ast.NewCIStr("c_str"),
//                     Length: 10,
//                     Offset: 5,
//                 },
//             },
//             State: model.StatePublic,
//         },
//     }
//     pkColumn := &model.ColumnInfo{
//         State:     model.StatePublic,
//         Offset:    0,
//         Name:      ast.NewCIStr("a"),
//         FieldType: newLongType(),
//         ID:        1,
//     }
//     col0 := &model.ColumnInfo{
//         State:     model.StatePublic,
//         Offset:    1,
//         Name:      ast.NewCIStr("b"),
//         FieldType: newLongType(),
//         ID:        2,
//     }
//     col1 := &model.ColumnInfo{
//         State:     model.StatePublic,
//         Offset:    2,
//         Name:      ast.NewCIStr("c"),
//         FieldType: newLongType(),
//         ID:        3,
//     }
//     col2 := &model.ColumnInfo{
//         State:     model.StatePublic,
//         Offset:    3,
//         Name:      ast.NewCIStr("d"),
//         FieldType: newLongType(),
//         ID:        4,
//     }
//     col3 := &model.ColumnInfo{
//         State:     model.StatePublic,
//         Offset:    4,
//         Name:      ast.NewCIStr("e"),
//         FieldType: newLongType(),
//         ID:        5,
//     }
//     colStr1 := &model.ColumnInfo{
//         State:     model.StatePublic,
//         Offset:    5,
//         Name:      ast.NewCIStr("c_str"),
//         FieldType: newStringType(),
//         ID:        6,
//     }
//     colStr2 := &model.ColumnInfo{
//         State:     model.StatePublic,
//         Offset:    6,
//         Name:      ast.NewCIStr("d_str"),
//         FieldType: newStringType(),
//         ID:        7,
//     }
//     colStr3 := &model.ColumnInfo{
//         State:     model.StatePublic,
//         Offset:    7,
//         Name:      ast.NewCIStr("e_str"),
//         FieldType: newStringType(),
//         ID:        8,
//     }
//     col4 := &model.ColumnInfo{
//         State:     model.StatePublic,
//         Offset:    8,
//         Name:      ast.NewCIStr("f"),
//         FieldType: newLongType(),
//         ID:        9,
//     }
//     col5 := &model.ColumnInfo{
//         State:     model.StatePublic,
//         Offset:    9,
//         Name:      ast.NewCIStr("g"),
//         FieldType: newLongType(),
//         ID:        10,
//     }
//     col6 := &model.ColumnInfo{
//         State:     model.StatePublic,
//         Offset:    10,
//         Name:      ast.NewCIStr("h"),
//         FieldType: newLongType(),
//         ID:        11,
//     }
//     col7 := &model.ColumnInfo{
//         State:     model.StatePublic,
//         Offset:    11,
//         Name:      ast.NewCIStr("i_date"),
//         FieldType: newDateType(),
//         ID:        12,
//     }
//     pkColumn.SetFlag(mysql.PriKeyFlag | mysql.NotNullFlag)
// Go 语义：Column 'b', 'c', 'd', 'f', 'g' is not null.
//     col0.SetFlag(mysql.NotNullFlag)
//     col1.SetFlag(mysql.NotNullFlag)
//     col2.SetFlag(mysql.NotNullFlag)
//     col4.SetFlag(mysql.NotNullFlag)
//     col5.SetFlag(mysql.NotNullFlag)
//     col6.SetFlag(mysql.NoDefaultValueFlag)
//     table := &model.TableInfo{
//         ID:         1,
//         Columns:    []*model.ColumnInfo{pkColumn, col0, col1, col2, col3, colStr1, colStr2, colStr3, col4, col5, col6, col7},
//         Indices:    indices,
//         Name:       ast.NewCIStr("t"),
//         PKIsHandle: true,
//         State:      model.StatePublic,
//     }
//     return table
// }
// */
/// 列字段类型桩（对应 MySQL/TiDB FieldType 子集）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldType {
    Unspecified,
    Long,
    Varchar,
    Date,
}
/// Schema 对象状态：DDL 在线变更中的可见性阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaState {
    None,
    WriteOnly,
    Public,
}
/// 分区类型：Range / Hash / List。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartitionType {
    Range,
    Hash,
    List,
}
/// 列元信息桩。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnInfo {
    pub id: i64,
    pub name: String,
    pub field_type: FieldType,
    pub offset: usize,
    pub state: SchemaState,
    pub primary_key: bool,
    pub not_null: bool,
    pub unsigned: bool,
    pub no_default: bool,
}
/// 索引元信息桩；`columns` 为 (列名, 可选前缀长度)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexInfo {
    pub id: i64,
    pub name: String,
    pub columns: Vec<(String, Option<usize>)>,
    pub column_offsets: Vec<usize>,
    pub state: SchemaState,
    pub unique: bool,
    pub global: bool,
}
/// 单个分区定义：Range 用 `less_than`，List 用 `in_values`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionDefinition {
    pub id: i64,
    pub name: String,
    pub less_than: Vec<String>,
    pub in_values: Vec<Vec<String>>,
}
/// 表级分区信息：类型、分区表达式与各分区定义。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionInfo {
    pub partition_type: PartitionType,
    pub expression: String,
    pub enabled: bool,
    pub num: usize,
    pub definitions: Vec<PartitionDefinition>,
}
/// 视图定义桩：SELECT 文本、定义者与输出列。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewSecurity {
    Definer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewInfo {
    pub select_statement: String,
    pub security: ViewSecurity,
    pub definer: String,
    pub columns: Vec<String>,
}
/// 表元信息桩：列、索引、是否以主键为 handle，以及可选分区/视图。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableInfo {
    pub id: i64,
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub primary_key_is_handle: bool,
    pub state: SchemaState,
    pub partition: Option<PartitionInfo>,
    pub view: Option<ViewInfo>,
}

/// Minimal information-schema container used by the planner fixtures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InfoSchema {
    pub tables: Vec<TableInfo>,
}

/// Minimal plan context snapshot returned by a mock session context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanContext {
    pub current_database: String,
    pub info_schema: Option<InfoSchema>,
}

/// 构造默认 Public 状态的列；offset 暂按 id-1 推算。
fn column(id: i64, name: &str, field_type: FieldType) -> ColumnInfo {
    ColumnInfo {
        id,
        name: name.to_string(),
        field_type,
        offset: (id - 1).max(0) as usize,
        state: SchemaState::Public,
        primary_key: false,
        not_null: false,
        unsigned: false,
        no_default: false,
    }
}
/// 构造 Public 状态索引；`None` 长度表示整列索引（非前缀）。
fn index_with_state(
    id: i64,
    name: &str,
    columns: &[(&str, Option<usize>)],
    state: SchemaState,
    unique: bool,
    global: bool,
) -> IndexInfo {
    IndexInfo {
        id,
        name: name.to_string(),
        columns: columns
            .iter()
            .map(|(name, length)| ((*name).to_string(), *length))
            .collect(),
        column_offsets: columns
            .iter()
            .map(|(name, _)| match *name {
                "a" => 0,
                "b" => 1,
                "c" => 2,
                "d" => 3,
                "e" => 4,
                "c_str" => 5,
                "d_str" => 6,
                "e_str" => 7,
                "f" => 8,
                "g" => 9,
                "h" => 10,
                "i_date" => 11,
                "ptn" => 12,
                _ => 0,
            })
            .collect(),
        state,
        unique,
        global,
    }
}

fn index(id: i64, name: &str, columns: &[(&str, Option<usize>)], unique: bool) -> IndexInfo {
    index_with_state(id, name, columns, SchemaState::Public, unique, false)
}

/// 有符号列 mock 表 `t`：主键 a，含复合/前缀索引，仅用于计划相关测试。
pub fn mock_signed_table() -> TableInfo {
    let names = [
        "a", "b", "c", "d", "e", "c_str", "d_str", "e_str", "f", "g", "h", "i_date",
    ];
    // 按列名后缀推断类型：*_str → Varchar，*_date → Date，其余 → Long。
    let mut columns = names
        .iter()
        .enumerate()
        .map(|(offset, name)| {
            let mut value = column(
                offset as i64 + 1,
                name,
                if name.ends_with("_str") {
                    FieldType::Varchar
                } else if name.ends_with("_date") {
                    FieldType::Date
                } else {
                    FieldType::Long
                },
            );
            value.offset = offset;
            value
        })
        .collect::<Vec<_>>();
    columns[0].primary_key = true;
    columns[0].not_null = true;
    columns[1].not_null = true;
    columns[2].not_null = true;
    columns[3].not_null = true;
    columns[8].not_null = true;
    columns[9].not_null = true;
    columns[10].no_default = true;
    TableInfo {
        id: 1,
        name: "t".to_string(),
        columns,
        indexes: vec![
            index(1, "c_d_e", &[("c", None), ("d", None), ("e", None)], true),
            index_with_state(2, "x", &[("e", None)], SchemaState::WriteOnly, true, false),
            index(3, "f", &[("f", None)], true),
            index(4, "g", &[("g", None)], false),
            index(5, "f_g", &[("f", None), ("g", None)], true),
            index(
                6,
                "c_d_e_str",
                &[("c_str", None), ("d_str", None), ("e_str", None)],
                false,
            ),
            index(
                7,
                "e_d_c_str_prefix",
                &[("e_str", None), ("d_str", None), ("c_str", Some(10))],
                false,
            ),
        ],
        primary_key_is_handle: true,
        state: SchemaState::Public,
        partition: None,
        view: None,
    }
}

/// 无符号列 mock 表 `t2`：主键 a 为 unsigned，含索引 b / b_c。
pub fn mock_unsigned_table() -> TableInfo {
    let mut columns = vec![
        column(1, "a", FieldType::Long),
        column(2, "b", FieldType::Long),
        column(3, "c", FieldType::Long),
    ];
    columns[0].primary_key = true;
    columns[0].not_null = true;
    columns[0].unsigned = true;
    columns[1].not_null = true;
    columns[2].unsigned = true;
    TableInfo {
        id: 2,
        name: "t2".to_string(),
        columns,
        indexes: vec![
            index(0, "b", &[("b", None)], true),
            index(0, "b_c", &[("b", None), ("c", None)], false),
        ],
        primary_key_is_handle: true,
        state: SchemaState::Public,
        partition: None,
        view: None,
    }
}

/// 无主键 mock 表 `t3`：基于 unsigned 表裁剪列并清空索引。
pub fn mock_no_pk_table() -> TableInfo {
    let mut table = mock_unsigned_table();
    table.id = 3;
    table.name = "t3".to_string();
    table.columns = vec![
        column(2, "a", FieldType::Long),
        column(3, "b", FieldType::Long),
    ];
    table.columns[0].not_null = true;
    table.columns[1].unsigned = true;
    table.indexes.clear();
    table.primary_key_is_handle = true;
    table
}
/// 视图 mock `v`：定义者 root，SELECT 投影 t 的 b,c,d。
pub fn mock_view() -> TableInfo {
    TableInfo {
        id: 4,
        name: "v".to_string(),
        columns: ["b", "c", "d"]
            .iter()
            .enumerate()
            .map(|(offset, name)| column(offset as i64 + 1, name, FieldType::Unspecified))
            .collect(),
        indexes: Vec::new(),
        primary_key_is_handle: false,
        state: SchemaState::Public,
        partition: None,
        view: Some(ViewInfo {
            select_statement: "select b,c,d from t".to_string(),
            security: ViewSecurity::Definer,
            definer: "root@".to_string(),
            columns: vec!["b".into(), "c".into(), "d".into()],
        }),
    }
}

/// 在基表上追加分区列 `ptn` 并挂上 PartitionInfo。
fn partitioned(
    mut table: TableInfo,
    id: i64,
    name: &str,
    partition_type: PartitionType,
    definitions: Vec<PartitionDefinition>,
) -> TableInfo {
    table.id = id;
    table.name = name.to_string();
    let mut ptn = column(
        table.columns.last().map_or(1, |column| column.id + 1),
        "ptn",
        FieldType::Long,
    );
    ptn.offset = table.columns.len();
    table.columns.push(ptn);
    table.partition = Some(PartitionInfo {
        partition_type,
        expression: "ptn".to_string(),
        enabled: true,
        num: match partition_type {
            PartitionType::Hash | PartitionType::List => definitions.len(),
            PartitionType::Range => 0,
        },
        definitions,
    });
    table
}
/// 由 (id, name) 列表生成空边界的分区定义骨架。
fn definitions(ids: &[(i64, &str)]) -> Vec<PartitionDefinition> {
    ids.iter()
        .map(|(id, name)| PartitionDefinition {
            id: *id,
            name: (*name).to_string(),
            less_than: Vec::new(),
            in_values: Vec::new(),
        })
        .collect()
}
/// Range 分区表 `pt1`：p1 < 16，p2 < 32。
pub fn mock_range_partition_table() -> TableInfo {
    let mut defs = definitions(&[(41, "p1"), (42, "p2")]);
    defs[0].less_than.push("16".into());
    defs[1].less_than.push("32".into());
    partitioned(mock_signed_table(), 5, "pt1", PartitionType::Range, defs)
}
/// Hash 分区表 `pt2`：两分区 p1/p2。
pub fn mock_hash_partition_table() -> TableInfo {
    partitioned(
        mock_signed_table(),
        6,
        "pt2",
        PartitionType::Hash,
        definitions(&[(51, "p1"), (52, "p2")]),
    )
}
/// List 分区表 `pt3`：p1 IN (1)，p2 IN (2)。
pub fn mock_list_partition_table() -> TableInfo {
    let mut defs = definitions(&[(61, "p1"), (62, "p2")]);
    defs[0].in_values.push(vec!["1".into()]);
    defs[1].in_values.push(vec!["2".into()]);
    partitioned(mock_signed_table(), 7, "pt3", PartitionType::List, defs)
}
/// 带全局索引的 Hash 分区表：同时含普通索引与 `*_global` 唯一全局索引。
pub fn mock_global_index_hash_partition_table() -> TableInfo {
    let mut table = partitioned(
        mock_signed_table(),
        1,
        "pt2_global_index",
        PartitionType::Hash,
        definitions(&[(51, "p1"), (52, "p2")]),
    );
    for (id, name, columns, unique, global) in [
        (0, "b", vec![("b", None)], false, false),
        (0, "b_global", vec![("b", None)], true, true),
        (0, "b_c", vec![("b", None), ("c", None)], false, false),
        (0, "b_c_global", vec![("b", None), ("c", None)], true, true),
    ] {
        let mut value = index(id, name, &columns, unique);
        value.global = global;
        table.indexes.push(value);
    }
    table
}
/// 含 SchemaState::None 列的表，用于测试 DDL 中间态列不可见场景。
pub fn mock_state_none_column_table() -> TableInfo {
    let mut table = TableInfo {
        id: 8,
        name: "T_StateNoneColumn".to_string(),
        columns: vec![
            column(1, "a", FieldType::Long),
            column(2, "b", FieldType::Long),
            column(3, "c", FieldType::Long),
        ],
        indexes: vec![index(0, "b", &[("b", None)], true)],
        primary_key_is_handle: true,
        state: SchemaState::Public,
        partition: None,
        view: None,
    };
    table.columns[0].primary_key = true;
    table.columns[0].not_null = true;
    table.columns[0].unsigned = true;
    table.columns[1].not_null = true;
    table.columns[2].unsigned = true;
    table.columns[2].state = SchemaState::None;
    table
}

/// 规划测试用会话上下文桩：当前库与除法精度增量。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockContext {
    pub current_database: String,
    pub division_precision_increment: u8,
    pub store_initialized: bool,
    pub domain_bound: bool,
    pub stats_handle_created: bool,
    pub info_schema: Option<InfoSchema>,
    pub window_functions_enabled: bool,
}
/// 构造默认 mock 上下文：库名 `test`，除法精度增量 4。
pub fn mock_context() -> MockContext {
    MockContext {
        current_database: "test".to_string(),
        division_precision_increment: 4,
        store_initialized: true,
        domain_bound: true,
        stats_handle_created: true,
        info_schema: None,
        window_functions_enabled: false,
    }
}

impl MockContext {
    pub fn get_plan_context(&self) -> PlanContext {
        PlanContext {
            current_database: self.current_database.clone(),
            info_schema: self.info_schema.clone(),
        }
    }
}

/// MockInfoSchema 对应 Go 测试包中的分区信息 schema 构造器。
pub fn mock_partition_info_schema(definitions: Vec<PartitionDefinition>) -> InfoSchema {
    let mut table = mock_signed_table();
    let last = table.columns.last().expect("signed table has columns");
    table.columns.push(ColumnInfo {
        id: last.id + 1,
        name: "ptn".to_string(),
        field_type: FieldType::Long,
        offset: last.offset + 1,
        state: SchemaState::Public,
        primary_key: false,
        not_null: false,
        unsigned: false,
        no_default: false,
    });
    table.partition = Some(PartitionInfo {
        partition_type: PartitionType::Range,
        expression: "ptn".to_string(),
        enabled: true,
        num: 0,
        definitions,
    });
    InfoSchema {
        tables: vec![table],
    }
}
/*

// Go 语义：MockUnsignedTable is only used for plan related tests.
pub fn MockUnsignedTable() -> model::TableInfo {
    // column: a, b, c
    // PK: a
    // indeices: b, b_c
    indices := []*model.IndexInfo{
        {
            Name: ast.NewCIStr("b"),
            Columns: []*model.IndexColumn{
                {
                    Name:   ast.NewCIStr("b"),
                    Length: types.UnspecifiedLength,
                    Offset: 1,
                },
            },
            State:  model.StatePublic,
            Unique: true,
        },
        {
            Name: ast.NewCIStr("b_c"),
            Columns: []*model.IndexColumn{
                {
                    Name:   ast.NewCIStr("b"),
                    Length: types.UnspecifiedLength,
                    Offset: 1,
                },
                {
                    Name:   ast.NewCIStr("c"),
                    Length: types.UnspecifiedLength,
                    Offset: 2,
                },
            },
            State: model.StatePublic,
        },
    }
    pkColumn := &model.ColumnInfo{
        State:     model.StatePublic,
        Offset:    0,
        Name:      ast.NewCIStr("a"),
        FieldType: newLongType(),
        ID:        1,
    }
    col0 := &model.ColumnInfo{
        State:     model.StatePublic,
        Offset:    1,
        Name:      ast.NewCIStr("b"),
        FieldType: newLongType(),
        ID:        2,
    }
    col1 := &model.ColumnInfo{
        State:     model.StatePublic,
        Offset:    2,
        Name:      ast.NewCIStr("c"),
        FieldType: newLongType(),
        ID:        3,
    }
    pkColumn.SetFlag(mysql.PriKeyFlag | mysql.NotNullFlag | mysql.UnsignedFlag)
    // Go 语义：Column 'b' is not null.
    col0.SetFlag(mysql.NotNullFlag)
    col1.SetFlag(mysql.UnsignedFlag)
    table := &model.TableInfo{
        ID:         2,
        Columns:    []*model.ColumnInfo{pkColumn, col0, col1},
        Indices:    indices,
        Name:       ast.NewCIStr("t2"),
        PKIsHandle: true,
        State:      model.StatePublic,
    }
    return table
}
*/

// Go 语义：MockNoPKTable is only used for plan related tests.
/*
pub fn MockNoPKTable() -> model::TableInfo {
    // column: a, b
    col0 := &model.ColumnInfo{
        State:     model.StatePublic,
        Offset:    1,
        Name:      ast.NewCIStr("a"),
        FieldType: newLongType(),
        ID:        2,
    }
    col1 := &model.ColumnInfo{
        State:     model.StatePublic,
        Offset:    2,
        Name:      ast.NewCIStr("b"),
        FieldType: newLongType(),
        ID:        3,
    }
    // Go 语义：Column 'a', 'b' is not null.
    col0.SetFlag(mysql.NotNullFlag)
    col1.SetFlag(mysql.UnsignedFlag)
    table := &model.TableInfo{
        ID:         3,
        Columns:    []*model.ColumnInfo{col0, col1},
        Name:       ast.NewCIStr("t3"),
        PKIsHandle: true,
        State:      model.StatePublic,
    }
    return table
}
*/

// Go 语义：MockView is only used for plan related tests.
/*
pub fn MockView() -> model::TableInfo {
    selectStmt := "select b,c,d from t"
    col0 := &model.ColumnInfo{
        State:  model.StatePublic,
        Offset: 0,
        Name:   ast.NewCIStr("b"),
        ID:     1,
    }
    col1 := &model.ColumnInfo{
        State:  model.StatePublic,
        Offset: 1,
        Name:   ast.NewCIStr("c"),
        ID:     2,
    }
    col2 := &model.ColumnInfo{
        State:  model.StatePublic,
        Offset: 2,
        Name:   ast.NewCIStr("d"),
        ID:     3,
    }
    view := &model.ViewInfo{SelectStmt: selectStmt, Security: ast.SecurityDefiner, Definer: &auth.UserIdentity{Username: "root", Hostname: ""}, Cols: []ast.CIStr{col0.Name, col1.Name, col2.Name}}
    table := &model.TableInfo{
        ID:      4,
        Name:    ast.NewCIStr("v"),
        Columns: []*model.ColumnInfo{col0, col1, col2},
        View:    view,
        State:   model.StatePublic,
    }
    return table
}

// Go 语义：MockContext is only used for plan related tests.
pub fn MockContext() -> mock::Context {
    ctx := mock.NewContext()
    ctx.Store = &mock.Store{
        Client: &mock.Client{},
    }
    ctx.GetSessionVars().SetCurrentDB("test")
    ctx.GetSessionVars().DivPrecisionIncrement = vardef.DefDivPrecisionIncrement
    do := domain.NewMockDomain()
    if err := do.CreateStatsHandle(context.Background()); err != nil {
        panic(fmt.Sprintf("create mock context panic: %+v", err))
    }
    ctx.BindDomainAndSchValidator(do, nil)
    return ctx
}

// Go 语义：MockPartitionInfoSchema mocks an info schema for partition table.
pub fn MockPartitionInfoSchema(definitions: Vec<model::PartitionDefinition>) -> infoschema::InfoSchema {
    tableInfo := MockSignedTable()
    cols := make([]*model.ColumnInfo, 0, len(tableInfo.Columns))
    cols = append(cols, tableInfo.Columns...)
    last := tableInfo.Columns[len(tableInfo.Columns)-1]
    cols = append(cols, &model.ColumnInfo{
        State:     model.StatePublic,
        Offset:    last.Offset + 1,
        Name:      ast.NewCIStr("ptn"),
        FieldType: newLongType(),
        ID:        last.ID + 1,
    })
    partition := &model.PartitionInfo{
        Type:        ast.PartitionTypeRange,
        Expr:        "ptn",
        Enable:      true,
        Definitions: definitions,
    }
    tableInfo.Columns = cols
    tableInfo.Partition = partition
    is := infoschema.MockInfoSchema([]*model.TableInfo{tableInfo})
    return is
}

// Go 语义：MockRangePartitionTable mocks a range partition table for test
pub fn MockRangePartitionTable() -> model::TableInfo {
    definitions := []model.PartitionDefinition{
        {
            ID:       41,
            Name:     ast.NewCIStr("p1"),
            LessThan: []string{"16"},
        },
        {
            ID:       42,
            Name:     ast.NewCIStr("p2"),
            LessThan: []string{"32"},
        },
    }
    tableInfo := MockSignedTable()
    tableInfo.ID = 5
    tableInfo.Name = ast.NewCIStr("pt1")
    cols := make([]*model.ColumnInfo, 0, len(tableInfo.Columns))
    cols = append(cols, tableInfo.Columns...)
    last := tableInfo.Columns[len(tableInfo.Columns)-1]
    cols = append(cols, &model.ColumnInfo{
        State:     model.StatePublic,
        Offset:    last.Offset + 1,
        Name:      ast.NewCIStr("ptn"),
        FieldType: newLongType(),
        ID:        last.ID + 1,
    })
    partition := &model.PartitionInfo{
        Type:        ast.PartitionTypeRange,
        Expr:        "ptn",
        Enable:      true,
        Definitions: definitions,
    }
    tableInfo.Columns = cols
    tableInfo.Partition = partition
    return tableInfo
}

// Go 语义：MockHashPartitionTable mocks a hash partition table for test
pub fn MockHashPartitionTable() -> model::TableInfo {
    definitions := []model.PartitionDefinition{
        {
            ID:   51,
            Name: ast.NewCIStr("p1"),
        },
        {
            ID:   52,
            Name: ast.NewCIStr("p2"),
        },
    }
    tableInfo := MockSignedTable()
    tableInfo.ID = 6
    tableInfo.Name = ast.NewCIStr("pt2")
    cols := make([]*model.ColumnInfo, 0, len(tableInfo.Columns))
    cols = append(cols, tableInfo.Columns...)
    last := tableInfo.Columns[len(tableInfo.Columns)-1]
    cols = append(cols, &model.ColumnInfo{
        State:     model.StatePublic,
        Offset:    last.Offset + 1,
        Name:      ast.NewCIStr("ptn"),
        FieldType: newLongType(),
        ID:        last.ID + 1,
    })
    partition := &model.PartitionInfo{
        Type:        ast.PartitionTypeHash,
        Expr:        "ptn",
        Enable:      true,
        Definitions: definitions,
        Num:         2,
    }
    tableInfo.Columns = cols
    tableInfo.Partition = partition
    return tableInfo
}

// Go 语义：MockListPartitionTable mocks a list partition table for test
pub fn MockListPartitionTable() -> model::TableInfo {
    definitions := []model.PartitionDefinition{
        {
            ID:   61,
            Name: ast.NewCIStr("p1"),
            InValues: [][]string{
                {
                    "1",
                },
            },
        },
        {
            ID:   62,
            Name: ast.NewCIStr("p2"),
            InValues: [][]string{
                {
                    "2",
                },
            },
        },
    }
    tableInfo := MockSignedTable()
    tableInfo.ID = 7
    tableInfo.Name = ast.NewCIStr("pt3")
    cols := make([]*model.ColumnInfo, 0, len(tableInfo.Columns))
    cols = append(cols, tableInfo.Columns...)
    last := tableInfo.Columns[len(tableInfo.Columns)-1]
    cols = append(cols, &model.ColumnInfo{
        State:     model.StatePublic,
        Offset:    last.Offset + 1,
        Name:      ast.NewCIStr("ptn"),
        FieldType: newLongType(),
        ID:        last.ID + 1,
    })
    partition := &model.PartitionInfo{
        Type:        ast.PartitionTypeList,
        Expr:        "ptn",
        Enable:      true,
        Definitions: definitions,
        Num:         2,
    }
    tableInfo.Columns = cols
    tableInfo.Partition = partition
    return tableInfo
}

// Go 语义：MockGlobalIndexHashPartitionTable mocks a hash partition table with global index for test
pub fn MockGlobalIndexHashPartitionTable() -> model::TableInfo {
    definitions := []model.PartitionDefinition{
        {
            ID:   51,
            Name: ast.NewCIStr("p1"),
        },
        {
            ID:   52,
            Name: ast.NewCIStr("p2"),
        },
    }
    tableInfo := MockSignedTable()
    tableInfo.Name = ast.NewCIStr("pt2_global_index")
    cols := make([]*model.ColumnInfo, 0, len(tableInfo.Columns))
    cols = append(cols, tableInfo.Columns...)
    last := tableInfo.Columns[len(tableInfo.Columns)-1]
    cols = append(cols, &model.ColumnInfo{
        State:     model.StatePublic,
        Offset:    last.Offset + 1,
        Name:      ast.NewCIStr("ptn"),
        FieldType: newLongType(),
        ID:        last.ID + 1,
    })
    partition := &model.PartitionInfo{
        Type:        ast.PartitionTypeHash,
        Expr:        "ptn",
        Enable:      true,
        Definitions: definitions,
        Num:         2,
    }
    tableInfo.Columns = cols
    tableInfo.Partition = partition
    // add a global index `b_global` and `b_c_global` and normal index `b` and `b_c`
    tableInfo.Indices = append(tableInfo.Indices, []*model.IndexInfo{
        {
            Name: ast.NewCIStr("b"),
            Columns: []*model.IndexColumn{
                {
                    Name:   ast.NewCIStr("b"),
                    Length: types.UnspecifiedLength,
                    Offset: 1,
                },
            },
            State: model.StatePublic,
        },
        {
            Name: ast.NewCIStr("b_global"),
            Columns: []*model.IndexColumn{
                {
                    Name:   ast.NewCIStr("b"),
                    Length: types.UnspecifiedLength,
                    Offset: 1,
                },
            },
            State:  model.StatePublic,
            Unique: true,
            Global: true,
        },
        {
            Name: ast.NewCIStr("b_c"),
            Columns: []*model.IndexColumn{
                {
                    Name:   ast.NewCIStr("b"),
                    Length: types.UnspecifiedLength,
                    Offset: 1,
                },
                {
                    Name:   ast.NewCIStr("c"),
                    Length: types.UnspecifiedLength,
                    Offset: 2,
                },
            },
            State: model.StatePublic,
        },
        {
            Name: ast.NewCIStr("b_c_global"),
            Columns: []*model.IndexColumn{
                {
                    Name:   ast.NewCIStr("b"),
                    Length: types.UnspecifiedLength,
                    Offset: 1,
                },
                {
                    Name:   ast.NewCIStr("c"),
                    Length: types.UnspecifiedLength,
                    Offset: 2,
                },
            },
            State:  model.StatePublic,
            Unique: true,
            Global: true,
        },
    }...)
    return tableInfo
}

// Go 语义：MockStateNoneColumnTable is only used for plan related tests.
pub fn MockStateNoneColumnTable() -> model::TableInfo {
    // column: a, b
    // PK: a
    // indeices: b
    indices := []*model.IndexInfo{
        {
            Name: ast.NewCIStr("b"),
            Columns: []*model.IndexColumn{
                {
                    Name:   ast.NewCIStr("b"),
                    Length: types.UnspecifiedLength,
                    Offset: 1,
                },
            },
            State:  model.StatePublic,
            Unique: true,
        },
    }
    pkColumn := &model.ColumnInfo{
        State:     model.StatePublic,
        Offset:    0,
        Name:      ast.NewCIStr("a"),
        FieldType: newLongType(),
        ID:        1,
    }
    col0 := &model.ColumnInfo{
        State:     model.StatePublic,
        Offset:    1,
        Name:      ast.NewCIStr("b"),
        FieldType: newLongType(),
        ID:        2,
    }
    col1 := &model.ColumnInfo{
        State:     model.StateNone,
        Offset:    2,
        Name:      ast.NewCIStr("c"),
        FieldType: newLongType(),
        ID:        3,
    }
    pkColumn.SetFlag(mysql.PriKeyFlag | mysql.NotNullFlag | mysql.UnsignedFlag)
    col0.SetFlag(mysql.NotNullFlag)
    col1.SetFlag(mysql.UnsignedFlag)
    table := &model.TableInfo{
        ID:         8,
        Columns:    []*model.ColumnInfo{pkColumn, col0, col1},
        Indices:    indices,
        Name:       ast.NewCIStr("T_StateNoneColumn"),
        PKIsHandle: true,
        State:      model.StatePublic,
    }
    return table
}
*/
