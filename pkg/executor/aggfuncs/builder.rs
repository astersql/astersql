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

// 聚合函数（aggregate function）实现选择器 / builder。
//
// 根据聚合描述（函数名、模式 AggMode、是否 DISTINCT、参数类型等）选择具体的
// `AggImplementation`，供哈希聚合 / 流式聚合 / 窗口函数执行器实例化。
// - `build`：普通聚合（COUNT/SUM/AVG/MAX/MIN/…）；
// - `build_window_function`：窗口函数（RANK/LEAD/LAG/…）及可滑动的 MAX/MIN。

/// 聚合计算模式：完整、两阶段部分聚合、最终合并、去重。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggMode {
    /// 单阶段完整聚合（输入原始行，输出最终结果）。
    Complete,
    /// 第一阶段部分聚合（输入原始行，输出部分结果）。
    Partial1,
    /// 第二阶段部分聚合（输入部分结果，再合并）。
    Partial2,
    /// 最终阶段：合并部分结果并产出最终值。
    Final,
    /// 去重模式（部分函数不支持）。
    Dedup,
}

/// 表达式求值类型（与 TiDB EvalType 对齐）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvalType {
    Int,
    Real,
    Decimal,
    Datetime,
    Timestamp,
    Duration,
    String,
    Json,
    VectorFloat32,
    Other,
}

/// MySQL/TiDB 字段类型种类（用于映射到 ValueKind）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldKind {
    LongLong,
    Float,
    Double,
    NewDecimal,
    Date,
    Datetime,
    Timestamp,
    Duration,
    String,
    Enum,
    Set,
    Bit,
    Json,
    VectorFloat32,
    Other,
}

/// 聚合参数/返回值的字段类型描述。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldType {
    /// 字段种类。
    pub kind: FieldKind,
    /// 求值类型。
    pub eval_type: EvalType,
    /// 是否无符号整数。
    pub unsigned: bool,
    /// 排序规则（collation）名。
    pub collation: String,
}

impl FieldType {
    /// 构造默认非 unsigned、空 collation 的字段类型。
    pub fn new(kind: FieldKind, eval_type: EvalType) -> Self {
        Self {
            kind,
            eval_type,
            unsigned: false,
            collation: String::new(),
        }
    }
}

/// 常量参数值（用于百分位、NTILE、LEAD/LAG 偏移等）。
#[derive(Clone, Debug, PartialEq)]
pub enum ConstantValue {
    Null,
    Int(i64),
    Uint(u64),
    Real(f64),
    String(String),
}

impl ConstantValue {
    /// 尝试转为 u64（非整数常量返回 0）。
    fn as_u64(&self) -> u64 {
        match self {
            Self::Int(value) => u64::try_from(*value).unwrap_or(0),
            Self::Uint(value) => *value,
            _ => 0,
        }
    }

    /// 尝试转为 i32。
    fn as_i32(&self) -> Option<i32> {
        match self {
            Self::Int(value) => i32::try_from(*value).ok(),
            Self::Uint(value) => i32::try_from(*value).ok(),
            _ => None,
        }
    }

    /// 将常量转换到目标字段类型的求值表示。
    fn converted_to(&self, field_type: &FieldType) -> Option<Self> {
        match (field_type.eval_type, self) {
            (_, Self::Null) => Some(Self::Null),
            (EvalType::Int, Self::Int(value)) => Some(Self::Int(*value)),
            (EvalType::Int, Self::Uint(value)) => i64::try_from(*value).ok().map(Self::Int),
            (EvalType::Real, Self::Int(value)) => Some(Self::Real(*value as f64)),
            (EvalType::Real, Self::Uint(value)) => Some(Self::Real(*value as f64)),
            (EvalType::Real, Self::Real(value)) => Some(Self::Real(*value)),
            (EvalType::String, Self::String(value)) => Some(Self::String(value.clone())),
            _ => None,
        }
    }
}

