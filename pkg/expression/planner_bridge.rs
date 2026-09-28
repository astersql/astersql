// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Formal expression APIs consumed by the planner rewriter.
//
// 规划器 rewriter 使用的形式化表达式桥接层。
//
// 提供 GROUPING / FTS MATCH...AGAINST / JSON_SUM_CRC32 等规划期签名，
// 以及比较类型精化、BETWEEN/控制函数类型推导、常量精化与默认时间值解析。

use std::any::Any;
use std::collections::HashSet;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use chrono::{Datelike, Timelike};
use protobuf::{Message, ProtobufEnum};
use types_dependency::json_functions::{
    JSONTypeCodeArray, JSONTypeCodeDatetime, JSONTypeCodeDuration, JSONTypeCodeFloat64,
    JSONTypeCodeInt64, JSONTypeCodeString, JSONTypeCodeUint64,
};

use crate::{
    BuildContext, Coercibility, CollationInfo, Constant, Error, EvalContext, ExprBox, Expression,
    Repertoire, ScalarFunction, ast, builtinFunc, charset, chunk, collate, collationInfo, errors,
    mysql, opcode, types,
};

pub use tipb::GroupingMode as PlannerGroupingMode;

/// 规划期 builtin 共用底座：参数、返回类型、PB 码、排序器与会话共享标志。
struct PlannerBuiltinBase {
    args: Vec<ExprBox>,
    return_type: types::FieldType,
    pb_code: i32,
    collator: Box<dyn collate::Collator>,
    collation_info: collationInfo,
    share_flag: std::sync::Arc<AtomicU32>,
}

impl Clone for PlannerBuiltinBase {
    fn clone(&self) -> Self {
        Self {
            args: self.args.clone(),
            return_type: self.return_type.clone(),
            pb_code: self.pb_code,
            collator: collate::GetCollator(self.return_type.GetCollate()),
            collation_info: self.collation_info.clone(),
            share_flag: std::sync::Arc::new(AtomicU32::new(self.share_flag.load(Ordering::SeqCst))),
        }
    }
}

impl PlannerBuiltinBase {
    /// 按返回类型排序规则初始化 collator。
    fn new(args: Vec<ExprBox>, return_type: types::FieldType, pb_code: i32) -> Self {
        Self {
            collator: collate::GetCollator(return_type.GetCollate()),
            args,
            return_type,
            pb_code,
            collation_info: collationInfo::default(),
            share_flag: std::sync::Arc::new(AtomicU32::new(0)),
        }
    }

    /// 返回类型与参数语义相等时视为 equal。
    fn equal(&self, ctx: &dyn EvalContext, other: &Self) -> bool {
        self.return_type == other.return_type
            && self.args.len() == other.args.len()
            && self
                .args
                .iter()
                .zip(&other.args)
                .all(|(left, right)| left.Equal(ctx, right.as_ref()))
    }

    /// 委托线程安全生成逻辑判断跨会话共享。
    fn safe_to_share(&self) -> bool {
        crate::builtin_threadsafe_generated_kernel::safeToShareAcrossSession(
            self.share_flag.as_ref(),
            &self.args,
            |argument| argument.SafeToShareAcrossSession(),
        )
    }

    /// 估算底座结构及参数堆内存。
    fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self.return_type.MemoryUsage()
            + self
                .args
                .iter()
                .map(|argument| argument.MemoryUsage())
                .sum::<i64>()
    }
}

macro_rules! impl_collation_info {
    ($type:ty) => {
        impl CollationInfo for $type {
            fn HasCoercibility(&self) -> bool {
                self.base.collation_info.HasCoercibility()
            }
            fn Coercibility(&self) -> Coercibility {
                self.base.collation_info.Coercibility()
            }
            fn SetCoercibility(&self, value: Coercibility) {
                self.base.collation_info.SetCoercibility(value)
            }
            fn Repertoire(&self) -> Repertoire {
                self.base.collation_info.Repertoire()
            }
            fn SetRepertoire(&mut self, value: Repertoire) {
                self.base.collation_info.SetRepertoire(value)
            }
            fn CharsetAndCollation(&self) -> (String, String) {
                self.base.collation_info.CharsetAndCollation()
            }
            fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
                self.base
                    .collation_info
                    .SetCharsetAndCollation(charset, collation)
            }
            fn IsExplicitCharset(&self) -> bool {
                self.base.collation_info.IsExplicitCharset()
            }
            fn SetExplicitCharset(&mut self, explicit: bool) {
                self.base.collation_info.SetExplicitCharset(explicit)
            }
        }
    };
}

/// Formal GROUPING signature carrying validated protobuf metadata.
///
/// GROUPING 聚合签名：携带已校验的 tipb 元数据（模式与分组标记集合）。
pub struct BuiltinGroupingImplSig {
    base: PlannerBuiltinBase,
    state: RwLock<crate::builtin_grouping_kernel::GroupingSig>,
}

impl Clone for BuiltinGroupingImplSig {
    fn clone(&self) -> Self {
        Self {
            base: self.base.clone(),
            state: RwLock::new(
                self.state
                    .read()
                    .expect("grouping metadata poisoned")
                    .clone(),
            ),
        }
    }
}

impl BuiltinGroupingImplSig {
    /// 构造无符号 BIGINT 返回类型的 GROUPING 签名壳。
    pub(crate) fn new(args: Vec<ExprBox>) -> Self {
        let mut return_type = *types::NewFieldType(mysql::TypeLonglong);
        return_type.AddFlag(mysql::UnsignedFlag);
        Self {
            base: PlannerBuiltinBase::new(
                args,
                return_type,
                tipb::ScalarFuncSig::GroupingSig as i32,
            ),
            state: RwLock::new(crate::builtin_grouping_kernel::GroupingSig::new()),
        }
    }

    /// Go-equivalent metadata installation. Invalid input leaves the signature uninitialized.
    ///
    /// 安装分组模式与标记；非法输入使签名保持未初始化。
    pub fn SetMetadata(
        &self,
        mode: tipb::GroupingMode,
        grouping_marks: Vec<HashSet<u64>>,
    ) -> Result<(), Error> {
        let mode = match mode {
            tipb::GroupingMode::ModeBitAnd => crate::builtin_grouping_kernel::GroupingMode::BitAnd,
            tipb::GroupingMode::ModeNumericCmp => {
                crate::builtin_grouping_kernel::GroupingMode::NumericCmp
            }
            tipb::GroupingMode::ModeNumericSet => {
                crate::builtin_grouping_kernel::GroupingMode::NumericSet
            }
        };
        self.state
            .write()
            .map_err(|_| errors::New("grouping metadata poisoned"))?
            .set_metadata(mode, grouping_marks)
            .map_err(|error| errors::New(error.to_string()))
    }
}

