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
// 聚合/窗口函数基础描述：签名、类型推断、默认值与参数 cast 包装。
//
// 优化器侧用 `baseFuncDesc` 表示函数名、参数表达式与返回类型；
// `TypeInfer*` 系列按 MySQL 语义推导 RetTp，供后续执行与下推使用。

use crate::*;

use std::any::Any;
use std::collections::HashSet;

use expression::base;
use expression::base::HashEquals as _;
use expression::{Expression as _, StringerWithCtx as _};

/// 规划器使用的函数签名（名称、参数、返回类型），聚合与窗口共用。
// baseFuncDesc describes an function signature, only used in planner.
#[derive(Clone)]
pub struct baseFuncDesc {
    /// 小写函数名（如 `avg`、`row_number`）。
    // Name represents the function name.
    pub Name: String,
    /// 参数表达式列表。
    // Args represents the arguments of the function.
    pub Args: Vec<expression::ExprBox>,
    /// 推断后的返回字段类型；构造后通常非空。
    // RetTp represents the return type of the function.
    pub RetTp: Option<types::FieldType>,
}

/// 构造基础描述并立即做类型推断。
// newBaseFuncDesc 对应 Go 的构造函数：函数名先转小写，再立即执行 TypeInfer。
pub fn newBaseFuncDesc(
    ctx: &dyn expression::BuildContext,
    name: &str,
    args: Vec<expression::ExprBox>,
) -> Result<baseFuncDesc, Error> {
    let mut b = baseFuncDesc {
        Name: name.to_ascii_lowercase(),
        Args: args,
        RetTp: None,
    };
    b.TypeInfer(ctx)?;
    Ok(b)
}

impl baseFuncDesc {
    /// 将名称、参数与返回类型写入 Hasher（计划缓存 / 等价判定用）。
    // Hash64 implements the base.Hasher interface.
    pub fn Hash64(&self, h: &mut dyn base::Hasher) {
        h.HashString(&self.Name);
        h.HashInt(self.Args.len() as isize);
        for arg in &self.Args {
            arg.Hash64(h);
        }
        if let Some(ret_tp) = &self.RetTp {
            h.HashByte(base::NotNilFlag);
            h.HashByte(ret_tp.GetType());
            h.HashUint64(ret_tp.GetFlag() as u64);
            h.HashInt(ret_tp.GetFlen());
            h.HashInt(ret_tp.GetDecimal());
            h.HashString(ret_tp.GetCharset());
            h.HashString(ret_tp.GetCollate());
            h.HashInt(ret_tp.GetElems().len() as isize);
            for element in ret_tp.GetElems() {
                h.HashString(element);
            }
            h.HashInt(ret_tp.GetElemsIsBinaryLit().len() as isize);
            for is_binary_literal in ret_tp.GetElemsIsBinaryLit() {
                h.HashBool(*is_binary_literal);
            }
            h.HashBool(ret_tp.IsArray());
        } else {
            h.HashByte(base::NilFlag);
        }
    }

    /// 结构化相等：名称、参数列表与返回类型均一致。
    // Equals implements the base.Equals interface.
    pub fn Equals(&self, other: &dyn Any) -> bool {
        let Some(a2) = other.downcast_ref::<baseFuncDesc>() else {
            return false;
        };
        let ret_equal = match (&self.RetTp, &a2.RetTp) {
            (None, None) => true,
            (Some(left), Some(right)) => left.Equals(right),
            _ => false,
        };
        let mut ok = self.Name == a2.Name && self.Args.len() == a2.Args.len() && ret_equal;
        if !ok {
            return false;
        }
        for (i, arg) in self.Args.iter().enumerate() {
            if !arg.Equals(a2.Args[i].as_any()) {
                ok = false;
                break;
            }
        }
        ok
    }

    /// 带求值上下文的语义相等（依赖 EvalContext 比较表达式内部）。
    // equal 对应 Go 的 planner 表达式相等判断，依赖 EvalContext 处理表达式内部语义比较。
    pub fn equal(&self, ctx: &dyn expression::EvalContext, other: &baseFuncDesc) -> bool {
        if self.Name != other.Name || self.Args.len() != other.Args.len() {
            return false;
        }
        for i in 0..self.Args.len() {
            if !self.Args[i].Equal(ctx, other.Args[i].as_ref()) {
                return false;
            }
        }
        true
    }

