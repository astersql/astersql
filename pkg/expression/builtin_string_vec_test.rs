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

// 字符串内建函数向量化测试与 benchmark case 表。
//
// 对应 Go `builtin_string_vec_test.go`：按内建函数分组保存返回类型、子参数类型、
// 字段类型与数据生成器配置，并提供 EvalOneVec / BuiltinFunc 两套入口。
// 含随机空格字符串 generator 草稿，便于对照 Go 生成器语义。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

// 这段逻辑覆盖字符串内建函数向量化 benchmark case、随机空格字符串 generator 与两组测试/benchmark 入口。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发、异步、IO 和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待 Rust crate 接线）：
// - "math/rand"
// - "testing"
// - "github.com/pingcap/tidb/pkg/parser/ast"
// - "github.com/pingcap/tidb/pkg/parser/charset"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/types"

// 迁移占位类型：这些名称来自 Go/TiDB 测试依赖，后续接入模块时再替换为真实 Rust 类型。
type GoAny = ();
type GoError = String;
type GoBytes = Vec<u8>;

// VecExprBenchCaseDraft 对应 Go 的 vecExprBenchCase；字段以字符串保存，避免误连真实 TiDB 类型。
/// VecExprBenchCaseDraft 对应 Go 的 vecExprBenchCase；字段以字符串保存，避免误连真实 TiDB 类型。
pub struct VecExprBenchCaseDraft {
    pub label: &'static str,
    pub ret_eval_type: &'static str,
    pub children_types: &'static str,
    pub children_field_types: &'static str,
    pub geners: &'static str,
    pub constants: &'static str,
    pub aes_modes: &'static str,
    pub chunk_size: &'static str,
    pub go_literal: &'static str,
}

// VecCaseGroupDraft 对应 Go map[string][]vecExprBenchCase 的单个内建函数分组。
/// VecCaseGroupDraft 对应 Go map[string][]vecExprBenchCase 的单个内建函数分组。
pub struct VecCaseGroupDraft {
    pub builtin: &'static str,
    pub cases: &'static [VecExprBenchCaseDraft],
}

// vec_case 保留 case 的返回类型、子参数类型、字段类型、数据生成器、常量参数和额外生成器参数。
/// vec_case 保留 case 的返回类型、子参数类型、字段类型、数据生成器、常量参数和额外生成器参数。
pub const fn vec_case(
    label: &'static str,
    ret_eval_type: &'static str,
    children_types: &'static str,
    children_field_types: &'static str,
    geners: &'static str,
    constants: &'static str,
    aes_modes: &'static str,
    chunk_size: &'static str,
    go_literal: &'static str,
) -> VecExprBenchCaseDraft {
    VecExprBenchCaseDraft {
        label,
        ret_eval_type,
        children_types,
        children_field_types,
        geners,
        constants,
        aes_modes,
        chunk_size,
        go_literal,
    }
}

// VECBUILTINSTRINGCASES 对应 Go 的向量化 case map；分组顺序和每组 case 顺序与源文件一致。
/// VECBUILTINSTRINGCASES 对应 Go 的向量化 case map；分组顺序和每组 case 顺序与源文件一致。
pub const VECBUILTINSTRINGCASES: &[VecCaseGroupDraft] = &[
    VecCaseGroupDraft {
        builtin: r#"ast.Length"#,
        cases: &LENGTH_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.ASCII"#,
        cases: &ASCII_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Concat"#,
        cases: &CONCAT_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.ConcatWS"#,
        cases: &CONCAT_WS_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Convert"#,
        cases: &CONVERT_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Substring"#,
        cases: &SUBSTRING_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.SubstringIndex"#,
        cases: &SUBSTRING_INDEX_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Locate"#,
        cases: &LOCATE_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Hex"#,
        cases: &HEX_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Unhex"#,
        cases: &UNHEX_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Trim"#,
        cases: &TRIM_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Translate"#,
        cases: &TRANSLATE_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.LTrim"#,
        cases: &LTRIM_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.RTrim"#,
        cases: &RTRIM_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Lpad"#,
        cases: &LPAD_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Rpad"#,
        cases: &RPAD_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.CharLength"#,
        cases: &CHAR_LENGTH_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.BitLength"#,
        cases: &BIT_LENGTH_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.CharFunc"#,
        cases: &CHAR_FUNC_VECBUILTINSTRINGCASES_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.FindInSet"#,
        cases: &FIND_IN_SET_VECBUILTINSTRINGCASES_CASES,
    },
];

// ast.Length 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Length 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const LENGTH_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETInt"#,
    r#"[types.ETString]"#,
    r#""#,
    r#"[newDefaultGener(0.2, types.ETString)]"#,
    r#""#,
    r#""#,
    r#""#,
    r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{newDefaultGener(0.2, types.ETString)}}"#,
)];

// ast.ASCII 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.ASCII 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const ASCII_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETInt"#,
    r#"[types.ETString]"#,
    r#""#,
    r#"[newDefaultGener(0.2, types.ETString)]"#,
    r#""#,
    r#""#,
    r#""#,
    r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{newDefaultGener(0.2, types.ETString)}}"#,
)];

// ast.Concat 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Concat 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const CONCAT_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETString, types.ETString, types.ETString]"#,
    r#""#,
    r#""#,
    r#""#,
    r#""#,
    r#""#,
    r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETString}}"#,
)];

// ast.ConcatWS 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.ConcatWS 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const CONCAT_WS_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString, types.ETString, types.ETString]"#,
        r#""#,
        r#"[&constStrGener[","]]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETString, types.ETString},
			geners:        []dataGenerator{&constStrGener{","}},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString, types.ETString, types.ETString]"#,
        r#""#,
        r#"[newDefaultGener(1, types.ETString)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETString, types.ETString},
			geners:        []dataGenerator{newDefaultGener(1, types.ETString)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString, types.ETString]"#,
        r#""#,
        r#"[ &constStrGener["<------------------>"], &constStrGener["1413006"], &constStrGener["idlfmv"], ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETString},
			geners: []dataGenerator{
				&constStrGener{"<------------------>"},
				&constStrGener{"1413006"},
				&constStrGener{"idlfmv"},
			},
		}"#,
    ),
];