/// 聚合函数单个参数的描述：类型与可选常量。
#[derive(Clone, Debug, PartialEq)]
pub struct ArgDesc {
    /// 参数字段类型。
    pub field_type: FieldType,
    /// 若为常量参数则携带其值。
    pub constant: Option<ConstantValue>,
}

impl ArgDesc {
    /// 仅类型、无常量的参数。
    pub fn typed(field_type: FieldType) -> Self {
        Self {
            field_type,
            constant: None,
        }
    }

    /// 带常量值的参数。
    pub fn constant(field_type: FieldType, value: ConstantValue) -> Self {
        Self {
            field_type,
            constant: Some(value),
        }
    }
}

/// GROUP_CONCAT 等排序项：降序与 collation。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderByItem {
    /// 是否降序。
    pub descending: bool,
    /// 排序规则名。
    pub collation: String,
}

/// 支持的聚合/窗口函数名。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionName {
    Count,
    Sum,
    SumInt,
    Avg,
    FirstRow,
    Max,
    Min,
    GroupConcat,
    BitOr,
    BitXor,
    BitAnd,
    VarPop,
    StddevPop,
    JsonArrayAgg,
    JsonObjectAgg,
    ApproxCountDistinct,
    ApproxPercentile,
    VarSamp,
    StddevSamp,
    Rank,
    DenseRank,
    RowNumber,
    FirstValue,
    LastValue,
    CumeDist,
    NthValue,
    Ntile,
    PercentRank,
    Lead,
    Lag,
}

/// 聚合函数描述符：名称、模式、DISTINCT、参数与返回类型。
#[derive(Clone, Debug, PartialEq)]
pub struct AggFuncDesc {
    /// 函数名。
    pub name: FunctionName,
    /// 聚合模式。
    pub mode: AggMode,
    /// 是否带 DISTINCT。
    pub has_distinct: bool,
    /// 参数列表。
    pub args: Vec<ArgDesc>,
    /// 返回类型。
    pub return_type: FieldType,
    /// ORDER BY 项（如 GROUP_CONCAT）。
    pub order_by_items: Vec<OrderByItem>,
}

/// 构建时会话配置：窗口高精度与 GROUP_CONCAT 最大长度。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AggFuncBuildContext {
    /// 窗口聚合是否使用高精度累加。
    pub windowing_use_high_precision: bool,
    /// GROUP_CONCAT 结果最大字节长度。
    pub group_concat_max_len: u64,
}

/// 运行时值种类，用于选择具体聚合实现变体。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueKind {
    Int,
    Uint,
    Float32,
    Float64,
    Decimal,
    Time,
    Duration,
    String,
    Enum,
    Set,
    Json,
    VectorFloat32,
}

/// 选定的具体聚合实现变体（按模式/类型/DISTINCT 细分）。
#[derive(Clone, Debug, PartialEq)]
pub enum AggImplementation {
    CountOriginal(ValueKind),
    CountPartial,
    CountOriginalDistinct(ValueKind),
    CountPartialDistinct(ValueKind),
    CountOriginalDistinctMulti,
    CountPartialDistinctMulti,
    ApproxCountDistinctOriginal,
    ApproxCountDistinctPartial1,
    ApproxCountDistinctPartial2,
    ApproxCountDistinctFinal,
    Percentile { kind: ValueKind, percent: i32 },
    PercentileNull { percent: i32 },
    SumDecimal,
    SumFloat64 { high_precision: bool },
    SumOriginalDistinctDecimal,
    SumOriginalDistinctFloat64,
    SumPartialDistinctDecimal,
    SumPartialDistinctFloat64,
    SumInt,
    SumUint,
    SumDistinctInt,
    SumDistinctUint,
    AvgOriginalDecimal,
    AvgOriginalFloat64 { high_precision: bool },
    AvgOriginalDistinctDecimal,
    AvgOriginalDistinctFloat64,
    AvgPartialDecimal,
    AvgPartialFloat64,
    AvgPartialDistinctDecimal,
    AvgPartialDistinctFloat64,
    FirstRow(ValueKind),
    MaxMin { kind: ValueKind, is_max: bool },
    SlidingMaxMin { kind: ValueKind, is_max: bool },
    GroupConcat,
    GroupConcatDistinctOriginal,
    GroupConcatDistinctPartial,
    GroupConcatOrder,
    GroupConcatDistinctOrder,
    BitOr,
    BitXor,
    BitAnd,
    VarPop,
    VarPopOriginalDistinct,
    VarPopPartialDistinct,
    StddevPop,
    StddevPopOriginalDistinct,
    StddevPopPartialDistinct,
    VarSamp,
    VarSampOriginalDistinct,
    VarSampPartialDistinct,
    StddevSamp,
    StddevSampOriginalDistinct,
    StddevSampPartialDistinct,
    JsonArrayAgg,
    JsonObjectAgg,
    RowNumber,
    Rank { dense: bool },
    FirstValue(ValueKind),
    LastValue(ValueKind),
    CumeDist,
    NthValue { kind: ValueKind, nth: u64 },
    Ntile { n: u64 },
    PercentRank,
    Lead { kind: ValueKind, offset: u64 },
    Lag { kind: ValueKind, offset: u64 },
}

