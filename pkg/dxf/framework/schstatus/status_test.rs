// Copyright 2025 PingCAP, Inc.
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

// Status 字符串化行为的单元测试。
//
// 对应 Go 的 TestStatusPrint：校验 BusyNodes 截断展示与原对象不被修改。

use super::status::{Node, Status};

// test_status_print 对应 Go 的 TestStatusPrint，逐项保留其 JSON 截断与不修改原值语义。
#[test]
/// 对应 Go TestStatusPrint：BusyNodes 截断为 6 项且原 Status 长度仍为 10。
fn test_status_print() {
    let mut status = Status::default();
    // 推入 10 个节点以超过 String() 的截断阈值（5）。
    for i in 0..10 {
        status.TiDBWorker.BusyNodes.push(Node {
            ID: format!("tidb-{}", i),
            ..Default::default()
        });
    }
    assert_eq!(status.TiDBWorker.BusyNodes.len(), 10);

    let parsed: Status =
        serde_json::from_str(&status.String()).expect("Status.String returns JSON");
    assert_eq!(parsed.TiDBWorker.BusyNodes.len(), 6);
    assert!(
        parsed.TiDBWorker.BusyNodes[5]
            .ID
            .contains("too many nodes, total 10 busy nodes")
    );
    assert_eq!(status.TiDBWorker.BusyNodes.len(), 10);
}
