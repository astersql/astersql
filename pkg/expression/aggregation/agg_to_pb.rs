// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 聚合/窗口函数描述符与 tipb（TiKV/TiFlash 下推用的 Protobuf）之间的双向转换。
//
// `AggFuncToPBExpr` 把聚合描述下推为 PB；`PBExprToAggFuncDesc` 从 DistSQL 结果反解。
// 聚合函数模式（Complete/Partial/Final）决定分布式聚合各阶段的输入输出形态。

use crate::*;

impl baseFuncDesc {
    /// 将聚合或窗口函数名映射为 tipb::ExprType；聚合未命中且允许窗口时再查窗口表。
    pub fn GetTiPBExpr(&self, try_window_desc: bool) -> tipb::ExprType {
        // 先按聚合函数名映射；未知则得到 Null。
        let aggregate = match self.Name.as_str() {
            ast::AggFuncCount => tipb::ExprType::Count,
            ast::AggFuncApproxCountDistinct => tipb::ExprType::ApproxCountDistinct,
            ast::AggFuncFirstRow => tipb::ExprType::First,
            ast::AggFuncGroupConcat => tipb::ExprType::GroupConcat,
            ast::AggFuncMax => tipb::ExprType::Max,
            ast::AggFuncMin => tipb::ExprType::Min,
            ast::AggFuncMaxCount => tipb::ExprType::MaxCount,
            ast::AggFuncMinCount => tipb::ExprType::MinCount,
            ast::AggFuncSum => tipb::ExprType::Sum,
            ast::AggFuncSumInt => tipb::ExprType::SumInt,
            ast::AggFuncAvg => tipb::ExprType::Avg,
            ast::AggFuncBitOr => tipb::ExprType::AggBitOr,
            ast::AggFuncBitXor => tipb::ExprType::AggBitXor,
            ast::AggFuncBitAnd => tipb::ExprType::AggBitAnd,
            ast::AggFuncVarPop => tipb::ExprType::VarPop,
            ast::AggFuncJsonArrayagg => tipb::ExprType::JsonArrayAgg,
            ast::AggFuncJsonObjectAgg => tipb::ExprType::JsonObjectAgg,
            ast::AggFuncStddevPop => tipb::ExprType::StddevPop,
            ast::AggFuncVarSamp => tipb::ExprType::VarSamp,
            ast::AggFuncStddevSamp => tipb::ExprType::StddevSamp,
            _ => tipb::ExprType::Null,
        };
        // 聚合已识别，或调用方不要求尝试窗口映射时直接返回。
        if aggregate != tipb::ExprType::Null || !try_window_desc {
            return aggregate;
        }
        // 回退到窗口函数名 → tipb ExprType。
        match self.Name.as_str() {
            ast::WindowFuncRowNumber => tipb::ExprType::RowNumber,
            ast::WindowFuncRank => tipb::ExprType::Rank,
            ast::WindowFuncDenseRank => tipb::ExprType::DenseRank,
            ast::WindowFuncCumeDist => tipb::ExprType::CumeDist,
            ast::WindowFuncPercentRank => tipb::ExprType::PercentRank,
            ast::WindowFuncNtile => tipb::ExprType::Ntile,
            ast::WindowFuncLead => tipb::ExprType::Lead,
            ast::WindowFuncLag => tipb::ExprType::Lag,
            ast::WindowFuncFirstValue => tipb::ExprType::FirstValue,
            ast::WindowFuncLastValue => tipb::ExprType::LastValue,
            ast::WindowFuncNthValue => tipb::ExprType::NthValue,
            _ => tipb::ExprType::Null,
        }
    }
}

/// 将聚合函数描述符转为可下推的 tipb::Expr（含参数、返回类型、DISTINCT 与模式）。
pub fn AggFuncToPBExpr(
    ctx: &expression::PushDownContext,
    aggregate: &AggFuncDesc,
    store_type: kv::StoreType,
) -> Result<tipb::Expr, Error> {
    let converter = ctx.PbConverter();
    let expression_type = aggregate.GetTiPBExpr(false);
    // 存储客户端须支持该 Select 子类型，否则拒绝下推。
    if !ctx
        .Client()
        .IsRequestTypeSupported(kv::ReqTypeSelect, expression_type as i64)
    {
        return Err(expression::errors::New(
            "select request is not supported by client",
        ));
    }
    // 递归转换聚合参数表达式；任一失败则整体失败。
    let children = aggregate
        .Args
        .iter()
        .map(|argument| converter.ExprToPB(argument.as_ref()))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            expression::errors::New(format!(
                "{} can't be converted to PB.",
                aggregate.StringWithCtx(Some(ctx.EvalCtx()), expression::errors::RedactLogDisable,)
            ))
        })?;

    let return_type = expression::ToPBFieldTypeWithCheck(
        aggregate
            .RetTp
            .as_ref()
            .expect("aggregate return type must be inferred"),
        store_type,
    )?;
    let mut result = tipb::Expr::new();
    result.set_tp(expression_type);
    result.set_children(children.into());
    result.set_field_type(return_type);
    result.set_has_distinct(aggregate.HasDistinct);
    result.set_agg_func_mode(AggFunctionModeToPB(aggregate.Mode));

    // GROUP_CONCAT 额外携带 ORDER BY 与最大长度限制。
    if expression_type == tipb::ExprType::GroupConcat {
        let order_by = aggregate
            .OrderByItems
            .iter()
            .map(|item| {
                expression::SortByItemToPB(
                    ctx.EvalCtx(),
                    ctx.Client(),
                    item.Expr.as_ref(),
                    item.Desc,
                )
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                expression::errors::New(format!(
                    "{} can't be converted to PB.",
                    aggregate
                        .StringWithCtx(Some(ctx.EvalCtx()), expression::errors::RedactLogDisable,)
                ))
            })?;
        result.set_order_by(order_by.into());
        result.set_val(codec::EncodeUint(Vec::new(), ctx.GetGroupConcatMaxLen()));
    }
    Ok(result)
}

