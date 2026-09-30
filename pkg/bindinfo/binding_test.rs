// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

use crate::*;
use std::cell::Cell;
use std::collections::HashMap;
use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn table(schema: &str, name: &str) -> TableName {
    TableName {
        Schema: schema.to_owned(),
        Name: name.to_owned(),
        ..Default::default()
    }
}

fn binding(id: &str, schema: &str, update_time: i64) -> Arc<Binding> {
    Arc::new(Binding {
        ID: id.to_owned(),
        Status: StatusEnabled.to_owned(),
        UpdateTime: BindingTime(update_time),
        TableNames: vec![table(schema, "t")],
        ..Default::default()
    })
}

#[test]
fn binding_size_matches_go_fields_and_fixed_time_storage() {
    let value = Binding {
        OriginalSQL: "original".to_owned(),
        Db: "db".to_owned(),
        BindSQL: "binding".to_owned(),
        Status: StatusEnabled.to_owned(),
        Source: "source-is-not-counted".to_owned(),
        Charset: "utf8mb4".to_owned(),
        Collation: "utf8mb4_bin".to_owned(),
        ID: "hint-id".to_owned(),
        SQLDigest: "digest-is-not-counted".to_owned(),
        PlanDigest: "plan-is-not-counted".to_owned(),
        TableNames: vec![table("schema-is-not-counted", "table-is-not-counted")],
        ..Default::default()
    };
    let expected = value.OriginalSQL.len()
        + value.Db.len()
        + value.BindSQL.len()
        + value.Status.len()
        + 2 * size_of::<BindingTime>()
        + value.Charset.len()
        + value.Collation.len()
        + value.ID.len();
    assert_eq!(value.size(), expected as f64);
}

#[test]
fn no_db_digest_is_derived_from_bind_sql() {
    let value = Binding {
        OriginalSQL: "select * from original_table".to_owned(),
        BindSQL: "select * from bound_table".to_owned(),
        ..Default::default()
    };
    let expected = NormalizeStmtForBinding(
        &Statement {
            SQL: value.BindSQL.clone(),
            ..Default::default()
        },
        "",
        true,
    )
    .1;
    assert_eq!(noDBDigestFromBinding(&value).unwrap(), expected);
}

#[test]
fn cross_db_matching_uses_star_and_preserves_first_tie() {
    let statement = [table("tenant", "t")];
    let first = binding("first", "*", 1);
    let newer = binding("newer", "*", 2);
    let (matched, hit) = crossDBMatchBindings("default", &statement, &[first.clone(), newer]);
    assert!(hit);
    assert!(Arc::ptr_eq(&matched.unwrap(), &first));
    assert!(!crossDBMatchBindingsWithFuzzy("default", &statement, &[first], false).1);

    let empty_schema = binding("empty", "", 3);
    assert!(!crossDBMatchBindings("default", &statement, &[empty_schema]).1);
    assert!(isCrossDBBinding(&Statement {
        SQL: "select * from *.t".to_owned(),
        Tables: vec![table("*", "t")],
        ..Default::default()
    }));
}

#[test]
fn cached_binding_filters_deleted_records_after_max_timestamp() {
    let cached = Arc::new(Binding {
        ID: "cached".to_owned(),
        Status: StatusEnabled.to_owned(),
        CreateTime: BindingTime(1),
        UpdateTime: BindingTime(10),
        ..Default::default()
    });
    let deleted = Arc::new(Binding {
        ID: "deleted".to_owned(),
        Status: StatusDeleted.to_owned(),
        CreateTime: BindingTime(2),
        UpdateTime: BindingTime(10),
        ..Default::default()
    });
    let selected = pickCachedBinding(Some(cached.clone()), [deleted]).unwrap();
    assert!(Arc::ptr_eq(&selected, &cached));
}

#[test]
fn cache_assertion_matches_go_nil_and_miss_contract() {
    assert!(assertMatchSQLBinding(None, false, None, ""));
    let stale = binding("stale", "test", 1);
    let miss = BindingCacheItem {
        Binding: Some(stale),
        Matched: false,
        Scope: "stale-scope".to_owned(),
    };
    assert!(assertMatchSQLBinding(Some(&miss), false, None, ""));
}

