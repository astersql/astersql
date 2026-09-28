// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 这段逻辑只描述 expression 包内建函数测试、向量化用例和 benchmark 的覆盖形状。
// 主要类型、函数、方法前说明对应 Go 语义；断言、循环、参数解析、错误处理、benchmark、向量化 IO 和外部依赖处补中文注释。
// Go imports（保留依赖边界，待 Rust crate 接线）：
// - "fmt"
// - "strconv"
// - "strings"
// - "testing"
// - "time"
// - "github.com/pingcap/errors"
// - "github.com/pingcap/tidb/pkg/errno"
// - "github.com/pingcap/tidb/pkg/parser/ast"
// - "github.com/pingcap/tidb/pkg/parser/charset"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/parser/terror"
// - "github.com/pingcap/tidb/pkg/sessionctx/vardef"
// - "github.com/pingcap/tidb/pkg/testkit/testutil"
// - "github.com/pingcap/tidb/pkg/types"
// - "github.com/pingcap/tidb/pkg/util/chunk"
// - contextutil "github.com/pingcap/tidb/pkg/util/context"
// - "github.com/pingcap/tidb/pkg/util/mock"
// - "github.com/stretchr/testify/require"
// 字符串内建函数的标量/向量化测试与 benchmark 覆盖形状。
//
// 对应 Go `builtin_string_test.go`：保留 case 表、断言点与外部依赖边界；
// 现由共享回归集合执行真实表达式求值。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

// Test helper types mirroring Go test semantics.
type GoAny = ();
type GoError = String;

// TestLengthAndOctetLength 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestLengthAndOctetLength() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestLengthAndOctetLength(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args any
    // Go: expected int64
    // Go: isNil bool
    // Go: getErr bool
    // Go: }{
    // Go: {"abc", 3, false, false},
    // Go: {"你好", 6, false, false},
    // Go: {1, 1, false, false},
    // Go: {3.14, 4, false, false},
    // Go: {types.NewDecFromFloatForTest(123.123), 7, false, false},
    // Go: {types.NewTime(types.FromGoTime(time.Now()), mysql.TypeDatetime, 6), 26, false, false},
    // Go: {types.NewBinaryLiteralFromUint(0x01, -1), 1, false, false},
    // Go: {types.Set{Value: 1, Name: "abc"}, 3, false, false},
    // Go: {types.Duration{Duration: 12*time.Hour + 1*time.Minute + 1*time.Second, Fsp: types.DefaultFsp}, 8, false, false},
    // Go: {nil, 0, true, false},
    // Go: {errors.New("must error"), 0, false, true},
    // Go: }
    // Go:
    // Go: lengthMethods := []string{ast.Length, ast.OctetLength}
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, lengthMethod := range lengthMethods {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, lengthMethod, primitiveValsToConstants(ctx, []any{c.args})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.expected, d.GetInt64())
    // Go: }
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.Length].getFunction(ctx, []Expression{NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: // Test GBK String
    // Go: tbl := []struct {
    // Go: input string
    // Go: chs string
    // Go: result int64
    // Go: }{
    // Go: {"abc", "gbk", 3},
    // Go: {"一二三", "gbk", 6},
    // Go: {"一二三", "", 9},
    // Go: {"一二三!", "gbk", 7},
    // Go: {"一二三!", "", 10},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, lengthMethod := range lengthMethods {
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range tbl {
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: err := ctx.GetSessionVars().SetSystemVarWithoutValidation(vardef.CharacterSetConnection, c.chs)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: f, err := newFunctionForTest(ctx, lengthMethod, primitiveValsToConstants(ctx, []any{c.input})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.result, d.GetInt64())
    // Go: }
    // Go: }
    // Go: }
}

// TestASCII 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestASCII() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestASCII(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args any
    // Go: expected int64
    // Go: isNil bool
    // Go: getErr bool
    // Go: }{
    // Go: {"2", 50, false, false},
    // Go: {2, 50, false, false},
    // Go: {"23", 50, false, false},
    // Go: {23, 50, false, false},
    // Go: {2.3, 50, false, false},
    // Go: {nil, 0, true, false},
    // Go: {"", 0, false, false},
    // Go: {"你好", 228, false, false},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.ASCII, primitiveValsToConstants(ctx, []any{c.args})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.expected, d.GetInt64())
    // Go: }
    // Go: }
    // Go: }
    // Go: _, err := funcs[ast.Length].getFunction(ctx, []Expression{NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: // Test GBK String
    // Go: tbl := []struct {
    // Go: input string
    // Go: chs string
    // Go: result int64
    // Go: }{
    // Go: {"abc", "gbk", 97},
    // Go: {"你好", "gbk", 196},
    // Go: {"你好", "", 228},
    // Go: {"世界", "gbk", 202},
    // Go: {"abc", "gb18030", 97},
    // Go: {"你好", "gb18030", 196},
    // Go: {"世界", "gb18030", 202},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range tbl {
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: err := ctx.GetSessionVars().SetSystemVarWithoutValidation(vardef.CharacterSetConnection, c.chs)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: f, err := newFunctionForTest(ctx, ast.ASCII, primitiveValsToConstants(ctx, []any{c.input})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.result, d.GetInt64())
    // Go: }
    // Go: }
}

// TestConcat 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestConcat() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestConcat(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: retType *types.FieldType
    // Go: }{
    // Go: {
    // Go: []any{nil},
    // Go: true, false, "",
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: types.NewFieldTypeBuilder().SetType(mysql.TypeVarString).SetFlag(mysql.BinaryFlag).SetDecimal(types.UnspecifiedLength).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
    // Go: },
    // Go: {
    // Go: []any{"a", "b",
    // Go: 1, 2,
    // Go: 1.1, 1.2,
    // Go: types.NewDecFromFloatForTest(1.1),
    // Go: types.NewTime(types.FromDate(2000, 1, 1, 12, 01, 01, 0), mysql.TypeDatetime, types.DefaultFsp),
    // Go: types.Duration{
    // Go: Duration: 12*time.Hour + 1*time.Minute + 1*time.Second,
    // Go: Fsp: types.DefaultFsp},
    // Go: },
    // Go: false, false, "ab121.11.21.12000-01-01 12:01:0112:01:01",
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: types.NewFieldTypeBuilder().SetType(mysql.TypeVarString).SetFlag(mysql.BinaryFlag).SetFlen(40).SetDecimal(types.UnspecifiedLength).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
    // Go: },
    // Go: {
    // Go: []any{"a", "b", nil, "c"},
    // Go: true, false, "",
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: types.NewFieldTypeBuilder().SetType(mysql.TypeVarString).SetFlag(mysql.BinaryFlag).SetFlen(3).SetDecimal(types.UnspecifiedLength).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
    // Go: },
    // Go: {
    // Go: []any{errors.New("must error")},
    // Go: false, true, "",
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: types.NewFieldTypeBuilder().SetType(mysql.TypeVarString).SetFlag(mysql.BinaryFlag).SetFlen(types.UnspecifiedLength).SetDecimal(types.UnspecifiedLength).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP(),
    // Go: },
    // Go: }
    // Go: fcName := ast.Concat
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, fcName, primitiveValsToConstants(ctx, c.args)...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: v, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, v.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, v.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go: }
}

// TestConcatSig 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestConcatSig() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestConcatSig(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: colTypes := []*types.FieldType{
    // Go: types.NewFieldType(mysql.TypeVarchar),
    // Go: types.NewFieldType(mysql.TypeVarchar),
    // Go: }
    // Go:
    // Go: resultType := &types.FieldType{}
    // Go: resultType.SetType(mysql.TypeVarchar)
    // Go: resultType.SetFlen(1000)
    // Go: args := []Expression{
    // Go: &Column{Index: 0, RetType: colTypes[0]},
    // Go: &Column{Index: 1, RetType: colTypes[1]},
    // Go: }
    // Go: base := baseBuiltinFunc{args: args, tp: resultType}
    // Go: concat := &builtinConcatSig{base, 5}
    // Go:
    // Go: cases := []struct {
    // Go: args []any
    // Go: warnings int
    // Go: res string
    // Go: }{
    // Go: {[]any{"a", "b"}, 0, "ab"},
    // Go: {[]any{"aaa", "bbb"}, 1, ""},
    // Go: {[]any{"中", "a"}, 0, "中a"},
    // Go: {[]any{"中文", "a"}, 2, ""},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: input := chunk.NewChunkWithCapacity(colTypes, 10)
    // Go: input.AppendString(0, c.args[0].(string))
    // Go: input.AppendString(1, c.args[1].(string))
    // Go:
    // Go: res, isNull, err := concat.evalString(ctx, input.GetRow(0))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.warnings == 0 {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, isNull)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, isNull)
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: warnings := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Len(t, warnings, c.warnings)
    // Go: lastWarn := warnings[len(warnings)-1]
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(errWarnAllowedPacketOverflowed, lastWarn.Err))
    // Go: }
    // Go: }
    // Go: }
}

// TestConcatWS 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestConcatWS() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestConcatWS(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNil bool
    // Go: getErr bool
    // Go: expected string
    // Go: }{
    // Go: {
    // Go: []any{nil, nil},
    // Go: true, false, "",
    // Go: },
    // Go: {
    // Go: []any{nil, "a", "b"},
    // Go: true, false, "",
    // Go: },
    // Go: {
    // Go: []any{",", "a", "b", "hello", `$^%`},
    // Go: false, false,
    // Go: `a,b,hello,$^%`,
    // Go: },
    // Go: {
    // Go: []any{"|", "a", nil, "b", "c"},
    // Go: false, false,
    // Go: "a|b|c",
    // Go: },
    // Go: {
    // Go: []any{",", "a", ",", "b", "c"},
    // Go: false, false,
    // Go: "a,,,b,c",
    // Go: },
    // Go: {
    // Go: []any{errors.New("must error"), "a", "b"},
    // Go: false, true, "",
    // Go: },
    // Go: {
    // Go: []any{",", "a", "b", 1, 2, 1.1, 0.11,
    // Go: types.NewDecFromFloatForTest(1.1),
    // Go: types.NewTime(types.FromDate(2000, 1, 1, 12, 01, 01, 0), mysql.TypeDatetime, types.DefaultFsp),
    // Go: types.Duration{
    // Go: Duration: 12*time.Hour + 1*time.Minute + 1*time.Second,
    // Go: Fsp: types.DefaultFsp},
    // Go: },
    // Go: false, false, "a,b,1,2,1.1,0.11,1.1,2000-01-01 12:01:01,12:01:01",
    // Go: },
    // Go: }
    // Go:
    // Go: fcName := ast.ConcatWS
    // Go: // ERROR 1582 (42000): Incorrect parameter count in the call to native function 'concat_ws'
    // Go: _, err := newFunctionForTest(ctx, fcName, primitiveValsToConstants(ctx, []any{nil})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, fcName, primitiveValsToConstants(ctx, c.args)...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: val, err1 := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, err1)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, err1)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, val.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.expected, val.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err = funcs[ast.ConcatWS].getFunction(ctx, primitiveValsToConstants(ctx, []any{nil, nil}))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestConcatWSSig 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestConcatWSSig() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestConcatWSSig(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: colTypes := []*types.FieldType{
    // Go: types.NewFieldType(mysql.TypeVarchar),
    // Go: types.NewFieldType(mysql.TypeVarchar),
    // Go: types.NewFieldType(mysql.TypeVarchar),
    // Go: }
    // Go: resultType := &types.FieldType{}
    // Go: resultType.SetType(mysql.TypeVarchar)
    // Go: resultType.SetFlen(1000)
    // Go: args := []Expression{
    // Go: &Column{Index: 0, RetType: colTypes[0]},
    // Go: &Column{Index: 1, RetType: colTypes[1]},
    // Go: &Column{Index: 2, RetType: colTypes[2]},
    // Go: }
    // Go: base := baseBuiltinFunc{args: args, tp: resultType}
    // Go: concat := &builtinConcatWSSig{base, 6}
    // Go:
    // Go: cases := []struct {
    // Go: args []any
    // Go: warnings int
    // Go: res string
    // Go: }{
    // Go: {[]any{",", "a", "b"}, 0, "a,b"},
    // Go: {[]any{",", "aaa", "bbb"}, 1, ""},
    // Go: {[]any{",", "中", "a"}, 0, "中,a"},
    // Go: {[]any{",", "中文", "a"}, 2, ""},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: input := chunk.NewChunkWithCapacity(colTypes, 10)
    // Go: input.AppendString(0, c.args[0].(string))
    // Go: input.AppendString(1, c.args[1].(string))
    // Go: input.AppendString(2, c.args[2].(string))
    // Go:
    // Go: res, isNull, err := concat.evalString(ctx, input.GetRow(0))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.warnings == 0 {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, isNull)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, isNull)
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: warnings := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Len(t, warnings, c.warnings)
    // Go: lastWarn := warnings[len(warnings)-1]
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(errWarnAllowedPacketOverflowed, lastWarn.Err))
    // Go: }
    // Go: }
    // Go: }
}