/// 构建完成的聚合函数元数据，供执行器实例化。
#[derive(Clone, Debug, PartialEq)]
pub struct BuiltAggFunc {
    /// 具体实现变体。
    pub implementation: AggImplementation,
    /// 结果列序号。
    pub ordinal: usize,
    /// 参与计算的参数个数。
    pub argument_count: usize,
    /// 排序项。
    pub order_by: Vec<OrderByItem>,
    /// GROUP_CONCAT 分隔符。
    pub separator: Option<String>,
    /// GROUP_CONCAT 最大长度。
    pub max_len: Option<u64>,
    /// LEAD/LAG 默认值。
    pub default_value: Option<ConstantValue>,
}

/// 从描述符构造默认 BuiltAggFunc（无 separator/max_len/default）。
fn built(desc: &AggFuncDesc, ordinal: usize, implementation: AggImplementation) -> BuiltAggFunc {
    BuiltAggFunc {
        implementation,
        ordinal,
        argument_count: desc.args.len(),
        order_by: desc.order_by_items.clone(),
        separator: None,
        max_len: None,
        default_value: None,
    }
}

/// 由字段类型推导 ValueKind；无法映射时返回 None。
fn value_kind(field_type: &FieldType) -> Option<ValueKind> {
    match field_type.kind {
        FieldKind::Enum => return Some(ValueKind::Enum),
        FieldKind::Set => return Some(ValueKind::Set),
        FieldKind::Bit => return Some(ValueKind::String),
        _ => {}
    }
    match field_type.eval_type {
        EvalType::Int => Some(if field_type.unsigned {
            ValueKind::Uint
        } else {
            ValueKind::Int
        }),
        EvalType::Real => match field_type.kind {
            FieldKind::Float => Some(ValueKind::Float32),
            _ => Some(ValueKind::Float64),
        },
        EvalType::Decimal => Some(ValueKind::Decimal),
        EvalType::Datetime | EvalType::Timestamp => Some(ValueKind::Time),
        EvalType::Duration => Some(ValueKind::Duration),
        EvalType::String => Some(ValueKind::String),
        EvalType::Json => Some(ValueKind::Json),
        EvalType::VectorFloat32 => Some(ValueKind::VectorFloat32),
        EvalType::Other => None,
    }
}

/// DISTINCT COUNT 支持的类型子集映射。
fn distinct_count_kind(field_type: &FieldType) -> Option<ValueKind> {
    match field_type.eval_type {
        EvalType::Int => Some(ValueKind::Int),
        EvalType::Real => Some(ValueKind::Float64),
        EvalType::Decimal => Some(ValueKind::Decimal),
        EvalType::Duration => Some(ValueKind::Duration),
        EvalType::String => Some(ValueKind::String),
        _ => None,
    }
}