#[test]
fn semicolon_and_parameter_checks_follow_ast_semantics() {
    let mut repeated = Statement {
        SQL: "select 1;;".to_owned(),
        ..Default::default()
    };
    eraseLastSemicolon(&mut repeated);
    assert_eq!(repeated.SQL, "select 1;");

    let quoted_question = Statement {
        SQL: "select '?'".to_owned(),
        HasParamMarker: false,
        ..Default::default()
    };
    assert!(!hasParam(&quoted_question));
    assert!(hasParam(&Statement {
        SQL: "select ?".to_owned(),
        HasParamMarker: true,
        ..Default::default()
    }));

    let nested = Statement {
        SQL: "select * from t1 where a > (select max(a) from db2.t2 where b in (select b from t3))"
            .to_owned(),
        ..Default::default()
    };
    assert_eq!(
        CollectTableNames(&nested),
        vec![table("", "t1"), table("db2", "t2"), table("", "t3")]
    );
}

struct Validator {
    calls: AtomicUsize,
    fail: bool,
}

impl Validator {
    fn rejecting() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            fail: true,
        }
    }
}

impl BindingValidator for Validator {
    fn validate_binding_sql(&self, _sql: &str) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(BindError("validator rejected SQL".to_owned()))
        } else {
            Ok(())
        }
    }
}

#[test]
fn prepare_hints_preserves_go_shortcuts_and_populates_metadata() {
    let rejecting = Validator::rejecting();
    let mut deleted = Binding {
        Status: StatusDeleted.to_owned(),
        ..Default::default()
    };
    prepareHints(&rejecting, &mut deleted).unwrap();
    assert_eq!(rejecting.calls.load(Ordering::SeqCst), 0);

    let mut prepared = Binding {
        BindSQL: "invalid".to_owned(),
        ID: "prepared".to_owned(),
        Hint: HintSet {
            Hints: vec!["prepared".to_owned()],
        },
        ..Default::default()
    };
    prepareHints(&rejecting, &mut prepared).unwrap();
    assert_eq!(rejecting.calls.load(Ordering::SeqCst), 0);

    let mut cross_db = Binding {
        BindSQL: "select /*+ use_index(t, idx_t) */ * from *.t".to_owned(),
        Charset: "utf8mb4".to_owned(),
        Collation: "utf8mb4_bin".to_owned(),
        ..Default::default()
    };
    prepareHints(&rejecting, &mut cross_db).unwrap();
    assert_eq!(rejecting.calls.load(Ordering::SeqCst), 0);
    assert!(!cross_db.ID.is_empty());
    assert_eq!(cross_db.TableNames, vec![table("*", "t")]);
}

#[test]
fn normalization_uses_parser_binding_rules_and_skips_explain() {
    let in_list = Statement {
        SQL: "select 1 from b where (x,y) in ((1, 3), ('3', 1))".to_owned(),
        ..Default::default()
    };
    let (normalized, digest) = NormalizeStmtForBinding(&in_list, "", true);
    assert_eq!(
        normalized,
        "select ? from `b` where row ( `x` , `y` ) in ( ... )"
    );
    assert_eq!(
        digest,
        "ab6c607d118c24030807f8d1c7c846ec23e3b752fd88ed763bb8e26fbfa56a83"
    );

    let explained = Statement {
        SQL: "explain select * from test.t where a = 1".to_owned(),
        ..Default::default()
    };
    let direct = Statement {
        SQL: "select * from test.t where a = 1".to_owned(),
        ..Default::default()
    };
    assert_eq!(
        NormalizeStmtForBinding(&explained, "", true),
        NormalizeStmtForBinding(&direct, "", true)
    );
    assert_eq!(
        NormalizeStmtForBinding(
            &Statement {
                SQL: "create table t(a int)".to_owned(),
                ..Default::default()
            },
            "",
            true,
        ),
        (String::new(), String::new())
    );
}

struct MatchContext {
    cache: HashMap<String, BindingCacheItem>,
    session: Option<Arc<Binding>>,
    global: Option<Arc<Binding>>,
    global_calls: Cell<usize>,
    test_mode: bool,
}

impl BindingMatchContext for MatchContext {
    fn in_test_mode(&self) -> bool {
        self.test_mode
    }
    fn use_plan_baselines(&self) -> bool {
        true
    }

    fn current_db(&self) -> &str {
        "test"
    }

    fn cached_match(&self, statement_key: &str) -> Option<BindingCacheItem> {
        self.cache.get(statement_key).cloned()
    }

    fn cache_match(&mut self, statement_key: String, item: BindingCacheItem) {
        self.cache.insert(statement_key, item);
    }

    fn match_session_binding(
        &self,
        _no_db_digest: &str,
        _tables: &[TableName],
    ) -> Option<Arc<Binding>> {
        self.session.clone()
    }