// ast.Convert 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Convert 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const CONVERT_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#""#,
        r#"[nil, [Value: types.NewDatum("utf8"), RetType: types.NewFieldType(mysql.TypeString)]]"#,
        r#""#,
        r#""#,
        r#"{
			retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETString},
			constants: []*Constant{nil, {Value: types.NewDatum("utf8"), RetType: types.NewFieldType(mysql.TypeString)}},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#""#,
        r#"[nil, [Value: types.NewDatum("binary"), RetType: types.NewFieldType(mysql.TypeString)]]"#,
        r#""#,
        r#""#,
        r#"{
			retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETString},
			constants: []*Constant{nil, {Value: types.NewDatum("binary"), RetType: types.NewFieldType(mysql.TypeString)}},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#""#,
        r#"[nil, [Value: types.NewDatum("utf8mb4"), RetType: types.NewFieldType(mysql.TypeString)]]"#,
        r#""#,
        r#""#,
        r#"{
			retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETString},
			constants: []*Constant{nil, {Value: types.NewDatum("utf8mb4"), RetType: types.NewFieldType(mysql.TypeString)}},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#""#,
        r#"[nil, [Value: types.NewDatum("ascii"), RetType: types.NewFieldType(mysql.TypeString)]]"#,
        r#""#,
        r#""#,
        r#"{
			retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETString},
			constants: []*Constant{nil, {Value: types.NewDatum("ascii"), RetType: types.NewFieldType(mysql.TypeString)}},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#""#,
        r#"[nil, [Value: types.NewDatum("latin1"), RetType: types.NewFieldType(mysql.TypeString)]]"#,
        r#""#,
        r#""#,
        r#"{
			retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETString},
			constants: []*Constant{nil, {Value: types.NewDatum("latin1"), RetType: types.NewFieldType(mysql.TypeString)}},
		}"#,
    ),
];

// ast.Substring 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Substring 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const SUBSTRING_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt]"#,
        r#""#,
        r#"[newRandLenStrGener(0, 20), newRangeInt64Gener(-25, 25)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETInt},
			geners:        []dataGenerator{newRandLenStrGener(0, 20), newRangeInt64Gener(-25, 25)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt, types.ETInt]"#,
        r#""#,
        r#"[newRandLenStrGener(0, 20), newRangeInt64Gener(-25, 25), newRangeInt64Gener(-25, 25)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETInt},
			geners:        []dataGenerator{newRandLenStrGener(0, 20), newRangeInt64Gener(-25, 25), newRangeInt64Gener(-25, 25)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt, types.ETInt]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).BuildP()]"#,
        r#"[newRandLenStrGener(0, 20), newRangeInt64Gener(-25, 25), newRangeInt64Gener(-25, 25)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETInt},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).BuildP()},
			geners: []dataGenerator{newRandLenStrGener(0, 20), newRangeInt64Gener(-25, 25), newRangeInt64Gener(-25, 25)},
		}"#,
    ),
];

// ast.SubstringIndex 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.SubstringIndex 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const SUBSTRING_INDEX_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETString, types.ETString, types.ETInt]"#,
    r#""#,
    r#"[newRandLenStrGener(0, 20), newRandLenStrGener(0, 2), newRangeInt64Gener(-4, 4)]"#,
    r#""#,
    r#""#,
    r#""#,
    r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETInt},
			geners:        []dataGenerator{newRandLenStrGener(0, 20), newRandLenStrGener(0, 2), newRangeInt64Gener(-4, 4)},
		}"#,
)];

