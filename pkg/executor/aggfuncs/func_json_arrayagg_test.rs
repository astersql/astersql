// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// JSON_ARRAYAGG 聚合的单元测试。
//
// 可执行用例验证元素顺序保持、merge 追加顺序以及 reset 后结果为空。
// 大段注释保留 Go 侧多类型 merge / 内存增量测试草稿。

/*

// TestMergePartialResult4JsonArrayagg 对应 Go 测试：逐类型构造两个 JSON array partial result 并合并。
#[test]
pub fn test_merge_partial_result_4_json_arrayagg() {
    let type_list = vec![
        mysql::TypeLonglong,
        mysql::TypeDouble,
        mysql::TypeFloat,
        mysql::TypeString,
        mysql::TypeJSON,
        mysql::TypeDate,
        mysql::TypeDuration,
    ];

    let mut tests = Vec::with_capacity(type_list.len());
    let num_rows = 5;
    for arg_type in type_list {
        let mut entries1 = Vec::new();
        let mut entries2 = Vec::new();
        let mut entries3 = Vec::new();

        let arg_field_type = types::NewFieldType(arg_type);
        let gen_func = getDataGenFunc(&arg_field_type);

        for m in 0..num_rows {
            let arg = gen_func(m);
            entries1.push(getJSONValue(arg, &arg_field_type));
        }
        // Go 这里追加 nil 以适配 genSrcChk 的 Chunk 格式；保留该测试夹具形状。
        entries1.push(Nil);

        for m in 2..num_rows {
            let arg = gen_func(m);
            entries2.push(getJSONValue(arg, &arg_field_type));
        }
        // 同上：末尾 nil 是源测试输入格式的一部分，不代表聚合结果中的业务值。
        entries2.push(Nil);

        entries3.extend(entries1.clone());
        entries3.extend(entries2.clone());

        tests.push(buildAggTester(
            ast::AggFuncJsonArrayagg,
            arg_type,
            0,
            num_rows,
            types::CreateBinaryJSON(entries1),
            types::CreateBinaryJSON(entries2),
            types::CreateBinaryJSON(entries3),
        ));
    }

    for test in tests {
        testMergePartialResult(test);
    }
}

// TestJsonArrayagg 对应 Go 测试：验证普通聚合把输入行按顺序收集为 JSON array。
#[test]
pub fn test_json_arrayagg() {
    let type_list = vec![
        mysql::TypeLonglong,
        mysql::TypeDouble,
        mysql::TypeFloat,
        mysql::TypeString,
        mysql::TypeJSON,
        mysql::TypeDate,
        mysql::TypeDuration,
    ];

    let mut tests = Vec::with_capacity(type_list.len());
    let num_rows = 5;
    for arg_type in type_list {
        let mut entries = Vec::new();

        let arg_field_type = types::NewFieldType(arg_type);
        let gen_func = getDataGenFunc(&arg_field_type);

        for m in 0..num_rows {
            let arg = gen_func(m);
            entries.push(getJSONValue(arg, &arg_field_type));
        }
        // Go 侧为了生成源 Chunk 追加 nil；这里作为 fixture 元素保留。
        entries.push(Nil);

        tests.push(buildAggTester(
            ast::AggFuncJsonArrayagg,
            arg_type,
            0,
            num_rows,
            Nil,
            types::CreateBinaryJSON(entries),
        ));
    }

    for test in tests {
        testAggFuncWithoutDistinct(test);
    }
}

// jsonArrayaggMemDeltaGens 对应 Go helper：按输入类型估算 JSON array 追加元素的内存增量。
pub fn json_arrayagg_mem_delta_gens(param: updateMemDeltaGensParams) -> Result<Vec<i64>, errors::Error> {
    let mut mem_deltas = Vec::new();
    for i in 0..param.srcChk.NumRows() {
        let row = param.srcChk.GetRow(i);
        if row.IsNull(0) {
            // NULL 元素仍以 interface{} 形式存入 JSON array 的 entries。
            mem_deltas.push(aggfuncs::DefInterfaceSize);
            continue;
        }

        let mut mem_delta = 0_i64;
        mem_delta += aggfuncs::DefInterfaceSize;
        match param.keyType.GetType() {
            mysql::TypeLonglong => mem_delta += aggfuncs::DefUint64Size,
            mysql::TypeFloat => mem_delta += aggfuncs::DefFloat64Size,
            mysql::TypeDouble => mem_delta += aggfuncs::DefFloat64Size,
            mysql::TypeString => {
                let val = row.GetString(0);
                mem_delta += val.len() as i64;
            }
            mysql::TypeJSON => {
                let val = row.GetJSON(0);
                // +1 for the memory usage of the JSONTypeCode of json
                mem_delta += (val.Value.len() + 1) as i64;
            }
            mysql::TypeDuration => mem_delta += aggfuncs::DefDurationSize,
            mysql::TypeDate | mysql::TypeDatetime => mem_delta += aggfuncs::DefTimeSize,
            mysql::TypeNewDecimal => mem_delta += aggfuncs::DefFloat64Size,
            _ => {
                // Go 版遇到未列出的类型直接返回错误，便于测试暴露 fixture 漏洞。
                return Err(errors::Errorf(format!("unsupported type - {:?}", param.keyType.GetType())));
            }
        }
        mem_deltas.push(mem_delta);
    }
    Ok(mem_deltas)
}

// TestMemJsonArrayagg 对应 Go 测试：对 JSON_ARRAYAGG 支持的类型逐一验证内存估算。
#[test]
pub fn test_mem_json_arrayagg() {
    let type_list = vec![
        mysql::TypeLonglong,
        mysql::TypeDouble,
        mysql::TypeString,
        mysql::TypeJSON,
        mysql::TypeDuration,
        mysql::TypeNewDecimal,
        mysql::TypeDate,
    ];

    let mut tests = Vec::with_capacity(type_list.len());
    let num_rows = 5;
    for arg_type in type_list {
        tests.push(buildAggMemTester(
            ast::AggFuncJsonArrayagg,
            arg_type,
            0,
            num_rows,
            aggfuncs::DefPartialResult4JsonArrayagg + aggfuncs::DefSliceSize,
            json_arrayagg_mem_delta_gens,
            false,
        ));
    }

    for test in tests {
        testAggMemFunc(test);
    }
}
*/