    fn match_global_binding(
        &self,
        no_db_digest: &str,
        tables: &[TableName],
    ) -> Option<Arc<Binding>> {
        self.global_calls.set(self.global_calls.get() + 1);
        if no_db_digest.is_empty() || tables.is_empty() {
            return None;
        }
        self.global.clone().map(|candidate| {
            Arc::new(Binding {
                ID: tables[0].Name.clone(),
                ..(*candidate).clone()
            })
        })
    }
}

#[test]
fn matching_avoids_cache_aliases_recomputes_partial_info_and_session_usage() {
    let global = binding("global", "*", 1);
    let mut context = MatchContext {
        cache: HashMap::new(),
        session: None,
        global: Some(global),
        global_calls: Cell::new(0),
        test_mode: false,
    };
    let first = Statement {
        SQL: "select * from t".to_owned(),
        Tables: vec![table("db1", "first")],
        ..Default::default()
    };
    let second = Statement {
        SQL: first.SQL.clone(),
        Tables: vec![table("db2", "second")],
        ..Default::default()
    };
    assert_eq!(MatchSQLBinding(&mut context, &first).0.unwrap().ID, "first");
    assert_eq!(
        MatchSQLBinding(&mut context, &second).0.unwrap().ID,
        "second"
    );
    assert_eq!(context.global_calls.get(), 2);

    context.cache.clear();
    let mut incomplete = BindingMatchInfo::default();
    assert!(MatchSQLBindingWithCache(&mut context, &first, Some(&mut incomplete)).1);
    assert!(!incomplete.NoDBDigest.is_empty());
    assert_eq!(incomplete.TableNames, first.Tables);

    let session = binding("session", "test", 1);
    let mut session_context = MatchContext {
        cache: HashMap::new(),
        session: Some(session.clone()),
        global: None,
        global_calls: Cell::new(0),
        test_mode: false,
    };
    assert!(MatchSQLBinding(&mut session_context, &first).1);
    assert_eq!(session.UsageInfo.last_used_at(), None);
}

#[test]
fn go_merge_41_insert_values_skip_binding_lookup_and_info_mutation() {
    let mut context = MatchContext {
        cache: HashMap::new(),
        session: None,
        global: Some(binding("global", "test", 1)),
        global_calls: Cell::new(0),
        test_mode: false,
    };
    for sql in [
        "insert into t values (1)",
        "insert into t values (1) on duplicate key update a = values(a)",
        "insert into t set a = 1",
        "replace into t values (1)",
        "explain insert into t values (1)",
    ] {
        let mut info = BindingMatchInfo::default();
        let result = MatchSQLBindingWithCache(
            &mut context,
            &Statement {
                SQL: sql.to_owned(),
                ..Default::default()
            },
            Some(&mut info),
        );
        assert!(!result.1, "{sql}");
        assert!(result.0.is_none(), "{sql}");
        assert!(info.NoDBDigest.is_empty(), "{sql}");
        assert!(info.TableNames.is_empty(), "{sql}");
    }
    assert_eq!(context.global_calls.get(), 0);
    for sql in [
        "insert into t select * from s",
        "replace into t select * from s",
        "explain insert into t select * from s",
        "select * from t",
        "update t set a = 1",
        "delete from t where a = 1",
    ] {
        let mut info = BindingMatchInfo::default();
        let _ = MatchSQLBindingWithCache(
            &mut context,
            &Statement {
                SQL: sql.to_owned(),
                ..Default::default()
            },
            Some(&mut info),
        );
        assert!(!info.NoDBDigest.is_empty(), "{sql}");
    }
}

#[test]
fn go_merge_41_test_mode_reuses_prepared_cache_and_populates_match_info() {
    let mut context = MatchContext {
        cache: HashMap::new(),
        session: None,
        global: Some(binding("global", "test", 1)),
        global_calls: Cell::new(0),
        test_mode: true,
    };
    let statement = Statement {
        SQL: "select * from t".to_owned(),
        ..Default::default()
    };
    assert!(MatchSQLBinding(&mut context, &statement).1);
    assert_eq!(context.global_calls.get(), 1);
    assert!(MatchSQLBinding(&mut context, &statement).1);
    assert_eq!(context.global_calls.get(), 1);
    context.cache.clear();
    let mut info = BindingMatchInfo::default();
    assert!(MatchSQLBindingWithCache(&mut context, &statement, Some(&mut info)).1);
    assert!(!info.NoDBDigest.is_empty());
    assert_eq!(context.global_calls.get(), 2);
}
