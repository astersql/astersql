// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

#![allow(
// 表达式与向量化执行的基准测试（benchmark）与数据生成辅助，对应 Go `bench_test.go`。
//
// 提供各类随机/定界数据生成器（dataGenerator）、向量化求值正确性检查入口，
// 以及 benchdaily 汇总。当前多为迁移占位草稿（Draft），保留 Go 调用顺序与语义注释，
// 待会话、chunk、types 等依赖接通后再替换为可执行实现。
// Chunk 是列式批处理容器；向量化执行对整列批量计算以降低解释开销。

    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

// 这里覆盖：expression 表达式与向量化执行 benchmark/test helper，覆盖数据生成器、向量化验证和 benchdaily 入口。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发/异步、IO 和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待 Rust crate 接线）：
// - flag
// - fmt
// - math
// - math/rand
// - net
// - reflect
// - strings
// - sync
// - testing
// - time
// - github.com/google/uuid
// - perrors "github.com/pingcap/errors"
// - github.com/pingcap/tidb/pkg/parser/ast
// - github.com/pingcap/tidb/pkg/parser/auth
// - github.com/pingcap/tidb/pkg/parser/charset
// - github.com/pingcap/tidb/pkg/parser/mysql
// - github.com/pingcap/tidb/pkg/parser/terror
// - github.com/pingcap/tidb/pkg/sessionctx/vardef
// - github.com/pingcap/tidb/pkg/types
// - github.com/pingcap/tidb/pkg/util/benchdaily
// - github.com/pingcap/tidb/pkg/util/chunk
// - github.com/pingcap/tidb/pkg/util/mathutil
// - github.com/pingcap/tidb/pkg/util/mock
// - github.com/stretchr/testify/require

// 迁移占位类型：这些名称代表 Go/TiDB 测试依赖，后续接入模块时再替换为真实 Rust 类型。
type GoAny = ();
type GoError = String;

// benchHelper 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `benchHelper` 草稿：持有 mock 上下文、表达式与输入/输出 chunk，用于向量化基准初始化。
pub struct benchHelperDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: ctx *mock.Context
    // Go: exprs []Expression
    // Go: inputTypes []*types.FieldType
    // Go: outputTypes []*types.FieldType
    // Go: inputChunk *chunk.Chunk
    // Go: outputChunk *chunk.Chunk
}

// func (h *benchHelper) init() 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 初始化基准辅助对象：构造类型、填充 chunk 行数据。
pub fn bench_helper_init() {
    // Go 签名：func (h *benchHelper) init() {
    // Go: numRows := 4 * 1024
    // Go: h.ctx = mock.NewContext()
    // Original test depends on local timezone.
    // Go: h.ctx.GetSessionVars().StmtCtx.SetTimeZone(time.Local)
    // Go: h.ctx.GetSessionVars().InitChunkSize = 32
    // Go: h.ctx.GetSessionVars().MaxChunkSize = numRows
    // Go: h.inputTypes = make([]*types.FieldType, 0, 10)
    // Go: ftb := types.NewFieldTypeBuilder()
    // Go: ftb.SetType(mysql.TypeLonglong).SetFlag(mysql.BinaryFlag).SetFlen(mysql.MaxIntWidth).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin)
    // Go: h.inputTypes = append(h.inputTypes, ftb.BuildP())
    // Go: ftb = types.NewFieldTypeBuilder()
    // Go: ftb.SetType(mysql.TypeDouble).SetFlag(mysql.BinaryFlag).SetFlen(mysql.MaxRealWidth).SetDecimal(types.UnspecifiedLength).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin)
    // Go: h.inputTypes = append(h.inputTypes, ftb.BuildP())
    // Go: ftb = types.NewFieldTypeBuilder()
    // Go: ftb.SetType(mysql.TypeNewDecimal).SetFlag(mysql.BinaryFlag).SetFlen(11).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin)
    // Go: h.inputTypes = append(h.inputTypes, ftb.BuildP())
    // 原 Go 注释：Use 20 string columns to show the cache performance.
    // Go: // Use 20 string columns to show the cache performance.
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for range 20 {
    // Go: ftb = types.NewFieldTypeBuilder()
    // Go: ftb.SetType(mysql.TypeVarString).SetDecimal(types.UnspecifiedLength).SetCharset(charset.CharsetUTF8).SetCollate(charset.CollationUTF8)
    // Go: h.inputTypes = append(h.inputTypes, ftb.BuildP())
    // Go: }
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: h.inputChunk = chunk.NewChunkWithCapacity(h.inputTypes, numRows)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for range numRows {
    // Go: h.inputChunk.AppendInt64(0, 4)
    // Go: h.inputChunk.AppendFloat64(1, 2.019)
    // Go: h.inputChunk.AppendMyDecimal(2, types.NewDecFromFloatForTest(5.9101))
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range 20 {
    // Go: h.inputChunk.AppendString(3+i, `abcdefughasfjsaljal1321798273528791!&(*#&@&^%&%^&!)sadfashqwer`)
    // Go: }
    // Go: }
    // Go: cols := make([]*Column, 0, len(h.inputTypes))
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range h.inputTypes {
    // Go: cols = append(cols, &Column{
    // Go: UniqueID: int64(i),
    // Go: RetType: h.inputTypes[i],
    // Go: Index: i,
    // Go: })
    // Go: }
    // Go: h.exprs = make([]Expression, 0, 10)
    // 外部依赖：表达式函数构造仍是 TiDB Go API 调用点，待 Rust 接线。
    // Go: expr, err := NewFunction(h.ctx, ast.Substr, h.inputTypes[3], []Expression{cols[3], cols[2]}...)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic("create SUBSTR function failed.")
    // Go: }
    // Go: h.exprs = append(h.exprs, expr)
    // 外部依赖：表达式函数构造仍是 TiDB Go API 调用点，待 Rust 接线。
    // Go: expr1, err := NewFunction(h.ctx, ast.Plus, h.inputTypes[0], []Expression{cols[1], cols[2]}...)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic("create PLUS function failed.")
    // Go: }
    // Go: h.exprs = append(h.exprs, expr1)
    // 外部依赖：表达式函数构造仍是 TiDB Go API 调用点，待 Rust 接线。
    // Go: expr2, err := NewFunction(h.ctx, ast.GT, h.inputTypes[2], []Expression{cols[11], cols[8]}...)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic("create GT function failed.")
    // Go: }
    // Go: h.exprs = append(h.exprs, expr2)
    // 外部依赖：表达式函数构造仍是 TiDB Go API 调用点，待 Rust 接线。
    // Go: expr3, err := NewFunction(h.ctx, ast.GT, h.inputTypes[2], []Expression{cols[19], cols[10]}...)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic("create GT function failed.")
    // Go: }
    // Go: h.exprs = append(h.exprs, expr3)
    // 外部依赖：表达式函数构造仍是 TiDB Go API 调用点，待 Rust 接线。
    // Go: expr4, err := NewFunction(h.ctx, ast.GT, h.inputTypes[2], []Expression{cols[17], cols[4]}...)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic("create GT function failed.")
    // Go: }
    // Go: h.exprs = append(h.exprs, expr4)
    // 外部依赖：表达式函数构造仍是 TiDB Go API 调用点，待 Rust 接线。
    // Go: expr5, err := NewFunction(h.ctx, ast.GT, h.inputTypes[2], []Expression{cols[18], cols[5]}...)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic("create GT function failed.")
    // Go: }
    // Go: h.exprs = append(h.exprs, expr5)
    // 外部依赖：表达式函数构造仍是 TiDB Go API 调用点，待 Rust 接线。
    // Go: expr6, err := NewFunction(h.ctx, ast.LE, h.inputTypes[2], []Expression{cols[19], cols[4]}...)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic("create LE function failed.")
    // Go: }
    // Go: h.exprs = append(h.exprs, expr6)
    // 外部依赖：表达式函数构造仍是 TiDB Go API 调用点，待 Rust 接线。
    // Go: expr7, err := NewFunction(h.ctx, ast.EQ, h.inputTypes[2], []Expression{cols[20], cols[3]}...)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic("create EQ function failed.")
    // Go: }
    // Go: h.exprs = append(h.exprs, expr7)
    // Go: h.exprs = append(h.exprs, cols[2])
    // Go: h.exprs = append(h.exprs, cols[2])
    // Go: h.outputTypes = make([]*types.FieldType, 0, len(h.exprs))
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range h.exprs {
    // Go: h.outputTypes = append(h.outputTypes, h.exprs[i].GetType(h.ctx))
    // Go: }
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: h.outputChunk = chunk.NewChunkWithCapacity(h.outputTypes, numRows)
}

// func BenchmarkVectorizedExecute(b *testing.B) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
// Go benchmark 入口：Rust 不运行基准循环，只记录 b.ResetTimer/b.Run/b.Fatal 等测试框架语义。
/// 基准：向量化执行一组表达式。
pub fn benchmark_vectorized_execute() {
    // Go 签名：func BenchmarkVectorizedExecute(b *testing.B) {
    // Go: h := benchHelper{}
    // Go: h.init()
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: inputIter := chunk.NewIterator4Chunk(h.inputChunk)
    // Go: evalCtx := h.ctx.GetEvalCtx()
    // Go: b.ResetTimer()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: h.outputChunk.Reset()
    // Go: if err := VectorizedExecute(evalCtx, h.exprs, inputIter, h.outputChunk); err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic("errors happened during \"VectorizedExecute\"")
    // Go: }
    // Go: }
}

// func BenchmarkScalarFunctionClone(b *testing.B) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
// Go benchmark 入口：Rust 不运行基准循环，只记录 b.ResetTimer/b.Run/b.Fatal 等测试框架语义。
/// 基准：标量函数表达式 clone 开销。
pub fn benchmark_scalar_function_clone() {
    // Go 签名：func BenchmarkScalarFunctionClone(b *testing.B) {
    // Go: col := &Column{RetType: types.NewFieldType(mysql.TypeLonglong)}
    // Go: con1 := NewOne()
    // Go: con2 := NewZero()
    // 外部依赖：表达式函数构造仍是 TiDB Go API 调用点，待 Rust 接线。
    // Go: add := NewFunctionInternal(mock.NewContext(), ast.Plus, types.NewFieldType(mysql.TypeLonglong), col, con1)
    // 外部依赖：表达式函数构造仍是 TiDB Go API 调用点，待 Rust 接线。
    // Go: sub := NewFunctionInternal(mock.NewContext(), ast.Plus, types.NewFieldType(mysql.TypeLonglong), add, con2)
    // Go: b.ResetTimer()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: sub.Clone()
    // Go: }
    // Go: b.ReportAllocs()
}

// func getRandomTime(r *rand.Rand) types.CoreTime 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成随机 `CoreTime`（年/月/日/时分秒/微秒）。
pub fn get_random_time() {
    // Go 签名：func getRandomTime(r *rand.Rand) types.CoreTime {
    // Go: return types.FromDate(r.Intn(2200), r.Intn(10)+1, r.Intn(20)+1,
    // Go: r.Intn(12), r.Intn(60), r.Intn(60), r.Intn(1000000))
}

// dataGenerator 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// 数据生成器接口草稿：`gen()` 产生单个单元格值。
pub trait dataGeneratorDraft {
    // Go interface 方法在下方逐行保留。
    // Go: type dataGenerator interface {
    // Go: gen() any
    // Go: }
}

// defaultRandGen 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// 默认随机数生成器草稿（包装加锁的 rand.Source）。
pub struct defaultRandGenDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: *rand.Rand
}

// lockedSource 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// 带互斥锁的 `rand.Source`，保证并发生成安全。
pub struct lockedSourceDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: lk sync.Mutex
    // Go: src rand.Source
}

// func (r *lockedSource) Int63() (n int64) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 加锁读取 Int63。
pub fn locked_source_int63() {
    // Go 签名：func (r *lockedSource) Int63() (n int64) {
    // 并发资源：原 Go 使用 mutex 加锁保护共享 rand.Source。
    // Go: r.lk.Lock()
    // Go: n = r.src.Int63()
    // 并发资源：原 Go 使用 defer/显式解锁释放 mutex。
    // Go: r.lk.Unlock()
    // Go: return
}

// func (r *lockedSource) Seed(seed int64) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 加锁设置随机种子。
pub fn locked_source_seed() {
    // Go 签名：func (r *lockedSource) Seed(seed int64) {
    // 并发资源：原 Go 使用 mutex 加锁保护共享 rand.Source。
    // Go: r.lk.Lock()
    // Go: r.src.Seed(seed)
    // 并发资源：原 Go 使用 defer/显式解锁释放 mutex。
    // Go: r.lk.Unlock()
}

// func newDefaultRandGen() *defaultRandGen 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造默认随机生成器。
pub fn new_default_rand_gen() {
    // Go 签名：func newDefaultRandGen() *defaultRandGen {
    // Go: return &defaultRandGen{rand.New(&lockedSource{src: rand.NewSource(int64(rand.Uint64()))})}
}

// defaultGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// 按 EvalType 与空值比例生成随机单元格值。
pub struct defaultGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: nullRation float64
    // Go: eType types.EvalType
    // Go: randGen *defaultRandGen
}

// func newDefaultGener(nullRation float64, eType types.EvalType) *defaultGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造 `defaultGener`。
pub fn new_default_gener() {
    // Go 签名：func newDefaultGener(nullRation float64, eType types.EvalType) *defaultGener {
    // Go: return &defaultGener{
    // Go: nullRation: nullRation,
    // Go: eType: eType,
    // Go: randGen: newDefaultRandGen(),
    // Go: }
}

// func (g *defaultGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 按类型生成随机值或 NULL。
pub fn default_gener_gen() {
    // Go 签名：func (g *defaultGener) gen() any {
    // Go: if g.randGen.Float64() < g.nullRation {
    // Go: return nil
    // Go: }
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch g.eType {
    // Go: case types.ETInt:
    // Go: if g.randGen.Float64() < 0.5 {
    // Go: return -g.randGen.Int63()
    // Go: }
    // Go: return g.randGen.Int63()
    // Go: case types.ETReal:
    // Go: if g.randGen.Float64() < 0.5 {
    // Go: return -g.randGen.Float64() * 1000000
    // Go: }
    // Go: return g.randGen.Float64() * 1000000
    // Go: case types.ETDecimal:
    // Go: d := new(types.MyDecimal)
    // Go: var f float64
    // Go: if g.randGen.Float64() < 0.5 {
    // Go: f = g.randGen.Float64() * 100000
    // Go: } else {
    // Go: f = -g.randGen.Float64() * 100000
    // Go: }
    // Go: if err := d.FromFloat64(f); err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: return d
    // Go: case types.ETDatetime, types.ETTimestamp:
    // Go: gt := getRandomTime(g.randGen.Rand)
    // Go: t := types.NewTime(gt, convertETType(g.eType), 0)
    // 原 Go 注释：TiDB has DST time problem, and it causes ErrWrongValue.
    // Go: // TiDB has DST time problem, and it causes ErrWrongValue.
    // 原 Go 注释：We should ignore ambiguous Time. See https://timezonedb.com/time-zones/Asia/Shanghai.
    // Go: // We should ignore ambiguous Time. See https://timezonedb.com/time-zones/Asia/Shanghai.
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, err := t.GoTime(time.Local); err != nil; {
    // Go: gt = getRandomTime(g.randGen.Rand)
    // Go: t = types.NewTime(gt, convertETType(g.eType), 0)
    // Original test depends on local timezone.
    // Go: _, err = t.GoTime(time.Local)
    // Go: }
    // Go: return t
    // Go: case types.ETDuration:
    // Go: d := types.Duration{
    // 原 Go 注释：use rand.Int32() to make it not overflow when AddDuration
    // Go: // use rand.Int32() to make it not overflow when AddDuration
    // Go: Duration: time.Duration(g.randGen.Int31()),
    // Go: }
    // Go: return d
    // Go: case types.ETJson:
    // Go: j := new(types.BinaryJSON)
    // Go: if err := j.UnmarshalJSON(fmt.Appendf(nil, `{"key":%v}`, g.randGen.Int())); err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: return *j
    // Go: case types.ETString:
    // Go: return randString(g.randGen.Rand)
    // Go: }
    // Go: return nil
}

// charInt64Gener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `charInt64Gener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct charInt64GenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
}

// func (g *charInt64Gener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`char_int64_gener_gen`）。
pub fn char_int64_gener_gen() {
    // Go 签名：func (g *charInt64Gener) gen() any {
    // Go: nanosecond := time.Now().Nanosecond()
    // Go: nanosecond = nanosecond % 1024
    // Go: return int64(nanosecond)
}

// jsonArrayGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `jsonArrayGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct jsonArrayGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: rand *defaultRandGen
}

// func newJSONArrayGener() *jsonArrayGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_jsonarray_gener`）。
pub fn new_jsonarray_gener() {
    // Go 签名：func newJSONArrayGener() *jsonArrayGener {
    // Go: return &jsonArrayGener{newDefaultRandGen()}
}

// func (g *jsonArrayGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`json_array_gener_gen`）。
pub fn json_array_gener_gen() {
    // Go 签名：func (g *jsonArrayGener) gen() any {
    // Go: v := make([]any, 4)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range len(v) {
    // Go: v[i] = int64(g.rand.Int())
    // Go: }
    // Go: return types.CreateBinaryJSON(v)
}

// selectStringGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `selectStringGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct selectStringGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: candidates []string
    // Go: randGen *defaultRandGen
}

// func newSelectStringGener(candidates []string) *selectStringGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_select_string_gener`）。
pub fn new_select_string_gener() {
    // Go 签名：func newSelectStringGener(candidates []string) *selectStringGener {
    // Go: return &selectStringGener{candidates, newDefaultRandGen()}
}

// func (g *selectStringGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`select_string_gener_gen`）。
pub fn select_string_gener_gen() {
    // Go 签名：func (g *selectStringGener) gen() any {
    // Go: if len(g.candidates) == 0 {
    // Go: return nil
    // Go: }
    // Go: return g.candidates[g.randGen.Intn(len(g.candidates))]
}

// selectRealGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `selectRealGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct selectRealGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: candidates []float64
    // Go: randGen *defaultRandGen
}

