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
// 聚合函数规划描述符：模式、DISTINCT、ORDER BY，以及拆分/实例化运行时聚合。
//
// `AggFuncDesc` 扩展 `baseFuncDesc`，携带分布式聚合 Mode（Complete/Partial/Final）
// 与 GROUP_CONCAT 的排序项；`Split` 把一阶段拆成局部与最终两段描述。

use crate::*;

use std::any::Any;
use std::ops::{Deref, DerefMut};

use expression::base;
use expression::base::{Equals as _, Hash64 as _};
use expression::{Expression as _, StringerWithCtx as _};

/// Planner descriptor for one aggregate function.
/// 规划器侧单个聚合函数的完整描述。
#[derive(Clone)]
pub struct AggFuncDesc {
    /// 名称、参数与返回类型。
    pub baseFuncDesc: baseFuncDesc,
    /// 聚合执行模式（一阶段 Complete，或分布式 Partial/Final）。
    pub Mode: AggFunctionMode,
    /// 是否带 DISTINCT。
    pub HasDistinct: bool,
    /// GROUP_CONCAT 等使用的 ORDER BY 项。
    pub OrderByItems: Vec<planner_util::ByItems>,
    /// GROUPING 相关标识（rollup 等）。
    pub GroupingID: isize,
}

impl Deref for AggFuncDesc {
    type Target = baseFuncDesc;

    fn deref(&self) -> &Self::Target {
        &self.baseFuncDesc
    }
}

impl DerefMut for AggFuncDesc {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.baseFuncDesc
    }
}

/// 新建聚合描述，默认 Mode 为 CompleteMode。
pub fn NewAggFuncDesc(
    ctx: &dyn expression::BuildContext,
    name: &str,
    args: Vec<expression::ExprBox>,
    has_distinct: bool,
) -> Result<AggFuncDesc, Error> {
    Ok(AggFuncDesc {
        baseFuncDesc: newBaseFuncDesc(ctx, name, args)?,
        Mode: CompleteMode,
        HasDistinct: has_distinct,
        OrderByItems: Vec::new(),
        GroupingID: 0,
    })
}

/// 由窗口描述构造聚合描述（窗口内复用聚合实现时）。
pub fn NewAggFuncDescForWindowFunc(
    ctx: &dyn expression::BuildContext,
    desc: &WindowFuncDesc,
    has_distinct: bool,
) -> Result<AggFuncDesc, Error> {
    // 若窗口侧尚未推断返回类型，则重新走 base 构造与 TypeInfer。
    let base = if desc.RetTp.is_none() {
        newBaseFuncDesc(ctx, &desc.Name, desc.Args.clone())?
    } else {
        desc.baseFuncDesc.clone_desc()
    };
    Ok(AggFuncDesc {
        baseFuncDesc: base,
        Mode: CompleteMode,
        HasDistinct: has_distinct,
        OrderByItems: Vec::new(),
        GroupingID: 0,
    })
}

impl AggFuncDesc {
    /// 运行时快速构造（跳过类型推断，RetTp 为空）。
    pub(crate) fn from_runtime(
        name: &str,
        args: Vec<expression::ExprBox>,
        has_distinct: bool,
    ) -> AggFuncDesc {
        AggFuncDesc {
            baseFuncDesc: baseFuncDesc {
                Name: name.to_ascii_lowercase(),
                Args: args,
                RetTp: None,
            },
            Mode: CompleteMode,
            HasDistinct: has_distinct,
            OrderByItems: Vec::new(),
            GroupingID: 0,
        }
    }

    /// 计算描述符指纹，含 Mode / DISTINCT / ORDER BY。
    pub fn Hash64(&self, hasher: &mut dyn base::Hasher) {
        self.baseFuncDesc.Hash64(hasher);
        hasher.HashInt(self.Mode as isize);
        hasher.HashBool(self.HasDistinct);
        hasher.HashInt(self.OrderByItems.len() as isize);
        for item in &self.OrderByItems {
            item.Hash64(hasher);
        }
    }

    /// 结构相等比较（不含求值语义）。
    pub fn Equals(&self, other: &dyn Any) -> bool {
        let Some(other) = other.downcast_ref::<AggFuncDesc>() else {
            return false;
        };
        self.Mode == other.Mode
            && self.HasDistinct == other.HasDistinct
            && self.OrderByItems.len() == other.OrderByItems.len()
            && self
                .OrderByItems
                .iter()
                .zip(&other.OrderByItems)
                .all(|(left, right)| left.Equals(right))
            && self.baseFuncDesc.Equals(&other.baseFuncDesc)
    }

