// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

//! 实例级执行计划缓存的主测试工作负载。
//!
//! 本模块生成测试表、预处理语句及其等价的直接查询，并通过多个真实会话并发执行，
//! 验证缓存计划在跨会话复用和重复执行时始终返回一致结果。

use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use astersql_testkit::TestKit;

use super::support::{Harness, exec, query, rand, rows};

pub const typeInt: &str = "int";
pub const typeVarchar: &str = "varchar(128)";
pub const typeFloat: &str = "float";
pub const typeDouble: &str = "double";
pub const typeDecimal: &str = "decimal(10,2)";
pub const typeDatetime: &str = "datetime";

/// 从候选集合中随机选择一个元素。
pub fn randomItem(items: &[&str]) -> String {
    items[(rand::intn(items.len() as i32)) as usize].to_owned()
}

pub fn randomItems(items: &[&str]) -> Vec<String> {
    let count = (rand::intn(items.len() as i32 - 1) + 1) as usize;
    let mut result = Vec::with_capacity(count);
    for _ in 0..count {
        let item = items[rand::intn(items.len() as i32) as usize].to_owned();
        if !result.contains(&item) {
            result.push(item);
        }
    }
    result
}

pub fn randomIntVal() -> String {
    match rand::intn(5) {
        0 => "null".to_owned(),
        1 => randomItem(&["0", "-1", "1", "100000000", "-1000000000"]),
        2 => randomItem(&[
            "2147483648",
            "2147483647",
            "2147483646",
            "-2147483648",
            "-2147483647",
            "-2147483646",
        ]),
        3 => randomItem(&[
            "9223372036854775807",
            "9223372036854775808",
            "9223372036854775806",
            "-9223372036854775807",
            "-9223372036854775808",
            "-9223372036854775806",
        ]),
        _ => {
            let values = [
                rand::intn(3) as i64 + 1_000,
                rand::intn(3) as i64 + 1_000_000,
                rand::intn(3) as i64 + 100_000_000_000,
                rand::intn(3) as i64 + 1_000_000_000_000_000,
            ];
            let mut choices = Vec::with_capacity(8);
            for value in values {
                choices.push(value.to_string());
                choices.push((-value).to_string());
            }
            choices[rand::intn(choices.len() as i32) as usize].clone()
        }
    }
}

pub fn randomVarcharVal() -> String {
    match rand::intn(4) {
        0 => "null".to_owned(),
        1 => "''".to_owned(),
        2 => {
            let value = rand::intn(1000);
            if rand::intn(2) == 0 {
                format!("'{value}'")
            } else {
                format!("'-{value}'")
            }
        }
        _ => {
            const TEXT: &str = "weoiruklmdsSDFjfDSFpqru23h#@$@#r90ds8a90dhfksdjfl#@!@#~$@#^BFDSAFDS=========+_+-21KLEJSDKLX;FJP;ipo][1";
            let start = rand::intn(TEXT.len() as i32) as usize;
            let end = start + rand::intn((TEXT.len() - start) as i32) as usize;
            format!("'{}'", &TEXT[start..end])
        }
    }
}

pub fn randomFloat() -> String {
    const ZEROS: &[&str] = &[
        "0",
        "0.000000000",
        "0000.000",
        "-0",
        "-0.000000000",
        "-0000.000",
        "1",
        "1.000000000",
        "0001.000",
        "-1",
        "-1.000000000",
        "-0001.000",
        "0.00001",
        "0.000000001",
        "0000.0000000001",
        "-0.00001",
        "-0.000000001",
        "-0000.0000000001",
    ];
    const DECIMALS: &[&str] = &[
        "1.234",
        "1.23456789",
        "1.234567890123456789",
        "-1.234",
        "-1.23456789",
        "-1.234567890123456789",
        "1234.567",
        "1234.567890123456789",
        "1234.567890123456789123456789",
        "-1234.567",
        "-1234.567890123456789",
        "-1234.567890123456789123456789",
        "0.00001",
        "0.000000001",
        "0000.0000000001",
        "-0.00001",
        "-0.000000001",
        "-0000.0000000001",
    ];
    match rand::intn(4) {
        0 => "null".to_owned(),
        1 => randomItem(ZEROS),
        2 => randomItem(DECIMALS),
        _ => {
            let value = rand::intn(1_000_000) as f64 / 1_000_000.0;
            if rand::intn(2) == 0 {
                value.to_string()
            } else {
                (-value).to_string()
            }
        }
    }
}

pub fn randomDatetime() -> String {
    match rand::intn(3) {
        0 => "null".to_owned(),
        1 => randomItem(&[
            "'2024-01-01 00:00:00'",
            "'2024-01-01 00:00:00.000000'",
            "'2024-01-01 00:00:00.000000000'",
            "'2024-01-01 00:00:00.000000000+08:00'",
        ]),
        _ => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            format_unix_datetime(now + rand::intn(100_000) as u64)
        }
    }
}

