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

// 聚合函数参数的 CAST 包装。
//
// 在两阶段聚合（partial/final）中，不同 Mode 下参数类型约定不同：
// Final/Partial2 阶段入参已是中间聚合结果类型，无需再 CAST。

/// Wraps aggregate arguments except in Final/Partial2 modes, where arguments
/// have already been converted to the final aggregate input types.
///
/// 为聚合函数参数包装 CAST；Final/Partial2 模式跳过，
/// 因为参数已转换为最终聚合输入类型。
pub fn WrapCastForAggFuncs(
    context: &dyn expression::BuildContext,
    aggregate_functions: &mut [aggregation::AggFuncDesc],
) {
    // 仅对 Complete / Partial1 等仍接收原始列类型的模式做 CAST。
    for aggregate_function in aggregate_functions {
        if aggregate_function.Mode != aggregation::FinalMode
            && aggregate_function.Mode != aggregation::Partial2Mode
        {
            aggregate_function.WrapCastForAggArgs(context);
        }
    }
}
