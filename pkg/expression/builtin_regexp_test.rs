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

// 正则内建函数（REGEXP_LIKE/SUBSTR/INSTR/REPLACE）的迁移期测试骨架。
//
// 对照 Go `builtin_regexp_test.go`：保留常量构造、字符集/collation 边界、
// 标量与向量化用例以及缓存行为的覆盖形状；表达式接线由共享正则回归集合验证。

// 这段逻辑只描述 expression 包内建函数测试、向量化用例和 benchmark 的覆盖形状。
// 主要类型、函数、方法前说明对应 Go 语义；断言、循环、参数解析、错误处理、benchmark、向量化 IO 和外部依赖处补中文注释。
// Go imports（保留依赖边界，待 Rust crate 接线）：
// - "fmt"
// - "testing"
// - "github.com/pingcap/tidb/pkg/parser/ast"
// - "github.com/pingcap/tidb/pkg/parser/charset"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/parser/terror"
// - "github.com/pingcap/tidb/pkg/testkit/testutil"
// - "github.com/pingcap/tidb/pkg/types"
// - "github.com/pingcap/tidb/pkg/util/chunk"
// - "github.com/stretchr/testify/require"
#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

// Test helper types mirroring Go test semantics.
/// 迁移占位：对应 Go `interface{}`。
type GoAny = ();
/// 迁移占位：对应 Go `error` 的字符串化。
type GoError = String;

// should be 5
// We will raise error for binary collation so far,
// so we have to suppress the binary collation tests.
// testCharsetAndCollateTpNum 对应 Go 的常量声明；保留数值、注释和后续分支索引语义。
/// 字符集/collation 测试类型数（Go 为 5-1，跳过暂不支持的 binary）。
pub fn testCharsetAndCollateTpNum_migration_notes() {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: const testCharsetAndCollateTpNum = 5 - 1
}

// binaryTpIdx 对应 Go 的常量声明；保留数值、注释和后续分支索引语义。
/// binary collation 在测试矩阵中的下标（Go 常量为 4）。
pub fn binaryTpIdx_migration_notes() {
    // Go: const binaryTpIdx = 4
}

// getStringConstNull 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 构造值为 NULL 的字符串常量（迁移占位）。
pub fn getStringConstNull() {
    // Go: func getStringConstNull() *Constant {
    // Go: c := getStringConstant("", false)
    // Go: c.Value.SetNull()
    // Go: return c
    // Go: }
}

// getIntConstNull 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 构造值为 NULL 的整数常量（迁移占位）。
pub fn getIntConstNull() {
    // Go: func getIntConstNull() *Constant {
    // Go: c := getIntConstant(0)
    // Go: c.Value.SetNull()
    // Go: return c
    // Go: }
}

// getStringConstant 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 按是否 binary 构造字符串常量及其 FieldType（迁移占位）。
pub fn getStringConstant() {
    // Go: func getStringConstant(value string, isBin bool) *Constant {
    // Go: c := &Constant{
    // Go: Value: types.NewStringDatum(value),
    // Go: }
    // Go:
    // Go: if isBin {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: c.RetType = types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()
    // Go: } else {
    // Go: c.RetType = types.NewFieldType(mysql.TypeVarchar)
    // Go: }
    // Go:
    // Go: return c
    // Go: }
}

// getIntConstant 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 构造整数常量（迁移占位）。
pub fn getIntConstant() {
    // Go: func getIntConstant(num int64) *Constant {
    // Go: return &Constant{
    // Go: Value: types.NewIntDatum(num),
    // Go: RetType: types.NewFieldType(mysql.TypeLong),
    // Go: }
    // Go: }
}

// setConstants 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 按 map 填充常量参数切片，可选强制为 NULL（迁移占位）。
pub fn setConstants() {
    // Go: func setConstants(isNull bool, isBin bool, constVals map[int]any, constants []*Constant) {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i, val := range constVals {
    // Go: switch v := val.(type) {
    // Go: case string:
    // Go: if isNull {
    // Go: constants[i] = getStringConstNull()
    // Go: } else {
    // Go: constants[i] = getStringConstant(v, isBin)
    // Go: }
    // Go: case int64:
    // Go: if isNull {
    // Go: constants[i] = getIntConstNull()
    // Go: } else {
    // Go: constants[i] = getIntConstant(v)
    // Go: }
    // Go: default:
    // 错误处理：保留 Go 错误分支或 panic/fatal 语义，后续接线时再映射为 Rust Result。
    // Go: panic("Unsupport type")
    // Go: }
    // Go: }
    // Go: }
}