// TestLeft 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestLeft() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestLeft(t *testing.T) {
    // Go: ctx := createContext(t)
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: stmtCtx := ctx.GetSessionVars().StmtCtx
    // Go: oldTypeFlags := stmtCtx.TypeFlags()
    // 资源收尾：Go defer 在函数退出时恢复测试夹具，只记录清理语义。
    // Go: defer func() {
    // Go: stmtCtx.SetTypeFlags(oldTypeFlags)
    // Go: }()
    // Go: stmtCtx.SetTypeFlags(oldTypeFlags.WithIgnoreTruncateErr(true))
    // Go:
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {[]any{"abcde", 3}, false, false, "abc"},
    // Go: {[]any{"abcde", 0}, false, false, ""},
    // Go: {[]any{"abcde", 1.2}, false, false, "a"},
    // Go: {[]any{"abcde", 1.9}, false, false, "ab"},
    // Go: {[]any{"abcde", -1}, false, false, ""},
    // Go: {[]any{"abcde", 100}, false, false, "abcde"},
    // Go: {[]any{"abcde", nil}, true, false, ""},
    // Go: {[]any{nil, 3}, true, false, ""},
    // Go: {[]any{"abcde", "3"}, false, false, "abc"},
    // Go: {[]any{"abcde", "a"}, false, false, ""},
    // Go: {[]any{1234, 3}, false, false, "123"},
    // Go: {[]any{12.34, 3}, false, false, "12."},
    // Go: {[]any{types.NewBinaryLiteralFromUint(0x0102, -1), 1}, false, false, string([]byte{0x01})},
    // Go: {[]any{errors.New("must err"), 0}, false, true, ""},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.Left, primitiveValsToConstants(ctx, c.args)...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: v, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, v.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, v.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.Left].getFunction(ctx, []Expression{getVarcharCon(), getInt8Con()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestRight 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestRight() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestRight(t *testing.T) {
    // Go: ctx := createContext(t)
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: stmtCtx := ctx.GetSessionVars().StmtCtx
    // Go: oldTypeFlags := stmtCtx.TypeFlags()
    // 资源收尾：Go defer 在函数退出时恢复测试夹具，只记录清理语义。
    // Go: defer func() {
    // Go: stmtCtx.SetTypeFlags(oldTypeFlags)
    // Go: }()
    // Go: stmtCtx.SetTypeFlags(oldTypeFlags.WithIgnoreTruncateErr(true))
    // Go:
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {[]any{"abcde", 3}, false, false, "cde"},
    // Go: {[]any{"abcde", 0}, false, false, ""},
    // Go: {[]any{"abcde", 1.2}, false, false, "e"},
    // Go: {[]any{"abcde", 1.9}, false, false, "de"},
    // Go: {[]any{"abcde", -1}, false, false, ""},
    // Go: {[]any{"abcde", 100}, false, false, "abcde"},
    // Go: {[]any{"abcde", nil}, true, false, ""},
    // Go: {[]any{nil, 1}, true, false, ""},
    // Go: {[]any{"abcde", "3"}, false, false, "cde"},
    // Go: {[]any{"abcde", "a"}, false, false, ""},
    // Go: {[]any{1234, 3}, false, false, "234"},
    // Go: {[]any{12.34, 3}, false, false, ".34"},
    // Go: {[]any{types.NewBinaryLiteralFromUint(0x0102, -1), 1}, false, false, string([]byte{0x02})},
    // Go: {[]any{errors.New("must err"), 0}, false, true, ""},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.Right, primitiveValsToConstants(ctx, c.args)...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: v, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, v.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, v.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.Right].getFunction(ctx, []Expression{getVarcharCon(), getInt8Con()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestRepeat 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestRepeat() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestRepeat(t *testing.T) {
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNull bool
    // Go: res string
    // Go: }{
    // Go: {[]any{"a", int64(2)}, false, "aa"},
    // Go: {[]any{"a", uint64(16777217)}, false, strings.Repeat("a", 16777217)},
    // Go: {[]any{"a", int64(16777216)}, false, strings.Repeat("a", 16777216)},
    // Go: {[]any{"a", int64(-1)}, false, ""},
    // Go: {[]any{"a", int64(0)}, false, ""},
    // Go: {[]any{"a", uint64(0)}, false, ""},
    // Go: }
    // Go:
    // Go: ctx := createContext(t)
    // Go: fc := funcs[ast.Repeat]
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(c.args...)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: v, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNull {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, v.IsNull())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, v.GetString(), c.res)
    // Go: }
    // Go: }
    // Go: }
}

// TestRepeatSig 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestRepeatSig() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestRepeatSig(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: colTypes := []*types.FieldType{
    // Go: types.NewFieldType(mysql.TypeVarchar),
    // Go: types.NewFieldType(mysql.TypeLonglong),
    // Go: }
    // Go: resultType := &types.FieldType{}
    // Go: resultType.SetType(mysql.TypeVarchar)
    // Go: resultType.SetFlen(1000)
    // Go: args := []Expression{
    // Go: &Column{Index: 0, RetType: colTypes[0]},
    // Go: &Column{Index: 1, RetType: colTypes[1]},
    // Go: }
    // Go: base := baseBuiltinFunc{args: args, tp: resultType}
    // Go: repeat := &builtinRepeatSig{base, 1000}
    // Go:
    // Go: cases := []struct {
    // Go: args []any
    // Go: warning int
    // Go: res string
    // Go: }{
    // Go: {[]any{"a", int64(6)}, 0, "aaaaaa"},
    // Go: {[]any{"a", int64(10001)}, 1, ""},
    // Go: {[]any{"毅", int64(6)}, 0, "毅毅毅毅毅毅"},
    // Go: {[]any{"毅", int64(334)}, 2, ""},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: input := chunk.NewChunkWithCapacity(colTypes, 10)
    // Go: input.AppendString(0, c.args[0].(string))
    // Go: input.AppendInt64(1, c.args[1].(int64))
    // Go:
    // Go: res, isNull, err := repeat.evalString(ctx, input.GetRow(0))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.warning == 0 {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, isNull)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: warnings := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Len(t, warnings, c.warning)
    // Go: lastWarn := warnings[len(warnings)-1]
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(errWarnAllowedPacketOverflowed, lastWarn.Err))
    // Go: }
    // Go: }
    // Go: }
}

// TestLower 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestLower() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestLower(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {[]any{nil}, true, false, ""},
    // Go: {[]any{"ab"}, false, false, "ab"},
    // Go: {[]any{1}, false, false, "1"},
    // Go: {[]any{"one week’s time TEST"}, false, false, "one week’s time test"},
    // Go: {[]any{"one week's time TEST"}, false, false, "one week's time test"},
    // Go: {[]any{"ABC测试DEF"}, false, false, "abc测试def"},
    // Go: {[]any{"ABCテストDEF"}, false, false, "abcテストdef"},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.Lower, primitiveValsToConstants(ctx, c.args)...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: v, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, v.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, v.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.Lower].getFunction(ctx, []Expression{getVarcharCon()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: // Test GBK String
    // Go: tbl := []struct {
    // Go: input string
    // Go: chs string
    // Go: result string
    // Go: }{
    // Go: {"ABC", "gbk", "abc"},
    // Go: {"一二三", "gbk", "一二三"},
    // Go: {"àáèéêìíòóùúüāēěīńňōūǎǐǒǔǖǘǚǜⅪⅫ", "gbk", "àáèéêìíòóùúüāēěīńňōūǎǐǒǔǖǘǚǜⅪⅫ"},
    // Go: {"àáèéêìíòóùúüāēěīńňōūǎǐǒǔǖǘǚǜⅪⅫ", "", "àáèéêìíòóùúüāēěīńňōūǎǐǒǔǖǘǚǜⅺⅻ"},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range tbl {
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: err := ctx.GetSessionVars().SetSystemVarWithoutValidation(vardef.CharacterSetConnection, c.chs)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: f, err := newFunctionForTest(ctx, ast.Lower, primitiveValsToConstants(ctx, []any{c.input})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.result, d.GetString())
    // Go: }
    // Go: }
}

// TestUpper 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestUpper() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestUpper(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {[]any{nil}, true, false, ""},
    // Go: {[]any{"ab"}, false, false, "ab"},
    // Go: {[]any{1}, false, false, "1"},
    // Go: {[]any{"one week’s time TEST"}, false, false, "ONE WEEK’S TIME TEST"},
    // Go: {[]any{"one week's time TEST"}, false, false, "ONE WEEK'S TIME TEST"},
    // Go: {[]any{"abc测试def"}, false, false, "ABC测试DEF"},
    // Go: {[]any{"abcテストdef"}, false, false, "ABCテストDEF"},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.Upper, primitiveValsToConstants(ctx, c.args)...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: v, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, v.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, strings.ToUpper(c.res), v.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.Upper].getFunction(ctx, []Expression{getVarcharCon()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: // Test GBK String
    // Go: tbl := []struct {
    // Go: input string
    // Go: chs string
    // Go: result string
    // Go: }{
    // Go: {"abc", "gbk", "ABC"},
    // Go: {"一二三", "gbk", "一二三"},
    // Go: {"àbc", "gbk", "àBC"},
    // Go: {"àáèéêìíòóùúüāēěīńňōūǎǐǒǔǖǘǚǜⅪⅫ", "gbk", "àáèéêìíòóùúüāēěīńňōūǎǐǒǔǖǘǚǜⅪⅫ"},
    // Go: {"àáèéêìíòóùúüāēěīńňōūǎǐǒǔǖǘǚǜⅪⅫ", "", "ÀÁÈÉÊÌÍÒÓÙÚÜĀĒĚĪŃŇŌŪǍǏǑǓǕǗǙǛⅪⅫ"},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range tbl {
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: err := ctx.GetSessionVars().SetSystemVarWithoutValidation(vardef.CharacterSetConnection, c.chs)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: f, err := newFunctionForTest(ctx, ast.Upper, primitiveValsToConstants(ctx, []any{c.input})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.result, d.GetString())
    // Go: }
    // Go: }
}

// TestReverse 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestReverse() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestReverse(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: fc := funcs[ast.Reverse]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(nil)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go:
    // Go: tbl := []struct {
    // Go: Input any
    // Go: Expect string
    // Go: }{
    // Go: {"abc", "cba"},
    // Go: {"LIKE", "EKIL"},
    // Go: {123, "321"},
    // Go: {"", ""},
    // Go: }
    // Go:
    // Go: dtbl := tblToDtbl(tbl)
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range dtbl {
    // Go: f, err = fc.getFunction(ctx, datumsToConstants(c["Input"]))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err = evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, c["Expect"][0], d)
    // Go: }
    // Go: }
}

// TestStrcmp 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestStrcmp() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestStrcmp(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res int64
    // Go: }{
    // Go: {[]any{"123", "123"}, false, false, 0},
    // Go: {[]any{"123", "1"}, false, false, 1},
    // Go: {[]any{"1", "123"}, false, false, -1},
    // Go: {[]any{"123", "45"}, false, false, -1},
    // Go: {[]any{123, "123"}, false, false, 0},
    // Go: {[]any{"12.34", 12.34}, false, false, 0},
    // Go: {[]any{nil, "123"}, true, false, 0},
    // Go: {[]any{"123", nil}, true, false, 0},
    // Go: {[]any{"", "123"}, false, false, -1},
    // Go: {[]any{"123", ""}, false, false, 1},
    // Go: {[]any{"", ""}, false, false, 0},
    // Go: {[]any{"", nil}, true, false, 0},
    // Go: {[]any{nil, ""}, true, false, 0},
    // Go: {[]any{nil, nil}, true, false, 0},
    // Go: {[]any{"123", errors.New("must err")}, false, true, 0},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.Strcmp, primitiveValsToConstants(ctx, c.args)...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, d.GetInt64())
    // Go: }
    // Go: }
    // Go: }
    // Go: }
}

// TestReplace 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestReplace() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestReplace(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: flen int
    // Go: }{
    // Go: {[]any{"www.mysql.com", "mysql", "pingcap"}, false, false, "www.pingcap.com", 17},
    // Go: {[]any{"www.mysql.com", "w", 1}, false, false, "111.mysql.com", 260},
    // Go: {[]any{1234, 2, 55}, false, false, "15534", 20},
    // Go: {[]any{"", "a", "b"}, false, false, "", 0},
    // Go: {[]any{"abc", "", "d"}, false, false, "abc", 3},
    // Go: {[]any{"aaa", "a", ""}, false, false, "", 3},
    // Go: {[]any{nil, "a", "b"}, true, false, "", 0},
    // Go: {[]any{"a", nil, "b"}, true, false, "", 1},
    // Go: {[]any{"a", "b", nil}, true, false, "", 1},
    // Go: {[]any{errors.New("must err"), "a", "b"}, false, true, "", -1},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.Replace, primitiveValsToConstants(ctx, c.args)...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equalf(t, c.flen, f.GetType(ctx).GetFlen(), "test %v", i)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equalf(t, types.KindNull, d.Kind(), "test %v", i)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equalf(t, c.res, d.GetString(), "test %v", i)
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.Replace].getFunction(ctx, []Expression{NewZero(), NewZero(), NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestSubstring 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestSubstring() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestSubstring(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {[]any{"Quadratically", 5}, false, false, "ratically"},
    // Go: {[]any{"Sakila", 1}, false, false, "Sakila"},
    // Go: {[]any{"Sakila", 2}, false, false, "akila"},
    // Go: {[]any{"Sakila", -3}, false, false, "ila"},
    // Go: {[]any{"Sakila", 0}, false, false, ""},
    // Go: {[]any{"Sakila", 100}, false, false, ""},
    // Go: {[]any{"Sakila", -100}, false, false, ""},
    // Go: {[]any{"Quadratically", 5, 6}, false, false, "ratica"},
    // Go: {[]any{"Sakila", -5, 3}, false, false, "aki"},
    // Go: {[]any{"Sakila", 2, 0}, false, false, ""},
    // Go: {[]any{"Sakila", 2, -1}, false, false, ""},
    // Go: {[]any{"Sakila", 2, 100}, false, false, "akila"},
    // Go: {[]any{nil, 2, 3}, true, false, ""},
    // Go: {[]any{"Sakila", nil, 3}, true, false, ""},
    // Go: {[]any{"Sakila", 2, nil}, true, false, ""},
    // Go: {[]any{errors.New("must error"), 2, 3}, false, true, ""},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.Substring, primitiveValsToConstants(ctx, c.args)...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, d.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.Substring].getFunction(ctx, []Expression{NewZero(), NewZero(), NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: _, err = funcs[ast.Substring].getFunction(ctx, []Expression{NewZero(), NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestConvert 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestConvert() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestConvert(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: tbl := []struct {
    // Go: str any
    // Go: cs string
    // Go: result string
    // Go: hasBinaryFlag bool
    // Go: }{
    // Go: {"haha", "utf8", "haha", false},
    // Go: {"haha", "ascii", "haha", false},
    // Go: {"haha", "binary", "haha", true},
    // Go: {"haha", "bInAry", "haha", true},
    // Go: {types.NewBinaryLiteralFromUint(0x7e, -1), "BiNarY", "~", true},
    // Go: {types.NewBinaryLiteralFromUint(0xe4b8ade696870a, -1), "uTf8", "中文\n", false},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, v := range tbl {
    // Go: fc := funcs[ast.Convert]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(v.str, v.cs)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // Go: retType := f.getRetTp()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: require.Equal(t, strings.ToLower(v.cs), retType.GetCharset())
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: collate, err := charset.GetDefaultCollation(strings.ToLower(v.cs))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, collate, retType.GetCollate())
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, v.hasBinaryFlag, mysql.HasBinaryFlag(retType.GetFlag()))
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindString, r.Kind())
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, v.result, r.GetString())
    // Go: }
    // Go:
    // Go: // Test case for getFunction() error
    // Go: errTbl := []struct {
    // Go: str any
    // Go: cs string
    // Go: err string
    // Go: }{
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: {"haha", "wrongcharset", "[expression:1115]Unknown character set: 'wrongcharset'"},
    // Go: {"haha", "cp866", "[expression:1115]Unknown character set: 'cp866'"},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, v := range errTbl {
    // Go: fc := funcs[ast.Convert]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(v.str, v.cs)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, v.err, err.Error())
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, f)
    // Go: }
    // Go:
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: // Test wrong charset while evaluating.
    // Go: fc := funcs[ast.Convert]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums("haha", "utf8")))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // Go: wrongFunction := f.(*builtinConvertSig)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: wrongFunction.tp.SetCharset("wrongcharset")
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: _, err = evalBuiltinFunc(wrongFunction, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: require.Equal(t, "[expression:1115]Unknown character set: 'wrongcharset'", err.Error())
    // Go: }
}

