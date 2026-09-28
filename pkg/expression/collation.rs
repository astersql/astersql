// Copyright 2020 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 表达式排序规则（collation）与字符集推导。
//
// 对应 Go `collation.go`：维护 coercibility / repertoire 元数据，按 MySQL 规则聚合多参数的
// 字符集与排序规则，并在构造标量函数时派生返回值的 collation。coercibility 表示强制转换优先级。

use std::any::Any;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crate::*;

/// ExprCollation 汇总一次表达式推导得到的 coercibility、repertoire、字符集和排序规则。
pub struct ExprCollation {
    pub Coer: Coercibility,
    pub Repe: Repertoire,
    pub Charset: String,
    pub Collation: String,
}

/// collationInfo 对应表达式内部可变的排序规则元数据，并实现 HashEquals 所需字段。
pub struct collationInfo {
    coer: AtomicI32,
    coerInit: AtomicBool,
    pub(crate) repertoire: Repertoire,
    pub(crate) charset: String,
    pub(crate) collation: String,
    isExplicitCharset: bool,
}

impl Default for collationInfo {
    fn default() -> Self {
        Self {
            coer: AtomicI32::new(0),
            coerInit: AtomicBool::new(false),
            repertoire: 0,
            charset: String::new(),
            collation: String::new(),
            isExplicitCharset: false,
        }
    }
}

impl Clone for collationInfo {
    fn clone(&self) -> Self {
        Self {
            coer: AtomicI32::new(self.coer.load(Ordering::SeqCst)),
            coerInit: AtomicBool::new(self.coerInit.load(Ordering::SeqCst)),
            repertoire: self.repertoire,
            charset: self.charset.clone(),
            collation: self.collation.clone(),
            isExplicitCharset: self.isExplicitCharset,
        }
    }
}

impl collationInfo {
    /// Hash64 按 Go 字段顺序写入哈希器，原子字段读取采用顺序一致语义。
    pub fn Hash64(&self, h: &mut dyn base::Hasher) {
        h.HashInt64(self.coer.load(Ordering::SeqCst) as i64);
        h.HashBool(self.coerInit.load(Ordering::SeqCst));
        h.HashInt(self.repertoire as isize);
        h.HashString(&self.charset);
        h.HashString(&self.collation);
        h.HashBool(self.isExplicitCharset);
    }

    /// Equals 接受值或引用形式的 collationInfo；其他动态类型均不相等。
    pub fn Equals(&self, other: &dyn Any) -> bool {
        let Some(c2) = other.downcast_ref::<collationInfo>() else {
            return false;
        };
        self.coer.load(Ordering::SeqCst) == c2.coer.load(Ordering::SeqCst)
            && self.coerInit.load(Ordering::SeqCst) == c2.coerInit.load(Ordering::SeqCst)
            && self.repertoire == c2.repertoire
            && self.charset == c2.charset
            && self.collation == c2.collation
            && self.isExplicitCharset == c2.isExplicitCharset
    }

    /// HasCoercibility 返回 coercibility 是否已经显式初始化。
    pub fn HasCoercibility(&self) -> bool {
        self.coerInit.load(Ordering::SeqCst)
    }

    /// Coercibility 原子读取当前强制转换级别。
    pub fn Coercibility(&self) -> Coercibility {
        self.coer.load(Ordering::SeqCst)
    }

    /// SetCoercibility 先写级别再发布初始化标志，保留 Go 的并发可见性。
    pub fn SetCoercibility(&self, val: Coercibility) {
        self.coer.store(val, Ordering::SeqCst);
        self.coerInit.store(true, Ordering::SeqCst);
    }

    /// Repertoire 返回字符覆盖范围位图。
    pub fn Repertoire(&self) -> Repertoire {
        self.repertoire
    }

    /// SetRepertoire 更新字符覆盖范围。
    pub fn SetRepertoire(&mut self, value: Repertoire) {
        self.repertoire = value;
    }

    /// SetCharsetAndCollation 成对更新字符集与排序规则。
    pub fn SetCharsetAndCollation(&mut self, chs: String, coll: String) {
        self.charset = chs;
        self.collation = coll;
    }

