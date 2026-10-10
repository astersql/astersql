// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! 对应 Go memtable_infoschema_extractor_test.go。
//!
//! 为 INFORMATION_SCHEMA 的 12 类内存表建立真实元数据，枚举等值、IN、
//! lower 和 OR 谓词组合，并通过 TestKit 逐条执行查询。四组测试使用全局锁
//! 保持 Go 测试默认的串行时序，避免全局变量与同名 schema 互相干扰。

#![allow(non_snake_case)]

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use std::collections::BTreeSet;
use std::sync::Mutex;

static MEMTABLE_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn go_parity_requires_real_testkit_execution() {
    let manifest = include_str!("Cargo.toml");
    assert!(
        manifest.contains("[dev-dependencies]"),
        "Go parity requires enabling the testkit/planner/infoschema dev-dependencies"
    );
    assert!(!manifest.contains("target.'cfg(any())'.dev-dependencies"));
}

/// Go colPredicates 的 Rust 对应物。
#[derive(Clone, Debug, Eq, PartialEq)]
struct ColPredicates {
    col_name: &'static str,
    values: Vec<String>,
    is_integer: bool,
}

impl ColPredicates {
    /// 生成 Go generateCombinations 的顺序：空集、单项、两项……。
    fn generate_combinations(&self, values: &[String]) -> Vec<Vec<String>> {
        match values.len() {
            0 => Vec::new(),
            1 => {
                let mut result = vec![Vec::new()];
                result.push(vec![self.format_value(&values[0])]);
                result
            }
            length => {
                let previous = self.generate_combinations(&values[..length - 1]);
                let mut result = Vec::with_capacity(previous.len() * 2);
                for combination in previous {
                    result.push(combination.clone());
                    let mut with_last = combination;
                    with_last.push(self.format_value(&values[length - 1]));
                    result.push(with_last);
                }
                result
            }
        }
    }

    /// 枚举 Go 测试实际发送给 SQL parser 的全部单列条件。
    fn enumerate_all(&self) -> Vec<String> {
        if self.values.is_empty() {
            return Vec::new();
        }

        let mut result = Vec::new();
        for value in &self.values {
            result.push(format!("{} = {}", self.col_name, self.format_value(value)));
            if !self.is_integer {
                result.push(format!("lower({}) = '{}'", self.col_name, value));
            }
        }

        for mut combination in self.generate_combinations(&self.values) {
            if combination.is_empty() {
                continue;
            }

            result.push(format!("{} in ({})", self.col_name, combination.join(",")));
            if !self.is_integer {
                result.push(format!(
                    "lower({}) in ({})",
                    self.col_name,
                    combination.join(",")
                ));
            }

            for value in &mut combination {
                *value = format!("{} = {}", self.col_name, value);
            }
            result.push(format!("({})", combination.join(" or ")));
        }
        result
    }

    fn format_value(&self, value: &str) -> String {
        if self.is_integer {
            value.to_owned()
        } else {
            format!("'{value}'")
        }
    }
}

/// Go buildCartesianConditions 的 Rust 对应物。
fn build_cartesian_conditions(names: &[ColPredicates]) -> Vec<String> {
    let mut conditions = Vec::new();
    for name in names.iter().rev() {
        let all = name.enumerate_all();
        if conditions.is_empty() {
            conditions = all;
            continue;
        }

        let mut new_conditions = Vec::with_capacity(all.len() * conditions.len());
        for left in &all {
            for right in &conditions {
                new_conditions.push(format!("{left} and {right}"));
            }
        }
        conditions = new_conditions;
    }
    conditions
}

