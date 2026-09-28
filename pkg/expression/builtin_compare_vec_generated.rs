// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Vectorized comparison and COALESCE behavior generated from TiDB's Go builtins.
//
// The Go file expands the same comparison loop for every operator and evaluation
// type. Rust keeps one checked loop and supplies the original type comparator.
// SQL NULL is represented by `None`; comparison results use `Some(0 | 1)`.
//
// 向量化比较与 COALESCE（取首个非 NULL）行为，由 TiDB Go 生成式内建迁移。
// Go 按算子×求值类型展开循环；Rust 保留统一循环并注入类型比较器。
// SQL NULL 用 `None` 表示；比较结果为 `Some(0|1)`（假/真）。

use std::cmp::Ordering;
use std::fmt;

use crate::collate;

pub use types_decimal::mydecimal::MyDecimal;
pub use types_json_functions::BinaryJSON;
pub use types_time::Time;

/// Operators emitted by `expression/generator/compare_vec.go`.
/// 比较算子：由 `expression/generator/compare_vec.go` 生成。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompareOp {
    /// 小于 `<`
    Lt,
    /// 小于等于 `<=`
    Le,
    /// 大于 `>`
    Gt,
    /// 大于等于 `>=`
    Ge,
    /// 等于 `=`
    Eq,
    /// 不等于 `!=` / `<>`
    Ne,
    /// MySQL's NULL-safe equality operator (`<=>`).
    /// MySQL 的 NULL-safe 相等 `<=>`：两侧皆 NULL 为真，一侧 NULL 为假。
    NullEq,
}

/// Errors which can occur before or while evaluating a vector expression.
/// 向量求值前/求值中可能出现的错误（列长不匹配、求值失败）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvalError {
    /// 左右列行数不一致。
    ColumnLengthMismatch { expected: usize, actual: usize },
    /// 参数表达式求值失败。
    Evaluation(String),
}

impl fmt::Display for EvalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ColumnLengthMismatch { expected, actual } => {
                write!(
                    formatter,
                    "column length mismatch: expected {expected} rows, got {actual}"
                )
            }
            Self::Evaluation(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for EvalError {}

/// Minimal observable evaluation context needed by generated COALESCE.
/// COALESCE 生成代码所需的最小可观测求值上下文（警告列表）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EvalContext {
    warnings: Vec<String>,
}

impl EvalContext {
    /// 追加一条警告（warning）。
    pub fn append_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    /// 当前警告条数。
    pub fn warning_count(&self) -> usize {
        self.warnings.len()
    }

    /// 只读访问全部警告。
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// 截断警告列表到指定条数（用于回滚投机性向量求值产生的警告）。
    fn truncate_warnings(&mut self, count: usize) {
        self.warnings.truncate(count);
    }
}