    /// CharsetAndCollation 返回字符集和排序规则的独立副本。
    pub fn CharsetAndCollation(&self) -> (String, String) {
        (self.charset.clone(), self.collation.clone())
    }

    /// IsExplicitCharset 表示字符集是否由用户显式指定。
    pub fn IsExplicitCharset(&self) -> bool {
        self.isExplicitCharset
    }

    /// SetExplicitCharset 更新显式字符集标志。
    pub fn SetExplicitCharset(&mut self, explicit: bool) {
        self.isExplicitCharset = explicit;
    }
}

/// CollationInfo 对应 Go 接口，集中定义表达式排序规则元数据的读写能力。
pub trait CollationInfo {
    fn HasCoercibility(&self) -> bool;
    fn Coercibility(&self) -> Coercibility;
    fn SetCoercibility(&self, val: Coercibility);
    fn Repertoire(&self) -> Repertoire;
    fn SetRepertoire(&mut self, value: Repertoire);
    fn CharsetAndCollation(&self) -> (String, String);
    fn SetCharsetAndCollation(&mut self, chs: String, coll: String);
    fn IsExplicitCharset(&self) -> bool;
    fn SetExplicitCharset(&mut self, explicit: bool);
}

impl CollationInfo for collationInfo {
    fn HasCoercibility(&self) -> bool {
        self.HasCoercibility()
    }
    fn Coercibility(&self) -> Coercibility {
        self.Coercibility()
    }
    fn SetCoercibility(&self, val: Coercibility) {
        self.SetCoercibility(val);
    }
    fn Repertoire(&self) -> Repertoire {
        self.Repertoire()
    }
    fn SetRepertoire(&mut self, value: Repertoire) {
        self.SetRepertoire(value);
    }
    fn CharsetAndCollation(&self) -> (String, String) {
        self.CharsetAndCollation()
    }
    fn SetCharsetAndCollation(&mut self, chs: String, coll: String) {
        self.SetCharsetAndCollation(chs, coll);
    }
    fn IsExplicitCharset(&self) -> bool {
        self.IsExplicitCharset()
    }
    fn SetExplicitCharset(&mut self, explicit: bool) {
        self.SetExplicitCharset(explicit);
    }
}

/// FieldType 使用 parser 层哈希接口；表达式层按同一字段顺序写入自身哈希器。
pub(crate) fn hashFieldType(hasher: &mut dyn base::Hasher, field_type: &types::FieldType) {
    hasher.HashByte(field_type.GetType());
    hasher.HashInt64(field_type.GetFlag() as i64);
    hasher.HashInt(field_type.GetFlen());
    hasher.HashInt(field_type.GetDecimal());
    hasher.HashString(field_type.GetCharset());
    hasher.HashString(field_type.GetCollate());
}

/// Coercibility 数值越小优先级越高，取值与 MySQL COLLATION coercibility 规则一致。
pub type Coercibility = i32;
/// 显式 COLLATE / CONVERT 指定，最高优先级。
pub const CoercibilityExplicit: Coercibility = 0;
/// 冲突或无法确定时的占位级别。
pub const CoercibilityNone: Coercibility = 1;
/// 列引用等隐式来源。
pub const CoercibilityImplicit: Coercibility = 2;
/// 系统常量（如 USER() 返回值）。
pub const CoercibilitySysconst: Coercibility = 3;
/// 字符串字面量，较易被强制转换。
pub const CoercibilityCoercible: Coercibility = 4;
/// 数值等非字符串类型。
pub const CoercibilityNumeric: Coercibility = 5;
/// NULL 等可忽略项，最低优先级。
pub const CoercibilityIgnorable: Coercibility = 6;

/// CollationStrictnessGroup 把常用排序规则映射到由弱到强的比较组。
pub static CollationStrictnessGroup: LazyLock<HashMap<&'static str, i32>> = LazyLock::new(|| {
    HashMap::from([
        ("utf8_general_ci", 1),
        ("utf8mb4_general_ci", 1),
        ("utf8_unicode_ci", 2),
        ("utf8mb4_unicode_ci", 2),
        (charset::CollationASCII, 3),
        (charset::CollationLatin1, 3),
        (charset::CollationUTF8, 3),
        (charset::CollationUTF8MB4, 3),
        (charset::CollationBin, 4),
    ])
});

