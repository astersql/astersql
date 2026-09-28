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

// 表达式到区间端点的构造、排序、交并与边界修正，对齐 points.go。
//
// `point` 表示 Range 的开/闭端点；builder 把比较/IN/IS NULL 等谓词
// 转成端点序列，并处理无符号、越界与 enum 等特殊边界。

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(dead_code)]

use crate::ranger_impl::{convertPointInPlace, cutPrefixForPoints};
use crate::{Range, Ranges, ast, charset, chunk, collate, errors, mysql, types};

// 表达式到区间端点的构造、排序、交并与边界修正，对齐 points.go。

// RangeType is alias for int.
// RangeType 对应 Go 的 int 别名，表示 range 构建结果面向整型、列或索引。
/// Range 构建目标类型别名（整型/列/索引）。
pub type RangeType = i32;

// RangeType constants.
// 保持 Go iota 顺序，避免改变调用方依赖的判别值。
/// 整型（表 handle）Range。
pub const IntRangeType: RangeType = 0;
/// 列 Range。
pub const ColumnRangeType: RangeType = 1;
/// 索引 Range。
pub const IndexRangeType: RangeType = 2;

// Point is the end point of range interval.
// point 是区间端点，value 保存 Datum，excl 表示开闭区间，start 表示左端点还是右端点。
#[derive(Clone, Default)]
pub(crate) struct point {
    pub(crate) value: types::Datum,
    pub(crate) excl: bool, // exclude
    pub(crate) start: bool,
}

// String implements debug formatting for range endpoints.
impl point {
    fn String(&self) -> String {
        // MinNotNull 和 MaxValue 在 Go 中被展示成负无穷和正无穷，便于阅读区间。
        let val = if self.value.Kind() == types::KindMinNotNull {
            "-inf".to_owned()
        } else if self.value.Kind() == types::KindMaxValue {
            "+inf".to_owned()
        } else {
            format!("{:?}", self.value.GetValue())
        };
        if self.start {
            let mut symbol = "[";
            if self.excl {
                symbol = "(";
            }
            return format!("{}{}", symbol, val);
        }
        let mut symbol = "]";
        if self.excl {
            symbol = ")";
        }
        format!("{}{}", val, symbol)
    }
}

// rangePointCmp 对应 Go 的 rangePointCmp，先按 Datum 值比较，再用端点开闭语义打破相等值排序。
fn rangePointCmp(
    tc: types::Context,
    a: &point,
    b: &point,
    collator: &dyn collate::Collator,
) -> Result<i32, errors::Error> {
    // enum 的比较在 Go 中走 int64 value，不能直接复用 Datum.Compare 的普通路径。
    if a.value.Kind() == types::KindMysqlEnum && b.value.Kind() == types::KindMysqlEnum {
        return rangePointEnumCmp(a, b);
    }
    let cmp = a.value.Compare(tc, &b.value, collator)?;
    if cmp != 0 {
        return Ok(cmp);
    }
    Ok(rangePointEqualValueCmp(a, b))
}