/// 按描述符构建普通聚合实现；不支持的函数返回 None。
pub fn build(
    context: AggFuncBuildContext,
    desc: &AggFuncDesc,
    ordinal: usize,
) -> Option<BuiltAggFunc> {
    let implementation = match desc.name {
        FunctionName::Count => build_count(desc)?,
        FunctionName::Sum => build_sum(context, desc)?,
        FunctionName::SumInt => build_sum_int(desc)?,
        FunctionName::Avg => build_avg(context, desc)?,
        FunctionName::FirstRow => build_first_row(desc)?,
        FunctionName::Max => build_max_min(desc, true)?,
        FunctionName::Min => build_max_min(desc, false)?,
        FunctionName::GroupConcat => {
            return build_group_concat(context, desc, ordinal);
        }
        FunctionName::BitOr => AggImplementation::BitOr,
        FunctionName::BitXor => AggImplementation::BitXor,
        FunctionName::BitAnd => AggImplementation::BitAnd,
        FunctionName::VarPop => build_variance(desc, VarianceKind::VarPop)?,
        FunctionName::StddevPop => build_variance(desc, VarianceKind::StddevPop)?,
        FunctionName::VarSamp => build_variance(desc, VarianceKind::VarSamp)?,
        FunctionName::StddevSamp => build_variance(desc, VarianceKind::StddevSamp)?,
        FunctionName::JsonArrayAgg => {
            (desc.mode != AggMode::Dedup).then_some(AggImplementation::JsonArrayAgg)?
        }
        FunctionName::JsonObjectAgg => {
            (desc.mode != AggMode::Dedup).then_some(AggImplementation::JsonObjectAgg)?
        }
        FunctionName::ApproxCountDistinct => build_approx_count_distinct(desc)?,
        FunctionName::ApproxPercentile => build_approx_percentile(desc)?,
        _ => return None,
    };
    Some(built(desc, ordinal, implementation))
}

/// 构建窗口函数实现；对可增量滑动的 MAX/MIN 选用 SlidingMaxMin。
pub fn build_window_function(
    context: AggFuncBuildContext,
    desc: &AggFuncDesc,
    ordinal: usize,
) -> Option<BuiltAggFunc> {
    let implementation = match desc.name {
        FunctionName::Rank => AggImplementation::Rank { dense: false },
        FunctionName::DenseRank => AggImplementation::Rank { dense: true },
        FunctionName::RowNumber => AggImplementation::RowNumber,
        FunctionName::FirstValue => AggImplementation::FirstValue(value_kind(&desc.return_type)?),
        FunctionName::LastValue => AggImplementation::LastValue(value_kind(&desc.return_type)?),
        FunctionName::CumeDist => AggImplementation::CumeDist,
        FunctionName::NthValue => AggImplementation::NthValue {
            kind: value_kind(&desc.return_type)?,
            nth: desc.args.get(1)?.constant.as_ref()?.as_u64(),
        },
        FunctionName::Ntile => AggImplementation::Ntile {
            n: desc.args.first()?.constant.as_ref()?.as_u64(),
        },
        FunctionName::PercentRank => AggImplementation::PercentRank,
        FunctionName::Lead | FunctionName::Lag => {
            return build_lead_lag(desc, ordinal, desc.name == FunctionName::Lead);
        }
        FunctionName::Max | FunctionName::Min => {
            let is_max = desc.name == FunctionName::Max;
            let ordinary = build_max_min(desc, is_max)?;
            match ordinary {
                AggImplementation::MaxMin { kind, is_max }
                    if matches!(
                        kind,
                        ValueKind::Int
                            | ValueKind::Uint
                            | ValueKind::Float32
                            | ValueKind::Float64
                            | ValueKind::Decimal
                            | ValueKind::String
                            | ValueKind::Time
                            | ValueKind::Duration
                    ) =>
                {
                    AggImplementation::SlidingMaxMin { kind, is_max }
                }
                other => other,
            }
        }
        _ => return build(context, desc, ordinal),
    };
    Some(built(desc, ordinal, implementation))
}

