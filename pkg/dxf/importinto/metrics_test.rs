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

// TaskMetricManager 引用计数与注销行为单测，对照 Go TestMetricManager。

use crate::TaskMetricManager;
use std::sync::Arc;

/// 同一 task_id 应返回同一 Arc；unregister 需引用计数归零后才从 map 移除。
#[test]
fn metric_manager_reference_counts_per_task() {
    let manager = TaskMetricManager::default();
    let first = manager.get_or_create_metrics(1);
    let second = manager.get_or_create_metrics(1);
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(manager.registered_task_count(), 1);
    let other = manager.get_or_create_metrics(2);
    assert!(!Arc::ptr_eq(&first, &other));
    assert_eq!(manager.registered_task_count(), 2);
    // 第一次 unregister 只减计数，任务仍注册。
    manager.unregister(1);
    assert_eq!(manager.registered_task_count(), 2);
    assert!(Arc::ptr_eq(&first, &manager.get_or_create_metrics(1)));
    // 上面的获取又增加一次引用，须两次注销才可清理 task 1。
    manager.unregister(1);
    manager.unregister(1);
    assert_eq!(manager.registered_task_count(), 1);
    manager.unregister(2);
    assert_eq!(manager.registered_task_count(), 0);
    // 若 collector 未从默认 registry 注销，使用相同 task_id 再注册会报重复注册。
    let recreated = manager.get_or_create_metrics(1);
    assert!(!Arc::ptr_eq(&first, &recreated));
    manager.unregister(1);
    assert_eq!(manager.registered_task_count(), 0);
}
