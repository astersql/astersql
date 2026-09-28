// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// DXF 调度资源估算的回归测试。
//
// 覆盖 add-index 与 import-into 的节点数上限、按数据量计算的执行槽位、
// DistSQL 并发插值，以及索引体积比和调谐放大因子对估算结果的影响。
// 各组边界值与 Go 版本保持一致，防止 Rust 移植偏离原有资源分配策略。

use crate::*;

const GIB: i64 = 1024 * 1024 * 1024;
const MIB: i64 = 1024 * 1024;
const TIB: i64 = 1024 * GIB;

/// 构造仅指定数据量放大倍数的调谐参数，供各测试复用。
fn factors(amplify_factor: f64) -> schstatus::TuneFactors {
    schstatus::TuneFactors {
        AmplifyFactor: amplify_factor,
    }
}

/// 验证 add-index 按表大小和单节点核数估算节点数，并遵守其专用上限。
#[test]
fn test_calc_max_node_count_by_table_size() {
    let cases = [
        (0, 8, 1),
        (10, 0, 0),
        (320 * GIB + 100, 4, 3),
        (100 * TIB, 4, 60),
        (10 * GIB, 8, 1),
        (200 * GIB, 8, 1),
        (800 * GIB, 8, 4),
        (1100 * GIB, 8, 6),
        (200 * TIB, 8, 30),
        (200 * GIB, 16, 1),
        (600 * GIB, 16, 2),
        (1200 * GIB, 16, 3),
        (4 * TIB, 16, 10),
        (6 * TIB, 16, 15),
        (10 * TIB, 16, 15),
    ];
    for (data_size, cores, expected) in cases {
        assert_eq!(
            NewRCCalcForAddIndex(data_size, cores, &factors(1.0)).max_node_count_for_add_index(),
            expected,
            "data_size={data_size}, cores={cores}"
        );
    }
}

/// 验证 import-into 的节点数随数据量增长，并在不同核数下正确夹取上限。
#[test]
fn test_calc_max_node_count_by_data_size() {
    let cases = [
        (0, 8, 1),
        (10, 0, 0),
        (320 * GIB + 100, 4, 3),
        (100 * TIB, 4, 64),
        (10 * GIB, 8, 1),
        (200 * GIB, 8, 1),
        (800 * GIB, 8, 4),
        (1100 * GIB - 100, 8, 5),
        (1100 * GIB, 8, 6),
        (200 * TIB, 8, 32),
        (200 * GIB, 16, 1),
        (600 * GIB, 16, 2),
        (1200 * GIB, 16, 3),
        (4 * TIB, 16, 10),
        (6 * TIB, 16, 15),
        (10 * TIB, 16, 16),
        (100 * TIB, 16, 16),
    ];
    for (data_size, cores, expected) in cases {
        assert_eq!(
            NewRCCalc(data_size, cores, 0.0, &factors(1.0)).max_node_count_for_import_into(),
            expected,
            "data_size={data_size}, cores={cores}"
        );
    }
}

/// 验证槽位估算的默认值、舍入结果及 `[1, node_cpu]` 边界。
#[test]
fn test_calc_required_slots_by_data_size() {
    let cases = [
        (0, 5, 4),
        (-100, 3, 4),
        (24 * GIB, 5, 1),
        (25 * GIB, 5, 1),
        (25 * GIB, 1, 1),
        (50 * GIB, 4, 2),
        (100 * GIB, 3, 3),
        (37 * GIB + 512 * MIB, 8, 2),
        (50 * GIB, 8, 2),
        (100 * GIB, 8, 4),
        (50 * GIB, 10, 2),
        (75 * GIB, 4, 3),
        (25 * 1000 * GIB, 16, 16),
        (1, 5, 1),
    ];
    for (data_size, cores, expected) in cases {
        assert_eq!(
            NewRCCalc(data_size, cores, 0.0, &factors(1.0)).required_slots(),
            expected,
            "data_size={data_size}, cores={cores}"
        );
    }
}

/// 验证 DistSQL 并发从单节点默认值插值到每核并发上限。
#[test]
fn test_calc_dist_sql_concurrency() {
    let cases = [
        (1, 1, 8, 15),
        (3, 1, 8, 45),
        (7, 1, 8, 105),
        (8, 1, 8, 120),
        (8, 2, 8, 124),
        (8, 5, 8, 137),
        (8, 32, 8, 256),
        (8, 33, 8, 256),
        (8, 50, 8, 256),
        (1, 1, 16, 15),
        (7, 1, 16, 105),
        (16, 1, 16, 240),
        (16, 5, 16, 275),
        (16, 32, 16, 512),
        (16, 33, 16, 512),
        (16, 50, 16, 512),
        (1, 1, 32, 15),
        (7, 1, 32, 105),
        (32, 1, 32, 480),
        (32, 5, 32, 550),
        (32, 32, 32, 1024),
        (32, 33, 32, 1024),
    ];
    for (threads, nodes, cores, expected) in cases {
        assert_eq!(
            CalcDistSQLConcurrency(threads, nodes, cores),
            expected,
            "threads={threads}, nodes={nodes}, cores={cores}"
        );
    }
}

/// Go's autoscaler also exposes the execution-node CPU lookup, including its
/// test-mode fallback when the DXF service task manager is unavailable.
#[test]
fn get_exec_cpu_node_keeps_go_error_and_test_fallback_contract() {
    let source = include_str!("autoscaler.rs");
    for required in [
        "pub fn GetExecCPUNode",
        "GetDXFSvcTaskMgr()",
        "GetTargetScope()",
        "GetCPUCountOfNodeByRole",
        "astersql_util_intest::InTest",
        "astersql_util_cpu::GetCPUCount()",
    ] {
        assert!(source.contains(required), "missing Go contract: {required}");
    }
}

/// 验证索引体积比通过放大有效数据量同时影响槽位数和导入节点数。
#[test]
fn test_index_size_ratio() {
    let cases = [(0.0, 4, 1), (1.0, 8, 1), (1.5, 8, 1), (2.0, 8, 2)];
    for (ratio, expected_slots, expected_nodes) in cases {
        let calc = NewRCCalc(100 * GIB, 8, ratio, &factors(1.0));
        assert_eq!(calc.required_slots(), expected_slots, "ratio={ratio}");
        assert_eq!(
            calc.max_node_count_for_import_into(),
            expected_nodes,
            "ratio={ratio}"
        );
    }
}

/// 验证调谐放大因子作用于有效数据量，并同步放宽两类任务的节点上限。
#[test]
fn test_tune_factors() {
    let cases = [
        (100 * GIB, 1.0, 4, 1, 1),
        (1000 * GIB, 1.0, 8, 5, 5),
        (1000 * GIB, 1.5, 8, 8, 8),
        (1000 * GIB, 2.0, 8, 10, 10),
        (1000 * GIB, 10.0, 8, 50, 50),
        (100 * TIB, 2.0, 8, 64, 60),
        (100 * TIB, 5.0, 8, 160, 150),
    ];
    for (data_size, amplify, slots, import_nodes, add_index_nodes) in cases {
        let calc = NewRCCalc(data_size, 8, 0.0, &factors(amplify));
        assert_eq!(calc.required_slots(), slots);
        assert_eq!(calc.max_node_count_for_import_into(), import_nodes);
        assert_eq!(calc.max_node_count_for_add_index(), add_index_nodes);
    }
}