use crate::aggfuncs::{
    DEF_BOOL_SIZE, DEF_DURATION_SIZE, DEF_FLOAT64_SIZE, DEF_INT64_SIZE, DEF_INTERFACE_SIZE,
    DEF_TIME_SIZE, DEF_UINT64_SIZE, SpillValue,
};
use crate::func_json_arrayagg::JsonArrayAgg;
use astersql_util_serialization::types;

/// 校验有序收集、merge 保持左右顺序，以及 reset 后 result 为 None。
#[test]
fn json_arrayagg_preserves_order_memory_accounting_and_merge_order() {
    let empty = JsonArrayAgg::default();
    assert_eq!(empty.result(), None);

    // 左侧先写入 Int64/String，再 merge 右侧 Uint64，顺序应为 1, "a", 2。
    let mut left = JsonArrayAgg::default();
    assert_eq!(
        left.update([
            SpillValue::Bool(true),
            SpillValue::Int64(1),
            SpillValue::Uint64(2),
            SpillValue::Float64(3.0),
            SpillValue::String("a".into()),
        ]),
        DEF_INTERFACE_SIZE * 5
            + DEF_BOOL_SIZE
            + DEF_INT64_SIZE
            + DEF_UINT64_SIZE
            + DEF_FLOAT64_SIZE
            + 1
    );
    let mut right = JsonArrayAgg::default();
    right.update([SpillValue::String("right".into())]);
    let source_before_merge = right.clone();
    left.merge(&right);
    assert_eq!(right, source_before_merge);
    assert_eq!(
        left.result(),
        Some(
            [
                SpillValue::Bool(true),
                SpillValue::Int64(1),
                SpillValue::Uint64(2),
                SpillValue::Float64(3.0),
                SpillValue::String("a".into()),
                SpillValue::String("right".into()),
            ]
            .as_slice()
        )
    );
    left.reset();
    assert_eq!(left.result(), None);
}

#[test]
fn json_arrayagg_accounts_for_all_supported_variable_and_temporal_values() {
    let mut array = JsonArrayAgg::default();
    let binary_json = types::BinaryJSON {
        Value: vec![1, 2, 3],
        ..Default::default()
    };
    let opaque = types::Opaque {
        Buf: vec![4, 5],
        ..Default::default()
    };

    assert_eq!(
        array.update([
            SpillValue::BinaryJson(binary_json),
            SpillValue::Opaque(opaque),
            SpillValue::Time(types::Time::default()),
            SpillValue::Duration(types::Duration::default()),
        ]),
        DEF_INTERFACE_SIZE * 4 + (3 + 1) + (2 + 1) + DEF_TIME_SIZE + DEF_DURATION_SIZE
    );
}