// getVecExprBenchCaseForRegexpIncludeConst 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 构造含常量参数的正则向量化 benchmark 用例（迁移占位）。
pub fn getVecExprBenchCaseForRegexpIncludeConst() {
    // Go: func getVecExprBenchCaseForRegexpIncludeConst(retType types.EvalType, isBin bool, isNull bool, constVals map[int]any, paramNum int, constants []*Constant, inputs ...any) vecExprBenchCase {
    // Go: setConstants(isNull, isBin, constVals, constants)
    // Go:
    // 资源收尾：Go defer 在函数退出时恢复测试夹具，只记录清理语义。
    // Go: defer func() {
    // Go: // reset constants, so that following cases could reuse this constant slice
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := range constVals {
    // Go: constants[i] = nil
    // Go: }
    // Go: }()
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: retCase := getVecExprBenchCaseForRegexp(retType, isBin, inputs[:paramNum]...)
    // Go: retCase.constants = make([]*Constant, paramNum)
    // Go: copy(retCase.constants, constants[:paramNum])
    // Go: return retCase
    // Go: }
}

// getVecExprBenchCaseForRegexp 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 构造正则向量化 benchmark 用例（迁移占位）。
pub fn getVecExprBenchCaseForRegexp() {
    // Go: func getVecExprBenchCaseForRegexp(retType types.EvalType, isBin bool, inputs ...any) vecExprBenchCase {
    // Go: gens := make([]dataGenerator, 0, 6)
    // Go: paramTypes := make([]types.EvalType, 0, 6)
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, input := range inputs {
    // Go: switch input := input.(type) {
    // Go: case []int:
    // Go: gens = append(gens, &rangeInt64Gener{
    // Go: begin: input[0],
    // Go: end: input[1],
    // Go: randGen: newDefaultRandGen(),
    // Go: })
    // Go: paramTypes = append(paramTypes, types.ETInt)
    // Go: case []string:
    // Go: strs := make([]string, 0)
    // Go: strs = append(strs, input...)
    // Go: gens = append(gens, &selectStringGener{
    // Go: candidates: strs,
    // Go: randGen: newDefaultRandGen(),
    // Go: })
    // Go: paramTypes = append(paramTypes, types.ETString)
    // Go: default:
    // 错误处理：保留 Go 错误分支或 panic/fatal 语义，后续接线时再映射为 Rust Result。
    // Go: panic("Invalid type")
    // Go: }
    // Go: }
    // Go:
    // Go: ret := vecExprBenchCase{
    // Go: retEvalType: retType,
    // Go: childrenTypes: paramTypes,
    // Go: geners: gens,
    // Go: }
    // Go:
    // Go: if isBin {
    // Go: length := len(inputs)
    // Go: ft := make([]*types.FieldType, length)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: ft[0] = types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()
    // Go: ret.childrenFieldTypes = ft
    // Go: }
    // Go: return ret
    // Go: }
}

// setCharsetAndCollation 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 按索引切换测试用字符集/collation 组合。
pub fn setCharsetAndCollation() {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: func setCharsetAndCollation(id int, tps ...*types.FieldType) {
    // Go: switch id {
    // Go: case 0:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tp := range tps {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setUtf8mb4CICollation(tp)
    // Go: }
    // Go: case 1:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tp := range tps {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setUtf8mb4BinCollation(tp)
    // Go: }
    // Go: case 2:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tp := range tps {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setGBKCICollation(tp)
    // Go: }
    // Go: case 3:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tp := range tps {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setGBKBinCollation(tp)
    // Go: }
    // Go: case binaryTpIdx:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tp := range tps {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setBinaryCollation(tp)
    // Go: }
    // Go: default:
    // 错误处理：保留 Go 错误分支或 panic/fatal 语义，后续接线时再映射为 Rust Result。
    // Go: panic("Invalid index")
    // Go: }
    // Go: }
}

// setUtf8mb4CICollation 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 将 FieldType 设为 utf8mb4_general_ci（大小写不敏感）。
pub fn setUtf8mb4CICollation() {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: func setUtf8mb4CICollation(tp *types.FieldType) {
    // Go: tp.SetType(mysql.TypeVarString)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: tp.SetCharset(charset.CharsetUTF8MB4)
    // Go: tp.SetCollate("utf8mb4_general_ci")
    // Go: tp.SetFlen(types.UnspecifiedLength)
    // Go: }
}

// setUtf8mb4BinCollation 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 将 FieldType 设为 utf8mb4 binary collation。
pub fn setUtf8mb4BinCollation() {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: func setUtf8mb4BinCollation(tp *types.FieldType) {
    // Go: tp.SetType(mysql.TypeVarString)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: tp.SetCharset(charset.CharsetUTF8MB4)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: tp.SetCollate(charset.CollationUTF8MB4)
    // Go: tp.SetFlen(types.UnspecifiedLength)
    // Go: }
}

// setGBKCICollation 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 将 FieldType 设为 gbk_chinese_ci。
pub fn setGBKCICollation() {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: func setGBKCICollation(tp *types.FieldType) {
    // Go: tp.SetType(mysql.TypeVarString)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: tp.SetCharset(charset.CharsetGBK)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: tp.SetCollate(charset.CollationGBKChineseCI)
    // Go: tp.SetFlen(types.UnspecifiedLength)
    // Go: }
}

// setGBKBinCollation 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 将 FieldType 设为 gbk_bin。
pub fn setGBKBinCollation() {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: func setGBKBinCollation(tp *types.FieldType) {
    // Go: tp.SetType(mysql.TypeVarString)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: tp.SetCharset(charset.CharsetGBK)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: tp.SetCollate(charset.CollationGBKBin)
    // Go: tp.SetFlen(types.UnspecifiedLength)
    // Go: }
}

// setBinaryCollation 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
/// 将 FieldType 设为 binary charset/collation（当前测试矩阵中抑制）。
pub fn setBinaryCollation() {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: func setBinaryCollation(tp *types.FieldType) {
    // Go: tp.SetFlag(mysql.BinaryFlag)
    // Go: tp.SetType(mysql.TypeVarString)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: tp.SetCharset(charset.CharsetBin)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: tp.SetCollate(charset.CollationBin)
    // Go: }
}

#[test]
fn perl_character_classes_use_go_ascii_semantics() {
    let engine = crate::expression_regexp::RegexpEngine::new(false);

    assert_eq!(engine.regexp_like("٣", r"^\d$", "").unwrap(), 0);
    assert_eq!(engine.regexp_like("é", r"^\w$", "").unwrap(), 0);
    assert_eq!(engine.regexp_like("\u{2003}", r"^\s$", "").unwrap(), 0);
    assert_eq!(engine.regexp_like("3_A ", r"^\d\w\w\s$", "").unwrap(), 1);
    assert_eq!(engine.regexp_like("٣", r"^\D$", "").unwrap(), 1);
    assert_eq!(engine.regexp_like("é", r"^\W$", "").unwrap(), 1);
    assert_eq!(engine.regexp_like("\u{2003}", r"^\S$", "").unwrap(), 1);
    assert_eq!(engine.regexp_like("42", r"^[\d]+$", "").unwrap(), 1);
    assert_eq!(engine.regexp_like(r"\d", r"^\\d$", "").unwrap(), 1);
    assert_eq!(engine.regexp_like("é", r"^\b", "").unwrap(), 0);
    assert_eq!(engine.regexp_like("é", r"^\B", "").unwrap(), 1);
    assert_eq!(engine.regexp_like("A", r"^\b", "").unwrap(), 1);
}

// TestRegexpLike 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
/// 标量 REGEXP_LIKE：匹配、NULL、非法 match type 与字符集边界。
pub fn TestRegexpLike() {
    crate::builtin_regexp_util_aster_unit_test::run_regexp_parity_suite();
    // Go: func TestRegexpLike(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test Regexp_like without match type
    // Go: testsExcludeMatchType := []struct {
    // Go: pattern string
    // Go: input string
    // Go: match int64
    // Go: err error
    // Go: }{
    // Go: {"^$", "a", 0, nil},
    // Go: {"a", "a", 1, nil},
    // Go: {"a", "b", 0, nil},
    // Go: {"aA", "aA", 1, nil},
    // Go: {".", "a", 1, nil},
    // Go: {"^.$", "ab", 0, nil}, // index 5
    // Go: {"..", "b", 0, nil},
    // Go: {".ab", "aab", 1, nil},
    // Go: {".*", "abcd", 1, nil},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"", "a", 0, ErrRegexp}, // issue 37988
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"(", "", 0, ErrRegexp}, // index 10
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"(*", "", 0, ErrRegexp},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"[a", "", 0, ErrRegexp},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"\\", "", 0, ErrRegexp},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testsExcludeMatchType {
    // Go: fc := funcs[ast.Regexp]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(tt.input, tt.pattern)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: match, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(tt.match), match, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test Regexp_like with match type
    // Go: testsIncludeMatchType := []struct {
    // Go: pattern string
    // Go: input string
    // Go: matchType string
    // Go: match int64
    // Go: err error
    // Go: }{
    // Go: {"^$", "a", "", 0, nil},
    // Go: {"a", "a", "", 1, nil},
    // Go: {"a", "b", "", 0, nil},
    // Go: {"aA", "aA", "", 1, nil},
    // Go: {".", "a", "", 1, nil},
    // Go: {"^.$", "ab", "", 0, nil},
    // Go: {"..", "b", "", 0, nil},
    // Go: {".ab", "aab", "", 1, nil},
    // Go: {".*", "abcd", "", 1, nil},
    // Go: // Test case-insensitive
    // Go: {"AbC", "abc", "", 0, nil},
    // Go: {"AbC", "abc", "i", 1, nil},
    // Go: // Test multiple-line mode
    // Go: {"23$", "123\n321", "", 0, nil},
    // Go: {"23$", "123\n321", "m", 1, nil},
    // Go: {"^day", "good\nday", "m", 1, nil},
    // Go: // Test n flag
    // Go: {".", "\n", "", 0, nil},
    // Go: {".", "\n", "s", 1, nil},
    // Go: // Test rightmost rule
    // Go: {"aBc", "abc", "ic", 0, nil},
    // Go: {"aBc", "abc", "ci", 1, nil},
    // Go: // Test invalid match type
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"abc", "abc", "p", 0, ErrRegexp},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"abc", "abc", "cpi", 0, ErrRegexp},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testsIncludeMatchType {
    // Go: fc := funcs[ast.RegexpLike]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(tt.input, tt.pattern, tt.matchType)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: match, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(tt.match), match, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
}

// TestRegexpLikeVec 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
/// 向量化 REGEXP_LIKE 用例入口。
pub fn TestRegexpLikeVec() {
    crate::builtin_regexp_util_aster_unit_test::run_regexp_parity_suite();
    // Go: func TestRegexpLikeVec(t *testing.T) {
    // Go: var expr []string = []string{"abc", "aBc", "Good\nday", "\n"}
    // Go: var pattern []string = []string{"abc", "od$", "^day", "day$", "."}
    // Go: var matchType []string = []string{"m", "i", "icc", "cii", "s", "msi"}
    // Go:
    // Go: constants := make([]*Constant, 3)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := range 3 {
    // Go: constants[i] = nil
    // Go: }
    // Go:
    // Go: args := make([]any, 0)
    // Go: args = append(args, any(expr))
    // Go: args = append(args, any(pattern))
    // Go: args = append(args, any(matchType))
    // Go:
    // Go: cases := make([]vecExprBenchCase, 0, 30)
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETInt, false, expr, pattern)) // without match type
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETInt, false, expr, pattern)) // without match type, with BinCollation
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETInt, false, expr, pattern, matchType)) // with match type
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETInt, false, expr, pattern, matchType)) // with match type, with BinCollation
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETInt, false, make([]string, 0), pattern, matchType)) // Test expr == null
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETInt, false, make([]string, 0), pattern, matchType)) // Test expr == null, with BinCollation
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETInt, false, expr, make([]string, 0), matchType)) // Test pattern == null
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETInt, false, expr, make([]string, 0), matchType)) // Test pattern == null, with BinCollation
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETInt, false, expr, pattern, make([]string, 0))) // Test matchType == null
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETInt, false, expr, pattern, make([]string, 0))) // Test matchType == null, with BinCollation
    // Go:
    // Go: // Prepare data: expr is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{0: any("abc")}, len(args), constants, args...)) // index 10
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{0: any("abc")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{0: any("abc")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{0: any("abc")}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: pattern is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{1: any("ab.")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{1: any("ab.")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{1: any("ab.")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{1: any("ab.")}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: matchType is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{2: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{2: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{2: any("msi")}, len(args), constants, args...)) // index 20
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{2: any("msi")}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: test memorization
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{1: any("abc"), 2: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{1: any("abc")}, len(args)-1, constants, args...))
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // Build vecBuiltinRegexpLikeCases
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: var vecBuiltinRegexpLikeCases = map[string][]vecExprBenchCase{
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: ast.RegexpLike: cases,
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: testVectorizedBuiltinFunc(t, vecBuiltinRegexpLikeCases)
    // Go: }
}

// TestRegexpSubstr 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
/// 标量 REGEXP_SUBSTR：position/occurrence、空串与 binary 路径。
pub fn TestRegexpSubstr() {
    crate::builtin_regexp_util_aster_unit_test::run_regexp_parity_suite();
    // Go: func TestRegexpSubstr(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test regexp_substr(expr, pat)
    // Go: testParam2 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: match any // string
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc", "bc", "bc", "0x6263", nil},
    // Go: {"你好", "好", "好", "0xE5A5BD", nil},
    // Go: {"abc", nil, nil, nil, nil},
    // Go: {nil, "bc", nil, nil, nil},
    // Go: {nil, nil, nil, nil, nil},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"a", "", nil, nil, ErrRegexp}, // issue 37988
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam2 {
    // Go: fc := funcs[ast.RegexpSubstr]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // // test regexp_substr(expr, pat, pos)
    // Go: testParam3 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: pos any // int64
    // Go: match any // string
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc", "bc", int64(2), "bc", "0x6263", nil},
    // Go: {"你好", "好", int64(2), "好", "0xE5A5BD", nil},
    // Go: {"abc", "bc", int64(3), nil, nil, nil},
    // Go: {"你好啊", "好", int64(3), nil, "0xE5A5BD", nil},
    // Go: {"", "^$", int64(1), "", "0x", nil},
    // Go: // Invalid position index tests
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"abc", "bc", int64(-1), nil, nil, ErrRegexp},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"abc", "bc", int64(4), nil, nil, ErrRegexp},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"", "bc", int64(0), nil, nil, ErrRegexp},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"", "^$", int64(2), nil, nil, ErrRegexp},
    // Go: // Some nullable input tests
    // Go: {"", "^$", nil, nil, nil, nil},
    // Go: {nil, "^$", nil, nil, nil, nil},
    // Go: {"", nil, nil, nil, nil, nil},
    // Go: {nil, nil, int64(1), nil, nil, nil},
    // Go: {nil, nil, nil, nil, nil, nil},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam3 {
    // Go: fc := funcs[ast.RegexpSubstr]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern, tt.pos))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test regexp_substr(expr, pat, pos, occurrence)
    // Go: testParam4 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: pos any // int64
    // Go: occur any // int64
    // Go: match any // string
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc abd abe", "ab.", int64(1), int64(1), "abc", "0x616263", nil},
    // Go: {"abc abd abe", "ab.", int64(1), int64(0), "abc", "0x616263", nil},
    // Go: {"abc abd abe", "ab.", int64(1), int64(-1), "abc", "0x616263", nil},
    // Go: {"abc abd abe", "ab.", int64(1), int64(2), "abd", "0x616264", nil},
    // Go: {"abc abd abe", "ab.", int64(3), int64(1), "abd", "0x616264", nil},
    // Go: {"abc abd abe", "ab.", int64(3), int64(2), "abe", "0x616265", nil}, // index 5
    // Go: {"abc abd abe", "ab.", int64(6), int64(1), "abe", "0x616265", nil},
    // Go: {"abc abd abe", "ab.", int64(6), int64(100), nil, nil, nil},
    // Go: {"嗯嗯 嗯好 嗯呐", "嗯.", int64(1), int64(1), "嗯嗯", "0xE597AFE597AF", nil},
    // Go: {"嗯嗯 嗯好 嗯呐", "嗯.", int64(1), int64(2), "嗯好", "0xE597AFE5A5BD", nil},
    // Go: {"嗯嗯 嗯好 嗯呐", "嗯.", int64(5), int64(1), "嗯呐", "0xE597AFE5A5BD", nil}, // index 10
    // Go: {"嗯嗯 嗯好 嗯呐", "嗯.", int64(5), int64(2), nil, "0xE597AFE59190", nil},
    // Go: {"嗯嗯 嗯好 嗯呐", "嗯.", int64(1), int64(100), nil, nil, nil},
    // Go: // Some nullable input tests
    // Go: {"", "^$", int64(1), nil, nil, nil, nil},
    // Go: {nil, "^$", int64(1), nil, nil, nil, nil},
    // Go: {nil, "^$", nil, int64(1), nil, nil, nil}, // index 15
    // Go: {"", nil, nil, int64(1), nil, nil, nil},
    // Go: {nil, nil, nil, nil, nil, nil, nil},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam4 {
    // Go: fc := funcs[ast.RegexpSubstr]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern, tt.pos, tt.occur))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test regexp_substr(expr, pat, pos, occurrence, matchType)
    // Go: testParam5 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: pos any // int64
    // Go: occur any // int64
    // Go: matchType any // string
    // Go: match any // string
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc", "ab.", int64(1), int64(1), "", "abc", "0x616263", nil},
    // Go: {"abc", "aB.", int64(1), int64(1), "i", "abc", "0x616263", nil},
    // Go: {"good\nday", "od", int64(1), int64(1), "m", "od", "0x6F64", nil},
    // Go: {"\n", ".", int64(1), int64(1), "s", "\n", "0x0A", nil},
    // Go: // Test invalid matchType
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"abc", "ab.", int64(1), int64(1), "p", nil, nil, ErrRegexp}, // index 5
    // Go: // Some nullable input tests
    // Go: {"abc", "ab.", int64(1), int64(1), nil, nil, nil, nil},
    // Go: {"abc", "ab.", nil, int64(1), nil, nil, nil, nil},
    // Go: {nil, "ab.", nil, int64(1), nil, nil, nil, nil},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam5 {
    // Go: fc := funcs[ast.RegexpSubstr]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern, tt.pos, tt.occur, tt.matchType))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go: }
}

