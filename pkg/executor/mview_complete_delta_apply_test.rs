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