/// 近似去重计数（HyperLogLog 类）按返回类型与模式选择阶段实现。
fn build_approx_count_distinct(desc: &AggFuncDesc) -> Option<AggImplementation> {
    match (desc.return_type.kind, desc.mode) {
        (FieldKind::LongLong, AggMode::Complete) => {
            Some(AggImplementation::ApproxCountDistinctOriginal)
        }
        (FieldKind::LongLong, AggMode::Partial1) => {
            Some(AggImplementation::ApproxCountDistinctPartial1)
        }
        (FieldKind::LongLong, AggMode::Partial2) => {
            Some(AggImplementation::ApproxCountDistinctPartial2)
        }
        (FieldKind::LongLong, AggMode::Final) => Some(AggImplementation::ApproxCountDistinctFinal),
        (FieldKind::String, AggMode::Complete | AggMode::Partial1) => {
            Some(AggImplementation::ApproxCountDistinctPartial1)
        }
        (FieldKind::String, AggMode::Partial2 | AggMode::Final) => {
            Some(AggImplementation::ApproxCountDistinctPartial2)
        }
        _ => None,
    }
}

/// 近似百分位：校验模式后按参数类型选择 Percentile / PercentileNull。
fn build_approx_percentile(desc: &AggFuncDesc) -> Option<AggImplementation> {
    if desc.mode == AggMode::Dedup
        || !matches!(
            desc.mode,
            AggMode::Complete | AggMode::Partial1 | AggMode::Final
        )
    {
        return None;
    }
    let percent = desc.args.get(1)?.constant.as_ref()?.as_i32()?;
    let argument = &desc.args.first()?.field_type;
    let eval_type = if matches!(
        argument.kind,
        FieldKind::Enum | FieldKind::Set | FieldKind::Bit
    ) {
        EvalType::String
    } else {
        argument.eval_type
    };
    Some(match eval_type {
        EvalType::Int => AggImplementation::Percentile {
            kind: ValueKind::Int,
            percent,
        },
        EvalType::Real => AggImplementation::Percentile {
            kind: ValueKind::Float64,
            percent,
        },
        EvalType::Decimal => AggImplementation::Percentile {
            kind: ValueKind::Decimal,
            percent,
        },
        EvalType::Datetime | EvalType::Timestamp => AggImplementation::Percentile {
            kind: ValueKind::Time,
            percent,
        },
        EvalType::Duration => AggImplementation::Percentile {
            kind: ValueKind::Duration,
            percent,
        },
        _ => AggImplementation::PercentileNull { percent },
    })
}

/// COUNT / COUNT(DISTINCT) 按模式与参数个数选择实现。
fn build_count(desc: &AggFuncDesc) -> Option<AggImplementation> {
    if desc.mode == AggMode::Dedup {
        return None;
    }
    if desc.has_distinct {
        let original = matches!(desc.mode, AggMode::Complete | AggMode::Partial1);
        let partial = matches!(desc.mode, AggMode::Partial2 | AggMode::Final);
        if !original && !partial {
            return None;
        }
        let single = (desc.args.len() == 1)
            .then(|| distinct_count_kind(&desc.args[0].field_type))
            .flatten();
        return Some(match (original, single) {
            (true, Some(kind)) => AggImplementation::CountOriginalDistinct(kind),
            (false, Some(kind)) => AggImplementation::CountPartialDistinct(kind),
            (true, None) => AggImplementation::CountOriginalDistinctMulti,
            (false, None) => AggImplementation::CountPartialDistinctMulti,
        });
    }
    match desc.mode {
        AggMode::Complete | AggMode::Partial1 => Some(AggImplementation::CountOriginal(
            value_kind(&desc.args.first()?.field_type)?,
        )),
        AggMode::Partial2 | AggMode::Final => Some(AggImplementation::CountPartial),
        AggMode::Dedup => None,
    }
}

