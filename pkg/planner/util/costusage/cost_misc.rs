// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// CostVer2 代价数值与追踪公式的构造、合并与缩放。
//
// 代价（cost）是优化器对候选执行计划资源消耗的估计；CostVer2 在数值之外
// 还可附带因子分解追踪（trace），便于 EXPLAIN 展示代价公式。

// costusage 与 util 隔离是为了避免 base 接口定义形成循环依赖；外部 planner 类型尚未接通。

/// 强制重新计算代价（忽略缓存）。
pub const COST_FLAG_RECALCULATE: u64 = 1;
/// 使用真实基数（cardinality，行数估计）而非统计估计。
pub const COST_FLAG_USE_TRUE_CARDINALITY: u64 = 2;
/// 启用代价公式追踪，记录各因子贡献。
pub const COST_FLAG_TRACE: u64 = 4;

/// 第二版代价：数值 + 可选追踪信息。
#[derive(Clone)]
pub struct CostVer2 {
    cost: f64,
    trace: Option<CostTrace>,
}

impl CostVer2 {
    /// 返回非负代价；NaN 原样保留（用于传播无效估计）。
    pub fn get_cost(&self) -> f64 {
        if self.cost.is_nan() {
            self.cost
        } else {
            self.cost.max(0.0)
        }
    }
    /// 返回可选的代价追踪详情。
    pub fn get_trace(&self) -> Option<&CostTrace> {
        self.trace.as_ref()
    }
}

/// 代价追踪：各因子成本与可展示的公式字符串。
#[derive(Clone)]
pub struct CostTrace {
    factor_costs: std::collections::HashMap<String, f64>,
    formula: String,
}

impl CostTrace {
    /// 返回拼接后的代价公式文本。
    pub fn get_formula(&self) -> &str {
        &self.formula
    }
    /// 返回各代价因子到成本贡献的映射。
    pub fn get_factor_costs(&self) -> &std::collections::HashMap<String, f64> {
        &self.factor_costs
    }
}

/// 构造数值为 0 的 CostVer2；`trace` 为真时附带空追踪结构。
pub fn new_zero_cost_ver2(trace: bool) -> CostVer2 {
    CostVer2 {
        cost: 0.0,
        trace: trace.then(|| CostTrace {
            factor_costs: std::collections::HashMap::new(),
            formula: String::new(),
        }),
    }
}

// ZeroCostVer2 is Go's pre-defined, untraced zero cost.
/// Go 预定义的零代价常量（不开启追踪）。
pub static ZERO_COST_VER2: std::sync::LazyLock<CostVer2> =
    std::sync::LazyLock::new(|| new_zero_cost_ver2(false));

/// 判断 `cost_flag` 位集合是否包含指定 `flag`。
pub fn has_cost_flag(cost_flag: u64, flag: u64) -> bool {
    (cost_flag & flag) > 0
}

/// 根据 PlanCostOption 判断是否需要记录代价追踪。
pub fn trace_cost(option: Option<&PlanCostOption>) -> bool {
    option.is_some_and(|op| has_cost_flag(op.cost_flag, COST_FLAG_TRACE))
}

/// 代价因子：名称与系数，用于 trace 中标注贡献来源。
pub struct CostVer2Factor {
    pub name: String,
    pub value: f64,
}
impl std::fmt::Display for CostVer2Factor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, format_go_float(self.value))
    }
}

/// Go `%v` 对 float64 使用最短 `%g` 表示，并固定拼写非有限值。
pub fn format_go_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == f64::INFINITY {
        return "+Inf".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-Inf".to_owned();
    }

    let raw = value.to_string();
    if let Some((mantissa, exponent)) = raw.split_once('e') {
        let exponent: i32 = exponent.parse().expect("Rust emits a numeric exponent");
        return format!("{mantissa}e{exponent:+03}");
    }

    let (sign, unsigned) = raw
        .strip_prefix('-')
        .map_or(("", raw.as_str()), |v| ("-", v));
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let (exponent, digits) = if integer != "0" {
        (integer.len() as i32 - 1, format!("{integer}{fraction}"))
    } else if let Some(first) = fraction.find(|ch| ch != '0') {
        (-(first as i32) - 1, fraction[first..].to_owned())
    } else {
        return raw;
    };

    if !(-4..6).contains(&exponent) {
        let mut chars = digits.chars();
        let first = chars.next().expect("non-zero value has digits");
        let rest = chars.as_str().trim_end_matches('0');
        let mantissa = if rest.is_empty() {
            format!("{sign}{first}")
        } else {
            format!("{sign}{first}.{rest}")
        };
        format!("{mantissa}e{exponent:+03}")
    } else {
        raw
    }
}

