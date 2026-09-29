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

// 物理计划类型字符串与数字 ID 映射的兼容性测试。
//
// 对应 Go `id_test.go`：冻结已有 ID 赋值，并验证正向/反向往返一致性。
// 新增计划类型时只能追加用例，不可修改既有期望值。

use plancodec_dependency::*;

#[test]
/// 校验历史计划类型字符串到数字 ID 的映射未被改动。
fn test_plan_id_changed() {
    // Attention: for compatibility, shouldn't modify the below test, you can only add test when add new plan ID.
    let test_cases = [
        (TypeSel, 1),
        (TypeSet, 2),
        (TypeProj, 3),
        (TypeAgg, 4),
        (TypeStreamAgg, 5),
        (TypeHashAgg, 6),
        (TypeShow, 7),
        (TypeJoin, 8),
        (TypeUnion, 9),
        (TypeTableScan, 10),
        (TypeMemTableScan, 11),
        (TypeUnionScan, 12),
        (TypeIdxScan, 13),
        (TypeSort, 14),
        (TypeTopN, 15),
        (TypeLimit, 16),
        (TypeHashJoin, 17),
        (TypeMergeJoin, 18),
        (TypeIndexJoin, 19),
        (TypeIndexMergeJoin, 20),
        (TypeIndexHashJoin, 21),
        (TypeApply, 22),
        (TypeMaxOneRow, 23),
        (TypeExists, 24),
        (TypeDual, 25),
        (TypeLock, 26),
        (TypeInsert, 27),
        (TypeUpdate, 28),
        (TypeDelete, 29),
        (TypeIndexLookUp, 30),
        (TypeTableReader, 31),
        (TypeIndexReader, 32),
        (TypeWindow, 33),
        (TypeTiKVSingleGather, 34),
        (TypeIndexMerge, 35),
        (TypePointGet, 36),
        (TypeShowDDLJobs, 37),
        (TypeBatchPointGet, 38),
        (TypeClusterMemTableReader, 39),
        (TypeDataSource, 40),
        (TypeLoadData, 41),
        (TypeTableSample, 42),
        (TypeTableFullScan, 43),
        (TypeTableRangeScan, 44),
        (TypeTableRowIDScan, 45),
        (TypeIndexFullScan, 46),
        (TypeIndexRangeScan, 47),
        (TypeExchangeReceiver, 48),
        (TypeExchangeSender, 49),
        (TypeCTE, 50),
        (TypeCTEDefinition, 51),
        (TypeCTETable, 52),
        (TypePartitionUnion, 53),
        (TypeShuffle, 54),
        (TypeShuffleReceiver, 55),
        (TypeImportInto, 59),
        (TypeLocalIndexLookUp, 61),
        (TypeAnalyze, 64),
    ];

    for (plan_type, expected) in test_cases {
        assert_eq!(TypeStringToPhysicalID(plan_type), expected);
    }
}

#[test]
/// 校验 ID 1..=64 的编码/解码往返一致。
fn test_reverse() {
    for id in 1..=64 {
        assert_eq!(TypeStringToPhysicalID(&PhysicalIDToTypeString(id)), id);
    }
}

#[test]
fn go_merge_32_analyze_id_and_full_reverse_range() {
    assert_eq!(TypeStringToPhysicalID("Analyze"), 64);
    assert_eq!(PhysicalIDToTypeString(64), "Analyze");
}