    /// 深拷贝：RetTp 与每个参数表达式均 clone，避免后续类型改写串扰。
    // clone 对应 Go 的深拷贝：返回类型和每个参数表达式都 clone，避免后续类型改写串扰。
    pub fn clone(&self) -> baseFuncDesc {
        let mut cloned = <Self as Clone>::clone(self);
        cloned.RetTp = self.RetTp.as_ref().map(|field_type| field_type.Clone());
        cloned.Args = self
            .Args
            .iter()
            .map(|argument| argument.CloneExpr())
            .collect();
        cloned
    }

    /// `clone` 的别名，便于与窗口描述符等 API 对齐。
    pub fn clone_desc(&self) -> baseFuncDesc {
        self.clone()
    }

    /// 格式化为 `name(arg0, arg1, ...)`，可按上下文脱敏参数。
    // StringWithCtx returns the string within given context.
    pub fn StringWithCtx(&self, ctx: Option<&dyn expression::ParamValues>, redact: &str) -> String {
        let mut buffer = format!("{}(", self.Name);
        for (i, arg) in self.Args.iter().enumerate() {
            buffer.push_str(&arg.StringWithCtx(ctx, redact));
            if i + 1 != self.Args.len() {
                buffer.push_str(", ");
            }
        }
        buffer.push(')');
        buffer
    }

    /// 按函数名分派到各 `typeInfer4*`，推断参数与返回类型。
    // TypeInfer infers the arguments and return types of an function.
    pub fn TypeInfer(&mut self, ctx: &dyn expression::BuildContext) -> Result<(), Error> {
        match self.Name.as_str() {
            ast::AggFuncCount => self.typeInfer4Count(),
            ast::AggFuncApproxCountDistinct => self.typeInfer4ApproxCountDistinct(),
            ast::AggFuncApproxPercentile => {
                return self.typeInfer4ApproxPercentile(ctx.GetEvalCtx());
            }
            ast::AggFuncSum => self.typeInfer4Sum(ctx.GetEvalCtx()),
            ast::AggFuncSumInt => return self.typeInfer4SumInt(ctx.GetEvalCtx()),
            ast::AggFuncAvg => self.typeInfer4Avg(ctx.GetEvalCtx()),
            ast::AggFuncGroupConcat => return self.typeInfer4GroupConcat(ctx),
            ast::AggFuncMax
            | ast::AggFuncMin
            | ast::AggFuncFirstRow
            | ast::WindowFuncFirstValue
            | ast::WindowFuncLastValue
            | ast::WindowFuncNthValue => self.typeInfer4MaxMin(ctx),
            ast::AggFuncBitAnd | ast::AggFuncBitOr | ast::AggFuncBitXor => {
                self.typeInfer4BitFuncs(ctx)
            }
            ast::WindowFuncRowNumber | ast::WindowFuncRank | ast::WindowFuncDenseRank => {
                self.typeInfer4NumberFuncs()
            }
            ast::WindowFuncCumeDist => self.typeInfer4CumeDist(),
            ast::WindowFuncNtile => self.typeInfer4Ntile(),
            ast::WindowFuncPercentRank => self.typeInfer4PercentRank(),
            ast::WindowFuncLead | ast::WindowFuncLag => return self.typeInfer4LeadLag(ctx),
            ast::AggFuncVarPop
            | ast::AggFuncStddevPop
            | ast::AggFuncVarSamp
            | ast::AggFuncStddevSamp => self.typeInfer4PopOrSamp(),
            ast::AggFuncJsonArrayagg => self.typeInfer4JsonArrayAgg(),
            ast::AggFuncJsonObjectAgg => return self.typeInfer4JsonObjectAgg(ctx),
            _ => {
                return Err(expression::errors::New(format!(
                    "unsupported agg function: {}",
                    self.Name
                )));
            }
        }
        Ok(())
    }

    // typeInfer4Count 设置 COUNT 的固定 longlong 返回类型，并标记非空二进制 charset/collation。
    pub fn typeInfer4Count(&mut self) {
        let mut ret_tp = *types::NewFieldType(mysql::TypeLonglong);
        ret_tp.SetFlen(21);
        ret_tp.SetDecimal(0);
        // count never returns null
        ret_tp.AddFlag(mysql::NotNullFlag);
        types::SetBinChsClnFlag(&mut ret_tp);
        self.RetTp = Some(ret_tp);
    }

