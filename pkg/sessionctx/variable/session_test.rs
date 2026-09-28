// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

use super::{LazyStmtText, PlanCacheParamList, RetryInfo, SessionVars};
use crate::vardef;

#[test]
fn unchanged_lock_keys_preserve_binary_record_key_bytes_and_modes() {
    let vars = SessionVars::new();
    let shared = [0x74, 0xff, 0x00, 0x31];
    let exclusive = [0x74, 0xfe, 0x00, 0x31];
    vars.TxnCtx.AddUnchangedKeyForLock(&shared, true);
    vars.TxnCtx.AddUnchangedKeyForLock(&exclusive, false);
    assert_eq!(
        vars.TxnCtx.CollectUnchangedKeysForSLock(Vec::new()),
        vec![shared.to_vec()]
    );
    assert_eq!(
        vars.TxnCtx.CollectUnchangedKeysForXLock(Vec::new()),
        vec![exclusive.to_vec()]
    );
}

#[test]
fn status_counters_use_the_go_hot_path_fields() {
    let mut vars = SessionVars::new();
    vars.SysWarningCount = 12;
    vars.SysErrorCount = 7;

    assert_eq!(
        vars.GetSystemVar(vardef::WarningCount).as_deref(),
        Some("12")
    );
    assert_eq!(vars.GetSystemVar(vardef::ErrorCount).as_deref(), Some("7"));
}

#[test]
fn lazy_statement_text_includes_arguments_and_applies_redaction() {
    let mut params = PlanCacheParamList::new();
    params.Append(&["1".to_owned(), "secret".to_owned()]);

    let mut text = LazyStmtText::default();
    text.Update("OFF", "select ?, ?", Some(&params));
    assert_eq!(text.String(), "select ?, ? [arguments: 1, secret]");

    text.Update("ON", "select ?", Some(&params));
    assert_eq!(text.String(), "");

    text.Update("MARKER", "select ?", Some(&params));
    assert_eq!(text.String(), "‹select ? [arguments: 1, secret]›");

    params.SetForNonPrepCache(true);
    text.Update("OFF", "select ?", Some(&params));
    assert_eq!(text.String(), "select ?");
}

#[test]
fn retry_id_cursor_can_be_reset_through_shared_session_variables() {
    let mut vars = SessionVars::new();
    vars.RetryInfo.AddAutoIncrementID(17);
    vars.RetryInfo.AddAutoRandomID(23);
    assert_eq!(vars.RetryInfo.GetCurrAutoIncrementID(), (17, true));
    assert_eq!(vars.RetryInfo.GetCurrAutoRandomID(), (23, true));
    let shared = std::sync::Arc::new(vars);
    shared.RetryInfo.ResetOffset();
    let mut copy: RetryInfo = shared.RetryInfo.clone();
    assert_eq!(copy.GetCurrAutoIncrementID(), (17, true));
    assert_eq!(copy.GetCurrAutoRandomID(), (23, true));
}
