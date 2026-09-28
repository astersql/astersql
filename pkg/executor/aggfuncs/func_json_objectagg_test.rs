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

// JSON_OBJECTAGG 聚合的单元测试。
//
// 可执行用例覆盖重复 key 覆盖写，以及 NULL key 被拒绝。
// 大段注释保留 Go 侧多类型组合 merge / 内存增量测试草稿。

/*
//

// getJSONValue 对应 Go helper：把 Datum 转成 BinaryJSON 可接受的值。
pub fn get_json_value(second_arg: types::Datum, value_type: &types::FieldType) -> JsonValue {
    if value_type.GetType() == mysql::TypeString && value_type.GetCharset() == charset::CharsetBin {
        let mut buf = vec![0_u8; value_type.GetFlen() as usize];
        buf.copy_from_slice(second_arg.GetBytes());
        return types::Opaque {
            TypeCode: mysql::TypeString,
            Buf: buf,
        };
    }

    if value_type.GetType() == mysql::TypeFloat {
        // Go 为 JSON 编码稳定性把 float32 提升为 float64。
        return second_arg.GetFloat32() as f64;
    }

    second_arg.GetValue()
}

// TestMergePartialResult4JsonObjectagg 对应 Go 测试：覆盖 key/value 类型笛卡尔组合的 partial 合并。
#[test]
pub fn test_merge_partial_result_4_json_objectagg() {
    let type_list = vec![
        types::NewFieldType(mysql::TypeLonglong),
        types::NewFieldType(mysql::TypeDouble),
        types::NewFieldType(mysql::TypeFloat),
        types::NewFieldType(mysql::TypeString),
        types::NewFieldType(mysql::TypeJSON),
        types::NewFieldTypeBuilder()
            .SetType(mysql::TypeString)
            .SetFlen(10)
            .SetCharset(charset::CharsetBin)
            .BuildP(),
        types::NewFieldType(mysql::TypeDate),
        types::NewFieldType(mysql::TypeDuration),
    ];

    let mut arg_combines = Vec::new();
    for i in 0..type_list.len() {
        if type_list[i].GetCharset() == charset::CharsetBin {
            // skip because binary charset cannot be used as key.
            continue;
        }
        for j in 0..type_list.len() {
            arg_combines.push(vec![type_list[i].clone(), type_list[j].clone()]);
        }
    }

    let mut tests = Vec::with_capacity(arg_combines.len());
    let num_rows = 5;
    for arg_types in arg_combines.iter() {
        let mut entries1 = map::new_string_any();
        let mut entries2 = map::new_string_any();

        let f_gen_func = getDataGenFunc(&arg_types[0]);
        let s_gen_func = getDataGenFunc(&arg_types[1]);

        for m in 0..num_rows {
            let first_arg = f_gen_func(m);
            let second_arg = s_gen_func(m);
            let key_string = first_arg.ToString().0;
            entries1.insert(key_string, get_json_value(second_arg, &arg_types[1]));
        }

        for m in 2..num_rows {
            let first_arg = f_gen_func(m);
            let second_arg = s_gen_func(m);
            let key_string = first_arg.ToString().0;
            entries2.insert(key_string, get_json_value(second_arg, &arg_types[1]));
        }

        let agg_test = buildMultiArgsAggTesterWithFieldType(
            ast::AggFuncJsonObjectAgg,
            arg_types.clone(),
            types::NewFieldType(mysql::TypeJSON),
            num_rows,
            types::CreateBinaryJSON(entries1.clone()),
            types::CreateBinaryJSON(entries2),
            types::CreateBinaryJSON(entries1),
        );
        tests.push(agg_test);
    }

    let ctx = mock::NewContext();
    for test in tests {
        testMultiArgsMergePartialResult(&ctx, test);
    }
}

// TestJsonObjectagg 对应 Go 测试：验证普通 JSON_OBJECTAGG 按 key 覆盖/保存 value。
#[test]
pub fn test_json_objectagg() {
    let type_list = vec![
        types::NewFieldType(mysql::TypeLonglong),
        types::NewFieldType(mysql::TypeDouble),
        types::NewFieldType(mysql::TypeFloat),
        types::NewFieldType(mysql::TypeString),
        types::NewFieldType(mysql::TypeJSON),
        types::NewFieldTypeBuilder()
            .SetType(mysql::TypeString)
            .SetFlen(10)
            .SetCharset(charset::CharsetBin)
            .BuildP(),
        types::NewFieldType(mysql::TypeDate),
        types::NewFieldType(mysql::TypeDuration),
    ];

    let mut arg_combines = Vec::new();
    for i in 0..type_list.len() {
        if type_list[i].GetCharset() == charset::CharsetBin {
            // binary charset 不能作为 JSON object key，这个跳过逻辑来自 Go 测试本身。
            continue;
        }
        for j in 0..type_list.len() {
            arg_combines.push(vec![type_list[i].clone(), type_list[j].clone()]);
        }
    }

    let mut tests = Vec::with_capacity(arg_combines.len());
    let num_rows = 5;
    for arg_types in arg_combines.iter() {
        let mut entries = map::new_string_any();
        let f_gen_func = getDataGenFunc(&arg_types[0]);
        let s_gen_func = getDataGenFunc(&arg_types[1]);

        for m in 0..num_rows {
            let first_arg = f_gen_func(m);
            let second_arg = s_gen_func(m);
            let key_string = first_arg.ToString().0;
            entries.insert(key_string, get_json_value(second_arg, &arg_types[1]));
        }

        let agg_test = buildMultiArgsAggTesterWithFieldType(
            ast::AggFuncJsonObjectAgg,
            arg_types.clone(),
            types::NewFieldType(mysql::TypeJSON),
            num_rows,
            Nil,
            types::CreateBinaryJSON(entries),
        );
        tests.push(agg_test);
    }

    let ctx = mock::NewContext();
    for test in tests {
        testMultiArgsAggFunc(&ctx, test);
    }
}

// TestMemJsonObjectagg 对应 Go 测试：遍历 key/value 类型组合并分别验证 distinct 与非 distinct。
#[test]
pub fn test_mem_json_objectagg() {
    let type_list = vec![
        mysql::TypeLonglong,
        mysql::TypeDouble,
        mysql::TypeFloat,
        mysql::TypeString,
        mysql::TypeJSON,
        mysql::TypeDuration,
        mysql::TypeNewDecimal,
        mysql::TypeDate,
    ];

    let mut arg_combines = Vec::new();
    for i in 0..type_list.len() {
        for j in 0..type_list.len() {
            arg_combines.push(vec![type_list[i], type_list[j]]);
        }
    }

    let num_rows = 5;
    for arg_types in arg_combines {
        let mut entries = map::new_string_any();
        let f_gen_func = getDataGenFunc(types::NewFieldType(arg_types[0]));
        let s_gen_func = getDataGenFunc(types::NewFieldType(arg_types[1]));

        for m in 0..num_rows {
            let first_arg = f_gen_func(m);
            let second_arg = s_gen_func(m);
            let key_string = first_arg.ToString().0;
            entries.insert(key_string, second_arg.GetValue());
        }

        for (key, val) in entries.iter_mut() {
            match val {
                JsonValue::Decimal(x) => {
                    // Go 版把 MyDecimal 转成 float64，因为 appendBinary 不支持直接编码 decimal。
                    *val = JsonValue::Float64(x.ToFloat64().0);
                }
                JsonValue::Bytes(_) | JsonValue::Time(_) | JsonValue::Duration(_) => {
                    // appendBinary 不支持 []uint8、types.Time、types.Duration 时，Go 测试先转字符串。
                    *val = JsonValue::String(types::ToString(val).0);
                }
                _ => {
                    // 其它 JSON 可编码值保持原样。
                }
            }
            let _ = key;
        }

        let tests = vec![
            buildMultiArgsAggMemTester(
                ast::AggFuncJsonObjectAgg,
                arg_types.clone(),
                mysql::TypeJSON,
                num_rows,
                aggfuncs::DefPartialResult4JsonObjectAgg
                    + hack::DefBucketMemoryUsageForMapStringToAny,
                json_multi_args_mem_delta_gens,
                true,
            ),
            buildMultiArgsAggMemTester(
                ast::AggFuncJsonObjectAgg,
                arg_types,
                mysql::TypeJSON,
                num_rows,
                aggfuncs::DefPartialResult4JsonObjectAgg
                    + hack::DefBucketMemoryUsageForMapStringToAny,
                json_multi_args_mem_delta_gens,
                false,
            ),
        ];

        for test in tests {
            testMultiArgsAggMemFunc(test);
        }
    }
}

// jsonMultiArgsMemDeltaGens 对应 Go helper：JSON_OBJECTAGG 按 key 去重后估算 key 与 value 内存。
pub fn json_multi_args_mem_delta_gens(
    _ctx: sessionctx::Context,
    src_chk: &chunk::Chunk,
    data_types: Vec<types::FieldType>,
    _by_items: Vec<util::ByItems>,
) -> Result<Vec<i64>, errors::Error> {
    let mut mem_deltas = Vec::new();
    let mut seen_keys = map::new_string_bool();

    for i in 0..src_chk.NumRows() {
        let row = src_chk.GetRow(i);
        if row.IsNull(0) {
            mem_deltas.push(0);
            continue;
        }

        let datum = row.GetDatum(0, &data_types[0]);
        if datum.IsNull() {
            mem_deltas.push(0);
            continue;
        }

        let mut mem_delta = 0_i64;
        let (key, err) = datum.ToString();
        if err.is_some() {
            // Go 返回 fail to get key 错误；这里保留错误路径以标记 key 转换失败。
            return Err(errors::Errorf(format!("fail to get key - {}", key)));
        }
        if seen_keys.contains_key(&key) {
            mem_deltas.push(0);
            continue;
        }
        seen_keys.insert(key.clone(), true);
        mem_delta += key.len() as i64;

        mem_delta += aggfuncs::DefInterfaceSize;
        match data_types[1].GetType() {
            mysql::TypeLonglong => mem_delta += aggfuncs::DefUint64Size,
            mysql::TypeFloat => mem_delta += aggfuncs::DefFloat64Size,
            mysql::TypeDouble => mem_delta += aggfuncs::DefFloat64Size,
            mysql::TypeString => {
                let val = row.GetString(1);
                mem_delta += val.len() as i64;
            }
            mysql::TypeJSON => {
                let val = row.GetJSON(1);
                // +1 for the memory usage of the JSONTypeCode of json
                mem_delta += (val.Value.len() + 1) as i64;
            }
            mysql::TypeDuration => mem_delta += aggfuncs::DefDurationSize,
            mysql::TypeDate => mem_delta += aggfuncs::DefTimeSize,
            mysql::TypeNewDecimal => mem_delta += aggfuncs::DefFloat64Size,
            _ => {
                return Err(errors::Errorf(format!(
                    "unsupported type - {:?}",
                    data_types[1].GetType()
                )));
            }
        }
        mem_deltas.push(mem_delta);
    }

    Ok(mem_deltas)
}
*/