    // typeInfer4ApproxCountDistinct 当前完全复用 COUNT 的返回类型推断。
    pub fn typeInfer4ApproxCountDistinct(&mut self) {
        self.typeInfer4Count();
    }

    // typeInfer4ApproxPercentile 对应 APPROX_PERCENTILE 参数个数、百分位常量和值域检查，再按第一个参数类型确定返回类型。
    pub fn typeInfer4ApproxPercentile(
        &mut self,
        ctx: &dyn expression::EvalContext,
    ) -> Result<(), Error> {
        if self.Args.len() != 2 {
            return Err(expression::errors::New(
                "APPROX_PERCENTILE should take 2 arguments",
            ));
        }

        if self.Args[1].ConstLevel() == expression::ConstNone {
            return Err(expression::errors::New(
                "APPROX_PERCENTILE should take a constant expression as percentage argument",
            ));
        }
        let (percent, isNull) = self.Args[1]
            .EvalInt(ctx, chunk::Row::default())
            .map_err(|_| {
                expression::errors::New(format!(
                    "APPROX_PERCENTILE: Invalid argument {}",
                    self.Args[1].StringWithCtx(None, expression::errors::RedactLogDisable)
                ))
            })?;
        if percent <= 0 || percent > 100 || isNull {
            if isNull {
                return Err(expression::errors::New(
                    "APPROX_PERCENTILE: Percentage value cannot be NULL",
                ));
            }
            return Err(expression::errors::New(format!(
                "Percentage value {} is out of range [1, 100]",
                percent
            )));
        }

        match self.Args[0].GetType(ctx).GetType() {
            mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong => {
                self.RetTp = Some(*types::NewFieldType(mysql::TypeLonglong));
            }
            mysql::TypeDouble | mysql::TypeFloat => {
                self.RetTp = Some(*types::NewFieldType(mysql::TypeDouble));
            }
            mysql::TypeNewDecimal => {
                let mut ret_tp = *types::NewFieldType(mysql::TypeNewDecimal);
                ret_tp.SetFlen(mysql::MaxDecimalWidth as isize);
                ret_tp.SetDecimal(self.Args[0].GetType(ctx).GetDecimal());
                if ret_tp.GetDecimal() < 0 || ret_tp.GetDecimal() > mysql::MaxDecimalScale as isize
                {
                    ret_tp.SetDecimal(mysql::MaxDecimalScale as isize);
                }
                self.RetTp = Some(ret_tp);
            }
            mysql::TypeDate | mysql::TypeDatetime | mysql::TypeNewDate | mysql::TypeTimestamp => {
                self.RetTp = Some(self.Args[0].GetType(ctx).Clone());
            }
            _ => {
                let mut ret_tp = self.Args[0].GetType(ctx).Clone();
                ret_tp.DelFlag(mysql::NotNullFlag);
                self.RetTp = Some(ret_tp);
            }
        }
        Ok(())
    }

    // typeInfer4Sum should return a "decimal", otherwise it returns a "double".
    // Because child returns integer or decimal type.
    pub fn typeInfer4Sum(&mut self, ctx: &dyn expression::EvalContext) {
        let arg_tp = self.Args[0].GetType(ctx);
        let mut ret_tp = match arg_tp.GetType() {
            mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeYear => {
                let mut tp = *types::NewFieldType(mysql::TypeNewDecimal);
                tp.SetFlenUnderLimit(arg_tp.GetFlen() + 21);
                tp.SetDecimal(0);
                if arg_tp.GetFlen() < 0 {
                    tp.SetFlen(mysql::MaxDecimalWidth as isize);
                }
                tp
            }
            mysql::TypeNewDecimal => {
                let mut tp = *types::NewFieldType(mysql::TypeNewDecimal);
                tp.UpdateFlenAndDecimalUnderLimit(arg_tp, 0, 22);
                tp
            }
            mysql::TypeDouble | mysql::TypeFloat => {
                let mut tp = *types::NewFieldType(mysql::TypeDouble);
                tp.SetFlen(mysql::MaxRealWidth as isize);
                tp.SetDecimal(self.Args[0].GetType(ctx).GetDecimal());
                tp
            }
            _ => {
                let mut tp = *types::NewFieldType(mysql::TypeDouble);
                tp.SetFlen(mysql::MaxRealWidth as isize);
                tp.SetDecimal(types::UnspecifiedLength as isize);
                tp
            }
        };
        types::SetBinChsClnFlag(&mut ret_tp);
        self.RetTp = Some(ret_tp);
    }

