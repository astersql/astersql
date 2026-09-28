// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// WrapCastForAggFuncs 对不同聚合模式、参数类型和 DISTINCT 标志的批量断言。

use astersql_expression::{Constant, Expression as _, ast, mysql, types};
use astersql_expression_aggregation as aggregation;
use astersql_expression_exprstatic as exprstatic;
use astersql_planner_util_coreusage::WrapCastForAggFuncs;

/// 对应 Go `TestWrapCastForAggFuncs` 的完整 2×1×4×5 矩阵。
#[test]
fn test_wrap_cast_for_agg_funcs() {
    let context = exprstatic::NewExprContext(Vec::new());
    let eval_context = exprstatic::NewEvalContext(Vec::new());
    let aggregate_names = [ast::AggFuncSum];
    let modes = [
        aggregation::CompleteMode,
        aggregation::FinalMode,
        aggregation::Partial1Mode,
        aggregation::Partial1Mode,
    ];
    let argument_types = [
        mysql::TypeLong,
        mysql::TypeNewDecimal,
        mysql::TypeDouble,
        mysql::TypeLonglong,
        mysql::TypeInt24,
    ];
    let distinct_modes = [true, false];

    let mut aggregate_functions = Vec::with_capacity(40);
    for has_distinct in distinct_modes {
        for name in aggregate_names {
            for mode in modes {
                for argument_type in argument_types {
                    let argument: astersql_expression::ExprBox = Box::new(Constant::with_type(
                        types::Datum::default(),
                        *types::NewFieldType(argument_type),
                    ));
                    let mut aggregate =
                        aggregation::NewAggFuncDesc(&context, name, vec![argument], has_distinct)
                            .expect("SUM descriptor must be constructible for every Go test type");
                    aggregate.Mode = mode;
                    aggregate_functions.push(aggregate);
                }
            }
        }
    }

    let original_aggregate_functions = aggregate_functions
        .iter()
        .map(aggregation::AggFuncDesc::Clone)
        .collect::<Vec<_>>();

    WrapCastForAggFuncs(&context, &mut aggregate_functions);

    assert_eq!(aggregate_functions.len(), 40);
    for (index, (aggregate, original)) in aggregate_functions
        .iter()
        .zip(&original_aggregate_functions)
        .enumerate()
    {
        let actual_argument_type = aggregate.Args[0].GetType(&eval_context).GetType();
        if aggregate.Mode != aggregation::FinalMode && aggregate.Mode != aggregation::Partial2Mode {
            assert_eq!(
                actual_argument_type,
                aggregate
                    .RetTp
                    .as_ref()
                    .expect("SUM descriptor must infer a return type")
                    .GetType(),
                "case {index}: Complete/Partial1 must cast the argument to the return type"
            );
        } else {
            assert_eq!(
                actual_argument_type,
                original.Args[0].GetType(&eval_context).GetType(),
                "case {index}: Final/Partial2 must preserve the original argument type"
            );
        }
    }
}