impl_collation_info!(BuiltinGroupingImplSig);

impl builtinFunc for BuiltinGroupingImplSig {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn groupingMetaInitialized(&self) -> Option<bool> {
        Some(
            self.state
                .read()
                .expect("grouping metadata poisoned")
                .is_metadata_initialized(),
        )
    }
    fn groupingModeAndMarks(&self) -> Option<(i64, Vec<Vec<u64>>)> {
        let metadata = self.state.read().ok()?.metadata().ok()?;
        let mode = match metadata.mode {
            crate::builtin_grouping_kernel::GroupingMode::BitAnd => {
                tipb::GroupingMode::ModeBitAnd as i64
            }
            crate::builtin_grouping_kernel::GroupingMode::NumericCmp => {
                tipb::GroupingMode::ModeNumericCmp as i64
            }
            crate::builtin_grouping_kernel::GroupingMode::NumericSet => {
                tipb::GroupingMode::ModeNumericSet as i64
            }
            crate::builtin_grouping_kernel::GroupingMode::Invalid => return None,
        };
        Some((mode, metadata.grouping_marks))
    }
    fn restoreGroupingModeAndMarks(&self, mode: i64, marks: Vec<Vec<u64>>) -> Result<(), Error> {
        let mode = tipb::GroupingMode::from_i32(mode as i32)
            .ok_or_else(|| errors::New(format!("invalid GROUPING mode {mode}")))?;
        self.SetMetadata(
            mode,
            marks
                .into_iter()
                .map(|values| values.into_iter().collect())
                .collect(),
        )
    }
    fn metadata(&self) -> Option<Vec<u8>> {
        let (mode, marks) = self.groupingModeAndMarks()?;
        let mode = tipb::GroupingMode::from_i32(mode as i32)?;
        let mut metadata = tipb::GroupingFunctionMetadata::new();
        metadata.set_mode(mode);
        for values in marks {
            let mut mark = tipb::GroupingMark::new();
            mark.set_grouping_nums(values);
            metadata.mut_grouping_marks().push(mark);
        }
        metadata.write_to_bytes().ok()
    }
    fn SafeToShareAcrossSession(&self) -> bool {
        self.base.safe_to_share()
    }
    fn evalInt(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(i64, bool), Error> {
        let (grouping_id, is_null) = self.base.args[0].EvalInt(ctx, row)?;
        if is_null {
            return Ok((0, true));
        }
        let value = self
            .state
            .read()
            .map_err(|_| errors::New("grouping metadata poisoned"))?
            .eval(grouping_id as u64)
            .map_err(|error| errors::New(error.to_string()))?;
        Ok((value, false))
    }
    fn getArgs(&self) -> &[ExprBox] {
        &self.base.args
    }
    fn getArgsMut(&mut self) -> &mut [ExprBox] {
        &mut self.base.args
    }
    fn equal(&self, ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| self.base.equal(ctx, &other.base))
    }
    fn getRetTp(&self) -> &types::FieldType {
        &self.base.return_type
    }
    fn setPbCode(&mut self, code: i32) {
        self.base.pb_code = code
    }
    fn PbCode(&self) -> i32 {
        self.base.pb_code
    }
    fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
        self.base.collator = collator
    }
    fn collator(&self) -> &dyn collate::Collator {
        self.base.collator.as_ref()
    }
    fn Clone(&self) -> Box<dyn builtinFunc> {
        Box::new(self.clone())
    }
    fn MemoryUsage(&self) -> i64 {
        self.base.memory_usage()
    }
    fn vectorized(&self) -> bool {
        true
    }
}

/// MySQL 风格 `MATCH ... AGAINST` 规划期签名；求值侧禁止在全文索引外执行。
struct BuiltinFtsMysqlMatchAgainstSig {
    base: PlannerBuiltinBase,
    modifier: std::sync::Arc<AtomicU8>,
}

impl Clone for BuiltinFtsMysqlMatchAgainstSig {
    fn clone(&self) -> Self {
        Self {
            base: self.base.clone(),
            modifier: std::sync::Arc::new(AtomicU8::new(self.modifier.load(Ordering::SeqCst))),
        }
    }
}

impl BuiltinFtsMysqlMatchAgainstSig {
    /// 返回 DOUBLE 的 FTS 匹配表达式签名。
    fn new(args: Vec<ExprBox>) -> Self {
        Self {
            base: PlannerBuiltinBase::new(
                args,
                *types::NewFieldType(mysql::TypeDouble),
                tipb::ScalarFuncSig::FtsMatchExpression as i32,
            ),
            modifier: std::sync::Arc::new(AtomicU8::new(0)),
        }
    }
    /// 写入全文检索修饰符（布尔/自然语言等）。
    fn SetModifier(&self, modifier: u8) {
        self.modifier.store(modifier, Ordering::SeqCst)
    }

    /// 读取全文检索修饰符。
    fn Modifier(&self) -> u8 {
        self.modifier.load(Ordering::SeqCst)
    }
}

impl_collation_info!(BuiltinFtsMysqlMatchAgainstSig);

impl builtinFunc for BuiltinFtsMysqlMatchAgainstSig {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn SafeToShareAcrossSession(&self) -> bool {
        false
    }
    fn evalReal(&self, _ctx: &dyn EvalContext, _row: chunk::Row) -> Result<(f64, bool), Error> {
        // NULL 常量参数直接返回 SQL NULL；其余场景必须走全文索引路径。
        if self
            .base
            .args
            .first()
            .and_then(|argument| argument.as_any().downcast_ref::<Constant>())
            .is_some_and(|constant| constant.Value.IsNull())
        {
            return Ok((0.0, true));
        }
        Err(errors::New(
            "cannot use 'MATCH ... AGAINST' outside of fulltext index",
        ))
    }
    fn getArgs(&self) -> &[ExprBox] {
        &self.base.args
    }
    fn getArgsMut(&mut self) -> &mut [ExprBox] {
        &mut self.base.args
    }
    fn equal(&self, ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| self.base.equal(ctx, &other.base))
    }
    fn getRetTp(&self) -> &types::FieldType {
        &self.base.return_type
    }
    fn setPbCode(&mut self, code: i32) {
        self.base.pb_code = code
    }
    fn PbCode(&self) -> i32 {
        self.base.pb_code
    }
    fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
        self.base.collator = collator
    }
    fn collator(&self) -> &dyn collate::Collator {
        self.base.collator.as_ref()
    }
    fn Clone(&self) -> Box<dyn builtinFunc> {
        Box::new(self.clone())
    }
    fn MemoryUsage(&self) -> i64 {
        self.base.memory_usage()
    }
    fn vectorized(&self) -> bool {
        true
    }
}

