// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 对应 Go `TestFastPointGetClone`：计划缓存命中后必须换绑会话上下文，
// 并深拷贝所有后续可变字段，避免两个会话共享缓存计划状态。

use astersql_planner_core::{
    AccessPath, FastClonePointGetForPlanCache, PlanKind, PlanNode, PlannerContext, PointGetPlan,
    SessionVars,
};

fn context(operator_num: u64, in_txn: bool) -> PlannerContext {
    PlannerContext {
        vars: SessionVars {
            in_txn,
            ..SessionVars::default()
        },
        operator_num: vec![operator_num],
    }
}

fn point_get_fixture() -> PointGetPlan {
    PointGetPlan {
        ctx: context(1, false),
        plan: Some(PlanNode::New(
            7,
            PlanKind::IndexScan {
                table: "orders".to_owned(),
                index: "idx_customer".to_owned(),
                ranges: vec!["[42,42]".to_owned()],
            },
            Vec::new(),
        )),
        access_path: Some(AccessPath {
            index_name: "idx_customer".to_owned(),
            ranges: vec!["[42,42]".to_owned()],
            count_after_access: 1,
            count_after_index: 1,
            partial_alternative_index_paths: vec![AccessPath {
                index_name: "PRIMARY".to_owned(),
                ranges: vec!["[42,42]".to_owned()],
                count_after_access: 1,
                count_after_index: 1,
                partial_alternative_index_paths: Vec::new(),
            }],
        }),
        partition_names: vec!["p0".to_owned()],
        handles: vec![42],
        index_values: vec![vec!["42".to_owned(), "customer-42".to_owned()]],
    }
}

#[test]
fn fast_point_get_clone_rebinds_context_and_preserves_every_field() {
    let source = point_get_fixture();
    let new_ctx = context(9, true);
    let mut destination = PointGetPlan::default();

    let returned = FastClonePointGetForPlanCache(new_ctx.clone(), &source, &mut destination);

    assert_eq!(destination.ctx, new_ctx);
    assert_eq!(destination.plan, source.plan);
    assert_eq!(destination.access_path, source.access_path);
    assert_eq!(destination.partition_names, source.partition_names);
    assert_eq!(destination.handles, source.handles);
    assert_eq!(destination.index_values, source.index_values);
    assert_eq!(returned, destination);
    assert_ne!(destination.ctx, source.ctx);
}

#[test]
fn fast_point_get_clone_does_not_share_mutable_state_with_source() {
    let source = point_get_fixture();
    let original = source.clone();
    let mut destination = PointGetPlan::default();

    FastClonePointGetForPlanCache(context(2, true), &source, &mut destination);
    destination.partition_names[0].push_str("_changed");
    destination.handles[0] = 99;
    destination.index_values[0][0].push_str("_changed");
    destination.access_path.as_mut().unwrap().ranges[0].push_str("_changed");
    destination
        .access_path
        .as_mut()
        .unwrap()
        .partial_alternative_index_paths[0]
        .index_name
        .push_str("_changed");
    let PlanKind::IndexScan { ranges, .. } = &mut destination.plan.as_mut().unwrap().kind else {
        panic!("expected index scan fixture");
    };
    ranges[0].push_str("_changed");

    assert_eq!(source, original);
}