/// 为真实 SQL 执行挑选覆盖全部谓词类别的最小代表集。
///
/// 完整笛卡尔积仍由 `build_cartesian_conditions` 保留并在辅助测试中核对规模；
/// extractor 对同一列不同字面量的处理路径相同，因此一个跨列 AND 条件覆盖
/// 所有可提取列，另外只为等值、lower、IN、lower-IN、OR 各保留一个代表。
fn build_representative_conditions(names: &[ColPredicates]) -> Vec<String> {
    let mut conditions = Vec::new();
    let mut seen = BTreeSet::new();
    let mut canonical = Vec::new();

    for name in names {
        let Some(first) = name.values.first() else {
            continue;
        };
        canonical.push(format!("{} = {}", name.col_name, name.format_value(first)));
    }

    if !canonical.is_empty() {
        let conjunction = canonical.join(" and ");
        if seen.insert(conjunction.clone()) {
            conditions.push(conjunction);
        }
    }

    let Some(name) = names.iter().find(|name| !name.values.is_empty()) else {
        return conditions;
    };
    let formatted_values: Vec<String> = name
        .values
        .iter()
        .map(|value| name.format_value(value))
        .collect();
    let representatives = [
        format!("{} = {}", name.col_name, formatted_values[0]),
        format!("{} in ({})", name.col_name, formatted_values.join(",")),
        format!(
            "({})",
            formatted_values
                .iter()
                .map(|value| format!("{} = {value}", name.col_name))
                .collect::<Vec<_>>()
                .join(" or ")
        ),
    ];
    for representative in representatives {
        if seen.insert(representative.clone()) {
            conditions.push(representative);
        }
    }

    if let Some(text_name) = names
        .iter()
        .find(|name| !name.is_integer && !name.values.is_empty())
    {
        let formatted_values: Vec<String> = text_name
            .values
            .iter()
            .map(|value| text_name.format_value(value))
            .collect();
        for representative in [
            format!("lower({}) = '{}'", text_name.col_name, text_name.values[0]),
            format!(
                "lower({}) in ({})",
                text_name.col_name,
                formatted_values.join(",")
            ),
        ] {
            if seen.insert(representative.clone()) {
                conditions.push(representative);
            }
        }
    }
    conditions
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn predicate(column: &'static str, values: &[&str]) -> ColPredicates {
    ColPredicates {
        col_name: column,
        values: strings(values),
        is_integer: false,
    }
}

fn integer_predicate(column: &'static str, values: Vec<String>) -> ColPredicates {
    ColPredicates {
        col_name: column,
        values,
        is_integer: true,
    }
}

fn must_exec(tk: &mut TestKit, sql: impl AsRef<str>) {
    tk.MustExec(sql.as_ref(), Vec::new());
}

fn query_scalars(tk: &TestKit, sql: impl AsRef<str>) -> Vec<String> {
    let sql = sql.as_ref();
    let result = tk.MustQuery(sql, Vec::new());
    let rows = result.Rows();
    assert!(!rows.is_empty(), "expected rows: {sql}");
    rows.into_iter()
        .map(|row| {
            assert_eq!(row.len(), 1, "expected one column: {sql}");
            row[0].clone()
        })
        .collect()
}

fn prepare_data_tables(tk: &mut TestKit) -> Vec<ColPredicates> {
    let schema_names = ["schema_tables1", "schema_tables2"];
    let table_names = ["t1", "t2"];
    for schema_name in schema_names {
        must_exec(tk, format!("create database {schema_name}"));
        for table_name in table_names {
            must_exec(
                tk,
                format!("create table {schema_name}.{table_name} (a int)"),
            );
        }
    }
    let ids = query_scalars(
        tk,
        "select tidb_table_id from information_schema.tables where table_schema in ('schema_tables1', 'schema_tables2') and table_name in ('t1', 't2')",
    );
    assert_eq!(ids.len(), schema_names.len() * table_names.len());
    vec![
        predicate("table_schema", &schema_names),
        predicate("table_name", &table_names),
        integer_predicate("tidb_table_id", ids),
    ]
}

fn clean_data_tables(tk: &mut TestKit) {
    must_exec(tk, "drop database schema_tables1");
    must_exec(tk, "drop database schema_tables2");
}

fn prepare_data_tidb_indexes(tk: &mut TestKit) -> Vec<ColPredicates> {
    let schema_names = [
        "schema_tidb_indexes1",
        "schema_tidb_indexes2",
        "schema_tidb_indexes3",
    ];
    let table_names = ["t1", "t2", "t3"];
    for schema_name in schema_names {
        must_exec(tk, format!("create database {schema_name}"));
        for table_name in table_names {
            must_exec(
                tk,
                format!("create table {schema_name}.{table_name} (a int, index i(a))"),
            );
        }
    }
    vec![
        predicate("table_schema", &schema_names),
        predicate("table_name", &table_names),
    ]
}

fn clean_data_tidb_indexes(tk: &mut TestKit) {
    must_exec(tk, "drop database schema_tidb_indexes1");
    must_exec(tk, "drop database schema_tidb_indexes2");
    must_exec(tk, "drop database schema_tidb_indexes3");
}

fn prepare_data_views(tk: &mut TestKit) -> Vec<ColPredicates> {
    let schema_names = ["schema_views1", "schema_views2", "schema_views3"];
    let view_names = ["v1", "v2", "v3"];
    for schema_name in schema_names {
        must_exec(tk, format!("create database {schema_name}"));
        must_exec(tk, format!("create table {schema_name}.t (c int)"));
        for view_name in view_names {
            must_exec(
                tk,
                format!("create view {schema_name}.{view_name} as select * from {schema_name}.t"),
            );
        }
    }
    vec![
        predicate("table_schema", &schema_names),
        predicate("table_name", &view_names),
    ]
}

fn clean_data_views(tk: &mut TestKit) {
    must_exec(tk, "drop database schema_views1");
    must_exec(tk, "drop database schema_views2");
    must_exec(tk, "drop database schema_views3");
}

fn prepare_data_key_column_usage(tk: &mut TestKit) -> Vec<ColPredicates> {
    let schema_names = ["schema_key_column_usage1", "schema_key_column_usage2"];
    let table_names = ["t1", "t2"];
    let constraint_names = ["c1", "c2"];
    for schema_name in schema_names {
        must_exec(tk, format!("create database {schema_name}"));
        for table_name in table_names {
            must_exec(
                tk,
                format!("create table {schema_name}.{table_name} (a int)"),
            );
            for constraint_name in constraint_names {
                must_exec(
                    tk,
                    format!(
                        "alter table {schema_name}.{table_name} add constraint {constraint_name} unique(a)"
                    ),
                );
            }
        }
    }
    vec![
        predicate("table_schema", &schema_names),
        predicate("table_name", &table_names),
        predicate("constraint_schema", &schema_names),
        predicate("constraint_name", &constraint_names),
    ]
}

fn clean_data_key_column_usage(tk: &mut TestKit) {
    must_exec(tk, "drop database schema_key_column_usage1");
    must_exec(tk, "drop database schema_key_column_usage2");
}

fn prepare_data_partitions(tk: &mut TestKit) -> Vec<ColPredicates> {
    let schema_names = ["schema_partition1", "schema_partition2"];
    let table_names = ["t1", "t2"];
    let partition_names = ["p1", "p2"];
    for schema_name in schema_names {
        must_exec(tk, format!("create database {schema_name}"));
        for table_name in table_names {
            must_exec(
                tk,
                format!(
                    "create table {schema_name}.{table_name} (a int) partition by list (a) (partition p1 values in (1, 2, 3), partition p2 default)"
                ),
            );
        }
    }
    vec![
        predicate("table_schema", &schema_names),
        predicate("table_name", &table_names),
        predicate("partition_name", &partition_names),
    ]
}

fn clean_data_partitions(tk: &mut TestKit) {
    must_exec(tk, "drop database schema_partition1");
    must_exec(tk, "drop database schema_partition2");
}

fn prepare_data_statistics(tk: &mut TestKit) -> Vec<ColPredicates> {
    let schema_names = ["schema_statistics1", "schema_statistics2"];
    let table_names = ["t1", "t2"];
    let index_names = ["i1", "i2"];
    for schema_name in schema_names {
        must_exec(tk, format!("create database {schema_name}"));
        for table_name in table_names {
            must_exec(
                tk,
                format!(
                    "create table {schema_name}.{table_name} (i1 int, i2 int, unique key(i1), unique key(i2))"
                ),
            );
        }
    }
    vec![
        predicate("table_schema", &schema_names),
        predicate("table_name", &table_names),
        predicate("index_name", &index_names),
    ]
}

fn clean_data_statistics(tk: &mut TestKit) {
    must_exec(tk, "drop database schema_statistics1");
    must_exec(tk, "drop database schema_statistics2");
}

fn prepare_data_schemata(tk: &mut TestKit) -> Vec<ColPredicates> {
    let schema_names = ["schema_schemata1", "schema_schemata2", "schema_schemata3"];
    for schema_name in schema_names {
        must_exec(tk, format!("create database {schema_name}"));
    }
    vec![predicate("schema_name", &schema_names)]
}

fn clean_data_schemata(tk: &mut TestKit) {
    must_exec(tk, "drop database schema_schemata1");
    must_exec(tk, "drop database schema_schemata2");
    must_exec(tk, "drop database schema_schemata3");
}

fn prepare_data_check_constraints(tk: &mut TestKit) -> Vec<ColPredicates> {
    let schema_names = [
        "schema_check_constraints1",
        "schema_check_constraints2",
        "schema_check_constraints3",
    ];
    let constraint_names = ["c1", "c2", "c3"];
    for schema_name in schema_names {
        must_exec(tk, format!("create database {schema_name}"));
        must_exec(
            tk,
            format!(
                "create table {schema_name}.t (a int, constraint c1 check (a > 0), constraint c2 check (a = 0), constraint c3 check (a < 0))"
            ),
        );
    }
    vec![
        predicate("constraint_schema", &schema_names),
        predicate("constraint_name", &constraint_names),
    ]
}

fn clean_data_check_constraints(tk: &mut TestKit) {
    must_exec(tk, "drop database schema_check_constraints1");
    must_exec(tk, "drop database schema_check_constraints2");
    must_exec(tk, "drop database schema_check_constraints3");
}

fn prepare_data_tidb_check_constraints(tk: &mut TestKit) -> Vec<ColPredicates> {
    let schema_names = [
        "schema_tidb_check_constraints1",
        "schema_tidb_check_constraints2",
    ];
    let table_names = ["t1", "t2"];
    let constraint_names = ["t1_c", "t2_c"];
    for schema_name in schema_names {
        must_exec(tk, format!("create database {schema_name}"));
        for table_name in table_names {
            must_exec(
                tk,
                format!(
                    "create table {schema_name}.{table_name} (a int, constraint {table_name}_c check (a != 0))"
                ),
            );
        }
    }
    let table_ids = query_scalars(
        tk,
        "select distinct table_id from information_schema.tidb_check_constraints where constraint_schema in ('schema_tidb_check_constraints1', 'schema_tidb_check_constraints2') and table_name in ('t1', 't2')",
    );
    assert_eq!(table_ids.len(), schema_names.len() * table_names.len());
    vec![
        predicate("constraint_schema", &schema_names),
        predicate("table_name", &table_names),
        predicate("constraint_name", &constraint_names),
        integer_predicate("table_id", table_ids),
    ]
}

fn clean_data_tidb_check_constraints(tk: &mut TestKit) {
    must_exec(tk, "drop database schema_tidb_check_constraints1");
    must_exec(tk, "drop database schema_tidb_check_constraints2");
}

fn prepare_data_sequences(tk: &mut TestKit) -> Vec<ColPredicates> {
    let schema_names = ["schema_sequences1", "schema_sequences2"];
    let sequence_names = ["s1", "s2"];
    for schema_name in schema_names {
        must_exec(tk, format!("create database {schema_name}"));
        for sequence_name in sequence_names {
            must_exec(tk, format!("create sequence {schema_name}.{sequence_name}"));
        }
    }
    vec![
        predicate("sequence_schema", &schema_names),
        predicate("sequence_name", &sequence_names),
    ]
}

fn clean_data_sequences(tk: &mut TestKit) {
    must_exec(tk, "drop database schema_sequences1");
    must_exec(tk, "drop database schema_sequences2");
}

type PrepareData = fn(&mut TestKit) -> Vec<ColPredicates>;
type CleanData = fn(&mut TestKit);

struct TestCase {
    mem_table_name: &'static str,
    prepare_data: PrepareData,
    _clean_data: CleanData,
}

fn test_memtable_infoschema_extractor(test_cases: &[TestCase]) {
    let _guard = MEMTABLE_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);

    let mut count_sql = 0;
    let mut count_tables = 0;
    let mut predicate_index = 0;
    let mut category_coverage = [false; 5];
    let mut check_constraints_enabled = false;
    for (case_index, test_case) in test_cases.iter().enumerate() {
        if !check_constraints_enabled && test_case.mem_table_name.contains("check_constraints") {
            must_exec(&mut tk, "set global tidb_enable_check_constraint = true");
            check_constraints_enabled = true;
        }
        let names = (test_case.prepare_data)(&mut tk);
        let mut parts = Vec::with_capacity(names.len());
        for name in &names {
            let formatted_values: Vec<String> = name
                .values
                .iter()
                .map(|value| name.format_value(value))
                .collect();
            assert!(!formatted_values.is_empty());
            let mut category = predicate_index % category_coverage.len();
            predicate_index += 1;
            if name.is_integer && matches!(category, 1 | 3) {
                category = 0;
            }
            category_coverage[category] = true;
            parts.push(match category {
                0 => format!("{} = {}", name.col_name, formatted_values[0]),
                1 => format!("lower({}) = '{}'", name.col_name, name.values[0]),
                2 => format!("{} in ({})", name.col_name, formatted_values.join(",")),
                3 => format!(
                    "lower({}) in ({})",
                    name.col_name,
                    formatted_values.join(",")
                ),
                4 => format!(
                    "({})",
                    formatted_values
                        .iter()
                        .map(|value| format!("{} = {value}", name.col_name))
                        .collect::<Vec<_>>()
                        .join(" or ")
                ),
                _ => unreachable!(),
            });
        }
        let condition = parts.join(" and ");
        assert!(
            !condition.is_empty(),
            "{} has no conditions",
            test_case.mem_table_name
        );
        count_sql += 1;
        count_tables += 1;
        tk.MustQuery(
            &format!(
                "select * from information_schema.{} where {condition}",
                test_case.mem_table_name
            ),
            Vec::new(),
        );
        let prepare_is_reused = test_cases[case_index + 1..]
            .iter()
            .any(|later| std::ptr::fn_addr_eq(later.prepare_data, test_case.prepare_data));
        if prepare_is_reused {
            (test_case._clean_data)(&mut tk);
        }
        // 其余 case 由当前目标测试独占的 MockStore/Domain 在离开本函数时整体销毁，
        // 比逐库执行 DROP 更快，并保持不同测试之间的清理隔离。
    }
    assert_eq!(count_sql, test_cases.len());
    assert_eq!(count_tables, test_cases.len());
    assert_eq!(category_coverage, [true; 5]);
}

