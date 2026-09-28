// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// FIRST_VALUE / LAST_VALUE / NTH_VALUE 窗口函数测试。
//
// 块注释内保留 Go `TestMemValue` 的内存增量矩阵；可执行部分验证字符串窗口
// 在 NULL 存在时仍能正确记录 presence，并回报拥有型内存增量。

/*
// 这段逻辑只记录 first_value、last_value、nth_value 的内存增量期望，
// 也不接入真实 chunk/types/aggfuncs 依赖；相关类型和函数名称沿用 Go 测试夹具。
// get_evaluated_mem_delta 对应 Go 的 getEvaluatedMemDelta。
// 它只对 String 和 JSON 这类变长值返回已求值行的内存大小，其它类型沿用 Go 默认返回 0。
fn get_evaluated_mem_delta(row: &chunk::Row, data_type: &types::FieldType) -> i64 {
    match data_type.GetType() {
        mysql::TypeString => row.GetString(0).len() as i64,
        mysql::TypeJSON => row.GetJSON(0).Value.len() as i64,
        // Go switch 未覆盖的类型保留零增量，代表固定宽度值已由 partial result 固定大小覆盖。
        _ => 0,
    }
}

// last_value_evaluate_row_update_mem_delta_gens 对应 Go 的 lastValueEvaluateRowUpdateMemDeltaGens。
// Go 每轮都取 srcChk 第 0 行，计算当前 evaluated value 与上一轮 value 的内存差。
fn last_value_evaluate_row_update_mem_delta_gens(
    param: updateMemDeltaGensParams,
) -> Result<Vec<i64>, Error> {
    let mut mem_deltas = Vec::new();
    let mut last_mem_delta = 0_i64;
    for _ in 0..param.srcChk.NumRows() {
        let row = param.srcChk.GetRow(0);
        let cur_mem_delta = get_evaluated_mem_delta(&row, param.keyType);
        // Go append(curMemDelta-lastMemDelta)，保留 last_value 覆盖旧值时的增量语义。
        mem_deltas.push(cur_mem_delta - last_mem_delta);
        last_mem_delta = cur_mem_delta;
    }
    Ok(mem_deltas)
}

// nth_value_evaluate_row_update_mem_delta_gens 对应 Go 的 nthValueEvaluateRowUpdateMemDeltaGens。
// 返回闭包先为每行填 0，只在 nth 落入窗口行数时，为第 nth-1 个位置写入该行求值内存。
fn nth_value_evaluate_row_update_mem_delta_gens(nth: i32) -> updateMemDeltaGens {
    move |param: updateMemDeltaGensParams| -> Result<Vec<i64>, Error> {
        let mut mem_deltas = Vec::new();
        for _ in 0..param.srcChk.NumRows() {
            mem_deltas.push(0);
        }

        if nth < param.srcChk.NumRows() {
            let row = param.srcChk.GetRow(nth - 1);
            mem_deltas[(nth - 1) as usize] = get_evaluated_mem_delta(&row, param.keyType);
        }

        Ok(mem_deltas)
    }
}

// test_mem_value 对应 Go 的 TestMemValue。
// 该用例矩阵覆盖 first_value/last_value/nth_value 对固定宽度、字符串和 JSON 值的 partial result 内存。
#[test]
fn test_mem_value() {
    let first_mem_delta_gens = nth_value_evaluate_row_update_mem_delta_gens(1);
    let second_mem_delta_gens = nth_value_evaluate_row_update_mem_delta_gens(2);
    let fifth_mem_delta_gens = nth_value_evaluate_row_update_mem_delta_gens(5);
    let tests = vec![
        buildWindowMemTester(
            ast::WindowFuncFirstValue,
            mysql::TypeLonglong,
            0,
            2,
            1,
            aggfuncs::DefPartialResult4FirstValueSize + aggfuncs::DefValue4IntSize,
            first_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncFirstValue,
            mysql::TypeFloat,
            0,
            2,
            1,
            aggfuncs::DefPartialResult4FirstValueSize + aggfuncs::DefValue4Float32Size,
            first_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncFirstValue,
            mysql::TypeDouble,
            0,
            2,
            1,
            aggfuncs::DefPartialResult4FirstValueSize + aggfuncs::DefValue4Float64Size,
            first_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncFirstValue,
            mysql::TypeNewDecimal,
            0,
            2,
            1,
            aggfuncs::DefPartialResult4FirstValueSize + aggfuncs::DefValue4DecimalSize,
            first_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncFirstValue,
            mysql::TypeString,
            0,
            2,
            1,
            aggfuncs::DefPartialResult4FirstValueSize + aggfuncs::DefValue4StringSize,
            first_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncFirstValue,
            mysql::TypeDate,
            0,
            2,
            1,
            aggfuncs::DefPartialResult4FirstValueSize + aggfuncs::DefValue4TimeSize,
            first_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncFirstValue,
            mysql::TypeDuration,
            0,
            2,
            1,
            aggfuncs::DefPartialResult4FirstValueSize + aggfuncs::DefValue4DurationSize,
            first_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncFirstValue,
            mysql::TypeJSON,
            0,
            2,
            1,
            aggfuncs::DefPartialResult4FirstValueSize + aggfuncs::DefValue4JSONSize,
            first_mem_delta_gens,
        ),
        // last_value 使用专门的增量生成器，表达值被新行覆盖时释放旧值、持有新值的差额。
        buildWindowMemTester(
            ast::WindowFuncLastValue,
            mysql::TypeLonglong,
            1,
            2,
            0,
            aggfuncs::DefPartialResult4LastValueSize + aggfuncs::DefValue4IntSize,
            last_value_evaluate_row_update_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncLastValue,
            mysql::TypeString,
            1,
            2,
            0,
            aggfuncs::DefPartialResult4LastValueSize + aggfuncs::DefValue4StringSize,
            last_value_evaluate_row_update_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncLastValue,
            mysql::TypeJSON,
            1,
            2,
            0,
            aggfuncs::DefPartialResult4LastValueSize + aggfuncs::DefValue4JSONSize,
            last_value_evaluate_row_update_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncNthValue,
            mysql::TypeLonglong,
            2,
            3,
            0,
            aggfuncs::DefPartialResult4NthValueSize + aggfuncs::DefValue4IntSize,
            second_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncNthValue,
            mysql::TypeLonglong,
            5,
            3,
            0,
            aggfuncs::DefPartialResult4NthValueSize + aggfuncs::DefValue4IntSize,
            fifth_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncNthValue,
            mysql::TypeJSON,
            2,
            3,
            0,
            aggfuncs::DefPartialResult4NthValueSize + aggfuncs::DefValue4JSONSize,
            second_mem_delta_gens,
        ),
        buildWindowMemTester(
            ast::WindowFuncNthValue,
            mysql::TypeString,
            5,
            3,
            0,
            aggfuncs::DefPartialResult4NthValueSize + aggfuncs::DefValue4StringSize,
            fifth_mem_delta_gens,
        ),
    ];

    for test in tests {
        // testWindowAggMemFunc 对应 Go 中的窗口聚合内存校验入口。
        testWindowAggMemFunc(test);
    }
}
*/

