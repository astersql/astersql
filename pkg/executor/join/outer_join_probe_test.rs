// Copyright 2026 AsterSQL.

use crate::base_join_probe::{HashJoinContext, Probe, new_join_probe};
use crate::joiner::{JoinType, Joiner, Row};
use crate::row_table_builder::Value;

fn row(value: i64) -> Row {
    vec![Value::Int(value)]
}

fn inner_side_build_probe() -> Box<dyn Probe> {
    let joiner = Joiner::new(JoinType::LeftOuter, false, row(-1), vec![], None, false, 32).unwrap();
    let context = HashJoinContext::new(vec![row(1)], vec![0], vec![0], joiner, true, true, 32);
    new_join_probe(context, 0, JoinType::LeftOuter, true, false).unwrap()
}

#[test]
#[should_panic(expected = "should not reach here")]
fn outer_join_inner_build_init_for_scan_row_table_panics_like_go() {
    inner_side_build_probe().init_for_scan_row_table();
}

#[test]
#[should_panic(expected = "should not reach here")]
fn outer_join_inner_build_scan_row_table_panics_like_go() {
    inner_side_build_probe().scan_row_table();
}

#[test]
#[should_panic(expected = "should not reach here")]
fn outer_join_inner_build_is_scan_row_table_done_panics_like_go() {
    inner_side_build_probe().is_scan_row_table_done();
}
