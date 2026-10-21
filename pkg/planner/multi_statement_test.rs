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

use super::*;

#[test]
fn multi_statement_without_prebuilt_point_plan_is_cache_eligible() {
    let mut vars = SessionVars::new();
    vars.EnableNonPreparedPlanCache = true;
    vars.DisableTxnAutoRetry = true;
    vars.InMultiStmts = true;

    assert_eq!(
        initialNonPreparedCacheEligibility(&vars, true, false, false),
        InitialNonPreparedCacheEligibility::Eligible
    );
}

#[test]
fn multi_statement_with_prebuilt_point_plan_bypasses_cache() {
    let mut vars = SessionVars::new();
    vars.EnableNonPreparedPlanCache = true;
    vars.DisableTxnAutoRetry = true;
    vars.InMultiStmts = true;

    assert_eq!(
        initialNonPreparedCacheEligibility(&vars, true, false, true),
        InitialNonPreparedCacheEligibility::Bypass
    );
}

#[test]
fn standalone_statement_keeps_normal_cache_eligibility() {
    let mut vars = SessionVars::new();
    vars.EnableNonPreparedPlanCache = true;
    vars.DisableTxnAutoRetry = true;

    assert_eq!(
        initialNonPreparedCacheEligibility(&vars, true, false, false),
        InitialNonPreparedCacheEligibility::Eligible
    );
}