    /// 格式化为可读字符串（含 distinct / order by）。
    pub fn StringWithCtx(&self, ctx: Option<&dyn expression::ParamValues>, redact: &str) -> String {
        let mut result = format!("{}(", self.Name);
        if self.HasDistinct {
            result.push_str("distinct ");
        }
        for (index, argument) in self.Args.iter().enumerate() {
            if index != 0 {
                result.push_str(", ");
            }
            result.push_str(&argument.StringWithCtx(ctx, redact));
        }
        if !self.OrderByItems.is_empty() {
            result.push_str(" order by ");
            for (index, item) in self.OrderByItems.iter().enumerate() {
                if index != 0 {
                    result.push_str(", ");
                }
                result.push_str(&item.StringWithCtx(ctx, redact));
            }
        }
        result.push(')');
        result
    }

    /// 带求值上下文的语义相等。
    pub fn Equal(&self, ctx: &dyn expression::EvalContext, other: &AggFuncDesc) -> bool {
        self.HasDistinct == other.HasDistinct
            && self.OrderByItems.len() == other.OrderByItems.len()
            && self
                .OrderByItems
                .iter()
                .zip(&other.OrderByItems)
                .all(|(left, right)| left.Equal(ctx, right))
            && self.baseFuncDesc.equal(ctx, &other.baseFuncDesc)
    }

    /// 深拷贝描述符与 ORDER BY 项。
    pub fn Clone(&self) -> AggFuncDesc {
        AggFuncDesc {
            baseFuncDesc: self.baseFuncDesc.clone_desc(),
            Mode: self.Mode,
            HasDistinct: self.HasDistinct,
            OrderByItems: self
                .OrderByItems
                .iter()
                .map(planner_util::ByItems::Clone)
                .collect(),
            GroupingID: self.GroupingID,
        }
    }

    /// 将聚合拆成局部与最终两段；`ordinal` 为局部输出列在 schema 中的下标。
    pub fn Split(&self, ordinal: &[isize]) -> (AggFuncDesc, AggFuncDesc) {
        let mut partial = self.Clone();
        // Complete→Partial1，Final→Partial2，其余 Mode 保持。
        partial.Mode = match self.Mode {
            CompleteMode => Partial1Mode,
            FinalMode => Partial2Mode,
            mode => mode,
        };

        let mut final_desc = AggFuncDesc {
            baseFuncDesc: baseFuncDesc {
                Name: self.Name.clone(),
                Args: Vec::new(),
                RetTp: self.RetTp.clone(),
            },
            Mode: FinalMode,
            HasDistinct: self.HasDistinct,
            OrderByItems: Vec::new(),
            GroupingID: 0,
        };
        let column = |index: isize, field_type: types::FieldType| -> expression::ExprBox {
            Box::new(expression::Column::new(field_type, 0, 0, index))
        };
        // AVG Final 需要 (count, sum) 两列；其余多数聚合 Final 只读一列局部结果。
        match self.Name.as_str() {
            ast::AggFuncAvg => {
                final_desc.Args.push(column(
                    ordinal[0],
                    *types::NewFieldType(mysql::TypeLonglong),
                ));
                final_desc.Args.push(column(
                    ordinal[1],
                    self.RetTp
                        .clone()
                        .expect("AVG return type must be inferred"),
                ));
            }
            ast::AggFuncApproxCountDistinct => final_desc
                .Args
                .push(column(ordinal[0], *types::NewFieldType(mysql::TypeString))),
            ast::AggFuncCount if self.HasDistinct => {
                final_desc.Args = self
                    .Args
                    .iter()
                    .map(|argument| argument.CloneExpr())
                    .collect();
            }
            _ => {
                let return_type = if IsMaxMinCount(&self.Name) {
                    // Split has no caller context; expression types are fixed after
                    // descriptor construction, so a static evaluation context suffices.
                    self.Args[0]
                        .GetType(&exprstatic::NewEvalContext(Vec::new()))
                        .Clone()
                } else {
                    self.RetTp
                        .clone()
                        .expect("aggregate return type must be inferred")
                };
                final_desc.Args.push(column(ordinal[0], return_type));
                if matches!(
                    self.Name.as_str(),
                    ast::AggFuncGroupConcat | ast::AggFuncApproxPercentile
                ) {
                    final_desc.Args.push(
                        self.Args
                            .last()
                            .expect("aggregate separator/percentile argument")
                            .CloneExpr(),
                    );
                }
            }
        }
        (partial, final_desc)
    }

