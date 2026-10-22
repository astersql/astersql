// Copyright 2026 AsterSQL.

use super::*;

struct ParallelPlan(Leaf);
impl PlanData for ParallelPlan {}
impl UnaryPlanData for ParallelPlan {
    fn child_plan(&self) -> Plan<'_> {
        Plan::TableDual(&self.0)
    }
}
impl ParallelUnaryPlanData for ParallelPlan {
    fn configured_workers(&self) -> i64 {
        8
    }
    fn estimated_rows(&self) -> i64 {
        10000
    }
    fn max_chunk_size(&self) -> i64 {
        1024
    }
}
#[test]
fn mview_complete_delta_apply_fast_refresh_disables_parallel_workers() {
    let mut b = builder(dependencies());
    let p = ParallelPlan(Leaf);
    assert_eq!(b.parallel_unary_workers(&p), 8);
    b.inMViewDeltaMergeStmt = true;
    assert_eq!(b.parallel_unary_workers(&p), 0);
}

pub(super) fn target_columns(
    fields: &[FieldType],
) -> crate::mview_complete_delta_apply_builder::TargetColumns {
    use crate::mview_complete_delta_apply_builder::{TargetColumn, TargetColumns};
    TargetColumns {
        public_count: fields.len(),
        writable: fields
            .iter()
            .enumerate()
            .map(|(offset, field_type)| {
                Some(TargetColumn {
                    name: format!("c{offset}"),
                    offset: offset as isize,
                    public: true,
                    field_type: field_type.clone(),
                })
            })
            .collect(),
    }
}
fn complete_plan() -> astersql_planner_core::MViewCompleteDeltaApply {
    astersql_planner_core::MViewCompleteDeltaApply {
        source: PlanNode::New(1, PlanKind::TableReader, vec![]),
        mv_table_id: 10,
        mv_column_count: 3,
        op_col_offset: 0,
        marker_mv_offset: 1,
        group_key_mv_offsets: vec![0],
        current_handle_cols: vec![2],
        current_row_input_col_ids: vec![1, 2, 3],
        recomputed_row_input_col_ids: vec![4, 5, 6],
    }
}
struct CompletePlan {
    data: astersql_planner_core::MViewCompleteDeltaApply,
    leaf: Leaf,
}
impl PlanData for CompletePlan {}
impl UnaryPlanData for CompletePlan {
    fn child_plan(&self) -> Plan<'_> {
        Plan::TableDual(&self.leaf)
    }
}
impl MViewCompleteDeltaApplyPlanData for CompletePlan {
    fn target_table_id(&self) -> i64 {
        self.data.mv_table_id
    }
    fn apply_plan(&self) -> &astersql_planner_core::MViewCompleteDeltaApply {
        &self.data
    }
}
#[test]
fn mview_complete_delta_apply_mapping_validation() {
    use crate::mview_complete_delta_apply_builder::mappings;
    let p = complete_plan();
    let f = fields(7);
    let t = target_columns(&fields(3));
    let m = mappings(&p, &f, &t).unwrap();
    assert_eq!(m.current, vec![1, 2, 3]);
    assert_eq!(m.recomputed, vec![4, 5, 6]);
    assert_eq!(m.compare, vec![1, 2]);
    let mut invalid = vec![];
    let mut v = p.clone();
    v.op_col_offset = 7;
    invalid.push((v, "op column"));
    let mut v = p.clone();
    v.current_handle_cols.clear();
    invalid.push((v, "handle"));
    let mut v = p.clone();
    v.current_handle_cols = vec![7];
    invalid.push((v, "handle"));
    let mut v = p.clone();
    v.mv_column_count = 4;
    invalid.push((v, "public column count"));
    let mut v = p.clone();
    v.current_row_input_col_ids.pop();
    invalid.push((v, "mapping length"));
    let mut v = p.clone();
    v.recomputed_row_input_col_ids.pop();
    invalid.push((v, "mapping length"));
    let mut v = p.clone();
    v.current_row_input_col_ids[0] = 7;
    invalid.push((v, "out of source"));
    let mut v = p.clone();
    v.recomputed_row_input_col_ids[0] = 7;
    invalid.push((v, "out of source"));
    let mut v = p.clone();
    v.group_key_mv_offsets.clear();
    invalid.push((v, "group key"));
    let mut v = p.clone();
    v.group_key_mv_offsets = vec![3];
    invalid.push((v, "group key"));
    for (v, expected) in invalid {
        assert!(
            mappings(&v, &f, &t)
                .unwrap_err()
                .to_string()
                .contains(expected),
            "{v:?}"
        );
    }
    let mut f2 = f.clone();
    f2[0] = NewFieldType(TypeVarchar);
    assert!(
        mappings(&p, &f2, &t)
            .unwrap_err()
            .to_string()
            .contains("op column")
    );
    for i in [1, 4] {
        let mut f2 = f.clone();
        f2[i] = NewFieldType(TypeVarchar);
        assert!(
            mappings(&p, &f2, &t)
                .unwrap_err()
                .to_string()
                .contains("type mismatch")
        );
    }
    let mut t2 = t.clone();
    t2.writable.clear();
    assert!(
        mappings(&p, &f, &t2)
            .unwrap_err()
            .to_string()
            .contains("no writable")
    );
    let mut t2 = t.clone();
    t2.writable.push(t.writable[0].clone());
    assert!(
        mappings(&p, &f, &t2)
            .unwrap_err()
            .to_string()
            .contains("non-public")
    );
    let mut t2 = t.clone();
    t2.writable[1].as_mut().unwrap().public = false;
    assert!(
        mappings(&p, &f, &t2)
            .unwrap_err()
            .to_string()
            .contains("non-public")
    );
    let mut t2 = t.clone();
    t2.writable[1] = None;
    assert!(
        mappings(&p, &f, &t2)
            .unwrap_err()
            .to_string()
            .contains("is nil")
    );
    let mut t2 = t.clone();
    t2.writable[1].as_mut().unwrap().offset = -1;
    assert!(
        mappings(&p, &f, &t2)
            .unwrap_err()
            .to_string()
            .contains("out of range")
    );
    let mut t2 = t.clone();
    t2.writable[1].as_mut().unwrap().offset = 3;
    assert!(
        mappings(&p, &f, &t2)
            .unwrap_err()
            .to_string()
            .contains("out of range")
    );
    let mut t2 = t.clone();
    t2.writable.swap(0, 2);
    let m = mappings(&p, &f, &t2).unwrap();
    assert_eq!(m.current, vec![3, 2, 1]);
    assert_eq!(m.recomputed, vec![6, 5, 4]);
    assert_eq!(m.compare, vec![0, 1]);
}
#[test]
fn mview_complete_delta_apply_production_mutations_preserve_keys_and_handles() {
    use crate::mview_complete_delta_apply_builder::build as build_complete;
    let p = complete_plan();
    let rt = Arc::new(Resource::new(
        vec![row(&[1, 2, 10]), row(&[2, 1, 20])],
        RecomputeStrategy::Batch,
    ));
    let child = Source::new(
        vec![
            row(&[3, 1, 2, 10, 999, 3, 15]), // group key remains old even if recomputed differs
            row(&[2, 2, 1, 20, 0, 0, 0]),
            row(&[1, 0, 0, 0, 3, 1, 30]),
            row(&[3, 1, 3, 15, 888, 3, 15]), // key-only difference is not an update
        ],
        fields(7),
    );
    let mut exec = build_complete(&p, Box::new(child), rt.clone()).unwrap();
    assert!(exec.IsWriteExecutor());
    assert!(exec.CalculateNoDelay());
    exec.Open().unwrap();
    let mut out = exec.NewChunk();
    exec.Next(&mut out).unwrap();
    assert_eq!(out.NumRows(), 0);
    exec.Next(&mut out).unwrap();
    exec.Close().unwrap();
    let w = rt.writes.lock().unwrap();
    assert_eq!(w.rows.len(), 2);
    assert!(same_row(&w.rows[0], &row(&[1, 3, 15])));
    assert!(same_row(&w.rows[1], &row(&[3, 1, 30])));
    assert_eq!(w.touched, vec![vec![false, true, true]]);
    assert_eq!(w.sources.len(), 2);
    assert!(same_row(&w.sources[0], &row(&[3, 1, 2, 10, 999, 3, 15])));
}
#[test]
fn mview_complete_delta_apply_invalid_op_rolls_back_chunk() {
    use crate::mview_complete_delta_apply_builder::build as build_complete;
    for op in [0, 4] {
        let rt = Arc::new(Resource::new(vec![], RecomputeStrategy::Batch));
        let mut exec = build_complete(
            &complete_plan(),
            Box::new(Source::new(
                vec![row(&[1, 0, 0, 0, 1, 1, 10]), row(&[op, 0, 0, 0, 2, 1, 20])],
                fields(7),
            )),
            rt.clone(),
        )
        .unwrap();
        exec.Open().unwrap();
        let mut out = exec.NewChunk();
        assert!(exec.Next(&mut out).is_err());
        assert!(rt.writes.lock().unwrap().rows.is_empty());
        exec.Close().unwrap();
    }
}
#[test]
fn mview_complete_delta_apply_dispatch_snapshot_and_state_restore() {
    let mut deps = dependencies();
    Arc::get_mut(&mut deps).unwrap().complete = true;
    let mut b = builder(deps.clone());
    let p = CompletePlan {
        data: complete_plan(),
        leaf: Leaf,
    };
    let mut exec = b.build(Some(Plan::MViewCompleteDeltaApply(&p))).unwrap();
    assert_eq!(*deps.calls.lock().unwrap(), vec!["update"]);
    assert!(!b.inMViewCompleteDeltaApplyStmt);
    assert_eq!(b.getSnapshotTS().unwrap(), 100);
    exec.Open().unwrap();
    let mut out = exec.NewChunk();
    exec.Next(&mut out).unwrap();
    exec.Close().unwrap();
    assert!(
        deps.rt
            .writes
            .lock()
            .unwrap()
            .rows
            .iter()
            .any(|r| same_row(r, &row(&[2, 3, 4])))
    );
    b.inMViewCompleteDeltaApplyStmt = true;
    assert_eq!(b.getSnapshotTS().unwrap(), 200);
    b.getSnapshot().unwrap();
    assert_eq!(deps.calls.lock().unwrap().last().unwrap(), "for-update");
    assert_eq!(b.parallel_unary_workers(&ParallelPlan(Leaf)), 0);
    assert!(b.build(Some(Plan::MViewCompleteDeltaApply(&p))).is_some());
    assert!(b.inMViewCompleteDeltaApplyStmt);
    b.forDataReaderBuilder = true;
    b.dataReaderTS = 400;
    assert_eq!(b.getSnapshotTS().unwrap(), 400);
    let mut deps = dependencies();
    Arc::get_mut(&mut deps).unwrap().fail_child = true;
    let mut b = builder(deps);
    assert!(b.build(Some(Plan::MViewCompleteDeltaApply(&p))).is_none());
    assert!(!b.inMViewCompleteDeltaApplyStmt);
    assert_eq!(b.getSnapshotTS().unwrap(), 100);
}