/// Go `strconv.FormatFloat(value, 'f', 2, 64)` 的展示格式。
fn format_go_fixed_2(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.2}")
    } else {
        format_go_float(value)
    }
}

/// 构造带可选追踪的 CostVer2；仅在 TRACE 开启时求值 `lazy_formula`。
pub fn new_cost_ver2(
    option: Option<&PlanCostOption>,
    factor: CostVer2Factor,
    cost: f64,
    lazy_formula: impl FnOnce() -> String,
) -> CostVer2 {
    let trace = if trace_cost(option) {
        let mut t = CostTrace {
            factor_costs: std::collections::HashMap::new(),
            formula: lazy_formula(),
        };
        t.factor_costs.insert(factor.name, cost);
        Some(t)
    } else {
        None
    };
    CostVer2 { cost, trace }
}

// Go SumCostVer2：先求成本，再合并因子；空 formula 表示 zero cost，不拼入表达式。
/// 累加多项 CostVer2：数值相加，并合并因子成本与公式（空 formula 不拼入）。
pub fn sum_cost_ver2(costs: &[CostVer2]) -> CostVer2 {
    let mut result = new_zero_cost_ver2(false);
    for cost in costs {
        result.cost += cost.cost;
        if let Some(trace) = &cost.trace {
            let out = result.trace.get_or_insert_with(|| CostTrace {
                factor_costs: std::collections::HashMap::new(),
                formula: String::new(),
            });
            for (factor, value) in &trace.factor_costs {
                *out.factor_costs.entry(factor.clone()).or_insert(0.0) += value;
            }
            // 空 formula 视为零代价项，不拼进展示公式。
            if !trace.formula.is_empty() {
                if !out.formula.is_empty() {
                    out.formula.push_str(" + ");
                }
                out.formula.push('(');
                out.formula.push_str(&trace.formula);
                out.formula.push(')');
            }
        }
    }
    result
}

/// 将代价按分母缩放；追踪中的因子与公式同步除法。
pub fn div_cost_ver2(cost: &CostVer2, denominator: f64) -> CostVer2 {
    let mut result = CostVer2 {
        cost: cost.cost / denominator,
        trace: None,
    };
    if let Some(source) = &cost.trace {
        let mut factors = std::collections::HashMap::new();
        for (name, value) in &source.factor_costs {
            factors.insert(name.clone(), value / denominator);
        }
        result.trace = Some(CostTrace {
            factor_costs: factors,
            formula: format!("({})/{}", source.formula, format_go_fixed_2(denominator)),
        });
    }
    result
}

/// 将代价按系数放大；追踪中的因子与公式同步乘法。
pub fn mul_cost_ver2(cost: &CostVer2, scale: f64) -> CostVer2 {
    let mut result = CostVer2 {
        cost: cost.cost * scale,
        trace: None,
    };
    if let Some(source) = &cost.trace {
        let factors = source
            .factor_costs
            .iter()
            .map(|(n, v)| (n.clone(), v * scale))
            .collect();
        result.trace = Some(CostTrace {
            factor_costs: factors,
            formula: format!("({})*{}", source.formula, format_go_fixed_2(scale)),
        });
    }
    result
}

/// 仅增加数值、不更新追踪（用作无追踪 tie-breaker）。
pub fn add_cost_without_trace(mut cost: CostVer2, additional: f64) -> CostVer2 {
    cost.cost += additional;
    cost
}

/// 构造默认 PlanCostOption（无任何代价标志位）。
pub fn new_default_plan_cost_option() -> PlanCostOption {
    PlanCostOption { cost_flag: 0 }
}
/// 计划代价计算选项：承载 COST_FLAG_* 位标志。
pub struct PlanCostOption {
    pub cost_flag: u64,
}
impl PlanCostOption {
    /// 设置代价标志位并返回自身（建造者模式）。
    pub fn with_cost_flag(mut self, flag: u64) -> Self {
        self.cost_flag = flag;
        self
    }
}
