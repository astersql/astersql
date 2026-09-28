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
// Copyright 2019-present PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
//
// fn makeDiagCtx(key: &str, resourceGroupTag: &str) -> diagnosticContext {
//     diagnosticContext {
//         key: key.as_bytes().to_vec(),
//         resourceGroupTag: resourceGroupTag.as_bytes().to_vec(),
//     }
// }
// */
// Detector 死锁检测行为单测。

use crate::detector::{Detector, DiagnosticContext};
use std::time::Duration;

/// 构造带 key/tag 字符串的诊断上下文。
fn diagnostic(key: &str, tag: &str) -> DiagnosticContext {
    DiagnosticContext {
        key: key.as_bytes().to_vec(),
        resource_group_tag: tag.as_bytes().to_vec(),
    }
}

fn assert_wait_entry(
    entry: &crate::detector::WaitForEntry,
    txn: u64,
    wait_for_txn: u64,
    key_hash: u64,
    key: &str,
    tag: &str,
) {
    assert_eq!(txn, entry.txn);
    assert_eq!(wait_for_txn, entry.wait_for_txn);
    assert_eq!(key_hash, entry.key_hash);
    assert_eq!(key.as_bytes(), entry.key);
    assert_eq!(tag.as_bytes(), entry.resource_group_tag);
}

#[test]
/// 1→2→3→1 成环时应返回有序等待链，且不落成闭环触发边。
fn deadlock_cycle_returns_ordered_diagnostic_wait_chain() {
    // urgent_size=1 且 expire_interval=0，便于触发主动过期路径。
    let detector = Detector::new(Duration::from_secs(1), 1, Duration::ZERO);
    // 前两条边不成环；第三条闭合环并返回错误。
    assert!(
        detector
            .detect(1, 2, 100, diagnostic("k1", "tag1"))
            .is_none()
    );
    assert!(
        detector
            .detect(2, 3, 200, diagnostic("k2", "tag2"))
            .is_none()
    );
    let deadlock = detector
        .detect(3, 1, 300, diagnostic("k3", "tag3"))
        .expect("cycle must be detected");
    assert_eq!(200, deadlock.deadlock_key_hash);
    assert_eq!(3, deadlock.wait_chain.len());
    assert_wait_entry(&deadlock.wait_chain[0], 1, 2, 100, "k1", "tag1");
    assert_wait_entry(&deadlock.wait_chain[1], 2, 3, 200, "k2", "tag2");
    assert_wait_entry(&deadlock.wait_chain[2], 3, 1, 300, "k3", "tag3");
    assert_eq!(2, detector.edge_count());
}

#[test]
/// clean_up_wait_for 只删指定边；clean_up 清空该事务全部边。
fn cleanup_removes_only_the_selected_edge() {
    let detector = Detector::new(Duration::from_secs(1), 10, Duration::from_secs(1));
    detector.detect(10, 20, 1, DiagnosticContext::default());
    detector.detect(10, 30, 2, DiagnosticContext::default());
    detector.clean_up_wait_for(10, 20, 1);
    assert_eq!(1, detector.edge_count());
    detector.clean_up(10);
    assert_eq!(0, detector.edge_count());
}

#[test]
/// 与 Go TestDeadlock 一致地覆盖断环、同目标不同键及重复边语义。
fn broken_cycle_and_duplicate_registration_match_go() {
    let detector = Detector::new(Duration::from_millis(50), 1, Duration::from_millis(100));
    let empty = DiagnosticContext::default();

    assert!(
        detector
            .detect(1, 2, 100, diagnostic("k1", "tag1"))
            .is_none()
    );
    assert!(
        detector
            .detect(2, 3, 200, diagnostic("k2", "tag2"))
            .is_none()
    );
    assert!(
        detector
            .detect(3, 1, 300, diagnostic("k3", "tag3"))
            .is_some()
    );
    assert_eq!(2, detector.edge_count());

    detector.clean_up(2);
    assert_eq!(1, detector.edge_count());

    assert!(detector.detect(3, 1, 300, empty.clone()).is_none());
    assert_eq!(2, detector.edge_count());

    assert!(detector.detect(3, 1, 400, empty.clone()).is_none());
    assert_eq!(3, detector.edge_count());

    assert!(detector.detect(3, 1, 400, empty).is_none());
    assert_eq!(3, detector.edge_count());

    detector.clean_up_wait_for(3, 1, 300);
    assert_eq!(2, detector.edge_count());
    detector.clean_up_wait_for(3, 1, 400);
    assert_eq!(1, detector.edge_count());
}

#[test]
/// TTL 过期边不能参与死锁检测，并应在下一次检测时惰性清理。
fn expired_edges_are_removed_without_reporting_a_deadlock() {
    let detector = Detector::new(Duration::from_millis(20), 1, Duration::from_millis(1_000));
    detector.detect(1, 2, 10, diagnostic("a", ""));
    std::thread::sleep(Duration::from_millis(30));
    assert!(detector.detect(2, 1, 20, diagnostic("b", "")).is_none());
    assert_eq!(1, detector.edge_count());
}