/// Converts a predicate to the integer representation used by SQL builtins.
/// 将布尔谓词转为 SQL 内建使用的整型表示：真→1，假→0。
pub const fn bool_to_int64(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

/// Every signature generated in the Go source advertises vectorized execution.
/// Go 生成的每个签名均声明支持向量化执行。
pub const fn vectorized() -> bool {
    true
}

/// 将 Go 风格的 i32 比较结果（负/零/正）转为 `Ordering`。
fn ordering_from_i32(value: i32) -> Ordering {
    match value.cmp(&0) {
        Ordering::Less => Ordering::Less,
        Ordering::Equal => Ordering::Equal,
        Ordering::Greater => Ordering::Greater,
    }
}

/// 将 Go 风格的 isize 比较结果转为 `Ordering`。
fn ordering_from_isize(value: isize) -> Ordering {
    match value.cmp(&0) {
        Ordering::Less => Ordering::Less,
        Ordering::Equal => Ordering::Equal,
        Ordering::Greater => Ordering::Greater,
    }
}

/// 按算子把 `Ordering` 映射为 SQL 比较结果 0/1。
fn comparison_result(operator: CompareOp, ordering: Ordering) -> i64 {
    bool_to_int64(match operator {
        CompareOp::Lt => ordering.is_lt(),
        CompareOp::Le => ordering.is_le(),
        CompareOp::Gt => ordering.is_gt(),
        CompareOp::Ge => ordering.is_ge(),
        CompareOp::Eq | CompareOp::NullEq => ordering.is_eq(),
        CompareOp::Ne => ordering.is_ne(),
    })
}

/// 通用逐行向量比较：校验列长，处理 NULL / `<=>`，再调用类型比较器。
fn vec_compare_by<T>(
    operator: CompareOp,
    left: &[Option<T>],
    right: &[Option<T>],
    compare: impl Fn(&T, &T) -> Ordering,
) -> Result<Vec<Option<i64>>, EvalError> {
    if left.len() != right.len() {
        return Err(EvalError::ColumnLengthMismatch {
            expected: left.len(),
            actual: right.len(),
        });
    }

    // NULL 传播：普通比较任一侧 NULL → 结果 NULL；NullEq 两侧 NULL→1，一侧 NULL→0。
    Ok(left
        .iter()
        .zip(right)
        .map(|(left, right)| match (left, right) {
            (None, None) if operator == CompareOp::NullEq => Some(1),
            (None, _) | (_, None) if operator == CompareOp::NullEq => Some(0),
            (None, _) | (_, None) => None,
            (Some(left), Some(right)) => Some(comparison_result(operator, compare(left, right))),
        })
        .collect())
}

/// Go's `cmp.Compare` treats every NaN as equal to NaN and less than non-NaN.
/// Go `cmp.Compare`：NaN 等于 NaN，且小于任何非 NaN。
fn compare_real(left: f64, right: f64) -> Ordering {
    match (left.is_nan(), right.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => left.partial_cmp(&right).unwrap_or(Ordering::Equal),
    }
}

/// 浮点向量比较（含 Go 风格 NaN 序）。
pub fn vec_compare_real(
    operator: CompareOp,
    left: &[Option<f64>],
    right: &[Option<f64>],
) -> Result<Vec<Option<i64>>, EvalError> {
    vec_compare_by(operator, left, right, |left, right| {
        compare_real(*left, *right)
    })
}

/// DECIMAL（MyDecimal）向量比较。
pub fn vec_compare_decimal(
    operator: CompareOp,
    left: &[Option<MyDecimal>],
    right: &[Option<MyDecimal>],
) -> Result<Vec<Option<i64>>, EvalError> {
    vec_compare_by(operator, left, right, |left, right| {
        ordering_from_isize(left.Compare(right))
    })
}

/// 字符串向量比较：按指定 collation（校对规则）取比较器。
pub fn vec_compare_string(
    operator: CompareOp,
    left: &[Option<String>],
    right: &[Option<String>],
    collation: &str,
) -> Result<Vec<Option<i64>>, EvalError> {
    let collator = collate::GetCollator(collation);
    vec_compare_by(operator, left, right, |left, right| {
        ordering_from_i32(collator.Compare(left, right))
    })
}

/// 时间类型向量比较。
pub fn vec_compare_time(
    operator: CompareOp,
    left: &[Option<Time>],
    right: &[Option<Time>],
) -> Result<Vec<Option<i64>>, EvalError> {
    vec_compare_by(operator, left, right, |left, right| {
        ordering_from_i32((*left).Compare(*right))
    })
}

/// Durations use Go's signed `time.Duration` representation (nanoseconds).
/// Duration 使用 Go 有符号 `time.Duration`（纳秒）表示。
pub fn vec_compare_duration(
    operator: CompareOp,
    left: &[Option<i64>],
    right: &[Option<i64>],
) -> Result<Vec<Option<i64>>, EvalError> {
    vec_compare_by(operator, left, right, Ord::cmp)
}

/// Binary JSON 向量比较。
pub fn vec_compare_json(
    operator: CompareOp,
    left: &[Option<BinaryJSON>],
    right: &[Option<BinaryJSON>],
) -> Result<Vec<Option<i64>>, EvalError> {
    vec_compare_by(operator, left, right, |left, right| {
        ordering_from_i32(types_json_functions::CompareBinaryJSON(left, right))
    })
}

/// One argument to vectorized COALESCE.
///
/// The scalar entry point is intentionally separate: evaluating every argument
/// as a vector may expose an error or warning from rows which scalar COALESCE
/// would never reach.
///
/// 向量化 COALESCE 的单个参数表达式。
/// 标量入口故意分离：整列向量求值可能暴露标量短路径永远碰不到的错误/警告。
pub trait VectorExpression<T> {
    /// 按列向量求值，返回每行 `Option<T>`（`None` 即 SQL NULL）。
    fn vec_eval(&self, context: &mut EvalContext, rows: usize)
    -> Result<Vec<Option<T>>, EvalError>;

    /// 按单行求值（标量回退路径）。
    fn eval_row(&self, context: &mut EvalContext, row: usize) -> Result<Option<T>, EvalError>;
}

/// 逐行 COALESCE 回退：对每行从左到右取首个非 NULL 参数。
fn fallback_coalesce<T: Clone>(
    context: &mut EvalContext,
    rows: usize,
    arguments: &[&dyn VectorExpression<T>],
) -> Result<Vec<Option<T>>, EvalError> {
    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut selected = None;
        for argument in arguments {
            if let Some(value) = argument.eval_row(context, row)? {
                selected = Some(value);
                break;
            }
        }
        result.push(selected);
    }
    Ok(result)
}