/// 将内存侧聚合模式枚举编码为 tipb::AggFunctionMode。
pub fn AggFunctionModeToPB(mode: AggFunctionMode) -> tipb::AggFunctionMode {
    match mode {
        CompleteMode => tipb::AggFunctionMode::CompleteMode,
        FinalMode => tipb::AggFunctionMode::FinalMode,
        Partial1Mode => tipb::AggFunctionMode::Partial1Mode,
        Partial2Mode => tipb::AggFunctionMode::Partial2Mode,
        DedupMode => tipb::AggFunctionMode::DedupMode,
    }
}

/// 将 tipb 聚合模式解码为内存枚举；缺省时按 Partial1Mode。
pub fn PBAggFuncModeToAggFuncMode(mode: Option<tipb::AggFunctionMode>) -> AggFunctionMode {
    match mode.unwrap_or(tipb::AggFunctionMode::Partial1Mode) {
        tipb::AggFunctionMode::CompleteMode => CompleteMode,
        tipb::AggFunctionMode::FinalMode => FinalMode,
        tipb::AggFunctionMode::Partial1Mode => Partial1Mode,
        tipb::AggFunctionMode::Partial2Mode => Partial2Mode,
        tipb::AggFunctionMode::DedupMode => DedupMode,
    }
}

/// 从 tipb::Expr 反构建 AggFuncDesc（名称、参数、返回类型、模式与 DISTINCT）。
pub fn PBExprToAggFuncDesc(
    ctx: &dyn expression::BuildContext,
    aggregate: &tipb::Expr,
    field_types: &[types::FieldType],
) -> Result<AggFuncDesc, Error> {
    // 仅识别可从 DistSQL 回传的聚合类型集合。
    let name = match aggregate.get_tp() {
        tipb::ExprType::Count => ast::AggFuncCount,
        tipb::ExprType::ApproxCountDistinct => ast::AggFuncApproxCountDistinct,
        tipb::ExprType::First => ast::AggFuncFirstRow,
        tipb::ExprType::GroupConcat => ast::AggFuncGroupConcat,
        tipb::ExprType::Max => ast::AggFuncMax,
        tipb::ExprType::Min => ast::AggFuncMin,
        tipb::ExprType::MaxCount => ast::AggFuncMaxCount,
        tipb::ExprType::MinCount => ast::AggFuncMinCount,
        tipb::ExprType::Sum => ast::AggFuncSum,
        tipb::ExprType::SumInt => ast::AggFuncSumInt,
        tipb::ExprType::Avg => ast::AggFuncAvg,
        tipb::ExprType::AggBitOr => ast::AggFuncBitOr,
        tipb::ExprType::AggBitXor => ast::AggFuncBitXor,
        tipb::ExprType::AggBitAnd => ast::AggFuncBitAnd,
        other => {
            return Err(expression::errors::New(format!(
                "unknown aggregation function type: {other:?}"
            )));
        }
    };
    let arguments = expression::PBToExprs(ctx, aggregate.get_children(), field_types)?;
    let mut base = baseFuncDesc {
        Name: name.to_owned(),
        Args: arguments,
        RetTp: Some(expression::FieldTypeFromPB(aggregate.get_field_type())),
    };
    // 为聚合参数补齐必要的类型转换包装。
    base.WrapCastForAggArgs(ctx);
    Ok(AggFuncDesc {
        baseFuncDesc: base,
        Mode: PBAggFuncModeToAggFuncMode(
            aggregate
                .has_agg_func_mode()
                .then(|| aggregate.get_agg_func_mode()),
        ),
        // 与 Go PBExprToAggFuncDesc 一致：该反解路径不恢复 DISTINCT，
        // 因为 PB 返回的聚合描述只用于执行阶段，DISTINCT 由上游计划保留。
        HasDistinct: false,
        OrderByItems: Vec::new(),
        GroupingID: 0,
    })
}