    /// 外连接无匹配行时，尝试常量折叠聚合空输入默认值。
    pub fn EvalNullValueInOuterJoin(
        &self,
        ctx: &mut dyn expression::BuildContext,
        schema: &expression::Schema,
    ) -> Result<(types::Datum, bool), Error> {
        match self.Name.as_str() {
            ast::AggFuncCount | ast::AggFuncMaxCount | ast::AggFuncMinCount => {
                self.evalNullValueInOuterJoin4Count(ctx, schema)
            }
            ast::AggFuncSum
            | ast::AggFuncSumInt
            | ast::AggFuncMax
            | ast::AggFuncMin
            | ast::AggFuncFirstRow => self.evalNullValueInOuterJoin4Sum(ctx, schema),
            ast::AggFuncAvg | ast::AggFuncGroupConcat => Ok((types::Datum::default(), false)),
            ast::AggFuncBitAnd => self.evalNullValueInOuterJoin4BitAnd(ctx, schema),
            ast::AggFuncBitOr | ast::AggFuncBitXor => {
                self.evalNullValueInOuterJoin4BitOr(ctx, schema)
            }
            _ => panic!("unsupported agg function {}", self.Name),
        }
    }

    /// 按函数名实例化对应运行时 `Aggregation` 实现。
    pub fn GetAggFunc(&self, ctx: &dyn expression::exprctx::ExprContext) -> Box<dyn Aggregation> {
        let aggFunction = aggFunction {
            AggFuncDesc: self.Clone(),
        };
        match self.Name.as_str() {
            ast::AggFuncSum => Box::new(sumFunction { aggFunction }),
            ast::AggFuncSumInt => Box::new(sumIntFunction { aggFunction }),
            ast::AggFuncCount => Box::new(countFunction { aggFunction }),
            ast::AggFuncAvg => Box::new(avgFunction { aggFunction }),
            ast::AggFuncGroupConcat => Box::new(concatFunction {
                aggFunction,
                separator: String::new(),
                maxLen: ctx.GetGroupConcatMaxLen(),
                sepInited: false,
                truncated: false,
            }),
            ast::AggFuncMax => Box::new(maxMinFunction {
                aggFunction,
                isMax: true,
                ctor: collate::GetCollator(self.Args[0].GetType(ctx.GetEvalCtx()).GetCollate()),
            }),
            ast::AggFuncMin => Box::new(maxMinFunction {
                aggFunction,
                isMax: false,
                ctor: collate::GetCollator(self.Args[0].GetType(ctx.GetEvalCtx()).GetCollate()),
            }),
            ast::AggFuncMaxCount | ast::AggFuncMinCount => {
                let compare_index =
                    if matches!(self.Mode, FinalMode | Partial2Mode) && self.Args.len() > 1 {
                        1
                    } else {
                        0
                    };
                Box::new(maxMinCountFunction {
                    aggFunction,
                    isMax: self.Name == ast::AggFuncMaxCount,
                    ctor: collate::GetCollator(
                        self.Args[compare_index]
                            .GetType(ctx.GetEvalCtx())
                            .GetCollate(),
                    ),
                })
            }
            ast::AggFuncFirstRow => Box::new(firstRowFunction { aggFunction }),
            ast::AggFuncBitOr => Box::new(bitOrFunction { aggFunction }),
            ast::AggFuncBitXor => Box::new(bitXorFunction { aggFunction }),
            ast::AggFuncBitAnd => Box::new(bitAndFunction { aggFunction }),
            _ => panic!("unsupported agg function {}", self.Name),
        }
    }

    /// COUNT：参数均可折叠为非 NULL 常量时返回 1，否则不可折叠。
    fn evalNullValueInOuterJoin4Count(
        &self,
        ctx: &mut dyn expression::BuildContext,
        schema: &expression::Schema,
    ) -> Result<(types::Datum, bool), Error> {
        for argument in &self.Args {
            let result = expression::EvaluateExprWithNull(ctx, schema, argument.CloneExpr(), true)?;
            let Some(constant) = result.as_any().downcast_ref::<expression::Constant>() else {
                return Ok((types::Datum::default(), false));
            };
            if constant.Value.IsNull() {
                return Ok((types::Datum::default(), true));
            }
        }
        Ok((types::NewIntDatum(1), true))
    }

