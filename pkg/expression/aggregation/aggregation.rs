// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 行式聚合运行时：从 tipb 构建聚合实现，以及下推可行性检查。
//
// `NewDistAggFunc` 供 mock TiKV 等按 PB 表达式实例化具体聚合；`Aggregation` trait
// 定义 Update/GetResult 契约。`CheckAggPushDown` 决定能否下推到 TiKV/TiFlash。
// 分布式聚合常拆成 Partial 与 Final 两阶段，由 AggFunctionMode 区分。

use crate::*;

use std::sync::Arc;

use expression::{ExprBox, Expression as _};

/// Builds the row aggregate used by mock TiKV from a distributed PB
/// expression, preserving the PB aggregate mode and planner descriptor.
/// 由分布式 tipb 表达式构建行式聚合实现，并保留 PB 模式与规划器描述符。
pub fn NewDistAggFunc(
    expr: &tipb::Expr,
    field_types: &[types::FieldType],
    ctx: &dyn expression::BuildContext,
) -> Result<(Box<dyn Aggregation>, crate::AggFuncDesc), crate::Error> {
    let args = expression::PBToExprs(ctx, expr.get_children(), field_types)?;
    // 将 tipb ExprType 映射为 AST 聚合函数名。
    let name = match expr.get_tp() {
        tipb::ExprType::Sum => ast::AggFuncSum,
        tipb::ExprType::SumInt => ast::AggFuncSumInt,
        tipb::ExprType::Count => ast::AggFuncCount,
        tipb::ExprType::Avg => ast::AggFuncAvg,
        tipb::ExprType::GroupConcat => ast::AggFuncGroupConcat,
        tipb::ExprType::Max => ast::AggFuncMax,
        tipb::ExprType::Min => ast::AggFuncMin,
        tipb::ExprType::MaxCount => ast::AggFuncMaxCount,
        tipb::ExprType::MinCount => ast::AggFuncMinCount,
        tipb::ExprType::First => ast::AggFuncFirstRow,
        tipb::ExprType::AggBitOr => ast::AggFuncBitOr,
        tipb::ExprType::AggBitXor => ast::AggFuncBitXor,
        tipb::ExprType::AggBitAnd => ast::AggFuncBitAnd,
        other => {
            return Err(expression::errors::New(format!(
                "Unknown aggregate function type {other:?}"
            )));
        }
    };
    let mut function = newAggFunc(name, args, false);
    function.AggFuncDesc.Mode =
        PBAggFuncModeToAggFuncMode(expr.has_agg_func_mode().then(|| expr.get_agg_func_mode()));
    let descriptor = function.AggFuncDesc.Clone();
    // 按类型装箱为具体聚合结构（SUM/AVG/MAX 等）。
    let aggregate: Box<dyn Aggregation> = match expr.get_tp() {
        tipb::ExprType::Sum => Box::new(crate::sumFunction {
            aggFunction: function,
        }),
        tipb::ExprType::SumInt => Box::new(crate::sumIntFunction {
            aggFunction: function,
        }),
        tipb::ExprType::Count => Box::new(crate::countFunction {
            aggFunction: function,
        }),
        tipb::ExprType::Avg => Box::new(crate::avgFunction {
            aggFunction: function,
        }),
        tipb::ExprType::GroupConcat => Box::new(crate::concatFunction {
            aggFunction: function,
            separator: String::new(),
            maxLen: 0,
            sepInited: false,
            truncated: false,
        }),
        tipb::ExprType::Max | tipb::ExprType::Min => {
            // MAX/MIN 比较依赖参数列的校对规则（collation）。
            let collator =
                collate::GetCollator(descriptor.Args[0].GetType(ctx.GetEvalCtx()).GetCollate());
            Box::new(crate::maxMinFunction {
                aggFunction: function,
                isMax: expr.get_tp() == tipb::ExprType::Max,
                ctor: collator,
            })
        }
        tipb::ExprType::MaxCount | tipb::ExprType::MinCount => {
            let compare_index = if matches!(descriptor.Mode, FinalMode | Partial2Mode)
                && descriptor.Args.len() > 1
            {
                1
            } else {
                0
            };
            let collator = collate::GetCollator(
                descriptor.Args[compare_index]
                    .GetType(ctx.GetEvalCtx())
                    .GetCollate(),
            );
            Box::new(crate::maxMinCountFunction {
                aggFunction: function,
                isMax: expr.get_tp() == tipb::ExprType::MaxCount,
                ctor: collator,
            })
        }
        tipb::ExprType::First => Box::new(crate::firstRowFunction {
            aggFunction: function,
        }),
        tipb::ExprType::AggBitOr => Box::new(crate::bitOrFunction {
            aggFunction: function,
        }),
        tipb::ExprType::AggBitXor => Box::new(crate::bitXorFunction {
            aggFunction: function,
        }),
        tipb::ExprType::AggBitAnd => Box::new(crate::bitAndFunction {
            aggFunction: function,
        }),
        _ => unreachable!(),
    };
    Ok((aggregate, descriptor))
}

