// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// VecGroupChecker 单元测试。
//
// 覆盖：变长类型 Datum 所有权（DATARACE）、跨 Chunk 组计数、collation
// 对字符串分组的影响，以及 `Reset` 后耗尽状态（Issue 53867）。

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{NewVecGroupChecker, chunk, expression, mysql, types};

/// 按 MySQL 类型构造 FieldType；DECIMAL 额外设置 flen/decimal。
fn field_type(mysql_type: u8) -> Box<types::FieldType> {
    let mut field_type = types::NewFieldType(mysql_type);
    if mysql_type == mysql::TypeNewDecimal {
        field_type.SetFlen(10);
        field_type.SetDecimal(0);
    }
    field_type
}

fn data_race_allocate(
    eval_type: types::EvalType,
    capacity: usize,
) -> Result<Box<chunk::Column>, expression::Error> {
    expression::GetColumn(eval_type, capacity)
}

fn data_race_release(_column: Box<chunk::Column>) {}

#[test]
/// 验证字符串/DECIMAL/JSON 的 Datum 在 Reset 输入 Chunk 后仍保持独立拷贝。
fn TestVecGroupCheckerDATARACE() {
    crate::main_test::setup();
    let cases = [mysql::TypeVarString, mysql::TypeNewDecimal, mysql::TypeJSON];
    for mysql_type in cases {
        let tp = field_type(mysql_type);
        let item = expression::Column::new((*tp).clone(), 0, 0, 0);
        let ctx = exprstatic_crate::NewEvalContext(Vec::new());
        let mut checker = NewVecGroupChecker(Some(&ctx), true, vec![Box::new(item)]);
        checker.allocateBuffer = Some(data_race_allocate);
        checker.releaseBuffer = Some(data_race_release);
        let mut input = chunk::New(vec![tp], 1, 1);

        // 向单行 Chunk 写入该类型的样例值。
        match mysql_type {
            mysql::TypeVarString => input.AppendString(0, "abc"),
            mysql::TypeNewDecimal => {
                let mut decimal = types::MyDecimal::default();
                decimal.FromString(b"123").unwrap();
                input.AppendMyDecimal(0, &decimal);
            }
            mysql::TypeJSON => input.AppendJSON(
                0,
                types::ParseBinaryJSONFromString(r#"{"123":123}"#).unwrap(),
            ),
            _ => unreachable!(),
        }

        checker.SplitIntoGroups(&input).unwrap();
        // 改写输入后，checker 内缓存的 first/last Datum 不应跟着变。
        match mysql_type {
            mysql::TypeVarString => {
                assert_eq!(checker.firstRowDatums[0].GetString(), "abc");
                assert_eq!(checker.lastRowDatums[0].GetString(), "abc");
                input.Reset();
                input.AppendString(0, "edf");
                assert_eq!(checker.firstRowDatums[0].GetString(), "abc");
                assert_eq!(checker.lastRowDatums[0].GetString(), "abc");
            }
            mysql::TypeNewDecimal => {
                assert_eq!(checker.firstRowDatums[0].GetMysqlDecimal().String(), "123");
                assert_eq!(checker.lastRowDatums[0].GetMysqlDecimal().String(), "123");
                input.Reset();
                let mut decimal = types::MyDecimal::default();
                decimal.FromString(b"456").unwrap();
                input.AppendMyDecimal(0, &decimal);
                assert_eq!(checker.firstRowDatums[0].GetMysqlDecimal().String(), "123");
                assert_eq!(checker.lastRowDatums[0].GetMysqlDecimal().String(), "123");
            }
            mysql::TypeJSON => {
                assert_eq!(
                    checker.firstRowDatums[0].GetMysqlJSON().String(),
                    r#"{"123": 123}"#
                );
                assert_eq!(
                    checker.lastRowDatums[0].GetMysqlJSON().String(),
                    r#"{"123": 123}"#
                );
                input.Reset();
                input.AppendJSON(
                    0,
                    types::ParseBinaryJSONFromString(r#"{"456":456}"#).unwrap(),
                );
                assert_eq!(
                    checker.firstRowDatums[0].GetMysqlJSON().String(),
                    r#"{"123": 123}"#
                );
                assert_eq!(
                    checker.lastRowDatums[0].GetMysqlJSON().String(),
                    r#"{"123": 123}"#
                );
            }
            _ => unreachable!(),
        }
    }
}

/// 生成多 Chunk 测试数据：每 `same_num` 行一组，中间一组写入 NULL。
fn genTestChunk4VecGroupChecker(
    chunk_rows: &[usize],
    same_num: usize,
) -> (Vec<expression::ExprBox>, Vec<Box<chunk::Chunk>>) {
    let tp = field_type(mysql::TypeLonglong);
    let mut inputs = Vec::with_capacity(chunk_rows.len());
    let total_rows: usize = chunk_rows.iter().sum();
    let null_group = (total_rows.div_ceil(same_num) / 2).max(1);
    let mut global_row = 0;

    for &row_count in chunk_rows {
        let mut input = chunk::New(vec![tp.clone()], row_count, row_count);
        for _ in 0..row_count {
            let group = global_row / same_num;
            if group == null_group {
                input.AppendNull(0);
            } else {
                input.AppendInt64(0, group as i64 + 100);
            }
            global_row += 1;
        }
        inputs.push(input);
    }

    (
        vec![Box::new(expression::Column::new((*tp).clone(), 0, 0, 0))],
        inputs,
    )
}

#[test]
/// 验证跨多个 Chunk 累计组数，以及首组是否与上一批连续的标志。
fn TestVecGroupChecker4GroupCount() {
    crate::main_test::setup();
    let cases = [
        (vec![1024, 1], 1025, vec![false, false], 1),
        (vec![1024, 1], 1, vec![false, true], 1025),
        (vec![1, 1], 1, vec![false, true], 2),
        (vec![1, 1], 2, vec![false, false], 1),
        (vec![2, 2], 2, vec![false, false], 2),
        (vec![2, 2], 1, vec![false, true], 4),
    ];

    for (chunk_rows, expected_groups, expected_flags, same_num) in cases {
        let (expressions, inputs) = genTestChunk4VecGroupChecker(&chunk_rows, same_num);
        let ctx = exprstatic_crate::NewEvalContext(Vec::new());
        let mut checker = NewVecGroupChecker(Some(&ctx), true, expressions);
        let mut group_count = 0;
        for (index, input) in inputs.iter().enumerate() {
            let same_as_previous = checker.SplitIntoGroups(input).unwrap();
            assert_eq!(same_as_previous, expected_flags[index]);
            // 与上一批连续的首组不重复计数。
            group_count += checker.GroupCount() - usize::from(same_as_previous);
        }
        assert_eq!(group_count, expected_groups);
    }
}

#[test]
/// 验证不同 collation 下字符串相邻行是否合并为同一组，以及 pad 空格处理。
fn TestVecGroupChecker() {
    crate::main_test::setup();
    let tp = field_type(mysql::TypeVarchar);
    let ctx = exprstatic_crate::NewEvalContext(Vec::new());
    let mut checker = NewVecGroupChecker(
        Some(&ctx),
        true,
        vec![Box::new(expression::Column::new((*tp).clone(), 0, 0, 0))],
    );
    let mut input = chunk::New(vec![tp], 6, 6);
    for value in ["aaa", "AAA", "😜", "😃", "À", "A"] {
        input.AppendString(0, value);
    }

    // binary collation：大小写与外观不同的字符各自成组。
    checker.GroupByItems[0]
        .GetTypeMut()
        .SetCollate("bin".to_owned());
    checker.SplitIntoGroups(&input).unwrap();
    for index in 0..6 {
        assert_eq!(checker.GetNextGroup(), (index, index + 1));
    }
    assert!(checker.IsExhausted());

    // ci collation：忽略大小写后两两合并。
    for collation in ["utf8_general_ci", "utf8_unicode_ci"] {
        checker.GroupByItems[0]
            .GetTypeMut()
            .SetCollate(collation.to_owned());
        checker.SplitIntoGroups(&input).unwrap();
        for index in 0..3 {
            assert_eq!(checker.GetNextGroup(), (index * 2, index * 2 + 2));
        }
        assert!(checker.IsExhausted());
    }

    // utf8_bin + 定长：尾部空格 pad 后视为同值，三行并为一组。
    checker.GroupByItems[0]
        .GetTypeMut()
        .SetCollate("utf8_bin".to_owned());
    checker.GroupByItems[0].GetTypeMut().SetFlen(6);
    input.Reset();
    for value in ["a", "a  ", "a    "] {
        input.AppendString(0, value);
    }
    checker.SplitIntoGroups(&input).unwrap();
    assert_eq!(checker.GetNextGroup(), (0, 3));
    assert!(checker.IsExhausted());
}

#[test]
/// Issue 53867：`Reset` 须清零 groupCount，使 `IsExhausted` 在未推进 nextGroupID 时也为真。
fn TestIssue53867() {
    crate::main_test::setup();
    let mut checker = NewVecGroupChecker(None, true, Vec::new());
    checker.groupOffset = vec![0; 20];
    checker.nextGroupID = 10;
    checker.groupCount = 15;
    assert!(!checker.IsExhausted());
    checker.Reset();
    assert!(checker.IsExhausted());
}

#[test]
/// GROUP BY 项必须接受任意 Expression，而不是仅接受输入 Chunk 的列引用。
fn constant_expression_is_evaluated_through_the_expression_contract() {
    crate::main_test::setup();
    let tp = field_type(mysql::TypeLonglong);
    let mut input = chunk::New(vec![tp], 3, 3);
    for value in [1, 2, 3] {
        input.AppendInt64(0, value);
    }

    let ctx = exprstatic_crate::NewEvalContext(Vec::new());
    let mut checker = NewVecGroupChecker(
        Some(&ctx),
        false,
        vec![Box::new(expression::NewInt64Const(7))],
    );

    assert!(!checker.SplitIntoGroups(&input).unwrap());
    assert_eq!(checker.GroupCount(), 1);
    assert_eq!(checker.GetNextGroup(), (0, 3));
}

static ALLOCATE_CALLS: AtomicUsize = AtomicUsize::new(0);
static RELEASE_CALLS: AtomicUsize = AtomicUsize::new(0);

fn tracking_allocate(
    eval_type: types::EvalType,
    capacity: usize,
) -> Result<Box<chunk::Column>, expression::Error> {
    ALLOCATE_CALLS.fetch_add(1, Ordering::SeqCst);
    expression::GetColumn(eval_type, capacity)
}

fn tracking_release(column: Box<chunk::Column>) {
    RELEASE_CALLS.fetch_add(1, Ordering::SeqCst);
    expression::PutColumn(column);
}

fn failing_allocate(
    _eval_type: types::EvalType,
    _capacity: usize,
) -> Result<Box<chunk::Column>, expression::Error> {
    Err(expression::errors::New("injected allocateBuffer failure"))
}

#[test]
/// 临时列必须通过可注入分配器借出，并在比较完成后恰好归还一次。
fn evaluated_columns_are_allocated_and_released() {
    crate::main_test::setup();
    ALLOCATE_CALLS.store(0, Ordering::SeqCst);
    RELEASE_CALLS.store(0, Ordering::SeqCst);

    let tp = field_type(mysql::TypeLonglong);
    let item_type = (*tp).clone();
    let mut input = chunk::New(vec![tp], 2, 2);
    input.AppendInt64(0, 1);
    input.AppendInt64(0, 2);
    let ctx = exprstatic_crate::NewEvalContext(Vec::new());
    for vec_enabled in [false, true] {
        let item = expression::Column::new(item_type.clone(), 0, 0, 0);
        let mut checker = NewVecGroupChecker(Some(&ctx), vec_enabled, vec![Box::new(item)]);
        checker.allocateBuffer = Some(tracking_allocate);
        checker.releaseBuffer = Some(tracking_release);

        checker.SplitIntoGroups(&input).unwrap();
        assert_eq!(checker.GroupCount(), 2);
    }

    assert_eq!(ALLOCATE_CALLS.load(Ordering::SeqCst), 2);
    assert_eq!(RELEASE_CALLS.load(Ordering::SeqCst), 2);
}

#[test]
/// 分配临时列失败时必须原样终止切分，不能回退为直接读取输入列。
fn allocation_errors_are_propagated() {
    crate::main_test::setup();
    let tp = field_type(mysql::TypeLonglong);
    let item = expression::Column::new((*tp).clone(), 0, 0, 0);
    let mut input = chunk::New(vec![tp], 2, 2);
    input.AppendInt64(0, 1);
    input.AppendInt64(0, 2);
    let ctx = exprstatic_crate::NewEvalContext(Vec::new());
    let mut checker = NewVecGroupChecker(Some(&ctx), true, vec![Box::new(item)]);
    checker.allocateBuffer = Some(failing_allocate);

    let error = checker.SplitIntoGroups(&input).unwrap_err();
    assert_eq!(error.to_string(), "injected allocateBuffer failure");
}
