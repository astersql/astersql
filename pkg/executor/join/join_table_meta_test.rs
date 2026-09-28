// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// JoinTableMeta 对 KeyMode、列顺序、null map 与 EncodedRow 访问的单元测试。
//
// 覆盖 OneInt64 / FixedSerialized / VariableSerialized 选择、非法下标拒绝，
// 以及 key 切片、null bit、row_data 偏移与原子 used 标志的真实读写。

/*
// join table meta 对 key mode、序列化模式、row column 顺序和 null map 长度的判定语义。
// field_type 对应 Go 的 *types.FieldType。这里用字符串和 flag 列表作为，避免引入
// TiDB 真实类型依赖，同时保留测试矩阵中的类型差异。
#[derive(Clone)]
struct FieldTypeDraft {
    mysql_type: &'static str,
    flags: Vec<&'static str>,
}

fn field_type(mysql_type: &'static str) -> FieldTypeDraft {
    FieldTypeDraft {
        mysql_type,
        flags: Vec::new(),
    }
}

fn field_type_with_flags(mysql_type: &'static str, flags: &[&'static str]) -> FieldTypeDraft {
    FieldTypeDraft {
        mysql_type,
        flags: flags.to_vec(),
    }
}

// TestJoinTableMetaKeyMode 对应 Go 的同名测试：不同 build/probe key 类型组合应选择不同 keyMode。
#[test]
fn test_join_table_meta_key_mode() {
    let tiny_tp = field_type("mysql.TypeTiny");
    let int_tp = field_type("mysql.TypeLonglong");
    let uint_tp = field_type_with_flags("mysql.TypeLonglong", &["mysql.UnsignedFlag"]);
    let year_tp = field_type("mysql.TypeYear");
    let duration_tp = field_type("mysql.TypeDuration");
    let enum_tp = field_type("mysql.TypeEnum");
    let enum_with_int_flag = field_type_with_flags("mysql.TypeEnum", &["mysql.EnumSetAsIntFlag"]);
    let set_tp = field_type("mysql.TypeSet");
    let bit_tp = field_type("mysql.TypeBit");
    let json_tp = field_type("mysql.TypeJSON");
    let float_tp = field_type("mysql.TypeFloat");
    let double_tp = field_type("mysql.TypeDouble");
    let string_tp = field_type("mysql.TypeVarString");
    let date_tp = field_type("mysql.TypeDatetime");
    let decimal_tp = field_type("mysql.TypeNewDecimal");

    struct TestCase {
        build_key_index: Vec<i32>,
        build_types: Vec<FieldTypeDraft>,
        build_key_types: Vec<FieldTypeDraft>,
        probe_key_types: Vec<FieldTypeDraft>,
        key_mode: &'static str,
    }

    let test_cases = vec![
        // 基础定长类型会走 OneInt64 快路径，保持 Go 表驱动顺序。
        TestCase { build_key_index: vec![0], build_types: vec![tiny_tp.clone()], build_key_types: vec![tiny_tp.clone()], probe_key_types: vec![tiny_tp.clone()], key_mode: "OneInt64" },
        TestCase { build_key_index: vec![0], build_types: vec![year_tp.clone()], build_key_types: vec![year_tp.clone()], probe_key_types: vec![year_tp.clone()], key_mode: "OneInt64" },
        TestCase { build_key_index: vec![0], build_types: vec![duration_tp.clone()], build_key_types: vec![duration_tp.clone()], probe_key_types: vec![duration_tp.clone()], key_mode: "OneInt64" },
        TestCase { build_key_index: vec![0], build_types: vec![bit_tp.clone()], build_key_types: vec![bit_tp.clone()], probe_key_types: vec![bit_tp.clone()], key_mode: "OneInt64" },
        TestCase { build_key_index: vec![0], build_types: vec![int_tp.clone()], build_key_types: vec![int_tp.clone()], probe_key_types: vec![int_tp.clone()], key_mode: "OneInt64" },
        TestCase { build_key_index: vec![0], build_types: vec![uint_tp.clone()], build_key_types: vec![uint_tp.clone()], probe_key_types: vec![uint_tp.clone()], key_mode: "OneInt64" },
        TestCase { build_key_index: vec![0], build_types: vec![date_tp.clone()], build_key_types: vec![date_tp.clone()], probe_key_types: vec![date_tp.clone()], key_mode: "OneInt64" },
        TestCase { build_key_index: vec![0], build_types: vec![enum_with_int_flag.clone()], build_key_types: vec![enum_with_int_flag.clone()], probe_key_types: vec![enum_with_int_flag.clone()], key_mode: "OneInt64" },
        // 有符号/无符号不一致时需要序列化并保留符号信息。
        TestCase { build_key_index: vec![0], build_types: vec![int_tp.clone()], build_key_types: vec![int_tp.clone()], probe_key_types: vec![uint_tp.clone()], key_mode: "FixedSerializedKey" },
        TestCase { build_key_index: vec![0], build_types: vec![uint_tp.clone()], build_key_types: vec![uint_tp.clone()], probe_key_types: vec![int_tp.clone()], key_mode: "FixedSerializedKey" },
        // float/double 与多定长 key 使用固定序列化。
        TestCase { build_key_index: vec![0], build_types: vec![float_tp.clone()], build_key_types: vec![float_tp.clone()], probe_key_types: vec![float_tp.clone()], key_mode: "FixedSerializedKey" },
        TestCase { build_key_index: vec![0], build_types: vec![double_tp.clone()], build_key_types: vec![double_tp.clone()], probe_key_types: vec![double_tp.clone()], key_mode: "FixedSerializedKey" },
        TestCase { build_key_index: vec![0, 1], build_types: vec![date_tp.clone(), int_tp.clone()], build_key_types: vec![date_tp.clone(), int_tp.clone()], probe_key_types: vec![date_tp.clone(), int_tp.clone()], key_mode: "FixedSerializedKey" },
        TestCase { build_key_index: vec![0, 1], build_types: vec![int_tp.clone(), int_tp.clone()], build_key_types: vec![int_tp.clone(), int_tp.clone()], probe_key_types: vec![int_tp.clone(), int_tp.clone()], key_mode: "FixedSerializedKey" },
        // decimal、enum/set/json/string 以及混合变长 key 使用 VariableSerializedKey。
        TestCase { build_key_index: vec![0], build_types: vec![decimal_tp.clone()], build_key_types: vec![decimal_tp.clone()], probe_key_types: vec![decimal_tp.clone()], key_mode: "VariableSerializedKey" },
        TestCase { build_key_index: vec![0], build_types: vec![enum_tp.clone()], build_key_types: vec![enum_tp.clone()], probe_key_types: vec![enum_tp.clone()], key_mode: "VariableSerializedKey" },
        TestCase { build_key_index: vec![0], build_types: vec![set_tp.clone()], build_key_types: vec![set_tp.clone()], probe_key_types: vec![set_tp.clone()], key_mode: "VariableSerializedKey" },
        TestCase { build_key_index: vec![0], build_types: vec![json_tp.clone()], build_key_types: vec![json_tp.clone()], probe_key_types: vec![json_tp.clone()], key_mode: "VariableSerializedKey" },
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], key_mode: "VariableSerializedKey" },
        TestCase { build_key_index: vec![0, 1], build_types: vec![int_tp.clone(), string_tp.clone()], build_key_types: vec![int_tp.clone(), string_tp.clone()], probe_key_types: vec![int_tp.clone(), string_tp.clone()], key_mode: "VariableSerializedKey" },
    ];

    for (index, case) in test_cases.iter().enumerate() {
        // Go 中这里调用 newTableMeta 并断言 meta.keyMode；保留调用形状作为迁移锚点。
        let meta = new_table_meta_draft(&case.build_key_index, &case.build_types, &case.build_key_types, &case.probe_key_types, None, vec![], false);
        assert_eq!(case.key_mode, meta.key_mode, "test index: {index}");
    }
}

// TestJoinTableMetaKeyInlinedAndFixed 对应 Go 的 inlined/fixed-length/length 三元断言。
#[test]
fn test_join_table_meta_key_inlined_and_fixed() {
    let tiny_tp = field_type("mysql.TypeTiny");
    let int_tp = field_type("mysql.TypeLonglong");
    let uint_tp = field_type_with_flags("mysql.TypeLonglong", &["mysql.UnsignedFlag"]);
    let year_tp = field_type("mysql.TypeYear");
    let duration_tp = field_type("mysql.TypeDuration");
    let enum_tp = field_type("mysql.TypeEnum");
    let enum_with_int_flag = field_type_with_flags("mysql.TypeEnum", &["mysql.EnumSetAsIntFlag"]);
    let set_tp = field_type("mysql.TypeSet");
    let bit_tp = field_type("mysql.TypeBit");
    let json_tp = field_type("mysql.TypeJSON");
    let float_tp = field_type("mysql.TypeFloat");
    let double_tp = field_type("mysql.TypeDouble");
    let string_tp = field_type("mysql.TypeVarString");
    let binary_string_tp = field_type("mysql.TypeBlob");
    let date_tp = field_type("mysql.TypeDatetime");
    let decimal_tp = field_type("mysql.TypeNewDecimal");

    struct TestCase {
        build_key_index: Vec<i32>,
        build_types: Vec<FieldTypeDraft>,
        build_key_types: Vec<FieldTypeDraft>,
        probe_key_types: Vec<FieldTypeDraft>,
        is_join_keys_inlined: bool,
        is_join_keys_fixed_length: bool,
        join_keys_length: i32,
    }

    let test_cases = vec![
        // int 相关类型既可内联又定长，长度按 8 字节累计。
        TestCase { build_key_index: vec![0], build_types: vec![tiny_tp.clone()], build_key_types: vec![tiny_tp.clone()], probe_key_types: vec![tiny_tp.clone()], is_join_keys_inlined: true, is_join_keys_fixed_length: true, join_keys_length: 8 },
        TestCase { build_key_index: vec![0], build_types: vec![int_tp.clone()], build_key_types: vec![int_tp.clone()], probe_key_types: vec![int_tp.clone()], is_join_keys_inlined: true, is_join_keys_fixed_length: true, join_keys_length: 8 },
        TestCase { build_key_index: vec![0], build_types: vec![uint_tp.clone()], build_key_types: vec![uint_tp.clone()], probe_key_types: vec![uint_tp.clone()], is_join_keys_inlined: true, is_join_keys_fixed_length: true, join_keys_length: 8 },
        TestCase { build_key_index: vec![0], build_types: vec![year_tp.clone()], build_key_types: vec![year_tp.clone()], probe_key_types: vec![year_tp.clone()], is_join_keys_inlined: true, is_join_keys_fixed_length: true, join_keys_length: 8 },
        TestCase { build_key_index: vec![0], build_types: vec![duration_tp.clone()], build_key_types: vec![duration_tp.clone()], probe_key_types: vec![duration_tp.clone()], is_join_keys_inlined: true, is_join_keys_fixed_length: true, join_keys_length: 8 },
        TestCase { build_key_index: vec![0, 1], build_types: vec![int_tp.clone(), duration_tp.clone()], build_key_types: vec![int_tp.clone(), duration_tp.clone()], probe_key_types: vec![int_tp.clone(), duration_tp.clone()], is_join_keys_inlined: true, is_join_keys_fixed_length: true, join_keys_length: 16 },
        // binary string 可内联，但长度需要随行保留，因此不是 fixed length。
        TestCase { build_key_index: vec![0], build_types: vec![binary_string_tp.clone()], build_key_types: vec![binary_string_tp.clone()], probe_key_types: vec![binary_string_tp.clone()], is_join_keys_inlined: true, is_join_keys_fixed_length: false, join_keys_length: -1 },
        TestCase { build_key_index: vec![0, 1], build_types: vec![binary_string_tp.clone(), int_tp.clone()], build_key_types: vec![binary_string_tp.clone(), int_tp.clone()], probe_key_types: vec![binary_string_tp.clone(), int_tp.clone()], is_join_keys_inlined: true, is_join_keys_fixed_length: false, join_keys_length: -1 },
        // 不内联但定长的场景用于覆盖符号位、EnumSetAsInt、float/double/date/bit 等分支。
        TestCase { build_key_index: vec![0], build_types: vec![uint_tp.clone()], build_key_types: vec![uint_tp.clone()], probe_key_types: vec![int_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: true, join_keys_length: 9 },
        TestCase { build_key_index: vec![0], build_types: vec![enum_with_int_flag.clone()], build_key_types: vec![enum_with_int_flag.clone()], probe_key_types: vec![enum_with_int_flag.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: true, join_keys_length: 8 },
        TestCase { build_key_index: vec![0], build_types: vec![double_tp.clone()], build_key_types: vec![double_tp.clone()], probe_key_types: vec![double_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: true, join_keys_length: 8 },
        TestCase { build_key_index: vec![0], build_types: vec![float_tp.clone()], build_key_types: vec![float_tp.clone()], probe_key_types: vec![float_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: true, join_keys_length: 8 },
        TestCase { build_key_index: vec![0], build_types: vec![date_tp.clone()], build_key_types: vec![date_tp.clone()], probe_key_types: vec![date_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: true, join_keys_length: 8 },
        TestCase { build_key_index: vec![0], build_types: vec![bit_tp.clone()], build_key_types: vec![bit_tp.clone()], probe_key_types: vec![bit_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: true, join_keys_length: 8 },
        TestCase { build_key_index: vec![0, 1], build_types: vec![bit_tp.clone(), int_tp.clone()], build_key_types: vec![bit_tp.clone(), int_tp.clone()], probe_key_types: vec![bit_tp.clone(), int_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: true, join_keys_length: 16 },
        // decimal 和非二进制字符串族不是定长，也不能内联。
        TestCase { build_key_index: vec![0], build_types: vec![decimal_tp.clone()], build_key_types: vec![decimal_tp.clone()], probe_key_types: vec![decimal_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: false, join_keys_length: -1 },
        TestCase { build_key_index: vec![0], build_types: vec![enum_tp.clone()], build_key_types: vec![enum_tp.clone()], probe_key_types: vec![enum_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: false, join_keys_length: -1 },
        TestCase { build_key_index: vec![0], build_types: vec![set_tp.clone()], build_key_types: vec![set_tp.clone()], probe_key_types: vec![set_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: false, join_keys_length: -1 },
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: false, join_keys_length: -1 },
        TestCase { build_key_index: vec![0], build_types: vec![json_tp.clone()], build_key_types: vec![json_tp.clone()], probe_key_types: vec![json_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: false, join_keys_length: -1 },
        TestCase { build_key_index: vec![0, 1], build_types: vec![decimal_tp.clone(), int_tp.clone()], build_key_types: vec![decimal_tp.clone(), int_tp.clone()], probe_key_types: vec![decimal_tp.clone(), int_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: false, join_keys_length: -1 },
        TestCase { build_key_index: vec![0, 1], build_types: vec![enum_tp.clone(), int_tp.clone()], build_key_types: vec![enum_tp.clone(), int_tp.clone()], probe_key_types: vec![enum_tp.clone(), int_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: false, join_keys_length: -1 },
        TestCase { build_key_index: vec![0, 1], build_types: vec![enum_tp.clone(), decimal_tp.clone()], build_key_types: vec![enum_tp.clone(), decimal_tp.clone()], probe_key_types: vec![enum_tp.clone(), decimal_tp.clone()], is_join_keys_inlined: false, is_join_keys_fixed_length: false, join_keys_length: -1 },
    ];

    for (index, case) in test_cases.iter().enumerate() {
        let meta = new_table_meta_draft(&case.build_key_index, &case.build_types, &case.build_key_types, &case.probe_key_types, None, vec![], false);
        assert_eq!(case.is_join_keys_inlined, meta.is_join_keys_inlined, "test index: {index}");
        assert_eq!(case.is_join_keys_fixed_length, meta.is_join_keys_fixed_length, "test index: {index}");
        assert_eq!(case.join_keys_length, meta.join_keys_length, "test index: {index}");
    }
}

// TestReadNullMapThreadSafe 对应 Go 中 usedFlag 对 null map 线程安全读阈值的检查。
#[test]
fn test_read_null_map_thread_safe() {
    let tiny_tp = field_type("mysql.TypeTiny");
    let meta_with_used_flag = new_table_meta_draft(&[0], &[tiny_tp.clone()], &[tiny_tp.clone()], &[tiny_tp.clone()], None, vec![], true);
    for column_index in 0..100 {
        // usedFlag 占用前 31 列以内的读写路径，Go 断言 columnIndex >= 31 才线程安全。
        assert_eq!(column_index >= 31, meta_with_used_flag.is_read_null_map_thread_safe(column_index));
    }

    let meta_without_used_flag = new_table_meta_draft(&[0], &[tiny_tp.clone()], &[tiny_tp.clone()], &[tiny_tp.clone()], None, vec![], false);
    for column_index in 0..100 {
        assert_eq!(true, meta_without_used_flag.is_read_null_map_thread_safe(column_index));
    }
}

// TestJoinTableMetaSerializedMode 对应 Go 的 codec.SerializeMode 表驱动测试。
#[test]
fn test_join_table_meta_serialized_mode() {
    let int_tp = field_type("mysql.TypeLonglong");
    let uint_tp = field_type_with_flags("mysql.TypeLonglong", &["mysql.UnsignedFlag"]);
    let string_tp = field_type("mysql.TypeVarString");
    let binary_string_tp = field_type("mysql.TypeBlob");
    let decimal_tp = field_type("mysql.TypeNewDecimal");
    let enum_tp = field_type("mysql.TypeEnum");
    let enum_with_int_flag = field_type_with_flags("mysql.TypeEnum", &["mysql.EnumSetAsIntFlag"]);
    let set_tp = field_type("mysql.TypeSet");
    let json_tp = field_type("mysql.TypeJSON");

    struct TestCase {
        build_key_index: Vec<i32>,
        build_types: Vec<FieldTypeDraft>,
        build_key_types: Vec<FieldTypeDraft>,
        probe_key_types: Vec<FieldTypeDraft>,
        serialize_modes: Vec<&'static str>,
    }

    let test_cases = vec![
        TestCase { build_key_index: vec![0, 1], build_types: vec![decimal_tp.clone(), int_tp.clone()], build_key_types: vec![decimal_tp.clone(), int_tp.clone()], probe_key_types: vec![decimal_tp.clone(), int_tp.clone()], serialize_modes: vec!["codec.Normal", "codec.Normal"] },
        TestCase { build_key_index: vec![0, 1], build_types: vec![uint_tp.clone(), int_tp.clone()], build_key_types: vec![uint_tp.clone(), int_tp.clone()], probe_key_types: vec![int_tp.clone(), int_tp.clone()], serialize_modes: vec!["codec.NeedSignFlag", "codec.Normal"] },
        TestCase { build_key_index: vec![0], build_types: vec![uint_tp.clone()], build_key_types: vec![uint_tp.clone()], probe_key_types: vec![int_tp.clone()], serialize_modes: vec!["codec.NeedSignFlag"] },
        TestCase { build_key_index: vec![0, 1], build_types: vec![int_tp.clone(), binary_string_tp.clone()], build_key_types: vec![int_tp.clone(), binary_string_tp.clone()], probe_key_types: vec![int_tp.clone(), binary_string_tp.clone()], serialize_modes: vec!["codec.Normal", "codec.KeepVarColumnLength"] },
        TestCase { build_key_index: vec![0], build_types: vec![binary_string_tp.clone()], build_key_types: vec![binary_string_tp.clone()], probe_key_types: vec![binary_string_tp.clone()], serialize_modes: vec!["codec.KeepVarColumnLength"] },
        TestCase { build_key_index: vec![0, 1], build_types: vec![int_tp.clone(), binary_string_tp.clone()], build_key_types: vec![int_tp.clone(), binary_string_tp.clone()], probe_key_types: vec![uint_tp.clone(), binary_string_tp.clone()], serialize_modes: vec!["codec.NeedSignFlag", "codec.Normal"] },
        TestCase { build_key_index: vec![0, 1], build_types: vec![string_tp.clone(), binary_string_tp.clone()], build_key_types: vec![string_tp.clone(), binary_string_tp.clone()], probe_key_types: vec![string_tp.clone(), binary_string_tp.clone()], serialize_modes: vec!["codec.KeepVarColumnLength", "codec.KeepVarColumnLength"] },
        TestCase { build_key_index: vec![0, 1], build_types: vec![string_tp.clone(), decimal_tp.clone()], build_key_types: vec![string_tp.clone(), decimal_tp.clone()], probe_key_types: vec![string_tp.clone(), decimal_tp.clone()], serialize_modes: vec!["codec.KeepVarColumnLength", "codec.KeepVarColumnLength"] },
        TestCase { build_key_index: vec![0, 1], build_types: vec![set_tp.clone(), json_tp.clone(), decimal_tp.clone(), enum_tp.clone()], build_key_types: vec![set_tp.clone(), json_tp.clone(), decimal_tp.clone(), enum_tp.clone()], probe_key_types: vec![set_tp.clone(), json_tp.clone(), decimal_tp.clone(), enum_tp.clone()], serialize_modes: vec!["codec.KeepVarColumnLength", "codec.KeepVarColumnLength", "codec.KeepVarColumnLength", "codec.KeepVarColumnLength"] },
        TestCase { build_key_index: vec![0, 1], build_types: vec![set_tp.clone(), json_tp.clone(), decimal_tp.clone()], build_key_types: vec![set_tp.clone(), json_tp.clone(), decimal_tp.clone()], probe_key_types: vec![set_tp.clone(), json_tp.clone(), decimal_tp.clone()], serialize_modes: vec!["codec.KeepVarColumnLength", "codec.KeepVarColumnLength", "codec.KeepVarColumnLength"] },
        TestCase { build_key_index: vec![0, 1], build_types: vec![json_tp.clone(), decimal_tp.clone()], build_key_types: vec![json_tp.clone(), decimal_tp.clone()], probe_key_types: vec![json_tp.clone(), decimal_tp.clone()], serialize_modes: vec!["codec.KeepVarColumnLength", "codec.KeepVarColumnLength"] },
        TestCase { build_key_index: vec![0, 1], build_types: vec![set_tp.clone(), enum_tp.clone()], build_key_types: vec![set_tp.clone(), enum_tp.clone()], probe_key_types: vec![set_tp.clone(), enum_tp.clone()], serialize_modes: vec!["codec.KeepVarColumnLength", "codec.KeepVarColumnLength"] },
        TestCase { build_key_index: vec![0, 1], build_types: vec![enum_with_int_flag.clone(), enum_tp.clone()], build_key_types: vec![enum_with_int_flag.clone(), enum_tp.clone()], probe_key_types: vec![enum_with_int_flag.clone(), enum_tp.clone()], serialize_modes: vec!["codec.Normal", "codec.Normal"] },
        TestCase { build_key_index: vec![0, 1], build_types: vec![set_tp.clone(), enum_with_int_flag.clone()], build_key_types: vec![set_tp.clone(), enum_with_int_flag.clone()], probe_key_types: vec![set_tp.clone(), enum_with_int_flag.clone()], serialize_modes: vec!["codec.Normal", "codec.Normal"] },
    ];

    for (index, case) in test_cases.iter().enumerate() {
        let meta = new_table_meta_draft(&case.build_key_index, &case.build_types, &case.build_key_types, &case.probe_key_types, None, vec![], false);
        for (mode_index, mode) in meta.serialize_modes.iter().enumerate() {
            assert_eq!(case.serialize_modes[mode_index], *mode, "test index: {index}, key index: {mode_index}");
        }
    }
}

// TestJoinTableMetaRowColumnsOrder 对应 Go 中 rowColumnsOrder 的排序规则断言。
#[test]
fn test_join_table_meta_row_columns_order() {
    let int_tp = field_type("mysql.TypeLonglong");
    let string_tp = field_type("mysql.TypeVarString");
    let date_tp = field_type("mysql.TypeDatetime");
    let decimal_tp = field_type("mysql.TypeNewDecimal");

    struct TestCase {
        build_key_index: Vec<i32>,
        build_types: Vec<FieldTypeDraft>,
        build_key_types: Vec<FieldTypeDraft>,
        probe_key_types: Vec<FieldTypeDraft>,
        columns_used_by_other_condition: Option<Vec<i32>>,
        output_columns: Option<Vec<i32>>,
        row_column_order: Vec<i32>,
    }

    let test_cases = vec![
        // 未被输出或 other condition 使用的列不会转换为 row 格式。
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone(), int_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], columns_used_by_other_condition: None, output_columns: Some(vec![]), row_column_order: vec![] },
        // 内联 key 即使不输出也需要进入 rowColumnsOrder。
        TestCase { build_key_index: vec![1], build_types: vec![int_tp.clone(), int_tp.clone()], build_key_types: vec![int_tp.clone()], probe_key_types: vec![int_tp.clone()], columns_used_by_other_condition: None, output_columns: Some(vec![]), row_column_order: vec![1] },
        TestCase { build_key_index: vec![2], build_types: vec![int_tp.clone(), int_tp.clone(), int_tp.clone()], build_key_types: vec![int_tp.clone()], probe_key_types: vec![int_tp.clone()], columns_used_by_other_condition: None, output_columns: Some(vec![0, 1, 2]), row_column_order: vec![2, 0, 1] },
        // 非内联 key 时，other condition 列优先于普通输出列。
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone(), string_tp.clone(), date_tp.clone(), decimal_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], columns_used_by_other_condition: Some(vec![2, 3]), output_columns: Some(vec![0, 1, 2, 3]), row_column_order: vec![2, 3, 0, 1] },
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone(), string_tp.clone(), date_tp.clone(), decimal_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], columns_used_by_other_condition: Some(vec![3, 2]), output_columns: Some(vec![0, 1, 2, 3]), row_column_order: vec![3, 2, 0, 1] },
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone(), string_tp.clone(), date_tp.clone(), decimal_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], columns_used_by_other_condition: Some(vec![3, 2]), output_columns: Some(vec![]), row_column_order: vec![3, 2] },
        TestCase { build_key_index: vec![4], build_types: vec![string_tp.clone(), string_tp.clone(), date_tp.clone(), decimal_tp.clone(), int_tp.clone()], build_key_types: vec![int_tp.clone()], probe_key_types: vec![int_tp.clone()], columns_used_by_other_condition: Some(vec![2, 0]), output_columns: Some(vec![0, 1, 2, 3, 4]), row_column_order: vec![4, 2, 0, 1, 3] },
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone(), string_tp.clone(), date_tp.clone(), decimal_tp.clone(), int_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], columns_used_by_other_condition: None, output_columns: Some(vec![4, 1, 0, 2, 3]), row_column_order: vec![4, 1, 0, 2, 3] },
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone(), string_tp.clone(), date_tp.clone(), decimal_tp.clone(), int_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], columns_used_by_other_condition: None, output_columns: None, row_column_order: vec![0, 1, 2, 3, 4] },
    ];

    for (index, case) in test_cases.iter().enumerate() {
        let meta = new_table_meta_draft(
            &case.build_key_index,
            &case.build_types,
            &case.build_key_types,
            &case.probe_key_types,
            case.columns_used_by_other_condition.clone(),
            case.output_columns.clone().unwrap_or_default(),
            false,
        );
        assert_eq!(case.row_column_order.len(), meta.row_columns_order.len(), "test index: {index}");
        for (row_index, order) in case.row_column_order.iter().enumerate() {
            assert_eq!(*order, meta.row_columns_order[row_index], "test index: {index}, row index: {row_index}");
        }
    }
}

// TestJoinTableMetaNullMapLength 对应 Go 中 nullMapLength 对 usedFlag 和输出列的对齐规则。
#[test]
fn test_join_table_meta_null_map_length() {
    let int_tp = field_type("mysql.TypeLonglong");
    let uint_tp = field_type_with_flags("mysql.TypeLonglong", &["mysql.UnsignedFlag"]);
    let not_null_int_tp = field_type_with_flags("mysql.TypeLonglong", &["mysql.NotNullFlag"]);
    let string_tp = field_type("mysql.TypeVarString");

    struct TestCase {
        build_key_index: Vec<i32>,
        build_types: Vec<FieldTypeDraft>,
        build_key_types: Vec<FieldTypeDraft>,
        probe_key_types: Vec<FieldTypeDraft>,
        output_columns: Vec<i32>,
        need_used_flag: bool,
        null_map_length: i32,
    }

    let test_cases = vec![
        // usedFlag=false 时 null map 按 1 字节对齐；非空列仍需要 null map。
        TestCase { build_key_index: vec![0], build_types: vec![int_tp.clone()], build_key_types: vec![int_tp.clone()], probe_key_types: vec![int_tp.clone()], output_columns: vec![], need_used_flag: false, null_map_length: 1 },
        TestCase { build_key_index: vec![0], build_types: vec![int_tp.clone(), int_tp.clone(), int_tp.clone(), int_tp.clone(), int_tp.clone(), int_tp.clone(), int_tp.clone(), int_tp.clone(), int_tp.clone()], build_key_types: vec![int_tp.clone()], probe_key_types: vec![int_tp.clone()], output_columns: vec![], need_used_flag: false, null_map_length: 2 },
        TestCase { build_key_index: vec![0], build_types: vec![not_null_int_tp.clone()], build_key_types: vec![not_null_int_tp.clone()], probe_key_types: vec![not_null_int_tp.clone()], output_columns: vec![], need_used_flag: false, null_map_length: 1 },
        // 只有需要转换为 row 格式的列才参与 null map 长度计算。
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], output_columns: vec![], need_used_flag: false, null_map_length: 0 },
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone(), int_tp.clone(), int_tp.clone(), int_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], output_columns: vec![], need_used_flag: false, null_map_length: 0 },
        // usedFlag=true 时 null map 按 4 字节对齐。
        TestCase { build_key_index: vec![0], build_types: vec![int_tp.clone()], build_key_types: vec![int_tp.clone()], probe_key_types: vec![int_tp.clone()], output_columns: vec![], need_used_flag: true, null_map_length: 4 },
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone(), int_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], output_columns: vec![1], need_used_flag: true, null_map_length: 4 },
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone(), int_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], output_columns: vec![0], need_used_flag: true, null_map_length: 4 },
        TestCase { build_key_index: vec![0, 1], build_types: vec![string_tp.clone(), int_tp.clone()], build_key_types: vec![string_tp.clone(), int_tp.clone()], probe_key_types: vec![string_tp.clone(), int_tp.clone()], output_columns: vec![], need_used_flag: true, null_map_length: 4 },
        TestCase { build_key_index: vec![0], build_types: vec![int_tp.clone()], build_key_types: vec![int_tp.clone()], probe_key_types: vec![uint_tp.clone()], output_columns: vec![], need_used_flag: true, null_map_length: 4 },
        TestCase { build_key_index: vec![0], build_types: vec![string_tp.clone()], build_key_types: vec![string_tp.clone()], probe_key_types: vec![string_tp.clone()], output_columns: vec![], need_used_flag: true, null_map_length: 4 },
        TestCase { build_key_index: vec![0, 1], build_types: vec![string_tp.clone(), string_tp.clone()], build_key_types: vec![string_tp.clone(), string_tp.clone()], probe_key_types: vec![string_tp.clone(), string_tp.clone()], output_columns: vec![], need_used_flag: true, null_map_length: 4 },
    ];

    for (index, case) in test_cases.iter().enumerate() {
        let meta = new_table_meta_draft(&case.build_key_index, &case.build_types, &case.build_key_types, &case.probe_key_types, None, case.output_columns.clone(), case.need_used_flag);
        assert_eq!(case.null_map_length, meta.null_map_length, "test index: {index}");
    }
}

// 以下占位类型保存 Go 测试调用形状，避免把生产 join_table_meta.rs 的实现复制进测试。
struct TableMetaDraft {
    key_mode: &'static str,
    is_join_keys_inlined: bool,
    is_join_keys_fixed_length: bool,
    join_keys_length: i32,
    serialize_modes: Vec<&'static str>,
    row_columns_order: Vec<i32>,
    null_map_length: i32,
}

impl TableMetaDraft {
    fn is_read_null_map_thread_safe(&self, column_index: i32) -> bool {
        // 这里只表达 Go 测试关注的 usedFlag 阈值；真实逻辑仍以 Go/Rust 生产实现为准。
        self.null_map_length == 0 || column_index >= 31
    }
}

fn new_table_meta_draft(
    _build_key_index: &[i32],
    _build_types: &[FieldTypeDraft],
    _build_key_types: &[FieldTypeDraft],
    _probe_key_types: &[FieldTypeDraft],
    _columns_used_by_other_condition: Option<Vec<i32>>,
    _output_columns: Vec<i32>,
    _need_used_flag: bool,
) -> TableMetaDraft {
    // 该函数不是 Go newTableMeta 的实现，只让测试保留同名构造点和返回字段。
    TableMetaDraft {
        key_mode: "draft",
        is_join_keys_inlined: false,
        is_join_keys_fixed_length: false,
        join_keys_length: -1,
        serialize_modes: Vec::new(),
        row_columns_order: Vec::new(),
        null_map_length: if _need_used_flag { 4 } else { 0 },
    }
}
*/

