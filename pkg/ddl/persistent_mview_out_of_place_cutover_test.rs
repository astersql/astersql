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

use crate::persistent_mview_out_of_place_cutover::{
    replace_materialized_view_id, rewrite_materialized_view_base,
};

#[test]
fn cutover_replaces_ids_and_removes_duplicates_like_go() {
    assert_eq!(
        replace_materialized_view_id(&[1, 2, 1, 3, 9], 1, 9),
        (vec![9, 2, 3], true)
    );
    assert_eq!(
        replace_materialized_view_id(&[2, 2, 3], 1, 9),
        (vec![2, 3], false)
    );
}

#[test]
fn cutover_rewrites_base_metadata_and_rejects_missing_old_id() {
    let mut table = astersql_meta_model::TableInfo {
        MaterializedViewBase: Some(astersql_meta_model::MaterializedViewBaseInfo {
            MLogID: 7,
            MViewIDs: vec![10, 20, 30, 20],
        }),
        ..Default::default()
    };
    rewrite_materialized_view_base(&mut table, 20, 30).unwrap();
    assert_eq!(table.MaterializedViewBase.unwrap().MViewIDs, vec![10, 30]);

    let mut missing = astersql_meta_model::TableInfo {
        MaterializedViewBase: Some(Default::default()),
        ..Default::default()
    };
    assert!(
        rewrite_materialized_view_base(&mut missing, 20, 30)
            .unwrap_err()
            .contains("old materialized view id is missing")
    );
}

#[test]
fn cutover_event_keeps_new_and_old_complete_metadata() {
    let new = astersql_meta_model::TableInfo {
        ID: 22,
        ..Default::default()
    };
    let old = astersql_meta_model::TableInfo {
        ID: 11,
        ..Default::default()
    };
    let event = astersql_ddl_notifier::NewMViewRefreshOutOfPlaceCutoverEvent(
        Some(Box::new(new)),
        Some(Box::new(old)),
    );
    let (new, old) = event.GetMViewRefreshOutOfPlaceCutoverInfo();
    assert_eq!(new.unwrap().ID, 22);
    assert_eq!(old.unwrap().ID, 11);
}

#[test]
fn normal_dispatch_accepts_cutover_action() {
    assert!(crate::persistent_actions::handler_available(
        astersql_meta_model::group_3::ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER
    ));
    assert!(crate::persistent_actions::handler_available(
        astersql_meta_model::group_3::ACTION_DROP_MATERIALIZED_VIEW_SHADOW
    ));
}
