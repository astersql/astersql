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

/// Go buildRepresentativeConditions 的 Rust 对应物。
fn build_representative_conditions(names: &[ColPredicates]) -> Vec<String> {
    let predicates: Vec<Vec<String>> = names
        .iter()
        .map(ColPredicates::enumerate_all)
        .filter(|conditions| !conditions.is_empty())
        .collect();
    let mut conditions = Vec::new();
    let mut seen = BTreeSet::new();

    let mut add_condition = |parts: Vec<String>| {
        let condition = parts.join(" and ");
        if seen.insert(condition.clone()) {
            conditions.push(condition);
        }
    };

    for all in &predicates {
        for condition in all {
            add_condition(vec![condition.clone()]);
        }
    }

    for left_index in 0..predicates.len() {
        for right_index in left_index + 1..predicates.len() {
            for left in &predicates[left_index] {
                for right in &predicates[right_index] {
                    add_condition(vec![left.clone(), right.clone()]);
                }
            }
        }
    }

    if predicates.len() <= 1 {
        return conditions;
    }

    let canonical: Vec<String> = predicates.iter().map(|all| all[0].clone()).collect();
    add_condition(canonical.clone());

    for (index, all) in predicates.iter().enumerate() {
        for condition in all {
            let mut parts = canonical.clone();
            parts[index] = condition.clone();
            add_condition(parts);
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

fn query_scalar(tk: &TestKit, sql: impl AsRef<str>) -> String {
    let sql = sql.as_ref();
    let result = tk.MustQuery(sql, Vec::new());
    let rows = result.Rows();
    assert_eq!(rows.len(), 1, "expected one row: {sql}");
    assert_eq!(rows[0].len(), 1, "expected one column: {sql}");
    rows[0][0].clone()
}

fn prepare_data_tables(tk: &mut TestKit) -> Vec<ColPredicates> {
    let schema_names = ["schema_tables1", "schema_tables2"];
    let table_names = ["t1", "t2"];
    let mut ids = Vec::new();
    for schema_name in schema_names {
        must_exec(tk, format!("create database {schema_name}"));
        for table_name in table_names {
            must_exec(
                tk,
                format!("create table {schema_name}.{table_name} (a int)"),
            );
            ids.push(query_scalar(
                tk,
                format!(
                    "select tidb_table_id from information_schema.tables where table_schema = '{schema_name}' and table_name = '{table_name}'"
                ),
            ));
        }
    }
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
    let mut table_ids = Vec::new();
    for schema_name in schema_names {
        must_exec(tk, format!("create database {schema_name}"));
        for table_name in table_names {
            must_exec(
                tk,
                format!(
                    "create table {schema_name}.{table_name} (a int, constraint {table_name}_c check (a != 0))"
                ),
            );
            table_ids.push(query_scalar(
                tk,
                format!(
                    "select distinct table_id from information_schema.tidb_check_constraints where CONSTRAINT_SCHEMA = '{schema_name}' and TABLE_NAME = '{table_name}'"
                ),
            ));
        }
    }
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
type BuildConditions = fn(&[ColPredicates]) -> Vec<String>;

struct TestCase {
    mem_table_name: &'static str,
    prepare_data: PrepareData,
    clean_data: CleanData,
    build_conditions: Option<BuildConditions>,
}

fn test_memtable_infoschema_extractor(test_cases: &[TestCase]) {
    let _guard = MEMTABLE_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);

    must_exec(&mut tk, "set global tidb_enable_check_constraint = true");
    let mut count_sql = 0;
    for test_case in test_cases {
        let names = (test_case.prepare_data)(&mut tk);
        let conditions = test_case
            .build_conditions
            .unwrap_or(build_cartesian_conditions)(&names);
        count_sql += conditions.len();
        for condition in conditions {
            tk.MustQuery(
                &format!(
                    "select * from information_schema.{} where {condition}",
                    test_case.mem_table_name
                ),
                Vec::new(),
            );
        }
        (test_case.clean_data)(&mut tk);
    }
    assert!(count_sql > 0);
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
}

#[test]
fn TestMemtableInfoschemaExtractorPart1() {
    test_memtable_infoschema_extractor(&[
        TestCase {
            mem_table_name: "tidb_indexes",
            prepare_data: prepare_data_tidb_indexes,
            clean_data: clean_data_tidb_indexes,
            build_conditions: None,
        },
        TestCase {
            mem_table_name: "tables",
            prepare_data: prepare_data_tables,
            clean_data: clean_data_tables,
            build_conditions: None,
        },
        TestCase {
            mem_table_name: "views",
            prepare_data: prepare_data_views,
            clean_data: clean_data_views,
            build_conditions: None,
        },
    ]);
}

#[test]
fn TestMemtableInfoschemaExtractorPart2() {
    test_memtable_infoschema_extractor(&[
        TestCase {
            mem_table_name: "key_column_usage",
            prepare_data: prepare_data_key_column_usage,
            clean_data: clean_data_key_column_usage,
            build_conditions: Some(build_representative_conditions),
        },
        TestCase {
            mem_table_name: "table_constraints",
            prepare_data: prepare_data_key_column_usage,
            clean_data: clean_data_key_column_usage,
            build_conditions: Some(build_representative_conditions),
        },
        TestCase {
            mem_table_name: "partitions",
            prepare_data: prepare_data_partitions,
            clean_data: clean_data_partitions,
            build_conditions: Some(build_representative_conditions),
        },
    ]);
}

#[test]
fn TestMemtableInfoschemaExtractorPart3() {
    test_memtable_infoschema_extractor(&[
        TestCase {
            mem_table_name: "statistics",
            prepare_data: prepare_data_statistics,
            clean_data: clean_data_statistics,
            build_conditions: None,
        },
        TestCase {
            mem_table_name: "schemata",
            prepare_data: prepare_data_schemata,
            clean_data: clean_data_schemata,
            build_conditions: None,
        },
        TestCase {
            mem_table_name: "check_constraints",
            prepare_data: prepare_data_check_constraints,
            clean_data: clean_data_check_constraints,
            build_conditions: None,
        },
    ]);
}

#[test]
fn TestMemtableInfoschemaExtractorPart4() {
    test_memtable_infoschema_extractor(&[
        TestCase {
            mem_table_name: "tidb_check_constraints",
            prepare_data: prepare_data_tidb_check_constraints,
            clean_data: clean_data_tidb_check_constraints,
            build_conditions: Some(build_representative_conditions),
        },
        TestCase {
            mem_table_name: "sequences",
            prepare_data: prepare_data_sequences,
            clean_data: clean_data_sequences,
            build_conditions: None,
        },
        TestCase {
            mem_table_name: "tidb_index_usage",
            prepare_data: prepare_data_statistics,
            clean_data: clean_data_statistics,
            build_conditions: None,
        },
    ]);
}
