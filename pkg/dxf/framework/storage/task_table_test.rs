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

// task_table 模块基础单元测试：常量、错误序列化与分批。

use crate::*;
use std::cell::RefCell;
use std::sync::Mutex;

/// 保护全局 TxnTotalSizeLimit / maxSubtaskBatchSize 的测试互斥锁。
static LIMIT_LOCK: Mutex<()> = Mutex::new(());

#[test]
/// 冒烟：空调用记录、保留天数与列清单包含关键字段。
fn TestMain() {
    let manager = TaskManager::new();
    assert!(manager.calls().is_empty());
    assert_eq!(defaultSubtaskKeepDays, 14);
    assert!(TaskColumns.contains("modify_params"));
    assert!(SubtaskColumns.contains("summary"));
}

#[test]
/// 校验 PingCAP/普通 Error 序列化后的 message、rfccode、code。
fn TestSerializeErr() {
    assert!(serializeErrOption(None).is_empty());
    let pingcap = Error::pingcap("annotated err: inner", "DXF:Err", 123);
    let value: serde_json::Value =
        serde_json::from_slice(&serializeErr(pingcap)).expect("serialized error JSON");
    assert_eq!(value["message"], "annotated err: inner");
    assert_eq!(value["rfccode"], "DXF:Err");
    assert_eq!(value["code"], 123);

    let ordinary = Error::new("ordinary");
    let value: serde_json::Value =
        serde_json::from_slice(&serializeErr(ordinary)).expect("serialized ordinary error");
    assert_eq!(value["message"], "ordinary");
    assert_eq!(value["rfccode"], "");
}

#[test]
/// 校验按大小分批及超大首条不成空批。
fn TestSplitSubtasks() {
    let _guard = LIMIT_LOCK.lock().unwrap();
    let manager = TaskManager::for_test(8);
    let subtasks = (0..10)
        .map(|id| proto::Subtask::for_test(id, vec![0; 100]))
        .collect::<Vec<_>>();

    setTxnTotalSizeLimitForTest(100 * 1024 * 1024);
    setMaxSubtaskBatchSizeForTest(300);
    let batches = manager.splitSubtasks(subtasks);
    assert_eq!(batches.len(), 4);
    assert_eq!(ids(&batches[0]), vec![0, 1, 2]);
    assert_eq!(ids(&batches[1]), vec![3, 4, 5]);
    assert_eq!(ids(&batches[2]), vec![6, 7, 8]);
    assert_eq!(ids(&batches[3]), vec![9]);

    setTxnTotalSizeLimitForTest(300);
    let batches = manager.splitSubtasks(vec![
        proto::Subtask::for_test(11, vec![0; 301]),
        proto::Subtask::for_test(12, vec![0; 1]),
    ]);
    assert_eq!(
        batches.len(),
        2,
        "oversized first item must not create an empty batch"
    );
    assert_eq!(ids(&batches[0]), vec![11]);
    assert_eq!(ids(&batches[1]), vec![12]);
}

#[test]
/// Go 的 defer 会在切步动作（含 subtask 插入）结束后才恢复查询内存配额。
fn TestRunWithRestoredSystemVar() {
    let events = RefCell::new(Vec::new());
    let result = runWithRestoredSystemVar(
        |value| {
            events.borrow_mut().push(value);
            Ok(())
        },
        "raised".into(),
        "original".into(),
        || {
            events.borrow_mut().push("action".into());
            Err::<(), _>(Error::new("insert failed"))
        },
    );

    assert_eq!(result.unwrap_err().to_string(), "insert failed");
    assert_eq!(events.into_inner(), vec!["raised", "action", "original"]);
}

/// 提取子任务 ID。
fn ids(subtasks: &[proto::Subtask]) -> Vec<i64> {
    subtasks.iter().map(|subtask| subtask.ID).collect()
}

#[test]
fn task_service_manager_routes_actual_kernel_and_keyspace() {
    let _guard = LIMIT_LOCK.lock().unwrap();
    struct Restore(astersql_config::Config);
    impl Drop for Restore {
        fn drop(&mut self) {
            astersql_config::store_global_config(self.0.clone());
        }
    }
    let _restore = Restore((*astersql_config::get_global_config()).clone());
    let mut actual = _restore.0.clone();
    actual.keyspace_name = "user-keyspace".into();
    astersql_config::store_global_config(actual.clone());
    assert_eq!(
        kerneltype::IsNextGen(),
        astersql_config_kerneltype::IsNextGen()
    );
    assert_eq!(config::GetGlobalKeyspaceName(), "user-keyspace");
    let local = TaskManager::new();
    let service = TaskManager::new();
    SetTaskManager(local.clone());
    SetDXFSvcTaskMgr(service.clone());
    GetDXFSvcTaskMgr()
        .unwrap()
        .ExecuteSQLWithNewSession((), "select 1", vec![])
        .unwrap();
    assert_eq!(
        local.calls().len(),
        usize::from(!astersql_config_kerneltype::IsNextGen())
    );
    assert_eq!(
        service.calls().len(),
        usize::from(astersql_config_kerneltype::IsNextGen())
    );
    actual.keyspace_name = keyspace::System.into();
    astersql_config::store_global_config(actual);
    GetDXFSvcTaskMgr()
        .unwrap()
        .ExecuteSQLWithNewSession((), "select 2", vec![])
        .unwrap();
    assert_eq!(
        local.calls().len(),
        1 + usize::from(!astersql_config_kerneltype::IsNextGen())
    );
}
