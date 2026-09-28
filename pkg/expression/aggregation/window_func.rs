// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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
// 窗口聚合函数相关实现与辅助逻辑。
//
// 窗口函数在分区内按排序与帧范围计算（如 ROW_NUMBER、RANK、LEAD/LAG）；
// 部分函数不依赖显式帧，或使用默认 CURRENT ROW 帧。本文件提供描述符构造、
// 帧需求判断，以及下推 TiFlash / tipb 转换。

use crate::*;

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use expression::Expression as _;

/// 窗口函数规划期描述，内嵌通用 `baseFuncDesc`（名称、参数、返回类型）。
#[derive(Clone)]
pub struct WindowFuncDesc {
    /// 与聚合共用的基础函数描述字段。
    pub baseFuncDesc: baseFuncDesc,
}

impl Deref for WindowFuncDesc {
    type Target = baseFuncDesc;

    fn deref(&self) -> &Self::Target {
        &self.baseFuncDesc
    }
}

impl DerefMut for WindowFuncDesc {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.baseFuncDesc
    }
}

/// 构造窗口函数描述；参数校验失败时返回 `Ok(None)`（由调用方改走其它路径）。
///
/// `skip_check_args` 为 true 时跳过 NTH_VALUE/NTILE/LEAD/LAG 的常量参数检查。
pub fn NewWindowFuncDesc(
    ctx: &dyn expression::BuildContext,
    name: &str,
    args: Vec<expression::ExprBox>,
    skip_check_args: bool,
) -> Result<Option<WindowFuncDesc>, Error> {
    let name = name.to_ascii_lowercase();
    if !skip_check_args {
        // 校验偏移/桶数等须为合法非零常量；不合法则返回 None。
        match name.as_str() {
            ast::WindowFuncNthValue => {
                let Some(argument) = args.get(1) else {
                    return Ok(None);
                };
                let (value, is_null, ok) =
                    expression::GetUint64FromConstant(ctx.GetEvalCtx(), argument.as_ref());
                if !ok || (value == 0 && !is_null) {
                    return Ok(None);
                }
            }
            ast::WindowFuncNtile => {
                let Some(argument) = args.first() else {
                    return Ok(None);
                };
                let (value, is_null, ok) =
                    expression::GetUint64FromConstant(ctx.GetEvalCtx(), argument.as_ref());
                if !ok || (value == 0 && !is_null) {
                    return Ok(None);
                }
            }
            ast::WindowFuncLead | ast::WindowFuncLag if args.len() >= 2 => {
                let (_, is_null, ok) =
                    expression::GetUint64FromConstant(ctx.GetEvalCtx(), args[1].as_ref());
                if !ok || is_null {
                    return Ok(None);
                }
            }
            _ => {}
        }
    }

    let mut base = newBaseFuncDesc(ctx, &name, args)?;
    // LEAD/LAG 在表达式与默认值均 NOT NULL 时可标返回类型 NOT NULL。
    let lead_lag_not_null = base.Args.len() == 3
        && mysql::HasNotNullFlag(base.Args[0].GetType(ctx.GetEvalCtx()).GetFlag())
        && mysql::HasNotNullFlag(base.Args[2].GetType(ctx.GetEvalCtx()).GetFlag());
    let return_type = base
        .RetTp
        .as_mut()
        .expect("window function return type must be inferred");
    match name.as_str() {
        // 排名类与计数/位运算类结果永非 NULL。
        ast::WindowFuncRowNumber
        | ast::WindowFuncRank
        | ast::WindowFuncDenseRank
        | ast::WindowFuncCumeDist
        | ast::WindowFuncPercentRank
        | ast::AggFuncCount
        | ast::AggFuncApproxCountDistinct
        | ast::AggFuncBitAnd
        | ast::AggFuncBitOr
        | ast::AggFuncBitXor => return_type.AddFlag(mysql::NotNullFlag),
        ast::WindowFuncLead | ast::WindowFuncLag if lead_lag_not_null => {
            return_type.AddFlag(mysql::NotNullFlag);
        }
        _ => return_type.DelFlag(mysql::NotNullFlag),
    }
    Ok(Some(WindowFuncDesc { baseFuncDesc: base }))
}

/// 不依赖窗口帧定义的函数名列表（排名/偏移类按整分区语义计算）。
pub const NO_FRAME_WINDOW_FUNCS: &[&str] = &[
    ast::WindowFuncCumeDist,
    ast::WindowFuncDenseRank,
    ast::WindowFuncLag,
    ast::WindowFuncLead,
    ast::WindowFuncNtile,
    ast::WindowFuncPercentRank,
    ast::WindowFuncRank,
    ast::WindowFuncRowNumber,
];

/// 内置默认帧：ROW_NUMBER 使用 ROWS BETWEEN CURRENT ROW AND CURRENT ROW。
pub fn useDefaultFrameWindowFuncs() -> HashMap<String, ast::FrameClause> {
    HashMap::from([(
        ast::WindowFuncRowNumber.to_owned(),
        ast::FrameClause {
            Type: ast::FrameType::Rows,
            Extent: ast::FrameExtent {
                Start: ast::FrameBound {
                    Type: ast::BoundType::CurrentRow,
                    ..Default::default()
                },
                End: ast::FrameBound {
                    Type: ast::BoundType::CurrentRow,
                    ..Default::default()
                },
            },
        },
    )])
}