/// CollationStrictness 给出每个组可安全加强到的组号。
pub static CollationStrictness: LazyLock<HashMap<i32, Vec<i32>>> =
    LazyLock::new(|| HashMap::from([(1, vec![3, 4]), (2, vec![3, 4]), (3, vec![4]), (4, vec![])]));

/// Repertoire 是字符覆盖范围位图；只有字符串表达式的值有实际意义。
pub type Repertoire = i32;
/// 仅含 ASCII 可表示字符。
pub const ASCII: Repertoire = 0x01;
/// 含 ASCII 以外的扩展字符。
pub const EXTENDED: Repertoire = ASCII << 1;
/// ASCII 与扩展字符的并集，表示完整 Unicode 覆盖。
pub const UNICODE: Repertoire = ASCII | EXTENDED;

/// CollationInput is the expression metadata consumed by collation aggregation.
/// It is intentionally value-only so callers that already have expression metadata
/// do not need to fabricate an executable expression merely to apply MySQL's rules.
/// 排序规则聚合所需的纯值元数据；调用方无需构造可执行表达式即可套用 MySQL 规则。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollationInput {
    pub coercibility: Coercibility,
    pub repertoire: Repertoire,
    pub charset: String,
    pub collation: String,
}

impl CollationInput {
    /// 由 coercibility、repertoire、字符集与排序规则构造输入。
    pub fn new(
        coercibility: Coercibility,
        repertoire: Repertoire,
        charset: &str,
        collation: &str,
    ) -> Self {
        Self {
            coercibility,
            repertoire,
            charset: charset.to_owned(),
            collation: collation.to_owned(),
        }
    }
}

/// InferCollationMetadata mirrors `inferCollation` without requiring executable
/// expressions. The expression-facing path below delegates to the same rules.
/// 不依赖可执行表达式的排序规则推断；表达式路径复用同一套规则。
pub fn InferCollationMetadata(exprs: &[CollationInput]) -> Option<ExprCollation> {
    if exprs.is_empty() {
        return Some(ExprCollation {
            Coer: CoercibilityIgnorable,
            Repe: UNICODE,
            Charset: charset::CharsetUTF8MB4.to_owned(),
            Collation: charset::CollationUTF8MB4.to_owned(),
        });
    }

    let mut repertoire = exprs[0].repertoire;
    let mut coercibility = exprs[0].coercibility;
    let mut dst_charset = exprs[0].charset.clone();
    let mut dst_collation = exprs[0].collation.clone();
    let mut unknown_charset = false;

    for arg in &exprs[1..] {
        // binary 排序规则优先：同级或更高优先级时接管结果字符集/排序规则。
        if dst_collation == charset::CollationBin || arg.collation == charset::CollationBin {
            if coercibility > arg.coercibility
                || (coercibility == arg.coercibility && arg.collation == charset::CollationBin)
            {
                coercibility = arg.coercibility;
                dst_charset = arg.charset.clone();
                dst_collation = arg.collation.clone();
            }
            repertoire |= arg.repertoire;
            continue;
        }

        if dst_charset != arg.charset {
            let convertible = if coercibility < arg.coercibility {
                arg.repertoire == ASCII
                    || arg.coercibility >= CoercibilitySysconst
                    || isUnicodeCollation(&dst_charset)
            } else if coercibility == arg.coercibility {
                if (isUnicodeCollation(&dst_charset) && !isUnicodeCollation(&arg.charset))
                    || (dst_charset == charset::CharsetUTF8MB4
                        && arg.charset == charset::CharsetUTF8)
                {
                    true
                } else if (isUnicodeCollation(&arg.charset) && !isUnicodeCollation(&dst_charset))
                    || (arg.charset == charset::CharsetUTF8MB4
                        && dst_charset == charset::CharsetUTF8)
                    || (repertoire == ASCII && arg.repertoire != ASCII)
                {
                    coercibility = arg.coercibility;
                    dst_charset = arg.charset.clone();
                    dst_collation = arg.collation.clone();
                    true
                } else {
                    repertoire != ASCII && arg.repertoire == ASCII
                }
            } else if repertoire == ASCII
                || coercibility >= CoercibilitySysconst
                || isUnicodeCollation(&arg.charset)
            {
                coercibility = arg.coercibility;
                dst_charset = arg.charset.clone();
                dst_collation = arg.collation.clone();
                true
            } else {
                false
            };

            repertoire |= arg.repertoire;
            if !convertible {
                coercibility = CoercibilityNone;
                dst_charset = charset::CharsetBin.to_owned();
                dst_collation = charset::CollationBin.to_owned();
                unknown_charset = true;
            }
            continue;
        }

        if coercibility == arg.coercibility {
            if dst_collation == arg.collation {
                // Keep the left-hand collation.
            } else if coercibility == CoercibilityExplicit {
                return None;
            } else if isBinCollation(&dst_collation) {
                // A `_bin` collation wins at equal coercibility.
            } else if isBinCollation(&arg.collation) {
                dst_collation = arg.collation.clone();
            } else {
                coercibility = CoercibilityNone;
                dst_collation = getBinCollation(&arg.charset);
            }
        } else if coercibility > arg.coercibility {
            coercibility = arg.coercibility;
            dst_charset = arg.charset.clone();
            dst_collation = arg.collation.clone();
        }
        repertoire |= arg.repertoire;
    }

    if unknown_charset && coercibility != CoercibilityExplicit {
        return None;
    }
    Some(ExprCollation {
        Coer: coercibility,
        Repe: repertoire,
        Charset: dst_charset,
        Collation: dst_collation,
    })
}

