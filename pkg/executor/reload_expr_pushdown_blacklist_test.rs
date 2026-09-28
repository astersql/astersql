// Copyright 2026 AsterSQL.

use std::collections::HashMap;

use crate::reload_expr_pushdown_blacklist::{
    ExprPushdownBlacklistRuntime, LOAD_EXPR_PUSHDOWN_BLACKLIST_SQL, LoadExprPushdownBlacklist,
    ReloadExprPushdownBlacklistExec, isSameExprPushDownBlackList,
};

struct TestRuntime {
    rows: Result<Vec<(String, String)>, &'static str>,
    current: HashMap<String, u32>,
    queries: Vec<String>,
    replacements: Vec<(HashMap<String, u32>, i64)>,
}

impl Default for TestRuntime {
    fn default() -> Self {
        Self {
            rows: Ok(Vec::new()),
            current: HashMap::new(),
            queries: Vec::new(),
            replacements: Vec::new(),
        }
    }
}

impl ExprPushdownBlacklistRuntime for TestRuntime {
    type Context = ();
    type Error = &'static str;

    fn query_blacklist(
        &mut self,
        _context: &mut Self::Context,
        sql: &str,
    ) -> Result<Vec<(String, String)>, Self::Error> {
        self.queries.push(sql.to_owned());
        self.rows.clone()
    }

    fn store_mask(&self, store_name: &str) -> Option<u32> {
        match store_name {
            "tidb" => Some(1),
            "tikv" => Some(2),
            "tiflash" => Some(4),
            _ => None,
        }
    }

    fn current_blacklist(&self) -> HashMap<String, u32> {
        self.current.clone()
    }

    fn unix_nanos(&self) -> i64 {
        42
    }

    fn replace_blacklist(&mut self, blacklist: HashMap<String, u32>, reload_time: i64) {
        self.replacements.push((blacklist, reload_time));
    }
}

#[test]
fn load_matches_go_lowercase_alias_aggregation_and_replacement() {
    let mut runtime = TestRuntime {
        rows: Ok(vec![
            ("ÄBS".to_owned(), "TiKV,unknown".to_owned()),
            ("<<".to_owned(), "TiDB,TIFLASH".to_owned()),
            ("<<".to_owned(), "TiKV".to_owned()),
        ]),
        ..TestRuntime::default()
    };

    LoadExprPushdownBlacklist(&mut runtime, &mut ()).unwrap();

    assert_eq!(runtime.queries, [LOAD_EXPR_PUSHDOWN_BLACKLIST_SQL]);
    assert_eq!(runtime.replacements.len(), 1);
    assert_eq!(runtime.replacements[0].1, 42);
    assert_eq!(
        runtime.replacements[0].0,
        HashMap::from([("äbs".to_owned(), 2), ("leftshift".to_owned(), 7)])
    );
}

#[test]
fn unchanged_blacklist_skips_timestamped_replacement() {
    let current = HashMap::from([("abs".to_owned(), 2)]);
    let mut runtime = TestRuntime {
        rows: Ok(vec![("ABS".to_owned(), "TiKV".to_owned())]),
        current,
        ..TestRuntime::default()
    };

    LoadExprPushdownBlacklist(&mut runtime, &mut ()).unwrap();

    assert!(runtime.replacements.is_empty());
}

#[test]
fn query_error_is_returned_without_replacement() {
    let mut runtime = TestRuntime {
        rows: Err("restricted SQL failed"),
        ..TestRuntime::default()
    };

    assert_eq!(
        LoadExprPushdownBlacklist(&mut runtime, &mut ()),
        Err("restricted SQL failed")
    );
    assert!(runtime.replacements.is_empty());
}

#[test]
fn next_delegates_to_the_loader() {
    let runtime = TestRuntime {
        rows: Ok(Vec::new()),
        ..TestRuntime::default()
    };
    let mut executor = ReloadExprPushdownBlacklistExec { runtime };

    executor.Next(&mut (), &mut ()).unwrap();

    assert_eq!(executor.runtime.queries.len(), 1);
}

#[test]
fn blacklist_equality_requires_identical_keys_and_values() {
    let left = HashMap::from([("abs".to_owned(), 2)]);
    assert!(isSameExprPushDownBlackList(&left, &left));
    assert!(!isSameExprPushDownBlackList(
        &left,
        &HashMap::from([("abs".to_owned(), 4)])
    ));
    assert!(!isSameExprPushDownBlackList(&left, &HashMap::new()));
}