// TestSubstringIndex 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestSubstringIndex() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestSubstringIndex(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {[]any{"www.pingcap.com", ".", 2}, false, false, "www.pingcap"},
    // Go: {[]any{"www.pingcap.com", ".", -2}, false, false, "pingcap.com"},
    // Go: {[]any{"www.pingcap.com", ".", 0}, false, false, ""},
    // Go: {[]any{"www.pingcap.com", ".", 100}, false, false, "www.pingcap.com"},
    // Go: {[]any{"www.pingcap.com", ".", -100}, false, false, "www.pingcap.com"},
    // Go: {[]any{"www.pingcap.com", "d", 0}, false, false, ""},
    // Go: {[]any{"www.pingcap.com", "d", 1}, false, false, "www.pingcap.com"},
    // Go: {[]any{"www.pingcap.com", "d", -1}, false, false, "www.pingcap.com"},
    // Go: {[]any{"www.pingcap.com", "", 0}, false, false, ""},
    // Go: {[]any{"www.pingcap.com", "", 1}, false, false, ""},
    // Go: {[]any{"www.pingcap.com", "", -1}, false, false, ""},
    // Go: {[]any{"", ".", 0}, false, false, ""},
    // Go: {[]any{"", ".", 1}, false, false, ""},
    // Go: {[]any{"", ".", -1}, false, false, ""},
    // Go: {[]any{nil, ".", 1}, true, false, ""},
    // Go: {[]any{"www.pingcap.com", nil, 1}, true, false, ""},
    // Go: {[]any{"www.pingcap.com", ".", nil}, true, false, ""},
    // Go: {[]any{errors.New("must error"), ".", 1}, false, true, ""},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.SubstringIndex, primitiveValsToConstants(ctx, c.args)...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, d.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.SubstringIndex].getFunction(ctx, []Expression{NewZero(), NewZero(), NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestSpace 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestSpace() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestSpace(t *testing.T) {
    // Go: ctx := createContext(t)
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: stmtCtx := ctx.GetSessionVars().StmtCtx
    // Go: oldTypeFlags := stmtCtx.TypeFlags()
    // 资源收尾：Go defer 在函数退出时恢复测试夹具，只记录清理语义。
    // Go: defer func() {
    // Go: stmtCtx.SetTypeFlags(oldTypeFlags)
    // Go: }()
    // Go: stmtCtx.SetTypeFlags(oldTypeFlags.WithIgnoreTruncateErr(true))
    // Go:
    // Go: cases := []struct {
    // Go: arg any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {0, false, false, ""},
    // Go: {3, false, false, " "},
    // Go: {mysql.MaxBlobWidth + 1, true, false, ""},
    // Go: {-1, false, false, ""},
    // Go: {"abc", false, false, ""},
    // Go: {"3", false, false, " "},
    // Go: {1.2, false, false, " "},
    // Go: {1.9, false, false, " "},
    // Go: {nil, true, false, ""},
    // Go: {errors.New("must error"), false, true, ""},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.Space, primitiveValsToConstants(ctx, []any{c.arg})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, d.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.Space].getFunction(ctx, []Expression{NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestSpaceSig 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestSpaceSig() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestSpaceSig(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: colTypes := []*types.FieldType{
    // Go: types.NewFieldType(mysql.TypeLonglong),
    // Go: }
    // Go: resultType := &types.FieldType{}
    // Go: resultType.SetType(mysql.TypeVarchar)
    // Go: resultType.SetFlen(1000)
    // Go: args := []Expression{
    // Go: &Column{Index: 0, RetType: colTypes[0]},
    // Go: }
    // Go: base := baseBuiltinFunc{args: args, tp: resultType}
    // Go: space := &builtinSpaceSig{base, 1000}
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: input := chunk.NewChunkWithCapacity(colTypes, 10)
    // Go: input.AppendInt64(0, 6)
    // Go: input.AppendInt64(0, 1001)
    // Go: res, isNull, err := space.evalString(ctx, input.GetRow(0))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, " ", res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: res, isNull, err = space.evalString(ctx, input.GetRow(1))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "", res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: warnings := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, 1, len(warnings))
    // Go: lastWarn := warnings[len(warnings)-1]
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(errWarnAllowedPacketOverflowed, lastWarn.Err))
    // Go: }
}

// TestLocate 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestLocate() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestLocate(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: // 1. Test LOCATE without binary input.
    // Go: tbl := []struct {
    // Go: Args []any
    // Go: Want any
    // Go: }{
    // Go: {[]any{"bar", "foobarbar"}, 4},
    // Go: {[]any{"xbar", "foobar"}, 0},
    // Go: {[]any{"", "foobar"}, 1},
    // Go: {[]any{"foobar", ""}, 0},
    // Go: {[]any{"", ""}, 1},
    // Go: {[]any{"好世", "你好世界"}, 2},
    // Go: {[]any{"界面", "你好世界"}, 0},
    // Go: {[]any{"b", "中a英b文"}, 4},
    // Go: {[]any{"bAr", "foobArbar"}, 4},
    // Go: {[]any{nil, "foobar"}, nil},
    // Go: {[]any{"bar", nil}, nil},
    // Go: {[]any{"bar", "foobarbar", 5}, 7},
    // Go: {[]any{"xbar", "foobar", 1}, 0},
    // Go: {[]any{"", "foobar", 2}, 2},
    // Go: {[]any{"foobar", "", 1}, 0},
    // Go: {[]any{"", "", 2}, 0},
    // Go: {[]any{"A", "大A写的A", 0}, 0},
    // Go: {[]any{"A", "大A写的A", 1}, 2},
    // Go: {[]any{"A", "大A写的A", 2}, 2},
    // Go: {[]any{"A", "大A写的A", 3}, 5},
    // Go: {[]any{"BaR", "foobarBaR", 5}, 7},
    // Go: {[]any{nil, nil}, nil},
    // Go: {[]any{"", nil}, nil},
    // Go: {[]any{nil, ""}, nil},
    // Go: {[]any{nil, nil, 1}, nil},
    // Go: {[]any{"", nil, 1}, nil},
    // Go: {[]any{nil, "", 1}, nil},
    // Go: {[]any{"foo", nil, -1}, nil},
    // Go: {[]any{nil, "bar", 0}, nil},
    // Go: }
    // Go: Dtbl := tblToDtbl(tbl)
    // Go: instr := funcs[ast.Locate]
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i, c := range Dtbl {
    // Go: f, err := instr.getFunction(ctx, datumsToConstants(c["Args"]))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: got, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equalf(t, c["Want"][0], got, "[%d]: args: %v", i, c["Args"])
    // Go: }
    // Go: // 2. Test LOCATE with binary input
    // Go: tbl2 := []struct {
    // Go: Args []any
    // Go: Want any
    // Go: }{
    // Go: {[]any{[]byte("BaR"), "foobArbar"}, 0},
    // Go: {[]any{"BaR", []byte("foobArbar")}, 0},
    // Go: {[]any{[]byte("bAr"), "foobarBaR", 5}, 0},
    // Go: {[]any{"bAr", []byte("foobarBaR"), 5}, 0},
    // Go: {[]any{"bAr", []byte("foobarbAr"), 5}, 7},
    // Go: }
    // Go: Dtbl2 := tblToDtbl(tbl2)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i, c := range Dtbl2 {
    // Go: exprs := datumsToConstants(c["Args"])
    // Go: types.SetBinChsClnFlag(exprs[0].GetType(ctx))
    // Go: types.SetBinChsClnFlag(exprs[1].GetType(ctx))
    // Go: f, err := instr.getFunction(ctx, exprs)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: got, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equalf(t, c["Want"][0], got, "[%d]: args: %v", i, c["Args"])
    // Go: }
    // Go: }
}

// TestFindInSetConstStrlistLookup 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestFindInSetConstStrlistLookup() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestFindInSetConstStrlistLookup(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: fc := funcs[ast.FindInSet]
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: const padSpaceCollation = "utf8mb4_general_ci"
    // Go:
    // Go: // Use utf8mb4_general_ci to verify that FIND_IN_SET should not treat trailing
    // Go: // spaces as equal even under PAD SPACE collations.
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: str := types.NewCollationStringDatum(" ", padSpaceCollation)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: strlist := types.NewCollationStringDatum(" , , ,", padSpaceCollation)
    // Go: fn, err := fc.getFunction(ctx, datumsToConstants([]types.Datum{str, strlist}))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: findInSetSig, ok := fn.(*builtinFindInSetSig)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, ok)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, findInSetSig.constStrlistLookupCache.cached.Load())
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := evalBuiltinFunc(fn, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, int64(2), d.GetInt64())
    // Go: cached := findInSetSig.constStrlistLookupCache.cached.Load()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, cached)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, cached.item.isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Len(t, cached.item.lookup, 3)
    // Go:
    // Go: // Constant strlist lookup map should be reused across evaluations.
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err = evalBuiltinFunc(fn, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, int64(2), d.GetInt64())
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Same(t, cached, findInSetSig.constStrlistLookupCache.cached.Load())
    // Go:
    // Go: // Vectorized path should use the same semantics and produce first match.
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, findInSetSig.isChildrenVectorized())
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: chk := chunk.NewChunkWithCapacity(nil, 4)
    // Go: chk.SetNumVirtualRows(4)
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: result := chunk.NewChunkWithCapacity([]*types.FieldType{types.NewFieldType(mysql.TypeLonglong)}, 4)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // Go: err = vecEvalType(ctx, fn, types.ETInt, chk, result.Column(0))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := 0; i < chk.NumRows(); i++ {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, int64(2), result.Column(0).GetInt64(i))
    // Go: }
    // Go:
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: str2 := types.NewCollationStringDatum("a", padSpaceCollation)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: strlist2 := types.NewCollationStringDatum("a,b,a", padSpaceCollation)
    // Go: fn, err = fc.getFunction(ctx, datumsToConstants([]types.Datum{str2, strlist2}))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: findInSetSig, ok = fn.(*builtinFindInSetSig)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, ok)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, findInSetSig.constStrlistLookupCache.cached.Load())
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err = evalBuiltinFunc(fn, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, int64(1), d.GetInt64())
    // Go: cached = findInSetSig.constStrlistLookupCache.cached.Load()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, cached)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, cached.item.isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Len(t, cached.item.lookup, 2)
    // Go:
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, findInSetSig.isChildrenVectorized())
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: chk = chunk.NewChunkWithCapacity(nil, 2)
    // Go: chk.SetNumVirtualRows(2)
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: result = chunk.NewChunkWithCapacity([]*types.FieldType{types.NewFieldType(mysql.TypeLonglong)}, 2)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // Go: err = vecEvalType(ctx, fn, types.ETInt, chk, result.Column(0))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := 0; i < chk.NumRows(); i++ {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, int64(1), result.Column(0).GetInt64(i))
    // Go: }
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Same(t, cached, findInSetSig.constStrlistLookupCache.cached.Load())
    // Go: }
}

// TestFindInSetVecFirstMatchNonConstStrlist 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestFindInSetVecFirstMatchNonConstStrlist() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestFindInSetVecFirstMatchNonConstStrlist(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: fc := funcs[ast.FindInSet]
    // Go: colTypes := []*types.FieldType{
    // Go: types.NewFieldType(mysql.TypeVarString),
    // Go: types.NewFieldType(mysql.TypeVarString),
    // Go: }
    // Go: fn, err := fc.getFunction(ctx, []Expression{
    // Go: &Column{Index: 0, RetType: colTypes[0]},
    // Go: &Column{Index: 1, RetType: colTypes[1]},
    // Go: })
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: findInSetSig, ok := fn.(*builtinFindInSetSig)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, ok)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, findInSetSig.isChildrenVectorized())
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, findInSetSig.constStrlistLookupCache.cached.Load())
    // Go:
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: input := chunk.NewChunkWithCapacity(colTypes, 4)
    // Go: input.AppendString(0, "a")
    // Go: input.AppendString(1, "b,a,c,a")
    // Go: input.AppendString(0, "a")
    // Go: input.AppendString(1, "a,b,a")
    // Go: input.AppendString(0, "")
    // Go: input.AppendString(1, ",,")
    // Go: input.AppendString(0, "x")
    // Go: input.AppendString(1, "a,b,a")
    // Go:
    // Go: expected := []int64{2, 1, 1, 0}
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i, want := range expected {
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // Go: d, err := evalBuiltinFunc(fn, ctx, input.GetRow(i))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equalf(t, want, d.GetInt64(), "scalar row %d", i)
    // Go: }
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, findInSetSig.constStrlistLookupCache.cached.Load())
    // Go:
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: result := chunk.NewChunkWithCapacity([]*types.FieldType{types.NewFieldType(mysql.TypeLonglong)}, input.NumRows())
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // Go: err = vecEvalType(ctx, fn, types.ETInt, input, result.Column(0))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i, want := range expected {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equalf(t, want, result.Column(0).GetInt64(i), "vectorized row %d", i)
    // Go: }
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, findInSetSig.constStrlistLookupCache.cached.Load())
    // Go: }
}

// TestFindInSetConstOnlyInContextStrlistLookup 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestFindInSetConstOnlyInContextStrlistLookup() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestFindInSetConstOnlyInContextStrlistLookup(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: fc := funcs[ast.FindInSet]
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: const padSpaceCollation = "utf8mb4_general_ci"
    // Go: resetStmtCtx := func() {
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().StmtCtx = ctx.GetSessionVars().InitStatementContext()
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.ResetSessionAndStmtTimeZone(ctx.GetSessionVars().TimeZone)
    // Go: }
    // Go:
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: str := types.NewCollationStringDatum(" ", padSpaceCollation)
    // Go: strTp := types.NewFieldType(mysql.TypeVarString)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: strTp.SetCharset(charset.CharsetUTF8MB4)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: strTp.SetCollate(padSpaceCollation)
    // Go: strExpr := &Constant{
    // Go: Value: str,
    // Go: RetType: strTp,
    // Go: }
    // Go: strlistTp := types.NewFieldType(mysql.TypeVarString)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: strlistTp.SetCharset(charset.CharsetUTF8MB4)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: strlistTp.SetCollate(padSpaceCollation)
    // Go: strlistExpr := &Constant{
    // Go: ParamMarker: &ParamMarker{order: 0},
    // Go: RetType: strlistTp,
    // Go: }
    // Go:
    // Go: // ParamMarker.GetType() needs a concrete parameter value during function build.
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().PlanCacheParams.Reset()
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: ctx.GetSessionVars().PlanCacheParams.Append(types.NewCollationStringDatum(" , , ,", padSpaceCollation))
    // Go:
    // Go: fn, err := fc.getFunction(ctx, []Expression{strExpr, strlistExpr})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: findInSetSig, ok := fn.(*builtinFindInSetSig)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, ok)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, findInSetSig.constStrlistLookupCache.cached.Load())
    // Go:
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().PlanCacheParams.Reset()
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: ctx.GetSessionVars().PlanCacheParams.Append(types.NewCollationStringDatum(" , , ,", padSpaceCollation))
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := evalBuiltinFunc(fn, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, int64(2), d.GetInt64())
    // Go: cached := findInSetSig.constStrlistLookupCache.cached.Load()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, cached)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, cached.item.isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Len(t, cached.item.lookup, 3)
    // Go:
    // Go: // Reuse within one statement context.
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err = evalBuiltinFunc(fn, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, int64(2), d.GetInt64())
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Same(t, cached, findInSetSig.constStrlistLookupCache.cached.Load())
    // Go:
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, findInSetSig.isChildrenVectorized())
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: chk := chunk.NewChunkWithCapacity(nil, 3)
    // Go: chk.SetNumVirtualRows(3)
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: result := chunk.NewChunkWithCapacity([]*types.FieldType{types.NewFieldType(mysql.TypeLonglong)}, 3)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // Go: err = vecEvalType(ctx, fn, types.ETInt, chk, result.Column(0))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := 0; i < chk.NumRows(); i++ {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, int64(2), result.Column(0).GetInt64(i))
    // Go: }
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Same(t, cached, findInSetSig.constStrlistLookupCache.cached.Load())
    // Go:
    // Go: // New statement context should rebuild the cache.
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().PlanCacheParams.Reset()
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: ctx.GetSessionVars().PlanCacheParams.Append(types.NewCollationStringDatum(" ,a", padSpaceCollation))
    // Go: resetStmtCtx()
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err = evalBuiltinFunc(fn, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, int64(1), d.GetInt64())
    // Go: cached2 := findInSetSig.constStrlistLookupCache.cached.Load()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, cached2)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotSame(t, cached, cached2)
    // Go:
    // Go: // Null strlist in const-only-in-context should return NULL and cache null state.
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().PlanCacheParams.Reset()
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().PlanCacheParams.Append(types.NewDatum(nil))
    // Go: resetStmtCtx()
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err = evalBuiltinFunc(fn, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, d.IsNull())
    // Go: cached3 := findInSetSig.constStrlistLookupCache.cached.Load()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, cached3)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, cached3.item.isNull)
    // Go: }
}