/// Runtime contract shared by the row-based aggregate implementations.
/// 行式聚合实现共享的运行时契约：按行 Update，再取部分/最终结果。
pub trait Aggregation {
    /// 用当前行更新聚合中间状态。
    fn Update(
        &mut self,
        eval_ctx: &mut AggEvaluateContext,
        statement_context: &stmtctx::StatementContext,
        row: chunk::Row,
    ) -> Result<(), crate::Error>;
    /// 返回可继续合并的部分聚合结果（多列 Datum）。
    fn GetPartialResult(&self, eval_ctx: &AggEvaluateContext) -> Vec<types::Datum>;
    /// 返回最终聚合标量结果。
    fn GetResult(&self, eval_ctx: &AggEvaluateContext) -> types::Datum;
    /// 为新分组创建求值上下文。
    fn CreateContext(&self, ctx: Arc<dyn expression::EvalContext>) -> AggEvaluateContext;
    /// 重置上下文以便复用到下一分组。
    fn ResetContext(
        &mut self,
        ctx: Arc<dyn expression::EvalContext>,
        eval_ctx: &mut AggEvaluateContext,
    );
}

/// Intermediate state for one aggregate group.
/// 单个聚合分组的中间状态（计数、当前值、DISTINCT 去重器等）。
pub struct AggEvaluateContext {
    /// 表达式求值上下文。
    pub Ctx: Arc<dyn expression::EvalContext>,
    /// DISTINCT 聚合时的去重检查器。
    pub DistinctChecker: Option<crate::distinctChecker>,
    /// 已计入的行数/计数值。
    pub Count: i64,
    /// 当前聚合值（SUM/MAX 等）。
    pub Value: types::Datum,
    /// 可变长度缓冲（如 GROUP_CONCAT）。
    pub Buffer: Vec<u8>,
    /// GROUP_CONCAT 缓冲区是否已初始化；区分“空字符串结果”和 NULL。
    pub BufferInitialized: bool,
    /// FIRST_ROW 是否已取到首行。
    pub GotFirstRow: bool,
}

/// 分布式聚合阶段模式：Complete / Final / Partial1 / Partial2 / Dedup。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(i32)]
pub enum AggFunctionMode {
    #[default]
    CompleteMode = 0,
    FinalMode = 1,
    Partial1Mode = 2,
    Partial2Mode = 3,
    DedupMode = 4,
}

/// Complete 模式常量别名（单阶段完成聚合）。
pub const CompleteMode: AggFunctionMode = AggFunctionMode::CompleteMode;
/// Final 模式常量别名（合并部分结果）。
pub const FinalMode: AggFunctionMode = AggFunctionMode::FinalMode;
/// Partial1 模式常量别名（第一阶段部分聚合）。
pub const Partial1Mode: AggFunctionMode = AggFunctionMode::Partial1Mode;
/// Partial2 模式常量别名（第二阶段部分聚合）。
pub const Partial2Mode: AggFunctionMode = AggFunctionMode::Partial2Mode;
/// Dedup 模式常量别名（去重阶段）。
pub const DedupMode: AggFunctionMode = AggFunctionMode::DedupMode;

