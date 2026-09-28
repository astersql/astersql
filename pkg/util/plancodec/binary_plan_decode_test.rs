// Copyright 2026 AsterSQL.

use protobuf::Message;

use crate::{Compress, DecodeBinaryPlan, tipb};

#[test]
fn act_rows_uses_go_int64_conversion() {
    let mut operator = tipb::ExplainOperator::new();
    operator.set_name("TableFullScan".to_owned());
    operator.set_task_type(tipb::TaskType::Root);
    operator.set_act_rows(u64::MAX);

    let mut data = tipb::ExplainData::new();
    data.set_main(operator);
    data.set_with_runtime_stats(true);
    let encoded = Compress(&data.write_to_bytes().unwrap());

    let decoded = DecodeBinaryPlan(&encoded).unwrap();
    let row = decoded
        .lines()
        .find(|line| line.contains("TableFullScan"))
        .unwrap();
    let fields: Vec<_> = row.split('|').map(str::trim).collect();
    assert_eq!(fields[4], "-1");
}

#[test]
fn estimates_use_go_non_finite_float_spelling() {
    let mut operator = tipb::ExplainOperator::new();
    operator.set_name("TableFullScan".to_owned());
    operator.set_task_type(tipb::TaskType::Root);
    operator.set_est_rows(f64::INFINITY);
    operator.set_cost(f64::NEG_INFINITY);

    let mut data = tipb::ExplainData::new();
    data.set_main(operator);
    let encoded = Compress(&data.write_to_bytes().unwrap());

    let decoded = DecodeBinaryPlan(&encoded).unwrap();
    let row = decoded
        .lines()
        .find(|line| line.contains("TableFullScan"))
        .unwrap();
    let fields: Vec<_> = row.split('|').map(str::trim).collect();
    assert_eq!(fields[2], "+Inf");
    assert_eq!(fields[3], "-Inf");
}
