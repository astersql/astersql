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
// 测试验证字符串窗口
// 在 NULL 存在时仍能正确记录 presence，并回报拥有型内存增量。


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