#[derive(Clone)]
/// JSON_SUM_CRC32 规划期签名，额外保留数组元素目标类型。
struct BuiltinJsonSumCrc32Sig {
    base: PlannerBuiltinBase,
    array_type: types::FieldType,
}

impl_collation_info!(BuiltinJsonSumCrc32Sig);

impl BuiltinJsonSumCrc32Sig {
    fn converted_item(
        &self,
        ctx: &dyn EvalContext,
        item: types::BinaryJSON,
    ) -> Result<String, Error> {
        let field_type = &self.array_type;
        match field_type.EvalType() {
            types::ETString => {
                if item.TypeCode != JSONTypeCodeString {
                    return Err(errors::New(format!(
                        "Invalid JSON value for CAST to type {}",
                        field_type.CompactStr()
                    )));
                }
                let (value, error) = types_dependency::datum::ProduceStrWithSpecifiedTp(
                    String::from_utf8_lossy(&item.GetString()).into_owned(),
                    field_type,
                    ctx.TypeCtx(),
                    false,
                );
                error.map_or_else(|| Ok(value), Err)
            }
            types::ETInt => {
                if item.TypeCode != JSONTypeCodeInt64 && item.TypeCode != JSONTypeCodeUint64 {
                    return Err(errors::New(format!(
                        "Invalid JSON value for CAST to type {}",
                        field_type.CompactStr()
                    )));
                }
                let value = types::ConvertJSONToInt(
                    ctx.TypeCtx(),
                    item,
                    mysql::HasUnsignedFlag(field_type.GetFlag()),
                    field_type.GetType(),
                )?;
                if mysql::HasUnsignedFlag(field_type.GetFlag()) {
                    Ok((value as u64).to_string())
                } else {
                    Ok(value.to_string())
                }
            }
            types::ETReal => {
                if !matches!(
                    item.TypeCode,
                    JSONTypeCodeFloat64 | JSONTypeCodeInt64 | JSONTypeCodeUint64
                ) {
                    return Err(errors::New(format!(
                        "Invalid JSON value for CAST to type {}",
                        field_type.CompactStr()
                    )));
                }
                Ok(types::ConvertJSONToFloat(ctx.TypeCtx(), item)?.to_string())
            }
            types::ETDatetime => {
                let expected = if field_type.GetType() == mysql::TypeDate {
                    types_dependency::json_functions::JSONTypeCodeDate
                } else {
                    JSONTypeCodeDatetime
                };
                if item.TypeCode != expected {
                    return Err(errors::New(format!(
                        "Invalid JSON value for CAST to type {}",
                        field_type.CompactStr()
                    )));
                }
                let json_time = item.GetTimeWithFsp(field_type.GetDecimal() as u8);
                let value = types_dependency::time::NewTime(
                    types_dependency::time::CoreTime(json_time.CoreTime),
                    field_type.GetType(),
                    field_type.GetDecimal() as i32,
                );
                Ok(value.String())
            }
            types::ETDuration => {
                if item.TypeCode != JSONTypeCodeDuration {
                    return Err(errors::New(format!(
                        "Invalid JSON value for CAST to type {}",
                        field_type.CompactStr()
                    )));
                }
                let duration = item.GetDuration();
                Ok(types_dependency::time::Duration {
                    Duration: duration.Duration,
                    Fsp: duration.Fsp as i32,
                }
                .String())
            }
            _ => Err(errors::New(format!(
                "calculating sum of {}",
                field_type.CompactStr()
            ))),
        }
    }
}

impl builtinFunc for BuiltinJsonSumCrc32Sig {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn SafeToShareAcrossSession(&self) -> bool {
        self.base.safe_to_share()
    }
    fn evalInt(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(i64, bool), Error> {
        let (value, is_null) = self.base.args[0].EvalJSON(ctx, row)?;
        if is_null {
            return Ok((0, true));
        }
        if value.TypeCode != JSONTypeCodeArray {
            return Err(errors::New(
                "Invalid data type for JSON data in argument 1 to function json_sum_crc32",
            ));
        }
        let mut sum = 0_i64;
        for index in 0..value.GetElemCount() {
            let item = self.converted_item(ctx, value.ArrayGetElem(index))?;
            sum = sum.wrapping_add(crc32fast::hash(item.as_bytes()) as i64);
        }
        Ok((sum, false))
    }
    fn getArgs(&self) -> &[ExprBox] {
        &self.base.args
    }
    fn getArgsMut(&mut self) -> &mut [ExprBox] {
        &mut self.base.args
    }
    fn equal(&self, ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
        other.as_any().downcast_ref::<Self>().is_some_and(|other| {
            self.array_type == other.array_type && self.base.equal(ctx, &other.base)
        })
    }
    fn getRetTp(&self) -> &types::FieldType {
        &self.base.return_type
    }
    fn setPbCode(&mut self, code: i32) {
        self.base.pb_code = code
    }
    fn PbCode(&self) -> i32 {
        self.base.pb_code
    }
    fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
        self.base.collator = collator
    }
    fn collator(&self) -> &dyn collate::Collator {
        self.base.collator.as_ref()
    }
    fn Clone(&self) -> Box<dyn builtinFunc> {
        Box::new(self.clone())
    }
    fn MemoryUsage(&self) -> i64 {
        self.base.memory_usage() + self.array_type.MemoryUsage()
    }
    fn vectorized(&self) -> bool {
        true
    }
}

pub(crate) fn build_builtin(
    name: &str,
    ctx: &dyn BuildContext,
    args: Vec<ExprBox>,
) -> Option<Result<Box<dyn builtinFunc>, Error>> {
    match name {
        ast::Grouping => Some(if args.len() == 1 {
            Ok(Box::new(BuiltinGroupingImplSig::new(args)))
        } else {
            Err(crate::ErrIncorrectParameterCount.GenWithStackByArgs(name))
        }),
        ast::FTSMysqlMatchAgainst => Some((|| {
            if args.len() < 2 {
                return Err(crate::ErrIncorrectParameterCount.GenWithStackByArgs(name));
            }
            let search = args[0].as_any().downcast_ref::<Constant>().ok_or_else(|| {
                crate::ErrNotSupportedYet.GenWithStackByArgs("match against a non-constant string")
            })?;
            if search.Value.Kind() != types::KindString && !search.Value.IsNull() {
                return Err(crate::ErrNotSupportedYet
                    .GenWithStackByArgs("match against a non-string constant"));
            }
            for argument in &args[1..] {
                if argument.as_any().downcast_ref::<crate::Column>().is_none() {
                    return Err(
                        crate::ErrNotSupportedYet.GenWithStackByArgs("not matching a column")
                    );
                }
                if argument.GetType(ctx.GetEvalCtx()).EvalType() != types::ETString {
                    return Err(crate::ErrNotSupportedYet.GenWithStackByArgs(
                        "Doesn't support match search on a non-string column without fulltext index",
                    ));
                }
            }
            Ok(Box::new(BuiltinFtsMysqlMatchAgainstSig::new(args)) as Box<dyn builtinFunc>)
        })()),
        _ => None,
    }
}