/// deriveCoercibilityForScalarFunc 不应被调用，标量函数在构造时已完成推导。
pub(crate) fn deriveCoercibilityForScalarFunc(_sf: &ScalarFunction) -> Coercibility {
    panic!("this function should never be called")
}

/// deriveCoercibilityForConstant 按 NULL、非字符串、字符串字面量依次确定优先级。
pub(crate) fn deriveCoercibilityForConstant(c: &Constant) -> Coercibility {
    if c.Value.IsNull() {
        CoercibilityIgnorable
    } else if c.RetType.as_ref().unwrap().EvalType() != types::ETString {
        CoercibilityNumeric
    } else {
        CoercibilityCoercible
    }
}

/// deriveCoercibilityForColumn 为 NULL 列、BIT/字符串/JSON 列和数值列推导级别。
pub(crate) fn deriveCoercibilityForColumn(c: &Column) -> Coercibility {
    let ret_type = c.RetType.as_ref().unwrap();
    if ret_type.GetType() == mysql::TypeNull {
        return CoercibilityIgnorable;
    }
    if types::IsTypeBit(ret_type) {
        return CoercibilityImplicit;
    }
    match ret_type.EvalType() {
        types::ETJson | types::ETString => CoercibilityImplicit,
        _ => CoercibilityNumeric,
    }
}

