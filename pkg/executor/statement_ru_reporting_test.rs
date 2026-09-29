// Copyright 2026 AsterSQL.

use crate::statement_ru_reporting::{
    StatementRUComputeUnits, StatementRUEngine, StatementRUEngineResult, StatementRUFailureReason,
    StatementRUFullReport, StatementRUOperator, statement_ru_engine_result,
};
use astersql_resourcegroup::ruv2::model::{StmtUnits, default_weights};

#[test]
fn go_merge_195_engine_result_attributes_remote_scan_to_tikv() {
    let units = StmtUnits {
        cpu_work: 5.0,
        scan_bytes: 7.0,
        ..Default::default()
    };
    let compute = [
        StatementRUComputeUnits {
            cpu_work: 5.0,
            ..Default::default()
        },
        StatementRUComputeUnits::default(),
        StatementRUComputeUnits::default(),
    ];
    let mut weights = default_weights();
    weights.cpu_work = 2.0;
    weights.scan_byte = 3.0;
    assert_eq!(
        statement_ru_engine_result(units, compute, weights),
        StatementRUEngineResult {
            tidb: 10.0,
            tikv: 21.0,
            tiflash: 0.0,
        }
    );
}

#[test]
fn go_merge_195_engine_result_keeps_tiflash_work_in_its_engine() {
    let units = StmtUnits {
        cpu_work: 10.0,
        scan_bytes: 10.0,
        net_bytes: 6.0,
        cross_az_net_bytes: 1.0,
        join_output_rows: 4.0,
        ..Default::default()
    };
    let compute = [
        StatementRUComputeUnits {
            cpu_work: 2.0,
            ..Default::default()
        },
        StatementRUComputeUnits {
            cpu_work: 3.0,
            ..Default::default()
        },
        StatementRUComputeUnits {
            cpu_work: 5.0,
            join_output_rows: 1.0,
            scan_bytes: 4.0,
            net_bytes: 2.0,
            cross_az_net_bytes: 1.0,
            ..Default::default()
        },
    ];
    let mut weights = default_weights();
    weights.cross_az_net_byte = 2.0;
    assert_eq!(
        statement_ru_engine_result(units, compute, weights),
        StatementRUEngineResult {
            tidb: 5.0,
            tikv: 13.0,
            tiflash: 14.0,
        }
    );
}

#[test]
fn go_merge_195_full_report_splits_remote_units_only_for_non_tiflash() {
    let mut report = StatementRUFullReport::default();
    report.add_operator(
        StatementRUEngine::TiDB,
        StatementRUOperator::Reader,
        StmtUnits {
            cpu_work: 2.0,
            scan_bytes: 3.0,
            net_bytes: 4.0,
            ..Default::default()
        },
    );
    assert_eq!(
        report.units[StatementRUEngine::TiDB as usize][StatementRUOperator::Reader as usize]
            .cpu_work,
        2.0
    );
    assert_eq!(
        report.units[StatementRUEngine::TiDB as usize][StatementRUOperator::Reader as usize]
            .scan_bytes,
        0.0
    );
    assert_eq!(
        report.units[StatementRUEngine::TiKV as usize][StatementRUOperator::Reader as usize]
            .scan_bytes,
        3.0
    );
    assert_eq!(
        report.units[StatementRUEngine::TiKV as usize][StatementRUOperator::Reader as usize]
            .net_bytes,
        4.0
    );
    report.add_operator(
        StatementRUEngine::TiFlash,
        StatementRUOperator::Reader,
        StmtUnits {
            scan_bytes: 5.0,
            ..Default::default()
        },
    );
    assert_eq!(
        report.units[StatementRUEngine::TiFlash as usize][StatementRUOperator::Reader as usize]
            .scan_bytes,
        5.0
    );
}

#[test]
fn go_merge_195_failure_status_matches_go_metric_labels() {
    use StatementRUFailureReason::*;
    assert_eq!(
        (Unsupported.status(), Unsupported.label()),
        ("skipped", "unsupported_plan")
    );
    assert_eq!(
        (Ineligible.status(), Ineligible.label()),
        ("skipped", "ineligible")
    );
    assert_eq!(
        (NotFinished.status(), NotFinished.label()),
        ("failed", "not_finished")
    );
    assert_eq!(
        (Invalid.status(), Invalid.label()),
        ("failed", "invalid_plan_or_evidence")
    );
    assert_eq!(
        (StatementError.status(), StatementError.label()),
        ("failed", "statement_error")
    );
    assert_eq!((Panic.status(), Panic.label()), ("failed", "panic"));
}
