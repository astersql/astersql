// Copyright 2026 AsterSQL.

use crate::statement_ru_reporting::{
    StatementRUComputeUnits, StatementRUEngine, StatementRUOperator,
};
use crate::statement_ru_result::{
    ScanEvidence, StatementRUCalculationSetup, StatementRUCalculator, classify_scan_evidence,
    trim_statement_ru_explain_prefix,
};
use astersql_resourcegroup::ruv2::model::StmtUnits;

#[test]
fn go_merge_197_scan_evidence_matches_go_validity_contract() {
    assert_eq!(classify_scan_evidence(0, 0, 0), ScanEvidence::Valid(0.0));
    assert_eq!(classify_scan_evidence(9, 0, 0), ScanEvidence::Valid(0.0));
    assert_eq!(classify_scan_evidence(9, 0, 1), ScanEvidence::Invalid);
    assert_eq!(classify_scan_evidence(-1, 1, 1), ScanEvidence::Invalid);
    assert_eq!(classify_scan_evidence(9, -1, 1), ScanEvidence::Invalid);
    assert_eq!(classify_scan_evidence(9, 1, -1), ScanEvidence::Invalid);
    assert_eq!(classify_scan_evidence(0, 1, 10), ScanEvidence::Unavailable);
    assert_eq!(classify_scan_evidence(9, 1, 0), ScanEvidence::Unavailable);
    assert_eq!(classify_scan_evidence(9, 3, 12), ScanEvidence::Valid(36.0));
    assert_eq!(
        classify_scan_evidence(i64::MAX, 1, i64::MAX),
        ScanEvidence::Valid((i64::MAX as f64) * (i64::MAX as f64))
    );
}

#[test]
fn go_merge_197_trim_explain_prefix_matches_go_exact_forms() {
    assert_eq!(
        trim_statement_ru_explain_prefix("explain analyze format = ? select * from t"),
        "select * from t"
    );
    assert_eq!(
        trim_statement_ru_explain_prefix("explain analyze format = ru select * from t"),
        "select * from t"
    );
    assert_eq!(
        trim_statement_ru_explain_prefix("explain analyze format = ? "),
        "explain analyze format = ? "
    );
    assert_eq!(trim_statement_ru_explain_prefix("select 1"), "select 1");
}

#[test]
fn go_merge_197_finalize_applies_tiflash_multiplier_and_freezes_report() {
    let mut calculator = StatementRUCalculator::new(StatementRUCalculationSetup {
        frontend_compile_bytes: 2.0,
        full_report: true,
    });
    calculator.units.cpu_work = 3.0;
    calculator.compute[StatementRUEngine::TiFlash as usize] = StatementRUComputeUnits {
        cpu_work: 3.0,
        ..Default::default()
    };
    let report = calculator.report.as_mut().unwrap();
    report.add_operator(
        StatementRUEngine::TiFlash,
        StatementRUOperator::HashAgg,
        StmtUnits {
            cpu_work: 3.0,
            ..Default::default()
        },
    );
    let finalized = calculator.finalize().unwrap();
    assert_eq!(finalized.result.total_ru, 32.0);
    assert_eq!(finalized.engine_ru.tiflash, 30.0);
    assert_eq!(finalized.engine_ru.tidb, 2.0);
    let frozen = finalized.report.unwrap();
    assert_eq!(
        frozen.units[StatementRUEngine::TiDB as usize][StatementRUOperator::Frontend as usize]
            .frontend_compile_bytes,
        2.0
    );
    assert_eq!(
        frozen.units[StatementRUEngine::TiFlash as usize][StatementRUOperator::HashAgg as usize]
            .cpu_work,
        3.0
    );
    assert!(
        !calculator.report.as_ref().unwrap().seen[StatementRUEngine::TiDB as usize]
            [StatementRUOperator::Frontend as usize]
    );
}

#[test]
fn go_merge_197_finalize_rejects_invalid_units() {
    let mut calculator = StatementRUCalculator::new(StatementRUCalculationSetup::default());
    calculator.units.cpu_work = f64::NAN;
    assert!(calculator.finalize().is_none());
}