/// deriveCollation 按内建函数语义选择参与聚合的参数，并为特殊返回值覆盖元数据。
fn deriveCollation(
    ctx: &dyn BuildContext,
    funcName: &str,
    args: &[Box<dyn Expression>],
    retType: types::EvalType,
    argTps: &[types::EvalType],
) -> Result<ExprCollation, Error> {
    let derive = |picked: Vec<&dyn Expression>, eval_type| {
        CheckAndDeriveCollationFromExprs(ctx, funcName, eval_type, &picked)
    };
    match funcName {
        ast::Concat
        | ast::ConcatWS
        | ast::Lower
        | ast::Lcase
        | ast::Reverse
        | ast::Upper
        | ast::Ucase
        | ast::Quote
        | ast::Coalesce
        | ast::Greatest
        | ast::Least => return derive(args.iter().map(|a| a.as_ref()).collect(), retType),
        ast::Left
        | ast::Right
        | ast::Repeat
        | ast::Trim
        | ast::LTrim
        | ast::RTrim
        | ast::Substr
        | ast::SubstringIndex
        | ast::Replace
        | ast::Substring
        | ast::Mid
        | ast::Translate => return derive(vec![args[0].as_ref()], retType),
        ast::InsertFunc => return derive(vec![args[0].as_ref(), args[3].as_ref()], retType),
        ast::Lpad | ast::Rpad => return derive(vec![args[0].as_ref(), args[2].as_ref()], retType),
        ast::Elt | ast::ExportSet | ast::MakeSet => {
            return derive(args[1..].iter().map(|a| a.as_ref()).collect(), retType);
        }
        ast::FindInSet | ast::Regexp => {
            return derive(args.iter().map(|a| a.as_ref()).collect(), types::ETInt);
        }
        ast::Field if argTps[0] == types::ETString => {
            return derive(args.iter().map(|a| a.as_ref()).collect(), retType);
        }
        ast::RegexpReplace => {
            return derive(
                vec![args[0].as_ref(), args[1].as_ref(), args[2].as_ref()],
                retType,
            );
        }
        ast::Locate
        | ast::Instr
        | ast::Position
        | ast::RegexpLike
        | ast::RegexpSubstr
        | ast::RegexpInStr => return derive(vec![args[0].as_ref(), args[1].as_ref()], retType),
        ast::GE | ast::LE | ast::GT | ast::LT | ast::EQ | ast::NE | ast::NullEQ | ast::Strcmp
            if argTps[0] == types::ETString =>
        {
            let mut ec = derive(args.iter().map(|a| a.as_ref()).collect(), types::ETInt)?;
            ec.Coer = CoercibilityNumeric;
            ec.Repe = ASCII;
            return Ok(ec);
        }
        ast::If => return derive(vec![args[1].as_ref(), args[2].as_ref()], retType),
        ast::Ifnull => return derive(vec![args[0].as_ref(), args[1].as_ref()], retType),
        ast::Like | ast::Ilike => {
            let mut ec = derive(vec![args[0].as_ref(), args[1].as_ref()], types::ETInt)?;
            ec.Coer = CoercibilityNumeric;
            ec.Repe = ASCII;
            return Ok(ec);
        }
        ast::In if args[0].GetType(ctx.GetEvalCtx()).EvalType() == types::ETString => {
            return derive(args.iter().map(|a| a.as_ref()).collect(), types::ETInt);
        }
        ast::DateFormat | ast::TimeFormat => {
            let (chs, coll) = ctx.GetCharsetInfo();
            return Ok(ExprCollation {
                Coer: args[1].Coercibility(),
                Repe: args[1].Repertoire(),
                Charset: chs,
                Collation: coll,
            });
        }
        ast::Cast => {
            // CAST 默认视为隐式；转成字符串时改用 connection 字符集与排序规则。
            let tp = args[0].GetType(ctx.GetEvalCtx());
            let mut ec = ExprCollation {
                Coer: args[0].Coercibility(),
                Repe: args[0].Repertoire(),
                Charset: tp.GetCharset().to_owned(),
                Collation: tp.GetCollate().to_owned(),
            };
            if retType == types::ETString {
                (ec.Charset, ec.Collation) = ctx.GetCharsetInfo();
            }
            return Ok(ec);
        }
        ast::Case if argTps[1] == types::ETString => {
            // CASE 只聚合 THEN 与 ELSE；奇数下标是 THEN，奇数参数总数的末项是 ELSE。
            let mut picked: Vec<&dyn Expression> = (1..args.len())
                .step_by(2)
                .map(|i| args[i].as_ref())
                .collect();
            if args.len() % 2 == 1 {
                picked.push(args[args.len() - 1].as_ref());
            }
            return derive(picked, retType);
        }
        ast::Database
        | ast::User
        | ast::CurrentUser
        | ast::Version
        | ast::CurrentRole
        | ast::TiDBVersion
        | ast::CurrentResourceGroup => {
            let (chs, coll) = charset::GetDefaultCharsetAndCollate();
            return Ok(ExprCollation {
                Coer: CoercibilitySysconst,
                Repe: UNICODE,
                Charset: chs,
                Collation: coll,
            });
        }
        ast::Format
        | ast::Space
        | ast::ToBase64
        | ast::UUID
        | ast::Hex
        | ast::MD5
        | ast::SHA
        | ast::SHA2
        | ast::SM3 => {
            let (chs, coll) = ctx.GetCharsetInfo();
            return Ok(ExprCollation {
                Coer: CoercibilityCoercible,
                Repe: ASCII,
                Charset: chs,
                Collation: coll,
            });
        }
        ast::JSONPretty | ast::JSONQuote => {
            return Ok(ExprCollation {
                Coer: CoercibilityCoercible,
                Repe: UNICODE,
                Charset: charset::CharsetUTF8MB4.into(),
                Collation: charset::CollationUTF8MB4.into(),
            });
        }
        _ => {}
    }
    let mut ec = ExprCollation {
        Coer: CoercibilityNumeric,
        Repe: ASCII,
        Charset: charset::CharsetBin.into(),
        Collation: charset::CollationBin.into(),
    };
    if retType == types::ETString {
        (ec.Charset, ec.Collation) = ctx.GetCharsetInfo();
        ec.Coer = CoercibilityCoercible;
        if ec.Charset != charset::CharsetASCII {
            ec.Repe = UNICODE;
        }
    }
    Ok(ec)
}

