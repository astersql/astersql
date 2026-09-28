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

use std::collections::HashSet;

use astersql_kv::{Context, GetInternalSourceType, InternalTxnPrivilege};

use crate::opt_rule_blacklist::{
    LoadOptRuleBlacklist, OptRuleBlacklistContext, ReloadOptRuleBlacklistExec,
};

const EXPECTED_SQL: &str = "select HIGH_PRIORITY name from mysql.opt_rule_blacklist";

#[derive(Default)]
struct MockContext {
    rows: Vec<String>,
    fail_query: bool,
    seen_source: Option<String>,
    seen_sql: Option<String>,
    replacements: Vec<HashSet<String>>,
}

impl OptRuleBlacklistContext for MockContext {
    type Error = &'static str;

    fn exec_restricted_sql(
        &mut self,
        context: &Context,
        sql: &str,
    ) -> Result<Vec<String>, Self::Error> {
        self.seen_source = Some(GetInternalSourceType(context));
        self.seen_sql = Some(sql.to_owned());
        if self.fail_query {
            return Err("query failed");
        }
        Ok(self.rows.clone())
    }

    fn replace_disabled_logical_rules(&mut self, rules: HashSet<String>) {
        self.replacements.push(rules);
    }
}

#[test]
fn next_uses_fresh_privilege_context_and_replaces_with_unique_names() {
    let mut exec = ReloadOptRuleBlacklistExec {
        context: MockContext {
            rows: vec!["rule-a".into(), "rule-a".into(), "RULE-B".into()],
            ..MockContext::default()
        },
    };
    let caller_context = astersql_kv::WithInternalSourceType(Context::new(), "caller");
    let mut output = ();

    exec.Next(caller_context, &mut output).unwrap();

    assert_eq!(
        exec.context.seen_source.as_deref(),
        Some(InternalTxnPrivilege)
    );
    assert_eq!(exec.context.seen_sql.as_deref(), Some(EXPECTED_SQL));
    assert_eq!(exec.context.replacements.len(), 1);
    assert_eq!(
        exec.context.replacements[0],
        HashSet::from(["rule-a".to_owned(), "RULE-B".to_owned()])
    );
}

#[test]
fn load_propagates_query_error_without_replacing_global_state() {
    let mut runtime = MockContext {
        fail_query: true,
        ..MockContext::default()
    };
    let context = Context::new();

    assert_eq!(
        LoadOptRuleBlacklist(&context, &mut runtime),
        Err("query failed")
    );
    assert!(runtime.replacements.is_empty());
}
