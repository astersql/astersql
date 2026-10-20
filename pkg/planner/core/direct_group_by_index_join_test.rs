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
// Copyright 2026 AsterSQL.

use super::exhaust_physical_plans::{IndexJoinRuntimeProp, checkIndexJoinInnerTaskWithAgg};
use super::task::Expression;

#[test]
fn direct_group_by_index_join() {
    let runtime = IndexJoinRuntimeProp {
        inner_join_keys: vec![2],
        ..Default::default()
    };
    let direct_column = Expression {
        column: Some(2),
        ..Default::default()
    };
    let expression_derived_column = Expression {
        name: "mod".into(),
        column: Some(2),
        function_count: 1,
        ..Default::default()
    };

    assert!(checkIndexJoinInnerTaskWithAgg(
        &[direct_column],
        &[2],
        &runtime,
    ));
    assert!(!checkIndexJoinInnerTaskWithAgg(
        &[expression_derived_column],
        &[2],
        &runtime,
    ));
}