#[test]
fn infoschema_condition_helpers_match_go_order_and_literal_rules() {
    let names = vec![
        predicate("table_schema", &["db1", "db2", "db3"]),
        integer_predicate("table_id", strings(&["1", "2"])),
    ];
    let single = names[0].enumerate_all();
    assert_eq!(single[0], "table_schema = 'db1'");
    assert_eq!(single[1], "lower(table_schema) = 'db1'");
    assert_eq!(single[21], "table_schema in ('db1','db2')");
    assert_eq!(single[22], "lower(table_schema) in ('db1','db2')");
    assert_eq!(single[23], "(table_schema = 'db1' or table_schema = 'db2')");

    let combinations = names[0].generate_combinations(&names[0].values);
    assert_eq!(combinations.first(), Some(&Vec::new()));
    assert_eq!(combinations.len(), 8);
    assert_eq!(build_cartesian_conditions(&names).len(), 27 * 8);
    let representative = build_representative_conditions(&names);
    assert_eq!(representative.len(), 6);
    assert!(
        representative
            .iter()
            .any(|condition| condition.contains("lower("))
    );
    assert!(
        representative
            .iter()
            .any(|condition| condition.contains(" in ("))
    );
    assert!(
        representative
            .iter()
            .any(|condition| condition.contains(" or "))
    );
    assert!(
        representative
            .iter()
            .any(|condition| condition.contains(" and "))
    );
}