/// Sets the modifier on the internal MATCH ... AGAINST signature.
///
/// 为内部 MATCH...AGAINST 签名写入修饰符位。
pub fn SetFTSMysqlMatchAgainstModifier(
    scalar_function: &ScalarFunction,
    modifier: u8,
) -> Result<(), Error> {
    let signature = scalar_function
        .Function
        .as_any()
        .downcast_ref::<BuiltinFtsMysqlMatchAgainstSig>()
        .ok_or_else(|| {
            errors::New(format!(
                "unexpected builtin signature for {}",
                ast::FTSMysqlMatchAgainst
            ))
        })?;
    signature.SetModifier(modifier);
    Ok(())
}

/// Validates the exact strict token subset accepted by the ILIKE fallback.
///
/// 校验 ILIKE 回退可接受的 token 子集：仅 ASCII 字母数字，或非 ASCII（如中文）；
/// 布尔模式下允许前导 +/-，但不允许空主体或通配符。
pub fn ValidateFTSSearchStringForLikeFallback(
    search_text: String,
    modifier: u8,
) -> Result<(), Error> {
    const MODE_MASK: u8 = 0x0f;
    const BOOLEAN_MODE: u8 = 1;
    let boolean_mode = modifier & MODE_MASK == BOOLEAN_MODE;
    for token in search_text.split_whitespace() {
        // 布尔模式去掉 +/- 前缀后再校验主体。
        let body = if boolean_mode && (token.starts_with('+') || token.starts_with('-')) {
            &token[1..]
        } else {
            token
        };
        if body.is_empty()
            || body
                .as_bytes()
                .iter()
                .any(|byte| !byte.is_ascii_alphanumeric() && *byte < 0x80)
        {
            return Err(crate::ErrNotSupportedYet.GenWithStackByArgs(format!(
                "MATCH...AGAINST search term '{token}' is not supported in the LIKE fallback"
            )));
        }
    }
    Ok(())
}

/// Builds the planner's direct MATCH ... AGAINST to ILIKE fallback.
///
/// 将 MATCH...AGAINST 直接改写为 ILIKE 谓词树：自然语言模式用 DNF 连 OR，
/// 布尔模式按 +/- 组装 CNF（必含 / 排除 / 可选）。
pub fn BuildFTSToILikeExpression(
    ctx: &dyn BuildContext,
    columns: Vec<ExprBox>,
    search_text: String,
    modifier: u8,
) -> Result<ExprBox, Error> {
    const MODE_MASK: u8 = 0x0f;
    const NATURAL_LANGUAGE_MODE: u8 = 0;
    const BOOLEAN_MODE: u8 = 1;
    const WITH_QUERY_EXPANSION: u8 = 1 << 4;

    if columns.is_empty() {
        return Err(crate::ErrNotSupportedYet.GenWithStackByArgs("MATCH...AGAINST with no columns"));
    }
    if modifier & WITH_QUERY_EXPANSION != 0 {
        return Err(crate::ErrNotSupportedYet.GenWithStackByArgs(
            "MATCH...AGAINST WITH QUERY EXPANSION is not supported in the LIKE fallback",
        ));
    }
    ValidateFTSSearchStringForLikeFallback(search_text.clone(), modifier)?;
    if search_text.trim().is_empty() {
        return Ok(Box::new(crate::NewSignedZero()));
    }

    /// 单列对单词构造 `IFNULL(column ILIKE '%term%', 0)`。
    fn predicate(ctx: &dyn BuildContext, column: &ExprBox, term: &str) -> Result<ExprBox, Error> {
        let mut escaped = String::with_capacity(term.len());
        // 转义 ILIKE 通配与反斜杠。
        for ch in term.chars() {
            if matches!(ch, '\\' | '%' | '_') {
                escaped.push('\\');
            }
            escaped.push(ch);
        }
        let ilike = crate::NewFunction(
            ctx,
            ast::Ilike,
            *types::NewFieldType(mysql::TypeTiny),
            vec![
                column.CloneExpr(),
                Box::new(crate::NewStrConst(&format!("%{escaped}%"))),
                Box::new(crate::NewInt64Const(92)),
            ],
        )?;
        crate::NewFunction(
            ctx,
            ast::Ifnull,
            *types::NewFieldType(mysql::TypeTiny),
            vec![ilike, Box::new(crate::NewSignedZero())],
        )
    }

    let mode = modifier & MODE_MASK;
    if mode == NATURAL_LANGUAGE_MODE {
        // 自然语言模式：所有列×词的 ILIKE 取 DNF（OR）。
        let mut predicates = Vec::new();
        for column in &columns {
            for word in search_text.split_whitespace() {
                predicates.push(predicate(ctx, column, word)?);
            }
        }
        return Ok(crate::ComposeDNFCondition(ctx, &predicates)
            .unwrap_or_else(|| Box::new(crate::NewSignedZero())));
    }

    if mode != BOOLEAN_MODE {
        return Err(crate::ErrNotSupportedYet
            .GenWithStackByArgs("MATCH...AGAINST modifier is not supported in the LIKE fallback"));
    }

    // 布尔模式：+必含、-排除、其余可选。
    let mut required = Vec::new();
    let mut excluded = Vec::new();
    let mut optional = Vec::new();
    for token in search_text.split_whitespace() {
        if let Some(word) = token.strip_prefix('+') {
            required.push(word);
        } else if let Some(word) = token.strip_prefix('-') {
            excluded.push(word);
        } else {
            optional.push(token);
        }
    }
    if required.is_empty() && optional.is_empty() {
        return Ok(Box::new(crate::NewSignedZero()));
    }

    let mut all = Vec::new();
    for term in required.iter().copied() {
        let predicates = columns
            .iter()
            .map(|column| predicate(ctx, column, term))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(expression) = crate::ComposeDNFCondition(ctx, &predicates) {
            all.push(expression);
        }
    }
    for term in excluded.iter().copied() {
        let predicates = columns
            .iter()
            .map(|column| predicate(ctx, column, term))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(expression) = crate::ComposeDNFCondition(ctx, &predicates) {
            all.push(crate::NewFunction(
                ctx,
                ast::UnaryNot,
                *types::NewFieldType(mysql::TypeTiny),
                vec![expression],
            )?);
        }
    }
    if required.is_empty() {
        let mut predicates = Vec::new();
        for term in optional {
            for column in &columns {
                predicates.push(predicate(ctx, column, term)?);
            }
        }
        if let Some(expression) = crate::ComposeDNFCondition(ctx, &predicates) {
            if excluded.is_empty() {
                return Ok(expression);
            }
            all.push(expression);
        }
    }
    Ok(crate::ComposeCNFCondition(ctx, &all).unwrap_or_else(|| Box::new(crate::NewSignedZero())))
}

