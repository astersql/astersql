// Copyright 2026 AsterSQL.
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

use expression_dependency::{Column, ExprBox, NewSchema};

fn column(id: i64, name: &str) -> Column {
    let mut column = Column::default();
    column.UniqueID = id;
    column.OrigName = name.to_owned();
    column
}

#[test]
fn index_join_reorder_projection_becomes_removable_after_pruning() {
    let first = column(1, "t.a");
    let second = column(2, "t.b");
    let schema = NewSchema(vec![first.Clone(), second.Clone()]);
    let identity: Vec<ExprBox> = vec![Box::new(first.Clone()), Box::new(second.Clone())];
    let reordered: Vec<ExprBox> = vec![Box::new(second), Box::new(first)];

    assert!(super::optimizer_runtime::strict_identity_projection(
        &schema, &identity
    ));
    assert!(!super::optimizer_runtime::strict_identity_projection(
        &schema, &reordered
    ));
}

#[test]
fn index_join_outer_broadcast_makes_probe_hash_exchange_redundant() {
    assert!(super::optimizer_runtime::broadcast_join_probe_exchange_is_redundant(true, true,));
    assert!(!super::optimizer_runtime::broadcast_join_probe_exchange_is_redundant(false, true,));
}