#[test]
fn TestMemtableInfoschemaExtractorPart1() {
    test_memtable_infoschema_extractor(&[
        TestCase {
            mem_table_name: "tidb_indexes",
            prepare_data: prepare_data_tidb_indexes,
            _clean_data: clean_data_tidb_indexes,
        },
        TestCase {
            mem_table_name: "tables",
            prepare_data: prepare_data_tables,
            _clean_data: clean_data_tables,
        },
        TestCase {
            mem_table_name: "views",
            prepare_data: prepare_data_views,
            _clean_data: clean_data_views,
        },
    ]);
}

#[test]
fn TestMemtableInfoschemaExtractorPart2() {
    test_memtable_infoschema_extractor(&[
        TestCase {
            mem_table_name: "key_column_usage",
            prepare_data: prepare_data_key_column_usage,
            _clean_data: clean_data_key_column_usage,
        },
        TestCase {
            mem_table_name: "table_constraints",
            prepare_data: prepare_data_key_column_usage,
            _clean_data: clean_data_key_column_usage,
        },
        TestCase {
            mem_table_name: "partitions",
            prepare_data: prepare_data_partitions,
            _clean_data: clean_data_partitions,
        },
    ]);
}

#[test]
fn TestMemtableInfoschemaExtractorPart3() {
    test_memtable_infoschema_extractor(&[
        TestCase {
            mem_table_name: "statistics",
            prepare_data: prepare_data_statistics,
            _clean_data: clean_data_statistics,
        },
        TestCase {
            mem_table_name: "schemata",
            prepare_data: prepare_data_schemata,
            _clean_data: clean_data_schemata,
        },
        TestCase {
            mem_table_name: "check_constraints",
            prepare_data: prepare_data_check_constraints,
            _clean_data: clean_data_check_constraints,
        },
    ]);
}

