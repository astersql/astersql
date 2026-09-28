// Copyright 2021 PingCAP, Inc.
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

// KvExecCounter 单测：同一 target 多次拦截只计一次，不同 target 分别计数。

#![allow(non_snake_case)]

use super::stmtstats_tests::*;

/// 验证对 TIKV-1/TIKV-2 各拦截 10 次后，统计中各仅记 1，且 next 结果透传。
#[test]
fn TestKvExecCounter() {
    let _guard = super::test_support::stmtstats_guard();
    reset_top_state();
    topsql_state::EnableTopSQL();

    let stats = CreateStatementStats();
    let counter = stats.CreateKvExecCounter(b"SQL-1", b"");
    // 同一 target 重复拦截，去重后只计 1。
    for _ in 0..10 {
        assert_eq!(
            counter.intercept("TIKV-1", (), |target, ()| Ok::<_, ()>(target.to_owned())),
            Ok("TIKV-1".to_owned())
        );
    }
    for _ in 0..10 {
        assert_eq!(
            counter.intercept("TIKV-2", (), |target, ()| Ok::<_, ()>(target.to_owned())),
            Ok("TIKV-2".to_owned())
        );
    }

    let data = stats.Take();
    let counts = data[&SQLPlanDigest::new(b"SQL-1", b"")]
        .KvStatsItem
        .KvExecCount
        .as_ref()
        .expect("KV execution counts initialized");
    assert_eq!(counts.len(), 2);
    assert_eq!(counts["TIKV-1"], 1);
    assert_eq!(counts["TIKV-2"], 1);
    reset_top_state();
}