/// Rewrites a single-column MATCH...AGAINST builtin into the same ILIKE
/// predicate family used by the planner fallback, so selectivity can evaluate
/// real TopN and histogram values.
///
/// 从已构造的 MATCH...AGAINST builtin 抽出常量搜索串与修饰符，再走同一套 ILIKE 回退，
/// 以便选择率（selectivity）可用 TopN/直方图估值。
pub fn BuildFTSToILikeExpressionFromBuiltin(
    ctx: &dyn BuildContext,
    fts: &ScalarFunction,
) -> Result<ExprBox, Error> {
    if fts.FuncName.L != ast::FTSMysqlMatchAgainst {
        return Err(errors::New(format!(
            "expected {}, got {}",
            ast::FTSMysqlMatchAgainst,
            fts.FuncName.L
        )));
    }
    let args = fts.GetArgs();
    if args.len() < 2 {
        return Err(errors::New(format!(
            "{} expects at least 2 args, got {}",
            ast::FTSMysqlMatchAgainst,
            args.len()
        )));
    }
    if args.len() > 2 {
        return Err(crate::ErrNotSupportedYet
            .GenWithStackByArgs("multi-column MATCH...AGAINST in selectivity substitution"));
    }
    let search = args[0].as_constant().ok_or_else(|| {
        crate::ErrNotSupportedYet
            .GenWithStackByArgs("MATCH...AGAINST with non-constant search string")
    })?;
    if search.Value.IsNull() {
        return Ok(Box::new(crate::NewNull()));
    }
    if search.Value.Kind() != types::KindString {
        return Err(crate::ErrNotSupportedYet
            .GenWithStackByArgs("MATCH...AGAINST with non-string search constant"));
    }
    let signature = fts
        .Function
        .as_any()
        .downcast_ref::<BuiltinFtsMysqlMatchAgainstSig>()
        .ok_or_else(|| errors::New("unexpected FTS builtin signature"))?;
    BuildFTSToILikeExpression(
        ctx,
        args[1..]
            .iter()
            .map(|argument| argument.CloneExpr())
            .collect(),
        search.Value.GetString(),
        signature.Modifier(),
    )
}

/// Builds JSON_SUM_CRC32 with the target array element type retained by the signature.
///
/// 构造 JSON_SUM_CRC32，签名内保留目标数组元素类型供后续求值使用。
pub fn BuildJSONSumCrc32FunctionWithCheck(
    _ctx: &dyn BuildContext,
    expression: ExprBox,
    mut target: types::FieldType,
) -> Result<ExprBox, Error> {
    if !mysql::HasNotNullFlag(expression.GetType(_ctx.GetEvalCtx()).GetFlag()) {
        target.DelFlag(mysql::NotNullFlag);
    }
    if target.EvalType() != types::ETJson || !target.IsArray() {
        return Err(errors::New(format!(
            "json_sum_crc32 can only built on JSON array, got type {}",
            target.EvalType()
        )));
    }
    if expression.GetType(_ctx.GetEvalCtx()).EvalType() != types::ETJson {
        return Err(errors::New(
            "Invalid data type for JSON data in argument 1 to function JSON_SUM_CRC32",
        ));
    }
    let array_type = target.ArrayType();
    if matches!(
        array_type.GetType(),
        mysql::TypeYear | mysql::TypeJSON | mysql::TypeFloat | mysql::TypeNewDecimal
    ) {
        return Err(crate::ErrNotSupportedYet.GenWithStackByArgs(format!(
            "calculating json_sum_crc32 on array of {}",
            array_type.CompactStr()
        )));
    }
    if array_type.EvalType() == types::ETString
        && !matches!(
            array_type.GetCharset(),
            charset::CharsetUTF8MB4 | charset::CharsetBin
        )
    {
        return Err(crate::ErrNotSupportedYet.GenWithStackByArgs("unsupported charset"));
    }
    if array_type.EvalType() == types::ETString && array_type.GetFlen() == -1 {
        return Err(crate::ErrNotSupportedYet.GenWithStackByArgs(
            "calculating json_sum_crc32 on array of char/binary BLOBs with unspecified length",
        ));
    }
    let function = BuiltinJsonSumCrc32Sig {
        base: PlannerBuiltinBase::new(
            vec![expression],
            *types::NewFieldType(mysql::TypeLonglong),
            0,
        ),
        array_type,
    };
    Ok(Box::new(ScalarFunction {
        FuncName: ast::NewCIStr(ast::JSONSumCrc32),
        RetType: Some(*types::NewFieldType(mysql::TypeLonglong)),
        Function: Box::new(function),
        hashcode: Vec::new(),
        canonicalhashcode: Vec::new(),
    }))
}

/// 比较类型基础推导：综合两侧 EvalType 与无符号等标志。
fn base_cmp_type(
    mut left: types::EvalType,
    mut right: types::EvalType,
    left_type: Option<&types::FieldType>,
    right_type: Option<&types::FieldType>,
) -> types::EvalType {
    if let (Some(left_field), Some(right_field)) = (left_type, right_type)
        && (left_field.GetType() == mysql::TypeUnspecified
            || right_field.GetType() == mysql::TypeUnspecified)
    {
        if left_field.GetType() == right_field.GetType() {
            return types::ETString;
        }
        if left_field.GetType() == mysql::TypeUnspecified {
            left = right;
        } else {
            right = left;
        }
    }
    if left.IsStringKind() && right.IsStringKind() {
        types::ETString
    } else if (left == types::ETInt || left_type.is_some_and(types::FieldType::Hybrid))
        && (right == types::ETInt || right_type.is_some_and(types::FieldType::Hybrid))
    {
        types::ETInt
    } else if matches!(
        (left, right),
        (types::ETDecimal, types::ETString) | (types::ETString, types::ETDecimal)
    ) {
        types::ETReal
    } else if (matches!(left, types::ETInt | types::ETDecimal)
        || left_type.is_some_and(types::FieldType::Hybrid))
        && (matches!(right, types::ETInt | types::ETDecimal)
            || right_type.is_some_and(types::FieldType::Hybrid))
    {
        types::ETDecimal
    } else if left_type.zip(right_type).is_some_and(|(left, right)| {
        (types_dependency::metadata::IsTemporalWithDate(left.GetType())
            && right.GetType() == mysql::TypeYear)
            || (left.GetType() == mysql::TypeYear
                && types_dependency::metadata::IsTemporalWithDate(right.GetType()))
    }) {
        types::ETDatetime
    } else {
        types::ETReal
    }
}