// ast.Locate 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Locate 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const LOCATE_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#"[newRandLenStrGener(0, 10), newRandLenStrGener(0, 20)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			geners:        []dataGenerator{newRandLenStrGener(0, 10), newRandLenStrGener(0, 20)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#"[newRandLenStrGener(1, 2), newRandLenStrGener(0, 20)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			geners:        []dataGenerator{newRandLenStrGener(1, 2), newRandLenStrGener(0, 20)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#"[newSelectStringGener([]string["01", "10", "001", "110", "0001", "1110"]), newSelectStringGener([]string["010010001000010", "101101110111101"])]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			geners:        []dataGenerator{newSelectStringGener([]string{"01", "10", "001", "110", "0001", "1110"}), newSelectStringGener([]string{"010010001000010", "101101110111101"})},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString, types.ETInt]"#,
        r#""#,
        r#"[newRandLenStrGener(0, 10), newRandLenStrGener(0, 20), newRangeInt64Gener(-10, 20)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETInt},
			geners:        []dataGenerator{newRandLenStrGener(0, 10), newRandLenStrGener(0, 20), newRangeInt64Gener(-10, 20)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString, types.ETInt]"#,
        r#""#,
        r#"[newRandLenStrGener(1, 2), newRandLenStrGener(0, 10), newRangeInt64Gener(0, 8)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETInt},
			geners:        []dataGenerator{newRandLenStrGener(1, 2), newRandLenStrGener(0, 10), newRangeInt64Gener(0, 8)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#"[newSelectStringGener([]string["01", "10", "001", "110", "0001", "1110"]), newSelectStringGener([]string["010010001000010", "101101110111101"])]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			geners:        []dataGenerator{newSelectStringGener([]string{"01", "10", "001", "110", "0001", "1110"}), newSelectStringGener([]string{"010010001000010", "101101110111101"})},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#"[nil, types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()]"#,
        r#"[newRandLenStrGener(0, 10), newRandLenStrGener(0, 20)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			childrenFieldTypes: []*types.FieldType{nil,
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()},
			geners: []dataGenerator{newRandLenStrGener(0, 10), newRandLenStrGener(0, 20)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#"[nil, types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()]"#,
        r#"[newRandLenStrGener(1, 2), newRandLenStrGener(0, 20)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			childrenFieldTypes: []*types.FieldType{nil,
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()},
			geners: []dataGenerator{newRandLenStrGener(1, 2), newRandLenStrGener(0, 20)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#"[nil, types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()]"#,
        r#"[newSelectStringGener([]string["01", "10", "001", "110", "0001", "1110"]), newSelectStringGener([]string["010010001000010", "101101110111101"])]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			childrenFieldTypes: []*types.FieldType{nil,
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()},
			geners: []dataGenerator{newSelectStringGener([]string{"01", "10", "001", "110", "0001", "1110"}), newSelectStringGener([]string{"010010001000010", "101101110111101"})},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), nil]"#,
        r#"[newRandLenStrGener(0, 10), newRandLenStrGener(0, 20)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), nil},
			geners: []dataGenerator{newRandLenStrGener(0, 10), newRandLenStrGener(0, 20)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), nil]"#,
        r#"[newRandLenStrGener(1, 2), newRandLenStrGener(0, 20)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), nil},
			geners: []dataGenerator{newRandLenStrGener(1, 2), newRandLenStrGener(0, 20)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), nil]"#,
        r#"[newSelectStringGener([]string["01", "10", "001", "110", "0001", "1110"]), newSelectStringGener([]string["010010001000010", "101101110111101"])]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), nil},
			geners: []dataGenerator{newSelectStringGener([]string{"01", "10", "001", "110", "0001", "1110"}), newSelectStringGener([]string{"010010001000010", "101101110111101"})},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()]"#,
        r#"[newRandLenStrGener(0, 10), newRandLenStrGener(0, 20)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()},
			geners: []dataGenerator{newRandLenStrGener(0, 10), newRandLenStrGener(0, 20)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()]"#,
        r#"[newRandLenStrGener(1, 2), newRandLenStrGener(0, 20)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()},
			geners: []dataGenerator{newRandLenStrGener(1, 2), newRandLenStrGener(0, 20)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()]"#,
        r#"[newSelectStringGener([]string["01", "10", "001", "110", "0001", "1110"]), newSelectStringGener([]string["010010001000010", "101101110111101"])]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()},
			geners: []dataGenerator{newSelectStringGener([]string{"01", "10", "001", "110", "0001", "1110"}), newSelectStringGener([]string{"010010001000010", "101101110111101"})},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString, types.ETInt]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeInt24).BuildP()]"#,
        r#"[newRandLenStrGener(0, 10), newRandLenStrGener(0, 20), newRangeInt64Gener(-10, 20)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETInt},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeInt24).BuildP()},
			geners: []dataGenerator{newRandLenStrGener(0, 10), newRandLenStrGener(0, 20), newRangeInt64Gener(-10, 20)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString, types.ETInt]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeInt24).BuildP()]"#,
        r#"[newSelectStringGener([]string["01", "10", "001", "110", "0001", "1110"]), newSelectStringGener([]string["010010001000010", "101101110111101"]), newRangeInt64Gener(-10, 20)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETInt,
			childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETInt},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeInt24).BuildP()},
			geners: []dataGenerator{newSelectStringGener([]string{"01", "10", "001", "110", "0001", "1110"}), newSelectStringGener([]string{"010010001000010", "101101110111101"}), newRangeInt64Gener(-10, 20)},
		}"#,
    ),
];

// ast.Hex 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Hex 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const HEX_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString]"#,
        r#""#,
        r#"[newRandHexStrGener(10, 100)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{newRandHexStrGener(10, 100)}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETInt]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETInt}}"#,
    ),
];

// ast.Unhex 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Unhex 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const UNHEX_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETString]"#,
    r#""#,
    r#"[newRandHexStrGener(10, 100)]"#,
    r#""#,
    r#""#,
    r#""#,
    r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{newRandHexStrGener(10, 100)}}"#,
)];

// ast.Trim 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Trim 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const TRIM_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString]"#,
        r#""#,
        r#"[&randSpaceStrGener[10, 100]]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{&randSpaceStrGener{10, 100}}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#"[newRandLenStrGener(10, 20), newRandLenStrGener(5, 25)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETString}, geners: []dataGenerator{newRandLenStrGener(10, 20), newRandLenStrGener(5, 25)}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString, types.ETInt]"#,
        r#""#,
        r#"[newRandLenStrGener(10, 20), newRandLenStrGener(5, 25), newRangeInt64Gener(0, 4)]"#,
        r#"[nil, nil, [Value: types.NewDatum(ast.TrimBoth), RetType: types.NewFieldType(mysql.TypeLonglong)]]"#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETInt},
			geners:        []dataGenerator{newRandLenStrGener(10, 20), newRandLenStrGener(5, 25), newRangeInt64Gener(0, 4)},
			constants:     []*Constant{nil, nil, {Value: types.NewDatum(ast.TrimBoth), RetType: types.NewFieldType(mysql.TypeLonglong)}},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString, types.ETInt]"#,
        r#""#,
        r#"[newRandLenStrGener(10, 20), newRandLenStrGener(5, 25), newRangeInt64Gener(0, 4)]"#,
        r#"[nil, nil, [Value: types.NewDatum(ast.TrimLeading), RetType: types.NewFieldType(mysql.TypeLonglong)]]"#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETInt},
			geners:        []dataGenerator{newRandLenStrGener(10, 20), newRandLenStrGener(5, 25), newRangeInt64Gener(0, 4)},
			constants:     []*Constant{nil, nil, {Value: types.NewDatum(ast.TrimLeading), RetType: types.NewFieldType(mysql.TypeLonglong)}},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString, types.ETInt]"#,
        r#""#,
        r#"[newRandLenStrGener(10, 20), newRandLenStrGener(5, 25), newRangeInt64Gener(0, 4)]"#,
        r#"[nil, nil, [Value: types.NewDatum(ast.TrimTrailing), RetType: types.NewFieldType(mysql.TypeLonglong)]]"#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETInt},
			geners:        []dataGenerator{newRandLenStrGener(10, 20), newRandLenStrGener(5, 25), newRangeInt64Gener(0, 4)},
			constants:     []*Constant{nil, nil, {Value: types.NewDatum(ast.TrimTrailing), RetType: types.NewFieldType(mysql.TypeLonglong)}},
		}"#,
    ),
];