/// CheckAndDeriveCollationFromExprs 聚合参数排序规则，并拒绝非法混用或有损转换。
pub fn CheckAndDeriveCollationFromExprs(
    ctx: &dyn BuildContext,
    funcName: &str,
    evalType: types::EvalType,
    args: &[&dyn Expression],
) -> Result<ExprCollation, Error> {
    let mut ec = inferCollation(ctx.GetEvalCtx(), args)
        .ok_or_else(|| illegalMixCollationErr(ctx.GetEvalCtx(), funcName, args))?;
    if evalType != types::ETString && ec.Coer == CoercibilityNone {
        return Err(illegalMixCollationErr(ctx.GetEvalCtx(), funcName, args));
    }
    if evalType == types::ETString && ec.Coer == CoercibilityNumeric {
        (ec.Charset, ec.Collation) = ctx.GetCharsetInfo();
        ec.Coer = CoercibilityCoercible;
        ec.Repe = ASCII;
    }
    if !safeConvert(ctx, &ec, args) {
        return Err(illegalMixCollationErr(ctx.GetEvalCtx(), funcName, args));
    }
    Ok(fixStringTypeForMaxLength(
        ctx.GetEvalCtx(),
        funcName,
        args,
        ec,
    ))
}

/// fixStringTypeForMaxLength 对可能传播 JSON 大长度的字符串函数切换为二进制排序规则。
fn fixStringTypeForMaxLength(
    ctx: &dyn EvalContext,
    funcName: &str,
    args: &[&dyn Expression],
    mut ec: ExprCollation,
) -> ExprCollation {
    let is_json = |arg: &dyn Expression| arg.GetType(ctx).EvalType() == types::ETJson;
    let shouldChangeToBin = match funcName {
        ast::Reverse
        | ast::Lower
        | ast::Upper
        | ast::SubstringIndex
        | ast::Trim
        | ast::Quote
        | ast::InsertFunc
        | ast::Substr
        | ast::Repeat
        | ast::Replace => is_json(args[0]),
        ast::Concat | ast::ConcatWS | ast::Elt | ast::MakeSet => {
            args.iter().any(|arg| is_json(*arg))
        }
        ast::ExportSet => {
            args.get(0).is_some_and(|a| is_json(*a))
                || args.get(1).is_some_and(|a| is_json(*a))
                || args.get(2).is_some_and(|a| is_json(*a))
        }
        _ => false,
    };
    if shouldChangeToBin {
        ec.Collation = collate::ConvertAndGetBinCollation(&ec.Collation);
    }
    ec
}