/// Evaluates COALESCE using the generated Go algorithm.
///
/// It evaluates each argument column and fills only rows which remain NULL. If
/// vector evaluation returns an error or adds a warning, those vector-only
/// warnings are removed and the whole expression is evaluated row by row.
///
/// 按 Go 生成算法求值 COALESCE：逐参数填尚未置值的行。
/// 若向量求值出错或新增警告，则回滚警告并整式逐行回退。
pub fn vec_coalesce<T: Clone>(
    context: &mut EvalContext,
    rows: usize,
    arguments: &[&dyn VectorExpression<T>],
) -> Result<Vec<Option<T>>, EvalError> {
    let before_warnings = context.warning_count();
    let mut result = vec![None; rows];

    for argument in arguments {
        let evaluated = argument.vec_eval(context, rows);
        let after_warnings = context.warning_count();
        let added_warning = after_warnings > before_warnings;
        // 向量路径失败或产生额外警告 → 回滚警告并逐行短路径求值。
        if evaluated.is_err() || added_warning {
            if added_warning {
                context.truncate_warnings(before_warnings);
            }
            return fallback_coalesce(context, rows, arguments);
        }

        let values = evaluated.expect("checked successful vector evaluation");
        if values.len() != rows {
            return fallback_coalesce(context, rows, arguments);
        }
        // 仅填充结果中仍为 NULL 的行（首个非 NULL 胜出）。
        for (output, value) in result.iter_mut().zip(values) {
            if output.is_none() && value.is_some() {
                *output = value;
            }
        }
    }
    Ok(result)
}

/// 整型 COALESCE 向量入口。
pub fn vec_coalesce_int(
    context: &mut EvalContext,
    rows: usize,
    arguments: &[&dyn VectorExpression<i64>],
) -> Result<Vec<Option<i64>>, EvalError> {
    vec_coalesce(context, rows, arguments)
}

/// 浮点 COALESCE 向量入口。
pub fn vec_coalesce_real(
    context: &mut EvalContext,
    rows: usize,
    arguments: &[&dyn VectorExpression<f64>],
) -> Result<Vec<Option<f64>>, EvalError> {
    vec_coalesce(context, rows, arguments)
}

/// DECIMAL COALESCE 向量入口。
pub fn vec_coalesce_decimal(
    context: &mut EvalContext,
    rows: usize,
    arguments: &[&dyn VectorExpression<MyDecimal>],
) -> Result<Vec<Option<MyDecimal>>, EvalError> {
    vec_coalesce(context, rows, arguments)
}

/// 字符串 COALESCE 向量入口。
pub fn vec_coalesce_string(
    context: &mut EvalContext,
    rows: usize,
    arguments: &[&dyn VectorExpression<String>],
) -> Result<Vec<Option<String>>, EvalError> {
    vec_coalesce(context, rows, arguments)
}

/// 时间 COALESCE：合并后统一设置结果 fsp（小数秒精度）。
pub fn vec_coalesce_time(
    context: &mut EvalContext,
    rows: usize,
    arguments: &[&dyn VectorExpression<Time>],
    result_fsp: i32,
) -> Result<Vec<Option<Time>>, EvalError> {
    let mut result = vec_coalesce(context, rows, arguments)?;
    // fsp：fractional seconds precision，控制时间值小数秒位数。
    for value in result.iter_mut().flatten() {
        value.SetFsp(result_fsp);
    }
    Ok(result)
}

/// Duration COALESCE 向量入口。
pub fn vec_coalesce_duration(
    context: &mut EvalContext,
    rows: usize,
    arguments: &[&dyn VectorExpression<i64>],
) -> Result<Vec<Option<i64>>, EvalError> {
    vec_coalesce(context, rows, arguments)
}

/// JSON COALESCE 向量入口。
pub fn vec_coalesce_json(
    context: &mut EvalContext,
    rows: usize,
    arguments: &[&dyn VectorExpression<BinaryJSON>],
) -> Result<Vec<Option<BinaryJSON>>, EvalError> {
    vec_coalesce(context, rows, arguments)
}