/// SUM / SUM(DISTINCT) 按 Decimal/Float 与模式选择实现。
fn build_sum(context: AggFuncBuildContext, desc: &AggFuncDesc) -> Option<AggImplementation> {
    if desc.has_distinct {
        return match (desc.mode, desc.return_type.eval_type) {
            (AggMode::Complete | AggMode::Partial1, EvalType::Decimal) => {
                Some(AggImplementation::SumOriginalDistinctDecimal)
            }
            (AggMode::Complete | AggMode::Partial1, _) => {
                Some(AggImplementation::SumOriginalDistinctFloat64)
            }
            (AggMode::Final | AggMode::Partial2, EvalType::Decimal) => {
                Some(AggImplementation::SumPartialDistinctDecimal)
            }
            (AggMode::Final | AggMode::Partial2, _) => {
                Some(AggImplementation::SumPartialDistinctFloat64)
            }
            _ => None,
        };
    }
    if desc.mode == AggMode::Dedup {
        return None;
    }
    Some(match desc.return_type.eval_type {
        EvalType::Decimal => AggImplementation::SumDecimal,
        _ => AggImplementation::SumFloat64 {
            high_precision: context.windowing_use_high_precision,
        },
    })
}

/// 整数 SUM（含无符号与 DISTINCT 变体）。
fn build_sum_int(desc: &AggFuncDesc) -> Option<AggImplementation> {
    if desc.mode == AggMode::Dedup {
        return None;
    }
    Some(match (desc.return_type.unsigned, desc.has_distinct) {
        (true, true) => AggImplementation::SumDistinctUint,
        (true, false) => AggImplementation::SumUint,
        (false, true) => AggImplementation::SumDistinctInt,
        (false, false) => AggImplementation::SumInt,
    })
}

/// AVG / AVG(DISTINCT) 按模式与返回类型选择 Original/Partial 实现。
fn build_avg(context: AggFuncBuildContext, desc: &AggFuncDesc) -> Option<AggImplementation> {
    match desc.mode {
        AggMode::Dedup => None,
        AggMode::Complete | AggMode::Partial1 => {
            Some(match (desc.return_type.eval_type, desc.has_distinct) {
                (EvalType::Decimal, true) => AggImplementation::AvgOriginalDistinctDecimal,
                (EvalType::Decimal, false) => AggImplementation::AvgOriginalDecimal,
                (_, true) => AggImplementation::AvgOriginalDistinctFloat64,
                (_, false) => AggImplementation::AvgOriginalFloat64 {
                    high_precision: context.windowing_use_high_precision,
                },
            })
        }
        AggMode::Partial2 | AggMode::Final => match (desc.return_type.kind, desc.has_distinct) {
            (FieldKind::NewDecimal, true) => Some(AggImplementation::AvgPartialDistinctDecimal),
            (FieldKind::NewDecimal, false) => Some(AggImplementation::AvgPartialDecimal),
            (FieldKind::Double, true) => Some(AggImplementation::AvgPartialDistinctFloat64),
            (FieldKind::Double, false) => Some(AggImplementation::AvgPartialFloat64),
            _ => None,
        },
    }
}

/// FIRST_ROW：取组内第一行值。
fn build_first_row(desc: &AggFuncDesc) -> Option<AggImplementation> {
    (desc.mode != AggMode::Dedup)
        .then(|| value_kind(&desc.return_type).map(AggImplementation::FirstRow))?
}

/// MAX/MIN 普通（非滑动）实现。
fn build_max_min(desc: &AggFuncDesc, is_max: bool) -> Option<AggImplementation> {
    if desc.mode == AggMode::Dedup {
        return None;
    }
    Some(AggImplementation::MaxMin {
        kind: value_kind(&desc.return_type)?,
        is_max,
    })
}