    // typeInfer4SumInt 校验 sum_int 只有一个整数参数，并继承无符号标记。
    pub fn typeInfer4SumInt(&mut self, ctx: &dyn expression::EvalContext) -> Result<(), Error> {
        if self.Args.len() != 1 {
            return Err(expression::errors::New("sum_int should take 1 argument"));
        }
        let arg_tp = self.Args[0].GetType(ctx);
        match arg_tp.GetType() {
            mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong => {}
            _ => {
                return Err(expression::errors::New(
                    "sum_int only accepts integer arguments",
                ));
            }
        }
        let mut ret_tp = *types::NewFieldType(mysql::TypeLonglong);
        ret_tp.SetFlen(21);
        ret_tp.SetDecimal(0);
        if mysql::HasUnsignedFlag(arg_tp.GetFlag()) {
            ret_tp.AddFlag(mysql::UnsignedFlag);
        }
        types::SetBinChsClnFlag(&mut ret_tp);
        self.RetTp = Some(ret_tp);
        Ok(())
    }

    // TypeInfer4AvgSum infers the type of sum from avg, which should extend the precision of decimal
    // compatible with mysql.
    pub fn TypeInfer4AvgSum(
        &mut self,
        ctx: &dyn expression::EvalContext,
        avgRetType: &types::FieldType,
    ) -> Result<(), Error> {
        if self.Name != ast::AggFuncSum {
            return Err(expression::errors::New(format!(
                "expect sum func, but got {}",
                self.Name
            )));
        }
        // Handling column and scalar function differently to avoid breaking a MySQL compatible issue.
        // Check: https://github.com/pingcap/tidb/blob/67edd7d8f73de399bd72490d449d1dede1ee637b/pkg/executor/test/tiflashtest/tiflash_test.go#L887
        // For avg(div(col1, col2)), the scale of div result should be same as the scale of avg, which has been increased by 4, to make sure the result is compatible with MySQL.
        // But for avg(col1), there is no need to increase the result scale of partial sum, because there is no complex scale upgrade for a simple column.
        if self.Args[0].as_any().is::<expression::Column>() {
            self.typeInfer4Sum(ctx);
        } else if avgRetType.GetType() == mysql::TypeNewDecimal {
            // Go 依赖 RetTp 已由调用方提前设置；这里保持对现有 RetTp 的就地精度扩展。
            if let Some(ret_tp) = &mut self.RetTp {
                ret_tp.SetFlen(std::cmp::min(
                    mysql::MaxDecimalWidth as isize,
                    ret_tp.GetFlen() + 22,
                ));
            }
        }
        Ok(())
    }

    // TypeInfer4FinalCount infers the type of sum agg which is rewritten from final count agg run on MPP mode.
    pub fn TypeInfer4FinalCount(&mut self, finalCountRetType: &types::FieldType) {
        self.RetTp = Some(finalCountRetType.Clone());
    }

    // typeInfer4Avg should returns a "decimal", otherwise it returns a "double".
    // Because child returns integer or decimal type.
    pub fn typeInfer4Avg(&mut self, ctx: &dyn expression::EvalContext) {
        let divPrecIncre = ctx.GetDivPrecisionIncrement() as isize;
        let arg_tp = self.Args[0].GetType(ctx);
        let mut ret_tp = match arg_tp.GetType() {
            mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong => {
                let mut tp = *types::NewFieldType(mysql::TypeNewDecimal);
                tp.SetDecimalUnderLimit(divPrecIncre);
                let (flen, _) = mysql::GetDefaultFieldLengthAndDecimal(arg_tp.GetType());
                tp.SetFlenUnderLimit(flen + divPrecIncre);
                tp
            }
            mysql::TypeYear | mysql::TypeNewDecimal => {
                let mut tp = *types::NewFieldType(mysql::TypeNewDecimal);
                tp.UpdateFlenAndDecimalUnderLimit(arg_tp, divPrecIncre, divPrecIncre);
                tp
            }
            mysql::TypeDouble | mysql::TypeFloat => {
                let mut tp = *types::NewFieldType(mysql::TypeDouble);
                tp.SetFlen(mysql::MaxRealWidth as isize);
                tp.SetDecimal(self.Args[0].GetType(ctx).GetDecimal());
                tp
            }
            mysql::TypeDate | mysql::TypeDuration | mysql::TypeDatetime | mysql::TypeTimestamp => {
                let mut tp = *types::NewFieldType(mysql::TypeDouble);
                tp.SetFlen(mysql::MaxRealWidth as isize);
                tp.SetDecimal(4);
                tp
            }
            _ => {
                let mut tp = *types::NewFieldType(mysql::TypeDouble);
                tp.SetFlen(mysql::MaxRealWidth as isize);
                tp.SetDecimal(types::UnspecifiedLength as isize);
                tp
            }
        };
        types::SetBinChsClnFlag(&mut ret_tp);
        self.RetTp = Some(ret_tp);
    }

