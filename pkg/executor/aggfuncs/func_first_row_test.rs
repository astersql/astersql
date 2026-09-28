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

// FIRST_ROW 聚合的单元测试。
//
// 验证「无行」与「首行为 SQL NULL」的区分，以及 merge 时已锁定的 NULL 首行
// 不会被对侧非空值覆盖。大段注释内保留 Go 侧多类型 merge/内存增量测试草稿。

/*
//

// TestMergePartialResult4FirstRow 对应 Go 测试：验证 FIRST_ROW 的 partial result 合并后仍取首行。
#[test]
pub fn test_merge_partial_result_4_first_row() {
    let elems = vec!["e", "d", "c", "b", "a"];
    let enum_c = types::ParseEnumName(&elems, "c", mysql::DefaultCollationName).0;
    let enum_e = types::ParseEnumName(&elems, "e", mysql::DefaultCollationName).0;

    let set_ed = types::ParseSetName(&elems, "e,d", mysql::DefaultCollationName).0;
    let set_e = types::ParseSetName(&elems, "e", mysql::DefaultCollationName).0;

    let tests = vec![
        buildAggTester(ast::AggFuncFirstRow, mysql::TypeLonglong, 0, 5, 0, 2, 0),
        buildAggTester(ast::AggFuncFirstRow, mysql::TypeFloat, 0, 5, 0.0, 2.0, 0.0),
        buildAggTester(ast::AggFuncFirstRow, mysql::TypeDouble, 0, 5, 0.0, 2.0, 0.0),
        buildAggTester(
            ast::AggFuncFirstRow,
            mysql::TypeNewDecimal,
            0,
            5,
            types::NewDecFromInt(0),
            types::NewDecFromInt(2),
            types::NewDecFromInt(0),
        ),
        buildAggTester(ast::AggFuncFirstRow, mysql::TypeString, 0, 5, "0", "2", "0"),
        buildAggTester(
            ast::AggFuncFirstRow,
            mysql::TypeDate,
            0,
            5,
            types::TimeFromDays(365),
            types::TimeFromDays(367),
            types::TimeFromDays(365),
        ),
        buildAggTester(
            ast::AggFuncFirstRow,
            mysql::TypeDuration,
            0,
            5,
            types::Duration { Duration: time::Duration::from_nanos(0) },
            types::Duration { Duration: time::Duration::from_nanos(2) },
            types::Duration { Duration: time::Duration::from_nanos(0) },
        ),
        buildAggTester(
            ast::AggFuncFirstRow,
            mysql::TypeJSON,
            0,
            5,
            types::CreateBinaryJSON(0_i64),
            types::CreateBinaryJSON(2_i64),
            types::CreateBinaryJSON(0_i64),
        ),
        buildAggTester(ast::AggFuncFirstRow, mysql::TypeEnum, 0, 5, enum_e, enum_c, enum_e),
        buildAggTester(ast::AggFuncFirstRow, mysql::TypeSet, 0, 5, set_e, set_ed, set_e),
    ];

    for test in tests {
        // Go 版把 testing.T 传入公共 helper；这里保留 helper 调用形状，不实现断言框架。
        testMergePartialResult(test);
    }
}

// TestMemFirstRow 对应 Go 测试：覆盖 FIRST_ROW 不同类型 partial result 的内存增量计算。
#[test]
pub fn test_mem_first_row() {
    let tests = vec![
        buildAggMemTester(
            ast::AggFuncFirstRow,
            mysql::TypeLonglong,
            0,
            5,
            aggfuncs::DefPartialResult4FirstRowIntSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncFirstRow,
            mysql::TypeFloat,
            0,
            5,
            aggfuncs::DefPartialResult4FirstRowFloat32Size,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncFirstRow,
            mysql::TypeDouble,
            0,
            5,
            aggfuncs::DefPartialResult4FirstRowFloat64Size,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncFirstRow,
            mysql::TypeNewDecimal,
            0,
            5,
            aggfuncs::DefPartialResult4FirstRowDecimalSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncFirstRow,
            mysql::TypeString,
            0,
            5,
            aggfuncs::DefPartialResult4FirstRowStringSize,
            first_row_update_mem_delta_gens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncFirstRow,
            mysql::TypeDate,
            0,
            5,
            aggfuncs::DefPartialResult4FirstRowTimeSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncFirstRow,
            mysql::TypeDuration,
            0,
            5,
            aggfuncs::DefPartialResult4FirstRowDurationSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncFirstRow,
            mysql::TypeJSON,
            0,
            5,
            aggfuncs::DefPartialResult4FirstRowJSONSize,
            first_row_update_mem_delta_gens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncFirstRow,
            mysql::TypeEnum,
            0,
            5,
            aggfuncs::DefPartialResult4FirstRowEnumSize,
            first_row_update_mem_delta_gens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncFirstRow,
            mysql::TypeSet,
            0,
            5,
            aggfuncs::DefPartialResult4FirstRowSetSize,
            first_row_update_mem_delta_gens,
            false,
        ),
    ];

    for test in tests {
        testAggMemFunc(test);
    }
}

// firstRowUpdateMemDeltaGens 对应 Go 同名 helper：只有首行会写入 FIRST_ROW partial result。
pub fn first_row_update_mem_delta_gens(param: updateMemDeltaGensParams) -> Result<Vec<i64>, errors::Error> {
    let mut mem_deltas = Vec::new();
    for i in 0..param.srcChk.NumRows() {
        let row = param.srcChk.GetRow(i);
        if i > 0 {
            // FIRST_ROW 在首行之后不再替换保存值，因此后续行的内存增量固定为 0。
            mem_deltas.push(0);
            continue;
        }

        match param.keyType.GetType() {
            mysql::TypeString => {
                let val = row.GetString(0);
                mem_deltas.push(val.len() as i64);
            }
            mysql::TypeJSON => {
                let json_val = row.GetJSON(0);
                mem_deltas.push(json_val.Value.to_string().len() as i64);
            }
            mysql::TypeEnum => {
                let enum_val = row.GetEnum(0);
                mem_deltas.push(enum_val.Name.len() as i64);
            }
            mysql::TypeSet => {
                let type_set = row.GetSet(0);
                mem_deltas.push(type_set.Name.len() as i64);
            }
            _ => {
                // Go helper 对其它类型不追加额外 delta；这里保留该“无分支动作”的测试语义。
            }
        }
    }

    Ok(mem_deltas)
}
*/