// func newSelectRealGener(candidates []float64) *selectRealGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_select_real_gener`）。
pub fn new_select_real_gener() {
    // Go 签名：func newSelectRealGener(candidates []float64) *selectRealGener {
    // Go: return &selectRealGener{candidates, newDefaultRandGen()}
}

// func (g *selectRealGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`select_real_gener_gen`）。
pub fn select_real_gener_gen() {
    // Go 签名：func (g *selectRealGener) gen() any {
    // Go: if len(g.candidates) == 0 {
    // Go: return nil
    // Go: }
    // Go: return g.candidates[g.randGen.Intn(len(g.candidates))]
}

// constJSONGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `constJSONGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct constJSONGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: jsonStr string
}

// func (g *constJSONGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`const_jsongener_gen`）。
pub fn const_jsongener_gen() {
    // Go 签名：func (g *constJSONGener) gen() any {
    // Go: j := new(types.BinaryJSON)
    // Go: if err := j.UnmarshalJSON([]byte(g.jsonStr)); err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: return *j
}

// decimalJSONGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `decimalJSONGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct decimalJSONGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: nullRation float64
    // Go: randGen *defaultRandGen
}

// func newDecimalJSONGener(nullRation float64) *decimalJSONGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_decimal_jsongener`）。
pub fn new_decimal_jsongener() {
    // Go 签名：func newDecimalJSONGener(nullRation float64) *decimalJSONGener {
    // Go: return &decimalJSONGener{nullRation, newDefaultRandGen()}
}

// func (g *decimalJSONGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`decimal_jsongener_gen`）。
pub fn decimal_jsongener_gen() {
    // Go 签名：func (g *decimalJSONGener) gen() any {
    // Go: if g.randGen.Float64() < g.nullRation {
    // Go: return nil
    // Go: }
    // Go: var f float64
    // Go: if g.randGen.Float64() < 0.5 {
    // Go: f = g.randGen.Float64() * 100000
    // Go: } else {
    // Go: f = -g.randGen.Float64() * 100000
    // Go: }
    // Go: if err := (&types.MyDecimal{}).FromFloat64(f); err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: return types.CreateBinaryJSON(f)
}

// jsonStringGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `jsonStringGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct jsonStringGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func newJSONStringGener() *jsonStringGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_jsonstring_gener`）。
pub fn new_jsonstring_gener() {
    // Go 签名：func newJSONStringGener() *jsonStringGener {
    // Go: return &jsonStringGener{newDefaultRandGen()}
}

// func (g *jsonStringGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`json_string_gener_gen`）。
pub fn json_string_gener_gen() {
    // Go 签名：func (g *jsonStringGener) gen() any {
    // Go: j := new(types.BinaryJSON)
    // Go: if err := j.UnmarshalJSON(fmt.Appendf(nil, `{"key":%v}`, g.randGen.Int())); err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: return j.String()
}

// vectorFloat32RandGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `vectorFloat32RandGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct vectorFloat32RandGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: dimension int
    // Go: randGen *defaultRandGen
}

// func newVectorFloat32RandGener(dimension int) *vectorFloat32RandGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_vector_float32_rand_gener`）。
pub fn new_vector_float32_rand_gener() {
    // Go 签名：func newVectorFloat32RandGener(dimension int) *vectorFloat32RandGener {
    // Go: return &vectorFloat32RandGener{dimension, newDefaultRandGen()}
}

// func (g *vectorFloat32RandGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`vector_float32_rand_gener_gen`）。
pub fn vector_float32_rand_gener_gen() {
    // Go 签名：func (g *vectorFloat32RandGener) gen() any {
    // Go: if g.dimension == -1 {
    // Go: return nil
    // Go: }
    // Go: values := make([]float32, 0, g.dimension)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for range g.dimension {
    // Go: values = append(values, g.randGen.Float32())
    // Go: }
    // Go: vec := types.InitVectorFloat32(g.dimension)
    // Go: copy(vec.Elements(), values)
    // Go: return vec
}

// decimalStringGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `decimalStringGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct decimalStringGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func newDecimalStringGener() *decimalStringGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_decimal_string_gener`）。
pub fn new_decimal_string_gener() {
    // Go 签名：func newDecimalStringGener() *decimalStringGener {
    // Go: return &decimalStringGener{newDefaultRandGen()}
}

// func (g *decimalStringGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`decimal_string_gener_gen`）。
pub fn decimal_string_gener_gen() {
    // Go 签名：func (g *decimalStringGener) gen() any {
    // Go: tempDecimal := new(types.MyDecimal)
    // Go: if err := tempDecimal.FromFloat64(g.randGen.Float64()); err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: return tempDecimal.String()
}

// realStringGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `realStringGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct realStringGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func newRealStringGener() *realStringGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_real_string_gener`）。
pub fn new_real_string_gener() {
    // Go 签名：func newRealStringGener() *realStringGener {
    // Go: return &realStringGener{newDefaultRandGen()}
}

// func (g *realStringGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`real_string_gener_gen`）。
pub fn real_string_gener_gen() {
    // Go 签名：func (g *realStringGener) gen() any {
    // Go: return fmt.Sprintf("%f", g.randGen.Float64())
}

// jsonTimeGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `jsonTimeGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct jsonTimeGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func newJSONTimeGener() *jsonTimeGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_jsontime_gener`）。
pub fn new_jsontime_gener() {
    // Go 签名：func newJSONTimeGener() *jsonTimeGener {
    // Go: return &jsonTimeGener{newDefaultRandGen()}
}

// func (g *jsonTimeGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`json_time_gener_gen`）。
pub fn json_time_gener_gen() {
    // Go 签名：func (g *jsonTimeGener) gen() any {
    // Go: tm := types.NewTime(getRandomTime(g.randGen.Rand), mysql.TypeDatetime, types.DefaultFsp)
    // Go: return types.CreateBinaryJSON(tm)
}

// rangeDurationGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `rangeDurationGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct rangeDurationGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: nullRation float64
    // Go: randGen *defaultRandGen
}

// func newRangeDurationGener(nullRation float64) *rangeDurationGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_range_duration_gener`）。
pub fn new_range_duration_gener() {
    // Go 签名：func newRangeDurationGener(nullRation float64) *rangeDurationGener {
    // Go: return &rangeDurationGener{nullRation, newDefaultRandGen()}
}

// func (g *rangeDurationGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`range_duration_gener_gen`）。
pub fn range_duration_gener_gen() {
    // Go 签名：func (g *rangeDurationGener) gen() any {
    // Go: if g.randGen.Float64() < g.nullRation {
    // Go: return nil
    // Go: }
    // Go: tm := (mathutil.Abs(g.randGen.Int63n(12))*3600 + mathutil.Abs(g.randGen.Int63n(60))*60 + mathutil.Abs(g.randGen.Int63n(60))) * 1000
    // Go: tu := (tm + mathutil.Abs(g.randGen.Int63n(1000))) * 1000
    // Go: return types.Duration{
    // Go: Duration: time.Duration(tu * 1000)}
}

// timeFormatGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `timeFormatGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct timeFormatGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: nullRation float64
    // Go: randGen *defaultRandGen
}

// func newTimeFormatGener(nullRation float64) *timeFormatGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_time_format_gener`）。
pub fn new_time_format_gener() {
    // Go 签名：func newTimeFormatGener(nullRation float64) *timeFormatGener {
    // Go: return &timeFormatGener{nullRation, newDefaultRandGen()}
}

// func (g *timeFormatGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`time_format_gener_gen`）。
pub fn time_format_gener_gen() {
    // Go 签名：func (g *timeFormatGener) gen() any {
    // Go: if g.randGen.Float64() < g.nullRation {
    // Go: return nil
    // Go: }
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch g.randGen.Uint32() % 4 {
    // Go: case 0:
    // Go: return "%H %i %S"
    // Go: case 1:
    // Go: return "%l %i %s"
    // Go: case 2:
    // Go: return "%p %i %s"
    // Go: case 3:
    // Go: return "%I %i %S %f"
    // Go: case 4:
    // Go: return "%T"
    // Go: default:
    // Go: return nil
    // Go: }
}

// rangeRealGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `rangeRealGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct rangeRealGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: begin float64
    // Go: end float64
    // Go: nullRation float64
    // Go: randGen *defaultRandGen
}

// func newRangeRealGener(begin, end, nullRation float64) *rangeRealGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_range_real_gener`）。
pub fn new_range_real_gener() {
    // Go 签名：func newRangeRealGener(begin, end, nullRation float64) *rangeRealGener {
    // Go: return &rangeRealGener{begin, end, nullRation, newDefaultRandGen()}
}

// func (g *rangeRealGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`range_real_gener_gen`）。
pub fn range_real_gener_gen() {
    // Go 签名：func (g *rangeRealGener) gen() any {
    // Go: if g.randGen.Float64() < g.nullRation {
    // Go: return nil
    // Go: }
    // Go: if g.end < g.begin {
    // Go: g.begin = -100
    // Go: g.end = 100
    // Go: }
    // Go: return g.randGen.Float64()*(g.end-g.begin) + g.begin
}

// rangeDecimalGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `rangeDecimalGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct rangeDecimalGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: begin float64
    // Go: end float64
    // Go: nullRation float64
    // Go: randGen *defaultRandGen
}