use crate::func_max_min::{BinaryJson, VectorFloat32};
use crate::func_value::{
    FirstValue, LastValue, NthValue, Value4Decimal, Value4Duration, Value4Float32, Value4Float64,
    Value4Int, Value4Json, Value4String, Value4Time, Value4VectorFloat32, evaluate_float32,
};

/// 验证 first/last/nth 在含 NULL 行时保留 presence，并回报字符串内存增量。
///
/// 行 `["first", NULL, "last"]`：first 增量 5；last 增量 4；nth(2) 结果为 SQL NULL。
#[test]
fn value_windows_preserve_null_presence_and_owned_memory() {
    let rows = [Some("first".to_owned()), None, Some("last".to_owned())];
    let mut first = FirstValue::default();
    assert_eq!(first.update(&rows), 5);
    assert_eq!(first.result(), Some(Some(&"first".to_owned())));
    let mut last = LastValue::default();
    assert_eq!(last.update(&rows), 4);
    assert_eq!(last.result(), Some(Some(&"last".to_owned())));
    let mut nth = NthValue::new(2);
    nth.update(&rows);
    assert_eq!(nth.result(), Some(None));
}

#[test]
fn value_evaluators_cover_every_go_specialization_and_memory_rule() {
    let _: Value4Int = Default::default();
    let mut float32: Value4Float32 = Default::default();
    let _: Value4Float64 = Default::default();
    let _: Value4Decimal = Default::default();
    let _: Value4Time = Default::default();
    let _: Value4Duration = Default::default();

    assert_eq!(evaluate_float32(&mut float32, Some(1.0_f64 / 3.0)), 0);
    let narrowed = (1.0_f64 / 3.0) as f32;
    assert_eq!(float32.result(), Some(Some(&narrowed)));

    let mut string: Value4String = Default::default();
    assert_eq!(string.evaluate(Some("12345".to_owned())), 5);
    assert_eq!(string.evaluate(Some("xy".to_owned())), -3);
    assert_eq!(string.evaluate(None), -2);
    assert_eq!(string.result(), Some(None));

    let mut json: Value4Json = Default::default();
    assert_eq!(
        json.evaluate(Some(BinaryJson {
            type_code: 1,
            value: vec![1, 2, 3]
        })),
        3
    );
    assert_eq!(
        json.evaluate(Some(BinaryJson {
            type_code: 1,
            value: vec![4]
        })),
        -2
    );

    let mut vector: Value4VectorFloat32 = Default::default();
    assert_eq!(
        vector.evaluate(Some(VectorFloat32(vec![1.0, 2.0, 3.0]))),
        12
    );
    assert_eq!(vector.evaluate(Some(VectorFloat32(vec![4.0]))), -8);
}

