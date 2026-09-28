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

use crate::indexadvisor::advise_indexes_for_sql;
use crate::model::Index;
use crate::optimizer::{FieldType, InMemoryOptimizer, Optimizer, TableMetadata};
use crate::options::AdvisorOptions;
use std::collections::BTreeMap;
use std::time::Duration;

/// Keep the Rust scenarios byte-for-byte tied to the canonical Go TPC-H workload.
fn tpch_queries() -> BTreeMap<usize, String> {
    let source = include_str!("indexadvisor_tpch_test.go");
    let mut queries = BTreeMap::new();
    for number in (1..=14).chain(16..=22) {
        let marker = format!("tpchQ{number} = `");
        let start = source
            .find(&marker)
            .unwrap_or_else(|| panic!("missing Go fixture {marker}"))
            + marker.len();
        let end = source[start..]
            .find("\n`")
            .unwrap_or_else(|| panic!("unterminated Go fixture {marker}"))
            + start;
        queries.insert(
            number,
            source[start..end].replace(concat!("/*PLACE", "HOLDER*/"), ""),
        );
    }
    assert_eq!(
        queries.len(),
        21,
        "TPC-H intentionally has no query 15 here"
    );
    queries
}

fn table(columns: &[&str]) -> TableMetadata {
    TableMetadata {
        columns: columns
            .iter()
            .map(|column| (column.to_string(), FieldType::Integer))
            .collect(),
        indexes: Vec::new(),
        row_count: 1,
        column_total_size: BTreeMap::new(),
    }
}

fn tpch_optimizer() -> InMemoryOptimizer {
    let tables = BTreeMap::from([
        (
            ("test".into(), "nation".into()),
            table(&["n_nationkey", "n_name", "n_regionkey", "n_comment"]),
        ),
        (
            ("test".into(), "region".into()),
            table(&["r_regionkey", "r_name", "r_comment"]),
        ),
        (
            ("test".into(), "part".into()),
            table(&[
                "p_partkey",
                "p_name",
                "p_mfgr",
                "p_brand",
                "p_type",
                "p_size",
                "p_container",
                "p_retailprice",
                "p_comment",
            ]),
        ),
        (
            ("test".into(), "supplier".into()),
            table(&[
                "s_suppkey",
                "s_name",
                "s_address",
                "s_nationkey",
                "s_phone",
                "s_acctbal",
                "s_comment",
            ]),
        ),
        (
            ("test".into(), "partsupp".into()),
            table(&[
                "ps_partkey",
                "ps_suppkey",
                "ps_availqty",
                "ps_supplycost",
                "ps_comment",
            ]),
        ),
        (
            ("test".into(), "customer".into()),
            table(&[
                "c_custkey",
                "c_name",
                "c_address",
                "c_nationkey",
                "c_phone",
                "c_acctbal",
                "c_mktsegment",
                "c_comment",
            ]),
        ),
        (
            ("test".into(), "orders".into()),
            table(&[
                "o_orderkey",
                "o_custkey",
                "o_orderstatus",
                "o_totalprice",
                "o_orderdate",
                "o_orderpriority",
                "o_clerk",
                "o_shippriority",
                "o_comment",
            ]),
        ),
        (
            ("test".into(), "lineitem".into()),
            table(&[
                "l_orderkey",
                "l_partkey",
                "l_suppkey",
                "l_linenumber",
                "l_quantity",
                "l_extendedprice",
                "l_discount",
                "l_tax",
                "l_returnflag",
                "l_linestatus",
                "l_shipdate",
                "l_commitdate",
                "l_receiptdate",
                "l_shipinstruct",
                "l_shipmode",
                "l_comment",
            ]),
        ),
    ]);
    InMemoryOptimizer::new(tables, |sql, indexes| {
        let sql = sql.to_ascii_lowercase();
        let useful = indexes
            .iter()
            .filter(|index: &&Index| {
                index
                    .columns
                    .first()
                    .is_some_and(|column| sql.contains(&column.column_name))
            })
            .count();
        Ok(100.0 / (useful + 1) as f64)
    })
}

fn check_tpch_group(numbers: &[usize]) {
    let queries = tpch_queries();
    let sqls = numbers
        .iter()
        .map(|number| queries[number].clone())
        .collect::<Vec<_>>();
    let options = AdvisorOptions {
        timeout: Duration::from_secs(120),
        ..AdvisorOptions::default()
    };
    let recommendations =
        advise_indexes_for_sql(&tpch_optimizer() as &dyn Optimizer, &sqls, "test", &options)
            .unwrap();
    assert!(
        !recommendations.is_empty(),
        "Go expects recommendations for TPC-H queries {numbers:?}"
    );
}

#[test]
fn index_advisor_tpch_1_matches_go_workload() {
    check_tpch_group(&[1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn index_advisor_tpch_2_matches_go_workload() {
    check_tpch_group(&[8, 9, 10, 11, 12]);
}

#[test]
fn index_advisor_tpch_3_matches_go_workload() {
    check_tpch_group(&[13, 14, 16, 17, 18]);
}

#[test]
fn index_advisor_tpch_4_matches_go_workload() {
    check_tpch_group(&[19, 20, 21, 22]);
}