    // typeInfer4GroupConcat 推导 GROUP_CONCAT 的字符串返回类型、字符集/排序规则，并对 decimal 参数补 cast。
    pub fn typeInfer4GroupConcat(
        &mut self,
        ctx: &dyn expression::BuildContext,
    ) -> Result<(), Error> {
        let mut ret_tp = *types::NewFieldType(mysql::TypeVarString);
        let arguments = self
            .Args
            .iter()
            .map(|argument| argument.as_ref())
            .collect::<Vec<_>>();
        let mut ec = expression::CheckAndDeriveCollationFromExprs(
            ctx,
            ast::AggFuncGroupConcat,
            types::ETString,
            &arguments,
        )?;
        if ec.Charset.is_empty() || ec.Collation.is_empty() {
            let (mut connCharset, mut connCollation) = ctx.GetCharsetInfo();
            if connCharset.is_empty() || connCollation.is_empty() {
                (connCharset, connCollation) = expression::charset::GetDefaultCharsetAndCollate();
            }
            if ec.Charset.is_empty() {
                ec.Charset = connCharset.clone();
            }
            if ec.Collation.is_empty() {
                if ec.Charset == connCharset {
                    ec.Collation = connCollation.clone();
                } else if let Ok(coll) = expression::charset::GetDefaultCollation(&ec.Charset) {
                    ec.Collation = coll;
                } else {
                    ec.Collation = connCollation;
                }
            }
        }
        ret_tp.SetCharset(ec.Charset.clone());
        ret_tp.SetCollate(ec.Collation.clone());

        ret_tp.SetFlen(mysql::MaxBlobWidth as isize);
        ret_tp.SetDecimal(0);
        self.RetTp = Some(ret_tp);
        // Go 的 range len(a.Args)-1 会跳过最后一个 separator 参数，只处理需要拼接的表达式参数。
        for i in 0..self.Args.len().saturating_sub(1) {
            let tp = self.Args[i].GetType(ctx.GetEvalCtx()).clone();
            if tp.GetType() == mysql::TypeNewDecimal {
                self.Args[i] =
                    expression::formal_registry::BuildCastFunction(ctx, &self.Args[i], &tp);
            }
        }
        Ok(())
    }

    // typeInfer4MaxMin 复用第一个参数的类型；max/min/lead/lag 会 clone 并清除 NotNullFlag。
    pub fn typeInfer4MaxMin(&mut self, ctx: &dyn expression::BuildContext) {
        let argIsScalaFunc = self.Args[0].as_any().is::<expression::ScalarFunction>();
        if argIsScalaFunc && self.Args[0].GetType(ctx.GetEvalCtx()).GetType() == mysql::TypeFloat {
            // For scalar function, the result of "float32" is set to the "float64"
            // field in the "Datum". If we do not wrap a cast-as-double function on a.Args[0],
            // error would happen when extracting the evaluation of a.Args[0] to a ProjectionExec.
            let mut tp = *types::NewFieldType(mysql::TypeDouble);
            tp.SetFlen(mysql::MaxRealWidth as isize);
            tp.SetDecimal(types::UnspecifiedLength as isize);
            types::SetBinChsClnFlag(&mut tp);
            self.Args[0] = expression::formal_registry::BuildCastFunction(ctx, &self.Args[0], &tp);
        }
        self.RetTp = Some(self.Args[0].GetType(ctx.GetEvalCtx()).clone());
        if self.Name == ast::AggFuncMax
            || self.Name == ast::AggFuncMin
            || self.Name == ast::WindowFuncLead
            || self.Name == ast::WindowFuncLag
        {
            let mut ret_tp = self.Args[0].GetType(ctx.GetEvalCtx()).Clone();
            ret_tp.DelFlag(mysql::NotNullFlag);
            self.RetTp = Some(ret_tp);
        }
        // issue #13027, #13961
        if let Some(ret_tp) = &self.RetTp {
            if (ret_tp.GetType() == mysql::TypeEnum || ret_tp.GetType() == mysql::TypeSet)
                && (self.Name != ast::AggFuncFirstRow
                    && self.Name != ast::AggFuncMax
                    && self.Name != ast::AggFuncMin)
            {
                let mut return_type = *types::NewFieldType(mysql::TypeString);
                return_type.SetFlen(mysql::MaxFieldCharLength as isize);
                self.RetTp = Some(return_type);
            }
        }
    }

