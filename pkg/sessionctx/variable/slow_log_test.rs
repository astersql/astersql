// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use crate::slow_log::{self, Threshold};

#[test]
fn rule_fields_match_the_go_accessor_registry() {
    assert!(slow_log::ParseSlowLogFieldValue("Plan_digest", "digest").is_err());
}

#[test]
fn rule_entry_searches_for_the_go_word_colon_pattern() {
    let rules = slow_log::ParseSessionSlowLogRules("ignored prefix DB:TeSt")
        .unwrap()
        .unwrap();
    assert_eq!(
        rules.rules[0].threshold("db"),
        Some(&Threshold::String("TeSt".to_owned()))
    );
}

#[test]
fn digest_setter_materializes_the_statement_digest_like_go() {
    let mut vars = crate::session::SessionVars::new();
    vars.StmtCtx.OriginalSQL = "select * from t".to_owned();
    let mut items = slow_log::SlowQueryLogItems::default();
    let accessor = slow_log::SlowLogRuleFieldAccessors
        .get(&slow_log::SlowLogDigestStr.to_ascii_lowercase())
        .unwrap();

    (accessor.Setter.as_ref().unwrap())(
        &slow_log::SlowLogExecContext::default(),
        Some(&vars),
        &mut items,
    );

    assert!(!items.Digest.is_empty());
    assert_eq!(items.Digest, vars.StmtCtx.SQLDigest().1.String());
}

#[test]
fn wait_ts_rule_reads_shared_wait_duration_in_seconds() {
    let vars = crate::session::SessionVars::new();
    let accessor = slow_log::SlowLogRuleFieldAccessors.get("wait_ts").unwrap();
    let items = slow_log::SlowQueryLogItems::default();
    *vars.DurationWaitTS.lock().unwrap() = std::time::Duration::from_millis(2);
    assert!((accessor.Match)(
        Some(&vars),
        &items,
        &Threshold::Float(0.002)
    ));
    assert!(!(accessor.Match)(
        Some(&vars),
        &items,
        &Threshold::Float(0.003)
    ));
    *vars.DurationWaitTS.lock().unwrap() = std::time::Duration::ZERO;
    assert!(!(accessor.Match)(
        Some(&vars),
        &items,
        &Threshold::Float(0.001)
    ));
}