impl AggFunctionMode {
    /// 返回模式的稳定字符串名，便于日志与 Explain。
    pub fn ToString(self) -> &'static str {
        match self {
            Self::CompleteMode => "complete",
            Self::FinalMode => "final",
            Self::Partial1Mode => "partial1",
            Self::Partial2Mode => "partial2",
            Self::DedupMode => "deduplicate",
        }
    }
}

/// 聚合函数公共基类：持有 AggFuncDesc，并提供求和更新等共享逻辑。
#[derive(Clone)]
pub struct aggFunction {
    /// 聚合函数描述符（名称、参数、模式、DISTINCT）。
    pub AggFuncDesc: crate::AggFuncDesc,
}

/// 按名称与参数构造基础聚合函数对象。
pub fn newAggFunc(name: &str, args: Vec<ExprBox>, has_distinct: bool) -> aggFunction {
    aggFunction {
        AggFuncDesc: crate::AggFuncDesc::from_runtime(name, args, has_distinct),
    }
}

impl aggFunction {
    /// 创建带可选 DISTINCT 检查器的求值上下文。
    pub fn CreateContext(&self, ctx: Arc<dyn expression::EvalContext>) -> AggEvaluateContext {
        let distinct = self
            .AggFuncDesc
            .HasDistinct
            .then(|| crate::createDistinctChecker(Arc::clone(&ctx)));
        AggEvaluateContext {
            Ctx: ctx,
            DistinctChecker: distinct,
            Count: 0,
            Value: types::Datum::default(),
            Buffer: Vec::new(),
            BufferInitialized: false,
            GotFirstRow: false,
        }
    }

    /// 清空计数/值/缓冲，并按需重建 DISTINCT 检查器。
    pub fn ResetContext(
        &self,
        ctx: Arc<dyn expression::EvalContext>,
        eval_ctx: &mut AggEvaluateContext,
    ) {
        eval_ctx.DistinctChecker = self
            .AggFuncDesc
            .HasDistinct
            .then(|| crate::createDistinctChecker(Arc::clone(&ctx)));
        eval_ctx.Ctx = ctx;
        eval_ctx.Count = 0;
        eval_ctx.Value.SetNull();
        eval_ctx.Buffer.clear();
        eval_ctx.BufferInitialized = false;
        eval_ctx.GotFirstRow = false;
    }

    /// SUM 类共享更新：跳过 NULL，DISTINCT 去重后累加并递增 Count。
    pub fn updateSum(
        &self,
        type_context: types::Context,
        eval_ctx: &mut AggEvaluateContext,
        row: chunk::Row,
    ) -> Result<(), crate::Error> {
        let value = self.AggFuncDesc.Args[0].Eval(eval_ctx.Ctx.as_ref(), row)?;
        if value.IsNull() {
            return Ok(());
        }
        // DISTINCT 且已见过该值则跳过。
        if let Some(checker) = &mut eval_ctx.DistinctChecker
            && !checker.Check(vec![value.clone()])?
        {
            return Ok(());
        }
        eval_ctx.Value = crate::calculateSum(type_context, eval_ctx.Value.clone(), value)?;
        eval_ctx.Count += 1;
        Ok(())
    }
}

/// 该聚合是否需要维护 Count 字段（COUNT/AVG）。
pub fn NeedCount(name: &str) -> bool {
    matches!(name, ast::AggFuncCount | ast::AggFuncAvg) || IsMaxMinCount(name)
}

/// Whether the aggregate counts occurrences of its maximum or minimum.
pub fn IsMaxMinCount(name: &str) -> bool {
    matches!(name, ast::AggFuncMaxCount | ast::AggFuncMinCount)
}

