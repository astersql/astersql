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

use crate::{
    newStmtSummaryByDigestEvicted, newStmtSummaryByDigestEvictedElement,
    stmtSummaryByDigestElement, stmtSummaryStats,
};
use std::time::Duration;

#[test]
fn go_merge_34_evicted_history_keeps_latest_intervals() {
    let mut evicted = newStmtSummaryByDigestEvicted();
    for begin in 1..=3 {
        evicted
            .history
            .push_back(newStmtSummaryByDigestEvictedElement(begin, begin + 1));
    }
    let selected = evicted.collectHistorySummaries(2);
    assert_eq!(selected.len(), 2);
    assert_eq!(selected[0].beginTime, 2);
    assert_eq!(selected[1].beginTime, 3);
}

#[test]
fn go_merge_34_evicted_merges_ia_metrics() {
    let mut combined = stmtSummaryByDigestElement {
        stmtSummaryStats: stmtSummaryStats {
            iaExecCount: 2,
            sumIARemoteReadSegmentCount: 8,
            maxIARemoteReadSegmentCount: 3,
            sumIARemoteReadSegmentSize: 12,
            maxIARemoteReadSegmentSize: 7,
            sumIARemoteReadSegmentWaitTime: Duration::from_millis(9),
            maxIARemoteReadSegmentWaitTime: Duration::from_millis(7),
            ..Default::default()
        },
        ..Default::default()
    };
    let other = stmtSummaryByDigestElement {
        stmtSummaryStats: stmtSummaryStats {
            iaExecCount: 3,
            sumIARemoteReadSegmentCount: 8,
            maxIARemoteReadSegmentCount: 5,
            sumIARemoteReadSegmentSize: 18,
            maxIARemoteReadSegmentSize: 10,
            sumIARemoteReadSegmentWaitTime: Duration::from_millis(11),
            maxIARemoteReadSegmentWaitTime: Duration::from_millis(8),
            ..Default::default()
        },
        ..Default::default()
    };
    crate::evicted::addInfo(&mut combined, &other);
    assert_eq!(combined.stmtSummaryStats.iaExecCount, 5);
    assert_eq!(combined.stmtSummaryStats.sumIARemoteReadSegmentCount, 16);
    assert_eq!(combined.stmtSummaryStats.maxIARemoteReadSegmentCount, 5);
    assert_eq!(combined.stmtSummaryStats.sumIARemoteReadSegmentSize, 30);
    assert_eq!(combined.stmtSummaryStats.maxIARemoteReadSegmentSize, 10);
    assert_eq!(
        combined.stmtSummaryStats.sumIARemoteReadSegmentWaitTime,
        Duration::from_millis(20)
    );
    assert_eq!(
        combined.stmtSummaryStats.maxIARemoteReadSegmentWaitTime,
        Duration::from_millis(8)
    );
}