// ast.Translate 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Translate 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const TRANSLATE_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETString, types.ETString, types.ETString]"#,
    r#""#,
    r#"[newRandLenStrGener(10, 20), newRandLenStrGener(5, 25), newRandLenStrGener(5, 25)]"#,
    r#""#,
    r#""#,
    r#""#,
    r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETString},
			geners:        []dataGenerator{newRandLenStrGener(10, 20), newRandLenStrGener(5, 25), newRandLenStrGener(5, 25)},
		}"#,
)];

// ast.LTrim 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.LTrim 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const LTRIM_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETString]"#,
    r#""#,
    r#"[&randSpaceStrGener[10, 100]]"#,
    r#""#,
    r#""#,
    r#""#,
    r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{&randSpaceStrGener{10, 100}}}"#,
)];

// ast.RTrim 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.RTrim 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const RTRIM_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETString]"#,
    r#""#,
    r#"[&randSpaceStrGener[10, 100]]"#,
    r#""#,
    r#""#,
    r#""#,
    r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{&randSpaceStrGener{10, 100}}}"#,
)];

// ast.Lpad 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Lpad 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const LPAD_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt, types.ETString]"#,
        r#""#,
        r#"[newRandLenStrGener(0, 20), newRangeInt64Gener(168435456, 368435456), newRandLenStrGener(0, 10)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETString},
			geners:        []dataGenerator{newRandLenStrGener(0, 20), newRangeInt64Gener(168435456, 368435456), newRandLenStrGener(0, 10)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt, types.ETString]"#,
        r#""#,
        r#"[newDefaultGener(0.2, types.ETString), newDefaultGener(0.2, types.ETInt), newDefaultGener(0.2, types.ETString)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETString},
			geners:        []dataGenerator{newDefaultGener(0.2, types.ETString), newDefaultGener(0.2, types.ETInt), newDefaultGener(0.2, types.ETString)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()]"#,
        r#"[newRandLenStrGener(0, 20), newRangeInt64Gener(168435456, 368435456), newRandLenStrGener(0, 10)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()},
			geners: []dataGenerator{newRandLenStrGener(0, 20), newRangeInt64Gener(168435456, 368435456), newRandLenStrGener(0, 10)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()]"#,
        r#"[newDefaultGener(0.2, types.ETString), newDefaultGener(0.2, types.ETInt), newDefaultGener(0.2, types.ETString)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()},
			geners: []dataGenerator{newDefaultGener(0.2, types.ETString), newDefaultGener(0.2, types.ETInt), newDefaultGener(0.2, types.ETString)},
		}"#,
    ),
];

// ast.Rpad 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Rpad 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const RPAD_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt, types.ETString]"#,
        r#""#,
        r#"[newRandLenStrGener(0, 20), newRangeInt64Gener(168435456, 368435456), newRandLenStrGener(0, 10)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETString},
			geners:        []dataGenerator{newRandLenStrGener(0, 20), newRangeInt64Gener(168435456, 368435456), newRandLenStrGener(0, 10)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt, types.ETString]"#,
        r#""#,
        r#"[newDefaultGener(0.2, types.ETString), newDefaultGener(0.2, types.ETInt), newDefaultGener(0.2, types.ETString)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETString},
			geners:        []dataGenerator{newDefaultGener(0.2, types.ETString), newDefaultGener(0.2, types.ETInt), newDefaultGener(0.2, types.ETString)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()]"#,
        r#"[newRandLenStrGener(0, 20), newRangeInt64Gener(168435456, 368435456), newRandLenStrGener(0, 10)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()},
			geners: []dataGenerator{newRandLenStrGener(0, 20), newRangeInt64Gener(168435456, 368435456), newRandLenStrGener(0, 10)},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), ]"#,
        r#"[newDefaultGener(0.2, types.ETString), newDefaultGener(0.2, types.ETInt), newDefaultGener(0.2, types.ETString)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
			},
			geners: []dataGenerator{newDefaultGener(0.2, types.ETString), newDefaultGener(0.2, types.ETInt), newDefaultGener(0.2, types.ETString)},
		}"#,
    ),
];

// ast.CharLength 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.CharLength 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const CHAR_LENGTH_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
			},
		}"#,
    ),
];

// ast.BitLength 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.BitLength 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const BIT_LENGTH_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETInt"#,
    r#"[types.ETString]"#,
    r#""#,
    r#""#,
    r#""#,
    r#""#,
    r#""#,
    r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString}}"#,
)];

// ast.CharFunc 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.CharFunc 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const CHAR_FUNC_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETInt, types.ETInt, types.ETInt, types.ETString]"#,
        r#""#,
        r#"[&charInt64Gener[], &charInt64Gener[], &charInt64Gener[], nil]"#,
        r#"[nil, nil, nil, [Value: types.NewDatum("ascii"), RetType: types.NewFieldType(mysql.TypeString)]]"#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETInt, types.ETInt, types.ETInt, types.ETString},
			geners:        []dataGenerator{&charInt64Gener{}, &charInt64Gener{}, &charInt64Gener{}, nil},
			constants:     []*Constant{nil, nil, nil, {Value: types.NewDatum("ascii"), RetType: types.NewFieldType(mysql.TypeString)}},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETInt, types.ETInt, types.ETInt, types.ETString]"#,
        r#""#,
        r#"[&charInt64Gener[], nil, &charInt64Gener[], nil]"#,
        r#"[nil, nil, nil, [Value: types.NewDatum("ascii"), RetType: types.NewFieldType(mysql.TypeString)]]"#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETInt, types.ETInt, types.ETInt, types.ETString},
			geners:        []dataGenerator{&charInt64Gener{}, nil, &charInt64Gener{}, nil},
			constants:     []*Constant{nil, nil, nil, {Value: types.NewDatum("ascii"), RetType: types.NewFieldType(mysql.TypeString)}},
		}"#,
    ),
];

