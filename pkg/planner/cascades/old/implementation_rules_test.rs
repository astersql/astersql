// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use astersql_planner_core_operator_logicalop::{JoinType, LogicalJoin};
use astersql_planner_memo::NewGroupExpr;
use astersql_planner_property::PhysicalProperty;

use crate::{ImplHashJoinBuildRight, ImplementationRule};

// Go ImplHashJoinBuildRight.OnImplement falls through to nil for join types the
// legacy rule cannot implement. FullOuterJoin must likewise produce no physical
// candidate instead of being treated as either one-sided outer join.
#[test]
fn build_right_rejects_full_outer_join() {
    let mut join = LogicalJoin::default();
    join.JoinType = JoinType::FullOuterJoin;
    let expression = NewGroupExpr(Box::new(join));

    let implementations = ImplHashJoinBuildRight
        .OnImplement(&expression.borrow(), &PhysicalProperty::default())
        .expect("FullOuterJoin rejection is not an implementation error");

    assert!(implementations.is_empty());
}
