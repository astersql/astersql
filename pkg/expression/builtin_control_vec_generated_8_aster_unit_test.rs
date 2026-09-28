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

// 向量化控制流生成代码的 Go 语义对齐单元测试。
//
// 覆盖 CASE/IFNULL/IF 的 NULL 规则、多类型矩阵、投机错误/警告回滚，
// 强制首参错误不可被回退隐藏，以及 21 个签名的可构造性与 vectorized 声明。
// 向量化控制流生成签名的 Aster 单元测试。
//
// 通过可脚本化的 `Scripted` 表达式模拟正常/错误/警告行为，
// 校验 CASE/IF/IFNULL 的 NULL 语义、多类型矩阵、
// 投机警告回滚与强制参数错误不被回退掩盖。

include!("builtin_control_vec_generated.rs");

#[derive(Clone, Copy)]
/// 脚本化向量行为：正常 / 仅错误 / 仅警告 / 警告并错误。
/// 向量求值脚本动作：正常、仅错误、仅警告、警告后错误。
enum VectorAction {
    Normal,
    Error,
    Warn,
    WarnAndError,
}

/// 可脚本化的表达式：按 `VectorAction` 与标量错误行驱动行为。
/// 可配置的测试用向量表达式。
struct Scripted<T> {
    values: Vec<Option<T>>,
    vector_action: VectorAction,
    scalar_error_rows: Vec<usize>,
}

impl<T> Scripted<T> {
    /// 正常向量/标量均返回给定值。
    /// 正常返回预置列。
    fn normal(values: Vec<Option<T>>) -> Self {
        Self {
            values,
            vector_action: VectorAction::Normal,
            scalar_error_rows: Vec::new(),
        }
    }

    /// 向量侧警告并错误；指定标量行也会报错。
    /// 向量路径先警告再报错；标量路径可在指定行报错。
    fn vector_hazard(values: Vec<Option<T>>, scalar_error_rows: Vec<usize>) -> Self {
        Self {
            values,
            vector_action: VectorAction::WarnAndError,
            scalar_error_rows,
        }
    }

    /// 向量侧直接错误。
    /// 向量路径直接报错。
    fn vector_error(values: Vec<Option<T>>) -> Self {
        Self {
            values,
            vector_action: VectorAction::Error,
            scalar_error_rows: Vec::new(),
        }
    }

    /// 向量侧仅追加警告仍返回值。
    /// 向量路径追加投机警告后仍返回列。
    fn vector_warning(values: Vec<Option<T>>) -> Self {
        Self {
            values,
            vector_action: VectorAction::Warn,
            scalar_error_rows: Vec::new(),
        }
    }
}

impl<T> VectorExpression<T> for Scripted<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// 标量求值；命中 `scalar_error_rows` 则报错。
    fn eval_row(&self, _ctx: &mut EvalContext, row: usize) -> Result<Option<T>, EvalError> {
        if self.scalar_error_rows.contains(&row) {
            return Err(EvalError::new(format!("scalar error at row {row}")));
        }
        Ok(self.values[row].clone())
    }

    /// 按 `vector_action` 执行向量行为。
    fn vec_eval(&self, ctx: &mut EvalContext, rows: usize) -> Result<Vec<Option<T>>, EvalError> {
        assert_eq!(rows, self.values.len());
        match self.vector_action {
            VectorAction::Normal => Ok(self.values.clone()),
            VectorAction::Error => Err(EvalError::new("vector error")),
            VectorAction::Warn => {
                ctx.append_warning("speculative warning");
                Ok(self.values.clone())
            }
            VectorAction::WarnAndError => {
                ctx.append_warning("speculative warning");
                Err(EvalError::new("vector error"))
            }
        }
    }
}

/// 装箱为普通（无危害）向量表达式。
/// 包装为正常向量表达式 trait 对象。
fn expr<T>(values: Vec<Option<T>>) -> Box<dyn VectorExpression<T>>
where
    T: Clone + Send + Sync + 'static,
{
    Box::new(Scripted::normal(values))
}

/// CASE：首个真分支与 ELSE；NULL 条件视为假。
#[test]
/// CASE：首个真分支与 ELSE；NULL 条件/结果按 SQL 三值逻辑处理。
fn case_when_uses_first_true_branch_and_else_with_sql_null_rules() {
    let mut sig = builtinCaseWhenIntSig::new(
        vec![
            (
                expr(vec![Some(0), Some(1), None, Some(1)]),
                expr(vec![Some(10), Some(11), Some(12), None]),
            ),
            (
                expr(vec![Some(1), Some(1), Some(0), Some(1)]),
                expr(vec![Some(20), Some(21), Some(22), Some(23)]),
            ),
        ],
        Some(expr(vec![Some(30), Some(31), None, Some(33)])),
    );
    let mut ctx = EvalContext::default();

    assert_eq!(
        sig.vecEvalInt(&mut ctx, 4).unwrap(),
        vec![Some(20), Some(11), None, None]
    );
    assert!(sig.vectorized());

    let mut no_else = builtinCaseWhenIntSig::new(
        vec![(expr(vec![None, Some(0)]), expr(vec![Some(1), Some(2)]))],
        None,
    );
    assert_eq!(no_else.vecEvalInt(&mut ctx, 2).unwrap(), vec![None, None]);
}