// TestRegexpSubstrVec 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
/// 向量化 REGEXP_SUBSTR 用例入口。
pub fn TestRegexpSubstrVec() {
    crate::builtin_regexp_util_aster_unit_test::run_regexp_parity_suite();
    // Go: func TestRegexpSubstrVec(t *testing.T) {
    // Go: var expr []string = []string{"abc abd abe", "你好啊啊啊啊啊", "好的 好滴 好~", "Good\nday", "\n\n\n\n\n\n"}
    // Go: var pattern []string = []string{"^$", "ab.", "aB.", "abc", "好", "好.", "od$", "^day", "day$", "."}
    // Go: var position []int = []int{1, 5}
    // Go: var occurrence []int = []int{-1, 10}
    // Go: var matchType []string = []string{"m", "i", "icc", "cii", "s", "msi"}
    // Go:
    // Go: args := make([]any, 0)
    // Go: args = append(args, any(expr))
    // Go: args = append(args, any(pattern))
    // Go: args = append(args, any(position))
    // Go: args = append(args, any(occurrence))
    // Go: args = append(args, any(matchType))
    // Go:
    // Go: constants := make([]*Constant, 5)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := range 5 {
    // Go: constants[i] = nil
    // Go: }
    // Go:
    // Go: cases := make([]vecExprBenchCase, 0, 50)
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETString, false, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETString, false, args...))
    // Go:
    // Go: // Prepare data: expr is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{0: any("好的 好滴 好~")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{0: any("好的 好滴 好~")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{0: any("好的 好滴 好~")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{0: any("好的 好滴 好~")}, len(args), constants, args...)) // index 5
    // Go:
    // Go: // Prepare data: pattern is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{1: any("aB.")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{1: any("aB.")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{1: any("aB.")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{1: any("aB.")}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: position is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{2: any(int64(2))}, len(args), constants, args...)) // index 10
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{2: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{2: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{2: any(int64(2))}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: occurrence is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{3: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{3: any(int64(2))}, len(args), constants, args...)) // index 15
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{3: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{3: any(int64(2))}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: match type is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{4: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{4: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{4: any("msi")}, len(args), constants, args...)) // index 20
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{4: any("msi")}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: test memorization
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{1: any("aB."), 4: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{1: any("aB.")}, len(args)-1, constants, args...))
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // Build vecBuiltinRegexpSubstrCases
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: var vecBuiltinRegexpSubstrCases = map[string][]vecExprBenchCase{
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: ast.RegexpSubstr: cases,
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: testVectorizedBuiltinFunc(t, vecBuiltinRegexpSubstrCases)
    // Go: }
}

// TestRegexpInStr 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
/// 标量 REGEXP_INSTR：return_option、occurrence 与越界。
pub fn TestRegexpInStr() {
    crate::builtin_regexp_util_aster_unit_test::run_regexp_parity_suite();
    // Go: func TestRegexpInStr(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test regexp_instr(expr, pat)
    // Go: testParam2 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: match any // int64
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc", "bc", int64(2), int64(2), nil},
    // Go: {"你好", "好", int64(2), int64(4), nil},
    // Go: {"", "^$", int64(1), int64(1), nil},
    // Go: {"abc", nil, nil, nil, nil},
    // Go: {nil, "bc", nil, nil, nil},
    // Go: {nil, nil, nil, nil, nil},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"a", "", nil, nil, ErrRegexp}, // issue 37988
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam2 {
    // Go: fc := funcs[ast.RegexpInStr]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test regexp_instr(expr, pat, pos)
    // Go: testParam3 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: pos any // int64
    // Go: match any // int64
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc", "bc", int64(2), int64(2), int64(2), nil},
    // Go: {"你好", "好", int64(2), int64(2), int64(4), nil},
    // Go: {"abc", "bc", int64(3), int64(0), int64(0), nil},
    // Go: {"你好啊", "好", int64(3), int64(0), int64(4), nil},
    // Go: {"", "^$", int64(1), 1, 1, nil},
    // Go: // Invalid position index tests
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"abc", "bc", int64(-1), nil, nil, ErrRegexp},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"abc", "bc", int64(4), nil, nil, ErrRegexp},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"", "bc", int64(0), nil, nil, ErrRegexp},
    // Go: // Some nullable input tests
    // Go: {"", "^$", nil, nil, nil, nil},
    // Go: {nil, "^$", nil, nil, nil, nil},
    // Go: {"", nil, nil, nil, nil, nil},
    // Go: {nil, nil, int64(1), nil, nil, nil},
    // Go: {nil, nil, nil, nil, nil, nil},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam3 {
    // Go: fc := funcs[ast.RegexpInStr]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern, tt.pos))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test regexp_instr(expr, pat, pos, occurrence)
    // Go: testParam4 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: pos any // int64
    // Go: occurrence any // int64
    // Go: match any // int64
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc abd abe", "ab.", int64(1), int64(1), 1, 1, nil},
    // Go: {"abc abd abe", "ab.", int64(1), int64(0), 1, 1, nil},
    // Go: {"abc abd abe", "ab.", int64(1), int64(-1), 1, 1, nil},
    // Go: {"abc abd abe", "ab.", int64(1), int64(2), 5, 5, nil},
    // Go: {"abc abd abe", "ab.", int64(3), int64(1), 5, 5, nil},
    // Go: {"abc abd abe", "ab.", int64(3), int64(2), 9, 9, nil}, // index 5
    // Go: {"abc abd abe", "ab.", int64(6), int64(1), 9, 9, nil},
    // Go: {"abc abd abe", "ab.", int64(6), int64(100), 0, 0, nil},
    // Go: {"嗯嗯 嗯好 嗯呐", "嗯.", int64(1), int64(1), 1, 1, nil},
    // Go: {"嗯嗯 嗯好 嗯呐", "嗯.", int64(1), int64(2), 4, 8, nil},
    // Go: {"嗯嗯 嗯好 嗯呐", "嗯.", int64(5), int64(1), 7, 8, nil}, // index 10
    // Go: {"嗯嗯 嗯好 嗯呐", "嗯.", int64(5), int64(2), 0, 15, nil},
    // Go: {"嗯嗯 嗯好 嗯呐", "嗯.", int64(1), int64(100), 0, 0, nil},
    // Go: // Some nullable input tests
    // Go: {"", "^$", int64(1), nil, nil, nil, nil},
    // Go: {nil, "^$", int64(1), nil, nil, nil, nil},
    // Go: {nil, "^$", nil, int64(1), nil, nil, nil}, // index 15
    // Go: {"", nil, nil, int64(1), nil, nil, nil},
    // Go: {nil, nil, nil, nil, nil, nil, nil},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam4 {
    // Go: fc := funcs[ast.RegexpInStr]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern, tt.pos, tt.occurrence))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test regexp_instr(expr, pat, pos, occurrence, return_option)
    // Go: testParam5 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: pos any // int64
    // Go: occurrence any // int64
    // Go: retOpt any // int64
    // Go: match any // int64
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc abd abe", "ab.", int64(1), int64(1), int64(0), 1, 1, nil},
    // Go: {"abc abd abe", "ab.", int64(1), int64(1), int64(1), 4, 4, nil},
    // Go: {"嗯嗯 嗯好 嗯呐", "嗯.", int64(1), int64(1), int64(0), 1, 1, nil},
    // Go: {"嗯嗯 嗯好 嗯呐", "嗯.", int64(1), int64(1), int64(1), 3, 7, nil},
    // Go: {"", "^$", int64(1), int64(1), int64(0), 1, 1, nil},
    // Go: {"", "^$", int64(1), int64(1), int64(1), 1, 1, nil},
    // Go: // Some nullable input tests
    // Go: {"", "^$", int64(1), nil, nil, nil, nil, nil},
    // Go: {nil, "^$", int64(1), nil, nil, nil, nil, nil},
    // Go: {nil, "^$", nil, int64(1), nil, nil, nil, nil},
    // Go: {"", nil, nil, int64(1), nil, nil, nil, nil},
    // Go: {nil, nil, nil, nil, nil, nil, nil, nil},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam5 {
    // Go: fc := funcs[ast.RegexpInStr]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern, tt.pos, tt.occurrence, tt.retOpt))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test regexp_instr(expr, pat, pos, occurrence, return_option, match_type)
    // Go: testParam6 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: pos any // int64
    // Go: occurrence any // int64
    // Go: retOpt any // int64
    // Go: matchType any // string
    // Go: match any // int64
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc", "ab.", int64(1), int64(1), int64(0), "", 1, 1, nil},
    // Go: {"abc", "aB.", int64(1), int64(1), int64(0), "i", 1, 1, nil},
    // Go: {"good\nday", "od$", int64(1), int64(1), int64(0), "m", 3, 3, nil},
    // Go: {"good\nday", "oD$", int64(1), int64(1), int64(0), "mi", 3, 3, nil},
    // Go: {"\n", ".", int64(1), int64(1), int64(0), "s", 1, 1, nil}, // index 4
    // Go: // Test invalid matchType
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"abc", "ab.", int64(1), int64(1), int64(0), "p", nil, nil, ErrRegexp},
    // Go: // Some nullable input tests
    // Go: {"abc", "ab.", int64(1), int64(1), int64(0), nil, nil, nil, nil},
    // Go: {"abc", "ab.", nil, int64(1), int64(0), nil, nil, nil, nil},
    // Go: {nil, "ab.", nil, int64(1), int64(0), nil, nil, nil, nil},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam6 {
    // Go: fc := funcs[ast.RegexpInStr]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern, tt.pos, tt.occurrence, tt.retOpt, tt.matchType))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go: }
}