struct CompleteParallelPlan {
    data: astersql_planner_core::MViewCompleteDeltaApply,
    parallel: ParallelPlan,
    expand: bool,
}
impl PlanData for CompleteParallelPlan {}
impl UnaryPlanData for CompleteParallelPlan {
    fn child_plan(&self) -> Plan<'_> {
        if self.expand {
            Plan::Expand(&self.parallel)
        } else {
            Plan::Projection(&self.parallel)
        }
    }
}
impl MViewCompleteDeltaApplyPlanData for CompleteParallelPlan {
    fn target_table_id(&self) -> i64 {
        self.data.mv_table_id
    }
    fn apply_plan(&self) -> &astersql_planner_core::MViewCompleteDeltaApply {
        &self.data
    }
}
#[test]
fn mview_complete_delta_apply_nested_projection_expand_and_failure_restore() {
    for expand in [false, true] {
        let mut deps = dependencies();
        Arc::get_mut(&mut deps).unwrap().complete = true;
        let mut b = builder(deps.clone());
        let p = CompleteParallelPlan {
            data: complete_plan(),
            parallel: ParallelPlan(Leaf),
            expand,
        };
        assert!(b.build(Some(Plan::MViewCompleteDeltaApply(&p))).is_some());
        let kind = if expand { "Expand" } else { "Projection" };
        assert!(deps.calls.lock().unwrap().contains(&format!("{kind}:0")));
        let ordinary = if expand {
            Plan::Expand(&p.parallel)
        } else {
            Plan::Projection(&p.parallel)
        };
        assert!(b.build(Some(ordinary)).is_some());
        assert!(deps.calls.lock().unwrap().contains(&format!("{kind}:8")));
    }
    for fail_update_ts in [false, true] {
        let mut deps = dependencies();
        let d = Arc::get_mut(&mut deps).unwrap();
        d.complete = true;
        d.fail_update_ts = fail_update_ts;
        d.missing_table = !fail_update_ts;
        let mut b = builder(deps);
        let p = CompletePlan {
            data: complete_plan(),
            leaf: Leaf,
        };
        assert!(b.build(Some(Plan::MViewCompleteDeltaApply(&p))).is_none());
        assert!(b.error().unwrap().to_string().contains(if fail_update_ts {
            "for-update TS"
        } else {
            "not found in infoschema"
        }));
        assert!(!b.inMViewCompleteDeltaApplyStmt);
        assert_eq!(b.getSnapshotTS().unwrap(), 100);
    }
    let mut deps = dependencies();
    Arc::get_mut(&mut deps).unwrap().complete = true;
    let mut b = builder(deps);
    let mut p = CompletePlan {
        data: complete_plan(),
        leaf: Leaf,
    };
    p.data.op_col_offset = 99;
    assert!(b.build(Some(Plan::MViewCompleteDeltaApply(&p))).is_none());
    assert!(b.error().unwrap().to_string().contains("op column"));
    assert!(!b.inMViewCompleteDeltaApplyStmt);
}
#[test]
fn mview_complete_delta_apply_field_types_match_go_nullable_and_unsigned_rules() {
    use crate::mview_complete_delta_apply_builder::mappings;
    use astersql_parser_mysql::r#type::{NotNullFlag, UnsignedFlag};
    let p = complete_plan();
    let f = fields(7);
    let mut t = target_columns(&fields(3));
    t.writable[0]
        .as_mut()
        .unwrap()
        .field_type
        .AddFlag(NotNullFlag);
    assert!(mappings(&p, &f, &t).is_ok());
    let mut f = f;
    f[1].AddFlag(UnsignedFlag);
    assert!(
        mappings(&p, &f, &t)
            .unwrap_err()
            .to_string()
            .contains("type mismatch")
    );
}
#[test]
fn mview_complete_delta_apply_context_null_op_and_write_failure() {
    use crate::mview_complete_delta_apply_builder::build as build_complete;
    for null_op in [false, true] {
        let mut rt = Resource::new(vec![], RecomputeStrategy::Batch);
        rt.fail = !null_op;
        let rt = Arc::new(rt);
        let mut r = row(&[1, 0, 0, 0, 1, 2, 10]);
        if null_op {
            r[0] = Datum::default();
        }
        let mut source = Source::new(vec![r], fields(7));
        source.require_context = true;
        let mut exec = build_complete(&complete_plan(), Box::new(source), rt.clone()).unwrap();
        exec.Open().unwrap();
        let mut out = exec.NewChunk();
        let context = ExecutionContext::default();
        let err = exec
            .NextWithContext(&context, &mut out)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(if null_op { "NULL" } else { "write failure" }),
            "{err}"
        );
        assert!(rt.writes.lock().unwrap().rows.is_empty());
        exec.Close().unwrap();
    }
}
