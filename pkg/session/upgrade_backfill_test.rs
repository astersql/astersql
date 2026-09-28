// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use std::collections::BTreeMap;

use crate::SessionResult;
use crate::upgrade_run::{
    BindingDigestRefreshAction, BindingDigestRefreshRow, BootstrapVariableUpgradeRuntime,
    plan_binding_digest_refresh, upgrade_bootstrap_variables,
};

#[derive(Default)]
struct VariableStore(BTreeMap<String, String>);

impl BootstrapVariableUpgradeRuntime for VariableStore {
    type Error = crate::SessionError;

    fn insert_global_if_missing(&mut self, name: &str, value: &str) -> SessionResult<()> {
        self.0
            .entry(name.to_owned())
            .or_insert_with(|| value.to_owned());
        Ok(())
    }

    fn delete_global_if_equal(&mut self, name: &str, value: &str) -> SessionResult<()> {
        if self.0.get(name).is_some_and(|current| current == value) {
            self.0.remove(name);
        }
        Ok(())
    }

    fn update_global_if_equal(&mut self, name: &str, old: &str, new: &str) -> SessionResult<()> {
        if self.0.get(name).is_some_and(|current| current == old) {
            self.0.insert(name.to_owned(), new.to_owned());
        }
        Ok(())
    }

    fn upsert_tidb_variable(&mut self, name: &str, value: &str, _: &str) -> SessionResult<()> {
        self.0.insert(name.to_owned(), value.to_owned());
        Ok(())
    }
}

#[test]
fn upgrade_to_ver259_backfills_ignore_inlist_plan_digest() {
    let mut store = VariableStore::default();
    upgrade_bootstrap_variables(&mut store, crate::upgrade_def::version258)
        .expect("upgrade variables from v258");
    assert_eq!(store.0["tidb_ignore_inlist_plan_digest"], "OFF");

    store
        .0
        .insert("tidb_ignore_inlist_plan_digest".into(), "ON".into());
    upgrade_bootstrap_variables(&mut store, crate::upgrade_def::version258)
        .expect("repeat v259 backfill");
    assert_eq!(store.0["tidb_ignore_inlist_plan_digest"], "ON");
}

#[test]
fn upgrade_to_ver261_backfills_historical_string_match_selectivity() {
    let mut store = VariableStore::default();
    upgrade_bootstrap_variables(&mut store, crate::upgrade_def::version260)
        .expect("upgrade variables from v260");
    assert_eq!(store.0["tidb_default_string_match_selectivity"], "0.8");

    store
        .0
        .insert("tidb_default_string_match_selectivity".into(), "0.6".into());
    upgrade_bootstrap_variables(&mut store, crate::upgrade_def::version260)
        .expect("repeat v261 backfill");
    assert_eq!(store.0["tidb_default_string_match_selectivity"], "0.6");
}

fn binding(identity: &str, bind_sql: &str, plan_digest: Option<&str>) -> BindingDigestRefreshRow {
    BindingDigestRefreshRow {
        identity: identity.to_owned(),
        bind_sql: bind_sql.to_owned(),
        default_db: "test".to_owned(),
        source: "manual".to_owned(),
        plan_digest: plan_digest.map(str::to_owned),
    }
}

#[test]
fn upgrade_to_ver262_refreshes_digests_and_resolves_duplicates_newest_first() {
    let winner = binding(
        "winner",
        "select /*+ use_index(t_issue, idx_issue_b) */ * from t_issue where ((a = 1) and (b = 1))",
        Some("same-plan"),
    );
    let older_one = binding(
        "older-one",
        "select /*+ use_index(t_issue, idx_issue_b) */ * from t_issue where (((a = 1)) and (b = 1))",
        Some("same-plan"),
    );
    let older_two = binding(
        "older-two",
        "select /*+ use_index(t_issue, idx_issue_b) */ * from t_issue where ((a = 1) and ((b = 1)))",
        Some("same-plan"),
    );
    let invalid = binding("invalid", "invalid binding", Some("same-plan"));
    let builtin = BindingDigestRefreshRow {
        source: "builtin".to_owned(),
        ..binding(
            "builtin",
            "select * from t_issue where a = 1",
            Some("builtin-plan"),
        )
    };

    let actions = plan_binding_digest_refresh([winner, older_one, older_two, invalid, builtin]);
    assert_eq!(actions.len(), 4);
    assert_eq!(
        actions[0],
        BindingDigestRefreshAction::ClearInvalidPlanDigest {
            identity: "invalid".into()
        }
    );
    assert_eq!(
        &actions[1..3],
        &[
            BindingDigestRefreshAction::DeleteDuplicate {
                identity: "older-one".into()
            },
            BindingDigestRefreshAction::DeleteDuplicate {
                identity: "older-two".into()
            },
        ]
    );
    let BindingDigestRefreshAction::UpdateDigest {
        identity,
        original_sql,
        sql_digest,
    } = &actions[3]
    else {
        panic!("newest valid binding must be refreshed");
    };
    assert_eq!(identity, "winner");
    assert!(!original_sql.is_empty());
    assert!(!sql_digest.is_empty());
}

#[test]
fn upgrade_to_ver262_keeps_equal_sql_digests_when_plan_digests_differ() {
    let first = binding(
        "first",
        "select /*+ use_index(t, idx_a) */ * from t where a = 1",
        Some("plan-a"),
    );
    let second = binding(
        "second",
        "select /*+ use_index(t, idx_b) */ * from t where a = 2",
        Some("plan-b"),
    );
    let actions = plan_binding_digest_refresh([first, second]);
    assert_eq!(
        actions
            .iter()
            .filter(|action| matches!(action, BindingDigestRefreshAction::UpdateDigest { .. }))
            .count(),
        2
    );
    assert!(
        !actions
            .iter()
            .any(|action| matches!(action, BindingDigestRefreshAction::DeleteDuplicate { .. }))
    );
}

#[test]
fn canonical_mock_upgrade_retargets_only_supported_versions() {
    use crate::mock_bootstrap::{
        MockUpgradeFlags, RegisterMockUpgradeFlag, modifyBootstrapVersionForTest,
    };
    let mut flags = MockUpgradeFlags::default();
    RegisterMockUpgradeFlag(&mut flags, true);
    let mut current = 262;
    modifyBootstrapVersionForTest(259, 260, &mut current, 300);
    assert_eq!(current, 262);
    modifyBootstrapVersionForTest(260, 260, &mut current, 300);
    assert_eq!(current, 300);
    RegisterMockUpgradeFlag(&mut flags, false);
}
