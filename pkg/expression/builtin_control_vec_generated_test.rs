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

// 向量化控制流内置函数（CASE/IF/IFNULL）生成签名的单元测试。
//
// 用固定列数据驱动 `builtinCaseWhen*` / `builtinIf*` / `builtinIfNull*`，
// 校验首个匹配分支、ELSE、NULL 行传播，以及列长度不一致时的错误。

use crate::builtin_control_vec_generated_kernel::*;

/// 测试用列表达式：按行下标返回预置的 Option 值。
struct Values<T>(Vec<Option<T>>);

impl<T> VectorExpression<T> for Values<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// 按行求值；越界返回错误。
    fn eval_row(&self, _ctx: &mut EvalContext, row: usize) -> Result<Option<T>, EvalError> {
        self.0
            .get(row)
            .cloned()
            .ok_or_else(|| EvalError::new("row out of range"))
    }

    /// 整列求值；行数与预置长度必须一致。
    fn vec_eval(&self, _ctx: &mut EvalContext, rows: usize) -> Result<Vec<Option<T>>, EvalError> {
        if self.0.len() != rows {
            return Err(EvalError::new("length mismatch"));
        }
        Ok(self.0.clone())
    }
}

/// 把预置列包装成向量化表达式 trait 对象。
fn expr<T>(values: Vec<Option<T>>) -> Box<dyn VectorExpression<T>>
where
    T: Clone + Send + Sync + 'static,
{
    Box::new(Values(values))
}

#[test]
/// CASE WHEN：从左到右取首个真条件结果；无命中则走 ELSE；条件/结果为 NULL 时保留 NULL。
fn generated_case_when_preserves_first_match_else_and_null_rows() {
    // 两对 (条件, 结果) + ELSE：验证首匹配、ELSE、NULL 条件行与结果 NULL。
    let mut signature = builtinCaseWhenIntSig::new(
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
    assert_eq!(
        signature
            .vecEvalInt(&mut EvalContext::default(), 4)
            .unwrap(),
        vec![Some(20), Some(11), None, None]
    );
    assert!(signature.vectorized());
}

#[test]
/// IFNULL 优先左值、IF 按条件选分支，并覆盖列长度不匹配错误。
fn generated_if_and_ifnull_cover_variable_width_and_length_errors() {
    let mut ifnull = builtinIfNullStringSig::new(
        expr(vec![Some("left".to_owned()), None, None]),
        expr(vec![
            Some("right".to_owned()),
            Some("fallback".to_owned()),
            None,
        ]),
    );
    assert_eq!(
        ifnull
            .vecEvalString(&mut EvalContext::default(), 3)
            .unwrap(),
        vec![Some("left".into()), Some("fallback".into()), None]
    );

    let mut if_sig = builtinIfIntSig::new(
        expr(vec![Some(1), Some(0), None]),
        expr(vec![Some(10), Some(11), Some(12)]),
        expr(vec![Some(20), None, Some(22)]),
    );
    assert_eq!(
        if_sig.vecEvalInt(&mut EvalContext::default(), 3).unwrap(),
        vec![Some(10), None, Some(22)]
    );

    // 请求 2 行但列只有 1 行，应报长度错误。
    let mut bad = builtinIfNullIntSig::new(expr(vec![Some(1)]), expr(vec![Some(2)]));
    assert!(bad.vecEvalInt(&mut EvalContext::default(), 2).is_err());
}