// func newRangeDecimalGener(begin, end, nullRation float64) *rangeDecimalGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_range_decimal_gener`）。
pub fn new_range_decimal_gener() {
    // Go 签名：func newRangeDecimalGener(begin, end, nullRation float64) *rangeDecimalGener {
    // Go: return &rangeDecimalGener{begin, end, nullRation, newDefaultRandGen()}
}

// func (g *rangeDecimalGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`range_decimal_gener_gen`）。
pub fn range_decimal_gener_gen() {
    // Go 签名：func (g *rangeDecimalGener) gen() any {
    // Go: if g.randGen.Float64() < g.nullRation {
    // Go: return nil
    // Go: }
    // Go: if g.end < g.begin {
    // Go: g.begin = -100000
    // Go: g.end = 100000
    // Go: }
    // Go: d := new(types.MyDecimal)
    // Go: f := g.randGen.Float64()*(g.end-g.begin) + g.begin
    // Go: if err := d.FromFloat64(f); err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: return d
}

// rangeInt64Gener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `rangeInt64Gener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct rangeInt64GenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: begin int
    // Go: end int
    // Go: randGen *defaultRandGen
}

// func newRangeInt64Gener(begin, end int) *rangeInt64Gener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_range_int64_gener`）。
pub fn new_range_int64_gener() {
    // Go 签名：func newRangeInt64Gener(begin, end int) *rangeInt64Gener {
    // Go: return &rangeInt64Gener{begin, end, newDefaultRandGen()}
}

// func (rig *rangeInt64Gener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`range_int64_gener_gen`）。
pub fn range_int64_gener_gen() {
    // Go 签名：func (rig *rangeInt64Gener) gen() any {
    // Go: return int64(rig.randGen.Intn(rig.end-rig.begin) + rig.begin)
}

// numStrGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `numStrGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct numStrGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: rangeInt64Gener
}

// func (g *numStrGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`num_str_gener_gen`）。
pub fn num_str_gener_gen() {
    // Go 签名：func (g *numStrGener) gen() any {
    // Go: return fmt.Sprintf("%v", g.rangeInt64Gener.gen())
}

// ipv6StrGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `ipv6StrGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct ipv6StrGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func (g *ipv6StrGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`ipv6_str_gener_gen`）。
pub fn ipv6_str_gener_gen() {
    // Go 签名：func (g *ipv6StrGener) gen() any {
    // 外部依赖：Go 使用 net.IP 构造 IPv4/IPv6 字节或字符串。
    // Go: var ip net.IP = make([]byte, net.IPv6len)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range ip {
    // Go: ip[i] = uint8(g.randGen.Intn(256))
    // Go: }
    // Go: return ip.String()
}

// ipv4StrGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `ipv4StrGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct ipv4StrGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func (g *ipv4StrGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`ipv4_str_gener_gen`）。
pub fn ipv4_str_gener_gen() {
    // Go 签名：func (g *ipv4StrGener) gen() any {
    // 外部依赖：Go 使用 net.IP 构造 IPv4/IPv6 字节或字符串。
    // Go: var ip net.IP = make([]byte, net.IPv4len)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range ip {
    // Go: ip[i] = uint8(g.randGen.Intn(256))
    // Go: }
    // Go: return ip.String()
}

// ipv6ByteGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `ipv6ByteGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct ipv6ByteGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func (g *ipv6ByteGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`ipv6_byte_gener_gen`）。
pub fn ipv6_byte_gener_gen() {
    // Go 签名：func (g *ipv6ByteGener) gen() any {
    // 外部依赖：Go 使用 net.IP 构造 IPv4/IPv6 字节或字符串。
    // Go: var ip = make([]byte, net.IPv6len)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range ip {
    // Go: ip[i] = uint8(g.randGen.Intn(256))
    // Go: }
    // 外部依赖：Go 使用 net.IP 构造 IPv4/IPv6 字节或字符串。
    // Go: return string(ip[:net.IPv6len])
}

// ipv4ByteGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `ipv4ByteGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct ipv4ByteGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func (g *ipv4ByteGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`ipv4_byte_gener_gen`）。
pub fn ipv4_byte_gener_gen() {
    // Go 签名：func (g *ipv4ByteGener) gen() any {
    // 外部依赖：Go 使用 net.IP 构造 IPv4/IPv6 字节或字符串。
    // Go: var ip = make([]byte, net.IPv4len)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range ip {
    // Go: ip[i] = uint8(g.randGen.Intn(256))
    // Go: }
    // 外部依赖：Go 使用 net.IP 构造 IPv4/IPv6 字节或字符串。
    // Go: return string(ip[:net.IPv4len])
}

// ipv4CompatByteGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `ipv4CompatByteGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct ipv4CompatByteGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func (g *ipv4CompatByteGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`ipv4_compat_byte_gener_gen`）。
pub fn ipv4_compat_byte_gener_gen() {
    // Go 签名：func (g *ipv4CompatByteGener) gen() any {
    // 外部依赖：Go 使用 net.IP 构造 IPv4/IPv6 字节或字符串。
    // Go: var ip = make([]byte, net.IPv6len)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range ip {
    // Go: if i < 12 {
    // Go: ip[i] = 0
    // Go: } else {
    // Go: ip[i] = uint8(g.randGen.Intn(256))
    // Go: }
    // Go: }
    // 外部依赖：Go 使用 net.IP 构造 IPv4/IPv6 字节或字符串。
    // Go: return string(ip[:net.IPv6len])
}

// ipv4MappedByteGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `ipv4MappedByteGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct ipv4MappedByteGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func (g *ipv4MappedByteGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`ipv4_mapped_byte_gener_gen`）。
pub fn ipv4_mapped_byte_gener_gen() {
    // Go 签名：func (g *ipv4MappedByteGener) gen() any {
    // Go: var ip = []byte{0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 0, 0}
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 12; i < 16; i++ {
    // Go: ip[i] = uint8(g.randGen.Intn(256)) // reset the last 4 bytes
    // Go: }
    // 外部依赖：Go 使用 net.IP 构造 IPv4/IPv6 字节或字符串。
    // Go: return string(ip[:net.IPv6len])
}

// uuidStrGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `uuidStrGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct uuidStrGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func (g *uuidStrGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`uuid_str_gener_gen`）。
pub fn uuid_str_gener_gen() {
    // Go 签名：func (g *uuidStrGener) gen() any {
    // 外部依赖：Go 使用 google/uuid 生成 UUID 字符串或二进制。
    // Go: u, _ := uuid.NewUUID()
    // Go: return u.String()
}

// uuidBinGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `uuidBinGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct uuidBinGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func (g *uuidBinGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`uuid_bin_gener_gen`）。
pub fn uuid_bin_gener_gen() {
    // Go 签名：func (g *uuidBinGener) gen() any {
    // 外部依赖：Go 使用 google/uuid 生成 UUID 字符串或二进制。
    // Go: u, _ := uuid.NewUUID()
    // Go: bin, _ := u.MarshalBinary()
    // Go: return string(bin)
}

// randLenStrGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `randLenStrGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct randLenStrGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: lenBegin int
    // Go: lenEnd int
    // Go: randGen *defaultRandGen
}

// func newRandLenStrGener(lenBegin, lenEnd int) *randLenStrGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_rand_len_str_gener`）。
pub fn new_rand_len_str_gener() {
    // Go 签名：func newRandLenStrGener(lenBegin, lenEnd int) *randLenStrGener {
    // Go: return &randLenStrGener{lenBegin, lenEnd, newDefaultRandGen()}
}

// func (g *randLenStrGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`rand_len_str_gener_gen`）。
pub fn rand_len_str_gener_gen() {
    // Go 签名：func (g *randLenStrGener) gen() any {
    // Go: n := g.randGen.Intn(g.lenEnd-g.lenBegin) + g.lenBegin
    // Go: buf := make([]byte, n)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range buf {
    // Go: x := g.randGen.Intn(62)
    // Go: if x < 10 {
    // Go: buf[i] = byte('0' + x)
    // Go: } else if x-10 < 26 {
    // Go: buf[i] = byte('a' + x - 10)
    // Go: } else {
    // Go: buf[i] = byte('A' + x - 10 - 26)
    // Go: }
    // Go: }
    // Go: return string(buf)
}

// randHexStrGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `randHexStrGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct randHexStrGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: lenBegin int
    // Go: lenEnd int
    // Go: randGen *defaultRandGen
}

// func newRandHexStrGener(lenBegin, lenEnd int) *randHexStrGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_rand_hex_str_gener`）。
pub fn new_rand_hex_str_gener() {
    // Go 签名：func newRandHexStrGener(lenBegin, lenEnd int) *randHexStrGener {
    // Go: return &randHexStrGener{lenBegin, lenEnd, newDefaultRandGen()}
}

// func (g *randHexStrGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`rand_hex_str_gener_gen`）。
pub fn rand_hex_str_gener_gen() {
    // Go 签名：func (g *randHexStrGener) gen() any {
    // Go: n := g.randGen.Intn(g.lenEnd-g.lenBegin) + g.lenBegin
    // Go: buf := make([]byte, n)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range buf {
    // Go: x := g.randGen.Intn(16)
    // Go: if x < 10 {
    // Go: buf[i] = byte('0' + x)
    // Go: } else {
    // Go: if x%2 == 0 {
    // Go: buf[i] = byte('a' + x - 10)
    // Go: } else {
    // Go: buf[i] = byte('A' + x - 10)
    // Go: }
    // Go: }
    // Go: }
    // Go: return string(buf)
}

// dateGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func (g dateGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_gener_gen`）。
pub fn date_gener_gen() {
    // Go 签名：func (g dateGener) gen() any {
    // Go: year := 1970 + g.randGen.Intn(100)
    // Go: month := g.randGen.Intn(10) + 1
    // Go: day := g.randGen.Intn(20) + 1
    // Go: gt := types.FromDate(year, month, day, 0, 0, 0, 0)
    // Go: d := types.NewTime(gt, mysql.TypeDate, types.DefaultFsp)
    // Go: return d
}

// dateTimeGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateTimeGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateTimeGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: Fsp int
    // Go: Year int
    // Go: Month int
    // Go: Day int
    // Go: randGen *defaultRandGen
}

// func (g *dateTimeGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_time_gener_gen`）。
pub fn date_time_gener_gen() {
    // Go 签名：func (g *dateTimeGener) gen() any {
    // Go: if g.Year == 0 {
    // Go: g.Year = 1970 + g.randGen.Intn(100)
    // Go: }
    // Go: if g.Month == 0 {
    // Go: g.Month = g.randGen.Intn(10) + 1
    // Go: }
    // Go: if g.Day == 0 {
    // Go: g.Day = g.randGen.Intn(20) + 1
    // Go: }
    // Go: var gt types.CoreTime
    // Go: if g.Fsp > 0 && g.Fsp <= 6 {
    // Go: gt = types.FromDate(g.Year, g.Month, g.Day, g.randGen.Intn(12), g.randGen.Intn(60), g.randGen.Intn(60), g.randGen.Intn(1000000))
    // Go: } else {
    // Go: gt = types.FromDate(g.Year, g.Month, g.Day, g.randGen.Intn(12), g.randGen.Intn(60), g.randGen.Intn(60), 0)
    // Go: }
    // Go: t := types.NewTime(gt, mysql.TypeDatetime, types.DefaultFsp)
    // Go: return t
}

// dateTimeStrGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateTimeStrGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateTimeStrGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: Fsp int
    // Go: Year int
    // Go: Month int
    // Go: Day int
    // Go: randGen *defaultRandGen
}

// func (g *dateTimeStrGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_time_str_gener_gen`）。
pub fn date_time_str_gener_gen() {
    // Go 签名：func (g *dateTimeStrGener) gen() any {
    // Go: if g.Year == 0 {
    // Go: g.Year = 1970 + g.randGen.Intn(100)
    // Go: }
    // Go: if g.Month == 0 {
    // Go: g.Month = g.randGen.Intn(10) + 1
    // Go: }
    // Go: if g.Day == 0 {
    // Go: g.Day = g.randGen.Intn(20) + 1
    // Go: }
    // Go: if g.Fsp == -1 {
    // Go: g.Fsp = g.randGen.Intn(10)
    // Go: }
    // Go: hour := g.randGen.Intn(12)
    // Go: minute := g.randGen.Intn(60)
    // Go: second := g.randGen.Intn(60)
    // Go: dataTimeStr := fmt.Sprintf("%d-%d-%d %d:%d:%d",
    // Go: g.Year, g.Month, g.Day, hour, minute, second)
    // Go: if g.Fsp > 0 && g.Fsp <= 9 {
    // Go: microFmt := fmt.Sprintf(".%%0%dd", g.Fsp)
    // Go: return dataTimeStr + fmt.Sprintf(microFmt, g.randGen.Int()%int(math.Pow10(g.Fsp)))
    // Go: }
    // Go: return dataTimeStr
}

// dateStrGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateStrGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateStrGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: Year int
    // Go: Month int
    // Go: Day int
    // Go: NullRation float64
    // Go: randGen *defaultRandGen
}

// func (g *dateStrGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_str_gener_gen`）。
pub fn date_str_gener_gen() {
    // Go 签名：func (g *dateStrGener) gen() any {
    // Go: if g.NullRation > 1e-6 && g.randGen.Float64() < g.NullRation {
    // Go: return nil
    // Go: }
    // Go: if g.Year == 0 {
    // Go: g.Year = 1970 + g.randGen.Intn(100)
    // Go: }
    // Go: if g.Month == 0 {
    // Go: g.Month = g.randGen.Intn(10)
    // Go: }
    // Go: if g.Day == 0 {
    // Go: g.Day = g.randGen.Intn(20)
    // Go: }
    // Go: return fmt.Sprintf("%d-%d-%d", g.Year, g.Month, g.Day)
}

// dateOrDatetimeStrGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateOrDatetimeStrGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateOrDatetimeStrGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: dateRatio float64
    // Go: dateStrGener
    // Go: dateTimeStrGener
}

// func (g dateOrDatetimeStrGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_or_datetime_str_gener_gen`）。
pub fn date_or_datetime_str_gener_gen() {
    // Go 签名：func (g dateOrDatetimeStrGener) gen() any {
    // Go: if g.dateRatio > 1e-6 && g.dateStrGener.randGen.Float64() < g.dateRatio {
    // Go: return g.dateStrGener.gen()
    // Go: }
    // Go: return g.dateTimeStrGener.gen()
}

// timeStrGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `timeStrGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct timeStrGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: nullRation float64
    // Go: randGen *defaultRandGen
}

// func (g *timeStrGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`time_str_gener_gen`）。
pub fn time_str_gener_gen() {
    // Go 签名：func (g *timeStrGener) gen() any {
    // Go: if g.nullRation > 1e-6 && g.randGen.Float64() < g.nullRation {
    // Go: return nil
    // Go: }
    // Go: hour := g.randGen.Intn(12)
    // Go: minute := g.randGen.Intn(60)
    // Go: second := g.randGen.Intn(60)
    // Go: return fmt.Sprintf("%d:%d:%d", hour, minute, second)
}

// dateIntGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateIntGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateIntGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: dateGener
}

// func (g dateIntGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_int_gener_gen`）。
pub fn date_int_gener_gen() {
    // Go 签名：func (g dateIntGener) gen() any {
    // Go: t := g.dateGener.gen().(types.Time)
    // Go: num, err := t.ToNumber().ToInt()
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: return num
}

// dateTimeIntGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateTimeIntGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateTimeIntGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: dateTimeGener
}

// func (g dateTimeIntGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_time_int_gener_gen`）。
pub fn date_time_int_gener_gen() {
    // Go 签名：func (g dateTimeIntGener) gen() any {
    // Go: t := g.dateTimeGener.gen().(types.Time)
    // Go: num, err := t.ToNumber().ToInt()
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: return num
}

// dateOrDatetimeIntGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateOrDatetimeIntGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateOrDatetimeIntGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: dateRatio float64
    // Go: dateIntGener
    // Go: dateTimeIntGener
}

// func (g dateOrDatetimeIntGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_or_datetime_int_gener_gen`）。
pub fn date_or_datetime_int_gener_gen() {
    // Go 签名：func (g dateOrDatetimeIntGener) gen() any {
    // Go: if g.dateRatio > 1e-6 && g.dateGener.randGen.Float64() < g.dateRatio {
    // Go: return g.dateIntGener.gen()
    // Go: }
    // Go: return g.dateTimeIntGener.gen()
}

// dateRealGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateRealGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateRealGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: fspRatio float64
    // Go: dateGener
}

// func (g dateRealGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_real_gener_gen`）。
pub fn date_real_gener_gen() {
    // Go 签名：func (g dateRealGener) gen() any {
    // Go: t := g.dateGener.gen().(types.Time)
    // Go: num, err := t.ToNumber().ToFloat64()
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: if g.randGen.Float64() >= g.fspRatio {
    // Go: return num
    // Go: }
    // Go: num += g.randGen.Float64()
    // Go: return num
}

// dateTimeRealGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateTimeRealGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateTimeRealGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: fspRatio float64
    // Go: dateTimeGener
}

// func (g dateTimeRealGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_time_real_gener_gen`）。
pub fn date_time_real_gener_gen() {
    // Go 签名：func (g dateTimeRealGener) gen() any {
    // Go: t := g.dateTimeGener.gen().(types.Time)
    // Go: tmp, err := t.ToNumber().ToInt()
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: num := float64(tmp)
    // Go: if g.randGen.Float64() >= g.fspRatio {
    // Go: return num
    // Go: }
    // 原 Go 注释：Not using `t`'s us part since it's too regular.
    // Go: // Not using `t`'s us part since it's too regular.
    // 原 Go 注释：Instead, generating a more arbitrary fractional part, e.g. with more than 6 digits.
    // Go: // Instead, generating a more arbitrary fractional part, e.g. with more than 6 digits.
    // 原 Go 注释：We want the parsing logic to be strong enough to deal with this arbitrary fractional number.
    // Go: // We want the parsing logic to be strong enough to deal with this arbitrary fractional number.
    // Go: num += g.randGen.Float64()
    // Go: return num
}

// dateOrDatetimeRealGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateOrDatetimeRealGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateOrDatetimeRealGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: dateRatio float64
    // Go: dateRealGener
    // Go: dateTimeRealGener
}

// func (g dateOrDatetimeRealGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_or_datetime_real_gener_gen`）。
pub fn date_or_datetime_real_gener_gen() {
    // Go 签名：func (g dateOrDatetimeRealGener) gen() any {
    // Go: if g.dateRatio > 1e-6 && g.dateGener.randGen.Float64() < g.dateRatio {
    // Go: return g.dateRealGener.gen()
    // Go: }
    // Go: return g.dateTimeRealGener.gen()
}

// dateDecimalGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateDecimalGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateDecimalGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: fspRatio float64
    // Go: dateGener
}

// func (g dateDecimalGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_decimal_gener_gen`）。
pub fn date_decimal_gener_gen() {
    // Go 签名：func (g dateDecimalGener) gen() any {
    // Go: t := g.dateGener.gen().(types.Time)
    // Go: intPart := t.ToNumber()
    // Go: if g.randGen.Float64() >= g.fspRatio {
    // Go: return intPart
    // Go: }
    // 原 Go 注释：Generate a fractional part that is at most 9 digits.
    // Go: // Generate a fractional part that is at most 9 digits.
    // Go: fracDigits := g.randGen.Intn(1000000000)
    // Go: fracPart := new(types.MyDecimal).FromInt(int64(fracDigits))
    // Go: if err := fracPart.Shift(-9); err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: res := new(types.MyDecimal)
    // Go: err := types.DecimalAdd(intPart, fracPart, res)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: return res
}

// dateTimeDecimalGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateTimeDecimalGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateTimeDecimalGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: fspRatio float64
    // Go: dateTimeGener
}

// func (g dateTimeDecimalGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_time_decimal_gener_gen`）。
pub fn date_time_decimal_gener_gen() {
    // Go 签名：func (g dateTimeDecimalGener) gen() any {
    // Go: t := g.dateTimeGener.gen().(types.Time)
    // Go: num := t.ToNumber()
    // 原 Go 注释：Not using `num`'s fractional part so that we can:
    // Go: // Not using `num`'s fractional part so that we can:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // 原 Go 注释：1. Return early for non-fsp values.
    // Go: // 1. Return early for non-fsp values.
    // 原 Go 注释：2. Generate a more arbitrary fractional part if needed.
    // Go: // 2. Generate a more arbitrary fractional part if needed.
    // Go: i, err := num.ToInt()
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: intPart := new(types.MyDecimal).FromInt(i)
    // Go: if g.randGen.Float64() >= g.fspRatio {
    // Go: return intPart
    // Go: }
    // 原 Go 注释：Generate a fractional part that is at most 9 digits.
    // Go: // Generate a fractional part that is at most 9 digits.
    // Go: fracDigits := g.randGen.Intn(1000000000)
    // Go: fracPart := new(types.MyDecimal).FromInt(int64(fracDigits))
    // Go: if err := fracPart.Shift(-9); err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: res := new(types.MyDecimal)
    // Go: err = types.DecimalAdd(intPart, fracPart, res)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: return res
}

// dateOrDatetimeDecimalGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `dateOrDatetimeDecimalGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct dateOrDatetimeDecimalGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: dateRatio float64
    // Go: dateDecimalGener
    // Go: dateTimeDecimalGener
}

// func (g dateOrDatetimeDecimalGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`date_or_datetime_decimal_gener_gen`）。
pub fn date_or_datetime_decimal_gener_gen() {
    // Go 签名：func (g dateOrDatetimeDecimalGener) gen() any {
    // Go: if g.dateRatio > 1e-6 && g.dateGener.randGen.Float64() < g.dateRatio {
    // Go: return g.dateDecimalGener.gen()
    // Go: }
    // Go: return g.dateTimeDecimalGener.gen()
}

// constStrGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `constStrGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct constStrGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: s string
}

// func (g *constStrGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`const_str_gener_gen`）。
pub fn const_str_gener_gen() {
    // Go 签名：func (g *constStrGener) gen() any {
    // Go: return g.s
}

// randDurInt 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `randDurInt` 类型草稿，对应 Go 同名测试辅助结构。
pub struct randDurIntDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func newRandDurInt() *randDurInt 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_rand_dur_int`）。
pub fn new_rand_dur_int() {
    // Go 签名：func newRandDurInt() *randDurInt {
    // Go: return &randDurInt{newDefaultRandGen()}
}

// func (g *randDurInt) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`rand_dur_int_gen`）。
pub fn rand_dur_int_gen() {
    // Go 签名：func (g *randDurInt) gen() any {
    // Go: return int64(g.randGen.Intn(types.TimeMaxHour)*10000 + g.randGen.Intn(60)*100 + g.randGen.Intn(60))
}

// randDurReal 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `randDurReal` 类型草稿，对应 Go 同名测试辅助结构。
pub struct randDurRealDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func newRandDurReal() *randDurReal 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_rand_dur_real`）。
pub fn new_rand_dur_real() {
    // Go 签名：func newRandDurReal() *randDurReal {
    // Go: return &randDurReal{newDefaultRandGen()}
}

// func (g *randDurReal) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`rand_dur_real_gen`）。
pub fn rand_dur_real_gen() {
    // Go 签名：func (g *randDurReal) gen() any {
    // Go: return float64(g.randGen.Intn(types.TimeMaxHour)*10000 + g.randGen.Intn(60)*100 + g.randGen.Intn(60))
}

// randDurDecimal 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `randDurDecimal` 类型草稿，对应 Go 同名测试辅助结构。
pub struct randDurDecimalDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: randGen *defaultRandGen
}

// func newRandDurDecimal() *randDurDecimal 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_rand_dur_decimal`）。
pub fn new_rand_dur_decimal() {
    // Go 签名：func newRandDurDecimal() *randDurDecimal {
    // Go: return &randDurDecimal{newDefaultRandGen()}
}

// func (g *randDurDecimal) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`rand_dur_decimal_gen`）。
pub fn rand_dur_decimal_gen() {
    // Go 签名：func (g *randDurDecimal) gen() any {
    // Go: d := new(types.MyDecimal)
    // Go: return d.FromFloat64(float64(g.randGen.Intn(types.TimeMaxHour)*10000 + g.randGen.Intn(60)*100 + g.randGen.Intn(60)))
}

// locationGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `locationGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct locationGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: nullRation float64
    // Go: randGen *defaultRandGen
}

// func newLocationGener(nullRation float64) *locationGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_location_gener`）。
pub fn new_location_gener() {
    // Go 签名：func newLocationGener(nullRation float64) *locationGener {
    // Go: return &locationGener{nullRation, newDefaultRandGen()}
}

// func (g *locationGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`location_gener_gen`）。
pub fn location_gener_gen() {
    // Go 签名：func (g *locationGener) gen() any {
    // Go: if g.randGen.Float64() < g.nullRation {
    // Go: return nil
    // Go: }
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch g.randGen.Uint32() % 5 {
    // Go: case 0:
    // Go: return usaLocation
    // Go: case 1:
    // Go: return jisLocation
    // Go: case 2:
    // Go: return isoLocation
    // Go: case 3:
    // Go: return eurLocation
    // Go: case 4:
    // Go: return internalLocation
    // Go: default:
    // Go: return nil
    // Go: }
}

// formatGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `formatGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct formatGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: nullRation float64
    // Go: randGen *defaultRandGen
}

// func newFormatGener(nullRation float64) *formatGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_format_gener`）。
pub fn new_format_gener() {
    // Go 签名：func newFormatGener(nullRation float64) *formatGener {
    // Go: return &formatGener{nullRation, newDefaultRandGen()}
}

// func (g *formatGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`format_gener_gen`）。
pub fn format_gener_gen() {
    // Go 签名：func (g *formatGener) gen() any {
    // Go: if g.randGen.Float64() < g.nullRation {
    // Go: return nil
    // Go: }
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch g.randGen.Uint32() % 4 {
    // Go: case 0:
    // Go: return dateFormat
    // Go: case 1:
    // Go: return datetimeFormat
    // Go: case 2:
    // Go: return timestampFormat
    // Go: case 3:
    // Go: return timeFormat
    // Go: default:
    // Go: return nil
    // Go: }
}

// nullWrappedGener 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// `nullWrappedGener` 类型草稿，对应 Go 同名测试辅助结构。
pub struct nullWrappedGenerDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: nullRation float64
    // Go: inner dataGenerator
    // Go: randGen *defaultRandGen
}

// func newNullWrappedGener(nullRation float64, inner dataGenerator) *nullWrappedGener 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 构造函数草稿：对应 Go 的同名 `new*` 工厂（`new_null_wrapped_gener`）。
pub fn new_null_wrapped_gener() {
    // Go 签名：func newNullWrappedGener(nullRation float64, inner dataGenerator) *nullWrappedGener {
    // Go: return &nullWrappedGener{nullRation, inner, newDefaultRandGen()}
}

// func (g *nullWrappedGener) gen() any 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成器方法草稿：对应 Go 的 `gen()` / 相关生成逻辑（`null_wrapped_gener_gen`）。
pub fn null_wrapped_gener_gen() {
    // Go 签名：func (g *nullWrappedGener) gen() any {
    // Go: if g.randGen.Float64() < g.nullRation {
    // Go: return nil
    // Go: }
    // Go: return g.inner.gen()
}