    /// SUM/MAX/MIN/FirstRow：取首参在 NULL 填充 schema 下的常量值。
    fn evalNullValueInOuterJoin4Sum(
        &self,
        ctx: &mut dyn expression::BuildContext,
        schema: &expression::Schema,
    ) -> Result<(types::Datum, bool), Error> {
        let result = expression::EvaluateExprWithNull(ctx, schema, self.Args[0].CloneExpr(), true)?;
        let Some(constant) = result.as_any().downcast_ref::<expression::Constant>() else {
            return Ok((types::Datum::default(), false));
        };
        Ok((constant.Value.clone(), true))
    }

    /// BIT_AND：无法折叠或结果为 NULL 时退回全 1（`u64::MAX`）。
    fn evalNullValueInOuterJoin4BitAnd(
        &self,
        ctx: &mut dyn expression::BuildContext,
        schema: &expression::Schema,
    ) -> Result<(types::Datum, bool), Error> {
        let (value, valid) = self.evalNullValueInOuterJoin4Sum(ctx, schema)?;
        if !valid || value.IsNull() {
            return Ok((types::NewUintDatum(u64::MAX), true));
        }
        Ok((value, true))
    }

    /// BIT_OR/XOR：无法折叠或结果为 NULL 时退回 0。
    fn evalNullValueInOuterJoin4BitOr(
        &self,
        ctx: &mut dyn expression::BuildContext,
        schema: &expression::Schema,
    ) -> Result<(types::Datum, bool), Error> {
        let (value, valid) = self.evalNullValueInOuterJoin4Sum(ctx, schema)?;
        if !valid || value.IsNull() {
            return Ok((types::NewIntDatum(0), true));
        }
        Ok((value, true))
    }

    /// 按是否有 GROUP BY 等语义，决定是否从返回类型清除 NotNullFlag。
    pub fn UpdateNotNullFlag4RetType(
        &mut self,
        has_group_by: bool,
        all_aggs_first_row: bool,
    ) -> Result<(), Error> {
        let remove = match self.Name.as_str() {
            ast::AggFuncCount
            | ast::AggFuncMaxCount
            | ast::AggFuncMinCount
            | ast::AggFuncApproxCountDistinct
            | ast::AggFuncApproxPercentile
            | ast::AggFuncBitAnd
            | ast::AggFuncBitOr
            | ast::AggFuncBitXor
            | ast::WindowFuncFirstValue
            | ast::WindowFuncLastValue
            | ast::WindowFuncNthValue
            | ast::WindowFuncRowNumber
            | ast::WindowFuncRank
            | ast::WindowFuncDenseRank
            | ast::WindowFuncCumeDist
            | ast::WindowFuncNtile
            | ast::WindowFuncPercentRank
            | ast::WindowFuncLead
            | ast::WindowFuncLag
            | ast::AggFuncJsonObjectAgg
            | ast::AggFuncJsonArrayagg
            | ast::AggFuncVarSamp
            | ast::AggFuncVarPop
            | ast::AggFuncStddevPop
            | ast::AggFuncStddevSamp => false,
            ast::AggFuncSum | ast::AggFuncSumInt | ast::AggFuncAvg | ast::AggFuncGroupConcat => {
                !has_group_by
            }
            ast::AggFuncMax | ast::AggFuncMin => {
                !has_group_by
                    && self
                        .RetTp
                        .as_ref()
                        .is_some_and(|field_type| field_type.GetType() != mysql::TypeBit)
            }
            ast::AggFuncFirstRow => !all_aggs_first_row && !has_group_by,
            _ => {
                return Err(expression::errors::New(format!(
                    "unsupported agg function: {}",
                    self.Name
                )));
            }
        };
        if remove && let Some(field_type) = &mut self.RetTp {
            field_type.DelFlag(mysql::NotNullFlag);
        }
        Ok(())
    }

    /// 估算描述符内存占用（含 ORDER BY 项）。
    pub fn MemoryUsage(&self) -> i64 {
        self.baseFuncDesc.MemoryUsage()
            + std::mem::size_of::<isize>() as i64
            + std::mem::size_of::<bool>() as i64
            + self
                .OrderByItems
                .iter()
                .map(planner_util::ByItems::MemoryUsage)
                .sum::<i64>()
    }
}