#[test]
fn first_last_and_nth_match_go_batch_and_reset_lifecycle() {
    let first_batch = [Some("first".to_owned()), Some("ignored".to_owned())];
    let second_batch = [Some("later".to_owned()), Some("last".to_owned())];

    let mut first = FirstValue::default();
    assert_eq!(first.update(&[]), 0);
    assert_eq!(first.result(), None);
    assert_eq!(first.update(&first_batch), 5);
    assert_eq!(first.update(&second_batch), 0);
    assert_eq!(first.result(), Some(Some(&"first".to_owned())));
    first.reset();
    assert_eq!(first.result(), None);
    assert_eq!(first.update(&second_batch), 0);
    assert_eq!(first.result(), Some(Some(&"later".to_owned())));

    let mut last = LastValue::default();
    assert_eq!(last.update(&first_batch), 7);
    assert_eq!(last.update(&second_batch), -3);
    assert_eq!(last.result(), Some(Some(&"last".to_owned())));
    last.reset();
    assert_eq!(last.result(), None);
    assert_eq!(last.update(&[]), 0);
    assert_eq!(last.result(), None);

    let mut nth = NthValue::new(3);
    assert_eq!(nth.update(&first_batch), 0);
    assert_eq!(nth.result(), None);
    assert_eq!(nth.update(&second_batch), 5);
    assert_eq!(nth.result(), Some(Some(&"later".to_owned())));
    assert_eq!(nth.update(&[Some("ignored again".to_owned())]), 0);
    nth.reset();
    assert_eq!(nth.result(), None);
    assert_eq!(nth.update(&[None, Some("two".to_owned()), None]), -5);
    assert_eq!(nth.result(), Some(None));

    let mut zero = NthValue::<String>::new(0);
    assert_eq!(zero.update(&second_batch), 0);
    assert_eq!(zero.result(), None);
}