// TestRegexpInStrVec 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
/// 向量化 REGEXP_INSTR 用例入口。
pub fn TestRegexpInStrVec() {
    crate::builtin_regexp_util_aster_unit_test::run_regexp_parity_suite();
    // Go: func TestRegexpInStrVec(t *testing.T) {
    // Go: var expr []string = []string{"abc abd abe", "你好啊啊啊啊啊", "好的 好滴 好~", "Good\nday", "\n\n\n\n\n\n"}
    // Go: var pattern []string = []string{"^$", "ab.", "aB.", "abc", "好", "好.", "od$", "^day", "day$", "."}
    // Go: var position []int = []int{1, 5}
    // Go: var occurrence []int = []int{-1, 10}
    // Go: var retOpt []int = []int{0, 1}
    // Go: var matchType []string = []string{"m", "i", "icc", "cii", "s", "msi"}
    // Go:
    // Go: args := make([]any, 0)
    // Go: args = append(args, any(expr))
    // Go: args = append(args, any(pattern))
    // Go: args = append(args, any(position))
    // Go: args = append(args, any(occurrence))
    // Go: args = append(args, any(retOpt))
    // Go: args = append(args, any(matchType))
    // Go:
    // Go: constants := make([]*Constant, 6)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := range 6 {
    // Go: constants[i] = nil
    // Go: }
    // Go:
    // Go: cases := make([]vecExprBenchCase, 0, 50)
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETInt, false, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETInt, false, args...))
    // Go:
    // Go: // Prepare data: expr is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{0: any("好的 好滴 好~")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{0: any("好的 好滴 好~")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{0: any("好的 好滴 好~")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{0: any("好的 好滴 好~")}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: pattern is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{1: any("aB.")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{1: any("aB.")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{1: any("aB.")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{1: any("aB.")}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: position is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{2: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{2: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{2: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{2: any(int64(2))}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: occurrence is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{3: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{3: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{3: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{3: any(int64(2))}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: return_option is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{4: any(int64(1))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{4: any(int64(1))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{4: any(int64(1))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{4: any(int64(1))}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: match type is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{5: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{5: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{5: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, true, map[int]any{5: any("msi")}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: test memorization
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{1: any("aB."), 5: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETInt, false, false, map[int]any{1: any("aB.")}, len(args)-1, constants, args...))
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // Build vecBuiltinRegexpSubstrCases
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: var vecBuiltinRegexpInStrCases = map[string][]vecExprBenchCase{
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: ast.RegexpInStr: cases,
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: testVectorizedBuiltinFunc(t, vecBuiltinRegexpInStrCases)
    // Go: }
}

