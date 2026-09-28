// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// DXF framework 集成测试入口的可移植行为检查。

/// 校验测试 guard 覆写最大并发任务数后，drop/调用 restore 能还原原值。
#[test]
fn max_concurrent_task_test_guard_restores_global_value() {
    astersql_testkit_testsetup::SetupForCommonTest();
    use astersql_dxf_framework_proto::{GetMaxConcurrentTask, SetMaxConcurrentTaskForTest};
    let original = GetMaxConcurrentTask();
    let restore = SetMaxConcurrentTaskForTest(77);
    assert_eq!(GetMaxConcurrentTask(), 77);
    restore();
    assert_eq!(GetMaxConcurrentTask(), original);
}