use crate::aggfuncs::{DEF_INT64_SIZE, DEF_INTERFACE_SIZE, SpillValue};
use crate::func_json_objectagg::JsonObjectAgg;

/// 校验同名 key 后写覆盖，以及 NULL key 触发包含 "NULL member names" 的错误。
#[test]
fn json_objectagg_overwrites_duplicate_keys_and_rejects_null_keys() {
    // 同一 key 连续写入时保留最后一次的 value。
    let mut object = JsonObjectAgg::default();
    object
        .update([
            (Some("k".into()), SpillValue::Int64(1)),
            (Some("k".into()), SpillValue::Int64(2)),
        ])
        .unwrap();
    assert_eq!(
        object.result().unwrap().get("k"),
        Some(&SpillValue::Int64(2))
    );
    // key 为 None 应对齐 MySQL：拒绝 NULL 成员名。
    let error = object
        .update([(None, SpillValue::String("invalid".into()))])
        .unwrap_err();
    assert!(error.0.contains("NULL member names"));
}

/// Go 仅为首次插入的 key/value 计入 update 增量；merge 则逐条计入源 partial。
#[test]
fn json_objectagg_reports_go_compatible_memory_deltas() {
    let mut destination = JsonObjectAgg::default();
    let inserted = destination
        .update([(Some("key".into()), SpillValue::Int64(1))])
        .unwrap();
    assert_eq!(
        inserted,
        "key".len() as i64 + DEF_INTERFACE_SIZE + DEF_INT64_SIZE
    );

    let replaced = destination
        .update([(Some("key".into()), SpillValue::Int64(2))])
        .unwrap();
    assert_eq!(replaced, 0);

    let mut source = JsonObjectAgg::default();
    source
        .update([(Some("key".into()), SpillValue::Int64(3))])
        .unwrap();
    assert_eq!(
        destination.merge(&source),
        "key".len() as i64 + DEF_INTERFACE_SIZE + DEF_INT64_SIZE
    );
    assert_eq!(
        destination.result().unwrap().get("key"),
        Some(&SpillValue::Int64(3))
    );
}