/// 该聚合是否需要维护 Value 字段（SUM/MAX/FIRST_ROW 等）。
pub fn NeedValue(name: &str) -> bool {
    matches!(
        name,
        ast::AggFuncSum
            | ast::AggFuncSumInt
            | ast::AggFuncAvg
            | ast::AggFuncFirstRow
            | ast::AggFuncMax
            | ast::AggFuncMin
            | ast::AggFuncMaxCount
            | ast::AggFuncMinCount
            | ast::AggFuncGroupConcat
            | ast::AggFuncBitOr
            | ast::AggFuncBitAnd
            | ast::AggFuncBitXor
            | ast::AggFuncApproxPercentile
    )
}

/// 聚合列表是否全部为 FIRST_ROW（可用于某些优化路径）。
pub fn IsAllFirstRow(functions: &[crate::AggFuncDesc]) -> bool {
    functions
        .iter()
        .all(|function| function.Name == ast::AggFuncFirstRow)
}

/// 判断聚合能否下推到指定存储类型（TiKV/TiFlash），并叠加会话开关。
pub fn CheckAggPushDown(
    ctx: &dyn expression::EvalContext,
    function: &crate::AggFuncDesc,
    store_type: kv::StoreType,
) -> bool {
    // ORDER BY 仅 GROUP_CONCAT 允许；ApproxPercentile 不下推；
    // ApproxCountDistinct 仅 TiFlash；向量类型另有限制。
    if (!function.OrderByItems.is_empty() && function.Name != ast::AggFuncGroupConcat)
        || function.Name == ast::AggFuncApproxPercentile
        || (store_type != kv::StoreType::TiFlash
            && function.Name == ast::AggFuncApproxCountDistinct)
        || !checkVectorAggPushDown(ctx, function)
    {
        return false;
    }
    if IsMaxMinCount(&function.Name)
        && (store_type != kv::StoreType::TiFlash
            || function.Args.len() != 1
            || function.Mode == DedupMode)
    {
        return false;
    }

    let supported = match store_type {
        kv::StoreType::TiFlash => CheckAggPushFlash(ctx, function),
        // TiKV 侧当前不支持 GROUP_CONCAT 下推。
        kv::StoreType::TiKV => function.Name != ast::AggFuncGroupConcat,
        _ => true,
    };
    supported && expression::IsPushDownEnabled(&function.Name, store_type)
}

/// 向量类型参数仅允许 COUNT/MIN/MAX/FIRST_ROW 等少数聚合下推。
fn checkVectorAggPushDown(
    ctx: &dyn expression::EvalContext,
    function: &crate::AggFuncDesc,
) -> bool {
    matches!(
        function.Name.as_str(),
        ast::AggFuncCount | ast::AggFuncMin | ast::AggFuncMax | ast::AggFuncFirstRow
    ) || function
        .Args
        .first()
        .is_none_or(|argument| argument.GetType(ctx).GetType() != mysql::TypeTiDBVectorFloat32)
}

/// TiFlash 下推白名单：禁止 Duration 参数；JSON 上限制 SUM/AVG/GROUP_CONCAT。
pub fn CheckAggPushFlash(ctx: &dyn expression::EvalContext, function: &crate::AggFuncDesc) -> bool {
    if function
        .Args
        .iter()
        .any(|argument| argument.GetType(ctx).GetType() == mysql::TypeDuration)
    {
        return false;
    }
    match function.Name.as_str() {
        ast::AggFuncCount
        | ast::AggFuncMin
        | ast::AggFuncMax
        | ast::AggFuncMaxCount
        | ast::AggFuncMinCount
        | ast::AggFuncFirstRow
        | ast::AggFuncApproxCountDistinct => true,
        ast::AggFuncSum | ast::AggFuncSumInt | ast::AggFuncAvg | ast::AggFuncGroupConcat => {
            function.Args[0].GetType(ctx).GetType() != mysql::TypeJSON
        }
        _ => false,
    }
}
