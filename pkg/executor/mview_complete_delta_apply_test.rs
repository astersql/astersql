// Copyright 2026 AsterSQL.

use crate::mview_complete_delta_apply::*;
use astersql_types::datum::{Datum, NewIntDatum};
use std::sync::Mutex;

#[derive(Default, Clone)]
struct State {
    rows: Vec<Vec<Datum>>,
    updates: Vec<Vec<bool>>,
}
struct Runtime {
    state: Mutex<State>,
    fail_after: Option<usize>,
}
struct Batch {
    staged: State,
    fail_after: Option<usize>,
    ops: usize,
}
impl MutationBatch for Batch {
    fn insert(&mut self, row: &[Datum]) -> Result<()> {
        self.ops += 1;
        if self.fail_after == Some(self.ops) {
            return Err("write failed".into());
        }
        self.staged.rows.push(row.to_vec());
        Ok(())
    }
    fn delete(&mut self, row: &[Datum]) -> Result<()> {
        self.ops += 1;
        if self.fail_after == Some(self.ops) {
            return Err("write failed".into());
        }
        if let Some(i) = self.staged.rows.iter().position(|r| same(r, row)) {
            self.staged.rows.remove(i);
        }
        Ok(())
    }
    fn update(&mut self, old: &[Datum], new: &[Datum], touched: &[bool]) -> Result<()> {
        self.ops += 1;
        if self.fail_after == Some(self.ops) {
            return Err("write failed".into());
        }
        let i = self
            .staged
            .rows
            .iter()
            .position(|r| same(r, old))
            .ok_or("old row absent")?;
        self.staged.rows[i] = new.to_vec();
        self.staged.updates.push(touched.to_vec());
        Ok(())
    }
}
fn same(a: &[Datum], b: &[Datum]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.Equals(y))
}
impl MutationRuntime for Runtime {
    fn run_atomic(&self, f: &mut dyn FnMut(&mut dyn MutationBatch) -> Result<()>) -> Result<()> {
        let initial = self.state.lock().unwrap().clone();
        let mut batch = Batch {
            staged: initial,
            fail_after: self.fail_after,
            ops: 0,
        };
        f(&mut batch)?;
        *self.state.lock().unwrap() = batch.staged;
        Ok(())
    }
}
fn row(values: &[i64]) -> Vec<Datum> {
    values.iter().map(|v| NewIntDatum(*v)).collect()
}

#[test]
fn mview_complete_delta_applies_and_marks_only_changed_columns() {
    let rt = Runtime {
        state: Mutex::new(State {
            rows: vec![row(&[1, 10])],
            updates: vec![],
        }),
        fail_after: None,
    };
    let stats = apply(
        &rt,
        &[
            DeltaRow {
                op: DIFF_UPDATE,
                current: row(&[1, 10]),
                recomputed: row(&[1, 11]),
            },
            DeltaRow {
                op: DIFF_INSERT,
                current: vec![],
                recomputed: row(&[2, 20]),
            },
        ],
    )
    .unwrap();
    assert_eq!(
        stats,
        ApplyStats {
            inserted: 1,
            updated: 1,
            deleted: 0
        }
    );
    assert_eq!(rt.state.lock().unwrap().updates, vec![vec![false, true]]);
}
#[test]
fn mview_complete_delta_noops_unchanged_update() {
    let rt = Runtime {
        state: Mutex::new(State {
            rows: vec![row(&[1])],
            updates: vec![],
        }),
        fail_after: None,
    };
    assert_eq!(
        apply(
            &rt,
            &[DeltaRow {
                op: DIFF_UPDATE,
                current: row(&[1]),
                recomputed: row(&[1])
            }]
        )
        .unwrap(),
        ApplyStats::default()
    );
}
#[test]
fn mview_complete_delta_rolls_back_statement_on_error() {
    let rt = Runtime {
        state: Mutex::new(State::default()),
        fail_after: Some(2),
    };
    assert!(
        apply(
            &rt,
            &[
                DeltaRow {
                    op: DIFF_INSERT,
                    current: vec![],
                    recomputed: row(&[1])
                },
                DeltaRow {
                    op: DIFF_INSERT,
                    current: vec![],
                    recomputed: row(&[2])
                }
            ]
        )
        .is_err()
    );
    assert!(rt.state.lock().unwrap().rows.is_empty());
}
#[test]
fn mview_complete_delta_rejects_invalid_op() {
    let rt = Runtime {
        state: Mutex::new(State::default()),
        fail_after: None,
    };
    assert!(
        apply(
            &rt,
            &[DeltaRow {
                op: 9,
                current: vec![],
                recomputed: vec![]
            }]
        )
        .unwrap_err()
        .contains("invalid diff op")
    );
}