    // typeInfer4BitFuncs 把 bit_and/bit_or/bit_xor 参数 cast 成 int，并返回 unsigned not-null longlong。
    pub fn typeInfer4BitFuncs(&mut self, ctx: &dyn expression::BuildContext) {
        let mut ret_tp = *types::NewFieldType(mysql::TypeLonglong);
        ret_tp.SetFlen(21);
        types::SetBinChsClnFlag(&mut ret_tp);
        ret_tp.AddFlag(mysql::UnsignedFlag | mysql::NotNullFlag);
        self.RetTp = Some(ret_tp);
        self.Args[0] = expression::WrapWithCastAsInt(ctx, self.Args[0].clone(), None);
    }

    // typeInfer4JsonArrayAgg 返回 JSON 类型，并保持二进制 charset/collation 标记。
    pub fn typeInfer4JsonArrayAgg(&mut self) {
        let mut ret_tp = *types::NewFieldType(mysql::TypeJSON);
        types::SetBinChsClnFlag(&mut ret_tp);
        self.RetTp = Some(ret_tp);
    }

    // typeInfer4JsonObjectAgg 返回 JSON 类型，并把 key 参数 cast 为 string。
    pub fn typeInfer4JsonObjectAgg(
        &mut self,
        ctx: &dyn expression::BuildContext,
    ) -> Result<(), Error> {
        let mut ret_tp = *types::NewFieldType(mysql::TypeJSON);
        types::SetBinChsClnFlag(&mut ret_tp);
        self.RetTp = Some(ret_tp);
        self.Args[0] = expression::WrapWithCastAsString(ctx, self.Args[0].clone());
        Ok(())
    }

    // typeInfer4NumberFuncs 推导 row_number/rank/dense_rank 的 longlong 返回类型。
    pub fn typeInfer4NumberFuncs(&mut self) {
        let mut ret_tp = *types::NewFieldType(mysql::TypeLonglong);
        ret_tp.SetFlen(21);
        types::SetBinChsClnFlag(&mut ret_tp);
        self.RetTp = Some(ret_tp);
    }

    // typeInfer4CumeDist 推导 cume_dist 的 double 返回类型。
    pub fn typeInfer4CumeDist(&mut self) {
        let mut ret_tp = *types::NewFieldType(mysql::TypeDouble);
        ret_tp.SetFlen(mysql::MaxRealWidth as isize);
        ret_tp.SetDecimal(mysql::NotFixedDec as isize);
        self.RetTp = Some(ret_tp);
    }

    // typeInfer4Ntile 推导 ntile 的 unsigned longlong 返回类型。
    pub fn typeInfer4Ntile(&mut self) {
        let mut ret_tp = *types::NewFieldType(mysql::TypeLonglong);
        ret_tp.SetFlen(21);
        types::SetBinChsClnFlag(&mut ret_tp);
        ret_tp.AddFlag(mysql::UnsignedFlag);
        self.RetTp = Some(ret_tp);
    }

    // typeInfer4PercentRank 推导 percent_rank 的 double 返回类型，保留 Go 中 SetFlag(MaxRealWidth) 的调用形状。
    pub fn typeInfer4PercentRank(&mut self) {
        let mut ret_tp = *types::NewFieldType(mysql::TypeDouble);
        ret_tp.SetFlag(mysql::MaxRealWidth);
        ret_tp.SetDecimal(mysql::NotFixedDec as isize);
        self.RetTp = Some(ret_tp);
    }

