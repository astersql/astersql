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

#[test]
fn go_merge_195_197_publish_snapshot_all_full_units() {
    use crate::statement_ru_result::*;
    use std::cell::RefCell;
    #[derive(Default)]
    struct Sink(RefCell<Vec<(String, String, String, f64)>>);
    impl StatementRUPublicationSink for Sink {
        fn consumption(&self, _: StatementRUEngineResult) {}
        fn results(&self, _: &StatementRUFinalizedSnapshot) {}
        fn unit(&self, engine: &str, operator: &str, unit: &str, value: f64) {
            self.0
                .borrow_mut()
                .push((engine.into(), operator.into(), unit.into(), value));
        }
        fn statement(&self, status: &str, reason: &str) {
            assert_eq!((status, reason), ("success", "incomplete"));
        }
        fn calibration(&self, _: StatementRUCalibrationState, _: StmtUnits) {}
    }
    let units = StmtUnits {
        cpu_work: 1.0,
        scan_bytes: 2.0,
        net_bytes: 3.0,
        cross_az_net_bytes: 4.0,
        frontend_compile_bytes: 5.0,
        hash_state_rows: 6.0,
        join_output_rows: 7.0,
        write_statement: 8.0,
        operator_num: 9.0,
        write_keys: 10.0,
        write_bytes: 11.0,
    };
    let mut report = StatementRUFullReport::default();
    report.add(
        StatementRUEngine::TiFlash,
        StatementRUOperator::CopTransport,
        units,
    );
    // Unseen entries and zero-valued seen entries must not emit unit metrics.
    report.units[1][0] = units;
    report.add(
        StatementRUEngine::TiDB,
        StatementRUOperator::Wrapper,
        StmtUnits::default(),
    );
    let snapshot = StatementRUFinalizedSnapshot {
        units,
        result: Default::default(),
        engine_ru: Default::default(),
        report: Some(report),
        calibration_state: StatementRUCalibrationState::Incomplete,
        sql_type: "select".into(),
    };
    let sink = Sink::default();
    crate::statement_ru_reporting::publish_statement_ru_full_metrics(&sink, &snapshot);
    let labels = [
        "cpu_work",
        "scan_bytes",
        "net_bytes",
        "cross_az_net_bytes",
        "frontend_compile_bytes",
        "hash_state_rows",
        "join_output_rows",
        "write_statement",
        "operator_num",
        "write_keys",
        "write_bytes",
    ];
    assert_eq!(sink.0.borrow().len(), 11);
    for (index, (engine, operator, unit, value)) in sink.0.borrow().iter().enumerate() {
        assert_eq!(
            (engine.as_str(), operator.as_str(), unit.as_str(), *value),
            ("tiflash", "coprocessor", labels[index], (index + 1) as f64)
        );
    }
}