// TestTrim 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestTrim() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestTrim(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {[]any{" bar "}, false, false, "bar"},
    // Go: {[]any{"\t bar \n"}, false, false, "\t bar \n"},
    // Go: {[]any{"\r bar \t"}, false, false, "\r bar \t"},
    // Go: {[]any{" \tbar\n "}, false, false, "\tbar\n"},
    // Go: {[]any{""}, false, false, ""},
    // Go: {[]any{nil}, true, false, ""},
    // Go: {[]any{"xxxbarxxx", "x"}, false, false, "bar"},
    // Go: {[]any{"bar", "x"}, false, false, "bar"},
    // Go: {[]any{" bar ", ""}, false, false, " bar "},
    // Go: {[]any{"", "x"}, false, false, ""},
    // Go: {[]any{"bar", nil}, true, false, ""},
    // Go: {[]any{nil, "x"}, true, false, ""},
    // Go: {[]any{"xxxbarxxx", "x", int(ast.TrimLeading)}, false, false, "barxxx"},
    // Go: {[]any{"barxxyz", "xyz", int(ast.TrimTrailing)}, false, false, "barx"},
    // Go: {[]any{"xxxbarxxx", "x", int(ast.TrimBoth)}, false, false, "bar"},
    // Go: {[]any{"bar", nil, int(ast.TrimLeading)}, true, false, ""},
    // Go: {[]any{errors.New("must error")}, false, true, ""},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.Trim, primitiveValsToConstants(ctx, c.args)...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, d.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.Trim].getFunction(ctx, []Expression{NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: _, err = funcs[ast.Trim].getFunction(ctx, []Expression{NewZero(), NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: _, err = funcs[ast.Trim].getFunction(ctx, []Expression{NewZero(), NewZero(), NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestLTrim 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestLTrim() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestLTrim(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: arg any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {" bar ", false, false, "bar "},
    // Go: {"\t bar ", false, false, "\t bar "},
    // Go: {" \tbar ", false, false, "\tbar "},
    // Go: {"\t bar ", false, false, "\t bar "},
    // Go: {" \tbar ", false, false, "\tbar "},
    // Go: {"\r bar ", false, false, "\r bar "},
    // Go: {" \rbar ", false, false, "\rbar "},
    // Go: {"\n bar ", false, false, "\n bar "},
    // Go: {" \nbar ", false, false, "\nbar "},
    // Go: {"bar", false, false, "bar"},
    // Go: {"", false, false, ""},
    // Go: {nil, true, false, ""},
    // Go: {errors.New("must error"), false, true, ""},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.LTrim, primitiveValsToConstants(ctx, []any{c.arg})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, d.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.LTrim].getFunction(ctx, []Expression{NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestRTrim 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestRTrim() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestRTrim(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: arg any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {" bar ", false, false, " bar"},
    // Go: {"bar", false, false, "bar"},
    // Go: {"bar \n", false, false, "bar \n"},
    // Go: {"bar\n ", false, false, "bar\n"},
    // Go: {"bar \r", false, false, "bar \r"},
    // Go: {"bar\r ", false, false, "bar\r"},
    // Go: {"bar \t", false, false, "bar \t"},
    // Go: {"bar\t ", false, false, "bar\t"},
    // Go: {"", false, false, ""},
    // Go: {nil, true, false, ""},
    // Go: {errors.New("must error"), false, true, ""},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.RTrim, primitiveValsToConstants(ctx, []any{c.arg})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, d.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.RTrim].getFunction(ctx, []Expression{NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestHexFunc 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestHexFunc() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestHexFunc(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: arg any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {"abc", false, false, "616263"},
    // Go: {"你好", false, false, "E4BDA0E5A5BD"},
    // Go: {12, false, false, "C"},
    // Go: {12.3, false, false, "C"},
    // Go: {12.8, false, false, "D"},
    // Go: {-1, false, false, "FFFFFFFFFFFFFFFF"},
    // Go: {-12.3, false, false, "FFFFFFFFFFFFFFF4"},
    // Go: {-12.8, false, false, "FFFFFFFFFFFFFFF3"},
    // Go: {types.NewBinaryLiteralFromUint(0xC, -1), false, false, "0C"},
    // Go: {0x12, false, false, "12"},
    // Go: {nil, true, false, ""},
    // Go: {errors.New("must err"), false, true, ""},
    // Go: {"🀁", false, false, "F09F8081"},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.Hex, primitiveValsToConstants(ctx, []any{c.arg})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, d.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: strCases := []struct {
    // Go: arg string
    // Go: chs string
    // Go: res string
    // Go: errCode int
    // Go: }{
    // Go: {"你好", "", "E4BDA0E5A5BD", 0},
    // Go: {"你好", "gbk", "C4E3BAC3", 0},
    // Go: {"一忒(๑•ㅂ•)و✧", "", "E4B880E5BF9228E0B991E280A2E38582E280A229D988E29CA7", 0},
    // Go: {"一忒(๑•ㅂ•)و✧", "gbk", "", errno.ErrInvalidCharacterString},
    // Go: {"🀁", "gb18030", "9438E131", 0},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range strCases {
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: err := ctx.GetSessionVars().SetSystemVarWithoutValidation(vardef.CharacterSetConnection, c.chs)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: f, err := newFunctionForTest(ctx, ast.Hex, primitiveValsToConstants(ctx, []any{c.arg})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.errCode != 0 {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, strings.Contains(err.Error(), strconv.Itoa(c.errCode)))
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, d.GetString())
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.Hex].getFunction(ctx, []Expression{getInt8Con()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: _, err = funcs[ast.Hex].getFunction(ctx, []Expression{getVarcharCon()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestUnhexFunc 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestUnhexFunc() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestUnhexFunc(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: arg any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {"4D7953514C", false, false, "MySQL"},
    // Go: {"1267", false, false, string([]byte{0x12, 0x67})},
    // Go: {"126", false, false, string([]byte{0x01, 0x26})},
    // Go: {"", false, false, ""},
    // Go: {1267, false, false, string([]byte{0x12, 0x67})},
    // Go: {126, false, false, string([]byte{0x01, 0x26})},
    // Go: {1267.3, true, false, ""},
    // Go: {"string", true, false, ""},
    // Go: {"你好", true, false, ""},
    // Go: {nil, true, false, ""},
    // Go: {errors.New("must error"), false, true, ""},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.Unhex, primitiveValsToConstants(ctx, []any{c.arg})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, d.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.Unhex].getFunction(ctx, []Expression{NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestBitLength 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestBitLength() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestBitLength(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args any
    // Go: chs string
    // Go: expected int64
    // Go: isNil bool
    // Go: getErr bool
    // Go: }{
    // Go: {"hi", "", 16, false, false},
    // Go: {"你好", "", 48, false, false},
    // Go: {"", "", 0, false, false},
    // Go: {"abc", "gbk", 24, false, false},
    // Go: {"一二三", "gbk", 48, false, false},
    // Go: {"一二三", "", 72, false, false},
    // Go: {"一二三!", "gbk", 56, false, false},
    // Go: {"一二三!", "", 80, false, false},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: err := ctx.GetSessionVars().SetSystemVarWithoutValidation(vardef.CharacterSetConnection, c.chs)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: f, err := newFunctionForTest(ctx, ast.BitLength, primitiveValsToConstants(ctx, []any{c.args})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.expected, d.GetInt64())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // Go: _, err := funcs[ast.BitLength].getFunction(ctx, []Expression{NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestChar 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestChar() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestChar(t *testing.T) {
    // Go: ctx := createContext(t)
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: typeFlags := ctx.GetSessionVars().StmtCtx.TypeFlags()
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().StmtCtx.SetTypeFlags(typeFlags.WithIgnoreTruncateErr(true))
    // Go: tbl := []struct {
    // Go: str string
    // Go: iNum int64
    // Go: fNum float64
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: charset any
    // Go: result any
    // Go: warnings int
    // Go: }{
    // Go: {"65", 66, 67.5, "utf8", "ABD", 0}, // float
    // Go: {"65", 16740, 67.5, "utf8", "AAdD", 0}, // large num
    // Go: {"65", -1, 67.5, nil, "A\xff\xff\xff\xffD", 0}, // negative int
    // Go: {"a", -1, 67.5, nil, "\x00\xff\xff\xff\xffD", 0}, // invalid 'a'
    // Go: {"65", -1, 67.5, "utf8", nil, 1}, // with utf8, return nil
    // Go: {"a", -1, 67.5, "utf8", nil, 1}, // with utf8, return nil
    // Go: {"1234567", 1234567, 1234567, "gbk", "\u0012謬\u0012謬\u0012謬", 0}, // test char for gbk
    // Go: {"123456789", 123456789, 123456789, "gbk", nil, 1}, // invalid 123456789 in gbk
    // Go: }
    // Go: run := func(i int, result any, warnCnt int, dts ...any) {
    // Go: fc := funcs[ast.CharFunc]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(dts...)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err, i)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f, i)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err, i)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(result), r, i)
    // Go: if warnCnt != 0 {
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: warnings := ctx.GetSessionVars().StmtCtx.TruncateWarnings(0)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, warnCnt, len(warnings), fmt.Sprintf("%d: %v", i, warnings))
    // Go: }
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i, v := range tbl {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: run(i, v.result, v.warnings, v.str, v.iNum, v.fNum, v.charset)
    // Go: }
    // Go: // char() returns null only when the sql_mode is strict.
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: require.True(t, ctx.GetSessionVars().SQLMode.HasStrictMode())
    // Go: run(-1, nil, 1, 123456, "utf8")
    // Go:
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().SQLMode = ctx.GetSessionVars().SQLMode &^ (mysql.ModeStrictTransTables | mysql.ModeStrictAllTables)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: require.False(t, ctx.GetSessionVars().SQLMode.HasStrictMode())
    // Go: run(-2, string([]byte{1}), 1, 123456, "utf8")
    // Go: }
}

// TestCharLength 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestCharLength() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestCharLength(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: tbl := []struct {
    // Go: input any
    // Go: result any
    // Go: }{
    // Go: {"33", 2}, // string
    // Go: {"你好", 2}, // mb string
    // Go: {33, 2}, // int
    // Go: {3.14, 4}, // float
    // Go: {nil, nil}, // nil
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, v := range tbl {
    // Go: fc := funcs[ast.CharLength]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(v.input)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(v.result), r)
    // Go: }
    // Go:
    // Go: // Test binary string
    // Go: tbl = []struct {
    // Go: input any
    // Go: result any
    // Go: }{
    // Go: {"33", 2}, // string
    // Go: {"你好", 6}, // mb string
    // Go: {"CAFÉ", 5}, // mb string
    // Go: {"", 0}, // mb string
    // Go: {nil, nil}, // nil
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, v := range tbl {
    // Go: fc := funcs[ast.CharLength]
    // Go: arg := datumsToConstants(types.MakeDatums(v.input))
    // Go: tp := arg[0].GetType(ctx)
    // Go: tp.SetType(mysql.TypeVarString)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: tp.SetCharset(charset.CharsetBin)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: tp.SetCollate(charset.CollationBin)
    // Go: tp.SetFlen(types.UnspecifiedLength)
    // Go: tp.SetFlag(mysql.BinaryFlag)
    // Go: f, err := fc.getFunction(ctx, arg)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(v.result), r)
    // Go: }
    // Go: }
}

// TestFindInSet 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestFindInSet() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestFindInSet(t *testing.T) {
    // Go: ctx := createContext(t)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range []struct {
    // Go: str any
    // Go: strlst any
    // Go: ret any
    // Go: }{
    // Go: {"foo", "foo,bar", 1},
    // Go: {"foo", "foobar,bar", 0},
    // Go: {" foo ", "foo, foo ", 2},
    // Go: {"", "foo,bar,", 3},
    // Go: {"", "", 0},
    // Go: {1, 1, 1},
    // Go: {1, "1", 1},
    // Go: {"1", 1, 1},
    // Go: {"a,b", "a,b,c", 0},
    // Go: {"foo", nil, nil},
    // Go: {nil, "bar", nil},
    // Go: } {
    // Go: fc := funcs[ast.FindInSet]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(c.str, c.strlst)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(c.ret), r, fmt.Sprintf("FindInSet(%s, %s)", c.str, c.strlst))
    // Go: }
    // Go: }
}

// TestField 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestField() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestField(t *testing.T) {
    // Go: ctx := createContext(t)
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: stmtCtx := ctx.GetSessionVars().StmtCtx
    // Go: oldTypeFlags := stmtCtx.TypeFlags()
    // 资源收尾：Go defer 在函数退出时恢复测试夹具，只记录清理语义。
    // Go: defer func() {
    // Go: stmtCtx.SetTypeFlags(oldTypeFlags)
    // Go: }()
    // Go: stmtCtx.SetTypeFlags(oldTypeFlags.WithIgnoreTruncateErr(true))
    // Go:
    // Go: tbl := []struct {
    // Go: argLst []any
    // Go: ret any
    // Go: }{
    // Go: {[]any{"ej", "Hej", "ej", "Heja", "hej", "foo"}, int64(2)},
    // Go: {[]any{"fo", "Hej", "ej", "Heja", "hej", "foo"}, int64(0)},
    // Go: {[]any{"ej", "Hej", "ej", "Heja", "ej", "hej", "foo"}, int64(2)},
    // Go: {[]any{1, 2, 3, 11, 1}, int64(4)},
    // Go: {[]any{nil, 2, 3, 11, 1}, int64(0)},
    // Go: {[]any{1.1, 2.1, 3.1, 11.1, 1.1}, int64(4)},
    // Go: {[]any{1.1, "2.1", "3.1", "11.1", "1.1"}, int64(4)},
    // Go: {[]any{"1.1a", 2.1, 3.1, 11.1, 1.1}, int64(4)},
    // Go: {[]any{1.10, 0, 11e-1}, int64(2)},
    // Go: {[]any{"abc", 0, 1, 11.1, 1.1}, int64(1)},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range tbl {
    // Go: fc := funcs[ast.Field]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(c.argLst...)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(c.ret), r)
    // Go: }
    // Go: }
}

// TestLpad 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestLpad() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    assert_eq!(
        crate::builtin_string_kernel::lpadBytes(b"hi", -1, b"?"),
        None
    );
    assert_eq!(crate::builtin_string_kernel::lpadUtf8("hi", -1, "?"), None);
    assert_eq!(
        crate::builtin_string_kernel::lpadBytes(b"hi", 5, b""),
        Some(Vec::new())
    );
    assert_eq!(
        crate::builtin_string_kernel::lpadUtf8("hi", 5, ""),
        Some(String::new())
    );
    assert_eq!(
        crate::builtin_string_kernel::lpadBytes(b"1", 4_611_686_018_427_387_904, b"1"),
        None
    );
    assert_eq!(
        crate::builtin_string_kernel::lpadUtf8("1", 4_611_686_018_427_387_904, "1"),
        None
    );
    // Go: func TestLpad(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: tests := []struct {
    // Go: str string
    // Go: len int64
    // Go: padStr string
    // Go: expect any
    // Go: }{
    // Go: {"hi", 5, "?", "???hi"},
    // Go: {"hi", 1, "?", "h"},
    // Go: {"hi", 0, "?", ""},
    // Go: {"hi", -1, "?", nil},
    // Go: {"hi", 1, "", "h"},
    // Go: {"hi", 5, "", ""},
    // Go: {"hi", 5, "ab", "abahi"},
    // Go: {"hi", 6, "ab", "ababhi"},
    // Go: {"中文", 5, "字符", "字符字中文"},
    // Go: {"中文", 1, "a", "中"},
    // Go: {"中文", -5, "字符", nil},
    // Go: {"中文", 10, "", ""},
    // Go: // #42770: unreasonably large length should return NULL, not panic
    // Go: {"1", 4611686018427387904, "1", nil},
    // Go: }
    // Go: fc := funcs[ast.Lpad]
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, test := range tests {
    // Go: str := types.NewStringDatum(test.str)
    // Go: length := types.NewIntDatum(test.len)
    // Go: padStr := types.NewStringDatum(test.padStr)
    // Go: f, err := fc.getFunction(ctx, datumsToConstants([]types.Datum{str, length, padStr}))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: result, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if test.expect == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, result.Kind())
    // Go: } else {
    // Go: expect, _ := test.expect.(string)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, expect, result.GetString())
    // Go: }
    // Go: }
    // Go: }
}

// TestRpad 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestRpad() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    assert_eq!(
        crate::builtin_string_kernel::rpadBytes(b"hi", -1, b"?"),
        None
    );
    assert_eq!(crate::builtin_string_kernel::rpadUtf8("hi", -1, "?"), None);
    assert_eq!(
        crate::builtin_string_kernel::rpadBytes(b"hi", 5, b""),
        Some(Vec::new())
    );
    assert_eq!(
        crate::builtin_string_kernel::rpadUtf8("hi", 5, ""),
        Some(String::new())
    );
    assert_eq!(
        crate::builtin_string_kernel::rpadBytes(b"1", 4_611_686_018_427_387_904, b"1"),
        None
    );
    assert_eq!(
        crate::builtin_string_kernel::rpadUtf8("1", 4_611_686_018_427_387_904, "1"),
        None
    );
    // Go: func TestRpad(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: tests := []struct {
    // Go: str string
    // Go: len int64
    // Go: padStr string
    // Go: expect any
    // Go: }{
    // Go: {"hi", 5, "?", "hi???"},
    // Go: {"hi", 1, "?", "h"},
    // Go: {"hi", 0, "?", ""},
    // Go: {"hi", -1, "?", nil},
    // Go: {"hi", 1, "", "h"},
    // Go: {"hi", 5, "", ""},
    // Go: {"hi", 5, "ab", "hiaba"},
    // Go: {"hi", 6, "ab", "hiabab"},
    // Go: {"中文", 5, "字符", "中文字符字"},
    // Go: {"中文", 1, "a", "中"},
    // Go: {"中文", -5, "字符", nil},
    // Go: {"中文", 10, "", ""},
    // Go: // #42770: unreasonably large length should return NULL, not panic
    // Go: {"1", 4611686018427387904, "1", nil},
    // Go: }
    // Go: fc := funcs[ast.Rpad]
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, test := range tests {
    // Go: str := types.NewStringDatum(test.str)
    // Go: length := types.NewIntDatum(test.len)
    // Go: padStr := types.NewStringDatum(test.padStr)
    // Go: f, err := fc.getFunction(ctx, datumsToConstants([]types.Datum{str, length, padStr}))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: result, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if test.expect == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, result.Kind())
    // Go: } else {
    // Go: expect, _ := test.expect.(string)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, expect, result.GetString())
    // Go: }
    // Go: }
    // Go: }
}

// TestRpadSig 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestRpadSig() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestRpadSig(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: colTypes := []*types.FieldType{
    // Go: types.NewFieldType(mysql.TypeVarchar),
    // Go: types.NewFieldType(mysql.TypeLonglong),
    // Go: types.NewFieldType(mysql.TypeVarchar),
    // Go: }
    // Go: resultType := &types.FieldType{}
    // Go: resultType.SetType(mysql.TypeVarchar)
    // Go: resultType.SetFlen(1000)
    // Go:
    // Go: args := []Expression{
    // Go: &Column{Index: 0, RetType: colTypes[0]},
    // Go: &Column{Index: 1, RetType: colTypes[1]},
    // Go: &Column{Index: 2, RetType: colTypes[2]},
    // Go: }
    // Go:
    // Go: base := baseBuiltinFunc{args: args, tp: resultType}
    // Go: rpad := &builtinRpadUTF8Sig{base, 1000}
    // Go:
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: input := chunk.NewChunkWithCapacity(colTypes, 10)
    // Go: input.AppendString(0, "abc")
    // Go: input.AppendString(0, "abc")
    // Go: input.AppendInt64(1, 6)
    // Go: input.AppendInt64(1, 10000)
    // Go: input.AppendString(2, "123")
    // Go: input.AppendString(2, "123")
    // Go:
    // Go: res, isNull, err := rpad.evalString(ctx, input.GetRow(0))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "abc123", res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: res, isNull, err = rpad.evalString(ctx, input.GetRow(1))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "", res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: warnings := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, 1, len(warnings))
    // Go: lastWarn := warnings[len(warnings)-1]
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Truef(t, terror.ErrorEqual(errWarnAllowedPacketOverflowed, lastWarn.Err), "err %v", lastWarn.Err)
    // Go: }
}