/// Go-equivalent precise comparison type selection.
///
/// 精确比较类型选择：按左右操作数 EvalType / 字段类型决定最终比较家族。
pub fn GetAccurateCmpType(
    ctx: &dyn EvalContext,
    left: &dyn Expression,
    right: &dyn Expression,
) -> types::EvalType {
    let left_type = left.GetType(ctx);
    let right_type = right.GetType(ctx);
    let left_eval = left_type.EvalType();
    let right_eval = right_type.EvalType();
    let mut result = base_cmp_type(left_eval, right_eval, Some(left_type), Some(right_type));
    if left_eval == types::ETVectorFloat32 || right_eval == types::ETVectorFloat32 {
        result = types::ETVectorFloat32;
    } else if (left_eval.IsStringKind() && left_type.GetType() == mysql::TypeJSON)
        || (right_eval.IsStringKind() && right_type.GetType() == mysql::TypeJSON)
    {
        result = types::ETJson;
    } else if result == types::ETString
        && (types_dependency::metadata::IsTypeTime(left_type.GetType())
            || types_dependency::metadata::IsTypeTime(right_type.GetType()))
    {
        result = if left_type.GetType() == right_type.GetType() {
            left_eval
        } else {
            types::ETDatetime
        };
    } else if left_type.GetType() == mysql::TypeDuration
        && right_type.GetType() == mysql::TypeDuration
    {
        result = types::ETDuration;
    } else if matches!(result, types::ETReal | types::ETString) {
        let left_constant = left.as_any().is::<Constant>();
        let right_constant = right.as_any().is::<Constant>();
        if (left_eval == types::ETDecimal
            && !left_constant
            && right_eval.IsStringKind()
            && right_constant)
            || (right_eval == types::ETDecimal
                && !right_constant
                && left_eval.IsStringKind()
                && left_constant)
        {
            result = types::ETDecimal;
        } else {
            let temporal_column = |expression: &dyn Expression| {
                expression.as_any().is::<crate::Column>()
                    && (types_dependency::metadata::IsTypeTime(expression.GetType(ctx).GetType())
                        || expression.GetType(ctx).GetType() == mysql::TypeDuration)
            };
            if (temporal_column(left) && right_constant)
                || (temporal_column(right) && left_constant)
            {
                let temporal = if temporal_column(left) { left } else { right };
                if temporal.GetType(ctx).GetType() == mysql::TypeDuration {
                    result = types::ETDuration;
                }
            }
        }
    }
    result
}

/// Resolves the common comparison type for BETWEEN's three operands.
///
/// 解析 BETWEEN 三个操作数的公共比较类型。
pub fn ResolveType4Between(
    ctx: &dyn EvalContext,
    arguments: [&dyn Expression; 3],
) -> types::EvalType {
    let mut result = arguments[0].GetType(ctx).EvalType();
    for argument in &arguments[1..] {
        result = base_cmp_type(result, argument.GetType(ctx).EvalType(), None, None);
    }
    if result == types::ETString {
        if arguments[0].GetType(ctx).GetType() == mysql::TypeDuration {
            result = types::ETDuration;
        } else if arguments.iter().any(|argument| {
            types_dependency::metadata::IsTypeTemporal(argument.GetType(ctx).GetType())
        }) {
            result = types::ETDatetime;
        }
    }
    if arguments.iter().all(|argument| {
        argument.GetType(ctx).EvalType() == types::ETInt || crate::IsBinaryLiteral(*argument)
    }) {
        return types::ETInt;
    }
    result
}

/// 取两侧显示宽度较大者（未指定时保留另一侧）。
fn max_length(left: isize, right: isize) -> isize {
    if left < 0 || right < 0 {
        mysql::MaxRealWidth as isize
    } else {
        left.max(right)
    }
}

/// 按参数小数位推导结果 decimal（控制函数类型合并用）。
fn set_decimal_from_args(
    eval_type: types::EvalType,
    result: &mut types::FieldType,
    arguments: &[&types::FieldType],
) {
    if eval_type == types::ETInt {
        result.SetDecimal(0);
        return;
    }
    let mut decimal = 0;
    for argument in arguments {
        if argument.GetDecimal() == -1 {
            result.SetDecimal(-1);
            return;
        }
        decimal = decimal.max(argument.GetDecimal());
    }
    result.SetDecimalUnderLimit(decimal);
}

/// 按参数显示宽度推导结果 flen。
fn set_flen_from_args(
    eval_type: types::EvalType,
    result: &mut types::FieldType,
    arguments: &[&types::FieldType],
) {
    if matches!(eval_type, types::ETDecimal | types::ETInt) {
        let mut maximum = 0;
        for argument in arguments {
            let sign = if mysql::HasUnsignedFlag(argument.GetFlag()) {
                0
            } else {
                1
            };
            let mut flen = argument.GetFlen() - sign;
            if argument.GetDecimal() != -1 {
                flen -= argument.GetDecimal();
            }
            maximum = max_length(maximum, flen);
        }
        result.SetFlenUnderLimit(maximum + result.GetDecimal() + 1);
    } else if eval_type == types::ETString {
        let mut maximum = 0;
        for argument in arguments {
            let flen = match argument.GetType() {
                mysql::TypeTiny => 4,
                mysql::TypeShort => 6,
                mysql::TypeInt24 => 9,
                mysql::TypeLong => 11,
                mysql::TypeLonglong => 20,
                _ if argument.GetFlen() == -1 => {
                    result.SetFlen(-1);
                    return;
                }
                _ => argument.GetFlen(),
            };
            maximum = max_length(maximum, flen);
        }
        result.SetFlen(maximum);
    } else {
        result.SetFlen(arguments.iter().fold(0, |maximum, argument| {
            max_length(maximum, argument.GetFlen())
        }));
    }
}

/// Infers the result type for IF/IFNULL/LEAD/LAG's two value operands.
///
/// 为 IF/IFNULL/LEAD/LAG 等控制函数的两个结果操作数推导统一返回类型。
pub fn InferType4ControlFuncs(
    ctx: &dyn BuildContext,
    function_name: &str,
    left: &dyn Expression,
    right: &dyn Expression,
) -> Result<types::FieldType, Error> {
    InferType4ControlFuncsVariadic(ctx, function_name, &[left, right])
}