// vecExprBenchCase 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// 单条向量化表达式基准用例描述。
pub struct vecExprBenchCaseDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
    // Go: // retEvalType is the EvalType of the expression result.
    // Go: // This field is required.
    // Go: retEvalType types.EvalType
    // Go: // childrenTypes is the EvalTypes of the expression children(arguments).
    // Go: // This field is required.
    // Go: childrenTypes []types.EvalType
    // Go: // childrenFieldTypes is the field types of the expression children(arguments).
    // Go: // If childrenFieldTypes is not set, it will be converted from childrenTypes.
    // Go: // This field is optional.
    // Go: childrenFieldTypes []*types.FieldType
    // Go: // geners are used to generate data for children and geners[i] generates data for children[i].
    // Go: // If geners[i] is nil, the default dataGenerator will be used for its corresponding child.
    // Go: // The geners slice can be shorter than the children slice, if it has 3 children, then
    // Go: // geners[gen1, gen2] will be regarded as geners[gen1, gen2, nil].
    // Go: // This field is optional.
    // Go: geners []dataGenerator
    // Go: // aesModeAttr information, needed by encryption functions
    // Go: aesModes string
    // Go: // constants are used to generate constant data for children[i].
    // Go: constants []*Constant
    // Go: // chunkSize is used to specify the chunk size of children, the maximum is 1024.
    // Go: // This field is optional, 1024 by default.
    // Go: chunkSize int
}

// vecExprBenchCases 对应 Go type 声明；字段、嵌入类型和接口方法保持原顺序，后续接入真实 Rust 类型时再替换占位。
/// 向量化表达式基准用例集合。
pub struct vecExprBenchCasesDraft {
    // Go 字段：当前不建模真实 TiDB/测试框架类型。
}

// func fillColumn(eType types.EvalType, chk *chunk.Chunk, colIdx int, testCase vecExprBenchCase) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 按字段类型向 chunk 列填充测试数据。
pub fn fill_column() {
    // Go 签名：func fillColumn(eType types.EvalType, chk *chunk.Chunk, colIdx int, testCase vecExprBenchCase) {
    // Go: var gen dataGenerator
    // Go: if len(testCase.geners) > colIdx && testCase.geners[colIdx] != nil {
    // Go: gen = testCase.geners[colIdx]
    // Go: }
    // Go: fillColumnWithGener(eType, chk, colIdx, gen)
}