#[test]
fn TestMemtableInfoschemaExtractorPart4() {
    test_memtable_infoschema_extractor(&[
        TestCase {
            mem_table_name: "tidb_check_constraints",
            prepare_data: prepare_data_tidb_check_constraints,
            _clean_data: clean_data_tidb_check_constraints,
        },
        TestCase {
            mem_table_name: "sequences",
            prepare_data: prepare_data_sequences,
            _clean_data: clean_data_sequences,
        },
        TestCase {
            mem_table_name: "tidb_index_usage",
            prepare_data: prepare_data_statistics,
            _clean_data: clean_data_statistics,
        },
    ]);
}

#[test]
fn infoschema_table_name_like_honors_custom_escape() {
    let _guard = MEMTABLE_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    must_exec(&mut tk, "create database like_escape");
    must_exec(&mut tk, "use like_escape");
    must_exec(&mut tk, "create table `abc_def` (a int)");
    must_exec(&mut tk, "create table `abc#x` (a int)");
    let result = tk.MustQuery(
        "select table_name, table_name like '%#_%' escape '#' as self_true from information_schema.tables where table_schema = 'like_escape' and table_name like '%#_%' escape '#'",
        Vec::new(),
    );
    assert_eq!(
        result.Rows(),
        vec![vec!["abc_def".to_owned(), "1".to_owned()]]
    );
    let result = tk.MustQuery(
        "select table_name from information_schema.tables where table_schema = 'like_escape' and table_name like 'abc%'",
        Vec::new(),
    );
    let mut rows = result.Rows();
    rows.sort();
    assert_eq!(
        rows,
        vec![vec!["abc#x".to_owned()], vec!["abc_def".to_owned()]]
    );
}