#[test]
fn port_a87bf8db64_runtime_step_contracts() {
    use crate::mview_refresh_steps::*;
    use astersql_parser_ast::RefreshMaterializedViewMode as Mode;
    for (mode, names) in [
        (
            Mode::Fast,
            vec![
                "TXN_BEGIN",
                "LOCK_REFRESH_INFO",
                "INSERT_HIST_RUNNING",
                "DATA_CHANGE_FAST_MERGE",
                "PERSIST_REFRESH_INFO",
                "TXN_COMMIT",
                "FINALIZE_HIST",
            ],
        ),
        (
            Mode::CompleteInPlace,
            vec![
                "TXN_BEGIN",
                "LOCK_REFRESH_INFO",
                "INSERT_HIST_RUNNING",
                "DATA_CHANGE_COMPLETE_DELETE",
                "DATA_CHANGE_COMPLETE_INSERT",
                "PERSIST_REFRESH_INFO",
                "TXN_COMMIT",
                "FINALIZE_HIST",
            ],
        ),
        (
            Mode::CompleteDeltaApply,
            vec![
                "TXN_BEGIN",
                "LOCK_REFRESH_INFO",
                "INSERT_HIST_RUNNING",
                "DATA_CHANGE_COMPLETE_DELTA_APPLY",
                "PERSIST_REFRESH_INFO",
                "TXN_COMMIT",
                "FINALIZE_HIST",
            ],
        ),
        (
            Mode::CompleteOutOfPlace,
            vec![
                "INSERT_HIST_RUNNING",
                "OUT_OF_PLACE_CREATE_SHADOW",
                "OUT_OF_PLACE_LOAD_SHADOW",
                "OUT_OF_PLACE_CUTOVER",
                "FINALIZE_HIST",
            ],
        ),
    ] {
        let steps = new_step_set(mode);
        assert_eq!(
            steps.iter().map(|step| step.name).collect::<Vec<_>>(),
            names
        );
        for (index, step) in steps.iter().enumerate() {
            assert_eq!(step.id, format!("S{:02}", index + 1));
        }
    }
    let mut rows = vec![
        vec!["TableReader".into(), "1".into()],
        vec!["TableScan".into(), "2".into()],
    ];
    let copy = clone_plan_rows(&rows);
    rows[0][0] = "changed".into();
    assert_eq!(copy[0][0], "TableReader");
    assert!(clone_plan_rows(&[]).is_empty());
}

#[test]
fn port_a87bf8db64_duration_and_history_contracts() {
    use crate::mview_task::*;
    use std::time::{Duration, UNIX_EPOCH};
    let start = UNIX_EPOCH + Duration::from_secs(42);
    let end = start + Duration::from_nanos(1_123_456_789);
    assert_eq!(
        format_duration_seconds_between(Some(start), Some(end)),
        "1.123456"
    );
    assert_eq!(
        format_duration_seconds_between(Some(end), Some(start)),
        "0.000000"
    );
    assert_eq!(format_duration_seconds_between(None, Some(end)), "0.000000");
    assert_eq!(
        format_duration_seconds_between(Some(start), None),
        "0.000000"
    );
    for value in [-1, 0, 1, 999_999, 1_000_000] {
        let expected = if value <= 0 {
            "0.000000".to_owned()
        } else {
            format!("{}.{:06}", value / 1_000_000, value % 1_000_000)
        };
        assert_eq!(format_duration_seconds_from_microseconds(value), expected);
    }
    assert_eq!(
        format_duration_seconds(Duration::from_nanos(999)),
        "0.000000"
    );
    assert_eq!(hist_time(None), None);
    assert_eq!(
        hist_time(Some(end)),
        Some(start + Duration::from_micros(1_123_456))
    );
    assert_eq!(
        hist_time(Some(UNIX_EPOCH - Duration::from_nanos(1))),
        Some(UNIX_EPOCH - Duration::from_micros(1))
    );
}

#[test]
fn port_a87bf8db64_observer_preserves_errors_and_unwinding() {
    use crate::mview_refresh_steps::*;
    use astersql_parser_ast::RefreshMaterializedViewMode;
    use std::time::SystemTime;
    #[derive(Default)]
    struct Observer(Vec<String>);
    impl StepObserver for Observer {
        fn on_step_start(&mut self, step: &ObserveStep, _: SystemTime) {
            self.0.push(format!("start:{}", step.name));
        }
        fn on_step_end(&mut self, step: &ObserveStep, _: SystemTime, error: Option<&str>) {
            self.0
                .push(format!("end:{}:{}", step.name, error.unwrap_or("ok")));
        }
        fn on_step_plan_rows(&mut self, _: &ObserveStep, _: Vec<Vec<String>>) {}
    }
    let step = new_step_set(RefreshMaterializedViewMode::Fast).remove(0);
    let mut observer = Observer::default();
    let error = observe_step(&mut Some(&mut observer), &step, || {
        Err::<(), _>("runtime failure".to_owned())
    })
    .unwrap_err();
    assert_eq!(error, "runtime failure");
    assert_eq!(
        observer.0,
        ["start:TXN_BEGIN", "end:TXN_BEGIN:runtime failure"]
    );
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = observe_step(
            &mut Some(&mut observer),
            &step,
            || -> std::result::Result<(), String> { panic!("original refresh panic") },
        );
    }));
    assert!(panic.is_err());
    assert_eq!(observer.0.last().unwrap(), "start:TXN_BEGIN");
    assert_eq!(observe_step(&mut None, &step, || Ok(42)).unwrap(), 42);
}

#[test]
fn port_a87bf8db64_restore_refresh_statement_for_slow_log() {
    use astersql_parser_ast as ast;
    let implementation = ast::RefreshMaterializedViewImplementStmt {
        RefreshStmt: Some(ast::RefreshMaterializedViewStmt {
            ViewName: Some(ast::TableName {
                Name: ast::NewCIStr("mv"),
                ..Default::default()
            }),
            Type: ast::RefreshMaterializedViewType::Complete,
            CompleteType: ast::RefreshMaterializedViewCompleteType::InPlace,
            ..Default::default()
        }),
        LastSuccessfulRefreshReadTSO: 123,
        TargetRefreshReadTSO: 456,
        ..Default::default()
    };
    assert_eq!(
        crate::adapter::restoreStmtTextForSlowLogWhenEmptySQL(&implementation),
        "IMPLEMENT FOR REFRESH MATERIALIZED VIEW `mv` COMPLETE IN PLACE USING TIMESTAMP"
    );
    assert!(
        crate::adapter::restoreStmtTextForSlowLogWhenEmptySQL(
            &ast::RefreshMaterializedViewImplementStmt::default()
        )
        .is_empty()
    );
}