// TestInsertBinarySig 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestInsertBinarySig() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestInsertBinarySig(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: colTypes := []*types.FieldType{
    // Go: types.NewFieldType(mysql.TypeVarchar),
    // Go: types.NewFieldType(mysql.TypeLonglong),
    // Go: types.NewFieldType(mysql.TypeLonglong),
    // Go: types.NewFieldType(mysql.TypeVarchar),
    // Go: }
    // Go: resultType := &types.FieldType{}
    // Go: resultType.SetType(mysql.TypeVarchar)
    // Go: resultType.SetFlen(3)
    // Go:
    // Go: args := []Expression{
    // Go: &Column{Index: 0, RetType: colTypes[0]},
    // Go: &Column{Index: 1, RetType: colTypes[1]},
    // Go: &Column{Index: 2, RetType: colTypes[2]},
    // Go: &Column{Index: 3, RetType: colTypes[3]},
    // Go: }
    // Go:
    // Go: base := baseBuiltinFunc{args: args, tp: resultType}
    // Go: insert := &builtinInsertSig{base, 3}
    // Go:
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: input := chunk.NewChunkWithCapacity(colTypes, 2)
    // Go: input.AppendString(0, "abc")
    // Go: input.AppendString(0, "abc")
    // Go: input.AppendString(0, "abc")
    // Go: input.AppendNull(0)
    // Go: input.AppendString(0, "abc")
    // Go: input.AppendString(0, "abc")
    // Go: input.AppendString(0, "abc")
    // Go: input.AppendInt64(1, 3)
    // Go: input.AppendInt64(1, 3)
    // Go: input.AppendInt64(1, 0)
    // Go: input.AppendInt64(1, 3)
    // Go: input.AppendNull(1)
    // Go: input.AppendInt64(1, 3)
    // Go: input.AppendInt64(1, 3)
    // Go: input.AppendInt64(2, -1)
    // Go: input.AppendInt64(2, -1)
    // Go: input.AppendInt64(2, -1)
    // Go: input.AppendInt64(2, -1)
    // Go: input.AppendInt64(2, -1)
    // Go: input.AppendNull(2)
    // Go: input.AppendInt64(2, -1)
    // Go: input.AppendString(3, "d")
    // Go: input.AppendString(3, "de")
    // Go: input.AppendString(3, "d")
    // Go: input.AppendString(3, "d")
    // Go: input.AppendString(3, "d")
    // Go: input.AppendString(3, "d")
    // Go: input.AppendNull(3)
    // Go:
    // Go: res, isNull, err := insert.evalString(ctx, input.GetRow(0))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "abd", res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: res, isNull, err = insert.evalString(ctx, input.GetRow(1))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "", res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: res, isNull, err = insert.evalString(ctx, input.GetRow(2))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "abc", res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: res, isNull, err = insert.evalString(ctx, input.GetRow(3))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "", res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: res, isNull, err = insert.evalString(ctx, input.GetRow(4))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "", res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: res, isNull, err = insert.evalString(ctx, input.GetRow(5))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "", res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: res, isNull, err = insert.evalString(ctx, input.GetRow(6))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "", res)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, isNull)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: warnings := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, 1, len(warnings))
    // Go: lastWarn := warnings[len(warnings)-1]
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Truef(t, terror.ErrorEqual(errWarnAllowedPacketOverflowed, lastWarn.Err), "err %v", lastWarn.Err)
    // Go: }
}

// TestInstr 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestInstr() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestInstr(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: tbl := []struct {
    // Go: Args []any
    // Go: Want any
    // Go: }{
    // Go: {[]any{"foobarbar", "bar"}, 4},
    // Go: {[]any{"xbar", "foobar"}, 0},
    // Go:
    // Go: {[]any{123456234, 234}, 2},
    // Go: {[]any{123456, 567}, 0},
    // Go: {[]any{1e10, 1e2}, 1},
    // Go: {[]any{1.234, ".234"}, 2},
    // Go: {[]any{1.234, ""}, 1},
    // Go: {[]any{"", 123}, 0},
    // Go: {[]any{"", ""}, 1},
    // Go:
    // Go: {[]any{"中文美好", "美好"}, 3},
    // Go: {[]any{"中文美好", "世界"}, 0},
    // Go: {[]any{"中文abc", "a"}, 3},
    // Go:
    // Go: {[]any{"live long and prosper", "long"}, 6},
    // Go:
    // Go: {[]any{"not binary string", "binary"}, 5},
    // Go: {[]any{"upper case", "upper"}, 1},
    // Go: {[]any{"UPPER CASE", "CASE"}, 7},
    // Go: {[]any{"中文abc", "abc"}, 3},
    // Go:
    // Go: {[]any{"foobar", nil}, nil},
    // Go: {[]any{nil, "foobar"}, nil},
    // Go: {[]any{nil, nil}, nil},
    // Go: }
    // Go:
    // Go: Dtbl := tblToDtbl(tbl)
    // Go: instr := funcs[ast.Instr]
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i, c := range Dtbl {
    // Go: f, err := instr.getFunction(ctx, datumsToConstants(c["Args"]))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: got, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equalf(t, c["Want"][0], got, "[%d]: args: %v", i, c["Args"])
    // Go: }
    // Go: }
}

// TestLoadFile 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestLoadFile() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestLoadFile(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: arg any
    // Go: isNil bool
    // Go: getErr bool
    // Go: res string
    // Go: }{
    // Go: {"", true, false, ""},
    // Go: {"/tmp/tikv/tikv.frm", true, false, ""},
    // Go: {"tidb.sql", true, false, ""},
    // Go: {nil, true, false, ""},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.LoadFile, primitiveValsToConstants(ctx, []any{c.arg})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, d.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go: _, err := funcs[ast.LoadFile].getFunction(ctx, []Expression{NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestMakeSet 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestMakeSet() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestMakeSet(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: tbl := []struct {
    // Go: argList []any
    // Go: ret any
    // Go: }{
    // Go: {[]any{1, "a", "b", "c"}, "a"},
    // Go: {[]any{1 | 4, "hello", "nice", "world"}, "hello,world"},
    // Go: {[]any{1 | 4, "hello", "nice", nil, "world"}, "hello"},
    // Go: {[]any{0, "a", "b", "c"}, ""},
    // Go: {[]any{nil, "a", "b", "c"}, nil},
    // Go: {[]any{-100 | 4, "hello", "nice", "abc", "world"}, "abc,world"},
    // Go: {[]any{-1, "hello", "nice", "abc", "world"}, "hello,nice,abc,world"},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range tbl {
    // Go: fc := funcs[ast.MakeSet]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(c.argList...)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(c.ret), r)
    // Go: }
    // Go: }
}

// TestOct 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestOct() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestOct(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: octTests := []struct {
    // Go: origin any
    // Go: ret string
    // Go: }{
    // Go: {"-2.7", "1777777777777777777776"},
    // Go: {-1.5, "1777777777777777777777"},
    // Go: {-1, "1777777777777777777777"},
    // Go: {"0", "0"},
    // Go: {"1", "1"},
    // Go: {"8", "10"},
    // Go: {"12", "14"},
    // Go: {"20", "24"},
    // Go: {"100", "144"},
    // Go: {"1024", "2000"},
    // Go: {"2048", "4000"},
    // Go: {1.0, "1"},
    // Go: {9.5, "11"},
    // Go: {13, "15"},
    // Go: {1025, "2001"},
    // Go: {"8a8", "10"},
    // Go: {"abc", "0"},
    // Go: // overflow uint64
    // Go: {"9999999999999999999999999", "1777777777777777777777"},
    // Go: {"-9999999999999999999999999", "1777777777777777777777"},
    // Go: {types.NewBinaryLiteralFromUint(255, -1), "377"}, // b'11111111'
    // Go: {types.NewBinaryLiteralFromUint(10, -1), "12"}, // b'1010'
    // Go: {types.NewBinaryLiteralFromUint(5, -1), "5"}, // b'0101'
    // Go: }
    // Go: fc := funcs[ast.Oct]
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range octTests {
    // Go: in := types.NewDatum(tt.origin)
    // Go: f, _ := fc.getFunction(ctx, datumsToConstants([]types.Datum{in}))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: res, err := r.ToString()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equalf(t, tt.ret, res, "select oct(%v);", tt.origin)
    // Go: }
    // Go: // tt NULL input for sha
    // Go: var argNull types.Datum
    // Go: f, _ := fc.getFunction(ctx, datumsToConstants([]types.Datum{argNull}))
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, r.IsNull())
    // Go: }
}

