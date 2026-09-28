// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Aster 侧补充单元测试：错误序列化形状与 subtask 分批边界。
//
// 与 Go 对照：PingCAP/普通 Error 的 JSON 字段，以及按 meta 大小切批
// （含首条超大不产生空批）的行为。

use astersql_dxf_framework_storage::*;
use std::sync::Mutex;

/// 串行化修改全局分批上限的测试，避免并行污染。
static LIMIT_LOCK: Mutex<()> = Mutex::new(());

#[test]
/// 校验 serializeErrOption 对 None / PingCAP / 普通错误的 JSON 形状。
fn serialize_error_matches_pingcap_json_shape() {
    assert!(serializeErrOption(None).is_empty());

    let error = Error::pingcap("annotated err: named err: inner err", "NAMED", 0);
    assert_eq!(
        String::from_utf8(serializeErrOption(Some(error))).unwrap(),
        r#"{"class":0,"code":0,"message":"annotated err: named err: inner err","rfccode":"NAMED"}"#
    );

    let error = Error::new("annotated err: some err");
    assert_eq!(
        String::from_utf8(serializeErrOption(Some(error))).unwrap(),
        r#"{"class":0,"code":0,"message":"annotated err: some err","rfccode":""}"#
    );
}

#[test]
/// 宽松上限单批；收紧 maxSubtaskBatchSize 后按 3+3+3+1 切分。
fn split_subtasks_matches_go_batch_boundaries() {
    let _guard = LIMIT_LOCK.lock().unwrap();
    let manager = TaskManager::for_test(8);
    let subtasks = (0..10)
        .map(|id| proto::Subtask::for_test(id, vec![0; 100]))
        .collect::<Vec<_>>();

    setTxnTotalSizeLimitForTest(100 * 1024 * 1024);
    setMaxSubtaskBatchSizeForTest(16 * 1024 * 1024);
    let batches = manager.splitSubtasks(subtasks.clone());
    assert_eq!(batches.len(), 1);
    assert_eq!(subtask_ids(&batches[0]), (0..10).collect::<Vec<_>>());

    setMaxSubtaskBatchSizeForTest(300);
    let batches = manager.splitSubtasks(subtasks);
    assert_eq!(batches.len(), 4);
    assert_eq!(subtask_ids(&batches[0]), vec![0, 1, 2]);
    assert_eq!(subtask_ids(&batches[1]), vec![3, 4, 5]);
    assert_eq!(subtask_ids(&batches[2]), vec![6, 7, 8]);
    assert_eq!(subtask_ids(&batches[3]), vec![9]);
}

#[test]
/// 首条 meta 已超限时仍单独成批，不得产生空批次。
fn oversized_first_subtask_never_creates_an_empty_batch() {
    let _guard = LIMIT_LOCK.lock().unwrap();
    let manager = TaskManager::for_test(8);
    setTxnTotalSizeLimitForTest(300);
    setMaxSubtaskBatchSizeForTest(300);

    let batches = manager.splitSubtasks(vec![
        proto::Subtask::for_test(1, vec![0; 301]),
        proto::Subtask::for_test(2, vec![0; 1]),
    ]);

    assert_eq!(batches.len(), 2);
    assert_eq!(subtask_ids(&batches[0]), vec![1]);
    assert_eq!(subtask_ids(&batches[1]), vec![2]);
}

/// 提取子任务 ID 列表便于断言。
fn subtask_ids(subtasks: &[proto::Subtask]) -> Vec<i64> {
    subtasks.iter().map(|subtask| subtask.ID).collect()
}