/// Infers one common result type for every value arm of a control function.
///
/// CASE may contain more than two THEN/ELSE expressions; Go performs one
/// aggregate type derivation across the complete list rather than choosing the
/// first non-NULL arm.
pub fn InferType4ControlFuncsVariadic(
    ctx: &dyn BuildContext,
    function_name: &str,
    arguments: &[&dyn Expression],
) -> Result<types::FieldType, Error> {
    if arguments.is_empty() {
        return Err(errors::New(
            "control type inference requires at least one value expression",
        ));
    }
    let field_types = arguments
        .iter()
        .map(|argument| argument.GetType(ctx.GetEvalCtx()))
        .collect::<Vec<_>>();
    let non_null = field_types
        .iter()
        .copied()
        .filter(|field_type| field_type.GetType() != mysql::TypeNull)
        .collect::<Vec<_>>();
    let null_count = field_types.len() - non_null.len();
    if non_null.is_empty() {
        let mut result = field_types[0].clone();
        result.DelFlag(mysql::NotNullFlag);
        result.SetType(mysql::TypeNull);
        result.SetFlen(0);
        result.SetDecimal(0);
        types_dependency::field::SetBinChsClnFlag(&mut result);
        return Ok(result);
    }
    let mut result = if non_null.len() == 1 {
        non_null[0].clone()
    } else {
        *types_dependency::field::AggFieldType(&non_null)
    };
    if non_null.len() > 1 {
        let mut flag = 0;
        let eval_type = types_dependency::field::AggregateEvalType(&non_null, &mut flag);
        result.SetFlag(flag);
        set_decimal_from_args(eval_type, &mut result, &non_null);
        let expression_refs = arguments.to_vec();
        let collation = crate::CheckAndDeriveCollationFromExprs(
            ctx,
            function_name,
            eval_type,
            &expression_refs,
        )?;
        let preferred_non_binary = field_types
            .iter()
            .copied()
            .find(|field_type| types_dependency::metadata::IsNonBinaryStr(field_type));
        if let Some(preferred) = preferred_non_binary.filter(|_| {
            field_types
                .iter()
                .any(|field_type| !types_dependency::metadata::IsBinaryStr(field_type))
        }) {
            result.SetCollate(collation.Collation);
            result.SetCharset(collation.Charset);
            result.SetFlag(0);
            if mysql::HasBinaryFlag(preferred.GetFlag())
                || field_types
                    .iter()
                    .any(|field_type| !types_dependency::metadata::IsNonBinaryStr(field_type))
            {
                result.AddFlag(mysql::BinaryFlag);
            }
        } else if field_types
            .iter()
            .any(|field_type| types_dependency::metadata::IsBinaryStr(field_type))
            || !eval_type.IsStringKind()
        {
            types_dependency::field::SetBinChsClnFlag(&mut result);
        } else {
            result.SetCharset(charset::CharsetUTF8MB4.to_owned());
            result.SetCollate(charset::CollationUTF8MB4.to_owned());
            result.SetFlag(0);
        }
        set_flen_from_args(eval_type, &mut result, &non_null);
    }
    if null_count > 0 {
        result.DelFlag(mysql::NotNullFlag);
    }
    match result.EvalType() {
        types::ETInt => result.SetDecimal(0),
        types::ETString => result.SetDecimal(-1),
        _ => {}
    }
    if matches!(result.GetType(), mysql::TypeEnum | mysql::TypeSet) {
        match result.EvalType() {
            types::ETInt => result.SetType(mysql::TypeLonglong),
            types::ETString => result.SetType(mysql::TypeVarchar),
            _ => {}
        }
    }
    types_dependency::field::TryToFixFlenOfDatetime(&mut result);
    Ok(result)
}

/// 用新 FieldType 克隆常量表达式。
fn clone_constant_with_type(
    constant: &Constant,
    value: types::Datum,
    return_type: types::FieldType,
) -> Constant {
    let mut result = Constant::with_type(value, return_type);
    result.DeferredExpr = constant.DeferredExpr.clone();
    result.ParamMarker = constant.ParamMarker.clone();
    result.SubqueryRefID = constant.SubqueryRefID;
    result
}

/// 尝试把常量转为整数 Datum；失败返回 None。
fn try_convert_constant_int(
    ctx: &dyn BuildContext,
    target: &types::FieldType,
    constant: &Constant,
) -> (ExprBox, bool) {
    if Expression::GetType(constant, ctx.GetEvalCtx()).EvalType() == types::ETInt {
        return (Box::new(constant.clone()), false);
    }
    let datum = match constant.Eval(ctx.GetEvalCtx(), chunk::Row::default()) {
        Ok(datum) => datum,
        Err(_) => return (Box::new(constant.clone()), false),
    };
    match datum.ConvertTo(ctx.GetEvalCtx().TypeCtx(), target) {
        Ok(value) => (
            Box::new(clone_constant_with_type(constant, value, target.clone())),
            false,
        ),
        Err(error) if types::terror::ErrorEqual(&error, &**types::ErrOverflow) => {
            let value = datum
                .ConvertTo(ctx.GetEvalCtx().TypeCtx(), target)
                .unwrap_or_default();
            (
                Box::new(clone_constant_with_type(constant, value, target.clone())),
                true,
            )
        }
        Err(_) => (Box::new(constant.clone()), false),
    }
}