/// safeConvert 验证各参数能否无损转换到目标字符集；常量会直接检查实际字节是否合法。
fn safeConvert(ctx: &dyn BuildContext, ec: &ExprCollation, args: &[&dyn Expression]) -> bool {
    let enc = charset::FindEncodingTakeUTF8AsNoop(&ec.Charset);
    for arg in args {
        let tp = arg.GetType(ctx.GetEvalCtx());
        if tp.GetCharset() == ec.Charset || arg.Repertoire() == ASCII || types::IsBinaryStr(tp) {
            continue;
        }
        if let Some(c) = arg.as_any().downcast_ref::<Constant>() {
            let Ok((value, is_null)) = c.EvalString(ctx.GetEvalCtx(), chunk::Row::default()) else {
                return false;
            };
            if !is_null && !enc.IsValid(value.as_bytes()) {
                return false;
            }
        } else if tp.GetCollate() != charset::CharsetBin
            && ec.Charset != charset::CharsetBin
            && !isUnicodeCollation(&ec.Charset)
        {
            return false;
        }
    }
    true
}

/// inferCollation 从左到右聚合参数：agg(a,b,c)=agg(agg(a,b),c)。
fn inferCollation(ctx: &dyn EvalContext, exprs: &[&dyn Expression]) -> Option<ExprCollation> {
    if exprs.is_empty() {
        let (chs, coll) = charset::GetDefaultCharsetAndCollate();
        return Some(ExprCollation {
            Coer: CoercibilityIgnorable,
            Repe: UNICODE,
            Charset: chs,
            Collation: coll,
        });
    }
    let normalized = |expr: &dyn Expression| -> (String, String) {
        let tp = expr.GetType(ctx);
        if tp.EvalType() == types::ETJson {
            (
                charset::CharsetUTF8MB4.to_owned(),
                charset::CollationUTF8MB4.to_owned(),
            )
        } else if types::IsTypeBit(tp) {
            (
                charset::CharsetBin.to_owned(),
                charset::CollationBin.to_owned(),
            )
        } else {
            (tp.GetCharset().to_owned(), tp.GetCollate().to_owned())
        }
    };
    let mut repertoire = exprs[0].Repertoire();
    let mut coercibility = exprs[0].Coercibility();
    let (mut dstCharset, mut dstCollation) = normalized(exprs[0]);
    let mut unknownCS = false;

    for arg in &exprs[1..] {
        let (argCharset, argCollation) = normalized(*arg);
        let arg_coer = arg.Coercibility();
        let arg_repe = arg.Repertoire();
        // binary charset 与任何字符集兼容；同 coercibility 时 binary 优先。
        if dstCollation == charset::CollationBin || argCollation == charset::CollationBin {
            if coercibility > arg_coer
                || (coercibility == arg_coer && argCollation == charset::CollationBin)
            {
                coercibility = arg_coer;
                dstCharset = argCharset;
                dstCollation = argCollation;
            }
            repertoire |= arg_repe;
            continue;
        }
        if dstCharset != argCharset {
            let mut converted = false;
            if coercibility < arg_coer {
                converted = arg_repe == ASCII
                    || arg_coer >= CoercibilitySysconst
                    || isUnicodeCollation(&dstCharset);
            } else if coercibility == arg_coer {
                if (isUnicodeCollation(&dstCharset) && !isUnicodeCollation(&argCharset))
                    || (dstCharset == charset::CharsetUTF8MB4 && argCharset == charset::CharsetUTF8)
                {
                    converted = true;
                } else if (isUnicodeCollation(&argCharset) && !isUnicodeCollation(&dstCharset))
                    || (argCharset == charset::CharsetUTF8MB4 && dstCharset == charset::CharsetUTF8)
                    || (repertoire == ASCII && arg_repe != ASCII)
                {
                    coercibility = arg_coer;
                    dstCharset = argCharset.clone();
                    dstCollation = argCollation.clone();
                    converted = true;
                } else if repertoire != ASCII && arg_repe == ASCII {
                    converted = true;
                }
            } else if repertoire == ASCII
                || coercibility >= CoercibilitySysconst
                || isUnicodeCollation(&argCharset)
            {
                coercibility = arg_coer;
                dstCharset = argCharset.clone();
                dstCollation = argCollation.clone();
                converted = true;
            }
            repertoire |= arg_repe;
            if converted {
                continue;
            }
            // 无损转换不可用时暂记 NONE/bin，等待后续显式 COLLATE 解开冲突。
            coercibility = CoercibilityNone;
            dstCharset = charset::CharsetBin.into();
            dstCollation = charset::CollationBin.into();
            unknownCS = true;
        } else {
            if coercibility == arg_coer {
                if dstCollation == argCollation || isBinCollation(&dstCollation) {
                } else if coercibility == CoercibilityExplicit {
                    return None;
                } else if isBinCollation(&argCollation) {
                    coercibility = arg_coer;
                    dstCharset = argCharset;
                    dstCollation = argCollation;
                } else {
                    coercibility = CoercibilityNone;
                    dstCharset = argCharset.clone();
                    dstCollation = getBinCollation(&argCharset);
                }
            } else if coercibility > arg_coer {
                coercibility = arg_coer;
                dstCharset = argCharset;
                dstCollation = argCollation;
            }
            repertoire |= arg_repe;
        }
    }
    if unknownCS && coercibility != CoercibilityExplicit {
        return None;
    }
    Some(ExprCollation {
        Coer: coercibility,
        Repe: repertoire,
        Charset: dstCharset,
        Collation: dstCollation,
    })
}

