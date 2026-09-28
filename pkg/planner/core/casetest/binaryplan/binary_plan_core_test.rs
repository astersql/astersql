// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Binary Plan（二进制执行计划）核心编解码用例。
//
// 对应 Go `binary_plan_core_test.go`：验证将算子名与运行时行数编码为二进制载荷后，
// 再解码能无损还原。Binary Plan 是语句摘要（statement summary）等路径上用于持久化/
// 传输物理执行计划摘要的紧凑表示。

/// 回归：IndexLookUp 算子与行数 42 经 encode/decode 往返后保持不变。
#[test]
fn canonical_binary_plan_round_trip_preserves_operator_and_runtime_rows() {
    let payload = super::encode_binary_plan("IndexLookUp", 42).unwrap();
    assert_eq!(
        super::decode_binary_plan(&payload).unwrap(),
        ("IndexLookUp".into(), 42)
    );
}

#[test]
fn binary_plan_codec_preserves_utf8_and_extreme_runtime_rows() {
    let payload = super::encode_binary_plan("表扫描", u64::MAX).unwrap();
    assert_eq!(
        super::decode_binary_plan(&payload).unwrap(),
        ("表扫描".into(), u64::MAX)
    );
}

#[test]
fn binary_plan_codec_reports_malformed_payloads_without_panicking() {
    assert_eq!(
        super::decode_binary_plan(&[]),
        Err(super::BinaryPlanCodecError::Truncated)
    );
    assert_eq!(
        super::decode_binary_plan(&[0, 0, 0, 3, b'a']),
        Err(super::BinaryPlanCodecError::Truncated)
    );
    let mut invalid_utf8 = vec![0, 0, 0, 1, 0xff];
    invalid_utf8.extend_from_slice(&0_u64.to_be_bytes());
    assert_eq!(
        super::decode_binary_plan(&invalid_utf8),
        Err(super::BinaryPlanCodecError::InvalidUtf8)
    );
    let mut trailing = super::encode_binary_plan("Scan", 1).unwrap();
    trailing.push(0);
    assert_eq!(
        super::decode_binary_plan(&trailing),
        Err(super::BinaryPlanCodecError::InvalidRuntimeRows)
    );
}

fn operator(name: &str, task_type: super::TaskType) -> super::BinaryPlanOperator {
    super::BinaryPlanOperator {
        name: name.to_owned(),
        task_type,
        root_basic_exec_info: Some("time:1ms".to_owned()),
        root_group_exec_info: Some(vec!["group".to_owned()]),
        cop_exec_info: Some("scan:1".to_owned()),
        access_objects: Some(vec!["test.t".to_owned()]),
        memory_bytes: 1024,
        disk_bytes: 2048,
        children: Vec::new(),
    }
}

#[test]
fn simplify_binary_plan_matches_go_runtime_and_json_stability_rules() {
    let mut root = operator("Projection", super::TaskType::Root);
    root.children
        .push(operator("TableFullScan", super::TaskType::Cop));
    root.children
        .push(operator("Selection", super::TaskType::Unknown));
    let mut plan = super::BinaryPlan {
        main: Some(root),
        ctes: vec![operator("CTEFullScan", super::TaskType::Cop)],
        with_runtime_stats: true,
        discarded_due_to_too_long: false,
    };

    plan.simplify_for_comparison().unwrap();
    let main = plan.main.unwrap();
    assert_eq!(main.root_basic_exec_info, None);
    assert_eq!(main.root_group_exec_info, None);
    assert_eq!(main.cop_exec_info, None);
    assert_eq!(main.access_objects, None);
    assert_eq!(main.memory_bytes, 0);
    assert_eq!(main.disk_bytes, 0);
    assert_eq!(main.children[0].access_objects, None);
    assert_eq!(plan.ctes[0].access_objects, None);
}

#[test]
fn simplify_binary_plan_rejects_missing_stats_or_access_objects() {
    let mut root = operator("Projection", super::TaskType::Root);
    root.root_basic_exec_info = None;
    let mut plan = super::BinaryPlan {
        main: Some(root),
        ctes: Vec::new(),
        with_runtime_stats: true,
        discarded_due_to_too_long: false,
    };
    assert!(plan.simplify_for_comparison().is_err());

    let mut scan = operator("IndexRangeScan", super::TaskType::Cop);
    scan.access_objects = None;
    scan.cop_exec_info = None;
    let mut plan = super::BinaryPlan {
        main: Some(scan),
        ctes: Vec::new(),
        with_runtime_stats: false,
        discarded_due_to_too_long: false,
    };
    assert!(plan.simplify_for_comparison().is_err());
}

#[test]
fn simplify_binary_plan_rejects_empty_runtime_stats_like_go_require_not_empty() {
    let mut root = operator("Projection", super::TaskType::Root);
    root.root_basic_exec_info = Some(String::new());
    let mut plan = super::BinaryPlan {
        main: Some(root),
        ctes: Vec::new(),
        with_runtime_stats: true,
        discarded_due_to_too_long: false,
    };
    assert!(plan.simplify_for_comparison().is_err());

    let mut cop = operator("TableFullScan", super::TaskType::Cop);
    cop.cop_exec_info = Some(String::new());
    let mut plan = super::BinaryPlan {
        main: Some(cop),
        ctes: Vec::new(),
        with_runtime_stats: true,
        discarded_due_to_too_long: false,
    };
    assert!(plan.simplify_for_comparison().is_err());
}

#[test]
fn simplify_binary_plan_uses_go_unanchored_access_operator_match() {
    for name in [
        "prefixTableFullScanSuffix",
        "prefixIndexRangeScanSuffix",
        "prefixCTEFullScanSuffix",
        "prefixPoint_GetSuffix",
    ] {
        let mut access_operator = operator(name, super::TaskType::Unknown);
        access_operator.access_objects = None;
        let mut plan = super::BinaryPlan {
            main: Some(access_operator),
            ctes: Vec::new(),
            with_runtime_stats: false,
            discarded_due_to_too_long: false,
        };
        assert!(
            plan.simplify_for_comparison().is_err(),
            "Go regexp matches access operator {name:?}"
        );
    }
}