    // typeInfer4LeadLag 合并 lead/lag 的第一、第三参数类型；少于三个参数时退回 max/min 推断。
    pub fn typeInfer4LeadLag(&mut self, ctx: &dyn expression::BuildContext) -> Result<(), Error> {
        if self.Args.len() < 3 {
            self.typeInfer4MaxMin(ctx);
        } else {
            // Merge the type of first and third argument.
            // FIXME: select lead(b collate utf8mb4_unicode_ci, 1, 'lead' collate utf8mb4_general_ci) over() as a from t; should report error.
            let ret_tp = expression::InferType4ControlFuncs(
                ctx,
                &self.Name,
                self.Args[0].as_ref(),
                self.Args[2].as_ref(),
            )?;
            self.RetTp = Some(ret_tp);
        }
        Ok(())
    }

    // typeInfer4PopOrSamp 推导 var/stddev 族函数的 double 返回类型。
    pub fn typeInfer4PopOrSamp(&mut self) {
        let mut ret_tp = *types::NewFieldType(mysql::TypeDouble);
        ret_tp.SetFlen(mysql::MaxRealWidth as isize);
        ret_tp.SetDecimal(types::UnspecifiedLength as isize);
        self.RetTp = Some(ret_tp);
    }

    /// 空输入组上的默认聚合结果（如 COUNT→0、BIT_AND→u64::MAX、AVG→NULL）。
    // GetDefaultValue gets the default value when the function's input is null.
    // According to MySQL, default values of the function are listed as follows:
    // e.g.
    // Table t which is empty:
    // +-------+---------+---------+
    // | Table | Field   | Type    |
    // +-------+---------+---------+
    // | t     | a       | int(11) |
    // +-------+---------+---------+
    //
    // Query: `select avg(a), sum(a), count(a), bit_xor(a), bit_or(a), bit_and(a), max(a), std::cmp::min(a), group_concat(a), approx_count_distinct(a), approx_percentile(a, 50) from test.t;`
    // +--------+--------+----------+------------+-----------+----------------------+--------+--------+-----------------+--------------------------+--------------------------+
    // | avg(a) | sum(a) | count(a) | bit_xor(a) | bit_or(a) | bit_and(a)           | max(a) | std::cmp::min(a) | group_concat(a) | approx_count_distinct(a) | approx_percentile(a, 50) |
    // +--------+--------+----------+------------+-----------+----------------------+--------+--------+-----------------+--------------------------+--------------------------+
    // |   NULL |   NULL |        0 |          0 |         0 | 18446744073709551615 |   NULL |   NULL | NULL            |                        0 |                     NULL |
    // +--------+--------+----------+------------+-----------+----------------------+--------+--------+-----------------+--------------------------+--------------------------+
    pub fn GetDefaultValue(&self) -> types::Datum {
        let mut v = types::Datum::default();
        match self.Name.as_str() {
            ast::AggFuncCount | ast::AggFuncBitOr | ast::AggFuncBitXor => {
                v = types::NewIntDatum(0);
            }
            ast::AggFuncApproxCountDistinct => {
                // approx_count_distinct 只有非 string 返回类型才使用 0；string 场景保留空 Datum。
                if self.RetTp.as_ref().map(|tp| tp.GetType()) != Some(mysql::TypeString) {
                    v = types::NewIntDatum(0);
                }
            }
            ast::AggFuncFirstRow
            | ast::AggFuncAvg
            | ast::AggFuncSum
            | ast::AggFuncSumInt
            | ast::AggFuncMax
            | ast::AggFuncMin
            | ast::AggFuncGroupConcat
            | ast::AggFuncApproxPercentile => {
                v = types::Datum::default();
            }
            ast::AggFuncBitAnd => {
                v = types::NewUintDatum(u64::MAX);
            }
            _ => {}
        }
        v
    }

