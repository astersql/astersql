// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 聚合描述符（AggFuncDesc）与 LogicalAggregation 模式判定单元测试。
//
// 确认 LogicalAggregation 复用 aggregation crate 的共享描述符，以及
// Partial1/Complete 等 AggFunctionMode 与 Go 侧 IsPartialModeAgg/IsCompleteModeAgg 语义一致。

use crate::*;
use std::any::TypeId;

/// 构造 count(1) 聚合描述符并设置指定执行模式（Partial1/Complete/Dedup 等）。
fn descriptor(mode: AggFunctionMode) -> AggFuncDesc {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let mut descriptor = aggregation::NewAggFuncDesc(
        &ctx,
        aggregation::ast::AggFuncCount,
        vec![Box::new(expression::Constant::with_type(
            expression::types::NewIntDatum(1),
            *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        ))],
        false,
    )
    .expect("build count descriptor");
    descriptor.Mode = mode;
    descriptor
}

#[test]
/// 断言 AggFuncDesc 与 aggregation::AggFuncDesc 为同一类型，且字段可写入 LogicalAggregation。
fn logical_aggregation_uses_the_shared_aggregation_descriptor() {
    assert_eq!(
        TypeId::of::<AggFuncDesc>(),
        TypeId::of::<aggregation::AggFuncDesc>()
    );

    let mut descriptor = descriptor(CompleteMode);
    descriptor.GroupingID = 7;
    let aggregation = LogicalAggregation {
        AggFuncs: vec![descriptor],
        ..LogicalAggregation::default()
    };

    assert_eq!(aggregation.AggFuncs[0].Name, "count");
    assert_eq!(aggregation.AggFuncs[0].GroupingID, 7);
}

#[test]
/// 校验 IsPartialModeAgg（首个为 Partial1）与 IsCompleteModeAgg（首个为 Complete）的判定。
fn aggregate_mode_checks_match_the_go_modes() {
    let partial = LogicalAggregation {
        AggFuncs: vec![descriptor(Partial1Mode), descriptor(CompleteMode)],
        ..LogicalAggregation::default()
    };
    assert!(partial.IsPartialModeAgg());
    assert!(!partial.IsCompleteModeAgg());

    let partial_two = LogicalAggregation {
        AggFuncs: vec![descriptor(Partial2Mode)],
        ..LogicalAggregation::default()
    };
    assert!(!partial_two.IsPartialModeAgg());
    assert!(!partial_two.IsCompleteModeAgg());

    let complete = LogicalAggregation {
        AggFuncs: vec![descriptor(CompleteMode)],
        ..LogicalAggregation::default()
    };
    assert!(!complete.IsPartialModeAgg());
    assert!(complete.IsCompleteModeAgg());

    let deduplicate = LogicalAggregation {
        AggFuncs: vec![descriptor(DedupMode)],
        ..LogicalAggregation::default()
    };
    assert!(!deduplicate.IsPartialModeAgg());
    assert!(!deduplicate.IsCompleteModeAgg());

    // Go indexes AggFuncs[0] in both helpers.  An empty descriptor list is an
    // invalid planner state and must not be silently classified as either mode.
    assert!(std::panic::catch_unwind(|| LogicalAggregation::default().IsPartialModeAgg()).is_err());
    assert!(
        std::panic::catch_unwind(|| LogicalAggregation::default().IsCompleteModeAgg()).is_err()
    );
}