// ast.FindInSet 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.FindInSet 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const FIND_IN_SET_VECBUILTINSTRINGCASES_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETString}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#"[&constStrGener["case"], &constStrGener["test,case"]]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETString}, geners: []dataGenerator{&constStrGener{"case"}, &constStrGener{"test,case"}}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#"[&constStrGener[""], &constStrGener["test,case"]]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETString}, geners: []dataGenerator{&constStrGener{""}, &constStrGener{"test,case"}}}"#,
    ),
];

// VECBUILTINSTRINGCASES2 对应 Go 的向量化 case map；分组顺序和每组 case 顺序与源文件一致。
/// VECBUILTINSTRINGCASES2 对应 Go 的向量化 case map；分组顺序和每组 case 顺序与源文件一致。
pub const VECBUILTINSTRINGCASES2: &[VecCaseGroupDraft] = &[
    VecCaseGroupDraft {
        builtin: r#"ast.MakeSet"#,
        cases: &MAKE_SET_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Oct"#,
        cases: &OCT_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Quote"#,
        cases: &QUOTE_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Ord"#,
        cases: &ORD_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Bin"#,
        cases: &BIN_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.ToBase64"#,
        cases: &TO_BASE64_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.FromBase64"#,
        cases: &FROM_BASE64_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.ExportSet"#,
        cases: &EXPORT_SET_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Repeat"#,
        cases: &REPEAT_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Lower"#,
        cases: &LOWER_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.IsNull"#,
        cases: &IS_NULL_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Upper"#,
        cases: &UPPER_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Right"#,
        cases: &RIGHT_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Left"#,
        cases: &LEFT_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Space"#,
        cases: &SPACE_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Reverse"#,
        cases: &REVERSE_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Instr"#,
        cases: &INSTR_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Replace"#,
        cases: &REPLACE_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.InsertFunc"#,
        cases: &INSERT_FUNC_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Elt"#,
        cases: &ELT_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.FromUnixTime"#,
        cases: &FROM_UNIX_TIME_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Strcmp"#,
        cases: &STRCMP_VECBUILTINSTRINGCASES2_CASES,
    },
    VecCaseGroupDraft {
        builtin: r#"ast.Format"#,
        cases: &FORMAT_VECBUILTINSTRINGCASES2_CASES,
    },
];

// ast.MakeSet 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.MakeSet 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const MAKE_SET_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETInt, types.ETString, types.ETString, types.ETString, types.ETString, types.ETString, types.ETString, types.ETString, types.ETString]"#,
    r#""#,
    r#""#,
    r#""#,
    r#""aes-128-ecb""#,
    r#""#,
    r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETInt, types.ETString, types.ETString, types.ETString, types.ETString, types.ETString, types.ETString, types.ETString, types.ETString}}"#,
)];

// ast.Oct 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Oct 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const OCT_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETInt]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""aes-128-ecb""#,
        r#""#,
        r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETInt}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString]"#,
        r#""#,
        r#"[&numStrGener[*newRangeInt64Gener(-10, 10)]]"#,
        r#""#,
        r#""aes-128-ecb""#,
        r#""#,
        r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{&numStrGener{*newRangeInt64Gener(-10, 10)}}}"#,
    ),
];

// ast.Quote 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Quote 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const QUOTE_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETString]"#,
    r#""#,
    r#""#,
    r#""#,
    r#""aes-128-ecb""#,
    r#""#,
    r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}}"#,
)];

// ast.Ord 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Ord 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const ORD_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETInt"#,
    r#"[types.ETString]"#,
    r#""#,
    r#""#,
    r#""#,
    r#""aes-128-ecb""#,
    r#""#,
    r#"{aesModes: "aes-128-ecb", retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString}}"#,
)];

// ast.Bin 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Bin 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const BIN_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETInt]"#,
    r#""#,
    r#""#,
    r#""#,
    r#""aes-128-ecb""#,
    r#""#,
    r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETInt}}"#,
)];

// ast.ToBase64 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.ToBase64 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const TO_BASE64_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETString]"#,
    r#""#,
    r#"[newRandLenStrGener(0, 10)]"#,
    r#""#,
    r#""aes-128-ecb""#,
    r#""#,
    r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{newRandLenStrGener(0, 10)}}"#,
)];

// ast.FromBase64 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.FromBase64 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const FROM_BASE64_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETString]"#,
    r#""#,
    r#"[newRandLenStrGener(10, 100)]"#,
    r#""#,
    r#""aes-128-ecb""#,
    r#""#,
    r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{newRandLenStrGener(10, 100)}}"#,
)];

// ast.ExportSet 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.ExportSet 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const EXPORT_SET_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETInt, types.ETString, types.ETString]"#,
        r#""#,
        r#"[newRangeInt64Gener(10, 100), &constStrGener["Y"], &constStrGener["N"]]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETInt, types.ETString, types.ETString},
			geners:        []dataGenerator{newRangeInt64Gener(10, 100), &constStrGener{"Y"}, &constStrGener{"N"}},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETInt, types.ETString, types.ETString, types.ETString]"#,
        r#""#,
        r#"[newRangeInt64Gener(10, 100), &constStrGener["Y"], &constStrGener["N"], &constStrGener[","]]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETInt, types.ETString, types.ETString, types.ETString},
			geners:        []dataGenerator{newRangeInt64Gener(10, 100), &constStrGener{"Y"}, &constStrGener{"N"}, &constStrGener{","}},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETInt, types.ETString, types.ETString, types.ETString, types.ETInt]"#,
        r#""#,
        r#"[newRangeInt64Gener(10, 100), &constStrGener["Y"], &constStrGener["N"], &constStrGener[","], newRangeInt64Gener(-10, 70)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{
			retEvalType:   types.ETString,
			childrenTypes: []types.EvalType{types.ETInt, types.ETString, types.ETString, types.ETString, types.ETInt},
			geners:        []dataGenerator{newRangeInt64Gener(10, 100), &constStrGener{"Y"}, &constStrGener{"N"}, &constStrGener{","}, newRangeInt64Gener(-10, 70)},
		}"#,
    ),
];

