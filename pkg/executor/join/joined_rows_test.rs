// Copyright 2026 AsterSQL.

use crate::hash_join_v1::{HashJoinCtxV1, HashJoinV1Exec};
use crate::hash_join_v2::{HashJoinCtxV2, HashJoinV2Exec};
use crate::index_lookup_hash_join::IndexNestedLoopHashJoin;
use crate::index_lookup_join::{
    IndexJoinExecutorBuilder, IndexJoinLookupContent, IndexLookUpJoin, InnerCtx, OuterCtx,
};
use crate::index_lookup_merge_join::IndexLookUpMergeJoin;
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::merge_join::{MergeJoinExec, MergeJoinTable};
use crate::row_table_builder::Value;
use astersql_util_execdetails::execdetails::{NewRuntimeStatsColl, RuntimeStatsColl};
use std::sync::{Arc, Mutex};

type Collection = Arc<Mutex<RuntimeStatsColl>>;
fn row(n: i64) -> Row {
    vec![Value::Int(n)]
}
fn inner_rows() -> Vec<Row> {
    vec![row(1), row(1), row(1)]
}
fn outer_rows() -> Vec<Row> {
    vec![row(1), row(1), row(2), vec![Value::Null]]
}

struct Lookup;
impl IndexJoinExecutorBuilder for Lookup {
    fn build(&self, content: &[IndexJoinLookupContent]) -> Result<Vec<Row>, String> {
        Ok(inner_rows()
            .into_iter()
            .filter(|row| content.iter().any(|key| key.keys == *row))
            .collect())
    }
}

trait Lifecycle {
    fn open(&mut self);
    fn drain(&mut self) -> Result<usize, String>;
    fn close(&mut self);
}
macro_rules! lifecycle {
    ($ty:ty, $next:expr) => {
        impl Lifecycle for $ty {
            fn open(&mut self) {
                self.open().unwrap();
            }
            fn drain(&mut self) -> Result<usize, String> {
                let mut rows = 0;
                loop {
                    let output: Vec<Row> = ($next)(self)?;
                    if output.is_empty() {
                        break;
                    }
                    rows += output.len();
                }
                Ok(rows)
            }
            fn close(&mut self) {
                self.close();
            }
        }
    };
}
lifecycle!(HashJoinV1Exec, |e: &mut HashJoinV1Exec| e
    .next()
    .map(|r| r.rows));
lifecycle!(HashJoinV2Exec, |e: &mut HashJoinV2Exec| e
    .next()
    .map(|r| r.rows));
lifecycle!(IndexLookUpJoin, |e: &mut IndexLookUpJoin| e.next(2));
lifecycle!(
    IndexNestedLoopHashJoin,
    |e: &mut IndexNestedLoopHashJoin| e.next(2).map(|r| r.rows)
);
lifecycle!(IndexLookUpMergeJoin, |e: &mut IndexLookUpMergeJoin| e
    .next(2)
    .map(|r| r.rows));
lifecycle!(MergeJoinExec, |e: &mut MergeJoinExec| e.next(2));

fn executor(
    algorithm: usize,
    kind: JoinType,
    condition: Vec<Predicate>,
    coll: Option<Collection>,
) -> Box<dyn Lifecycle> {
    let joiner = Joiner::new(
        kind,
        false,
        vec![Value::Null],
        condition,
        Some([vec![], vec![]]),
        false,
        2,
    )
    .unwrap();
    macro_rules! bound {
        ($e:expr) => {{
            let e = $e;
            Box::new(if let Some(coll) = coll {
                e.with_runtime_stats(7, coll)
            } else {
                e
            }) as Box<dyn Lifecycle>
        }};
    }
    match algorithm {
        0 => bound!(
            HashJoinV1Exec::new(
                HashJoinCtxV1 {
                    base: Default::default(),
                    join_type: kind,
                    build_key_indices: vec![0],
                    probe_key_indices: vec![0],
                    build_side_is_outer: false,
                    null_aware: false,
                    concurrency: 2,
                    max_chunk_size: 2
                },
                joiner,
                vec![inner_rows()],
                vec![outer_rows()]
            )
            .unwrap()
        ),
        1 | 2 => bound!(
            HashJoinV2Exec::new(
                HashJoinCtxV2::new(
                    kind,
                    vec![0],
                    vec![0],
                    true,
                    false,
                    2,
                    2,
                    if algorithm == 2 { Some(1) } else { None }
                )
                .unwrap(),
                joiner,
                vec![inner_rows()],
                vec![outer_rows()]
            )
            .unwrap()
        ),
        3 => bound!(
            IndexLookUpJoin::new(
                OuterCtx {
                    rows: outer_rows(),
                    key_columns: vec![0],
                    filters: vec![]
                },
                InnerCtx {
                    builder: Box::new(Lookup),
                    key_columns: vec![0],
                    key_column_ids: vec![1],
                    null_safe: false
                },
                joiner,
                true,
                1,
                4
            )
            .unwrap()
        ),
        4 | 5 => bound!(
            IndexNestedLoopHashJoin::new(
                outer_rows(),
                vec![0],
                vec![0],
                Box::new(Lookup),
                joiner,
                algorithm == 4,
                1
            )
            .unwrap()
        ),
        6 => bound!(
            IndexLookUpMergeJoin::new(
                outer_rows(),
                vec![0],
                vec![0],
                Box::new(Lookup),
                joiner,
                1,
                4
            )
            .unwrap()
        ),
        7 => {
            // Sorted merge inputs put NULL first, as in SQL ascending order.
            let mut rows = outer_rows();
            rows.rotate_right(1);
            bound!(
                MergeJoinExec::new(
                    MergeJoinTable::new(rows, vec![0], false).unwrap(),
                    MergeJoinTable::new(inner_rows(), vec![0], true).unwrap(),
                    joiner
                )
                .unwrap()
            )
        }
        _ => unreachable!(),
    }
}

