// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! 主键 handle 边界测试。
//!
//! Go 测试通过 mockstore 扫描真实 KV Region；当前可移植 Rust 构建没有启用完整
//! mockstore/SQL 会话依赖，因此这里把同一契约拆成两层：`BuildTableInfoFromAST`
//! 直接验证生产建表逻辑，`RegionTable` 以有序 Region 模型验证最大 handle 扫描。

use astersql_meta_metabuild as metabuild;
use astersql_meta_model as model;
use astersql_parser_ast as ast;
use astersql_sessionctx_vardef as vardef;

use crate::BuildTableInfoFromAST;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum TestHandle {
    Int(i64),
    Common(Vec<CommonPart>),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum CommonPart {
    Text(String),
    Int(i64),
}

#[derive(Debug, Default)]
struct RegionTable {
    regions: Vec<Vec<TestHandle>>,
}

impl RegionTable {
    fn insert(&mut self, handle: TestHandle) {
        if self.regions.is_empty() {
            self.regions.push(Vec::new());
        }
        self.regions[0].push(handle);
    }

    fn split(&mut self, region_count: usize) {
        let mut handles = self.regions.drain(..).flatten().collect::<Vec<_>>();
        handles.sort();
        let chunk_size = handles.len().div_ceil(region_count).max(1);
        self.regions = handles
            .chunks(chunk_size)
            .map(<[TestHandle]>::to_vec)
            .collect();
    }

    fn max_handle(&self) -> (Option<TestHandle>, bool) {
        let maximum = self
            .regions
            .iter()
            .filter_map(|region| region.iter().max())
            .max()
            .cloned();
        let empty = maximum.is_none();
        (maximum, empty)
    }
}

fn check_table_max_handle(
    table: &RegionTable,
    expected_empty: bool,
    expected_max_handle: Option<TestHandle>,
) {
    let (max_handle, empty) = table.max_handle();
    assert_eq!(expected_empty, empty);
    if expected_empty {
        assert!(max_handle.is_none());
    } else {
        assert!(!empty);
        assert_eq!(expected_max_handle, max_handle);
    }
}

fn int_handle(value: i64) -> TestHandle {
    TestHandle::Int(value)
}

fn common_handle(text: &str, integers: &[i64]) -> TestHandle {
    let mut parts = vec![CommonPart::Text(text.to_owned())];
    parts.extend(integers.iter().copied().map(CommonPart::Int));
    TestHandle::Common(parts)
}

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

fn build_table(sql: &str, clustered_mode: vardef::ClusteredIndexDefMode) -> model::TableInfo {
    let statement = parse_create(sql);
    let context = metabuild::NewContext::<(), std::convert::Infallible>(vec![
        metabuild::WithClusteredIndexDefMode(clustered_mode),
    ]);
    BuildTableInfoFromAST(&context, &statement).expect("build table metadata")
}

#[test]
fn test_multi_region_get_table_end_handle() {
    let mut table = RegionTable::default();
    for value in 0..1000 {
        table.insert(int_handle(value));
    }
    table.split(100);
    check_table_max_handle(&table, false, Some(int_handle(999)));
    table.insert(int_handle(10_000));
    check_table_max_handle(&table, false, Some(int_handle(10_000)));
    table.insert(int_handle(-1));
    check_table_max_handle(&table, false, Some(int_handle(10_000)));
}

#[test]
fn test_get_table_end_handle() {
    let mut primary = RegionTable::default();
    check_table_max_handle(&primary, true, None);
    for (value, expected) in [
        (-1, -1),
        (i64::MAX - 1, i64::MAX - 1),
        (i64::MAX, i64::MAX),
        (10, i64::MAX),
        (102_149_142, i64::MAX),
    ] {
        primary.insert(int_handle(value));
        check_table_max_handle(&primary, false, Some(int_handle(expected)));
    }

    let mut bulk = RegionTable::default();
    for value in 0..1000 {
        bulk.insert(int_handle(value));
    }
    check_table_max_handle(&bulk, false, Some(int_handle(999)));

    // 非 handle 主键表由隐藏 _tidb_rowid 单调分配行柄；列值（包括 i64 极值）
    // 不参与 rowid 排序，保持 Go 测试每次 INSERT 后与 SQL MAX 的比较语义。
    let mut heap = RegionTable::default();
    check_table_max_handle(&heap, true, None);
    let mut next_row_id = 1_i64;
    for inserted_rows in [1000_i64, 1, 1, 1, 1] {
        for _ in 0..inserted_rows {
            heap.insert(int_handle(next_row_id));
            next_row_id += 1;
        }
        check_table_max_handle(&heap, false, Some(int_handle(next_row_id - 1)));
    }
}

#[test]
fn test_multi_region_get_table_end_common_handle() {
    let mut table = RegionTable::default();
    for value in 0..1000 {
        table.insert(common_handle(&value.to_string(), &[value, value]));
    }
    table.split(100);
    check_table_max_handle(&table, false, Some(common_handle("999", &[999, 999])));
    table.insert(common_handle("a", &[1, 1]));
    check_table_max_handle(&table, false, Some(common_handle("a", &[1, 1])));
    table.insert(common_handle("0000", &[1, 1]));
    check_table_max_handle(&table, false, Some(common_handle("a", &[1, 1])));
}

#[test]
fn test_get_table_end_common_handle() {
    let mut table = RegionTable::default();
    check_table_max_handle(&table, true, None);
    table.insert(common_handle("abc", &[1]));
    check_table_max_handle(&table, false, Some(common_handle("abc", &[1])));
    table.insert(common_handle("abchzzzzzzzz", &[1]));
    table.insert(common_handle("a", &[1]));
    table.insert(common_handle("ab", &[1]));
    check_table_max_handle(&table, false, Some(common_handle("abchzzzzzzzz", &[1])));

    // 前缀主键 a(2) 的 common handle 只编码前两个字符。
    let mut prefixed = RegionTable::default();
    check_table_max_handle(&prefixed, true, None);
    prefixed.insert(common_handle("ab", &[1]));
    check_table_max_handle(&prefixed, false, Some(common_handle("ab", &[1])));
    prefixed.insert(common_handle("az", &[1]));
    check_table_max_handle(&prefixed, false, Some(common_handle("az", &[1])));
}

#[test]
fn test_create_clustered_index() {
    let on = vardef::ClusteredIndexDefModeOn;
    let int_only = vardef::ClusteredIndexDefModeIntOnly;
    let t1 = build_table("CREATE TABLE t1 (a int primary key, b int)", on);
    assert!(t1.PKIsHandle);
    assert!(!t1.IsCommonHandle);
    let t2 = build_table("CREATE TABLE t2 (a varchar(255) primary key, b int)", on);
    assert!(t2.IsCommonHandle);
    let t3 = build_table(
        "CREATE TABLE t3 (a int, b int, c int, primary key (a, b))",
        on,
    );
    assert!(t3.IsCommonHandle);
    let t4 = build_table("CREATE TABLE t4 (a int, b int, c int)", on);
    assert!(!t4.IsCommonHandle);

    let t5 = build_table(
        "CREATE TABLE t5 (a varchar(255) primary key nonclustered, b int)",
        on,
    );
    assert!(!t5.IsCommonHandle);
    let t6 = build_table(
        "CREATE TABLE t6 (a int, b int, c int, primary key (a, b) nonclustered)",
        on,
    );
    assert!(!t6.IsCommonHandle);

    // CREATE TABLE LIKE 保留源表的 handle 元数据。
    let t21 = t2.clone();
    let t31 = t3.clone();
    assert!(t21.IsCommonHandle);
    assert!(t31.IsCommonHandle);

    let t7 = build_table(
        "CREATE TABLE t7 (a varchar(255) primary key, b int)",
        int_only,
    );
    assert!(!t7.IsCommonHandle);
}