use crate::func_first_row::{
    FirstRow, FirstRow4Decimal, FirstRow4Duration, FirstRow4Enum, FirstRow4Float32,
    FirstRow4Float64, FirstRow4Int, FirstRow4Json, FirstRow4Set, FirstRow4String, FirstRow4Time,
    FirstRow4VectorFloat32,
};

/// 校验无行/NULL 首行语义，以及已锁定 NULL 后 merge 不被覆盖、reset 后再 merge 可采纳对侧。
#[test]
fn first_row_distinguishes_no_row_from_a_null_first_row() {
    // 首个输入为 NULL：标记 got_first_row，且 is_null 为真。
    let mut empty = FirstRow::<i64>::default();
    assert!(!empty.got_first_row());
    empty.update([None, Some(2)]);
    assert!(empty.got_first_row());
    assert!(empty.is_null());
    assert_eq!(empty.value(), None);

    // 已锁定 NULL 时 merge 对侧非空值应被忽略；reset 后才能采纳。
    let mut source = FirstRow::default();
    source.update([Some(7)]);
    empty.merge(&source);
    assert!(empty.is_null());
    empty.reset();
    empty.merge(&source);
    assert_eq!(empty.value(), Some(&7));
}

/// 对齐 Go 的首行锁定和 partial merge 顺序：空批次不锁定，首个非空值不会被后续值覆盖。
#[test]
fn first_row_keeps_the_first_value_and_merges_only_into_empty_state() {
    let mut destination = FirstRow::<String>::default();
    destination.update([]);
    assert!(!destination.got_first_row());

    destination.update([Some("first".to_owned()), Some("second".to_owned())]);
    destination.update([Some("third".to_owned())]);
    assert_eq!(destination.value().map(String::as_str), Some("first"));

    let mut source = FirstRow::default();
    source.update([Some("source".to_owned())]);
    destination.merge(&source);
    assert_eq!(destination.value().map(String::as_str), Some("first"));

    let result = destination.into_result();
    assert_eq!(result, Some(Some("first".to_owned())));
}

/// Go 为每种执行类型提供独立实现；Rust 用同一泛型状态机承载这些类型别名。
/// 此测试固定全部公开别名均保持可分配、可重置的契约。
#[test]
fn all_go_first_row_specializations_have_rust_aliases() {
    fn assert_first_row_contract<T: Default>() {
        let mut row = FirstRow::<T>::default();
        assert!(!row.got_first_row());
        row.reset();
        assert!(!row.got_first_row());
    }

    assert_first_row_contract::<i64>();
    assert_first_row_contract::<f32>();
    assert_first_row_contract::<f64>();
    assert_first_row_contract::<crate::func_sum::Decimal>();
    assert_first_row_contract::<String>();
    assert_first_row_contract::<crate::func_max_min::TimeValue>();
    assert_first_row_contract::<crate::func_max_min::DurationValue>();
    assert_first_row_contract::<crate::func_max_min::BinaryJson>();
    assert_first_row_contract::<crate::func_max_min::VectorFloat32>();
    assert_first_row_contract::<crate::func_max_min::NamedValue>();

    let _: FirstRow4Int = FirstRow::default();
    let _: FirstRow4Float32 = FirstRow::default();
    let _: FirstRow4Float64 = FirstRow::default();
    let _: FirstRow4Decimal = FirstRow::default();
    let _: FirstRow4String = FirstRow::default();
    let _: FirstRow4Time = FirstRow::default();
    let _: FirstRow4Duration = FirstRow::default();
    let _: FirstRow4Json = FirstRow::default();
    let _: FirstRow4VectorFloat32 = FirstRow::default();
    let _: FirstRow4Enum = FirstRow::default();
    let _: FirstRow4Set = FirstRow::default();
}