// ast.Repeat 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Repeat 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const REPEAT_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETString, types.ETInt]"#,
    r#""#,
    r#"[newRandLenStrGener(10, 20), newRangeInt64Gener(-10, 10)]"#,
    r#""#,
    r#""aes-128-ecb""#,
    r#""#,
    r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETInt}, geners: []dataGenerator{newRandLenStrGener(10, 20), newRangeInt64Gener(-10, 10)}}"#,
)];

// ast.Lower 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Lower 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const LOWER_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""aes-128-ecb""#,
        r#""#,
        r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString]"#,
        r#""#,
        r#"[newSelectStringGener([]string["one week’s time TEST", "one week's time TEST", "ABC测试DEF", "ABCテストABC"])]"#,
        r#""#,
        r#""aes-128-ecb""#,
        r#""#,
        r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{newSelectStringGener([]string{"one week’s time TEST", "one week's time TEST", "ABC测试DEF", "ABCテストABC"})}}"#,
    ),
];

// ast.IsNull 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.IsNull 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const IS_NULL_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString]"#,
        r#""#,
        r#"[newRandLenStrGener(10, 20)]"#,
        r#""#,
        r#""aes-128-ecb""#,
        r#""#,
        r#"{aesModes: "aes-128-ecb", retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{newRandLenStrGener(10, 20)}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString]"#,
        r#""#,
        r#"[newDefaultGener(0.2, types.ETString)]"#,
        r#""#,
        r#""aes-128-ecb""#,
        r#""#,
        r#"{aesModes: "aes-128-ecb", retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{newDefaultGener(0.2, types.ETString)}}"#,
    ),
];

// ast.Upper 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Upper 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const UPPER_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""aes-128-ecb""#,
        r#""#,
        r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString]"#,
        r#""#,
        r#"[newSelectStringGener([]string["one week’s time TEST", "one week's time TEST", "abc测试DeF", "AbCテストAbC"])]"#,
        r#""#,
        r#""aes-128-ecb""#,
        r#""#,
        r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{newSelectStringGener([]string{"one week’s time TEST", "one week's time TEST", "abc测试DeF", "AbCテストAbC"})}}"#,
    ),
];

// ast.Right 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Right 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const RIGHT_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""aes-128-ecb""#,
        r#""#,
        r#"{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETInt}}"#,
    ),
    vec_case(
        r#"need to add BinaryFlag for the Binary func"#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).BuildP()]"#,
        r#"[ newRandLenStrGener(10, 20), newRangeInt64Gener(-10, 20), ]"#,
        r#""#,
        r#""aes-128-ecb""#,
        r#""#,
        r#"// need to add BinaryFlag for the Binary func
		{aesModes: "aes-128-ecb", retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETInt},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).BuildP()},
			geners: []dataGenerator{
				newRandLenStrGener(10, 20),
				newRangeInt64Gener(-10, 20),
			},
		}"#,
    ),
];

// ast.Left 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Left 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const LEFT_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETInt}}"#,
    ),
    vec_case(
        r#"need to add BinaryFlag for the Binary func"#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).BuildP()]"#,
        r#"[ newRandLenStrGener(10, 20), newRangeInt64Gener(-10, 20), ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"// need to add BinaryFlag for the Binary func
		{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETInt},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).BuildP()},
			geners: []dataGenerator{
				newRandLenStrGener(10, 20),
				newRangeInt64Gener(-10, 20),
			},
		}"#,
    ),
];

// ast.Space 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Space 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const SPACE_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETInt]"#,
        r#""#,
        r#"[newRangeInt64Gener(-10, 2000)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETInt}, geners: []dataGenerator{newRangeInt64Gener(-10, 2000)}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETInt]"#,
        r#""#,
        r#"[newRangeInt64Gener(5, 10)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETInt}, geners: []dataGenerator{newRangeInt64Gener(5, 10)}}"#,
    ),
];

// ast.Reverse 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Reverse 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const REVERSE_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString]"#,
        r#""#,
        r#"[newRandLenStrGener(10, 20)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{newRandLenStrGener(10, 20)}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString]"#,
        r#""#,
        r#"[newDefaultGener(0.2, types.ETString)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{newDefaultGener(0.2, types.ETString)}}"#,
    ),
    vec_case(
        r#"need to add BinaryFlag for the Binary func"#,
        r#"types.ETString"#,
        r#"[types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"// need to add BinaryFlag for the Binary func
		{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()},
		}"#,
    ),
];

// ast.Instr 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Instr 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const INSTR_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETString}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#"[&constStrGener["test,case"], &constStrGener["case"]]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETString}, geners: []dataGenerator{&constStrGener{"test,case"}, &constStrGener{"case"}}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#"[&constStrGener["test,case"], &constStrGener["testcase"]]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETString}, geners: []dataGenerator{&constStrGener{"test,case"}, &constStrGener{"testcase"}}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
			},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), ]"#,
        r#"[&constStrGener["test,case"], &constStrGener["case"]]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
			},
			geners: []dataGenerator{&constStrGener{"test,case"}, &constStrGener{"case"}},
		}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), ]"#,
        r#"[&constStrGener["test,case"], &constStrGener[""]]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
			},
			geners: []dataGenerator{&constStrGener{"test,case"}, &constStrGener{""}},
		}"#,
    ),
];

// ast.Replace 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Replace 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const REPLACE_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETString, types.ETString, types.ETString]"#,
    r#""#,
    r#"[newRandLenStrGener(10, 20), newRandLenStrGener(0, 10), newRandLenStrGener(0, 10)]"#,
    r#""#,
    r#""#,
    r#""#,
    r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETString}, geners: []dataGenerator{newRandLenStrGener(10, 20), newRandLenStrGener(0, 10), newRandLenStrGener(0, 10)}}"#,
)];