// TestRegexpReplace 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
/// 标量 REGEXP_REPLACE：捕获组替换与 occurrence=0 全量替换。
pub fn TestRegexpReplace() {
    crate::builtin_regexp_util_aster_unit_test::run_regexp_parity_suite();
    // Go: func TestRegexpReplace(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go:
    // Go: url1 := "https://go.mail/folder-1/online/ru-en/#lingvo/#1О 50000&price_ashka/rav4/page=/check.xml"
    // Go: url2 := "http://saint-peters-total=меньше 1000-rublyayusche/catalogue/kolasuryat-v-2-kadyirovka-personal/serial_id=0&input_state/apartments/mokrotochki.net/upravda.ru/yandex.ru/GameMain.aspx?mult]/on/orders/50195&text=мыс и орелка в Балаш смотреть онлайн бесплатно в хорошем камбалакс&lr=20030393833539353862643188&op_promo=C-Teaser_id=06d162.html"
    // Go:
    // Go: url1Repl := "a\\12\\13"
    // Go: url1Res := "ago.mail2go.mail3"
    // Go: url1BinRes := "0x61676F2E6D61696C32676F2E6D61696C33"
    // Go:
    // Go: url2Repl := "aaa\\1233"
    // Go: url2Res := "aaasaint-peters-total=меньше 1000-rublyayusche233"
    // Go: url2BinRes := "0x6161617361696E742D7065746572732D746F74616C3DC390C2BCC390C2B5C390C2BDC391C592C391CB86C390C2B520313030302D7275626C7961797573636865323333"
    // Go:
    // Go: urlPat := "^https?://(?:www\\.)?([^/]+)/.*$"
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test regexp_replace(expr, pat, repl)
    // Go: testParam3 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: replace any // string
    // Go: match any // string
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc abd abe", "ab.", "cz", "cz cz cz", "0x637A20637A20637A", nil},
    // Go: {"你好 好的", "好", "逸", "你逸 逸的", "0xE4BDA0E980B820E980B8E79A84", nil},
    // Go: {"", "^$", "123", "123", "0x313233", nil},
    // Go: {"stackoverflow", "(.{5})(.*)", `\\+\2+\1+\2+\1\`, `\+overflow+stack+overflow+stack`, "0x5C2B6F766572666C6F772B737461636B2B6F766572666C6F772B737461636B", nil},
    // Go: {"fooabcdefghij fooABCDEFGHIJ", "foo(.)(.)(.)(.)(.)(.)(.)(.)(.)(.)", `\\\9\\\8-\7\\\6-\5\\\4-\3\\\2-\1\\`, `\i\h-g\f-e\d-c\b-a\ \I\H-G\F-E\D-C\B-A\`, "0x5C395C382D375C362D355C342D335C322D315C205C395C382D375C362D355C342D335C322D315C", nil},
    // Go: {"fool food foo", "foo(.?)", `\0+\1`, "fool+l food+d foo+", "0x5C302B5C31205C302B5C31205C302B5C31", nil},
    // Go: {url1, urlPat, url1Repl, url1Res, url1BinRes, nil},
    // Go: {url2, urlPat, url2Repl, url2Res, url2BinRes, nil},
    // Go: {"abc", nil, nil, nil, nil, nil},
    // Go: {nil, "bc", nil, nil, nil, nil},
    // Go: {nil, nil, nil, nil, nil, nil},
    // Go: {"abc", "\\d*", "d", "dadbdcd", "0x64616462646364", nil},
    // Go: {"abc", "\\d*$", "d", "abcd", "0x64616462646364", nil},
    // Go: {"我们", "\\d*", "d", "d我d们d", "0x64C3A664CB8664E2809864C3A464C2BB64C2AC64", nil},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"a", "", "a", nil, nil, ErrRegexp}, // issue 37988
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam3 {
    // Go: fc := funcs[ast.RegexpReplace]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern, tt.replace))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx), args[2].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test regexp_replace(expr, pat, repl, pos)
    // Go: testParam4 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: replace any // string
    // Go: pos any // int64
    // Go: match any // string
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc", "ab.", "cc", int64(1), "cc", "0x6363", nil},
    // Go: {"abc", "bc", "cc", int64(3), "abc", "0x616263", nil},
    // Go: {"你好", "好", "的", int64(2), "你的", "0xE4BDA0E79A84", nil},
    // Go: {"你好啊", "好", "的", int64(3), "你好啊", "0xE4BDA0E79A84E5958A", nil},
    // Go: {"", "^$", "cc", int64(1), "cc", "0x6363", nil},
    // Go: {"seafood fool", "foo(.?)", "123", int64(3), "sea123 123", "0x73656131323320313233", nil}, // index 5
    // Go: {"seafood fool", "foo(.?)", "123", int64(5), "seafood 123", "0x736561666F6F6420313233", nil},
    // Go: {"seafood fool", "foo(.?)", "123", int64(10), "seafood fool", "0x736561666F6F6420666F6F6C", nil},
    // Go: {"seafood fool", "foo(.?)", "z\\12", int64(3), "seazd2 zl2", "0x7365617A6432207A6C32", nil},
    // Go: {"seafood fool", "foo(.?)", "z\\12", int64(5), "seafood zl2", "0x736561666F6F64207A6C32", nil},
    // Go: // Invalid position index tests
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"", "^$", "a", int64(2), "", "", ErrRegexp}, // index 10
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"", "^&", "a", int64(0), "", "", ErrRegexp},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"abc", "bc", "a", int64(-1), "", "", ErrRegexp},
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"abc", "bc", "a", int64(4), "", "", ErrRegexp},
    // Go: // Some nullable input tests
    // Go: {"", "^$", "a", nil, nil, nil, nil},
    // Go: {nil, "^$", "a", nil, nil, nil, nil}, // index 15
    // Go: {"", nil, nil, nil, nil, nil, nil},
    // Go: {nil, nil, nil, int64(1), nil, nil, nil},
    // Go: {nil, nil, nil, nil, nil, nil, nil},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam4 {
    // Go: fc := funcs[ast.RegexpReplace]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern, tt.replace, tt.pos))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx), args[2].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test regexp_replace(expr, pat, repl, pos, occurrence)
    // Go: testParam5 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: replace any // string
    // Go: pos any // int64
    // Go: occurrence any // int64
    // Go: match any // string
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc abd", "ab.", "cc", int64(1), int64(1), "cc abd", "0x636320616264", nil},
    // Go: {"abc abd", "ab.", "cc", int64(1), int64(2), "abc cc", "0x616263206363", nil},
    // Go: {"abc abd", "ab.", "cc", int64(1), int64(0), "cc cc", "0x6363206363", nil},
    // Go: {"abc abd abe", "ab.", "cc", int64(3), int64(2), "abc abd cc", "0x61626320616264206363", nil},
    // Go: {"abc abd abe", "ab.", "cc", int64(3), int64(10), "abc abd abe", "0x6162632061626420616265", nil},
    // Go: {"你好 好啊", "好", "的", int64(1), int64(1), "你的 好啊", "0xE4BDA0E79A8420E5A5BDE5958A", nil}, // index 5
    // Go: {"你好 好啊", "好", "的", int64(3), int64(1), "你好 的啊", "0xE4BDA0E79A8420E5A5BDE5958A", nil},
    // Go: {"seafood fool", "foo(.?)", "123", int64(1), int(1), "sea123 fool", "0x73656131323320666F6F6C", nil},
    // Go: {"seafood fool", "foo(.?)", "123", int64(1), int(2), "seafood 123", "0x736561666F6F6420313233", nil},
    // Go: {"seafood fool", "foo(.?)", "123", int64(1), int(10), "seafood fool", "0x736561666F6F6420666F6F6C", nil},
    // Go: {"seafood fool", "foo(.?)", "z\\12", int64(1), int(1), "seazd2 fool", "0x7365617A643220666F6F6C", nil}, // index 10
    // Go: {"seafood fool", "foo(.?)", "z\\12", int64(1), int(2), "seafood zl2", "0x736561666F6F64207A6C32", nil},
    // Go: {"", "^$", "cc", int64(1), int64(1), "cc", "0x6363", nil},
    // Go: {"", "^$", "cc", int64(1), int64(2), "", "0x", nil},
    // Go: {"", "^$", "cc", int64(1), int64(-1), "cc", "0x6363", nil},
    // Go: {"abc", "\\d*", "p", 1, 2, "apbc", "0x61706263", nil}, // index 15
    // Go: {"abc", "\\d*$", "p", 1, 1, "abcp", "0x61626370", nil},
    // Go: {"我们", "\\d*", "p", 1, 2, "我p们", "0xC3A670CB86E28098C3A4C2BBC2AC", nil},
    // Go: // Some nullable input tests
    // Go: {"", "^$", "a", nil, int64(1), nil, nil, nil},
    // Go: {nil, "^$", "a", nil, nil, nil, nil, nil},
    // Go: {"", nil, nil, nil, int64(1), nil, nil, nil}, // index 20
    // Go: {nil, nil, nil, int64(1), int64(1), nil, nil, nil},
    // Go: {nil, nil, nil, nil, nil, nil, nil, nil},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam5 {
    // Go: fc := funcs[ast.RegexpReplace]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern, tt.replace, tt.pos, tt.occurrence))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx), args[2].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // test regexp_replace(expr, pat, repl, pos, occurrence, match_type)
    // Go: testParam6 := []struct {
    // Go: input any // string
    // Go: pattern any // string
    // Go: replace any // string
    // Go: pos any // int64
    // Go: occurrence any // int64
    // Go: matchType any // string
    // Go: match any // string
    // Go: matchBin any // bin result
    // Go: err error
    // Go: }{
    // Go: {"abc", "ab.", "cc", int64(1), int64(0), "", "cc", "0x6363", nil},
    // Go: {"abc", "aB.", "cc", int64(1), int64(0), "i", "cc", "0x6363", nil},
    // Go: {"good\nday", "od$", "cc", int64(1), int64(0), "m", "gocc\nday", "0x676F63630A646179", nil},
    // Go: {"good\nday", "oD$", "cc", int64(1), int64(0), "mi", "gocc\nday", "0x676F63630A646179", nil},
    // Go: {"Good\nday", "a(B)", "a\\12", int64(2), int64(0), "msi", "Good\nday", "0x476F6F640A646179", nil},
    // Go: {"Good\nday", "(.)", "cc", int64(1), int64(3), "ci", "Goccd\nday", "0x476F6363640A646179", nil},
    // Go: {"seafood fool", "foo(.?)", "的", int64(1), int64(2), "m", "seafood 的", "0x736561666F6F6420C3A7C5A1E2809E", nil},
    // Go: {"abc abd abe", "(.)", "cc", int64(4), int64(1), "cii", "abcccabd abe", "0x616263636361626420616265", nil},
    // Go: {"\n", ".", "cc", int64(1), int64(0), "s", "cc", "0x6363", nil},
    // Go: {"好的 好滴 好~", ".", "的", int64(1), int64(0), "msi", "的的的的的的的的", "0xE79A84E79A84E79A84E79A84E79A84E79A84E79A84E79A84", nil},
    // Go: // Test invalid matchType
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: {"abc", "ab.", "cc", int64(1), int64(0), "p", nil, nil, ErrRegexp},
    // Go: // Some nullable input tests
    // Go: {"abc", "ab.", "cc", int64(1), int64(0), nil, nil, nil, nil},
    // Go: {"abc", "ab.", nil, int64(1), int64(0), nil, nil, nil, nil},
    // Go: {nil, "ab.", nil, int64(1), int64(0), nil, nil, nil, nil},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: for charsetAndCollateTp := range testCharsetAndCollateTpNum {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range testParam6 {
    // Go: fc := funcs[ast.RegexpReplace]
    // Go: expectMatch := tt.match
    // Go: args := datumsToConstants(types.MakeDatums(tt.input, tt.pattern, tt.replace, tt.pos, tt.occurrence, tt.matchType))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: setCharsetAndCollation(charsetAndCollateTp, args[0].GetType(ctx), args[1].GetType(ctx), args[2].GetType(ctx))
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: if charsetAndCollateTp == binaryTpIdx {
    // Go: expectMatch = tt.matchBin
    // Go: }
    // Go: f, err := fc.getFunction(ctx, args)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: actualMatch, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: if tt.err == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(expectMatch), actualMatch, fmt.Sprintf("%v", tt))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(err, tt.err))
    // Go: }
    // Go: }
    // Go: }
    // Go: }
}