/// IFNULL/IF 在变长类型上保留值与 NULL。
#[test]
/// IFNULL/IF 在变长类型（String/JSON）上保留值与 NULL。
fn ifnull_and_if_preserve_values_and_nulls_for_variable_width_types() {
    let mut ifnull = builtinIfNullStringSig::new(
        expr(vec![Some("left".to_owned()), None, None]),
        expr(vec![
            Some("right-0".to_owned()),
            Some("right-1".to_owned()),
            None,
        ]),
    );
    let mut ctx = EvalContext::default();
    assert_eq!(
        ifnull.vecEvalString(&mut ctx, 3).unwrap(),
        vec![Some("left".to_owned()), Some("right-1".to_owned()), None]
    );

    let mut if_sig = builtinIfJSONSig::new(
        expr(vec![Some(1), Some(0), None]),
        expr(vec![Some(serde_json::json!({"branch": "true"})); 3]),
        expr(vec![
            Some(serde_json::json!({"branch": "false"})),
            None,
            Some(serde_json::json!(3)),
        ]),
    );
    assert_eq!(
        if_sig.vecEvalJSON(&mut ctx, 3).unwrap(),
        vec![
            Some(serde_json::json!({"branch": "true"})),
            None,
            Some(serde_json::json!(3)),
        ]
    );
}

/// Real/Decimal/Time/Duration 共用同一套 Go 控制矩阵。
#[test]
/// Real/Decimal/Time/Duration 走同一套 Go 控制流矩阵。
fn real_decimal_time_and_signed_duration_follow_the_same_go_control_matrix() {
    let mut ctx = EvalContext::default();

    let mut real_case = builtinCaseWhenRealSig::new(
        vec![(
            expr(vec![Some(1), Some(0)]),
            expr(vec![Some(1.25), Some(2.5)]),
        )],
        Some(expr(vec![Some(3.75), None])),
    );
    assert_eq!(
        real_case.vecEvalReal(&mut ctx, 2).unwrap(),
        vec![Some(1.25), None]
    );

    let mut decimal_ifnull = builtinIfNullDecimalSig::new(
        expr(vec![Some(Decimal::new(125, 2)), None]),
        expr(vec![Some(Decimal::new(250, 2)), Some(Decimal::new(375, 2))]),
    );
    assert_eq!(
        decimal_ifnull.vecEvalDecimal(&mut ctx, 2).unwrap(),
        vec![Some(Decimal::new(125, 2)), Some(Decimal::new(375, 2))]
    );

    let epoch = Time::default();
    let later = epoch + Duration::seconds(1);
    let mut time_if = builtinIfTimeSig::new(
        expr(vec![Some(1), None]),
        expr(vec![Some(epoch), Some(epoch)]),
        expr(vec![Some(later), Some(later)]),
    );
    assert_eq!(
        time_if.vecEvalTime(&mut ctx, 2).unwrap(),
        vec![Some(epoch), Some(later)]
    );

    let mut duration_ifnull = builtinIfNullDurationSig::new(
        expr(vec![Some(Duration::seconds(-2)), None]),
        expr(vec![
            Some(Duration::seconds(3)),
            Some(Duration::seconds(-4)),
        ]),
    );
    assert_eq!(
        duration_ifnull.vecEvalDuration(&mut ctx, 2).unwrap(),
        vec![Some(Duration::seconds(-2)), Some(Duration::seconds(-4))]
    );
}

/// 投机分支错误/警告应回滚后再标量回退。
#[test]
/// 投机分支错误与警告在进入标量回退前必须回滚。
fn speculative_branch_error_and_warning_roll_back_before_scalar_fallback() {
    let mut ctx = EvalContext::default();
    // 既有警告在回滚后应保留。
    ctx.append_warning("existing warning");
    let hazardous = Scripted::vector_hazard(vec![Some(7), Some(8)], vec![0, 1]);
    let mut ifnull = builtinIfNullIntSig::new(expr(vec![Some(1), Some(2)]), Box::new(hazardous));

    assert_eq!(
        ifnull.vecEvalInt(&mut ctx, 2).unwrap(),
        vec![Some(1), Some(2)]
    );
    assert_eq!(ctx.warnings(), &["existing warning"]);

    let warning_only = Scripted::vector_warning(vec![Some(7), Some(8)]);
    let mut warning_ifnull =
        builtinIfNullIntSig::new(expr(vec![Some(1), Some(2)]), Box::new(warning_only));
    assert_eq!(
        warning_ifnull.vecEvalInt(&mut ctx, 2).unwrap(),
        vec![Some(1), Some(2)]
    );
    assert_eq!(ctx.warnings(), &["existing warning"]);

    let hazardous_then = Scripted::vector_hazard(vec![Some(9), Some(9)], vec![0, 1]);
    let mut case_when = builtinCaseWhenIntSig::new(
        vec![(expr(vec![Some(0), None]), Box::new(hazardous_then))],
        Some(expr(vec![Some(4), Some(5)])),
    );
    assert_eq!(
        case_when.vecEvalInt(&mut ctx, 2).unwrap(),
        vec![Some(4), Some(5)]
    );
    assert_eq!(ctx.warnings(), &["existing warning"]);
}