#[test]
fn joined_rows_all_executors_residual_reopen_eof_and_no_collector() {
    for algorithm in 0..8 {
        for kind in [
            JoinType::Inner,
            JoinType::LeftOuter,
            JoinType::RightOuter,
            JoinType::Semi,
            JoinType::AntiSemi,
            JoinType::LeftOuterSemi,
            JoinType::AntiLeftOuterSemi,
        ] {
            let coll = Arc::new(Mutex::new(NewRuntimeStatsColl(None)));
            let conditions: Vec<Predicate> = vec![Arc::new(|_| Ok(Some(false)))];
            let mut enabled = executor(algorithm, kind, conditions.clone(), Some(coll.clone()));
            let mut disabled = executor(algorithm, kind, conditions, None);
            for expected in [6, 12] {
                enabled.open();
                disabled.open();
                assert_eq!(enabled.drain().unwrap(), disabled.drain().unwrap());
                assert_eq!(enabled.drain().unwrap(), 0);
                enabled.close();
                disabled.close();
                enabled.close();
                assert_eq!(
                    coll.lock().unwrap().GetRootJoinedRows(7),
                    Some(expected),
                    "algorithm={algorithm} kind={kind:?}"
                );
            }
        }
    }
}

#[test]
fn joined_rows_all_executors_publish_residual_error_and_panic() {
    for algorithm in 0..8 {
        for panic in [false, true] {
            let coll = Arc::new(Mutex::new(NewRuntimeStatsColl(None)));
            let mut e = executor(
                algorithm,
                JoinType::Inner,
                vec![Arc::new(move |_| {
                    if panic {
                        panic!("joined rows residual panic");
                    }
                    Err("joined rows residual error".into())
                })],
                Some(coll.clone()),
            );
            e.open();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| e.drain()));
            if panic && algorithm != 0 && algorithm != 1 && algorithm != 2 {
                assert!(result.is_err());
            } else if let Ok(result) = result {
                assert!(result.is_err());
            }
            e.close();
            assert_eq!(
                coll.lock().unwrap().GetRootJoinedRows(7),
                Some(1),
                "algorithm={algorithm} panic={panic}"
            );
        }
    }
}

#[test]
fn joined_rows_hash_v2_build_sides_spill_and_short_circuit() {
    for right in [false, true] {
        for spill in [None, Some(1)] {
            for kind in [
                JoinType::Inner,
                JoinType::LeftOuter,
                JoinType::RightOuter,
                JoinType::Semi,
                JoinType::AntiSemi,
                JoinType::LeftOuterSemi,
                JoinType::AntiLeftOuterSemi,
            ] {
                let coll = Arc::new(Mutex::new(NewRuntimeStatsColl(None)));
                let ctx =
                    HashJoinCtxV2::new(kind, vec![0], vec![0], right, false, 2, 2, spill).unwrap();
                let j =
                    Joiner::new(kind, !right, vec![Value::Null], vec![], None, false, 2).unwrap();
                let mut e = HashJoinV2Exec::new(ctx, j, vec![inner_rows()], vec![outer_rows()])
                    .unwrap()
                    .with_runtime_stats(7, coll.clone());
                e.execute_all().unwrap();
                e.close();
                let semi = matches!(
                    kind,
                    JoinType::Semi
                        | JoinType::AntiSemi
                        | JoinType::LeftOuterSemi
                        | JoinType::AntiLeftOuterSemi
                );
                assert_eq!(
                    coll.lock().unwrap().GetRootJoinedRows(7),
                    Some(if semi { 2 } else { 6 }),
                    "kind={kind:?} right={right} spill={spill:?}"
                );
            }
        }
    }
}