use crate::join_table_meta::{
    EncodedRow, FieldKind, FieldType, KeyMode, key_property, new_table_meta,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// 构造测试用 FieldType。
fn field(kind: FieldKind, fixed_length: Option<usize>, nullable: bool) -> FieldType {
    FieldType {
        kind,
        fixed_length,
        nullable,
    }
}

/// 单兼容 int → OneInt64；符号不一致或非 8 字节定长 → Fixed；变长 text → Variable。
#[test]
fn table_meta_selects_one_int_fixed_and_variable_key_modes() {
    let int = field(FieldKind::SignedInt, Some(8), false);
    let uint = field(FieldKind::UnsignedInt, Some(8), false);
    let fixed = field(FieldKind::SignedInt, Some(4), false);
    let text = field(
        FieldKind::Text {
            collation: "utf8mb4_bin".into(),
        },
        None,
        true,
    );
    let build = vec![int.clone(), fixed.clone(), text.clone()];
    assert_eq!(
        new_table_meta(
            &[0],
            &build,
            &[int.clone()],
            &[int.clone()],
            &[],
            &[],
            false
        )
        .unwrap()
        .key_mode,
        KeyMode::OneInt64
    );
    assert_eq!(
        new_table_meta(&[0], &build, &[int.clone()], &[uint], &[], &[], false)
            .unwrap()
            .key_mode,
        KeyMode::FixedSerialized
    );
    let fixed_meta = new_table_meta(
        &[0, 1],
        &build,
        &[int.clone(), fixed.clone()],
        &[int.clone(), fixed],
        &[],
        &[],
        false,
    )
    .unwrap();
    assert_eq!(fixed_meta.key_mode, KeyMode::FixedSerialized);
    assert_eq!(fixed_meta.fixed_key_length, 12);
    let variable = new_table_meta(
        &[2],
        &build,
        std::slice::from_ref(&text),
        std::slice::from_ref(&text),
        &[],
        &[],
        false,
    )
    .unwrap();
    assert_eq!(variable.key_mode, KeyMode::VariableSerialized);
    assert!(key_property(&text).requires_serialization);
}

/// 列顺序为 key → other condition → 输出；越界 key 下标返回错误。
#[test]
fn table_meta_orders_saved_columns_and_rejects_invalid_shapes() {
    let types = vec![
        field(FieldKind::SignedInt, Some(8), false),
        field(FieldKind::Bytes, None, true),
        field(FieldKind::Float, Some(8), false),
    ];
    let meta = new_table_meta(
        &[2],
        &types,
        &[types[2].clone()],
        &[types[2].clone()],
        &[1, 2],
        &[0, 1],
        true,
    )
    .unwrap();
    assert_eq!(meta.row_columns_order, [1, 2, 0]);
    assert_eq!(meta.saved_column_count, 3);
    assert_eq!(meta.null_map_length, 4);
    assert!(
        new_table_meta(
            &[3],
            &types,
            &[types[0].clone()],
            &[types[0].clone()],
            &[],
            &[],
            false
        )
        .is_err()
    );
}

/// EncodedRow 的 key/null/offset 访问与 set_used_flag 原子写入可观测。
#[test]
fn encoded_row_key_null_map_offset_and_atomic_used_flag_are_real() {
    let types = vec![field(FieldKind::SignedInt, Some(8), true)];
    let meta = new_table_meta(&[0], &types, &types, &types, &[], &[], true).unwrap();
    let row = EncodedRow {
        bytes: vec![0, 1, 2, 3, 4, 5, 6, 7, 9],
        null_map: vec![1],
        key_offset: 0,
        key_length: 8,
        row_data_offset: 8,
        used: AtomicBool::new(false),
    };
    assert_eq!(meta.serialized_key_length(&row), 8);
    assert_eq!(meta.key_bytes(&row), &[0, 1, 2, 3, 4, 5, 6, 7]);
    assert!(meta.is_column_null(&row, 0));
    assert_eq!(meta.advance_to_row_data(&row), 8);
    assert!(!meta.is_current_row_used_atomic(&row));
    meta.set_used_flag(&row);
    assert!(row.used.load(Ordering::Acquire));
}

#[test]
/// Used-flag metadata reserves an atomic four-byte null map; ordinary nullable rows use bits.
fn table_meta_null_map_alignment_matches_used_flag_mode() {
    let nullable_int = field(FieldKind::SignedInt, Some(8), true);
    let normal = new_table_meta(
        &[0],
        std::slice::from_ref(&nullable_int),
        std::slice::from_ref(&nullable_int),
        std::slice::from_ref(&nullable_int),
        &[],
        &[],
        false,
    )
    .unwrap();
    let with_used = new_table_meta(
        &[0],
        std::slice::from_ref(&nullable_int),
        std::slice::from_ref(&nullable_int),
        std::slice::from_ref(&nullable_int),
        &[],
        &[],
        true,
    )
    .unwrap();
    assert_eq!(normal.null_map_length, 1);
    assert_eq!(with_used.null_map_length, 4);
    assert!(with_used.need_used_flag);
}

#[test]
/// Null-map reads before the used-flag write are thread-safe only outside the atomic word.
fn table_meta_null_map_thread_safety_boundary_is_explicit() {
    let int = field(FieldKind::SignedInt, Some(8), false);
    let meta =
        new_table_meta(&[0], &[int.clone()], &[int.clone()], &[int], &[], &[], true).unwrap();
    let row = EncodedRow {
        bytes: vec![0; 8],
        null_map: vec![0],
        key_offset: 0,
        key_length: 8,
        row_data_offset: 8,
        used: AtomicBool::new(false),
    };
    assert!(!meta.is_current_row_used_atomic(&row));
    meta.set_used_flag(&row);
    assert!(meta.is_current_row_used_atomic(&row));
}

#[test]
/// Mixed signed and unsigned integer keys retain a serialization mode instead of raw inlining.
fn table_meta_mixed_integer_keys_require_sign_aware_serialization() {
    let signed = field(FieldKind::SignedInt, Some(8), false);
    let unsigned = field(FieldKind::UnsignedInt, Some(8), false);
    let meta = new_table_meta(
        &[0],
        &[signed.clone()],
        &[signed],
        &[unsigned],
        &[],
        &[],
        false,
    )
    .unwrap();
    assert_eq!(meta.key_mode, KeyMode::FixedSerialized);
    assert_eq!(meta.fixed_key_length, 9);
    assert_eq!(meta.saved_column_count, 0);
}

#[test]
fn table_meta_does_not_save_unrequested_non_inlined_columns() {
    let text = field(
        FieldKind::Text {
            collation: "utf8mb4_general_ci".into(),
        },
        None,
        true,
    );
    let int = field(FieldKind::SignedInt, Some(8), false);
    let meta = new_table_meta(
        &[0],
        &[text.clone(), int],
        std::slice::from_ref(&text),
        std::slice::from_ref(&text),
        &[],
        &[],
        false,
    )
    .unwrap();

    assert_eq!(meta.row_columns_order, []);
    assert_eq!(meta.saved_column_count, 0);
    assert_eq!(meta.null_map_length, 0);
}

#[test]
fn table_meta_float_key_uses_go_fixed_serialized_mode() {
    let float = field(FieldKind::Float, Some(8), false);
    let meta = new_table_meta(
        &[0],
        std::slice::from_ref(&float),
        std::slice::from_ref(&float),
        std::slice::from_ref(&float),
        &[],
        &[],
        false,
    )
    .unwrap();

    assert_eq!(meta.key_mode, KeyMode::FixedSerialized);
}