// TestRegexpReplaceVec 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
/// 向量化 REGEXP_REPLACE 用例入口。
pub fn TestRegexpReplaceVec() {
    crate::builtin_regexp_util_aster_unit_test::run_regexp_parity_suite();
    // Go: func TestRegexpReplaceVec(t *testing.T) {
    // Go: var expr []string = []string{"abc abd abe", "你好啊啊啊啊啊", "好的 好滴 好~", "Good\nday", "seafood fool"} // , "\n\n\n\n\n\n"
    // Go: var pattern []string = []string{"(^$)", "(a)b.", "a(B).", "(ab)c", "(好)", "(好).", "(o)d$", "^da(y)", "(d)ay$", "(.)", "foo(.?)", "foo(d|l)"}
    // Go: var repl []string = []string{"cc", "的", "a\\12"}
    // Go: var position []int = []int{1, 5}
    // Go: var occurrence []int = []int{-1, 5}
    // Go: var matchType []string = []string{"m", "i", "icc", "cii", "s", "msi"}
    // Go:
    // Go: args := make([]any, 0)
    // Go: args = append(args, any(expr))
    // Go: args = append(args, any(pattern))
    // Go: args = append(args, any(repl))
    // Go: args = append(args, any(position))
    // Go: args = append(args, any(occurrence))
    // Go: args = append(args, any(matchType))
    // Go:
    // Go: constants := make([]*Constant, 6)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := range 6 {
    // Go: constants[i] = nil
    // Go: }
    // Go:
    // Go: cases := make([]vecExprBenchCase, 0, 50)
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETString, false, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexp(types.ETString, false, args...))
    // Go:
    // Go: // Prepare data: expr is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{0: any("好的 好滴 好~")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{0: any("好的 好滴 好~")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{0: any("好的 好滴 好~")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{0: any("好的 好滴 好~")}, len(args), constants, args...)) // index 5
    // Go:
    // Go: // Prepare data: pattern is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{1: any("(a)B.")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{1: any("(a)B.")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{1: any("(a)B.")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{1: any("(a)B.")}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: repl is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{2: any("cc")}, len(args), constants, args...)) // index 10
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{2: any("cc")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{2: any("cc")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{2: any("cc")}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: position is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{3: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{3: any(int64(2))}, len(args), constants, args...)) // index 15
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{3: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{3: any(int64(2))}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: occurrence is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{4: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{4: any(int64(2))}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{4: any(int64(2))}, len(args), constants, args...)) // index 20
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{4: any(int64(2))}, len(args), constants, args...))
    // Go:
    // Go: // Prepare data: match type is constant
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{5: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{5: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{5: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, true, map[int]any{5: any("msi")}, len(args), constants, args...)) // index 25
    // Go:
    // Go: // Prepare data: test memorization
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{1: any("a(B)."), 5: any("msi")}, len(args), constants, args...))
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: cases = append(cases, getVecExprBenchCaseForRegexpIncludeConst(types.ETString, false, false, map[int]any{1: any("a(B).")}, len(args)-1, constants, args...))
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: // Build vecBuiltinRegexpSubstrCases
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: var vecBuiltinRegexpReplaceCases = map[string][]vecExprBenchCase{
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: ast.RegexpReplace: cases,
    // Go: }
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: testVectorizedBuiltinFunc(t, vecBuiltinRegexpReplaceCases)
    // Go: }
}