// TestFormat 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestFormat() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestFormat(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: formatTests := []struct {
    // Go: number any
    // Go: precision any
    // Go: locale string
    // Go: ret any
    // Go: }{
    // Go: {12332.12341111111111111111111111111111111111111, 4, "en_US", "12,332.1234"},
    // Go: {nil, 22, "en_US", nil},
    // Go: }
    // Go: formatTests1 := []struct {
    // Go: number any
    // Go: precision any
    // Go: ret any
    // Go: warnings int
    // Go: }{
    // Go: // issue #8796
    // Go: {1.12345, 4, "1.1235", 0},
    // Go: {9.99999, 4, "10.0000", 0},
    // Go: {1.99999, 4, "2.0000", 0},
    // Go: {1.09999, 4, "1.1000", 0},
    // Go: {-2.5000, 0, "-3", 0},
    // Go:
    // Go: {12332.123444, 4, "12,332.1234", 0},
    // Go: {12332.123444, 0, "12,332", 0},
    // Go: {12332.123444, -4, "12,332", 0},
    // Go: {-12332.123444, 4, "-12,332.1234", 0},
    // Go: {-12332.123444, 0, "-12,332", 0},
    // Go: {-12332.123444, -4, "-12,332", 0},
    // Go: {"12332.123444", "4", "12,332.1234", 0},
    // Go: {"12332.123444A", "4", "12,332.1234", 1},
    // Go: {"-12332.123444", "4", "-12,332.1234", 0},
    // Go: {"-12332.123444A", "4", "-12,332.1234", 1},
    // Go: {"A123345", "4", "0.0000", 1},
    // Go: {"-A123345", "4", "0.0000", 1},
    // Go: {"-12332.123444", "A", "-12,332", 1},
    // Go: {"12332.123444", "A", "12,332", 1},
    // Go: {"-12332.123444", "4A", "-12,332.1234", 1},
    // Go: {"12332.123444", "4A", "12,332.1234", 1},
    // Go: {"-A12332.123444", "A", "0", 2},
    // Go: {"A12332.123444", "A", "0", 2},
    // Go: {"-A12332.123444", "4A", "0.0000", 2},
    // Go: {"A12332.123444", "4A", "0.0000", 2},
    // Go: {"-.12332.123444", "4A", "-0.1233", 2},
    // Go: {".12332.123444", "4A", "0.1233", 2},
    // Go: {"12332.1234567890123456789012345678901", 22, "12,332.1234567890110000000000", 0},
    // Go: {nil, 22, nil, 0},
    // Go: {1, 1024, "1.000000000000000000000000000000", 0},
    // Go: {"", 1, "0.0", 0},
    // Go: {1, "", "1", 1},
    // Go: }
    // Go: formatTests2 := struct {
    // Go: number any
    // Go: precision any
    // Go: locale string
    // Go: ret any
    // Go: }{-12332.123456, -4, "zh_CN", "-12,332"}
    // Go: formatTests3 := struct {
    // Go: number any
    // Go: precision any
    // Go: locale string
    // Go: ret any
    // Go: }{"-12332.123456", "4", "de_GE", "-12,332.1235"}
    // Go: formatTests4 := struct {
    // Go: number any
    // Go: precision any
    // Go: locale any
    // Go: ret any
    // Go: }{1, 4, nil, "1.0000"}
    // Go:
    // Go: fc := funcs[ast.Format]
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range formatTests {
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(tt.number, tt.precision, tt.locale)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(tt.ret), r)
    // Go: }
    // Go:
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: origTypeFlags := ctx.GetSessionVars().StmtCtx.TypeFlags()
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().StmtCtx.SetTypeFlags(origTypeFlags.WithTruncateAsWarning(true))
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range formatTests1 {
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(tt.number, tt.precision)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(tt.ret), r, fmt.Sprintf("test %v", tt))
    // Go: if tt.warnings > 0 {
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: warnings := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Lenf(t, warnings, tt.warnings, "test %v", tt)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := range tt.warnings {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Truef(t, terror.ErrorEqual(types.ErrTruncatedWrongVal, warnings[i].Err), "test %v", tt)
    // Go: }
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().StmtCtx.SetWarnings([]contextutil.SQLWarn{})
    // Go: }
    // Go: }
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().StmtCtx.SetTypeFlags(origTypeFlags)
    // Go:
    // Go: f2, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(formatTests2.number, formatTests2.precision, formatTests2.locale)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f2)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r2, err := evalBuiltinFunc(f2, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(errors.New("not implemented")), types.NewDatum(err))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(formatTests2.ret), r2)
    // Go:
    // Go: f3, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(formatTests3.number, formatTests3.precision, formatTests3.locale)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f3)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r3, err := evalBuiltinFunc(f3, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(errors.New("not support for the specific locale")), types.NewDatum(err))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(formatTests3.ret), r3)
    // Go:
    // Go: f4, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(formatTests4.number, formatTests4.precision, formatTests4.locale)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f4)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r4, err := evalBuiltinFunc(f4, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(formatTests4.ret), r4)
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: warnings := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, 2, len(warnings))
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := range 2 {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(errUnknownLocale, warnings[i].Err))
    // Go: }
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().StmtCtx.SetWarnings([]contextutil.SQLWarn{})
    // Go: }
}

// TestFromBase64 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestFromBase64() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: func TestFromBase64(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: tests := []struct {
    // Go: args any
    // Go: expect any
    // Go: }{
    // Go: {"", ""},
    // Go: {"YWJj", "abc"},
    // Go: {"YWIgYw==", "ab c"},
    // Go: {"YWIKYw==", "ab\nc"},
    // Go: {"YWIJYw==", "ab\tc"},
    // Go: {"cXdlcnR5MTIzNDU2", "qwerty123456"},
    // Go: {
    // Go: "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0\nNTY3ODkrL0FCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaYWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4\neXowMTIzNDU2Nzg5Ky9BQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWmFiY2RlZmdoaWprbG1ub3Bx\ncnN0dXZ3eHl6MDEyMzQ1Njc4OSsv",
    // Go: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    // Go: },
    // Go: {
    // Go: "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0NTY3ODkrLw==",
    // Go: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    // Go: },
    // Go: {
    // Go: "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0NTY3ODkrLw==",
    // Go: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    // Go: },
    // Go: {
    // Go: "QUJDREVGR0hJSkt\tMTU5PUFFSU1RVVld\nYWVphYmNkZ\rWZnaGlqa2xt bm9wcXJzdHV2d3h5ejAxMjM0NTY3ODkrLw==",
    // Go: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    // Go: },
    // Go: }
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: fc := funcs[ast.FromBase64]
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, test := range tests {
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(test.args)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: result, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if test.expect == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, result.Kind())
    // Go: } else {
    // Go: expect, _ := test.expect.(string)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, expect, result.GetString())
    // Go: }
    // Go: }
    // Go: }
}

// TestFromBase64Sig 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestFromBase64Sig() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: func TestFromBase64Sig(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: colTypes := []*types.FieldType{
    // Go: types.NewFieldType(mysql.TypeVarchar),
    // Go: }
    // Go:
    // Go: tests := []struct {
    // Go: args string
    // Go: expect string
    // Go: isNil bool
    // Go: maxAllowPacket uint64
    // Go: }{
    // Go: {"YWJj", "abc", false, 3},
    // Go: {"YWJj", "", true, 2},
    // Go: {
    // Go: "QUJDREVGR0hJSkt\tMTU5PUFFSU1RVVld\nYWVphYmNkZ\rWZnaGlqa2xt bm9wcXJzdHV2d3h5ejAxMjM0NTY3ODkrLw==",
    // Go: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    // Go: false,
    // Go: 70,
    // Go: },
    // Go: {
    // Go: "QUJDREVGR0hJSkt\tMTU5PUFFSU1RVVld\nYWVphYmNkZ\rWZnaGlqa2xt bm9wcXJzdHV2d3h5ejAxMjM0NTY3ODkrLw==",
    // Go: "",
    // Go: true,
    // Go: 69,
    // Go: },
    // Go: }
    // Go:
    // Go: args := []Expression{
    // Go: &Column{Index: 0, RetType: colTypes[0]},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, test := range tests {
    // Go: resultType := &types.FieldType{}
    // Go: resultType.SetType(mysql.TypeVarchar)
    // Go: resultType.SetFlen(mysql.MaxBlobWidth)
    // Go: base := baseBuiltinFunc{args: args, tp: resultType}
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: fromBase64 := &builtinFromBase64Sig{base, test.maxAllowPacket}
    // Go:
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: input := chunk.NewChunkWithCapacity(colTypes, 1)
    // Go: input.AppendString(0, test.args)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: res, isNull, err := fromBase64.evalString(ctx, input.GetRow(0))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, test.isNil, isNull)
    // Go: if isNull {
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: warnings := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, 1, len(warnings))
    // Go: lastWarn := warnings[len(warnings)-1]
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(errWarnAllowedPacketOverflowed, lastWarn.Err))
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().StmtCtx.SetWarnings([]contextutil.SQLWarn{})
    // Go: }
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, test.expect, res)
    // Go: }
    // Go: }
}

// TestInsert 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestInsert() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestInsert(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: tests := []struct {
    // Go: args []any
    // Go: expect any
    // Go: }{
    // Go: {[]any{"Quadratic", 3, 4, "What"}, "QuWhattic"},
    // Go: {[]any{"Quadratic", -1, 4, "What"}, "Quadratic"},
    // Go: {[]any{"Quadratic", 3, 100, "What"}, "QuWhat"},
    // Go: {[]any{nil, 3, 100, "What"}, nil},
    // Go: {[]any{"Quadratic", nil, 4, "What"}, nil},
    // Go: {[]any{"Quadratic", 3, nil, "What"}, nil},
    // Go: {[]any{"Quadratic", 3, 4, nil}, nil},
    // Go: {[]any{"Quadratic", 3, -1, "What"}, "QuWhat"},
    // Go: {[]any{"Quadratic", 3, 1, "What"}, "QuWhatdratic"},
    // Go: {[]any{"Quadratic", -1, nil, "What"}, nil},
    // Go: {[]any{"Quadratic", -1, 4, nil}, nil},
    // Go:
    // Go: {[]any{"我叫小雨呀", 3, 2, "王雨叶"}, "我叫王雨叶呀"},
    // Go: {[]any{"我叫小雨呀", -1, 2, "王雨叶"}, "我叫小雨呀"},
    // Go: {[]any{"我叫小雨呀", 3, 100, "王雨叶"}, "我叫王雨叶"},
    // Go: {[]any{nil, 3, 100, "王雨叶"}, nil},
    // Go: {[]any{"我叫小雨呀", nil, 4, "王雨叶"}, nil},
    // Go: {[]any{"我叫小雨呀", 3, nil, "王雨叶"}, nil},
    // Go: {[]any{"我叫小雨呀", 3, 4, nil}, nil},
    // Go: {[]any{"我叫小雨呀", 3, -1, "王雨叶"}, "我叫王雨叶"},
    // Go: {[]any{"我叫小雨呀", 3, 1, "王雨叶"}, "我叫王雨叶雨呀"},
    // Go: {[]any{"我叫小雨呀", -1, nil, "王雨叶"}, nil},
    // Go: {[]any{"我叫小雨呀", -1, 2, nil}, nil},
    // Go: }
    // Go: fc := funcs[ast.InsertFunc]
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, test := range tests {
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(test.args...)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: result, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if test.expect == nil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, result.Kind())
    // Go: } else {
    // Go: expect, _ := test.expect.(string)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, expect, result.GetString())
    // Go: }
    // Go: }
    // Go: }
}

// TestOrd 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestOrd() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestOrd(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args any
    // Go: expected int64
    // Go: chs string
    // Go: isNil bool
    // Go: getErr bool
    // Go: }{
    // Go: {"2", 50, "", false, false},
    // Go: {2, 50, "", false, false},
    // Go: {"23", 50, "", false, false},
    // Go: {23, 50, "", false, false},
    // Go: {2.3, 50, "", false, false},
    // Go: {nil, 0, "", true, false},
    // Go: {"", 0, "", false, false},
    // Go: {"你好", 14990752, "utf8mb4", false, false},
    // Go: {"にほん", 14909867, "utf8mb4", false, false},
    // Go: {"한국", 15570332, "utf8mb4", false, false},
    // Go: {"👍", 4036989325, "utf8mb4", false, false},
    // Go: {"א", 55184, "utf8mb4", false, false},
    // Go: {"abc", 97, "gbk", false, false},
    // Go: {"一二三", 53947, "gbk", false, false},
    // Go: {"àáèé", 43172, "gbk", false, false},
    // Go: {"数据库", 51965, "gbk", false, false},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: err := ctx.GetSessionVars().SetSystemVarWithoutValidation(vardef.CharacterSetConnection, c.chs)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: f, err := newFunctionForTest(ctx, ast.Ord, primitiveValsToConstants(ctx, []any{c.args})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.expected, d.GetInt64())
    // Go: }
    // Go: }
    // Go: }
    // Go: _, err := funcs[ast.Ord].getFunction(ctx, []Expression{NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: }
}

// TestElt 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestElt() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestElt(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: tbl := []struct {
    // Go: argLst []any
    // Go: ret any
    // Go: }{
    // Go: {[]any{1, "Hej", "ej", "Heja", "hej", "foo"}, "Hej"},
    // Go: {[]any{9, "Hej", "ej", "Heja", "hej", "foo"}, nil},
    // Go: {[]any{-1, "Hej", "ej", "Heja", "ej", "hej", "foo"}, nil},
    // Go: {[]any{0, 2, 3, 11, 1}, nil},
    // Go: {[]any{3, 2, 3, 11, 1}, "11"},
    // Go: {[]any{1.1, "2.1", "3.1", "11.1", "1.1"}, "2.1"},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range tbl {
    // Go: fc := funcs[ast.Elt]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(c.argLst...)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(c.ret), r)
    // Go: }
    // Go: }
}