/// 若函数有默认帧则返回 `(true, frame)`，否则 `(false, 空帧)`。
pub fn UseDefaultFrame(name: &str) -> (bool, ast::FrameClause) {
    let frames = useDefaultFrameWindowFuncs();
    frames
        .get(&name.to_ascii_lowercase())
        .cloned()
        .map_or((false, ast::FrameClause::default()), |frame| (true, frame))
}

/// 判断窗口函数是否需要显式或默认帧（不在 `NO_FRAME_WINDOW_FUNCS` 中则为 true）。
pub fn NeedFrame(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !NO_FRAME_WINDOW_FUNCS.contains(&name.as_str())
}

impl WindowFuncDesc {
    /// 深拷贝描述符（参数表达式等一并克隆）。
    pub fn Clone(&self) -> WindowFuncDesc {
        WindowFuncDesc {
            baseFuncDesc: self.baseFuncDesc.clone_desc(),
        }
    }

    /// 参数可序列化为 PB 且函数名在 TiFlash 支持列表中时返回 true。
    pub fn CanPushDownToTiFlash(&self, ctx: &expression::PushDownContext) -> bool {
        if !canExprsPushDownToTiFlash(ctx.EvalCtx(), ctx.Client(), &self.Args) {
            return false;
        }
        matches!(
            self.Name.as_str(),
            ast::WindowFuncRowNumber
                | ast::WindowFuncRank
                | ast::WindowFuncDenseRank
                | ast::WindowFuncLead
                | ast::WindowFuncLag
                | ast::WindowFuncFirstValue
                | ast::WindowFuncLastValue
                | ast::AggFuncSum
                | ast::AggFuncCount
                | ast::AggFuncAvg
                | ast::AggFuncMax
                | ast::AggFuncMin
        )
    }
}

/// Go CanExprsPushDown 的 TiFlash 类型门禁；PB 可编码不等于 TiFlash 可执行。
fn canExprsPushDownToTiFlash(
    ctx: &dyn expression::EvalContext,
    client: &dyn kv::Client,
    expressions: &[expression::ExprBox],
) -> bool {
    expressions
        .iter()
        .all(|expr| canExprPushDownToTiFlash(ctx, client, expr.as_ref()))
}

fn canExprPushDownToTiFlash(
    ctx: &dyn expression::EvalContext,
    client: &dyn kv::Client,
    expr: &dyn expression::Expression,
) -> bool {
    let field_type = expr.GetType(ctx);
    match field_type.GetType() {
        mysql::TypeEnum
        | mysql::TypeBit
        | mysql::TypeSet
        | mysql::TypeGeometry
        | mysql::TypeUnspecified => return false,
        mysql::TypeNewDecimal if !field_type.IsDecimalValid() => return false,
        _ => {}
    }
    if let Some(function) = expr.as_any().downcast_ref::<expression::ScalarFunction>() {
        if !expression::IsPushDownEnabled(&function.FuncName.L, kv::StoreType::TiFlash) {
            return false;
        }
    }
    let converter = expression::NewPBConverter(client, ctx);
    let Some(encoded) = converter.ExprToPB(expr) else {
        return false;
    };
    if let Some(function) = expr.as_any().downcast_ref::<expression::ScalarFunction>() {
        let full_name =
            format!("{}.{:?}", function.FuncName.L, encoded.get_sig()).to_ascii_lowercase();
        if !expression::IsPushDownEnabled(&full_name, kv::StoreType::TiFlash) {
            return false;
        }
        return function
            .GetArgs()
            .iter()
            .all(|argument| canExprPushDownToTiFlash(ctx, client, argument.as_ref()));
    }
    true
}

/// 将窗口函数描述转为 tipb::Expr；客户端不支持该类型或参数转换失败时返回 None。
/// 将窗口描述转为 tipb::Expr；存储端不支持该 ExprType 时返回 None。
pub fn WindowFuncToPBExpr(
    ctx: &dyn expression::EvalContext,
    client: &dyn kv::Client,
    desc: &WindowFuncDesc,
) -> Option<tipb::Expr> {
    let converter = expression::NewPBConverter(client, ctx);
    let expression_type = desc.GetTiPBExpr(true);
    if !client.IsRequestTypeSupported(kv::ReqTypeSelect, expression_type as i64) {
        return None;
    }
    let children = desc
        .Args
        .iter()
        .map(|argument| converter.ExprToPB(argument.as_ref()))
        .collect::<Option<Vec<_>>>()?;
    let mut result = tipb::Expr::new();
    result.set_tp(expression_type);
    result.set_children(children.into());
    result.set_field_type(expression::ToPBFieldType(
        desc.RetTp
            .as_ref()
            .expect("window return type must be inferred"),
    ));
    Some(result)
}