#[test]
fn joined_rows_probe_workers_restore_reset_and_error() {
    use crate::base_join_probe::{HashJoinContext, new_join_probe};
    for kind in [
        JoinType::Inner,
        JoinType::LeftOuter,
        JoinType::RightOuter,
        JoinType::Semi,
        JoinType::AntiSemi,
        JoinType::LeftOuterSemi,
        JoinType::AntiLeftOuterSemi,
    ] {
        for right in [false, true] {
            if !right && matches!(kind, JoinType::LeftOuterSemi | JoinType::AntiLeftOuterSemi) {
                continue;
            }
            for error in [false, true] {
                let coll = Arc::new(Mutex::new(NewRuntimeStatsColl(None)));
                let mut j = Joiner::new(
                    kind,
                    !right,
                    vec![Value::Null],
                    vec![Arc::new(move |_| {
                        if error {
                            Err("probe residual".into())
                        } else {
                            Ok(Some(false))
                        }
                    })],
                    Some([vec![], vec![]]),
                    false,
                    2,
                )
                .unwrap()
                .with_runtime_stats(7, coll.clone());
                j.reset_joined_rows();
                let ctx = HashJoinContext::new(inner_rows(), vec![0], vec![0], j, right, true, 2);
                let mut probe = new_join_probe(ctx, 0, kind, right, false).unwrap();
                for restored in [false, true] {
                    if restored {
                        probe.set_restored_chunk_for_probe(vec![row(1)]).unwrap();
                    } else {
                        probe.set_chunk_for_probe(vec![row(1)]).unwrap();
                    }
                    while !probe.is_current_chunk_probe_done() {
                        let result = probe.probe();
                        if error {
                            assert_eq!(result.error.as_deref(), Some("probe residual"));
                            break;
                        }
                        assert!(result.error.is_none());
                    }
                    if error {
                        break;
                    }
                    probe.reset_probe();
                }
                drop(probe);
                assert_eq!(
                    coll.lock().unwrap().GetRootJoinedRows(7),
                    Some(if error { 1 } else { 6 }),
                    "kind={kind:?} right={right} error={error}"
                );
            }
        }
    }
}

#[test]
fn joined_rows_hash_cancel_keeps_already_consumed_candidates() {
    use crate::hash_join_base::HashJoinContextBase;
    for v2 in [false, true] {
        let coll = Arc::new(Mutex::new(NewRuntimeStatsColl(None)));
        let base = HashJoinContextBase::default();
        let cancel = base.clone();
        let j = Joiner::new(
            JoinType::Inner,
            false,
            vec![],
            vec![Arc::new(move |_| {
                cancel.cancel();
                Err("query interrupted".into())
            })],
            None,
            false,
            2,
        )
        .unwrap();
        let mut e: Box<dyn Lifecycle> = if v2 {
            let mut ctx =
                HashJoinCtxV2::new(JoinType::Inner, vec![0], vec![0], true, false, 2, 2, None)
                    .unwrap();
            ctx.base = base;
            Box::new(
                HashJoinV2Exec::new(ctx, j, vec![inner_rows()], vec![outer_rows()])
                    .unwrap()
                    .with_runtime_stats(7, coll.clone()),
            )
        } else {
            let ctx = HashJoinCtxV1 {
                base,
                join_type: JoinType::Inner,
                build_key_indices: vec![0],
                probe_key_indices: vec![0],
                null_aware: false,
                build_side_is_outer: false,
                concurrency: 2,
                max_chunk_size: 2,
            };
            Box::new(
                HashJoinV1Exec::new(ctx, j, vec![inner_rows()], vec![outer_rows()])
                    .unwrap()
                    .with_runtime_stats(7, coll.clone()),
            )
        };
        e.open();
        assert_eq!(e.drain().unwrap_err(), "query interrupted");
        e.close();
        assert_eq!(coll.lock().unwrap().GetRootJoinedRows(7), Some(1));
    }
}