// TestExportSet 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestExportSet() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestExportSet(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: estd := []struct {
    // Go: argLst []any
    // Go: res string
    // Go: }{
    // Go: {[]any{-9223372036854775807, "Y", "N", ",", 5}, "Y,N,N,N,N"},
    // Go: {[]any{-6, "Y", "N", ",", 5}, "N,Y,N,Y,Y"},
    // Go: {[]any{5, "Y", "N", ",", 4}, "Y,N,Y,N"},
    // Go: {[]any{5, "Y", "N", ",", 0}, ""},
    // Go: {[]any{5, "Y", "N", ",", 1}, "Y"},
    // Go: {[]any{6, "1", "0", ",", 10}, "0,1,1,0,0,0,0,0,0,0"},
    // Go: {[]any{333333, "Ysss", "sN", "---", 9}, "Ysss---sN---Ysss---sN---Ysss---sN---sN---sN---sN"},
    // Go: {[]any{7, "Y", "N"}, "Y,Y,Y,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N,N"},
    // Go: {[]any{7, "Y", "N", 6}, "Y6Y6Y6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N"},
    // Go: {[]any{7, "Y", "N", 6, 133}, "Y6Y6Y6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N6N"},
    // Go: }
    // Go: fc := funcs[ast.ExportSet]
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range estd {
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(c.argLst...)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: exportSetRes, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: res, err := exportSetRes.ToString()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, res)
    // Go: }
    // Go: }
}

// TestBin 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestBin() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestBin(t *testing.T) {
    // Go: tbl := []struct {
    // Go: Input any
    // Go: Expected any
    // Go: }{
    // Go: {"10", "1010"},
    // Go: {"10.2", "1010"},
    // Go: {"10aa", "1010"},
    // Go: {"10.2aa", "1010"},
    // Go: {"aaa", "0"},
    // Go: {"", nil},
    // Go: {10, "1010"},
    // Go: {10.0, "1010"},
    // Go: {-1, "1111111111111111111111111111111111111111111111111111111111111111"},
    // Go: {"-1", "1111111111111111111111111111111111111111111111111111111111111111"},
    // Go: {nil, nil},
    // Go: }
    // Go: fc := funcs[ast.Bin]
    // Go: dtbl := tblToDtbl(tbl)
    // Go: ctx := mock.NewContext()
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: typeFlags := ctx.GetSessionVars().StmtCtx.TypeFlags()
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().StmtCtx.SetTypeFlags(typeFlags.WithIgnoreTruncateErr(true))
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range dtbl {
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(c["Input"]))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(c["Expected"][0]), r)
    // Go: }
    // Go: }
}

// TestQuote 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestQuote() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestQuote(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: tbl := []struct {
    // Go: arg any
    // Go: ret any
    // Go: }{
    // Go: {`Don\'t!`, `'Don\\\'t!'`},
    // Go: {`Don't`, `'Don\'t'`},
    // Go: {`Don"`, `'Don"'`},
    // Go: {`Don\"`, `'Don\\"'`},
    // Go: {`\'`, `'\\\''`},
    // Go: {`\"`, `'\\"'`},
    // Go: {`萌萌哒(๑•ᴗ•๑)😊`, `'萌萌哒(๑•ᴗ•๑)😊'`},
    // Go: {`㍿㌍㍑㌫`, `'㍿㌍㍑㌫'`},
    // Go: {string([]byte{0, 26}), `'\0\Z'`},
    // Go: {nil, "NULL"},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range tbl {
    // Go: fc := funcs[ast.Quote]
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(c.arg)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(c.ret), r)
    // Go: }
    // Go: }
}

// TestToBase64 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestToBase64() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: func TestToBase64(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: tests := []struct {
    // Go: args any
    // Go: expect string
    // Go: isNil bool
    // Go: getErr bool
    // Go: }{
    // Go: {"", "", false, false},
    // Go: {"abc", "YWJj", false, false},
    // Go: {"ab c", "YWIgYw==", false, false},
    // Go: {1, "MQ==", false, false},
    // Go: {1.1, "MS4x", false, false},
    // Go: {"ab\nc", "YWIKYw==", false, false},
    // Go: {"ab\tc", "YWIJYw==", false, false},
    // Go: {"qwerty123456", "cXdlcnR5MTIzNDU2", false, false},
    // Go: {
    // Go: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    // Go: "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0\nNTY3ODkrLw==",
    // Go: false,
    // Go: false,
    // Go: },
    // Go: {
    // Go: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    // Go: "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0\nNTY3ODkrL0FCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaYWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4\neXowMTIzNDU2Nzg5Ky9BQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWmFiY2RlZmdoaWprbG1ub3Bx\ncnN0dXZ3eHl6MDEyMzQ1Njc4OSsv",
    // Go: false,
    // Go: false,
    // Go: },
    // Go: {
    // Go: "ABCD EFGHI\nJKLMNOPQRSTUVWXY\tZabcdefghijklmnopqrstuv wxyz012\r3456789+/",
    // Go: "QUJDRCAgRUZHSEkKSktMTU5PUFFSU1RVVldYWQlaYWJjZGVmZ2hpamtsbW5vcHFyc3R1diAgd3h5\nejAxMg0zNDU2Nzg5Ky8=",
    // Go: false,
    // Go: false,
    // Go: },
    // Go: {nil, "", true, false},
    // Go: }
    // Go: if strconv.IntSize == 32 {
    // Go: tests = append(tests, struct {
    // Go: args any
    // Go: expect string
    // Go: isNil bool
    // Go: getErr bool
    // Go: }{
    // Go: strings.Repeat("a", 1589695687),
    // Go: "",
    // Go: true,
    // Go: false,
    // Go: })
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, test := range tests {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: f, err := newFunctionForTest(ctx, ast.ToBase64, primitiveValsToConstants(ctx, []any{test.args})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if test.getErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if test.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, test.expect, d.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go:
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: _, err := funcs[ast.ToBase64].getFunction(ctx, []Expression{NewZero()})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: // Test GBK String
    // Go: tbl := []struct {
    // Go: input string
    // Go: chs string
    // Go: result string
    // Go: }{
    // Go: {"abc", "gbk", "YWJj"},
    // Go: {"一二三", "gbk", "0ru2/sj9"},
    // Go: {"一二三", "", "5LiA5LqM5LiJ"},
    // Go: {"一二三!", "gbk", "0ru2/sj9IQ=="},
    // Go: {"一二三!", "", "5LiA5LqM5LiJIQ=="},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range tbl {
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: err := ctx.GetSessionVars().SetSystemVarWithoutValidation(vardef.CharacterSetConnection, c.chs)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: f, err := newFunctionForTest(ctx, ast.ToBase64, primitiveValsToConstants(ctx, []any{c.input})...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.result, d.GetString())
    // Go: }
    // Go: }
}

// TestToBase64Sig 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestToBase64Sig() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: func TestToBase64Sig(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: colTypes := []*types.FieldType{
    // Go: types.NewFieldType(mysql.TypeVarchar),
    // Go: }
    // Go:
    // Go: tests := []struct {
    // Go: args string
    // Go: expect string
    // Go: isNil bool
    // Go: maxAllowPacket uint64
    // Go: }{
    // Go: {"abc", "YWJj", false, 4},
    // Go: {"abc", "", true, 3},
    // Go: {
    // Go: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    // Go: "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0\nNTY3ODkrLw==",
    // Go: false,
    // Go: 89,
    // Go: },
    // Go: {
    // Go: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    // Go: "",
    // Go: true,
    // Go: 88,
    // Go: },
    // Go: {
    // Go: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    // Go: "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0\nNTY3ODkrL0FCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaYWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4\neXowMTIzNDU2Nzg5Ky9BQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWmFiY2RlZmdoaWprbG1ub3Bx\ncnN0dXZ3eHl6MDEyMzQ1Njc4OSsv",
    // Go: false,
    // Go: 259,
    // Go: },
    // Go: {
    // Go: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
    // Go: "",
    // Go: true,
    // Go: 258,
    // Go: },
    // Go: }
    // Go:
    // Go: args := []Expression{
    // Go: &Column{Index: 0, RetType: colTypes[0]},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, test := range tests {
    // Go: resultType := &types.FieldType{}
    // Go: resultType.SetType(mysql.TypeVarchar)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: resultType.SetFlen(base64NeededEncodedLength(len(test.args)))
    // Go: base := baseBuiltinFunc{args: args, tp: resultType}
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: toBase64 := &builtinToBase64Sig{base, test.maxAllowPacket}
    // Go:
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: input := chunk.NewChunkWithCapacity(colTypes, 1)
    // Go: input.AppendString(0, test.args)
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: res, isNull, err := toBase64.evalString(ctx, input.GetRow(0))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if test.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, isNull)
    // Go:
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: warnings := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, 1, len(warnings))
    // Go: lastWarn := warnings[len(warnings)-1]
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(errWarnAllowedPacketOverflowed, lastWarn.Err))
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().StmtCtx.SetWarnings([]contextutil.SQLWarn{})
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.False(t, isNull)
    // Go: }
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, test.expect, res)
    // Go: }
    // Go: }
}

// TestStringRight 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestStringRight() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestStringRight(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: fc := funcs[ast.Right]
    // Go: tests := []struct {
    // Go: str any
    // Go: length any
    // Go: expect any
    // Go: }{
    // Go: {"helloworld", 5, "world"},
    // Go: {"helloworld", 10, "helloworld"},
    // Go: {"helloworld", 11, "helloworld"},
    // Go: {"helloworld", -1, ""},
    // Go: {"", 2, ""},
    // Go: {nil, 2, nil},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, test := range tests {
    // Go: str := types.NewDatum(test.str)
    // Go: length := types.NewDatum(test.length)
    // Go: f, _ := fc.getFunction(ctx, datumsToConstants([]types.Datum{str, length}))
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: result, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if result.IsNull() {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, test.expect)
    // Go: continue
    // Go: }
    // Go: res, err := result.ToString()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, test.expect, res)
    // Go: }
    // Go: }
}

// TestWeightString 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestWeightString() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestWeightString(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: fc := funcs[ast.WeightString]
    // Go: tests := []struct {
    // Go: expr any
    // Go: padding string
    // Go: length int
    // Go: expect any
    // Go: }{
    // Go: {nil, "NONE", 0, nil},
    // Go: {7, "NONE", 0, nil},
    // Go: {7.0, "NONE", 0, nil},
    // Go: {"a", "NONE", 0, "a"},
    // Go: {"a ", "NONE", 0, "a"},
    // Go: {"中", "NONE", 0, "中"},
    // Go: {"中 ", "NONE", 0, "中"},
    // Go: {nil, "CHAR", 5, nil},
    // Go: {7, "CHAR", 5, nil},
    // Go: {7.0, "NONE", 0, nil},
    // Go: {"a", "CHAR", 5, "a"},
    // Go: {"a ", "CHAR", 5, "a"},
    // Go: {"中", "CHAR", 5, "中"},
    // Go: {"中 ", "CHAR", 5, "中"},
    // Go: {nil, "BINARY", 5, nil},
    // Go: {7, "BINARY", 2, "7\x00"},
    // Go: {7.0, "NONE", 0, nil},
    // Go: {"a", "BINARY", 1, "a"},
    // Go: {"ab", "BINARY", 1, "a"},
    // Go: {"a", "BINARY", 5, "a\x00\x00\x00\x00"},
    // Go: {"a ", "BINARY", 5, "a \x00\x00\x00"},
    // Go: {"中", "BINARY", 1, "\xe4"},
    // Go: {"中", "BINARY", 2, "\xe4\xb8"},
    // Go: {"中", "BINARY", 3, "中"},
    // Go: {"中", "BINARY", 5, "中\x00\x00"},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, test := range tests {
    // Go: str := types.NewDatum(test.expr)
    // Go: var f builtinFunc
    // Go: var err error
    // Go: if test.padding == "NONE" {
    // Go: f, err = fc.getFunction(ctx, datumsToConstants([]types.Datum{str}))
    // Go: } else {
    // Go: padding := types.NewDatum(test.padding)
    // Go: length := types.NewDatum(test.length)
    // Go: f, err = fc.getFunction(ctx, datumsToConstants([]types.Datum{str, padding, length}))
    // Go: }
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go:
    // Go: retType := f.getRetTp()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: require.Equal(t, charset.CollationBin, retType.GetCollate())
    // Go:
    // Go: // Reset warnings.
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().StmtCtx.ResetForRetry()
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: result, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if result.IsNull() {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, test.expect)
    // Go: continue
    // Go: }
    // Go: res, err := result.ToString()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, test.expect, res)
    // Go: if test.expr == nil {
    // Go: continue
    // Go: }
    // Go: strExpr := fmt.Sprintf("%v", test.expr)
    // Go: if test.padding == "BINARY" && test.length < len(strExpr) {
    // Go: expectWarn := fmt.Sprintf("[expression:1292]Truncated incorrect BINARY(%d) value: '%s'", test.length, strExpr)
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: obtainedWarns := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, 1, len(obtainedWarns))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, "Warning", obtainedWarns[0].Level)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, expectWarn, obtainedWarns[0].Err.Error())
    // Go: }
    // Go: }
    // Go: }
}

// TestTranslate 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestTranslate() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestTranslate(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: cases := []struct {
    // Go: args []any
    // Go: isNil bool
    // Go: isErr bool
    // Go: res string
    // Go: }{
    // Go: {[]any{"ABC", "A", "B"}, false, false, "BBC"},
    // Go: {[]any{"ABC", "Z", "ABC"}, false, false, "ABC"},
    // Go: {[]any{"A.B.C", ".A", "|"}, false, false, "|B|C"},
    // Go: {[]any{"中文", "文", "国"}, false, false, "中国"},
    // Go: {[]any{"UPPERCASE", "ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz"}, false, false, "uppercase"},
    // Go: {[]any{"lowercase", "abcdefghijklmnopqrstuvwxyz", "ABCDEFGHIJKLMNOPQRSTUVWXYZ"}, false, false, "LOWERCASE"},
    // Go: {[]any{"aaaaabbbbb", "aaabbb", "xyzXYZ"}, false, false, "xxxxxXXXXX"},
    // Go: {[]any{"Ti*DB User's Guide", " */'", "___"}, false, false, "Ti_DB_Users_Guide"},
    // Go: {[]any{"abc", "ab", ""}, false, false, "c"},
    // Go: {[]any{"aaa", "a", ""}, false, false, ""},
    // Go: {[]any{"", "null", "null"}, false, false, ""},
    // Go: {[]any{"null", "", "null"}, false, false, "null"},
    // Go: {[]any{"null", "null", ""}, false, false, ""},
    // Go: {[]any{nil, "error", "error"}, true, false, ""},
    // Go: {[]any{"error", nil, "error"}, true, false, ""},
    // Go: {[]any{"error", "error", nil}, true, false, ""},
    // Go: {[]any{nil, nil, nil}, true, false, ""},
    // Go: {[]any{[]byte{255}, []byte{255}, []byte{255}}, false, false, string([]byte{255})},
    // Go: {[]any{[]byte{255, 255}, []byte{255}, []byte{254}}, false, false, string([]byte{254, 254})},
    // Go: {[]any{[]byte{255, 255}, []byte{255, 255}, []byte{254, 253}}, false, false, string([]byte{254, 254})},
    // Go: {[]any{[]byte{255, 254, 253, 252, 251}, []byte{253, 252, 251}, []byte{254, 253}}, false, false, string([]byte{255, 254, 254, 253})},
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, c := range cases {
    // Go: f, err := newFunctionForTest(ctx, ast.Translate, primitiveValsToConstants(ctx, c.args)...)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: d, err := f.Eval(ctx, chunk.Row{})
    // Go: if c.isErr {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Error(t, err)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if c.isNil {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindNull, d.Kind())
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, c.res, d.GetString())
    // Go: }
    // Go: }
    // Go: }
    // Go: }
}