/// Refines a non-integral constant compared with an integer field.
///
/// 与整型字段比较时精化非整数常量（截断/进位），以匹配索引范围边界语义。
pub fn RefineComparedConstant(
    ctx: &dyn BuildContext,
    mut target: types::FieldType,
    constant: &Constant,
    operation: opcode::Op,
) -> (ExprBox, bool) {
    let datum = match constant.Eval(ctx.GetEvalCtx(), chunk::Row::default()) {
        Ok(datum) => datum,
        Err(_) => return (Box::new(constant.clone()), false),
    };
    if target.GetType() == mysql::TypeBit {
        target = *types::NewFieldType(mysql::TypeLonglong);
    }
    let type_context = ctx.GetEvalCtx().TypeCtx();
    let conversion_context =
        type_context.WithFlags(type_context.Flags().WithAllowNegativeToUnsigned(false));
    let integer = match datum.ConvertTo(conversion_context, &target) {
        Ok(integer) => integer,
        Err(error) if types::terror::ErrorEqual(&error, &**types::ErrOverflow) => {
            return (
                Box::new(clone_constant_with_type(
                    constant,
                    types::Datum::default(),
                    target,
                )),
                true,
            );
        }
        Err(_) => return (Box::new(constant.clone()), false),
    };
    if integer
        .Compare(
            ctx.GetEvalCtx().TypeCtx(),
            &constant.Value,
            collate::GetBinaryCollator().as_ref(),
        )
        .ok()
        == Some(0)
    {
        return (
            Box::new(clone_constant_with_type(constant, integer, target)),
            false,
        );
    }
    match operation {
        opcode::Op::LT | opcode::Op::GE => {
            if let Ok(expression) = crate::NewFunction(
                ctx,
                ast::Ceil,
                *types::NewFieldType(mysql::TypeUnspecified),
                vec![Box::new(constant.clone())],
            ) && let Some(rounded) = expression.as_any().downcast_ref::<Constant>()
            {
                return try_convert_constant_int(ctx, &target, rounded);
            }
            if let Ok(real) = datum.ConvertTo(
                ctx.GetEvalCtx().TypeCtx(),
                &types::NewFieldType(mysql::TypeDouble),
            ) {
                let rounded = Constant::with_type(
                    types::NewFloat64Datum(real.GetFloat64().ceil()),
                    *types::NewFieldType(mysql::TypeDouble),
                );
                let (mut rounded, exceptional) = try_convert_constant_int(ctx, &target, &rounded);
                if let Some(rounded) = rounded.as_any_mut().downcast_mut::<Constant>() {
                    rounded.SubqueryRefID = constant.SubqueryRefID;
                }
                return (rounded, exceptional);
            }
        }
        opcode::Op::LE | opcode::Op::GT => {
            if let Ok(expression) = crate::NewFunction(
                ctx,
                ast::Floor,
                *types::NewFieldType(mysql::TypeUnspecified),
                vec![Box::new(constant.clone())],
            ) && let Some(rounded) = expression.as_any().downcast_ref::<Constant>()
            {
                return try_convert_constant_int(ctx, &target, rounded);
            }
            if let Ok(real) = datum.ConvertTo(
                ctx.GetEvalCtx().TypeCtx(),
                &types::NewFieldType(mysql::TypeDouble),
            ) {
                let rounded = Constant::with_type(
                    types::NewFloat64Datum(real.GetFloat64().floor()),
                    *types::NewFieldType(mysql::TypeDouble),
                );
                let (mut rounded, exceptional) = try_convert_constant_int(ctx, &target, &rounded);
                if let Some(rounded) = rounded.as_any_mut().downcast_mut::<Constant>() {
                    rounded.SubqueryRefID = constant.SubqueryRefID;
                }
                return (rounded, exceptional);
            }
        }
        opcode::Op::NullEQ | opcode::Op::EQ => {
            match Expression::GetType(constant, ctx.GetEvalCtx()).EvalType() {
                types::ETReal | types::ETDecimal => return (Box::new(constant.clone()), true),
                types::ETString => {
                    let double = match datum.ConvertTo(
                        ctx.GetEvalCtx().TypeCtx(),
                        &types::NewFieldType(mysql::TypeDouble),
                    ) {
                        Ok(double) => double.GetFloat64(),
                        Err(_) => return (Box::new(constant.clone()), false),
                    };
                    if double != double.trunc() {
                        return (Box::new(constant.clone()), true);
                    }
                    return (
                        Box::new(clone_constant_with_type(constant, integer, target)),
                        false,
                    );
                }
                _ => {}
            }
        }
        _ => {}
    }
    (Box::new(constant.clone()), false)
}

/// Gets a typed temporal default value from a string, integer, or datum input.
///
/// 从字符串、整数或 Datum 解析带类型的默认时间值（列默认值场景）。
pub fn GetTimeValue<V: Any>(
    ctx: &dyn BuildContext,
    input: V,
    target_type: u8,
    fsp: isize,
    explicit_timezone: Option<chrono_tz::Tz>,
) -> Result<types::Datum, Error> {
    let mut type_context = ctx.GetEvalCtx().TypeCtx();
    if let Some(timezone) = explicit_timezone {
        type_context = type_context.WithLocation(timezone);
    }
    let input = &input as &dyn Any;
    let string_value = input
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| input.downcast_ref::<String>().map(String::as_str));
    let value = if let Some(value) = string_value {
        let lowered = value.to_ascii_lowercase();
        if matches!(lowered.as_str(), ast::CurrentTimestamp | ast::CurrentDate) {
            let now = ctx
                .GetEvalCtx()
                .CurrentTime()
                .map_err(|error| errors::New(error.to_string()))?;
            let mut value = types_dependency::time::NewTime(
                types_dependency::time::FromDate(
                    now.year(),
                    now.month() as i32,
                    now.day() as i32,
                    if lowered == ast::CurrentDate {
                        0
                    } else {
                        now.hour() as i32
                    },
                    if lowered == ast::CurrentDate {
                        0
                    } else {
                        now.minute() as i32
                    },
                    if lowered == ast::CurrentDate {
                        0
                    } else {
                        now.second() as i32
                    },
                    if lowered == ast::CurrentDate {
                        0
                    } else {
                        now.timestamp_subsec_micros() as i32
                    },
                ),
                target_type,
                fsp as i32,
            );
            value = value
                .RoundFrac(&type_context, fsp as i32)
                .map_err(|error| errors::New(error.to_string()))?;
            value
        } else if value == types_dependency::time::ZeroDatetimeStr {
            types_dependency::time::ParseTimeFromNum(&type_context, 0, target_type, fsp as i32)
                .map_err(|error| errors::New(error.to_string()))?
        } else {
            types_dependency::time::ParseTime(&type_context, value, target_type, fsp as i32)
                .map_err(|error| errors::New(error.to_string()))?
        }
    } else if let Some(value) = input.downcast_ref::<i64>() {
        types_dependency::time::ParseTimeFromNum(&type_context, *value, target_type, fsp as i32)
            .map_err(|error| errors::New(error.to_string()))?
    } else if let Some(value) = input.downcast_ref::<types::Datum>() {
        match value.Kind() {
            types::KindNull => return Ok(types::Datum::default()),
            types::KindString => types_dependency::time::ParseTime(
                &type_context,
                &value.GetString(),
                target_type,
                fsp as i32,
            )
            .map_err(|error| errors::New(error.to_string()))?,
            types::KindInt64 => types_dependency::time::ParseTimeFromNum(
                &type_context,
                value.GetInt64(),
                target_type,
                fsp as i32,
            )
            .map_err(|error| errors::New(error.to_string()))?,
            _ => return Err(errors::New("invalid default value")),
        }
    } else {
        return Ok(types::Datum::default());
    };
    let mut datum = types::Datum::default();
    datum.SetMysqlTime(value);
    Ok(datum)
}