fn format_unix_datetime(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds = seconds % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "'{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.000000000'",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

pub fn prepareTableData(table: &str, rows: usize, col_types: &[&str]) -> Vec<String> {
    let column_values = col_types
        .iter()
        .map(|kind| {
            (0..rows)
                .map(|_| match *kind {
                    typeInt => randomIntVal(),
                    typeVarchar => randomVarcharVal(),
                    typeFloat | typeDouble | typeDecimal => randomFloat(),
                    typeDatetime => randomDatetime(),
                    _ => panic!("unsupported column type {kind}"),
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    (0..rows)
        .map(|row| {
            let values = column_values
                .iter()
                .map(|column| column[row].clone())
                .collect::<Vec<_>>();
            format!("insert ignore into {table} values ({});", values.join(", "))
        })
        .collect()
}

/// 生成测试表及其固定规模的数据，供不同会话共享同一组查询对象。
pub fn prepareTables(n: usize) -> Vec<String> {
    let mut statements = Vec::new();
    for table in 0..n {
        let names = ["c0", "c1", "c2", "c3", "c4", "c5"];
        let types = (0..6)
            .map(|_| randomItem(&[typeInt, typeVarchar, typeFloat, typeDouble, typeDatetime]))
            .collect::<Vec<_>>();
        let columns = names
            .iter()
            .zip(&types)
            .map(|(name, kind)| format!("{name} {kind}"))
            .collect::<Vec<_>>();
        let primary = randomItems(&names);
        let index1 = randomItems(&names);
        let index2 = randomItems(&names);
        statements.push(format!(
            "create table t{table} ({}, primary key ({}), index idx1 ({}), index idx2 ({}));",
            columns.join(", "),
            primary.join(", "),
            index1.join(", "),
            index2.join(", ")
        ));
        statements.extend(prepareTableData(
            &format!("t{table}"),
            100,
            &types.iter().map(String::as_str).collect::<Vec<_>>(),
        ));
    }
    statements
}

#[derive(Debug, Clone)]
/// 一组可相互对照的直接查询与预处理语句执行步骤。
pub struct TestCase {
    /// 创建预处理语句的 SQL。
    pub prep_stmt: String,
    /// 将参数直接代入模板后的查询，用于取得预期结果。
    pub sel_stmts: Vec<String>,
    /// 每轮执行前设置用户变量的 SQL。
    pub set_stmts: Vec<String>,
    /// 使用对应用户变量执行预处理语句的 SQL。
    pub exec_stmts: Vec<String>,
}

/// 根据查询模板生成预处理语句，以及 `n` 组直接查询和参数化执行步骤。
pub fn prepareStmts(query_template: &str, n_tables: usize, n: usize) -> TestCase {
    let mut query = query_template.to_owned();
    while query.contains("{T}") {
        query = query.replacen("{T}", &format!("t{}", rand::intn(n_tables as i32)), 1);
    }
    let mut case = TestCase {
        prep_stmt: format!("prepare stmt from '{}'", query),
        sel_stmts: Vec::new(),
        set_stmts: Vec::new(),
        exec_stmts: Vec::new(),
    };
    for _ in 0..n {
        let parameter_count = query.matches('?').count();
        let values = genRandomValues(parameter_count);
        if values.is_empty() {
            continue;
        }
        let mut direct = query.clone();
        // 按占位符顺序逐个替换，使直接查询与随后绑定的用户变量完全对应。
        for value in &values {
            direct = direct.replacen('?', &value.to_string(), 1);
        }
        case.sel_stmts.push(direct);
        case.set_stmts.push(format!(
            "set {}",
            values
                .iter()
                .enumerate()
                .map(|(index, value)| format!("@p{index}={value}"))
                .collect::<Vec<_>>()
                .join(",")
        ));
        case.exec_stmts.push(format!(
            "execute stmt using {}",
            (0..parameter_count)
                .map(|index| format!("@p{index}"))
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    case
}

pub fn genRandomValues(num_vals: usize) -> Vec<String> {
    (0..num_vals)
        .map(|_| match rand::intn(4) {
            0 => randomIntVal(),
            1 => randomVarcharVal(),
            2 => randomFloat(),
            _ => randomDatetime(),
        })
        .collect()
}

pub fn queryPattern() -> Vec<String> {
    let mut patterns = Vec::with_capacity(340);
    for _ in 0..100 {
        patterns.push(format!(
            "select * from {{T}} where {}",
            randomFilters("", 5)
        ));
    }
    for _ in 0..30 {
        let filter = randomFilters("", 5);
        patterns.push(format!(
            "select * from {{T}} where {filter} order by {}",
            randomItem(&["c0", "c1", "c2", "c3"])
        ));
        patterns.push(format!(
            "select * from {{T}} where {} limit 10",
            randomFilters("", 5)
        ));
        patterns.push(format!(
            "select * from {{T}} where {} order by {} limit 10",
            randomFilters("", 5),
            randomItem(&["c0", "c1", "c2", "c3"])
        ));
    }
    for _ in 0..30 {
        patterns.push(format!(
            "select sum(c0) from {{T}} where {} group by {}",
            randomFilters("", 5),
            randomItem(&["c0", "c1", "c2", "c3"])
        ));
        patterns.push(format!(
            "select c0, c1, sum(c2) from {{T}} where {} group by c0, c1",
            randomFilters("", 5)
        ));
    }
    for _ in 0..30 {
        patterns.push(format!(
            "select * from {{T}} t1 join {{T}} t2 on t1.c0=t2.c0 where {}",
            randomFilters("t1", 5)
        ));
        patterns.push(format!(
            "select * from {{T}} t1 join {{T}} t2 on t1.c0=t2.c0 where {}",
            randomFilters("t2", 5)
        ));
        patterns.push(format!(
            "select * from {{T}} t1 join {{T}} t2 on t1.c0=t2.c0 where {} and {}",
            randomFilters("t2", 5),
            randomFilter("t1", 5)
        ));
    }
    patterns
}

pub fn randomFilters(table: &str, n_cols: usize) -> String {
    let filters = (0..rand::intn(3) + 1)
        .map(|_| randomFilter(table, n_cols))
        .collect::<Vec<_>>();
    filters.join(if rand::intn(2) == 0 { " and " } else { " or " })
}

pub fn randomFilter(table: &str, n_cols: usize) -> String {
    let column = format!(
        "{}c{}",
        if table.is_empty() {
            String::new()
        } else {
            format!("{table}.")
        },
        rand::intn(n_cols as i32)
    );
    match rand::intn(10) {
        0 => format!("{column}=?"),
        1 => format!("{column}>?"),
        2 => format!("{column}<?"),
        3 => format!("{column} between ? and ?"),
        4 => format!("{column} in (?, ?, ?)"),
        5 => format!("{column} in (?)"),
        6 => format!("{column} is null"),
        7 => format!("{column} is not null"),
        8 => format!("{column} != ?"),
        _ => format!("{column} like ?"),
    }
}

fn run_case(mut tk: TestKit, case: TestCase) {
    exec(&mut tk, "use test");
    exec(&mut tk, &case.prep_stmt);
    for (direct, (set, execute)) in case
        .sel_stmts
        .iter()
        .zip(case.set_stmts.iter().zip(case.exec_stmts.iter()))
    {
        // 直接查询提供基准；预处理语句首次执行后再重复一次，以覆盖缓存计划复用路径。
        let expected = query(&tk, direct);
        exec(&mut tk, set);
        let mut actual = query(&tk, execute);
        actual.Sort();
        let mut expected = expected;
        expected.Sort().Check(actual.Rows());
        query(&tk, execute).Sort().Check(actual.Rows());
        query(&tk, execute).Sort().Check(actual.Rows());
    }
}

#[test]
/// 启用实例级计划缓存，并发验证多个会话共享缓存计划时的结果一致性。
pub fn TestInstancePlanCache() {
    let mut harness = Harness::new();
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    for statement in prepareTables(10) {
        exec(&mut harness.tk, &statement);
    }
    let cases = queryPattern()
        .into_iter()
        .map(|pattern| prepareStmts(&pattern, 10, 5))
        .collect::<Vec<_>>();
    thread::scope(|scope| {
        // 每个工作线程创建独立会话，但共享同一存储实例和同一批测试用例。
        for _ in 0..5 {
            let store = Arc::clone(&harness.store);
            let cases = cases.clone();
            scope.spawn(move || {
                for case in cases {
                    run_case(TestKit::new(store.clone()), case);
                }
            });
        }
    });
}

#[allow(dead_code)]
/// 为其他并发测试保留的单用例执行入口。
pub fn executeWorker(tk: &mut TestKit, case: &TestCase) {
    run_case(tk.clone(), case.clone());
}

#[allow(dead_code)]
fn assert_query_equal(tk: &TestKit, sql: &str, expected: &[String]) {
    query(tk, sql).Sort().Check(rows(expected));
}

#[test]
fn generated_workload_matches_go_suite_shape() {
    let patterns = queryPattern();
    assert_eq!(patterns.len(), 340);
    assert!(patterns.iter().any(|sql| sql.contains(" order by ")));
    assert!(patterns.iter().any(|sql| sql.contains(" limit 10")));
    assert!(patterns.iter().any(|sql| sql.contains(" group by ")));
    assert!(patterns.iter().any(|sql| sql.contains(" join ")));

    let tables = prepareTables(1);
    assert_eq!(tables.len(), 101);
    assert!(tables[0].contains("index idx1"));
    assert!(tables[0].contains("index idx2"));
}
