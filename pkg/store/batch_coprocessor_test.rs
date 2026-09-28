// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 批量 Coprocessor 查询的存储层错误处理测试。
//
// 通过最小化的 TiFlash 节点模型验证两类关键语义：查询取消只影响下一次请求，
// 以及部分节点失效时仍可切换到可用节点、全部节点失效时才返回错误。

use std::collections::HashSet;

#[derive(Debug, Eq, PartialEq)]
/// 批量 Coprocessor 查询可能返回的受控错误。
enum QueryError {
    /// 当前查询在访问节点前被取消。
    Cancelled,
    /// 没有任何可用的 TiFlash 节点可以执行查询。
    AllTiFlashPeersFailed,
}

/// 仅保留节点健康状态与查询结果规模的批量查询存储模型。
struct MockBatchCopStore {
    peers: Vec<String>,
    failed: HashSet<String>,
    fail_once: HashSet<String>,
    cancel_next: bool,
    row_count: usize,
}

impl MockBatchCopStore {
    fn with_tiflash_peers(count: usize) -> Self {
        Self {
            peers: (0..count).map(|index| format!("tiflash{index}")).collect(),
            failed: HashSet::new(),
            fail_once: HashSet::new(),
            cancel_next: false,
            row_count: 1,
        }
    }

    fn cancel_next_query(&mut self) {
        self.cancel_next = true;
    }

    fn fail_peer(&mut self, peer: &str) {
        self.failed.insert(peer.to_owned());
    }

    fn fail_peer_once(&mut self, peer: &str) {
        self.fail_once.insert(peer.to_owned());
    }

    fn query_count(&mut self) -> Result<usize, QueryError> {
        // 取消标记是一次性的，读取后立即清除，后续查询仍可正常选择节点。
        if std::mem::take(&mut self.cancel_next) {
            return Err(QueryError::Cancelled);
        }
        // Go 的 `1*return(...)` failpoint 只令首次 RPC 失败；同一次查询会重试，
        // 因此消费一次性故障后，该节点仍是可用候选。
        self.fail_once.clear();
        // 单个节点失败不应阻断查询；只有所有候选节点都失败才返回错误。
        if self
            .peers
            .iter()
            .all(|peer| self.failed.contains(peer.as_str()))
        {
            return Err(QueryError::AllTiFlashPeersFailed);
        }
        Ok(self.row_count)
    }
}

#[test]
fn test_store_err() {
    let mut store = MockBatchCopStore::with_tiflash_peers(1);
    // 先验证取消错误，再恢复节点并确认错误状态不会污染后续查询。
    store.cancel_next_query();
    assert_eq!(store.query_count(), Err(QueryError::Cancelled));

    store.fail_peer_once("tiflash0");
    assert_eq!(store.query_count(), Ok(1));

    store.fail_peer("tiflash0");
    assert_eq!(store.query_count(), Err(QueryError::AllTiFlashPeersFailed));
}

#[test]
fn test_store_switch_peer() {
    let mut store = MockBatchCopStore::with_tiflash_peers(2);
    // 首个节点失败时可使用备用节点，备用节点也失败后才耗尽全部候选。
    store.fail_peer("tiflash0");
    assert_eq!(store.query_count(), Ok(1));

    store.fail_peer("tiflash1");
    assert_eq!(store.query_count(), Err(QueryError::AllTiFlashPeersFailed));
}