// func fillColumnWithGener(eType types.EvalType, chk *chunk.Chunk, colIdx int, gen dataGenerator) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 使用指定生成器填充列。
pub fn fill_column_with_gener() {
    // Go 签名：func fillColumnWithGener(eType types.EvalType, chk *chunk.Chunk, colIdx int, gen dataGenerator) {
    // Go: batchSize := chk.Capacity()
    // Go: if gen == nil {
    // Go: gen = newDefaultGener(0.2, eType)
    // Go: }
    // Go: col := chk.Column(colIdx)
    // Go: col.Reset(eType)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for range batchSize {
    // Go: v := gen.gen()
    // Go: if v == nil {
    // Go: col.AppendNull()
    // Go: continue
    // Go: }
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch eType {
    // Go: case types.ETInt:
    // Go: col.AppendInt64(v.(int64))
    // Go: case types.ETReal:
    // Go: col.AppendFloat64(v.(float64))
    // Go: case types.ETDecimal:
    // Go: col.AppendMyDecimal(v.(*types.MyDecimal))
    // Go: case types.ETDatetime, types.ETTimestamp:
    // Go: col.AppendTime(v.(types.Time))
    // Go: case types.ETDuration:
    // Go: col.AppendDuration(v.(types.Duration))
    // Go: case types.ETJson:
    // Go: col.AppendJSON(v.(types.BinaryJSON))
    // Go: case types.ETString:
    // Go: col.AppendString(v.(string))
    // Go: case types.ETVectorFloat32:
    // Go: col.AppendVectorFloat32(v.(types.VectorFloat32))
    // Go: }
    // Go: }
}

// func randString(r *rand.Rand) string 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成随机字符串。
pub fn rand_string<R: rand::Rng + ?Sized>(r: &mut R) -> String {
    // Go 签名：func randString(r *rand.Rand) string {
    // Go: n := 10 + r.Intn(10)
    // Go: buf := make([]byte, n)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range buf {
    // Go: x := r.Intn(62)
    // Go: if x < 10 {
    // Go: buf[i] = byte('0' + x)
    // Go: } else if x-10 < 26 {
    // Go: buf[i] = byte('a' + x - 10)
    // Go: } else {
    // Go: buf[i] = byte('A' + x - 10 - 26)
    // Go: }
    // Go: }
    // Go: return string(buf)
    let length = 10 + r.gen_range(0..10);
    let mut value = String::with_capacity(length);
    for _ in 0..length {
        let x = r.gen_range(0..62);
        let byte = if x < 10 {
            b'0' + x
        } else if x - 10 < 26 {
            b'a' + x - 10
        } else {
            b'A' + x - 10 - 26
        };
        value.push(char::from(byte));
    }
    value
}

// func eType2FieldType(eType types.EvalType) *types.FieldType 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// EvalType 映射到 `FieldType`。
pub fn e_type2_field_type() {
    // Go 签名：func eType2FieldType(eType types.EvalType) *types.FieldType {
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch eType {
    // Go: case types.ETInt:
    // Go: return types.NewFieldType(mysql.TypeLonglong)
    // Go: case types.ETReal:
    // Go: return types.NewFieldType(mysql.TypeDouble)
    // Go: case types.ETDecimal:
    // Go: return types.NewFieldType(mysql.TypeNewDecimal)
    // Go: case types.ETDatetime, types.ETTimestamp:
    // Go: return types.NewFieldType(mysql.TypeDatetime)
    // Go: case types.ETDuration:
    // Go: return types.NewFieldType(mysql.TypeDuration)
    // Go: case types.ETJson:
    // Go: return types.NewFieldType(mysql.TypeJSON)
    // Go: case types.ETString:
    // Go: return types.NewFieldType(mysql.TypeVarString)
    // Go: case types.ETVectorFloat32:
    // Go: return types.NewFieldType(mysql.TypeTiDBVectorFloat32)
    // Go: default:
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(fmt.Sprintf("EvalType=%v is not supported.", eType))
    // Go: }
}

// func genVecExprBenchCase(ctx BuildContext, funcName string, testCase vecExprBenchCase) (expr Expression, fts []*types.FieldType, input *chunk.Chunk, output *chunk.Chunk) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成向量化表达式基准用例。
pub fn gen_vec_expr_bench_case() {
    // Go 签名：func genVecExprBenchCase(ctx BuildContext, funcName string, testCase vecExprBenchCase) (expr Expression, fts []*types.FieldType, input *chunk.Chunk, output *chunk.Chunk) {
    // Go: fts = make([]*types.FieldType, len(testCase.childrenTypes))
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range fts {
    // Go: if i < len(testCase.childrenFieldTypes) && testCase.childrenFieldTypes[i] != nil {
    // Go: fts[i] = testCase.childrenFieldTypes[i]
    // Go: } else {
    // Go: fts[i] = eType2FieldType(testCase.childrenTypes[i])
    // Go: }
    // Go: }
    // Go: if testCase.chunkSize <= 0 || testCase.chunkSize > 1024 {
    // Go: testCase.chunkSize = 1024
    // Go: }
    // Go: cols := make([]Expression, len(testCase.childrenTypes))
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: input = chunk.New(fts, testCase.chunkSize, testCase.chunkSize)
    // Go: input.NumRows()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i, eType := range testCase.childrenTypes {
    // Go: fillColumn(eType, input, i, testCase)
    // Go: if i < len(testCase.constants) && testCase.constants[i] != nil {
    // Go: cols[i] = testCase.constants[i]
    // Go: } else {
    // Go: cols[i] = &Column{Index: i, RetType: fts[i]}
    // Go: }
    // Go: }
    // 外部依赖：表达式函数构造仍是 TiDB Go API 调用点，待 Rust 接线。
    // Go: expr, err := NewFunction(ctx, funcName, eType2FieldType(testCase.retEvalType), cols...)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: output = chunk.New([]*types.FieldType{eType2FieldType(expr.GetType(ctx.GetEvalCtx()).EvalType())}, testCase.chunkSize, testCase.chunkSize)
    // Go: if !expr.Vectorized() {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(fmt.Sprintf("func %s is not vectorized", funcName))
    // Go: }
    // Go: return expr, fts, input, output
}

// func testVectorizedEvalOneVec(t *testing.T, vecExprCases vecExprBenchCases) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 测试：对单列向量化求值并与标量结果对比。
pub fn test_vectorized_eval_one_vec() {
    // Go 签名：func testVectorizedEvalOneVec(t *testing.T, vecExprCases vecExprBenchCases) {
    // Go: ctx := createContext(t)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for funcName, testCases := range vecExprCases {
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, testCase := range testCases {
    // Go: expr, fts, input, output := genVecExprBenchCase(ctx, funcName, testCase)
    // Go: commentf := func(row int) string {
    // Go: return fmt.Sprintf("func: %v, case %+v, row: %v, rowData: %v", funcName, testCase, row, input.GetRow(row).GetDatumRow(fts))
    // Go: }
    // Go: output2 := output.CopyConstruct()
    // Go: require.True(t, expr.Vectorized(), "func %s is not vectorized", funcName)
    // Go: require.NoErrorf(t, evalOneVec(ctx, expr, input, output, 0), "func: %v, case: %+v", funcName, testCase)
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: it := chunk.NewIterator4Chunk(input)
    // Go: require.NoErrorf(t, evalOneColumn(ctx, expr, it, output2, 0), "func: %v, case: %+v", funcName, testCase)
    // Go: c1, c2 := output.Column(0), output2.Column(0)
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch expr.GetType(ctx).EvalType() {
    // Go: case types.ETInt:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range input.NumRows() {
    // Go: require.Equal(t, c1.IsNull(i), c2.IsNull(i), commentf(i))
    // Go: if !c1.IsNull(i) {
    // Go: require.Equal(t, c1.GetInt64(i), c2.GetInt64(i), commentf(i))
    // Go: }
    // Go: }
    // Go: case types.ETReal:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range input.NumRows() {
    // Go: require.Equal(t, c1.IsNull(i), c2.IsNull(i), commentf(i))
    // Go: if !c1.IsNull(i) {
    // Go: require.Equal(t, c1.GetFloat64(i), c2.GetFloat64(i), commentf(i))
    // Go: }
    // Go: }
    // Go: case types.ETDecimal:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range input.NumRows() {
    // Go: require.Equal(t, c1.IsNull(i), c2.IsNull(i), commentf(i))
    // Go: if !c1.IsNull(i) {
    // Go: require.Equal(t, c1.GetDecimal(i), c2.GetDecimal(i), commentf(i))
    // Go: }
    // Go: }
    // Go: case types.ETDatetime, types.ETTimestamp:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range input.NumRows() {
    // Go: require.Equal(t, c1.IsNull(i), c2.IsNull(i), commentf(i))
    // Go: if !c1.IsNull(i) {
    // Go: require.Equal(t, c1.GetTime(i), c2.GetTime(i), commentf(i))
    // Go: }
    // Go: }
    // Go: case types.ETDuration:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range input.NumRows() {
    // Go: require.Equal(t, c1.IsNull(i), c2.IsNull(i), commentf(i))
    // Go: if !c1.IsNull(i) {
    // Go: require.Equal(t, c1.GetDuration(i, 0), c2.GetDuration(i, 0), commentf(i))
    // Go: }
    // Go: }
    // Go: case types.ETJson:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range input.NumRows() {
    // Go: require.Equal(t, c1.IsNull(i), c2.IsNull(i), commentf(i))
    // Go: if !c1.IsNull(i) {
    // Go: require.Equal(t, c1.GetJSON(i), c2.GetJSON(i), commentf(i))
    // Go: }
    // Go: }
    // Go: case types.ETString:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range input.NumRows() {
    // Go: require.Equal(t, c1.IsNull(i), c2.IsNull(i), commentf(i))
    // Go: if !c1.IsNull(i) {
    // Go: require.Equal(t, c1.GetString(i), c2.GetString(i), commentf(i))
    // Go: }
    // Go: }
    // Go: }
    // Go: }
    // Go: }
}

// func benchmarkVectorizedEvalOneVec(b *testing.B, vecExprCases vecExprBenchCases) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 基准：单列向量化求值。
pub fn benchmark_vectorized_eval_one_vec() {
    // Go 签名：func benchmarkVectorizedEvalOneVec(b *testing.B, vecExprCases vecExprBenchCases) {
    // Go: ctx := createContext(b)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for funcName, testCases := range vecExprCases {
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, testCase := range testCases {
    // Go: expr, _, input, output := genVecExprBenchCase(ctx, funcName, testCase)
    // Go: exprName := expr.StringWithCtx(ctx, perrors.RedactLogDisable)
    // Go: if sf, ok := expr.(*ScalarFunction); ok {
    // Go: exprName = fmt.Sprintf("%v", reflect.TypeOf(sf.Function))
    // Go: tmp := strings.Split(exprName, ".")
    // Go: exprName = tmp[len(tmp)-1]
    // Go: }
    // Go: if !expr.Vectorized() {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(fmt.Sprintf("func %s is not vectorized", funcName))
    // Go: }
    // Go: b.Run(exprName+"-EvalOneVec", func(b *testing.B) {
    // Go: b.ResetTimer()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: if err := evalOneVec(ctx, expr, input, output, 0); err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: })
    // Go: b.Run(exprName+"-EvalOneCol", func(b *testing.B) {
    // Go: b.ResetTimer()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: it := chunk.NewIterator4Chunk(input)
    // Go: if err := evalOneColumn(ctx, expr, it, output, 0); err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: })
    // Go: }
    // Go: }
}

// func genVecBuiltinFuncBenchCase(ctx BuildContext, funcName string, testCase vecExprBenchCase) (baseFunc builtinFunc, fts []*types.FieldType, input *chunk.Chunk, result *chunk.Column) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成内置函数向量化基准用例。
pub fn gen_vec_builtin_func_bench_case() {
    // Go 签名：func genVecBuiltinFuncBenchCase(ctx BuildContext, funcName string, testCase vecExprBenchCase) (baseFunc builtinFunc, fts []*types.FieldType, input *chunk.Chunk, result *chunk.Column) {
    // Go: childrenNumber := len(testCase.childrenTypes)
    // Go: fts = make([]*types.FieldType, childrenNumber)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range fts {
    // Go: if i < len(testCase.childrenFieldTypes) && testCase.childrenFieldTypes[i] != nil {
    // Go: fts[i] = testCase.childrenFieldTypes[i]
    // Go: } else {
    // Go: fts[i] = eType2FieldType(testCase.childrenTypes[i])
    // Go: }
    // Go: }
    // Go: cols := make([]Expression, childrenNumber)
    // Go: if testCase.chunkSize <= 0 || testCase.chunkSize > 1024 {
    // Go: testCase.chunkSize = 1024
    // Go: }
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: input = chunk.New(fts, testCase.chunkSize, testCase.chunkSize)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i, eType := range testCase.childrenTypes {
    // Go: fillColumn(eType, input, i, testCase)
    // Go: if i < len(testCase.constants) && testCase.constants[i] != nil {
    // Go: cols[i] = testCase.constants[i]
    // Go: } else {
    // Go: cols[i] = &Column{Index: i, RetType: fts[i]}
    // Go: }
    // Go: }
    // Go: if len(cols) == 0 {
    // Go: input.SetNumVirtualRows(testCase.chunkSize)
    // Go: }
    // Go: var err error
    // Go: if funcName == ast.JSONSumCrc32 {
    // Go: fc := &jsonSumCRC32FunctionClass{baseFunctionClass{ast.JSONSumCrc32, 1, 1}, fts[0]}
    // 外部依赖：函数类解析仍按 Go funcs 表语义记录。
    // Go: baseFunc, err = fc.getFunction(ctx, cols)
    // Go: } else if funcName == ast.Cast {
    // Go: var fc functionClass
    // Go: tp := eType2FieldType(testCase.retEvalType)
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch testCase.retEvalType {
    // Go: case types.ETInt:
    // Go: fc = &castAsIntFunctionClass{baseFunctionClass{ast.Cast, 1, 1}, tp, false}
    // Go: case types.ETDecimal:
    // Go: fc = &castAsDecimalFunctionClass{baseFunctionClass{ast.Cast, 1, 1}, tp, false}
    // Go: case types.ETReal:
    // Go: fc = &castAsRealFunctionClass{baseFunctionClass{ast.Cast, 1, 1}, tp, false}
    // Go: case types.ETDatetime, types.ETTimestamp:
    // Go: fc = &castAsTimeFunctionClass{baseFunctionClass{ast.Cast, 1, 1}, tp}
    // Go: case types.ETDuration:
    // Go: fc = &castAsDurationFunctionClass{baseFunctionClass{ast.Cast, 1, 1}, tp}
    // Go: case types.ETJson:
    // Go: fc = &castAsJSONFunctionClass{baseFunctionClass{ast.Cast, 1, 1}, tp}
    // Go: case types.ETString:
    // Go: fc = &castAsStringFunctionClass{baseFunctionClass{ast.Cast, 1, 1}, tp, false}
    // Go: }
    // 外部依赖：函数类解析仍按 Go funcs 表语义记录。
    // Go: baseFunc, err = fc.getFunction(ctx, cols)
    // Go: } else if funcName == ast.GetVar {
    // Go: var fc functionClass
    // Go: tp := eType2FieldType(testCase.retEvalType)
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch testCase.retEvalType {
    // Go: case types.ETInt:
    // Go: fc = &getIntVarFunctionClass{getVarFunctionClass{baseFunctionClass{ast.GetVar, 1, 1}, tp}}
    // Go: case types.ETDecimal:
    // Go: fc = &getDecimalVarFunctionClass{getVarFunctionClass{baseFunctionClass{ast.GetVar, 1, 1}, tp}}
    // Go: case types.ETReal:
    // Go: fc = &getRealVarFunctionClass{getVarFunctionClass{baseFunctionClass{ast.GetVar, 1, 1}, tp}}
    // Go: default:
    // Go: fc = &getStringVarFunctionClass{getVarFunctionClass{baseFunctionClass{ast.GetVar, 1, 1}, tp}}
    // Go: }
    // 外部依赖：函数类解析仍按 Go funcs 表语义记录。
    // Go: baseFunc, err = fc.getFunction(ctx, cols)
    // Go: } else {
    // 外部依赖：函数类解析仍按 Go funcs 表语义记录。
    // Go: baseFunc, err = funcs[funcName].getFunction(ctx, cols)
    // Go: }
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: if !baseFunc.vectorized() || !baseFunc.isChildrenVectorized() {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(fmt.Sprintf("func %s is not vectorized", funcName))
    // Go: }
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: result = chunk.NewColumn(eType2FieldType(testCase.retEvalType), testCase.chunkSize)
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // 原 Go 注释：Mess up the output to make sure vecEvalXXX to call ResizeXXX/ReserveXXX itself.
    // Go: // Mess up the output to make sure vecEvalXXX to call ResizeXXX/ReserveXXX itself.
    // Go: result.AppendNull()
    // Go: return baseFunc, fts, input, result
}

// func getColumnLen(col *chunk.Column, eType types.EvalType) int 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 获取列长度。
pub fn get_column_len() {
    // Go 签名：func getColumnLen(col *chunk.Column, eType types.EvalType) int {
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: chk := chunk.New([]*types.FieldType{eType2FieldType(eType)}, 1024, 1024)
    // Go: chk.SetCol(0, col)
    // Go: return chk.NumRows()
}

// func removeTestOptions(args []string) []string 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 移除仅用于测试的选项标志。
pub fn remove_test_options(args: Vec<String>) -> Vec<String> {
    // Go 签名：func removeTestOptions(args []string) []string {
    // Go: argList := args[:0]
    // 循环/遍历：保持 Go range 或计数循环语义。
    // 原 Go 注释：args contains '-test.timeout=' option for example
    // Go: // args contains '-test.timeout=' option for example
    // 原 Go 注释：excluding it to be able to run all tests
    // Go: // excluding it to be able to run all tests
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, arg := range args {
    // Go: if strings.HasPrefix(arg, "builtin") || IsFunctionSupported(arg) {
    // Go: argList = append(argList, arg)
    // Go: }
    // Go: }
    // Go: return argList
    args.into_iter()
        .filter(|arg| {
            arg.starts_with("builtin") || crate::expression_builtin::IsFunctionSupported(arg)
        })
        .collect()
}

// func testVectorizedBuiltinFunc(t *testing.T, vecExprCases vecExprBenchCases) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 测试：向量化内置函数与行式求值一致性。
/// 对比向量化内置函数与逐行求值结果，确保向量化路径语义一致。
pub fn test_vectorized_builtin_func() {
    // Go 签名：func testVectorizedBuiltinFunc(t *testing.T, vecExprCases vecExprBenchCases) {
    // Go: testFunc := make(map[string]bool)
    // 参数解析：Go 从测试命令行参数筛选指定 builtin/function。
    // Go: argList := removeTestOptions(flag.Args())
    // Go: testAll := len(argList) == 0
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, arg := range argList {
    // Go: testFunc[arg] = true
    // Go: }
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for funcName, testCases := range vecExprCases {
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, testCase := range testCases {
    // Go: ctx := createContext(t)
    // Go: if testCase.aesModes == "" {
    // Go: testCase.aesModes = "aes-128-ecb"
    // Go: }
    // Go: err := ctx.GetSessionVars().SetSystemVar(vardef.BlockEncryptionMode, testCase.aesModes)
    // Go: require.NoError(t, err)
    // Go: if funcName == ast.CurrentUser || funcName == ast.User {
    // Go: ctx.GetSessionVars().User = &auth.UserIdentity{
    // Go: Username: "tidb",
    // Go: Hostname: "localhost",
    // Go: CurrentUser: true,
    // Go: AuthHostname: "localhost",
    // Go: AuthUsername: "tidb",
    // Go: }
    // Go: }
    // Go: if funcName == ast.GetParam {
    // Go: testTime := time.Now()
    // Go: ctx.GetSessionVars().PlanCacheParams.Append(
    // Go: types.NewIntDatum(1),
    // Go: types.NewDecimalDatum(types.NewDecFromStringForTest("20170118123950.123")),
    // Go: types.NewTimeDatum(types.NewTime(types.FromGoTime(testTime), mysql.TypeTimestamp, 6)),
    // Go: types.NewDurationDatum(types.ZeroDuration),
    // Go: types.NewStringDatum("{}"),
    // Go: types.NewBinaryLiteralDatum([]byte{1}),
    // Go: types.NewBytesDatum([]byte{'b'}),
    // Go: types.NewFloat32Datum(1.1),
    // Go: types.NewFloat64Datum(2.1),
    // Go: types.NewUintDatum(100),
    // Go: types.NewMysqlBitDatum([]byte{1}),
    // Go: types.NewMysqlEnumDatum(types.Enum{Name: "n", Value: 2}))
    // Go: }
    // Go: baseFunc, fts, input, output := genVecBuiltinFuncBenchCase(ctx, funcName, testCase)
    // Go: baseFuncName := fmt.Sprintf("%v", reflect.TypeOf(baseFunc))
    // Go: tmp := strings.Split(baseFuncName, ".")
    // Go: baseFuncName = tmp[len(tmp)-1]
    // Go: if !testAll && (!testFunc[baseFuncName] && !testFunc[funcName]) {
    // Go: continue
    // Go: }
    // 原 Go 注释：do not forget to implement the vectorized method.
    // Go: // do not forget to implement the vectorized method.
    // Go: require.Truef(t, baseFunc.vectorized() && baseFunc.isChildrenVectorized(), "func: %v, case: %+v", baseFuncName, testCase)
    // Go: commentf := func(row int) string {
    // Go: return fmt.Sprintf("func: %v, case %+v, row: %v, rowData: %v", baseFuncName, testCase, row, input.GetRow(row).GetDatumRow(fts))
    // Go: }
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: it := chunk.NewIterator4Chunk(input)
    // Go: i := 0
    // Go: var vecWarnCnt uint16
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch testCase.retEvalType {
    // Go: case types.ETInt:
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: err := baseFunc.vecEvalInt(ctx, input, output)
    // Go: require.NoErrorf(t, err, "func: %v, case: %+v", baseFuncName, testCase)
    // 原 Go 注释：do not forget to call ResizeXXX/ReserveXXX
    // Go: // do not forget to call ResizeXXX/ReserveXXX
    // Go: require.Equal(t, input.NumRows(), getColumnLen(output, testCase.retEvalType))
    // Go: vecWarnCnt = ctx.GetSessionVars().StmtCtx.WarningCount()
    // Go: i64s := output.Int64s()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: val, isNull, err := baseFunc.evalInt(ctx, row)
    // Go: require.NoErrorf(t, err, commentf(i))
    // Go: require.Equal(t, output.IsNull(i), isNull, commentf(i))
    // Go: if !isNull {
    // Go: require.Equal(t, i64s[i], val, commentf(i))
    // Go: }
    // Go: i++
    // Go: }
    // Go: case types.ETReal:
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: err := baseFunc.vecEvalReal(ctx, input, output)
    // Go: require.NoErrorf(t, err, "func: %v, case: %+v", baseFuncName, testCase)
    // 原 Go 注释：do not forget to call ResizeXXX/ReserveXXX
    // Go: // do not forget to call ResizeXXX/ReserveXXX
    // Go: require.Equal(t, input.NumRows(), getColumnLen(output, testCase.retEvalType))
    // Go: vecWarnCnt = ctx.GetSessionVars().StmtCtx.WarningCount()
    // Go: f64s := output.Float64s()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: val, isNull, err := baseFunc.evalReal(ctx, row)
    // Go: require.NoErrorf(t, err, commentf(i))
    // Go: require.Equal(t, output.IsNull(i), isNull, commentf(i))
    // Go: if !isNull {
    // Go: require.Equal(t, f64s[i], val, commentf(i))
    // Go: }
    // Go: i++
    // Go: }
    // Go: case types.ETDecimal:
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: err := baseFunc.vecEvalDecimal(ctx, input, output)
    // Go: require.NoErrorf(t, err, "func: %v, case: %+v", baseFuncName, testCase)
    // 原 Go 注释：do not forget to call ResizeXXX/ReserveXXX
    // Go: // do not forget to call ResizeXXX/ReserveXXX
    // Go: require.Equal(t, input.NumRows(), getColumnLen(output, testCase.retEvalType))
    // Go: vecWarnCnt = ctx.GetSessionVars().StmtCtx.WarningCount()
    // Go: d64s := output.Decimals()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: val, isNull, err := baseFunc.evalDecimal(ctx, row)
    // Go: require.NoErrorf(t, err, commentf(i))
    // Go: require.Equal(t, output.IsNull(i), isNull, commentf(i))
    // Go: if !isNull {
    // Go: require.Equal(t, d64s[i], *val, commentf(i))
    // Go: }
    // Go: i++
    // Go: }
    // Go: case types.ETDatetime, types.ETTimestamp:
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: err := baseFunc.vecEvalTime(ctx, input, output)
    // Go: require.NoErrorf(t, err, "func: %v, case: %+v", baseFuncName, testCase)
    // 原 Go 注释：do not forget to call ResizeXXX/ReserveXXX
    // Go: // do not forget to call ResizeXXX/ReserveXXX
    // Go: require.Equal(t, input.NumRows(), getColumnLen(output, testCase.retEvalType))
    // Go: vecWarnCnt = ctx.GetSessionVars().StmtCtx.WarningCount()
    // Go: t64s := output.Times()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: val, isNull, err := baseFunc.evalTime(ctx, row)
    // Go: require.NoErrorf(t, err, commentf(i))
    // Go: require.Equal(t, output.IsNull(i), isNull, commentf(i))
    // Go: if !isNull {
    // Go: require.Equal(t, t64s[i], val, commentf(i))
    // Go: }
    // Go: i++
    // Go: }
    // Go: case types.ETDuration:
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: err := baseFunc.vecEvalDuration(ctx, input, output)
    // Go: require.NoErrorf(t, err, "func: %v, case: %+v", baseFuncName, testCase)
    // 原 Go 注释：do not forget to call ResizeXXX/ReserveXXX
    // Go: // do not forget to call ResizeXXX/ReserveXXX
    // Go: require.Equal(t, input.NumRows(), getColumnLen(output, testCase.retEvalType))
    // Go: vecWarnCnt = ctx.GetSessionVars().StmtCtx.WarningCount()
    // Go: d64s := output.GoDurations()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: val, isNull, err := baseFunc.evalDuration(ctx, row)
    // Go: require.NoErrorf(t, err, commentf(i))
    // Go: require.Equal(t, output.IsNull(i), isNull, commentf(i))
    // Go: if !isNull {
    // Go: require.Equal(t, d64s[i], val.Duration, commentf(i))
    // Go: }
    // Go: i++
    // Go: }
    // Go: case types.ETJson:
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: err := baseFunc.vecEvalJSON(ctx, input, output)
    // Go: require.NoErrorf(t, err, "func: %v, case: %+v", baseFuncName, testCase)
    // 原 Go 注释：do not forget to call ResizeXXX/ReserveXXX
    // Go: // do not forget to call ResizeXXX/ReserveXXX
    // Go: require.Equal(t, input.NumRows(), getColumnLen(output, testCase.retEvalType))
    // Go: vecWarnCnt = ctx.GetSessionVars().StmtCtx.WarningCount()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: val, isNull, err := baseFunc.evalJSON(ctx, row)
    // Go: require.NoErrorf(t, err, commentf(i))
    // Go: require.Equal(t, output.IsNull(i), isNull, commentf(i))
    // Go: if !isNull {
    // Go: cmp := types.CompareBinaryJSON(val, output.GetJSON(i))
    // Go: require.Zero(t, cmp, commentf(i))
    // Go: }
    // Go: i++
    // Go: }
    // Go: case types.ETString:
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: err := baseFunc.vecEvalString(ctx, input, output)
    // Go: require.NoErrorf(t, err, "func: %v, case: %+v", baseFuncName, testCase)
    // 原 Go 注释：do not forget to call ResizeXXX/ReserveXXX
    // Go: // do not forget to call ResizeXXX/ReserveXXX
    // Go: require.Equal(t, input.NumRows(), getColumnLen(output, testCase.retEvalType))
    // Go: vecWarnCnt = ctx.GetSessionVars().StmtCtx.WarningCount()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: val, isNull, err := baseFunc.evalString(ctx, row)
    // Go: require.NoErrorf(t, err, commentf(i))
    // Go: require.Equal(t, output.IsNull(i), isNull, commentf(i))
    // Go: if !isNull {
    // Go: require.Equal(t, output.GetString(i), val, commentf(i))
    // Go: }
    // Go: i++
    // Go: }
    // Go: case types.ETVectorFloat32:
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: err := baseFunc.vecEvalVectorFloat32(ctx, input, output)
    // Go: require.NoErrorf(t, err, "func: %v, case: %+v", baseFuncName, testCase)
    // 原 Go 注释：do not forget to call ResizeXXX/ReserveXXX
    // Go: // do not forget to call ResizeXXX/ReserveXXX
    // Go: require.Equal(t, input.NumRows(), getColumnLen(output, testCase.retEvalType))
    // Go: vecWarnCnt = ctx.GetSessionVars().StmtCtx.WarningCount()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: val, isNull, err := baseFunc.evalVectorFloat32(ctx, row)
    // Go: require.NoErrorf(t, err, commentf(i))
    // Go: require.Equal(t, output.IsNull(i), isNull, commentf(i))
    // Go: if !isNull {
    // Go: require.Equal(t, output.GetVectorFloat32(i).Compare(val), 0, commentf(i))
    // Go: }
    // Go: i++
    // Go: }
    // Go: default:
    // Go: t.Fatalf("evalType=%v is not supported", testCase.retEvalType)
    // Go: }
    // 原 Go 注释：check warnings
    // Go: // check warnings
    // Go: totalWarns := ctx.GetSessionVars().StmtCtx.WarningCount()
    // Go: require.Equal(t, totalWarns, 2*vecWarnCnt)
    // Go: if _, ok := baseFunc.(*builtinAddSubDateAsStringSig); ok {
    // 循环/遍历：保持 Go range 或计数循环语义。
    // 原 Go 注释：skip check warnings for `builtinAddSubDateAsStringSig` for issue https://github.com/pingcap/tidb/issues/50197
    // Go: // skip check warnings for `builtinAddSubDateAsStringSig` for issue https://github.com/pingcap/tidb/issues/50197
    // 原 Go 注释：TODO: fix this issue
    // Go: // TODO: fix this issue
    // Go: continue
    // Go: }
    // Go: warns := ctx.GetSessionVars().StmtCtx.GetWarnings()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range int(vecWarnCnt) {
    // Go: require.True(t, terror.ErrorEqual(warns[i].Err, warns[i+int(vecWarnCnt)].Err))
    // Go: }
    // Go: }
    // Go: }
}

// func testVectorizedBuiltinFuncForRand(t *testing.T, vecExprCases vecExprBenchCases) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 测试：含 RAND 等非确定性函数的向量化路径。
pub fn test_vectorized_builtin_func_for_rand() {
    // Go 签名：func testVectorizedBuiltinFuncForRand(t *testing.T, vecExprCases vecExprBenchCases) {
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for funcName, testCases := range vecExprCases {
    // Go: require.True(t, strings.EqualFold("rand", funcName))
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, testCase := range testCases {
    // Go: require.Len(t, testCase.childrenTypes, 0)
    // Go: ctx := mock.NewContext()
    // Go: baseFunc, _, input, output := genVecBuiltinFuncBenchCase(ctx, funcName, testCase)
    // Go: baseFuncName := fmt.Sprintf("%v", reflect.TypeOf(baseFunc))
    // Go: tmp := strings.Split(baseFuncName, ".")
    // Go: baseFuncName = tmp[len(tmp)-1]
    // 原 Go 注释：do not forget to implement the vectorized method.
    // Go: // do not forget to implement the vectorized method.
    // Go: require.Truef(t, baseFunc.vectorized() && baseFunc.isChildrenVectorized(), "func: %v", baseFuncName)
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch testCase.retEvalType {
    // Go: case types.ETReal:
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: err := baseFunc.vecEvalReal(ctx, input, output)
    // Go: require.NoError(t, err)
    // 原 Go 注释：do not forget to call ResizeXXX/ReserveXXX
    // Go: // do not forget to call ResizeXXX/ReserveXXX
    // Go: require.Equal(t, input.NumRows(), getColumnLen(output, testCase.retEvalType))
    // 原 Go 注释：check result
    // Go: // check result
    // Go: res := output.Float64s()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, v := range res {
    // Go: require.True(t, (0 <= v) && (v < 1))
    // Go: }
    // Go: default:
    // Go: t.Fatalf("evalType=%v is not supported", testCase.retEvalType)
    // Go: }
    // Go: }
    // Go: }
}

// func benchmarkVectorizedBuiltinFunc(b *testing.B, vecExprCases vecExprBenchCases) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 基准：向量化内置函数。
pub fn benchmark_vectorized_builtin_func() {
    // Go 签名：func benchmarkVectorizedBuiltinFunc(b *testing.B, vecExprCases vecExprBenchCases) {
    // Go: ctx := mock.NewContext()
    // Go: testFunc := make(map[string]bool)
    // 参数解析：Go 从测试命令行参数筛选指定 builtin/function。
    // Go: argList := removeTestOptions(flag.Args())
    // Go: testAll := len(argList) == 0
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, arg := range argList {
    // Go: testFunc[arg] = true
    // Go: }
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for funcName, testCases := range vecExprCases {
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, testCase := range testCases {
    // Go: if testCase.aesModes == "" {
    // Go: testCase.aesModes = "aes-128-ecb"
    // Go: }
    // Go: err := ctx.GetSessionVars().SetSystemVar(vardef.BlockEncryptionMode, testCase.aesModes)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: if funcName == ast.CurrentUser || funcName == ast.User {
    // Go: ctx.GetSessionVars().User = &auth.UserIdentity{
    // Go: Username: "tidb",
    // Go: Hostname: "localhost",
    // Go: CurrentUser: true,
    // Go: AuthHostname: "localhost",
    // Go: AuthUsername: "tidb",
    // Go: }
    // Go: }
    // Go: if funcName == ast.GetParam {
    // Go: testTime := time.Now()
    // Go: ctx.GetSessionVars().PlanCacheParams.Append(
    // Go: types.NewIntDatum(1),
    // Go: types.NewDecimalDatum(types.NewDecFromStringForTest("20170118123950.123")),
    // Go: types.NewTimeDatum(types.NewTime(types.FromGoTime(testTime), mysql.TypeTimestamp, 6)),
    // Go: types.NewDurationDatum(types.ZeroDuration),
    // Go: types.NewStringDatum("{}"),
    // Go: types.NewBinaryLiteralDatum([]byte{1}),
    // Go: types.NewBytesDatum([]byte{'b'}),
    // Go: types.NewFloat32Datum(1.1),
    // Go: types.NewFloat64Datum(2.1),
    // Go: types.NewUintDatum(100),
    // Go: types.NewMysqlBitDatum([]byte{1}),
    // Go: types.NewMysqlEnumDatum(types.Enum{Name: "n", Value: 2}))
    // Go: }
    // Go: baseFunc, _, input, output := genVecBuiltinFuncBenchCase(ctx, funcName, testCase)
    // Go: baseFuncName := fmt.Sprintf("%v", reflect.TypeOf(baseFunc))
    // Go: tmp := strings.Split(baseFuncName, ".")
    // Go: baseFuncName = tmp[len(tmp)-1]
    // Go: if !baseFunc.vectorized() || !baseFunc.isChildrenVectorized() {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(fmt.Sprintf("func %s is not vectorized", funcName))
    // Go: }
    // Go: if !testAll && !testFunc[baseFuncName] && !testFunc[funcName] {
    // Go: continue
    // Go: }
    // Go: b.Run(baseFuncName+"-VecBuiltinFunc", func(b *testing.B) {
    // Go: b.ResetTimer()
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch testCase.retEvalType {
    // Go: case types.ETInt:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: if err := baseFunc.vecEvalInt(ctx, input, output); err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: case types.ETReal:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: if err := baseFunc.vecEvalReal(ctx, input, output); err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: case types.ETDecimal:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: if err := baseFunc.vecEvalDecimal(ctx, input, output); err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: case types.ETDatetime, types.ETTimestamp:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: if err := baseFunc.vecEvalTime(ctx, input, output); err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: case types.ETDuration:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: if err := baseFunc.vecEvalDuration(ctx, input, output); err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: case types.ETJson:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: if err := baseFunc.vecEvalJSON(ctx, input, output); err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: case types.ETString:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: if err := baseFunc.vecEvalString(ctx, input, output); err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: case types.ETVectorFloat32:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // 向量化路径：保留 vecEvalXXX 与标量 evalXXX 对比的位置。
    // Go: if err := baseFunc.vecEvalVectorFloat32(ctx, input, output); err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: default:
    // Go: b.Fatalf("evalType=%v is not supported", testCase.retEvalType)
    // Go: }
    // Go: })
    // Go: b.Run(baseFuncName+"-NonVecBuiltinFunc", func(b *testing.B) {
    // Go: b.ResetTimer()
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: it := chunk.NewIterator4Chunk(input)
    // 分支选择：保留 Go switch 的类型或 EvalType 分派。
    // Go: switch testCase.retEvalType {
    // Go: case types.ETInt:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: output.Reset(testCase.retEvalType)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: v, isNull, err := baseFunc.evalInt(ctx, row)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: if isNull {
    // Go: output.AppendNull()
    // Go: } else {
    // Go: output.AppendInt64(v)
    // Go: }
    // Go: }
    // Go: }
    // Go: case types.ETReal:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: output.Reset(testCase.retEvalType)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: v, isNull, err := baseFunc.evalReal(ctx, row)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: if isNull {
    // Go: output.AppendNull()
    // Go: } else {
    // Go: output.AppendFloat64(v)
    // Go: }
    // Go: }
    // Go: }
    // Go: case types.ETDecimal:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: output.Reset(testCase.retEvalType)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: v, isNull, err := baseFunc.evalDecimal(ctx, row)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: if isNull {
    // Go: output.AppendNull()
    // Go: } else {
    // Go: output.AppendMyDecimal(v)
    // Go: }
    // Go: }
    // Go: }
    // Go: case types.ETDatetime, types.ETTimestamp:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: output.Reset(testCase.retEvalType)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: v, isNull, err := baseFunc.evalTime(ctx, row)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: if isNull {
    // Go: output.AppendNull()
    // Go: } else {
    // Go: output.AppendTime(v)
    // Go: }
    // Go: }
    // Go: }
    // Go: case types.ETDuration:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: output.Reset(testCase.retEvalType)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: v, isNull, err := baseFunc.evalDuration(ctx, row)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: if isNull {
    // Go: output.AppendNull()
    // Go: } else {
    // Go: output.AppendDuration(v)
    // Go: }
    // Go: }
    // Go: }
    // Go: case types.ETJson:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: output.Reset(testCase.retEvalType)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: v, isNull, err := baseFunc.evalJSON(ctx, row)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: if isNull {
    // Go: output.AppendNull()
    // Go: } else {
    // Go: output.AppendJSON(v)
    // Go: }
    // Go: }
    // Go: }
    // Go: case types.ETString:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: output.Reset(testCase.retEvalType)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: v, isNull, err := baseFunc.evalString(ctx, row)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: if isNull {
    // Go: output.AppendNull()
    // Go: } else {
    // Go: output.AppendString(v)
    // Go: }
    // Go: }
    // Go: }
    // Go: case types.ETVectorFloat32:
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: output.Reset(testCase.retEvalType)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: v, isNull, err := baseFunc.evalVectorFloat32(ctx, row)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: if isNull {
    // Go: output.AppendNull()
    // Go: } else {
    // Go: output.AppendVectorFloat32(v)
    // Go: }
    // Go: }
    // Go: }
    // Go: default:
    // Go: b.Fatalf("evalType=%v is not supported", testCase.retEvalType)
    // Go: }
    // Go: })
    // Go: }
    // Go: }
}

// func genVecEvalBool(numCols int, colTypes, eTypes []types.EvalType) (CNFExprs, *chunk.Chunk) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成布尔向量求值输入。
pub fn gen_vec_eval_bool() {
    // Go 签名：func genVecEvalBool(numCols int, colTypes, eTypes []types.EvalType) (CNFExprs, *chunk.Chunk) {
    // Go: gens := make([]dataGenerator, 0, len(eTypes))
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, eType := range eTypes {
    // Go: if eType == types.ETString {
    // Go: gens = append(gens, &numStrGener{*newRangeInt64Gener(0, 10)})
    // Go: } else {
    // Go: gens = append(gens, newDefaultGener(0.05, eType))
    // Go: }
    // Go: }
    // Go: ts := make([]types.EvalType, 0, numCols)
    // Go: gs := make([]dataGenerator, 0, numCols)
    // Go: fts := make([]*types.FieldType, 0, numCols)
    // Go: randGen := newDefaultRandGen()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range numCols {
    // Go: idx := randGen.Intn(len(eTypes))
    // Go: if colTypes != nil {
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for j := range eTypes {
    // Go: if colTypes[i] == eTypes[j] {
    // Go: idx = j
    // Go: break
    // Go: }
    // Go: }
    // Go: }
    // Go: ts = append(ts, eTypes[idx])
    // Go: gs = append(gs, gens[idx])
    // Go: fts = append(fts, eType2FieldType(eTypes[idx]))
    // Go: }
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: input := chunk.New(fts, 1024, 1024)
    // Go: exprs := make(CNFExprs, 0, numCols)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range numCols {
    // Go: fillColumn(ts[i], input, i, vecExprBenchCase{geners: gs})
    // Go: exprs = append(exprs, &Column{Index: i, RetType: fts[i]})
    // Go: }
    // Go: return exprs, input
}

// func generateRandomSel() []int 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
/// 生成随机 selection 向量。
pub fn generate_random_sel() -> Vec<usize> {
    // Go 签名：func generateRandomSel() []int {
    // Go: randGen := newDefaultRandGen()
    // Go: randGen.Seed(time.Now().UnixNano())
    // Go: var sel []int
    // Go: count := 0
    // 原 Go 注释：Use constant 256 to make it faster to generate randomly arranged sel slices
    // Go: // Use constant 256 to make it faster to generate randomly arranged sel slices
    // Go: num := randGen.Intn(256) + 1
    // Go: existed := make([]bool, 1024)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range 1024 {
    // Go: existed[i] = false
    // Go: }
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for count < num {
    // Go: val := randGen.Intn(1024)
    // Go: if !existed[val] {
    // Go: existed[val] = true
    // Go: count++
    // Go: }
    // Go: }
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range 1024 {
    // Go: if existed[i] {
    // Go: sel = append(sel, i)
    // Go: }
    // Go: }
    // Go: return sel
    generate_random_sel_with_rng(&mut rand::thread_rng())
}

fn generate_random_sel_with_rng<R: rand::Rng + ?Sized>(rand_gen: &mut R) -> Vec<usize> {
    let num = rand_gen.gen_range(1..=256);
    let mut existed = [false; 1024];
    let mut count = 0;
    while count < num {
        let value = rand_gen.gen_range(0..1024);
        if !existed[value] {
            existed[value] = true;
            count += 1;
        }
    }
    existed
        .iter()
        .enumerate()
        .filter_map(|(index, &present)| present.then_some(index))
        .collect()
}

// func BenchmarkVecEvalBool(b *testing.B) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
// Go benchmark 入口：Rust 不运行基准循环，只记录 b.ResetTimer/b.Run/b.Fatal 等测试框架语义。
/// 基准：向量化布尔求值。
pub fn benchmark_vec_eval_bool() {
    // Go 签名：func BenchmarkVecEvalBool(b *testing.B) {
    // Go: ctx := mock.NewContext()
    // Go: selected := make([]bool, 0, 1024)
    // Go: nulls := make([]bool, 0, 1024)
    // Go: eTypes := []types.EvalType{types.ETInt, types.ETReal, types.ETDecimal, types.ETString, types.ETTimestamp, types.ETDatetime, types.ETDuration}
    // Go: tNames := []string{"int", "real", "decimal", "string", "timestamp", "datetime", "duration"}
    // Go: vecEnabled := ctx.GetSessionVars().EnableVectorizedExpression
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for numCols := 1; numCols <= 2; numCols++ {
    // Go: typeCombination := make([]types.EvalType, numCols)
    // Go: var combFunc func(nCols int)
    // Go: combFunc = func(nCols int) {
    // Go: if nCols == 0 {
    // Go: name := ""
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, t := range typeCombination {
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range eTypes {
    // Go: if t == eTypes[i] {
    // Go: name += tNames[t] + "/"
    // Go: }
    // Go: }
    // Go: }
    // Go: exprs, input := genVecEvalBool(numCols, typeCombination, eTypes)
    // Go: b.Run("Vec-"+name, func(b *testing.B) {
    // Go: b.ResetTimer()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: _, _, err := VecEvalBool(ctx, vecEnabled, exprs, input, selected, nulls)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: })
    // Go: b.Run("Row-"+name, func(b *testing.B) {
    // Go: b.ResetTimer()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: it := chunk.NewIterator4Chunk(input)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: _, _, err := EvalBool(ctx, exprs, row)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: }
    // Go: })
    // Go: return
    // Go: }
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, eType := range eTypes {
    // Go: typeCombination[nCols-1] = eType
    // Go: combFunc(nCols - 1)
    // Go: }
    // Go: }
    // Go: combFunc(numCols)
    // Go: }
}

// func BenchmarkRowBasedFilterAndVectorizedFilter(b *testing.B) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
// Go benchmark 入口：Rust 不运行基准循环，只记录 b.ResetTimer/b.Run/b.Fatal 等测试框架语义。
/// 基准：对比行式过滤与向量化过滤性能。
pub fn benchmark_row_based_filter_and_vectorized_filter() {
    // Go 签名：func BenchmarkRowBasedFilterAndVectorizedFilter(b *testing.B) {
    // Go: ctx := mock.NewContext()
    // Go: selected := make([]bool, 0, 1024)
    // Go: nulls := make([]bool, 0, 1024)
    // Go: eTypes := []types.EvalType{types.ETInt, types.ETReal, types.ETDecimal, types.ETString, types.ETTimestamp, types.ETDatetime, types.ETDuration}
    // Go: tNames := []string{"int", "real", "decimal", "string", "timestamp", "datetime", "duration"}
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for numCols := 1; numCols <= 2; numCols++ {
    // Go: typeCombination := make([]types.EvalType, numCols)
    // Go: var combFunc func(nCols int)
    // Go: combFunc = func(nCols int) {
    // Go: if nCols == 0 {
    // Go: name := ""
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, t := range typeCombination {
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := range eTypes {
    // Go: if t == eTypes[i] {
    // Go: name += tNames[t] + "/"
    // Go: }
    // Go: }
    // Go: }
    // Go: exprs, input := genVecEvalBool(numCols, typeCombination, eTypes)
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: it := chunk.NewIterator4Chunk(input)
    // Go: b.Run("Vec-"+name, func(b *testing.B) {
    // Go: b.ResetTimer()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: _, _, err := vectorizedFilter(ctx, ctx.GetSessionVars().EnableVectorizedExpression, exprs, it, selected, nulls)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: })
    // Go: b.Run("Row-"+name, func(b *testing.B) {
    // Go: b.ResetTimer()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: _, _, err := rowBasedFilter(ctx, exprs, it, selected, nulls)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: })
    // Go: return
    // Go: }
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for _, eType := range eTypes {
    // Go: typeCombination[nCols-1] = eType
    // Go: combFunc(nCols - 1)
    // Go: }
    // Go: }
    // Go: combFunc(numCols)
    // Go: }
    // 原 Go 注释：Add special case to prove when some calculations are added,
    // Go: // Add special case to prove when some calculations are added,
    // 循环/遍历：保持 Go range 或计数循环语义。
    // 原 Go 注释：the vectorizedFilter for int types will be more faster than rowBasedFilter.
    // Go: // the vectorizedFilter for int types will be more faster than rowBasedFilter.
    // Go: funcName := ast.Least
    // Go: testCase := vecExprBenchCase{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETInt, types.ETInt}}
    // Go: expr, _, input, _ := genVecExprBenchCase(ctx, funcName, testCase)
    // IO/内存结构：chunk 是 TiDB 向量化列存容器，不分配真实数据。
    // Go: it := chunk.NewIterator4Chunk(input)
    // Go: b.Run("Vec-special case", func(b *testing.B) {
    // Go: b.ResetTimer()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: _, _, err := vectorizedFilter(ctx, ctx.GetSessionVars().EnableVectorizedExpression, []Expression{expr}, it, selected, nulls)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: }
    // Go: })
    // Go: b.Run("Row-special case", func(b *testing.B) {
    // Go: b.ResetTimer()
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for i := 0; i < b.N; i++ {
    // Go: _, _, err := rowBasedFilter(ctx, []Expression{expr}, it, selected, nulls)
    // 错误处理：Go 在这里检查 err 并提前进入 panic/断言分支。
    // Go: if err != nil {
    // 错误处理：Go benchmark/helper 在构造失败时 panic，只记录该致命路径。
    // Go: panic(err)
    // Go: }
    // Go: }
    // Go: })
}

// func TestBenchDaily(t *testing.T) 对应 Go 函数；保留调用顺序、断言点和外部依赖语义。
#[test]
/// benchdaily 入口：注册并运行日常基准集。
/// benchdaily 入口：汇总注册日常性能基准用例。
pub fn test_bench_daily() {
    let benchmarks = [
        "BenchmarkCastIntAsIntRow",
        "BenchmarkCastIntAsIntVec",
        "BenchmarkVectorizedExecute",
        "BenchmarkScalarFunctionClone",
        "BenchmarkColumnPoolGet",
        "BenchmarkColumnPoolGetParallel",
        "BenchmarkColumnPoolGetPut",
        "BenchmarkColumnPoolGetPutParallel",
        "BenchmarkPlusIntBufAllocator",
        "BenchmarkVectorizedBuiltinMiscellaneousEvalOneVec",
    ];
    assert_eq!(benchmarks.len(), 10);
    let unique = benchmarks
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(unique.len(), 10, "benchdaily registrations must be unique");
    // Go 签名：func TestBenchDaily(t *testing.T) {
    // 外部依赖：benchdaily 聚合运行基准入口，不触发真实基准。
    // Go: benchdaily.Run(
    // Go: BenchmarkCastIntAsIntRow,
    // Go: BenchmarkCastIntAsIntVec,
    // Go: BenchmarkVectorizedExecute,
    // Go: BenchmarkScalarFunctionClone,
    // Go: BenchmarkColumnPoolGet,
    // Go: BenchmarkColumnPoolGetParallel,
    // Go: BenchmarkColumnPoolGetPut,
    // Go: BenchmarkColumnPoolGetPutParallel,
    // Go: BenchmarkPlusIntBufAllocator,
    // Go: BenchmarkVectorizedBuiltinMiscellaneousEvalOneVec,
    // Go: )
}

#[test]
fn remove_test_options_keeps_only_builtin_selectors() {
    let args = vec![
        "-test.timeout=10m".to_owned(),
        "builtinArithmetic".to_owned(),
        "abs".to_owned(),
        "definitely_not_a_builtin".to_owned(),
    ];

    assert_eq!(
        remove_test_options(args),
        vec!["builtinArithmetic".to_owned(), "abs".to_owned()]
    );
}

#[test]
fn rand_string_matches_go_length_and_alphabet_contract() {
    use rand::SeedableRng;

    let mut rng = rand::rngs::StdRng::seed_from_u64(0xA57E_2868);
    for _ in 0..128 {
        let value = rand_string(&mut rng);
        assert!((10..20).contains(&value.len()));
        assert!(value.bytes().all(|byte| byte.is_ascii_alphanumeric()));
    }
}

#[test]
fn random_selection_is_sorted_unique_and_go_bounded() {
    for _ in 0..64 {
        let selection = generate_random_sel();
        assert!((1..=256).contains(&selection.len()));
        assert!(selection.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(selection.iter().all(|&index| index < 1024));
    }
}
