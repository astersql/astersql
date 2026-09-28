// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

use crate::{GenHintsFromPhysicalPlan, JoinType, PlanKind, PlanNode, StoreType};

fn table(id: i32, name: &str) -> PlanNode {
    PlanNode::New(
        id,
        PlanKind::TableScan {
            table: name.to_owned(),
        },
        vec![],
    )
}

fn hash_join(id: i32, children: Vec<PlanNode>, has_equal_condition: bool) -> PlanNode {
    PlanNode::New(
        id,
        PlanKind::HashJoin {
            inner_child: 1,
            equal_conditions: has_equal_condition
                .then(|| vec![("a".to_owned(), "b".to_owned())])
                .unwrap_or_default(),
        },
        children,
    )
}

#[test]
fn index_join_variants_keep_their_go_hint_names() {
    let cases = [
        (PlanKind::IndexJoin { keys: vec![] }, "INL_JOIN"),
        (PlanKind::IndexMergeJoin { keys: vec![] }, "INL_MERGE_JOIN"),
        (PlanKind::IndexHashJoin { keys: vec![] }, "INL_HASH_JOIN"),
    ];
    for (kind, expected) in cases {
        let plan = PlanNode::New(10, kind, vec![table(1, "outer"), table(2, "inner")]);
        let hints = GenHintsFromPhysicalPlan(&plan);
        assert!(hints.iter().any(|hint| hint.name == expected), "{hints:?}");
    }
}

#[test]
fn two_table_and_cartesian_joins_do_not_emit_leading() {
    for plan in [
        hash_join(3, vec![table(1, "a"), table(2, "b")], true),
        hash_join(
            6,
            vec![
                hash_join(4, vec![table(1, "a"), table(2, "b")], true),
                table(5, "c"),
            ],
            false,
        ),
    ] {
        let hints = GenHintsFromPhysicalPlan(&plan);
        assert!(
            !hints.iter().any(|hint| hint.name == "LEADING"),
            "{hints:?}"
        );
    }
}

#[test]
fn bushy_and_unsupported_join_groups_do_not_emit_leading() {
    let bushy = hash_join(
        7,
        vec![
            hash_join(3, vec![table(1, "a"), table(2, "b")], true),
            hash_join(6, vec![table(4, "c"), table(5, "d")], true),
        ],
        true,
    );
    let unsupported = PlanNode::New(
        10,
        PlanKind::MergeJoin {
            join_type: JoinType::SemiJoin,
            keys: vec![("a".to_owned(), "b".to_owned())],
        },
        vec![
            hash_join(8, vec![table(1, "a"), table(2, "b")], true),
            table(9, "c"),
        ],
    );
    for plan in [bushy, unsupported] {
        let hints = GenHintsFromPhysicalPlan(&plan);
        assert!(
            !hints.iter().any(|hint| hint.name == "LEADING"),
            "{hints:?}"
        );
    }
}

#[test]
fn algorithm_hints_do_not_carry_storage_engine_data() {
    let mut plan = hash_join(3, vec![table(1, "a"), table(2, "b")], true);
    plan.store_type = StoreType::TiKV;
    let hints = GenHintsFromPhysicalPlan(&plan);
    let hash_hint = hints
        .iter()
        .find(|hint| hint.name == "HASH_JOIN")
        .expect("hash join hint");
    assert_eq!(hash_hint.store, None);
}