// ast.InsertFunc 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.InsertFunc 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const INSERT_FUNC_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt, types.ETInt, types.ETString]"#,
        r#""#,
        r#"[newRandLenStrGener(10, 20), newRangeInt64Gener(-10, 20), newRangeInt64Gener(0, 100), newRandLenStrGener(0, 10)]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETInt, types.ETString}, geners: []dataGenerator{newRandLenStrGener(10, 20), newRangeInt64Gener(-10, 20), newRangeInt64Gener(0, 100), newRandLenStrGener(0, 10)}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETInt, types.ETInt, types.ETString]"#,
        r#"[ types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).BuildP(), types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(), ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETInt, types.ETString},
			childrenFieldTypes: []*types.FieldType{
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).BuildP(),
				types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
			},
		}"#,
    ),
];

// ast.Elt 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Elt 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const ELT_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETInt, types.ETString, types.ETString, types.ETString]"#,
    r#""#,
    r#"[newRangeInt64Gener(-1, 5)]"#,
    r#""#,
    r#""#,
    r#""#,
    r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETInt, types.ETString, types.ETString, types.ETString}, geners: []dataGenerator{newRangeInt64Gener(-1, 5)}}"#,
)];

// ast.FromUnixTime 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.FromUnixTime 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const FROM_UNIX_TIME_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[vec_case(
    r#""#,
    r#"types.ETString"#,
    r#"[types.ETDecimal, types.ETString]"#,
    r#""#,
    r#"[ gener[*newDefaultGener(0.9, types.ETDecimal)], &constStrGener["%y-%m-%d"], ]"#,
    r#""#,
    r#""#,
    r#""#,
    r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETDecimal, types.ETString},
			geners: []dataGenerator{
				gener{*newDefaultGener(0.9, types.ETDecimal)},
				&constStrGener{"%y-%m-%d"},
			},
		}"#,
)];

// ast.Strcmp 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Strcmp 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const STRCMP_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETString}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#"[ newSelectStringGener( []string[ "test", ], ), newSelectStringGener( []string[ "test", ], ), ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETString}, geners: []dataGenerator{
			newSelectStringGener(
				[]string{
					"test",
				},
			),
			newSelectStringGener(
				[]string{
					"test",
				},
			),
		}}"#,
    ),
];

// ast.Format 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Format 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
pub const FORMAT_VECBUILTINSTRINGCASES2_CASES: &[VecExprBenchCaseDraft] = &[
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETDecimal, types.ETInt]"#,
        r#""#,
        r#"[ newRangeDecimalGener(-10000, 10000, 0), newRangeInt64Gener(-10, 40), ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETDecimal, types.ETInt}, geners: []dataGenerator{
			newRangeDecimalGener(-10000, 10000, 0),
			newRangeInt64Gener(-10, 40),
		}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETReal, types.ETInt]"#,
        r#""#,
        r#"[ newRangeRealGener(-10000, 10000, 0), newRangeInt64Gener(-10, 40), ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETReal, types.ETInt}, geners: []dataGenerator{
			newRangeRealGener(-10000, 10000, 0),
			newRangeInt64Gener(-10, 40),
		}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETDecimal, types.ETInt]"#,
        r#""#,
        r#"[ newRangeDecimalGener(-10000, 10000, 1), newRangeInt64Gener(-10, 40), ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETDecimal, types.ETInt}, geners: []dataGenerator{
			newRangeDecimalGener(-10000, 10000, 1),
			newRangeInt64Gener(-10, 40),
		}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETReal, types.ETInt]"#,
        r#""#,
        r#"[ newRangeRealGener(-10000, 10000, 1), newRangeInt64Gener(-10, 40), ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETReal, types.ETInt}, geners: []dataGenerator{
			newRangeRealGener(-10000, 10000, 1),
			newRangeInt64Gener(-10, 40),
		}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETString, types.ETString]"#,
        r#""#,
        r#"[ newRealStringGener(), &numStrGener[*newRangeInt64Gener(-10, 40)], ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETString, types.ETString}, geners: []dataGenerator{
			newRealStringGener(),
			&numStrGener{*newRangeInt64Gener(-10, 40)},
		}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETDecimal, types.ETInt, types.ETString]"#,
        r#""#,
        r#"[ newRangeDecimalGener(-10000, 10000, 0.5), newRangeInt64Gener(-10, 40), newNullWrappedGener(0.1, &constStrGener["en_US"]), ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETDecimal, types.ETInt, types.ETString}, geners: []dataGenerator{
			newRangeDecimalGener(-10000, 10000, 0.5),
			newRangeInt64Gener(-10, 40),
			newNullWrappedGener(0.1, &constStrGener{"en_US"}),
		}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETString"#,
        r#"[types.ETReal, types.ETInt, types.ETString]"#,
        r#""#,
        r#"[ newRangeRealGener(-10000, 10000, 0.5), newRangeInt64Gener(-10, 40), newNullWrappedGener(0.1, &constStrGener["en_US"]), ]"#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETReal, types.ETInt, types.ETString}, geners: []dataGenerator{
			newRangeRealGener(-10000, 10000, 0.5),
			newRangeInt64Gener(-10, 40),
			newNullWrappedGener(0.1, &constStrGener{"en_US"}),
		}}"#,
    ),
];

// randSpaceStrGener 对应 Go type 声明；字段和嵌入关系按源码顺序保留为注释。
/// randSpaceStrGener 对应 Go type 声明；字段和嵌入关系按源码顺序保留为注释。
pub struct rand_space_str_generDraft;
// Go: type randSpaceStrGener struct {
// Go: lenBegin int
// Go: lenEnd int
// Go: }