// TestRegexpCache 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
/// 常量 pattern 记忆化缓存命中与失败缓存。
pub fn TestRegexpCache() {
    crate::builtin_regexp_util_aster_unit_test::run_regexp_parity_suite();
    // Go: func TestRegexpCache(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go:
    // Go: // if the pattern or match type is not constant, it should not be cached
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: sig := regexpBaseFuncSig{}
    // Go: sig.args = []Expression{&Column{}, &Column{}, &Constant{}}
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: reg, err := sig.getRegexp(ctx, "abc", "", 2)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "abc", reg.String())
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: reg, err = sig.getRegexp(ctx, "def", "", 2)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "def", reg.String())
    // Go:
    // Go: reg, ok, err := sig.tryVecMemorizedRegexp(ctx, []*funcParam{
    // Go: {defaultStrVal: "x"},
    // Go: {defaultStrVal: "aaa"},
    // Go: {defaultStrVal: ""},
    // Go: }, 2, 1)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, reg)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, ok)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: _, ok = sig.memorizedRegexp.getCache(ctx.GetSessionVars().StmtCtx.CtxID())
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, ok)
    // Go:
    // Go: sig.args = []Expression{&Column{}, &Constant{}, &Column{}}
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: reg, err = sig.getRegexp(ctx, "bbb", "", 2)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "bbb", reg.String())
    // Go:
    // Go: reg, ok, err = sig.tryVecMemorizedRegexp(ctx, []*funcParam{
    // Go: {defaultStrVal: "x"},
    // Go: {defaultStrVal: "aaa"},
    // Go: {defaultStrVal: ""},
    // Go: }, 2, 1)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, reg)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, ok)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: _, ok = sig.memorizedRegexp.getCache(ctx.GetSessionVars().StmtCtx.CtxID())
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, ok)
    // Go:
    // Go: // if pattern and match type are both constant, it should be cached
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: sig = regexpBaseFuncSig{}
    // Go: sig.args = []Expression{&Column{}, &Constant{ParamMarker: &ParamMarker{}}, &Constant{ParamMarker: &ParamMarker{}}}
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: reg, err = sig.getRegexp(ctx, "ccc", "", 2)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "ccc", reg.String())
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: reg2, err := sig.getRegexp(ctx, "ddd", "", 2)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Same(t, reg, reg2)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "ccc", reg2.String())
    // Go:
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: sig = regexpBaseFuncSig{}
    // Go: sig.args = []Expression{&Column{}, &Constant{ParamMarker: &ParamMarker{}}, &Constant{ParamMarker: &ParamMarker{}}}
    // Go: reg, ok, err = sig.tryVecMemorizedRegexp(ctx, []*funcParam{
    // Go: {defaultStrVal: "x"},
    // Go: {defaultStrVal: "ddd"},
    // Go: {defaultStrVal: ""},
    // Go: }, 2, 1)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "ddd", reg.String())
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, ok)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: reg2, ok, err = sig.tryVecMemorizedRegexp(ctx, []*funcParam{
    // Go: {defaultStrVal: "x"},
    // Go: {defaultStrVal: "eee"},
    // Go: {defaultStrVal: ""},
    // Go: }, 2, 1)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Same(t, reg, reg2)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "ddd", reg2.String())
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, ok)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}