    /// 按返回 EvalType 为参数包 cast；LEAD/LAG/NTH_VALUE 的偏移参数跳过。
    // WrapCastForAggArgs wraps the args of an aggregate function with a cast function.
    pub fn WrapCastForAggArgs(&mut self, ctx: &dyn expression::BuildContext) {
        if self.Args.is_empty() {
            return;
        }
        if noNeedCastAggFuncs().contains(self.Name.as_str()) {
            return;
        }

        // Rust 用枚举保留 Go 函数变量的延迟 cast 选择语义。
        let ret_tp = self
            .RetTp
            .clone()
            .expect("RetTp must be inferred before wrapping aggregate args");
        let cast_kind = match ret_tp.EvalType() {
            types::ETInt => AggCastKind::Int(ret_tp.clone()),
            types::ETReal => AggCastKind::Real,
            types::ETString => AggCastKind::String,
            types::ETDecimal => AggCastKind::Decimal,
            types::ETDatetime | types::ETTimestamp => AggCastKind::Time(ret_tp.clone()),
            types::ETDuration => AggCastKind::Duration,
            types::ETJson => AggCastKind::Json,
            types::ETVectorFloat32 => AggCastKind::VectorFloat32,
            _ => panic!("unsupported type {} during evaluation", ret_tp.EvalType()),
        };

        for i in 0..self.Args.len() {
            // Do not cast the second args of these functions, as they are simply non-negative numbers.
            if i == 1
                && (self.Name == ast::WindowFuncLead
                    || self.Name == ast::WindowFuncLag
                    || self.Name == ast::WindowFuncNthValue)
            {
                continue;
            }
            if self.Args[i].GetType(ctx.GetEvalCtx()).GetType() == mysql::TypeNull {
                continue;
            }
            self.Args[i] = castAggArg(ctx, self.Args[i].clone(), &cast_kind);
        }
    }

    /// 估算描述符内存：名称、RetTp 元数据与各参数表达式。
    // MemoryUsage return the memory usage of baseFuncDesc
    pub fn MemoryUsage(&self) -> i64 {
        let mut sum = size::SizeOfString + self.Name.len() as i64;
        if let Some(ret_tp) = &self.RetTp {
            sum += std::mem::size_of::<types::FieldType>() as i64
                + ret_tp.GetCharset().len() as i64
                + ret_tp.GetCollate().len() as i64
                + ret_tp
                    .GetElems()
                    .iter()
                    .map(|item| item.len() as i64)
                    .sum::<i64>();
        }
        for expr in &self.Args {
            sum += expr.MemoryUsage();
        }
        sum
    }
}

/// 无需 WrapCast 的聚合名集合（求值路径已由参数自身类型决定）。
// We do not need to wrap cast upon these functions,
// since the EvalXXX method called by the arg is determined by the corresponding arg type.
pub fn noNeedCastAggFuncs() -> HashSet<&'static str> {
    HashSet::from([
        ast::AggFuncCount,
        ast::AggFuncApproxCountDistinct,
        ast::AggFuncApproxPercentile,
        ast::AggFuncMax,
        ast::AggFuncMin,
        ast::AggFuncFirstRow,
        ast::WindowFuncNtile,
        ast::AggFuncJsonArrayagg,
        ast::AggFuncJsonObjectAgg,
    ])
}

/// WrapCastForAggArgs 选用的 cast 类别（部分变体携带目标 FieldType）。
// AggCastKind 记录 WrapCastForAggArgs 应调用的 expression cast 类别。
pub enum AggCastKind {
    Int(types::FieldType),
    Real,
    String,
    Decimal,
    Time(types::FieldType),
    Duration,
    Json,
    VectorFloat32,
}

/// 按 `AggCastKind` 调用对应 WrapWithCast / BuildCastFunction。
// castAggArg 对应 Go 中被赋给 castFunc 的各个闭包；需要目标 FieldType 的 cast 分支把 ret_tp 传入。
pub fn castAggArg(
    ctx: &dyn expression::BuildContext,
    expr: expression::ExprBox,
    cast_kind: &AggCastKind,
) -> expression::ExprBox {
    match cast_kind {
        AggCastKind::Int(ret_tp) => expression::WrapWithCastAsInt(ctx, expr, Some(ret_tp)),
        AggCastKind::Real => expression::WrapWithCastAsReal(ctx, expr),
        AggCastKind::String => expression::WrapWithCastAsString(ctx, expr),
        AggCastKind::Decimal => expression::WrapWithCastAsDecimal(ctx, expr),
        AggCastKind::Time(ret_tp) => expression::WrapWithCastAsTime(ctx, expr, ret_tp.clone()),
        AggCastKind::Duration => expression::formal_registry::BuildCastFunction(
            ctx,
            &expr,
            &*types::NewFieldType(mysql::TypeDuration),
        ),
        AggCastKind::Json => expression::formal_registry::BuildCastFunction(
            ctx,
            &expr,
            &*types::NewFieldType(mysql::TypeJSON),
        ),
        AggCastKind::VectorFloat32 => expression::formal_registry::BuildCastFunction(
            ctx,
            &expr,
            &*types::NewFieldType(mysql::TypeTiDBVectorFloat32),
        ),
    }
}