/// isUnicodeCollation 判断字符集是否属于 utf8/utf8mb4 Unicode 家族。
fn isUnicodeCollation(ch: &str) -> bool {
    ch == charset::CharsetUTF8 || ch == charset::CharsetUTF8MB4
}

/// isBinCollation 判断排序规则是否具有 coercibility 聚合所需的 `_bin` 语义。
/// 该判定不同于存储层 sortkey 是否等于原始字节：gbk_bin 在此为真，binary 在此为假。
fn isBinCollation(coll: &str) -> bool {
    matches!(
        coll,
        charset::CollationASCII
            | charset::CollationLatin1
            | charset::CollationUTF8
            | charset::CollationUTF8MB4
            | charset::CollationGBKBin
            | charset::CollationUTF8MB40900Bin
    )
}

/// getBinCollation 返回字符集对应的 `_bin` 排序规则；未知字符集记录错误并返回兜底值。
fn getBinCollation(cs: &str) -> String {
    match cs {
        charset::CharsetUTF8 => charset::CollationUTF8.into(),
        charset::CharsetUTF8MB4 => charset::CollationUTF8MB4.into(),
        charset::CharsetGBK => charset::CollationGBKBin.into(),
        _ => {
            logutil::BgLogger().error(format!("unexpected charset {cs}"));
            charset::CollationUTF8MB4.into()
        }
    }
}

/// coerString 用于非法排序规则混用错误中的可读 coercibility 名称。
static coerString: [&str; 7] = [
    "EXPLICIT",
    "NONE",
    "IMPLICIT",
    "SYSCONST",
    "COERCIBLE",
    "NUMERIC",
    "IGNORABLE",
];

/// illegalMixCollationErr 根据参数个数生成 MySQL 兼容的两参、三参或通用错误。
fn illegalMixCollationErr(
    ctx: &dyn EvalContext,
    funcName: &str,
    args: &[&dyn Expression],
) -> Error {
    let display = GetDisplayName(funcName);
    match args.len() {
        2 => errors::New(format!(
            "Illegal mix of collations ({},{}) and ({},{}) for operation '{}'",
            args[0].GetType(ctx).GetCollate(),
            coerString[args[0].Coercibility() as usize],
            args[1].GetType(ctx).GetCollate(),
            coerString[args[1].Coercibility() as usize],
            display,
        )),
        3 => errors::New(format!(
            "Illegal mix of collations ({},{}), ({},{}) and ({},{}) for operation '{}'",
            args[0].GetType(ctx).GetCollate(),
            coerString[args[0].Coercibility() as usize],
            args[1].GetType(ctx).GetCollate(),
            coerString[args[1].Coercibility() as usize],
            args[2].GetType(ctx).GetCollate(),
            coerString[args[2].Coercibility() as usize],
            display,
        )),
        _ => errors::New(format!(
            "Illegal mix of collations for operation '{display}'"
        )),
    }
}