/// GROUP_CONCAT：解析分隔符，并按 DISTINCT/ORDER BY 选择变体。
fn build_group_concat(
    context: AggFuncBuildContext,
    desc: &AggFuncDesc,
    ordinal: usize,
) -> Option<BuiltAggFunc> {
    if desc.mode == AggMode::Dedup {
        return None;
    }
    let separator = match desc.args.last()?.constant.as_ref()? {
        ConstantValue::String(separator) => separator.clone(),
        _ => return None,
    };
    let implementation = if desc.has_distinct && !desc.order_by_items.is_empty() {
        AggImplementation::GroupConcatDistinctOrder
    } else if desc.has_distinct {
        match desc.mode {
            AggMode::Complete | AggMode::Partial1 => AggImplementation::GroupConcatDistinctOriginal,
            AggMode::Partial2 | AggMode::Final => AggImplementation::GroupConcatDistinctPartial,
            AggMode::Dedup => return None,
        }
    } else if !desc.order_by_items.is_empty() {
        AggImplementation::GroupConcatOrder
    } else {
        AggImplementation::GroupConcat
    };
    let mut result = built(desc, ordinal, implementation);
    result.argument_count -= 1;
    result.separator = Some(separator);
    result.max_len = Some(context.group_concat_max_len);
    Some(result)
}

/// 方差/标准差种类（总体/样本）。
#[derive(Clone, Copy)]
enum VarianceKind {
    VarPop,
    StddevPop,
    VarSamp,
    StddevSamp,
}

/// VAR_POP/STDDEV_POP/VAR_SAMP/STDDEV_SAMP 及 DISTINCT 变体。
fn build_variance(desc: &AggFuncDesc, kind: VarianceKind) -> Option<AggImplementation> {
    if desc.mode == AggMode::Dedup {
        return None;
    }
    let original = matches!(desc.mode, AggMode::Complete | AggMode::Partial1);
    Some(match (kind, desc.has_distinct, original) {
        (VarianceKind::VarPop, false, _) => AggImplementation::VarPop,
        (VarianceKind::VarPop, true, true) => AggImplementation::VarPopOriginalDistinct,
        (VarianceKind::VarPop, true, false) => AggImplementation::VarPopPartialDistinct,
        (VarianceKind::StddevPop, false, _) => AggImplementation::StddevPop,
        (VarianceKind::StddevPop, true, true) => AggImplementation::StddevPopOriginalDistinct,
        (VarianceKind::StddevPop, true, false) => AggImplementation::StddevPopPartialDistinct,
        (VarianceKind::VarSamp, false, _) => AggImplementation::VarSamp,
        (VarianceKind::VarSamp, true, true) => AggImplementation::VarSampOriginalDistinct,
        (VarianceKind::VarSamp, true, false) => AggImplementation::VarSampPartialDistinct,
        (VarianceKind::StddevSamp, false, _) => AggImplementation::StddevSamp,
        (VarianceKind::StddevSamp, true, true) => AggImplementation::StddevSampOriginalDistinct,
        (VarianceKind::StddevSamp, true, false) => AggImplementation::StddevSampPartialDistinct,
    })
}

/// LEAD/LAG：解析偏移与默认值，并写入 BuiltAggFunc。
fn build_lead_lag(desc: &AggFuncDesc, ordinal: usize, is_lead: bool) -> Option<BuiltAggFunc> {
    let offset = desc
        .args
        .get(1)
        .and_then(|argument| argument.constant.as_ref())
        .map_or(1, ConstantValue::as_u64);
    let default_value = desc
        .args
        .get(2)
        .and_then(|argument| argument.constant.as_ref())
        .map(|value| {
            value
                .converted_to(&desc.return_type)
                .unwrap_or_else(|| value.clone())
        })
        .unwrap_or(ConstantValue::Null);
    let kind = value_kind(&desc.return_type)?;
    let implementation = if is_lead {
        AggImplementation::Lead { kind, offset }
    } else {
        AggImplementation::Lag { kind, offset }
    };
    let mut result = built(desc, ordinal, implementation);
    result.default_value = Some(default_value);
    Some(result)
}