/// 标量回退后仍应报告被选中分支的错误。
#[test]
/// 标量回退后，被选中的分支仍应报告其行级错误。
fn fallback_still_reports_errors_from_a_branch_selected_by_scalar_evaluation() {
    let hazardous = Scripted::vector_hazard(vec![Some(9)], vec![0]);
    let mut if_sig = builtinIfIntSig::new(
        expr(vec![Some(1)]),
        Box::new(hazardous),
        expr(vec![Some(3)]),
    );
    let error = if_sig
        .vecEvalInt(&mut EvalContext::default(), 1)
        .unwrap_err();
    assert_eq!(error.to_string(), "scalar error at row 0");
}

/// 首参（条件/IFNULL 左）向量错误不可被回退隐藏。
#[test]
/// 强制首参（IFNULL 左值 / IF 条件）的向量错误不可被回退掩盖。
fn mandatory_first_argument_vector_errors_are_not_hidden_by_fallback() {
    let mut ifnull = builtinIfNullIntSig::new(
        Box::new(Scripted::vector_error(vec![Some(1)])),
        expr(vec![Some(2)]),
    );
    assert_eq!(
        ifnull
            .vecEvalInt(&mut EvalContext::default(), 1)
            .unwrap_err()
            .to_string(),
        "vector error"
    );

    let mut if_sig = builtinIfIntSig::new(
        Box::new(Scripted::vector_error(vec![Some(1)])),
        expr(vec![Some(2)]),
        expr(vec![Some(3)]),
    );
    assert_eq!(
        if_sig
            .vecEvalInt(&mut EvalContext::default(), 1)
            .unwrap_err()
            .to_string(),
        "vector error"
    );
}

/// 全部 21 个 Go 签名应可构造且声明 vectorized。
#[test]
/// 全部 21 个 Go 签名均可构造且声明向量化。
fn all_twenty_one_go_signatures_are_vectorized_and_constructible() {
    // 批量校验 CASE / IFNULL / IF 各类型签名。
    macro_rules! check_case {
        ($sig:ident) => {{
            let value = $sig::new(Vec::new(), None);
            assert!(value.vectorized());
        }};
    }
    macro_rules! check_ifnull {
        ($sig:ident, $value:expr) => {{
            let value = $sig::new(expr(vec![Some($value.clone())]), expr(vec![Some($value)]));
            assert!(value.vectorized());
        }};
    }
    macro_rules! check_if {
        ($sig:ident, $value:expr) => {{
            let value = $sig::new(
                expr(vec![Some(1)]),
                expr(vec![Some($value.clone())]),
                expr(vec![Some($value)]),
            );
            assert!(value.vectorized());
        }};
    }

    check_case!(builtinCaseWhenIntSig);
    check_case!(builtinCaseWhenRealSig);
    check_case!(builtinCaseWhenDecimalSig);
    check_case!(builtinCaseWhenStringSig);
    check_case!(builtinCaseWhenTimeSig);
    check_case!(builtinCaseWhenDurationSig);
    check_case!(builtinCaseWhenJSONSig);

    check_ifnull!(builtinIfNullIntSig, 1_i64);
    check_ifnull!(builtinIfNullRealSig, 1.5_f64);
    check_ifnull!(builtinIfNullDecimalSig, Decimal::ONE);
    check_ifnull!(builtinIfNullStringSig, "x".to_owned());
    check_ifnull!(builtinIfNullTimeSig, Time::default());
    check_ifnull!(builtinIfNullDurationSig, Duration::zero());
    check_ifnull!(builtinIfNullJSONSig, serde_json::json!(1));

    check_if!(builtinIfIntSig, 1_i64);
    check_if!(builtinIfRealSig, 1.5_f64);
    check_if!(builtinIfDecimalSig, Decimal::ONE);
    check_if!(builtinIfStringSig, "x".to_owned());
    check_if!(builtinIfTimeSig, Time::default());
    check_if!(builtinIfDurationSig, Duration::zero());
    check_if!(builtinIfJSONSig, serde_json::json!(1));
}