// gen 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
/// gen 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
pub fn r#gen() {
    // Go 签名（源文件第 32 行）：func (g *randSpaceStrGener) gen() any {
    // Go: n := rand.Intn(g.lenEnd-g.lenBegin) + g.lenBegin
    // Go: buf := make([]byte, n)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range buf {
    // Go: x := rand.Intn(150)
    // Go: if x < 10 {
    // Go: buf[i] = byte('0' + x)
    // Go: } else if x-10 < 26 {
    // Go: buf[i] = byte('a' + x - 10)
    // Go: } else if x < 62 {
    // Go: buf[i] = byte('A' + x - 10 - 26)
    // Go: } else {
    // Go: buf[i] = byte(' ')
    // Go: }
    // Go: }
    // Go: return string(buf)
    // Go: }
}

// TestVectorizedBuiltinStringEvalOneVec 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
/// TestVectorizedBuiltinStringEvalOneVec 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn test_vectorized_builtin_string_eval_one_vec() {
    crate::builtin_string_vec_aster_unit_test::run_string_vector_parity_suite();
    // Go 签名（源文件第 567 行）：func TestVectorizedBuiltinStringEvalOneVec(t *testing.T) {
    // Go: testVectorizedEvalOneVec(t, vecBuiltinStringCases)
    // Go: }
}

// TestVectorizedBuiltinStringFunc 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
/// TestVectorizedBuiltinStringFunc 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn test_vectorized_builtin_string_func() {
    crate::builtin_string_vec_aster_unit_test::run_string_vector_parity_suite();
    // Go 签名（源文件第 571 行）：func TestVectorizedBuiltinStringFunc(t *testing.T) {
    // Go: testVectorizedBuiltinFunc(t, vecBuiltinStringCases)
    // Go: }
}

// BenchmarkVectorizedBuiltinStringEvalOneVec 对应 Go benchmark；保留 benchmark 入口和调用目标，当前不执行性能测试。
/// BenchmarkVectorizedBuiltinStringEvalOneVec 对应 Go benchmark；保留 benchmark 入口和调用目标，当前不执行性能测试。
pub fn benchmark_vectorized_builtin_string_eval_one_vec() {
    // Go 签名（源文件第 575 行）：func BenchmarkVectorizedBuiltinStringEvalOneVec(b *testing.B) {
    // Go: benchmarkVectorizedEvalOneVec(b, vecBuiltinStringCases)
    // Go: }
}

// BenchmarkVectorizedBuiltinStringFunc 对应 Go benchmark；保留 benchmark 入口和调用目标，当前不执行性能测试。
/// BenchmarkVectorizedBuiltinStringFunc 对应 Go benchmark；保留 benchmark 入口和调用目标，当前不执行性能测试。
pub fn benchmark_vectorized_builtin_string_func() {
    // Go 签名（源文件第 579 行）：func BenchmarkVectorizedBuiltinStringFunc(b *testing.B) {
    // Go: benchmarkVectorizedBuiltinFunc(b, vecBuiltinStringCases)
    // Go: }
}

// TestVectorizedBuiltinStringEvalOneVec2 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
/// TestVectorizedBuiltinStringEvalOneVec2 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn test_vectorized_builtin_string_eval_one_vec2() {
    crate::builtin_string_vec_aster_unit_test::run_string_vector_parity_suite();
    // Go 签名（源文件第 583 行）：func TestVectorizedBuiltinStringEvalOneVec2(t *testing.T) {
    // Go: testVectorizedEvalOneVec(t, vecBuiltinStringCases2)
    // Go: }
}

// TestVectorizedBuiltinStringFunc2 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
/// TestVectorizedBuiltinStringFunc2 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn test_vectorized_builtin_string_func2() {
    crate::builtin_string_vec_aster_unit_test::run_string_vector_parity_suite();
    // Go 签名（源文件第 587 行）：func TestVectorizedBuiltinStringFunc2(t *testing.T) {
    // Go: testVectorizedBuiltinFunc(t, vecBuiltinStringCases2)
    // Go: }
}

/// Go `strings.ToUpper`/`ToLower` apply simple one-rune mappings instead of
/// Rust's expanding full Unicode mappings.
#[test]
fn unicode_case_conversion_uses_go_simple_mappings() {
    use crate::string_vec::{EvalConfig, StringBuiltin, Value, eval_rows};

    let rows = [vec![Value::from("straße")], vec![Value::from("İ")]];
    assert_eq!(
        eval_rows(
            &StringBuiltin::UpperUtf8,
            &rows[..1],
            &EvalConfig::default()
        )
        .unwrap()
        .values,
        vec![Value::from("STRAßE")]
    );
    assert_eq!(
        eval_rows(
            &StringBuiltin::LowerUtf8,
            &rows[1..],
            &EvalConfig::default()
        )
        .unwrap()
        .values,
        vec![Value::from("i")]
    );
}

// BenchmarkVectorizedBuiltinStringEvalOneVec2 对应 Go benchmark；保留 benchmark 入口和调用目标，当前不执行性能测试。
/// BenchmarkVectorizedBuiltinStringEvalOneVec2 对应 Go benchmark；保留 benchmark 入口和调用目标，当前不执行性能测试。
pub fn benchmark_vectorized_builtin_string_eval_one_vec2() {
    // Go 签名（源文件第 591 行）：func BenchmarkVectorizedBuiltinStringEvalOneVec2(b *testing.B) {
    // Go: benchmarkVectorizedEvalOneVec(b, vecBuiltinStringCases2)
    // Go: }
}

// BenchmarkVectorizedBuiltinStringFunc2 对应 Go benchmark；保留 benchmark 入口和调用目标，当前不执行性能测试。
/// BenchmarkVectorizedBuiltinStringFunc2 对应 Go benchmark；保留 benchmark 入口和调用目标，当前不执行性能测试。
pub fn benchmark_vectorized_builtin_string_func2() {
    // Go 签名（源文件第 595 行）：func BenchmarkVectorizedBuiltinStringFunc2(b *testing.B) {
    // Go: benchmarkVectorizedBuiltinFunc(b, vecBuiltinStringCases2)
    // Go: }
}
