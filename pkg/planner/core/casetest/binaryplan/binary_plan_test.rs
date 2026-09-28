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

// Binary Plan 在 EXPLAIN 与 slow log 之间的一致性用例。
//
// 对应 Go `binary_plan_test.go`：同一条 EXPLAIN 的二进制计划必须原样进入 slow log，
// 解码后再清除不稳定字段，并与预期计划比较。

fn operator(
    name: &str,
    task_type: super::TaskType,
    children: Vec<super::BinaryPlanOperator>,
) -> super::BinaryPlanOperator {
    let is_root = task_type == super::TaskType::Root;
    super::BinaryPlanOperator {
        name: name.to_owned(),
        task_type,
        root_basic_exec_info: is_root.then(|| "time:1ms, loops:1".to_owned()),
        root_group_exec_info: is_root.then(|| vec!["executor:1".to_owned()]),
        cop_exec_info: (!is_root).then(|| "cop_task:{num:1}".to_owned()),
        access_objects: Some(vec!["test.t".to_owned()]),
        memory_bytes: 1024,
        disk_bytes: 2048,
        children,
    }
}

/// Go `TestBinaryPlanInExplainAndSlowLog` 的模型级契约：slow log 保存 EXPLAIN 的同一计划，
/// 且 main、children、CTE 的不稳定字段会递归清除后再参与 golden 比较。
#[test]
fn test_binary_plan_in_explain_and_slow_log_matches_go_contract() {
    let plan = super::BinaryPlan {
        main: Some(operator(
            "Projection",
            super::TaskType::Root,
            vec![operator("TableFullScan", super::TaskType::Cop, Vec::new())],
        )),
        ctes: vec![operator("CTEFullScan", super::TaskType::Cop, Vec::new())],
        with_runtime_stats: true,
        discarded_due_to_too_long: false,
    };

    let explain_binary_plan = plan.clone();
    let routed = super::route_binary_plan(&plan, true, 128, 1024);
    let slow_log_binary_plan = routed.slow_log.expect("enabled plan reaches slow log");
    assert_eq!(slow_log_binary_plan, explain_binary_plan);

    let mut decoded = slow_log_binary_plan;
    decoded.simplify_for_comparison().unwrap();
    let main = decoded.main.as_ref().unwrap();
    assert_eq!(main.root_basic_exec_info, None);
    assert_eq!(main.root_group_exec_info, None);
    assert_eq!(main.memory_bytes, 0);
    assert_eq!(main.disk_bytes, 0);
    assert_eq!(main.children[0].cop_exec_info, None);
    assert_eq!(main.children[0].access_objects, None);
    assert_eq!(decoded.ctes[0].cop_exec_info, None);
    assert_eq!(decoded.ctes[0].access_objects, None);
}