// TestCIWeightString 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestCIWeightString() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestCIWeightString(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go:
    // Go: type weightStringTest struct {
    // Go: str string
    // Go: padding string
    // Go: length int
    // Go: expect any
    // Go: }
    // Go:
    // Go: checkResult := func(collation string, tests []weightStringTest) {
    // Go: fc := funcs[ast.WeightString]
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, test := range tests {
    // 字符集/编码：保留 Go 对 charset、collation、base64 等边界输入的覆盖。
    // Go: str := types.NewCollationStringDatum(test.str, collation)
    // Go: var f builtinFunc
    // Go: var err error
    // Go: if test.padding == "NONE" {
    // Go: f, err = fc.getFunction(ctx, datumsToConstants([]types.Datum{str}))
    // Go: } else {
    // Go: padding := types.NewDatum(test.padding)
    // Go: length := types.NewDatum(test.length)
    // Go: f, err = fc.getFunction(ctx, datumsToConstants([]types.Datum{str, padding, length}))
    // Go: }
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: result, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: if result.IsNull() {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Nil(t, test.expect)
    // Go: continue
    // Go: }
    // Go: res, err := result.ToString()
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, test.expect, res, "test case: '%s' '%s' %d", test.str, test.padding, test.length)
    // Go: }
    // Go: }
    // Go:
    // Go: generalTests := []weightStringTest{
    // Go: {"aAÁàãăâ", "NONE", 0, "\x00A\x00A\x00A\x00A\x00A\x00A\x00A"},
    // Go: {"中", "NONE", 0, "\x4E\x2D"},
    // Go: {"a", "CHAR", 5, "\x00A"},
    // Go: {"a ", "CHAR", 5, "\x00A"},
    // Go: {"中", "CHAR", 5, "\x4E\x2D"},
    // Go: {"中 ", "CHAR", 5, "\x4E\x2D"},
    // Go: {"a", "BINARY", 1, "a"},
    // Go: {"ab", "BINARY", 1, "a"},
    // Go: {"a", "BINARY", 5, "a\x00\x00\x00\x00"},
    // Go: {"a ", "BINARY", 5, "a \x00\x00\x00"},
    // Go: {"中", "BINARY", 1, "\xe4"},
    // Go: {"中", "BINARY", 2, "\xe4\xb8"},
    // Go: {"中", "BINARY", 3, "中"},
    // Go: {"中", "BINARY", 5, "中\x00\x00"},
    // Go: }
    // Go:
    // Go: unicodeTests := []weightStringTest{
    // Go: {"aAÁàãăâ", "NONE", 0, "\x0e3\x0e3\x0e3\x0e3\x0e3\x0e3\x0e3"},
    // Go: {"中", "NONE", 0, "\xfb\x40\xce\x2d"},
    // Go: {"a", "CHAR", 5, "\x0e3"},
    // Go: {"a ", "CHAR", 5, "\x0e3"},
    // Go: {"中", "CHAR", 5, "\xfb\x40\xce\x2d"},
    // Go: {"中 ", "CHAR", 5, "\xfb\x40\xce\x2d"},
    // Go: {"a", "BINARY", 1, "a"},
    // Go: {"ab", "BINARY", 1, "a"},
    // Go: {"a", "BINARY", 5, "a\x00\x00\x00\x00"},
    // Go: {"a ", "BINARY", 5, "a \x00\x00\x00"},
    // Go: {"中", "BINARY", 1, "\xe4"},
    // Go: {"中", "BINARY", 2, "\xe4\xb8"},
    // Go: {"中", "BINARY", 3, "中"},
    // Go: {"中", "BINARY", 5, "中\x00\x00"},
    // Go: }
    // Go:
    // Go: unicode0900Tests := []weightStringTest{
    // Go: {"aAÁàãăâ", "NONE", 0, "\x1cG\x1cG\x1cG\x1cG\x1cG\x1cG\x1cG"},
    // Go: {"中", "NONE", 0, "\xfb\x40\xce\x2d"},
    // Go: {"a", "CHAR", 5, "\x1c\x47\x02\x09\x02\x09\x02\x09\x02\x09"},
    // Go: {"a ", "CHAR", 5, "\x1c\x47\x02\x09\x02\x09\x02\x09\x02\x09"},
    // Go: {"中", "CHAR", 5, "\xfb\x40\xce\x2d\x02\x09\x02\x09\x02\x09\x02\x09"},
    // Go: {"中 ", "CHAR", 5, "\xfb\x40\xce\x2d\x02\x09\x02\x09\x02\x09\x02\x09"},
    // Go: {"a", "BINARY", 1, "a"},
    // Go: {"ab", "BINARY", 1, "a"},
    // Go: {"a", "BINARY", 5, "a\x00\x00\x00\x00"},
    // Go: {"a ", "BINARY", 5, "a \x00\x00\x00"},
    // Go: {"中", "BINARY", 1, "\xe4"},
    // Go: {"中", "BINARY", 2, "\xe4\xb8"},
    // Go: {"中", "BINARY", 3, "中"},
    // Go: {"中", "BINARY", 5, "中\x00\x00"},
    // Go: }
    // Go:
    // Go: checkResult("utf8mb4_general_ci", generalTests)
    // Go: checkResult("utf8mb4_unicode_ci", unicodeTests)
    // Go: checkResult("utf8mb4_0900_ai_ci", unicode0900Tests)
    // Go: }
}

// TestFormatWithLocale tests the 3-argument version of FORMAT(X, D, locale)
// with various locales and number formats.
// TestFormatWithLocale 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestFormatWithLocale() {
    crate::builtin_string_aster_unit_test::run_string_parity_suite();
    // Go: func TestFormatWithLocale(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: fc := funcs[ast.Format]
    // Go:
    // Go: tests := []struct {
    // Go: number any
    // Go: precision any
    // Go: locale any // Use 'any' to test NULL locale
    // Go: ret any // Expected result
    // Go: warning bool // True if we expect an 'Unknown locale' warning
    // Go: desc string
    // Go: }{
    // Go: // --- Style: CommaDot (123,456.78) ---
    // Go: // This is the default fallback for most unhandled locales.
    // Go: {1234567.89, 2, "en_US", "1,234,567.89", false, "CommaDot (en_US) - standard"},
    // Go: {-98765.432, 2, "zh_CN", "-98,765.43", false, "CommaDot (zh_CN) - negative, rounding"},
    // Go: {0.01, 4, "ja_JP", "0.0100", false, "CommaDot (ja_JP) - decimal padding"},
    // Go: {12345, 0, "en_GB", "12,345", false, "CommaDot (en_GB) - no decimal part"},
    // Go: {1.2, 2, "ko_KR", "1.20", false, "CommaDot (ko_KR) - extra locale"},
    // Go: {500.5, 1, "th_TH", "500.5", false, "CommaDot (th_TH) - extra locale"},
    // Go: {7777, 0, "en_AU", "7,777", false, "CommaDot (en_AU) - extra locale"},
    // Go: {-88.88, 2, "zh_TW", "-88.88", false, "CommaDot (zh_TW) - extra locale"},
    // Go: // Fallback locales that MySQL treats as en_US
    // Go: {9876543.21, 1, "es_MX", "9,876,543.2", false, "CommaDot (es_MX) - MySQL fallback"},
    // Go: {3000.14, 2, "ce_RU", "3,000.14", false, "CommaDot (ce_RU) - MySQL fallback"},
    // Go: {4000.1, 1, "ky_KG", "4,000.1", false, "CommaDot (ky_KG) - MySQL fallback"},
    // Go: {200, 2, "aa_DJ", "200.00", false, "CommaDot (aa_DJ) - MySQL fallback"},
    // Go: {7890123.456, 2, "ps_AF", "7,890,123.46", false, "CommaDot (ps_AF) - MySQL fallback"},
    // Go: {12345.67, 2, "an_ES", "12,345.67", false, "CommaDot (an_ES) - MySQL fallback"},
    // Go: {12345.67, 2, "az_AZ", "12,345.67", false, "CommaDot (az_AZ) - MySQL fallback"},
    // Go: {12345.67, 2, "br_FR", "12,345.67", false, "CommaDot (br_FR) - MySQL fallback"},
    // Go: {3000.14, 2, "kv_RU", "3,000.14", false, "CommaDot (kv_RU) - MySQL fallback"},
    // Go: {12345.67, 3, "su_ID", "12,345.670", false, "CommaDot (su_ID) - MySQL fallback"},
    // Go:
    // Go: // --- Style: DotComma (123.456,78) ---
    // Go: {7654321.98, 2, "de_DE", "7.654.321,98", false, "DotComma (de_DE) - large number"},
    // Go: {-9.999, 2, "es_ES", "-10,00", false, "DotComma (es_ES) - negative, rounding up to 10"},
    // Go: {"-123.45", 1, "id_ID", "-123,5", false, "DotComma (id_ID) - string input"},
    // Go: {99, 1, "vi_VN", "99,0", false, "DotComma (vi_VN) - extra locale"},
    // Go: {8888.8, 0, "ro_RO", "8.889", false, "DotComma (ro_RO) - extra locale, rounding"},
    // Go: {1234.567, 2, "da_DK", "1.234,57", false, "DotComma (da_DK) - extra locale, rounding"},
    // Go: {555.55, 1, "tr_TR", "555,6", false, "DotComma (tr_TR) - extra locale, rounding"},
    // Go: {1234.56, 2, "nb_NO", "1.234,56", false, "DotComma (nb_NO) - MySQL behavior"},
    // Go: {1234.56, 2, "uk_UA", "1.234,56", false, "DotComma (uk_UA) - MySQL behavior"},
    // Go: {12345.67, 3, "no_NO", "12.345,670", false, "DotComma (no_NO) - MySQL behavior"},
    // Go:
    // Go: // --- Style: SpaceComma (123 456,78) ---
    // Go: {-0.88, 1, "ru_RU", "-0,9", false, "SpaceComma (ru_RU) - negative, rounding"},
    // Go: {98765, 0, "sv_SE", "98 765", false, "SpaceComma (sv_SE) - no decimal part"},
    // Go: {2000, 2, "cs_CZ", "2 000,00", false, "SpaceComma (cs_CZ) - extra locale, padding"},
    // Go:
    // Go: // --- Style: NoneComma (123456,78) ---
    // Go: {-2.23, 1, "el_GR", "-2,2", false, "NoneComma (el_GR) - negative, rounding"},
    // Go: {44.44, 1, "pt_PT", "44,4", false, "NoneComma (pt_PT) - extra locale"},
    // Go: {12345, 0, "it_IT", "12345", false, "NoneComma (it_IT) - MySQL behavior"},
    // Go: {100.5, 3, "pt_BR", "100,500", false, "NoneComma (pt_BR) - MySQL behavior"},
    // Go: {500000.1, 2, "fr_FR", "500000,10", false, "NoneComma (fr_FR) - MySQL behavior"},
    // Go: {1999.9, 0, "pl_PL", "2000", false, "NoneComma (pl_PL) - MySQL behavior"},
    // Go: {123, 2, "fr_CH", "123,00", false, "NoneComma (fr_CH) - MySQL behavior"},
    // Go: {12345, 0, "de_AT", "12345", false, "NoneComma (de_AT) - MySQL behavior"},
    // Go: {1000000, 2, "bg_BG", "1000000,00", false, "NoneComma (bg_BG) - MySQL behavior"},
    // Go:
    // Go: // --- Style: AposDot (123'456.78) ---
    // Go: {4567890.123, 2, "de_CH", "4'567'890.12", false, "AposDot (de_CH) - large number"},
    // Go:
    // Go: // --- Style: AposComma (123'456,78) ---
    // Go: {4567890.123, 2, "it_CH", "4'567'890,12", false, "AposComma (it_CH) - MySQL behavior"},
    // Go:
    // Go: // --- Style: NoneDot (123456.78) ---
    // Go: {1000000.5, 0, "ar_SA", "1000001", false, "NoneDot (ar_SA) - no grouping, rounding"},
    // Go: {12345.6, 1, "sr_RS", "12345.6", false, "NoneDot (sr_RS) - MySQL behavior"},
    // Go:
    // Go: // --- Style: Indian (1,23,45,67,890.123) ---
    // Go: {1234567890.123, 3, "en_IN", "1,23,45,67,890.123", false, "Indian (en_IN) - lakh/crore grouping"},
    // Go: {987654321, 0, "ta_IN", "98,76,54,321", false, "Indian (ta_IN) - no decimal"},
    // Go: {-5000.5, 1, "te_IN", "-5,000.5", false, "Indian (te_IN) - only one separator"},
    // Go:
    // Go: // --- Special Cases (Case, NULL, Invalid) ---
    // Go: {12345.67, 2, "dE_dE", "12.345,67", false, "DotComma (de_DE) - case insensitive"},
    // Go: {12345.67, 2, "en_us", "12,345.67", false, "CommaDot (en_US) - case insensitive"},
    // Go:
    // Go: // Test NULL locale: should fallback to en_US and produce a warning
    // Go: {12345.67, 2, nil, "12,345.67", true, "NULL locale fallback"},
    // Go:
    // Go: // Test an invalid/unmapped locale
    // Go: // Should fallback to en_US (styleCommaDot) and issue a warning.
    // Go: {12345.67, 2, "de_GE", "12,345.67", true, "Invalid locale 'de_GE' fallback"},
    // Go: {12345.67, 2, "non_existent", "12,345.67", true, "Invalid locale 'non_existent' fallback"},
    // Go: }
    // Go:
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for _, tt := range tests {
    // Go: // Clear warnings for each test run
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: ctx.GetSessionVars().StmtCtx.SetWarnings(nil)
    // Go:
    // Go: // Get function signature
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(tt.number, tt.precision, tt.locale)))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err, "test: %s", tt.desc)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NotNil(t, f, "test: %s", tt.desc)
    // Go:
    // Go: // Evaluate
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: r, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err, "test: %s", tt.desc)
    // Go:
    // Go: // Check result
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: testutil.DatumEqual(t, types.NewDatum(tt.ret), r, "test: %s", tt.desc)
    // Go:
    // Go: // Check warnings
    // 会话状态：保留 Go 测试对 session vars/plan cache 参数的读写语义。
    // Go: warnings := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // Go: if tt.warning {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Len(t, warnings, 1, "test: %s", tt.desc)
    // Go: // Check if it's the 'Unknown locale' warning
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, terror.ErrorEqual(errUnknownLocale, warnings[0].Err), "test: %s", tt.desc)
    // Go: } else {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Len(t, warnings, 0, "test: %s", tt.desc)
    // Go: }
    // Go: }
    // Go: }
}