// rangePointEnumCmp compares enum endpoints; when values are equal, sort by open/closed endpoint.
fn rangePointEnumCmp(a: &point, b: &point) -> Result<i32, errors::Error> {
    let cmp = match a.value.GetInt64().cmp(&b.value.GetInt64()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    if cmp != 0 {
        return Ok(cmp);
    }
    Ok(rangePointEqualValueCmp(a, b))
}

// rangePointEqualValueCmp 处理两个端点 Datum 相等时的排序规则。
fn rangePointEqualValueCmp(a: &point, b: &point) -> i32 {
    let result = if a.start && b.start {
        !a.excl && b.excl
    } else if a.start {
        !a.excl && !b.excl
    } else if b.start {
        a.excl || b.excl
    } else {
        a.excl && !b.excl
    };
    if result {
        return -1;
    }
    0
}

// convertPointsToSortKeyInPlace converts points to sort keys in place.
fn convertPointsToSortKeyInPlace(
    sctx: &rangerctx::RangerContext,
    points: &mut Vec<point>,
    newTp: &types::FieldType,
) -> Result<(), errors::Error> {
    // Only handle normal string type here.
    // Currently, set won't be pushed down and it shouldn't reach here in theory.
    // For enum, we have separate logic for it, like handleEnumFromBinOp(). For now, it only supports point range,
    // intervals are not supported. So we also don't need to handle enum here.
    // 中文补充：只有普通字符串需要转 sort key，enum/set 分支必须绕开，避免破坏专门的 enum range 逻辑。
    if newTp.EvalType() != types::ETString
        || newTp.GetType() == mysql::TypeEnum
        || newTp.GetType() == mysql::TypeSet
    {
        return Ok(());
    }
    for p in points.iter_mut() {
        convertPointToSortKeyInPlace(sctx, p, newTp, true)?;
    }
    Ok(())
}

// convertPointToSortKeyInPlace 先按目标类型转换端点，再把新 collation 字符串转成 sort key。
fn convertPointToSortKeyInPlace(
    sctx: &rangerctx::RangerContext,
    p: &mut point,
    newTp: &types::FieldType,
    trimTrailingSpace: bool,
) -> Result<(), errors::Error> {
    // convertPointInPlace 来自同包其它 Go 文件，这里保留调用点，不在本任务虚构实现。
    convertPointInPlace(sctx, p, newTp)?;
    if p.value.Kind() != types::KindString
        || newTp.GetCollate() == charset::CollationBin
        || !collate::NewCollationEnabled()
    {
        return Ok(());
    }

    let mut sortKey = p.value.GetBytes();
    let sortKeyText =
        std::str::from_utf8(&sortKey).map_err(|error| errors::New(error.to_string()))?;
    if !trimTrailingSpace {
        // LIKE 前缀场景会选择不裁剪右侧空格，以便构造更窄的扫描范围。
        sortKey = collate::GetCollator(newTp.GetCollate()).KeyWithoutTrimRightSpace(sortKeyText);
    } else {
        sortKey = collate::GetCollator(newTp.GetCollate()).Key(sortKeyText);
    }

    p.value = types::NewBytesDatum(sortKey);
    Ok(())
}

/*
 * If use []point, fullRange will be copied when used.
 * So for keep this behaver, getFullRange function is introduced.
 */
// getFullRange 每次返回新的端点数组，保持 Go 中避免共享 fullRange slice 的行为。
pub(crate) fn getFullRange() -> Vec<point> {
    vec![
        point {
            start: true,
            ..Default::default()
        },
        point {
            value: types::MaxValueDatum(),
            ..Default::default()
        },
    ]
}

// getNotNullFullRange 返回 (-inf, +inf] 风格的非空全范围。
fn getNotNullFullRange() -> Vec<point> {
    vec![
        point {
            value: types::MinNotNullDatum(),
            start: true,
            ..Default::default()
        },
        point {
            value: types::MaxValueDatum(),
            ..Default::default()
        },
    ]
}

// FullIntRange is used for table range. Since table range cannot accept MaxValueDatum as the max value.
// So we need to set it to MaxInt64.
// FullIntRange 迁移表 range 的整型全范围，上界不能用 MaxValueDatum，必须落到具体 int/uint 边界。
/// 表 Range 的整型全范围（有符号/无符号真实边界）。
pub fn FullIntRange(isUnsigned: bool) -> Ranges {
    if isUnsigned {
        return Ranges(vec![Range {
            LowVal: vec![types::NewUintDatum(0)],
            HighVal: vec![types::NewUintDatum(u64::MAX)],
            Collators: collate::GetBinaryCollatorSlice(1),
            ..Default::default()
        }]);
    }
    Ranges(vec![Range {
        LowVal: vec![types::NewIntDatum(i64::MIN)],
        HighVal: vec![types::NewIntDatum(i64::MAX)],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Default::default()
    }])
}

// FullRange is [null, +∞) for Range.
// FullRange 保留 Go 的 [null, +inf) 通用全范围。
/// 通用全范围 [null, +inf)。
pub fn FullRange() -> Ranges {
    Ranges(vec![Range {
        LowVal: vec![types::Datum::default()],
        HighVal: vec![types::MaxValueDatum()],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Default::default()
    }])
}

// FullNotNullRange is (-∞, +∞) for Range.
// FullNotNullRange 保留 Go 的非空全范围。
/// 非空全范围。
pub fn FullNotNullRange() -> Ranges {
    Ranges(vec![Range {
        LowVal: vec![types::MinNotNullDatum()],
        HighVal: vec![types::MaxValueDatum()],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Default::default()
    }])
}

// NullRange is [null, null] for Range.
// NullRange 保留 Go 的单点 NULL 范围。
/// 单点 NULL 范围。
pub fn NullRange() -> Ranges {
    Ranges(vec![Range {
        LowVal: vec![types::Datum::default()],
        HighVal: vec![types::Datum::default()],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Default::default()
    }])
}

// builder is the range builder struct.
// builder 保存 range 构建时的首个错误和 RangerContext，行为对应 Go 的 *builder。
pub(crate) struct builder<'a, 'ctx> {
    pub(crate) err: Option<errors::Error>,
    pub(crate) sctx: &'a rangerctx::RangerContext<'ctx>,
}

impl builder<'_, '_> {
    // build converts Expression on one column into point, which can be further built into Range.
    // If the input prefixLen is not types.UnspecifiedLength, it means it's for a prefix column in a prefix index. In such
    // cases, we should cut the prefix and adjust the exclusiveness. Ref: cutPrefixForPoints().
    // convertToSortKey indicates whether the string values should be converted to sort key.
    // Converting to sort key can make `like` function be built into Range for new collation column. But we can't restore
    // the original value from the sort key, so the usage of the result may be limited, like when you need to restore the
    // result points back to Expression.
    // build 根据表达式动态类型分派到列、函数或常量构造逻辑，无法识别时返回全范围以保持正确性。
    pub(crate) fn build(
        &mut self,
        expr: &dyn expression::Expression,
        newTp: &types::FieldType,
        prefixLen: i32,
        convertToSortKey: bool,
    ) -> Vec<point> {
        if expr.as_column().is_some() {
            self.buildFromColumn()
        } else if let Some(scalar) = expr.as_scalar_function() {
            self.buildFromScalarFunc(scalar, newTp, prefixLen, convertToSortKey)
        } else if let Some(constant) = expr.as_constant() {
            self.buildFromConstant(constant)
        } else {
            getFullRange()
        }
    }

    // buildFromConstant: NULL or false cannot form a usable range.
    fn buildFromConstant(&mut self, expr: &expression::Constant) -> Vec<point> {
        let dt = match expr.Eval(self.sctx.ExprCtx.GetEvalCtx(), chunk::Row::default()) {
            Ok(value) => value,
            Err(error) => {
                self.err = Some(error);
                return Vec::new();
            }
        };
        if dt.IsNull() {
            return Vec::new();
        }

        let tc = self.sctx.TypeCtx.clone();
        let val = match dt.ToBool(tc) {
            Ok(value) => value,
            Err(error) => {
                self.err = Some(error);
                return Vec::new();
            }
        };
        if val == 0 {
            return Vec::new();
        }
        getFullRange()
    }

    // buildFromColumn 把裸列名表达式视作 col is true，对应 Go 中两个非零区间。
    fn buildFromColumn(&self) -> Vec<point> {
        // column name expression is equivalent to column name is true.
        let mut startPoint1 = point {
            value: types::MinNotNullDatum(),
            start: true,
            ..Default::default()
        };
        let mut endPoint1 = point {
            excl: true,
            ..Default::default()
        };
        endPoint1.value.SetInt64(0);
        let mut startPoint2 = point {
            excl: true,
            start: true,
            ..Default::default()
        };
        startPoint2.value.SetInt64(0);
        let endPoint2 = point {
            value: types::MaxValueDatum(),
            ..Default::default()
        };
        vec![startPoint1, endPoint1, startPoint2, endPoint2]
    }

    // buildFromBinOp 迁移单列二元比较表达式到端点数组。
    fn buildFromBinOp(
        &mut self,
        expr: &expression::ScalarFunction,
        newTp: &types::FieldType,
        prefixLen: i32,
        convertToSortKey: bool,
    ) -> Vec<point> {
        // This has been checked that the binary operation is comparison operation, and one of
        // the operand is column name expression.
        // Go 代码会先找出哪一侧是列；如果列在右边，需要反转比较操作符方向。
        let tc = self.sctx.TypeCtx.clone();
        let mut op = String::new();
        let mut value = types::Datum::default();
        let mut ft: Option<types::FieldType> = None;
        let mut col: Option<expression::Column> = None;

        if let Some(left_col) = expr.GetArgs()[0].as_column() {
            ft = left_col.RetType.clone();
            value = match expr.GetArgs()[1]
                .Eval(self.sctx.ExprCtx.GetEvalCtx(), chunk::Row::default())
            {
                Ok(value) => value,
                Err(_) => return Vec::new(),
            };
            op = expr.FuncName.L.clone();
            col = Some(left_col.clone());
        } else {
            let Some(right_col) = expr.GetArgs()[1].as_column() else {
                return Vec::new();
            };
            ft = right_col.RetType.clone();
            value = match expr.GetArgs()[0]
                .Eval(self.sctx.ExprCtx.GetEvalCtx(), chunk::Row::default())
            {
                Ok(value) => value,
                Err(_) => return Vec::new(),
            };
            // 列在右边时需要把 a < col 改写成 col > a，保持 range 统一按列在左侧构造。
            op = match expr.FuncName.L.as_str() {
                ast::GE => ast::LE.to_string(),
                ast::GT => ast::LT.to_string(),
                ast::LT => ast::GT.to_string(),
                ast::LE => ast::GE.to_string(),
                _ => expr.FuncName.L.clone(),
            };
            col = Some(right_col.clone());
        }

        if op != ast::NullEQ && value.IsNull() {
            return Vec::new();
        }

        // refineValueAndOp refines the constant datum and operator:
        // 1. for string type since we may eval the constant to another collation instead of its own collation.
        // 2. for year type since 2-digit year value need adjustment, see https://dev.mysql.com/doc/refman/5.6/en/year.html
        // 中文补充：Go 的局部闭包在 Rust 里展开成辅助调用，重点保留字符串 collation 和 YEAR 越界修正。
        if let Err(err) = refineValueAndOp(
            &self.sctx,
            tc.clone(),
            col.as_ref().unwrap(),
            &mut value,
            &mut op,
        ) {
            if op == ast::NE {
                // col != an impossible value (not valid year)
                return getNotNullFullRange();
            }
            // col = an impossible value (not valid year)
            return Vec::new();
        }

        let ft = ft.unwrap();
        let (next_value, next_op, isValidRange) = handleUnsignedCol(&ft, value, op);
        value = next_value;
        op = next_op;
        if !isValidRange {
            return Vec::new();
        }

        let (next_value, next_op, isValidRange) = handleBoundCol(&ft, value, op);
        value = next_value;
        op = next_op;
        if !isValidRange {
            return Vec::new();
        }

        if ft.GetType() == mysql::TypeEnum && ft.EvalType() == types::ETString {
            return handleEnumFromBinOp(tc, &ft, value, op);
        }

        let mut res = Vec::new();
        match op.as_str() {
            ast::NullEQ => {
                if value.IsNull() {
                    res = vec![
                        point {
                            start: true,
                            ..Default::default()
                        },
                        point::default(),
                    ]; // [null, null]
                } else {
                    res = vec![
                        point {
                            value: value.clone(),
                            start: true,
                            ..Default::default()
                        },
                        point {
                            value,
                            ..Default::default()
                        },
                    ];
                }
            }
            ast::EQ => {
                res = vec![
                    point {
                        value: value.clone(),
                        start: true,
                        ..Default::default()
                    },
                    point {
                        value,
                        ..Default::default()
                    },
                ];
            }
            ast::NE => {
                res = vec![
                    point {
                        value: types::MinNotNullDatum(),
                        start: true,
                        ..Default::default()
                    },
                    point {
                        value: value.clone(),
                        excl: true,
                        ..Default::default()
                    },
                    point {
                        value: value.clone(),
                        start: true,
                        excl: true,
                        ..Default::default()
                    },
                    point {
                        value: types::MaxValueDatum(),
                        ..Default::default()
                    },
                ];
            }
            ast::LT => {
                res = vec![
                    point {
                        value: types::MinNotNullDatum(),
                        start: true,
                        ..Default::default()
                    },
                    point {
                        value,
                        excl: true,
                        ..Default::default()
                    },
                ];
            }
            ast::LE => {
                res = vec![
                    point {
                        value: types::MinNotNullDatum(),
                        start: true,
                        ..Default::default()
                    },
                    point {
                        value,
                        ..Default::default()
                    },
                ];
            }
            ast::GT => {
                res = vec![
                    point {
                        value,
                        start: true,
                        excl: true,
                        ..Default::default()
                    },
                    point {
                        value: types::MaxValueDatum(),
                        ..Default::default()
                    },
                ];
            }
            ast::GE => {
                res = vec![
                    point {
                        value,
                        start: true,
                        ..Default::default()
                    },
                    point {
                        value: types::MaxValueDatum(),
                        ..Default::default()
                    },
                ];
            }
            _ => {}
        }
        // 前缀索引需要在构造后裁剪端点，并调整开闭区间；该函数来自同包其它迁移文件。
        cutPrefixForPoints(&mut res, prefixLen, &ft);
        if convertToSortKey {
            if let Err(err) = convertPointsToSortKeyInPlace(self.sctx, &mut res, newTp) {
                self.err = Some(err);
                return getFullRange();
            }
        }
        res
    }

    // buildFromIsTrue 迁移 IS TRUE 和 IS NOT TRUE 两种谓词的 range。
    fn buildFromIsTrue(
        &self,
        _expr: &expression::ScalarFunction,
        isNot: i32,
        keepNull: bool,
    ) -> Vec<point> {
        if isNot == 1 {
            if keepNull {
                // Range is {[0, 0]}
                let mut startPoint = point {
                    start: true,
                    ..Default::default()
                };
                startPoint.value.SetInt64(0);
                let mut endPoint = point::default();
                endPoint.value.SetInt64(0);
                return vec![startPoint, endPoint];
            }
            // NOT TRUE range is {[null null] [0, 0]}
            let startPoint1 = point {
                start: true,
                ..Default::default()
            };
            let endPoint1 = point::default();
            let mut startPoint2 = point {
                start: true,
                ..Default::default()
            };
            startPoint2.value.SetInt64(0);
            let mut endPoint2 = point::default();
            endPoint2.value.SetInt64(0);
            return vec![startPoint1, endPoint1, startPoint2, endPoint2];
        }
        // TRUE range is {[-inf 0) (0 +inf]}
        let startPoint1 = point {
            value: types::MinNotNullDatum(),
            start: true,
            ..Default::default()
        };
        let mut endPoint1 = point {
            excl: true,
            ..Default::default()
        };
        endPoint1.value.SetInt64(0);
        let mut startPoint2 = point {
            excl: true,
            start: true,
            ..Default::default()
        };
        startPoint2.value.SetInt64(0);
        let endPoint2 = point {
            value: types::MaxValueDatum(),
            ..Default::default()
        };
        vec![startPoint1, endPoint1, startPoint2, endPoint2]
    }

    // buildFromIsFalse 迁移 IS FALSE 和 IS NOT FALSE 的范围。
    fn buildFromIsFalse(&self, _expr: &expression::ScalarFunction, isNot: i32) -> Vec<point> {
        if isNot == 1 {
            // NOT FALSE range is {[-inf, 0), (0, +inf], [null, null]}
            let startPoint1 = point {
                start: true,
                ..Default::default()
            };
            let mut endPoint1 = point {
                excl: true,
                ..Default::default()
            };
            endPoint1.value.SetInt64(0);
            let mut startPoint2 = point {
                start: true,
                excl: true,
                ..Default::default()
            };
            startPoint2.value.SetInt64(0);
            let endPoint2 = point {
                value: types::MaxValueDatum(),
                ..Default::default()
            };
            return vec![startPoint1, endPoint1, startPoint2, endPoint2];
        }
        // FALSE range is {[0, 0]}
        let mut startPoint = point {
            start: true,
            ..Default::default()
        };
        startPoint.value.SetInt64(0);
        let mut endPoint = point::default();
        endPoint.value.SetInt64(0);
        vec![startPoint, endPoint]
    }

    // buildFromIn 迁移 IN 列表：常量求值、enum/year 修正、排序、去重、前缀裁剪和 sort key 转换。
    fn buildFromIn(
        &mut self,
        expr: &expression::ScalarFunction,
        newTp: &types::FieldType,
        prefixLen: i32,
        convertToSortKey: bool,
    ) -> (Vec<point>, bool) {
        let list = expr.GetArgs()[1..].to_vec();
        let mut rangePoints: Vec<point> = Vec::with_capacity(list.len() * 2);
        let mut hasNull = false;
        let ft = expr.GetArgs()[0]
            .GetType(self.sctx.ExprCtx.GetEvalCtx())
            .clone();
        let colCollate = ft.GetCollate();
        let tc = self.sctx.TypeCtx.clone();
        let evalCtx = self.sctx.ExprCtx.GetEvalCtx();

        for e in list {
            let Some(v) = e.as_constant() else {
                let message = format!(
                    "expr:{} is not constant",
                    e.StringWithCtx(None, errors::RedactLogDisable)
                );
                self.err = Some(
                    plannererrors::planner_terror::ErrUnsupportedType
                        .GenWithStack("%s", &[message.into()])
                        .into(),
                );
                return (getFullRange(), hasNull);
            };
            let mut dt = match v.Eval(evalCtx, chunk::Row::default()) {
                Ok(value) => value,
                Err(_) => {
                    let message = format!(
                        "expr:{} is not evaluated",
                        e.StringWithCtx(None, errors::RedactLogDisable)
                    );
                    self.err = Some(
                        plannererrors::planner_terror::ErrUnsupportedType
                            .GenWithStack("%s", &[message.into()])
                            .into(),
                    );
                    return (getFullRange(), hasNull);
                }
            };
            if dt.IsNull() {
                hasNull = true;
                continue;
            }

            if ft.GetType() == mysql::TypeEnum {
                match dt.Kind() {
                    types::KindString | types::KindBytes | types::KindBinaryLiteral => {
                        // Can't use ConvertTo directly, since we shouldn't convert numerical string to Enum in select stmt.
                        // 中文补充：数值字符串不能在 SELECT 的 IN 谓词里被强制当 enum 数值转换。
                        match types::ParseEnumName(ft.GetElems(), &dt.GetString(), ft.GetCollate())
                        {
                            Ok(enum_value) => {
                                dt.SetMysqlEnum(enum_value, ft.GetCollate().to_owned())
                            }
                            Err(_) => continue,
                        }
                    }
                    _ => {
                        let Ok(converted) = dt.ConvertTo(tc.clone(), &ft) else {
                            continue;
                        };
                        dt = converted;
                    }
                }
            }

            if ft.GetType() == mysql::TypeYear {
                match dt.ConvertToMysqlYear(tc.clone(), &ft) {
                    Ok(converted) => dt = converted,
                    Err(_) => {
                        // in (..., an impossible value (not valid year), ...), the range is empty, so skip it.
                        continue;
                    }
                }
            }

            if ft.EvalType() == types::ETString
                && (dt.Kind() == types::KindString || dt.Kind() == types::KindBinaryLiteral)
            {
                if ft.GetCharset() == types::charset::CharsetBin {
                    let bytes = dt.GetBytes();
                    dt.SetBytesAsString(
                        bytes.clone(),
                        ft.GetCollate().to_owned(),
                        bytes.len() as u32,
                    );
                } else {
                    dt.SetString(dt.GetString(), ft.GetCollate().to_owned());
                }
            }

            // Go 里用 pointObjs 预分配，确保 append 后指针稳定；直接 clone 两个端点表达相同语义。
            let mut startPoint = point {
                start: true,
                ..Default::default()
            };
            dt.Copy(&mut startPoint.value);
            let mut endPoint = point::default();
            dt.Copy(&mut endPoint.value);
            rangePoints.push(startPoint);
            rangePoints.push(endPoint);
        }

        let collator = collate::GetCollator(colCollate);
        rangePoints.sort_by(|a, b| {
            // 排序错误在 Go 中记录到 r.err，这里保留同样的副作用通道。
            match rangePointCmp(tc.clone(), a, b, collator.as_ref()) {
                Ok(cmpare) => cmpare.cmp(&0),
                Err(err) => {
                    self.err = Some(err);
                    std::cmp::Ordering::Equal
                }
            }
        });

        // check and remove duplicates
        // Go 的去重依赖 start/end 交替出现，这里保留相邻端点 start 标记变化的压缩逻辑。
        let mut curPos = 0usize;
        let mut frontPos = 0usize;
        while frontPos < rangePoints.len() {
            if rangePoints[curPos].start == rangePoints[frontPos].start {
                frontPos += 1;
            } else {
                curPos += 1;
                rangePoints[curPos] = rangePoints[frontPos].clone();
                frontPos += 1;
            }
        }
        if curPos > 0 {
            curPos += 1;
        }
        rangePoints.truncate(curPos);

        cutPrefixForPoints(&mut rangePoints, prefixLen, &ft);
        if convertToSortKey {
            if let Err(err) = convertPointsToSortKeyInPlace(self.sctx, &mut rangePoints, newTp) {
                self.err = Some(err);
                return (getFullRange(), false);
            }
        }
        (rangePoints, hasNull)
    }

    // newBuildFromPatternLike 迁移 LIKE 模式到范围端点的构造逻辑。
    fn newBuildFromPatternLike(
        &mut self,
        expr: &expression::ScalarFunction,
        newTp: &types::FieldType,
        prefixLen: i32,
        convertToSortKey: bool,
    ) -> Vec<point> {
        let (_, collation) = expr.CharsetAndCollation();
        if !collate::CompatibleCollate(
            expr.GetArgs()[0]
                .GetType(self.sctx.ExprCtx.GetEvalCtx())
                .GetCollate(),
            &collation,
        ) {
            return getFullRange();
        }
        let pdt = match expr.GetArgs()[1]
            .as_constant()
            .unwrap()
            .Eval(self.sctx.ExprCtx.GetEvalCtx(), chunk::Row::default())
        {
            Ok(value) => value,
            Err(error) => {
                self.err = Some(errors::Trace(error));
                return getFullRange();
            }
        };
        let tpOfPattern = expr.GetArgs()[0]
            .GetType(self.sctx.ExprCtx.GetEvalCtx())
            .clone();
        let pattern = match pdt.ToString() {
            Ok(value) => value,
            Err(error) => {
                self.err = Some(errors::Trace(error));
                return getFullRange();
            }
        };

        // non-exceptional return case 1: empty pattern
        if pattern == "" {
            let startPoint = point {
                value: types::NewStringDatum(String::new()),
                start: true,
                ..Default::default()
            };
            let endPoint = point {
                value: types::NewStringDatum(String::new()),
                ..Default::default()
            };
            let mut res = vec![startPoint, endPoint];
            if convertToSortKey {
                if let Err(err) = convertPointsToSortKeyInPlace(self.sctx, &mut res, newTp) {
                    self.err = Some(err);
                    return getFullRange();
                }
            }
            return res;
        }

        let mut lowValue: Vec<u8> = Vec::with_capacity(pattern.len());
        let edt = match expr.GetArgs()[2]
            .as_constant()
            .unwrap()
            .Eval(self.sctx.ExprCtx.GetEvalCtx(), chunk::Row::default())
        {
            Ok(value) => value,
            Err(error) => {
                self.err = Some(errors::Trace(error));
                return getFullRange();
            }
        };
        let escape = edt.GetInt64() as u8;
        let mut exclude = false;
        let mut isExactMatch = true;
        let pattern_bytes = pattern.as_bytes();
        let mut i = 0usize;
        while i < pattern_bytes.len() {
            if pattern_bytes[i] == escape {
                i += 1;
                if i < pattern_bytes.len() {
                    lowValue.push(pattern_bytes[i]);
                } else {
                    lowValue.push(escape);
                }
                i += 1;
                continue;
            }
            if pattern_bytes[i] == b'%' {
                // Get the prefix.
                isExactMatch = false;
                break;
            } else if pattern_bytes[i] == b'_' {
                // Get the prefix, but exclude the prefix.
                // e.g., "abc_x", the start point excludes "abc" because the string length is more than 3.
                // However, like the similar check in (*conditionChecker).checkLikeFunc(), in tidb's implementation, for
                // PAD SPACE collations, the trailing spaces are removed in the index key. So we are unable to distinguish
                // 'xxx' from 'xxx ' by a single index range scan. If we exclude the start point for PAD SPACE collation,
                // we will actually miss 'xxx ', which will cause wrong results.
                // 中文补充：PAD SPACE collation 下不能排除前缀端点，否则带尾随空格的数据会被漏扫。
                if !collate::IsPadSpaceCollation(&collation) {
                    exclude = true;
                }
                isExactMatch = false;
                break;
            }
            lowValue.push(pattern_bytes[i]);
            i += 1;
        }

        // non-exceptional return case 2: no characters before the wildcard
        if lowValue.is_empty() {
            return vec![
                point {
                    value: types::MinNotNullDatum(),
                    start: true,
                    ..Default::default()
                },
                point {
                    value: types::MaxValueDatum(),
                    ..Default::default()
                },
            ];
        }

        // non-exceptional return case 3: pattern contains valid characters and doesn't contain the wildcard
        if isExactMatch {
            let val = types::NewCollationStringDatum(
                String::from_utf8_lossy(&lowValue).into_owned(),
                tpOfPattern.GetCollate().to_owned(),
            );
            let startPoint = point {
                value: val.clone(),
                start: true,
                ..Default::default()
            };
            let endPoint = point {
                value: val,
                ..Default::default()
            };
            let mut res = vec![startPoint, endPoint];
            cutPrefixForPoints(&mut res, prefixLen, &tpOfPattern);
            if convertToSortKey {
                if let Err(err) = convertPointsToSortKeyInPlace(&mut self.sctx, &mut res, newTp) {
                    self.err = Some(err);
                    return getFullRange();
                }
            }
            return res;
        }

        // non-exceptional return case 4: pattern contains valid characters and contains the wildcard
        // non-exceptional return case 4-1
        // If it's not a _bin or binary collation, and we don't convert the value to the sort key, we can't build
        // a range for the wildcard.
        if !convertToSortKey && !collate::IsBinCollation(tpOfPattern.GetCollate()) {
            return vec![
                point {
                    value: types::MinNotNullDatum(),
                    start: true,
                    ..Default::default()
                },
                point {
                    value: types::MaxValueDatum(),
                    ..Default::default()
                },
            ];
        }

        // non-exceptional return case 4-2: build a range for the wildcard
        // the end_key is sortKey(start_value) + 1
        let mut originalStartPoint = point {
            start: true,
            excl: exclude,
            ..Default::default()
        };
        originalStartPoint.value.SetBytesAsString(
            lowValue,
            tpOfPattern.GetCollate().to_owned(),
            tpOfPattern.GetFlen() as u32,
        );
        // Go 传入 []*point{&originalStartPoint}，cutPrefixForPoints 会原地调整这个端点。
        let mut originalStartPoints = vec![originalStartPoint];
        cutPrefixForPoints(&mut originalStartPoints, prefixLen, &tpOfPattern);
        originalStartPoint = originalStartPoints.remove(0);

        // If we don't trim the trailing spaces, which means using KeyWithoutTrimRightSpace() instead of Key(), we can build
        // a smaller range for better performance, e.g., LIKE ' %'.
        // However, if it's a PAD SPACE collation, we must trim the trailing spaces for the start point to ensure the correctness.
        // Because the trailing spaces are trimmed in the stored index key. For example, for LIKE 'abc %' on utf8mb4_bin
        // column, the start key should be 'abd' instead of 'abc ', but the end key can be 'abc!'. ( ' ' is 32 and '!' is 33
        // in ASCII)
        // 中文补充：start key 与 end key 使用不同 trim 策略，是为了同时保证 PAD SPACE 正确性和 LIKE 扫描范围尽量窄。
        let shouldTrimTrailingSpace = collate::IsPadSpaceCollation(&collation);
        let mut startPoint = originalStartPoint.clone();
        if let Err(err) = convertPointToSortKeyInPlace(
            &mut self.sctx,
            &mut startPoint,
            newTp,
            shouldTrimTrailingSpace,
        ) {
            self.err = Some(errors::Trace(err));
            return getFullRange();
        }
        let mut sortKeyPointWithoutTrim = originalStartPoint;
        if let Err(err) =
            convertPointToSortKeyInPlace(&mut self.sctx, &mut sortKeyPointWithoutTrim, newTp, false)
        {
            self.err = Some(errors::Trace(err));
            return getFullRange();
        }
        let mut sortKeyWithoutTrim = sortKeyPointWithoutTrim.value.GetBytes().clone();
        let mut endPoint = point {
            value: types::MaxValueDatum(),
            excl: true,
            ..Default::default()
        };
        for i in (0..sortKeyWithoutTrim.len()).rev() {
            // Make the end point value more than the start point value,
            // and the length of the end point value is the same as the length of the start point value.
            // e.g., the start point value is "abc", so the end point value is "abd".
            sortKeyWithoutTrim[i] = sortKeyWithoutTrim[i].wrapping_add(1);
            if sortKeyWithoutTrim[i] != 0 {
                endPoint.value.SetBytes(sortKeyWithoutTrim);
                break;
            }
            // If sortKeyWithoutTrim[i] is 255 and sortKeyWithoutTrim[i]++ is 0, then the end point value is max value.
            if i == 0 {
                endPoint.value = types::MaxValueDatum();
            }
        }
        vec![startPoint, endPoint]
    }

    // buildFromNot 迁移 NOT 形式，包括 NOT TRUE、NOT FALSE、NOT IN、NOT LIKE 和 IS NOT NULL。
    fn buildFromNot(
        &mut self,
        expr: &expression::ScalarFunction,
        newTp: &types::FieldType,
        prefixLen: i32,
        convertToSortKey: bool,
    ) -> Vec<point> {
        match expr.FuncName.L.as_str() {
            ast::IsTruthWithoutNull => self.buildFromIsTrue(expr, 1, false),
            ast::IsTruthWithNull => self.buildFromIsTrue(expr, 1, true),
            ast::IsFalsity => self.buildFromIsFalse(expr, 1),
            ast::In => {
                let mut isUnsignedIntCol = false;
                let mut nonNegativePos = 0usize;

                // Note that we must handle the cutting prefix and converting to sort key in buildFromNot, because if we cut the
                // prefix inside buildFromIn(), the inversion logic here would make an incomplete and wrong range.
                // For example, for index col(1), col NOT IN ('aaa', 'bbb'), if we cut the prefix in buildFromIn(), we would get
                // ['a', 'a'], ['b', 'b'] from there. Then after in this function we would get ['' 'a'), ('a', 'b'), ('b', +inf]
                // as the result. This is wrong because data like 'ab' would be missed. Actually we are unable to build a range
                // for this case.
                // So we must cut the prefix in this function, therefore converting to sort key must also be done here.
                // 中文补充：NOT IN 必须先反转完整点集，再做 prefix/sort-key，否则会因为前缀裁剪丢失中间值。
                let (mut rangePoints, hasNull) =
                    self.buildFromIn(expr, newTp, types::UnspecifiedLength, false);
                if hasNull {
                    return Vec::new();
                }
                if let Some(x) = expr.GetArgs()[0].as_column() {
                    let ret_type = x.RetType.as_ref().expect("resolved column type");
                    isUnsignedIntCol = mysql::HasUnsignedFlag(ret_type.GetFlag())
                        && mysql::IsIntegerType(ret_type.GetType());
                }

                // negative ranges can be directly ignored for unsigned int columns.
                if isUnsignedIntCol {
                    while nonNegativePos < rangePoints.len() {
                        if rangePoints[nonNegativePos].value.Kind() == types::KindUint64
                            || rangePoints[nonNegativePos].value.GetInt64() >= 0
                        {
                            break;
                        }
                        nonNegativePos += 2;
                    }
                    rangePoints = rangePoints[nonNegativePos..].to_vec();
                }

                let mut retRangePoints = Vec::with_capacity(2 + rangePoints.len());
                let mut previousValue = types::Datum::default();
                let mut i = 0usize;
                while i < rangePoints.len() {
                    retRangePoints.push(point {
                        value: previousValue.clone(),
                        start: true,
                        excl: true,
                        ..Default::default()
                    });
                    retRangePoints.push(point {
                        value: rangePoints[i].value.clone(),
                        excl: true,
                        ..Default::default()
                    });
                    previousValue = rangePoints[i].value.clone();
                    i += 2;
                }
                // Append the interval (last element, max value].
                retRangePoints.push(point {
                    value: previousValue,
                    start: true,
                    excl: true,
                    ..Default::default()
                });
                retRangePoints.push(point {
                    value: types::MaxValueDatum(),
                    ..Default::default()
                });
                cutPrefixForPoints(
                    &mut retRangePoints,
                    prefixLen,
                    &expr.GetArgs()[0].GetType(self.sctx.ExprCtx.GetEvalCtx()),
                );
                if convertToSortKey {
                    if let Err(err) =
                        convertPointsToSortKeyInPlace(&mut self.sctx, &mut retRangePoints, newTp)
                    {
                        self.err = Some(err);
                        return getFullRange();
                    }
                }
                retRangePoints
            }
            ast::Like => {
                // Pattern not like is not supported.
                self.err = Some(
                    plannererrors::planner_terror::ErrUnsupportedType
                        .GenWithStack("%s", &["NOT LIKE is not supported.".into()])
                        .into(),
                );
                getFullRange()
            }
            ast::IsNull => {
                let startPoint = point {
                    value: types::MinNotNullDatum(),
                    start: true,
                    ..Default::default()
                };
                let endPoint = point {
                    value: types::MaxValueDatum(),
                    ..Default::default()
                };
                vec![startPoint, endPoint]
            }
            _ => {
                // TODO: currently we don't handle ast.LogicAnd, ast.LogicOr, ast.GT, ast.LT and so on. Most of those cases are eliminated
                // by PushDownNot but they may happen. For now, we return full range for those unhandled cases in order to keep correctness.
                // Later we need to cover those cases and set r.err when meeting some unexpected case.
                getFullRange()
            }
        }
    }

    // buildFromScalarFunc 根据函数名分派到二元比较、AND/OR、真假谓词、IN、LIKE、IS NULL 和 NOT。
    fn buildFromScalarFunc(
        &mut self,
        expr: &expression::ScalarFunction,
        newTp: &types::FieldType,
        prefixLen: i32,
        convertToSortKey: bool,
    ) -> Vec<point> {
        match expr.FuncName.L.as_str() {
            ast::GE | ast::GT | ast::LT | ast::LE | ast::EQ | ast::NE | ast::NullEQ => {
                self.buildFromBinOp(expr, newTp, prefixLen, convertToSortKey)
            }
            ast::LogicAnd => {
                let mut collator = collate::GetCollator(newTp.GetCollate());
                if convertToSortKey {
                    // sort key 已经是二进制序，后续点比较必须强制使用 bin collation。
                    collator = collate::GetCollator(charset::CollationBin);
                }
                let left = self.build(
                    expr.GetArgs()[0].as_ref(),
                    newTp,
                    prefixLen,
                    convertToSortKey,
                );
                let right = self.build(
                    expr.GetArgs()[1].as_ref(),
                    newTp,
                    prefixLen,
                    convertToSortKey,
                );
                self.intersection(left, right, collator.as_ref())
            }
            ast::LogicOr => {
                let mut collator = collate::GetCollator(newTp.GetCollate());
                if convertToSortKey {
                    collator = collate::GetCollator(charset::CollationBin);
                }
                let left = self.build(
                    expr.GetArgs()[0].as_ref(),
                    newTp,
                    prefixLen,
                    convertToSortKey,
                );
                let right = self.build(
                    expr.GetArgs()[1].as_ref(),
                    newTp,
                    prefixLen,
                    convertToSortKey,
                );
                self.union(left, right, collator.as_ref())
            }
            ast::IsTruthWithoutNull => self.buildFromIsTrue(expr, 0, false),
            ast::IsTruthWithNull => self.buildFromIsTrue(expr, 0, true),
            ast::IsFalsity => self.buildFromIsFalse(expr, 0),
            ast::In => {
                let (retPoints, _) = self.buildFromIn(expr, newTp, prefixLen, convertToSortKey);
                retPoints
            }
            ast::Like => self.newBuildFromPatternLike(expr, newTp, prefixLen, convertToSortKey),
            ast::IsNull => vec![
                point {
                    start: true,
                    ..Default::default()
                },
                point::default(),
            ],
            ast::UnaryNot => self.buildFromNot(
                expr.GetArgs()[0].as_scalar_function().unwrap(),
                newTp,
                prefixLen,
                convertToSortKey,
            ),
            _ => Vec::new(),
        }
    }

    // We need an input collator because our (*Datum).Compare(), which is used in this method, needs an explicit collator
    // input to handle comparison for string and bytes.
    // Note that if the points are converted to sort key, the collator should be set to charset.CollationBin.
    // intersection 对应 Go 的 r.merge(..., false)，用于逻辑 AND。
    pub(crate) fn intersection(
        &mut self,
        a: Vec<point>,
        b: Vec<point>,
        collator: &dyn collate::Collator,
    ) -> Vec<point> {
        self.merge(a, b, false, collator)
    }

    // We need an input collator because our (*Datum).Compare(), which is used in this method, needs an explicit collator
    // input to handle comparison for string and bytes.
    // Note that if the points are converted to sort key, the collator should be set to charset.CollationBin.
    // union 对应 Go 的 r.merge(..., true)，用于逻辑 OR。
    fn union(
        &mut self,
        a: Vec<point>,
        b: Vec<point>,
        collator: &dyn collate::Collator,
    ) -> Vec<point> {
        self.merge(a, b, true, collator)
    }

    // mergeSorted 合并两个已排序端点序列，比较出错时记录到 builder.err 并返回空序列。
    fn mergeSorted(
        &mut self,
        a: Vec<point>,
        b: Vec<point>,
        collator: &dyn collate::Collator,
    ) -> Vec<point> {
        let mut ret = Vec::with_capacity(a.len() + b.len());
        let mut i = 0usize;
        let mut j = 0usize;
        let tc = self.sctx.TypeCtx.clone();
        while i < a.len() && j < b.len() {
            let less = match rangePointCmp(tc.clone(), &a[i], &b[j], collator) {
                Ok(v) => v,
                Err(err) => {
                    self.err = Some(err);
                    return Vec::new();
                }
            };
            if less < 0 {
                ret.push(a[i].clone());
                i += 1;
            } else {
                ret.push(b[j].clone());
                j += 1;
            }
        }
        if i < a.len() {
            ret.extend_from_slice(&a[i..]);
        } else if j < b.len() {
            ret.extend_from_slice(&b[j..]);
        }
        ret
    }

    // merge 用扫描线合并端点，union=true 时进入任一范围即输出，false 时必须同时在两个范围内。
    fn merge(
        &mut self,
        a: Vec<point>,
        b: Vec<point>,
        union: bool,
        collator: &dyn collate::Collator,
    ) -> Vec<point> {
        let mut mergedPoints = self.mergeSorted(a, b, collator);
        if self.err.is_some() {
            return Vec::new();
        }

        let mut inRangeCount = 0;
        let requiredInRangeCount = if union { 1 } else { 2 };
        let mut curTail = 0usize;
        let snapshot = mergedPoints.clone();
        for val in snapshot {
            if val.start {
                inRangeCount += 1;
                if inRangeCount == requiredInRangeCount {
                    // Just reached the required in range count, a new range started.
                    // 中文补充：扫描线刚进入满足 union/intersection 条件的覆盖层数，记录左端点。
                    mergedPoints[curTail] = val;
                    curTail += 1;
                }
            } else {
                if inRangeCount == requiredInRangeCount {
                    // Just about to leave the required in range count, the range is ended.
                    // 中文补充：离开满足条件的覆盖层数前记录右端点，保持 Go 的半开/闭端点语义。
                    mergedPoints[curTail] = val;
                    curTail += 1;
                }
                inRangeCount -= 1;
            }
        }
        mergedPoints.truncate(curTail);
        mergedPoints
    }
}

// handleUnsignedCol handles the case when unsigned column meets negative value.
// The three returned values are: fixed constant value, fixed operator, and a boolean
// which indicates whether the range is valid or not.
// handleUnsignedCol 迁移 unsigned 列遇到负常量时的边界修正。
fn handleUnsignedCol(
    ft: &types::FieldType,
    mut val: types::Datum,
    mut op: String,
) -> (types::Datum, String, bool) {
    let isUnsigned = mysql::HasUnsignedFlag(ft.GetFlag());
    let isNegative = (val.Kind() == types::KindInt64 && val.GetInt64() < 0)
        || (val.Kind() == types::KindFloat32 && val.GetFloat32() < 0.0)
        || (val.Kind() == types::KindFloat64 && val.GetFloat64() < 0.0)
        || (val.Kind() == types::KindMysqlDecimal && val.GetMysqlDecimal().IsNegative());

    if !isUnsigned || !isNegative {
        return (val, op, true);
    }

    // If the operator is GT, GE or NE, the range should be [0, +inf].
    // Otherwise the value is out of valid range.
    // 中文补充：unsigned 列不可能小于负数，只有大于或不等于负数能归约成从 0 开始的非空范围。
    if op == ast::GT || op == ast::GE || op == ast::NE {
        op = ast::GE.to_string();
        match val.Kind() {
            types::KindInt64 => val.SetUint64(0),
            types::KindFloat32 => val.SetFloat32(0.0),
            types::KindFloat64 => val.SetFloat64(0.0),
            types::KindMysqlDecimal => val.SetMysqlDecimal(types::MyDecimal::default()),
            _ => {}
        }
        return (val, op, true);
    }

    (val, op, false)
}

// handleBoundCol handles the case when column meets overflow value.
// The three returned values are: fixed constant value, fixed operator, and a boolean
// which indicates whether the range is valid or not.
// handleBoundCol 处理有符号整数和 float 列遇到越界常量时的范围归约。
fn handleBoundCol(
    ft: &types::FieldType,
    mut val: types::Datum,
    mut op: String,
) -> (types::Datum, String, bool) {
    let isUnsigned = mysql::HasUnsignedFlag(ft.GetFlag());
    let isNegative = val.Kind() == types::KindInt64 && val.GetInt64() < 0;
    if isUnsigned {
        return (val, op, true);
    }

    match ft.GetType() {
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong => {
            if !isNegative && val.GetUint64() > i64::MAX as u64 {
                match op.as_str() {
                    ast::GT | ast::GE => return (val, op, false),
                    ast::NE | ast::LE | ast::LT => {
                        op = ast::LE.to_string();
                        val = types::NewIntDatum(i64::MAX);
                    }
                    _ => {}
                }
            }
        }
        mysql::TypeFloat => {
            if val.GetFloat64() > f32::MAX as f64 {
                match op.as_str() {
                    ast::GT | ast::GE => return (val, op, false),
                    ast::NE | ast::LE | ast::LT => {
                        op = ast::LE.to_string();
                        val = types::NewFloat32Datum(f32::MAX);
                    }
                    _ => {}
                }
            } else if val.GetFloat64() < -(f32::MAX as f64) {
                match op.as_str() {
                    ast::LE | ast::LT => return (val, op, false),
                    ast::GT | ast::GE | ast::NE => {
                        op = ast::GE.to_string();
                        val = types::NewFloat32Datum(-f32::MAX);
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }

    (val, op, true)
}

// handleEnumFromBinOp 迁移 enum 比较：枚举所有可能 enum 值，并保留符合比较条件的点范围。
fn handleEnumFromBinOp(
    tc: types::Context,
    ft: &types::FieldType,
    val: types::Datum,
    op: String,
) -> Vec<point> {
    let mut res: Vec<point> = Vec::with_capacity(ft.GetElems().len() * 2);
    let mut appendPointFunc = |res: &mut Vec<point>, d: types::Datum| {
        res.push(point {
            value: d.clone(),
            excl: false,
            start: true,
            ..Default::default()
        });
        res.push(point {
            value: d,
            excl: false,
            start: false,
            ..Default::default()
        });
    };

    if op == ast::NullEQ && val.IsNull() {
        res.push(point {
            start: true,
            ..Default::default()
        });
        res.push(point::default()); // null point
    }

    let mut tmpEnum = types::Enum::default();
    for i in 0..=ft.GetElems().len() {
        if i == 0 {
            tmpEnum = types::Enum::default();
        } else {
            tmpEnum.Name = ft.GetElems()[i - 1].clone();
            tmpEnum.Value = i as u64;
        }

        let d = types::NewCollateMysqlEnumDatum(tmpEnum.clone(), ft.GetCollate().to_owned());
        if let Ok(v) = d.Compare(
            tc.clone(),
            &val,
            collate::GetCollator(ft.GetCollate()).as_ref(),
        ) {
            match op.as_str() {
                ast::LT if v < 0 => appendPointFunc(&mut res, d),
                ast::LE if v <= 0 => appendPointFunc(&mut res, d),
                ast::GT if v > 0 => appendPointFunc(&mut res, d),
                ast::GE if v >= 0 => appendPointFunc(&mut res, d),
                ast::EQ | ast::NullEQ if v == 0 => appendPointFunc(&mut res, d),
                ast::NE if v != 0 => appendPointFunc(&mut res, d),
                _ => {}
            }
        }
    }
    res
}

// refineValueAndOp 对应 buildFromBinOp 内的 Go 闭包，单独提出来只是为了 Rust 可读。
fn refineValueAndOp(
    sctx: &rangerctx::RangerContext,
    tc: types::Context,
    col: &expression::Column,
    value: &mut types::Datum,
    op: &mut String,
) -> Result<(), errors::Error> {
    let column_type = col.RetType.as_ref().expect("resolved column type");
    if column_type.EvalType() == types::ETString
        && (value.Kind() == types::KindString || value.Kind() == types::KindBinaryLiteral)
    {
        if column_type.GetCharset() == types::charset::CharsetBin {
            let bytes = value.GetBytes();
            value.SetBytesAsString(
                bytes.clone(),
                column_type.GetCollate().to_owned(),
                bytes.len() as u32,
            );
        } else {
            value.SetString(value.GetString(), column_type.GetCollate().to_owned());
        }
    }
    // If nulleq with null value, values.ToInt64 will return err
    if col.GetType(sctx.ExprCtx.GetEvalCtx()).GetType() == mysql::TypeYear && !value.IsNull() {
        // Convert the out-of-range uint number to int and then let the following logic can handle it correctly.
        // Since the max value of year is 2155, `col op MaxUint` should have the same result with `col op MaxInt`.
        if value.Kind() == types::KindUint64 && value.GetUint64() > i64::MAX as u64 {
            value.SetInt64(i64::MAX);
        }

        // If the original value is adjusted, we need to change the condition.
        // For example, col < 2156. Since the max year is 2155, 2156 is changed to 2155.
        // col < 2155 is wrong. It should be col <= 2155.
        // 中文补充：YEAR 越界值会被 MySQL 年份转换截断，比较操作符必须同步放宽或保持错误。
        let preValue = value.ToInt64(tc.clone())?;
        let mut err = match value.ConvertToMysqlYear(tc.clone(), column_type) {
            Ok(converted) => {
                *value = converted;
                None
            }
            Err(error) if error.Equal(&types::ErrWarnDataOutOfRange) => {
                *value = value.ConvertToMysqlYear(
                    tc.WithFlags(tc.Flags().WithIgnoreTruncateErr(true)),
                    column_type,
                )?;
                Some(error)
            }
            Err(error) => return Err(error),
        };
        if err
            .as_ref()
            .is_some_and(|error| error.Equal(&types::ErrWarnDataOutOfRange))
        {
            // Keep err for EQ and NE.
            match op.as_str() {
                ast::GT => {
                    if value.GetInt64() > preValue {
                        *op = ast::GE.to_string();
                    }
                    err = None;
                }
                ast::LT => {
                    if value.GetInt64() < preValue {
                        *op = ast::LE.to_string();
                    }
                    err = None;
                }
                ast::GE | ast::LE => {
                    err = None;
                }
                _ => {}
            }
        }
        if let Some(error) = err {
            return Err(error);
        }
    }
    Ok(())
}
