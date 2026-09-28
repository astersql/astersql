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

// 聚合函数通用测试夹具与契约测试。
//
// 文件前半为大段块注释，保留从 Go `aggfunc_test.go` 机械迁移的表驱动
// 用例说明（数据生成、partial/final 合并、DISTINCT、内存增量、benchmark、
// pushdown）。块注释结束后的可执行测试验证 AVG/COUNT 等实现满足
// update → merge → reset 的生产契约。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables,
    unused_mut
)]

/*
// 这段逻辑覆盖聚合函数通用测试夹具：数据生成、partial/final 合并、distinct 随机用例、内存增量估算、benchmark 和 pushdown 判定。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发、异步、IO 和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待后续 Rust crate 接线）：
// - "fmt"
// - "math"
// - "math/rand"
// - "slices"
// - "strconv"
// - "strings"
// - "testing"
// - "time"
// - "unsafe"
// - "github.com/dgryski/go-farm"
// - "github.com/pingcap/errors"
// - "github.com/pingcap/tidb/pkg/executor/aggfuncs"
// - internalutil "github.com/pingcap/tidb/pkg/executor/internal/util"
// - "github.com/pingcap/tidb/pkg/expression"
// - "github.com/pingcap/tidb/pkg/expression/aggregation"
// - "github.com/pingcap/tidb/pkg/kv"
// - "github.com/pingcap/tidb/pkg/parser/ast"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/planner/util"
// - "github.com/pingcap/tidb/pkg/sessionctx"
// - "github.com/pingcap/tidb/pkg/types"
// - "github.com/pingcap/tidb/pkg/util/chunk"
// - "github.com/pingcap/tidb/pkg/util/codec"
// - "github.com/pingcap/tidb/pkg/util/collate"
// - "github.com/pingcap/tidb/pkg/util/hack"
// - "github.com/pingcap/tidb/pkg/util/mock"
// - "github.com/pingcap/tidb/pkg/util/set"
// - "github.com/stretchr/testify/require"

// 迁移占位类型：这些名称代表 Go/TiDB 测试依赖，后续接入模块时再替换为真实 Rust 类型。
type GoAny = ();
type GoError = String;

// separator argument for group_concat() test cases
// separator 对应 Go 的同名常量，保留测试 fixture 字面量。
// Go 常量声明: const separator = " "
pub const separator: &str = " ";

// aggTest 对应 Go 的同名辅助结构，字段和嵌入类型按原测试 fixture 语义保留。
// Go 类型声明: type aggTest struct {
pub struct aggTest {
    keyType  *types.FieldType
    numRows  int
    dataGen  func(i int) types.Datum
    funcName string
    results  []types.Datum
    orderBy  bool

    // Most data type in distinct agg only need key, such as map[string]struct{}
    // However, some data type need both key and value, such map[string]*types.MyDecimal
    // When this field is nil, it means that we only key.
    valType *types.FieldType
}

// genSrcChk 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func (p *aggTest) genSrcChk() *chunk.Chunk {
pub fn gen_src_chk() {
    srcChk := chunk.NewChunkWithCapacity([]*types.FieldType{p.keyType}, p.numRows)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for i := range p.numRows {
        dt := p.dataGen(i)
        srcChk.AppendDatum(0, &dt)
    }
    srcChk.AppendDatum(0, &types.Datum{})
    return srcChk
}

// messUpChunk messes up the chunk for testing memory reference.
// messUpChunk 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func (p *aggTest) messUpChunk(c *chunk.Chunk) {
pub fn mess_up_chunk() {
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for i := range p.numRows {
        raw := c.Column(0).GetRaw(i)
        // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
        for i := range raw {
            raw[i] = 255
        }
    }
}

// parallelDistinctAggTestCase 对应 Go 的同名辅助结构，字段和嵌入类型按原测试 fixture 语义保留。
// Go 类型声明: type parallelDistinctAggTestCase struct {
pub struct parallelDistinctAggTestCase {
    dataTypes []*types.FieldType
    funcName  string
    srcChks   []*chunk.Chunk
    result    types.Datum
}

// newParallelDistinctAggTestCase 对应 Go 的同名测试/辅助函数，保留 distinct 随机数据、NDV/null 分布和期望值计算语义。
// Go 签名: func newParallelDistinctAggTestCase(funcName string, dataTypes []*types.FieldType, numRows, ndv int, needNull bool, allNull bool) *parallelDistinctAggTestCase {
pub fn new_parallel_distinct_agg_test_case() {
    testCase := &parallelDistinctAggTestCase{
        dataTypes: dataTypes,
        funcName:  funcName,
    }

    var dataGenFunc func() types.Datum

    intDatums := make(map[int]struct{})
    float64Datums := make(map[float64]struct{})
    decimalDatums := make(map[string]*types.MyDecimal)
    stringDatums := make(map[string]struct{})
    durationDatums := make(map[int64]struct{})

    hasMultiArgs := len(dataTypes) > 1

    // In this ut, we ensure arg types are the same when there are multi args.
    // Just for convenience.
    if hasMultiArgs && dataTypes[0].GetType() != dataTypes[1].GetType() {
        // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
        panic("Need same types")
    }

    // switch 分支保留 Go 对类型、函数名或状态枚举的判定顺序。
    switch dataTypes[0].GetType() {
    case mysql.TypeLonglong:
        dataGenFunc = func() types.Datum {
            for {
                // 随机数据生成用于 distinct/聚合 fixture；Rust 接线时需保留 NDV/null 分布语义。
                newVal := rand.Intn(1000000000)
                if !mysql.HasUnsignedFlag(dataTypes[0].GetFlag()) {
                    newVal -= 500000000
                }
                _, ok := intDatums[newVal]
                if ok {
                    continue
                }

                intDatums[newVal] = struct{}{}
                if mysql.HasUnsignedFlag(dataTypes[0].GetFlag()) {
                    return types.NewUintDatum(uint64(newVal))
                }
                return types.NewIntDatum(int64(newVal))
            }
        }
    case mysql.TypeDouble:
        dataGenFunc = func() types.Datum {
            for {
                // 随机数据生成用于 distinct/聚合 fixture；Rust 接线时需保留 NDV/null 分布语义。
                newVal := rand.Float64()*100 - 50
                _, ok := float64Datums[newVal]
                if ok {
                    continue
                }

                float64Datums[newVal] = struct{}{}
                return types.NewFloat64Datum(newVal)
            }
        }
    case mysql.TypeNewDecimal:
        dataGenFunc = func() types.Datum {
            for {
                // 随机数据生成用于 distinct/聚合 fixture；Rust 接线时需保留 NDV/null 分布语义。
                newVal := types.NewDecFromStringForTest(fmt.Sprintf("%.4f", rand.Float64()*100-50))
                hashKeyBytes, err := newVal.ToHashKey()
                // 错误分支保留 Go 的显式 err 检查，Rust 接线时应改为 Result 传播或断言。
                if err != nil {
                    // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
                    panic(fmt.Sprintf("newVal: %s is invalid, err: %v", newVal, err))
                }

                _, ok := decimalDatums[string(hashKeyBytes)]
                if ok {
                    continue
                }

                decimalDatums[string(hashKeyBytes)] = newVal
                return types.NewDecimalDatum(newVal)
            }
        }
    case mysql.TypeVarString:
        dataGenFunc = func() types.Datum {
            for {
                // 随机数据生成用于 distinct/聚合 fixture；Rust 接线时需保留 NDV/null 分布语义。
                newVal := internalutil.GenerateRandomString(rand.Intn(100))
                _, ok := stringDatums[newVal]
                if ok {
                    continue
                }

                stringDatums[newVal] = struct{}{}
                return types.NewStringDatum(newVal)
            }
        }
    case mysql.TypeDuration:
        dataGenFunc = func() types.Datum {
            for {
                // 随机数据生成用于 distinct/聚合 fixture；Rust 接线时需保留 NDV/null 分布语义。
                newVal := types.NewDuration(rand.Intn(800), rand.Intn(60), rand.Intn(60), 0, 0)
                _, ok := durationDatums[int64(newVal.Duration)]
                if ok {
                    continue
                }

                durationDatums[int64(newVal.Duration)] = struct{}{}
                return types.NewDurationDatum(newVal)
            }
        }
    }

    datumsForNDV := make([][]types.Datum, 0, ndv)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for range ndv {
        if hasMultiArgs {
            datumsForNDV = append(datumsForNDV, []types.Datum{dataGenFunc(), dataGenFunc()})
        } else {
            datumsForNDV = append(datumsForNDV, []types.Datum{dataGenFunc()})
        }
    }

    srcChkNum := 10
    testCase.srcChks = make([]*chunk.Chunk, 0, srcChkNum)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for range srcChkNum {
        testCase.srcChks = append(testCase.srcChks, chunk.NewChunkWithCapacity(dataTypes, numRows))
    }

    insertedIdxs := make(map[int]struct{})
    // 随机数据生成用于 distinct/聚合 fixture；Rust 接线时需保留 NDV/null 分布语义。
    nullValProportion := rand.Intn(9) + 1
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for range numRows {
        // 随机数据生成用于 distinct/聚合 fixture；Rust 接线时需保留 NDV/null 分布语义。
        chkIdx := rand.Intn(srcChkNum)
        // 随机数据生成用于 distinct/聚合 fixture；Rust 接线时需保留 NDV/null 分布语义。
        if allNull || (needNull && rand.Intn(10) < nullValProportion) {
            nilDatum := types.NewDatum(nil)
            testCase.srcChks[chkIdx].AppendDatum(0, &nilDatum)
            if hasMultiArgs {
                testCase.srcChks[chkIdx].AppendDatum(1, &nilDatum)
            }
            continue
        }

        idx := rand.Intn(ndv)
        testCase.srcChks[chkIdx].AppendDatum(0, &datumsForNDV[idx][0])
        if hasMultiArgs {
            testCase.srcChks[chkIdx].AppendDatum(1, &datumsForNDV[idx][1])
        }

        insertedIdxs[idx] = struct{}{}
    }

    insertedDistinctValNum := len(insertedIdxs)

    // switch 分支保留 Go 对类型、函数名或状态枚举的判定顺序。
    switch funcName {
    case ast.AggFuncCount:
        testCase.result = types.NewIntDatum(int64(insertedDistinctValNum))
    case ast.AggFuncAvg:
        if len(insertedIdxs) == 0 {
            testCase.result = types.NewDatum(nil)
            break
        }
        // switch 分支保留 Go 对类型、函数名或状态枚举的判定顺序。
        switch dataTypes[0].GetType() {
        case mysql.TypeDouble:
            s := float64(0)
            for idx := range insertedIdxs {
                s += datumsForNDV[idx][0].GetFloat64()
            }
            testCase.result = types.NewFloat64Datum(s / float64(insertedDistinctValNum))
        case mysql.TypeNewDecimal:
            s := types.NewDecFromStringForTest("0.0000")
            for idx := range insertedIdxs {
                dec := datumsForNDV[idx][0].GetMysqlDecimal()
                tmp := s
                s = types.NewDecFromStringForTest("0.0000")
                err := types.DecimalAdd(dec, tmp, s)
                // 错误分支保留 Go 的显式 err 检查，Rust 接线时应改为 Result 传播或断言。
                if err != nil {
                    // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
                    panic(err)
                }
            }
            num := types.NewDecFromInt(int64(insertedDistinctValNum))
            res := types.NewDecFromInt(0)
            types.DecimalDiv(s, num, res, 0)
            testCase.result = types.NewDecimalDatum(res)
        default:
            // In actual execution, some data type will be converted before entering avg agg.
            // So it's needless to test them in the ut.
            // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
            panic("Not supported in test")
        }
    case ast.AggFuncVarPop, ast.AggFuncVarSamp, ast.AggFuncStddevPop, ast.AggFuncStddevSamp:
        if dataTypes[0].GetType() != mysql.TypeDouble {
            // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
            panic("Not supported in test")
        }
        testCase.result = buildParallelDistinctVarianceResult(funcName, insertedIdxs, datumsForNDV)
    case ast.AggFuncSum, ast.AggFuncSumInt:
        if len(insertedIdxs) == 0 {
            testCase.result = types.NewDatum(nil)
            break
        }
        switch dataTypes[0].GetType() {
        case mysql.TypeLonglong:
            if mysql.HasUnsignedFlag(dataTypes[0].GetFlag()) {
                // s 是局部求和累加器，保留 unsigned longlong 的 distinct sum 期望值计算。
                var s uint64
                for idx := range insertedIdxs {
                    s += datumsForNDV[idx][0].GetUint64()
                }
                testCase.result = types.NewUintDatum(s)
            } else {
                // s 是局部求和累加器，保留 signed longlong 的 distinct sum 期望值计算。
                var s int64
                for idx := range insertedIdxs {
                    s += datumsForNDV[idx][0].GetInt64()
                }
                testCase.result = types.NewIntDatum(s)
            }
        case mysql.TypeDouble:
            s := float64(0)
            for idx := range insertedIdxs {
                s += datumsForNDV[idx][0].GetFloat64()
            }
            testCase.result = types.NewFloat64Datum(s)
        case mysql.TypeNewDecimal:
            s := types.NewDecFromStringForTest("0.0000")
            for idx := range insertedIdxs {
                dec := datumsForNDV[idx][0].GetMysqlDecimal()
                tmp := s
                s = types.NewDecFromStringForTest("0.0000")
                err := types.DecimalAdd(dec, tmp, s)
                // 错误分支保留 Go 的显式 err 检查，Rust 接线时应改为 Result 传播或断言。
                if err != nil {
                    // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
                    panic(err)
                }
            }
            testCase.result = types.NewDecimalDatum(s)
        default:
            // In actual execution, some data type will be converted before entering avg agg.
            // So it's needless to test them in the ut.
            // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
            panic("Not supported in test")
        }
    case ast.AggFuncGroupConcat:
        if dataTypes[0].GetType() != mysql.TypeVarString {
            // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
            panic("Data type is not string")
        }

        isFirst := true
        resultStr := ""
        for idx := range insertedIdxs {
            if isFirst {
                resultStr = fmt.Sprintf("%s%s", datumsForNDV[idx][0].GetString(), datumsForNDV[idx][1].GetString())
                isFirst = false
            } else {
                resultStr = fmt.Sprintf("%s%s%s%s", resultStr, separator, datumsForNDV[idx][0].GetString(), datumsForNDV[idx][1].GetString())
            }
        }
        testCase.result = types.NewStringDatum(resultStr)
    default:
        panic("Not supported")
    }
    return testCase
}

// buildParallelDistinctVarianceResult 对应 Go 的同名测试/辅助函数，保留 distinct 随机数据、NDV/null 分布和期望值计算语义。
// Go 签名: func buildParallelDistinctVarianceResult(funcName string, insertedIdxs map[int]struct{}, datumsForNDV [][]types.Datum) types.Datum {
pub fn build_parallel_distinct_variance_result() {
    if len(insertedIdxs) == 0 {
        return types.NewDatum(nil)
    }

    values := make([]float64, 0, len(insertedIdxs))
    sum := float64(0)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for idx := range insertedIdxs {
        val := datumsForNDV[idx][0].GetFloat64()
        values = append(values, val)
        sum += val
    }

    if (funcName == ast.AggFuncVarSamp || funcName == ast.AggFuncStddevSamp) && len(values) <= 1 {
        return types.NewDatum(nil)
    }

    mean := sum / float64(len(values))
    variance := float64(0)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for _, val := range values {
        diff := val - mean
        variance += diff * diff
    }

    // switch 分支保留 Go 对类型、函数名或状态枚举的判定顺序。
    switch funcName {
    case ast.AggFuncVarPop:
        return types.NewFloat64Datum(variance / float64(len(values)))
    case ast.AggFuncVarSamp:
        return types.NewFloat64Datum(variance / float64(len(values)-1))
    case ast.AggFuncStddevPop:
        return types.NewFloat64Datum(math.Sqrt(variance / float64(len(values))))
    case ast.AggFuncStddevSamp:
        return types.NewFloat64Datum(math.Sqrt(variance / float64(len(values)-1)))
    default:
        // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
        panic("Not supported")
    }
}

// multiArgsAggTest 对应 Go 的同名辅助结构，字段和嵌入类型按原测试 fixture 语义保留。
// Go 类型声明: type multiArgsAggTest struct {
pub struct multiArgsAggTest {
    dataTypes []*types.FieldType
    retType   *types.FieldType
    numRows   int
    dataGens  []func(i int) types.Datum
    funcName  string
    results   []types.Datum
    orderBy   bool
}

// genSrcChk 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func (p *multiArgsAggTest) genSrcChk() *chunk.Chunk {
pub fn gen_src_chk() {
    srcChk := chunk.NewChunkWithCapacity(p.dataTypes, p.numRows)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for i := range p.numRows {
        // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
        for j := range p.dataGens {
            fdt := p.dataGens[j](i)
            srcChk.AppendDatum(j, &fdt)
        }
    }
    srcChk.AppendDatum(0, &types.Datum{})
    return srcChk
}

// messUpChunk messes up the chunk for testing memory reference.
// messUpChunk 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func (p *multiArgsAggTest) messUpChunk(c *chunk.Chunk) {
pub fn mess_up_chunk() {
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for i := range p.numRows {
        // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
        for j := range p.dataGens {
            raw := c.Column(j).GetRaw(i)
            // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
            for i := range raw {
                raw[i] = 255
            }
        }
    }
}

// updateMemDeltaGensParams 对应 Go 的同名辅助结构，字段和嵌入类型按原测试 fixture 语义保留。
// Go 类型声明: type updateMemDeltaGensParams struct {
pub struct updateMemDeltaGensParams {
    srcChk  *chunk.Chunk
    keyType *types.FieldType
    valType *types.FieldType
}

// updateMemDeltaGens 对应 Go 的同名类型别名/函数类型，保留测试辅助接口形状。
// Go 类型声明: type updateMemDeltaGens func(param updateMemDeltaGensParams) (memDeltas []int64, err error)
pub type updateMemDeltaGens = GoAny;

// defaultUpdateMemDeltaGens 对应 Go 的同名测试/辅助函数，保留内存增量估算、更新路径和错误返回语义。
// Go 签名: func defaultUpdateMemDeltaGens(param updateMemDeltaGensParams) (memDeltas []int64, err error) {
pub fn default_update_mem_delta_gens() {
    memDeltas = make([]int64, 0)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for range param.srcChk.NumRows() {
        memDeltas = append(memDeltas, int64(0))
    }
    return memDeltas, nil
}

// approxCountDistinctUpdateMemDeltaGens 对应 Go 的同名测试/辅助函数，保留内存增量估算、更新路径和错误返回语义。
// Go 签名: func approxCountDistinctUpdateMemDeltaGens(param updateMemDeltaGensParams) (memDeltas []int64, err error) {
pub fn approx_count_distinct_update_mem_delta_gens() {
    memDeltas = make([]int64, 0)

    buf := make([]byte, 8)
    p := aggfuncs.NewPartialResult4ApproxCountDistinct()
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for i := range param.srcChk.NumRows() {
        row := param.srcChk.GetRow(i)
        if row.IsNull(0) {
            memDeltas = append(memDeltas, int64(0))
            continue
        }
        oldMemUsage := p.MemUsage()
        // switch 分支保留 Go 对类型、函数名或状态枚举的判定顺序。
        switch param.keyType.GetType() {
        case mysql.TypeLonglong:
            val := row.GetInt64(0)
            // unsafe.Pointer 保留 Go 低层编码路径；Rust 实现需显式审查字节序和别名规则。
            *(*int64)(unsafe.Pointer(&buf[0])) = val
        case mysql.TypeString:
            val := row.GetString(0)
            buf = codec.EncodeCompactBytes(buf, hack.Slice(val))
        default:
            return memDeltas, errors.Errorf("unsupported type - %v", param.keyType.GetType())
        }

        x := farm.Hash64(buf)
        p.InsertHash64(x)
        newMemUsage := p.MemUsage()
        memDelta := newMemUsage - oldMemUsage
        memDeltas = append(memDeltas, memDelta)
    }
    return memDeltas, nil
}

// distinctUpdateMemDeltaGens 对应 Go 的同名测试/辅助函数，保留内存增量估算、更新路径和错误返回语义。
// Go 签名: func distinctUpdateMemDeltaGens(param updateMemDeltaGensParams) (memDeltas []int64, err error) {
pub fn distinct_update_mem_delta_gens() {
    valSet := set.NewStringSet()
    memDeltas = make([]int64, 0)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for i := range param.srcChk.NumRows() {
        row := param.srcChk.GetRow(i)
        if row.IsNull(0) {
            memDeltas = append(memDeltas, int64(0))
            continue
        }
        val := ""
        memDelta := int64(0)
        // switch 分支保留 Go 对类型、函数名或状态枚举的判定顺序。
        switch param.keyType.GetType() {
        case mysql.TypeLonglong:
            val = strconv.FormatInt(row.GetInt64(0), 10)
        case mysql.TypeFloat:
            val = strconv.FormatFloat(float64(row.GetFloat32(0)), 'f', 6, 64)
        case mysql.TypeDouble:
            val = strconv.FormatFloat(row.GetFloat64(0), 'f', 6, 64)
        case mysql.TypeNewDecimal:
            decimal := row.GetMyDecimal(0)
            hash, err := decimal.ToHashKey()
            // 错误分支保留 Go 的显式 err 检查，Rust 接线时应改为 Result 传播或断言。
            if err != nil {
                memDeltas = append(memDeltas, int64(0))
                continue
            }
            val = string(hack.String(hash))
            memDelta = int64(len(val))
        case mysql.TypeString:
            val = row.GetString(0)
            memDelta = int64(len(val))
        case mysql.TypeDate:
            val = row.GetTime(0).String()
            // the distinct count aggFunc need 16 bytes to encode the Datetime type.
            memDelta = 16
        case mysql.TypeDuration:
            val = strconv.FormatInt(row.GetInt64(0), 10)
        case mysql.TypeJSON:
            jsonVal := row.GetJSON(0)
            bytes := make([]byte, 0)
            bytes = jsonVal.HashValue(bytes)
            val = string(bytes)
            memDelta = int64(len(val))
        default:
            return memDeltas, errors.Errorf("unsupported type - %v", param.keyType.GetType())
        }
        if valSet.Exist(val) {
            memDeltas = append(memDeltas, int64(0))
            continue
        }
        if param.valType != nil {
            // switch 分支保留 Go 对类型、函数名或状态枚举的判定顺序。
            switch param.valType.GetType() {
            case mysql.TypeNewDecimal:
                memDelta += types.MyDecimalStructSize
            default:
                // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
                panic("Not supported")
            }
        }
        valSet.Insert(val)
        memDeltas = append(memDeltas, memDelta)
    }
    return memDeltas, nil
}

// rowMemDeltaGens 对应 Go 的同名测试/辅助函数，保留内存增量估算、更新路径和错误返回语义。
// Go 签名: func rowMemDeltaGens(param updateMemDeltaGensParams) (memDeltas []int64, err error) {
pub fn row_mem_delta_gens() {
    memDeltas = make([]int64, 0)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for range param.srcChk.NumRows() {
        memDelta := aggfuncs.DefRowSize
        memDeltas = append(memDeltas, memDelta)
    }
    return memDeltas, nil
}

// multiArgsUpdateMemDeltaGens 对应 Go 的同名类型别名/函数类型，保留测试辅助接口形状。
// Go 类型声明: type multiArgsUpdateMemDeltaGens func(sessionctx.Context, *chunk.Chunk, []*types.FieldType, []*util.ByItems) (memDeltas []int64, err error)
pub type multiArgsUpdateMemDeltaGens = GoAny;

// aggMemTest 对应 Go 的同名辅助结构，字段和嵌入类型按原测试 fixture 语义保留。
// Go 类型声明: type aggMemTest struct {
pub struct aggMemTest {
    aggTest            aggTest
    allocMemDelta      int64
    updateMemDeltaGens updateMemDeltaGens
    isDistinct         bool
}

// buildAggMemTester 对应 Go 的同名测试/辅助函数，保留内存增量估算、更新路径和错误返回语义。
// Go 签名: func buildAggMemTester(funcName string, keyTp byte, valTp byte, numRows int, allocMemDelta int64, updateMemDeltaGens updateMemDeltaGens, isDistinct bool) aggMemTest {
pub fn build_agg_mem_tester() {
    aggTest := buildAggTester(funcName, keyTp, valTp, numRows)
    pt := aggMemTest{
        aggTest:            aggTest,
        allocMemDelta:      allocMemDelta,
        updateMemDeltaGens: updateMemDeltaGens,
        isDistinct:         isDistinct,
    }
    return pt
}

// multiArgsAggMemTest 对应 Go 的同名辅助结构，字段和嵌入类型按原测试 fixture 语义保留。
// Go 类型声明: type multiArgsAggMemTest struct {
pub struct multiArgsAggMemTest {
    multiArgsAggTest            multiArgsAggTest
    allocMemDelta               int64
    multiArgsUpdateMemDeltaGens multiArgsUpdateMemDeltaGens
    isDistinct                  bool
}

// buildMultiArgsAggMemTester 对应 Go 的同名测试/辅助函数，保留内存增量估算、更新路径和错误返回语义。
// Go 签名: func buildMultiArgsAggMemTester(funcName string, tps []byte, rt byte, numRows int, allocMemDelta int64, updateMemDeltaGens multiArgsUpdateMemDeltaGens, isDistinct bool) multiArgsAggMemTest {
pub fn build_multi_args_agg_mem_tester() {
    multiArgsAggTest := buildMultiArgsAggTester(funcName, tps, rt, numRows)
    pt := multiArgsAggMemTest{
        multiArgsAggTest:            multiArgsAggTest,
        allocMemDelta:               allocMemDelta,
        multiArgsUpdateMemDeltaGens: updateMemDeltaGens,
        isDistinct:                  isDistinct,
    }
    return pt
}

// testMergePartialResult 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func testMergePartialResult(t *testing.T, p aggTest) {
pub fn test_merge_partial_result() {
    ctx := mock.NewContext()
    srcChk := p.genSrcChk()
    iter := chunk.NewIterator4Chunk(srcChk)

    args := []expression.Expression{&expression.Column{RetType: p.keyType, Index: 0}}
    ctor := collate.GetCollator(p.keyType.GetCollate())
    if p.funcName == ast.AggFuncGroupConcat {
        args = append(args, &expression.Constant{Value: types.NewStringDatum(separator), RetType: types.NewFieldType(mysql.TypeString)})
    }
    desc, err := aggregation.NewAggFuncDesc(ctx, p.funcName, args, false)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    if p.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    partialDesc, finalDesc := desc.Split([]int{0, 1})

    // build partial func for partial phase.
    partialFunc := aggfuncs.Build(ctx, partialDesc, 0)
    partialResult, _ := partialFunc.AllocPartialResult()

    // build final func for final phase.
    finalFunc := aggfuncs.Build(ctx, finalDesc, 0)
    finalPr, _ := finalFunc.AllocPartialResult()
    resultChk := chunk.NewChunkWithCapacity([]*types.FieldType{p.keyType}, 1)
    if p.funcName == ast.AggFuncApproxCountDistinct {
        resultChk = chunk.NewChunkWithCapacity([]*types.FieldType{types.NewFieldType(mysql.TypeString)}, 1)
    }
    if p.funcName == ast.AggFuncJsonArrayagg {
        resultChk = chunk.NewChunkWithCapacity([]*types.FieldType{types.NewFieldType(mysql.TypeJSON)}, 1)
    }

    // update partial result.
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        _, err = partialFunc.UpdatePartialResult(ctx, []chunk.Row{row}, partialResult)
        // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
        require.NoError(t, err)
    }
    p.messUpChunk(srcChk)
    err = partialFunc.AppendFinalResult2Chunk(ctx, partialResult, resultChk)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    dt := resultChk.GetRow(0).GetDatum(0, p.keyType)
    if p.funcName == ast.AggFuncApproxCountDistinct {
        dt = resultChk.GetRow(0).GetDatum(0, types.NewFieldType(mysql.TypeString))
    }
    if p.funcName == ast.AggFuncJsonArrayagg {
        dt = resultChk.GetRow(0).GetDatum(0, types.NewFieldType(mysql.TypeJSON))
    }
    result, err := dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[0], ctor)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    require.Equalf(t, 0, result, "%v != %v", dt.String(), p.results[0])

    _, err = finalFunc.MergePartialResult(ctx, partialResult, finalPr)
    require.NoError(t, err)
    partialFunc.ResetPartialResult(partialResult)

    srcChk = p.genSrcChk()
    iter = chunk.NewIterator4Chunk(srcChk)
    iter.Begin()
    iter.Next()
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Next(); row != iter.End(); row = iter.Next() {
        _, err = partialFunc.UpdatePartialResult(ctx, []chunk.Row{row}, partialResult)
        require.NoError(t, err)
    }
    p.messUpChunk(srcChk)
    resultChk.Reset()
    err = partialFunc.AppendFinalResult2Chunk(ctx, partialResult, resultChk)
    require.NoError(t, err)
    dt = resultChk.GetRow(0).GetDatum(0, p.keyType)
    if p.funcName == ast.AggFuncApproxCountDistinct {
        dt = resultChk.GetRow(0).GetDatum(0, types.NewFieldType(mysql.TypeString))
    }
    if p.funcName == ast.AggFuncJsonArrayagg {
        dt = resultChk.GetRow(0).GetDatum(0, types.NewFieldType(mysql.TypeJSON))
    }
    result, err = dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[1], ctor)
    require.NoError(t, err)
    require.Equalf(t, 0, result, "%v != %v", dt.String(), p.results[1])
    _, err = finalFunc.MergePartialResult(ctx, partialResult, finalPr)
    require.NoError(t, err)

    if p.funcName == ast.AggFuncApproxCountDistinct {
        resultChk = chunk.NewChunkWithCapacity([]*types.FieldType{types.NewFieldType(mysql.TypeLonglong)}, 1)
    }
    if p.funcName == ast.AggFuncJsonArrayagg {
        resultChk = chunk.NewChunkWithCapacity([]*types.FieldType{types.NewFieldType(mysql.TypeJSON)}, 1)
    }
    resultChk.Reset()
    err = finalFunc.AppendFinalResult2Chunk(ctx, finalPr, resultChk)
    require.NoError(t, err)

    dt = resultChk.GetRow(0).GetDatum(0, p.keyType)
    if p.funcName == ast.AggFuncApproxCountDistinct {
        dt = resultChk.GetRow(0).GetDatum(0, types.NewFieldType(mysql.TypeLonglong))
    }
    if p.funcName == ast.AggFuncJsonArrayagg {
        dt = resultChk.GetRow(0).GetDatum(0, types.NewFieldType(mysql.TypeJSON))
    }
    result, err = dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[2], ctor)
    require.NoError(t, err)
    require.Equalf(t, 0, result, "%v != %v", dt.String(), p.results[2])
}

// buildAggTester 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func buildAggTester(funcName string, keyTp byte, valTp byte, numRows int, results ...any) aggTest {
pub fn build_agg_tester() {
    var valFt *types.FieldType
    if valTp != 0 {
        valFt = types.NewFieldType(valTp)
    }
    return buildAggTesterWithFieldType(funcName, types.NewFieldType(keyTp), valFt, numRows, results...)
}

// buildAggTesterWithFieldType 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func buildAggTesterWithFieldType(funcName string, keyTt *types.FieldType, valFt *types.FieldType, numRows int, results ...any) aggTest {
pub fn build_agg_tester_with_field_type() {
    pt := aggTest{
        keyType:  keyTt,
        valType:  valFt,
        numRows:  numRows,
        funcName: funcName,
        dataGen:  getDataGenFunc(keyTt),
    }
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for _, result := range results {
        pt.results = append(pt.results, types.NewDatum(result))
    }
    return pt
}

// testMultiArgsMergePartialResult 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func testMultiArgsMergePartialResult(t *testing.T, ctx *mock.Context, p multiArgsAggTest) {
pub fn test_multi_args_merge_partial_result() {
    srcChk := p.genSrcChk()
    iter := chunk.NewIterator4Chunk(srcChk)

    args := make([]expression.Expression, len(p.dataTypes))
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for k := range p.dataTypes {
        args[k] = &expression.Column{RetType: p.dataTypes[k], Index: k}
    }

    desc, err := aggregation.NewAggFuncDesc(ctx, p.funcName, args, false)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    if p.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    ctor := collate.GetCollator(args[0].GetType(ctx).GetCollate())
    partialDesc, finalDesc := desc.Split([]int{0, 1})

    // build partial func for partial phase.
    partialFunc := aggfuncs.Build(ctx, partialDesc, 0)
    partialResult, _ := partialFunc.AllocPartialResult()

    // build final func for final phase.
    finalFunc := aggfuncs.Build(ctx, finalDesc, 0)
    finalPr, _ := finalFunc.AllocPartialResult()
    resultChk := chunk.NewChunkWithCapacity([]*types.FieldType{p.retType}, 1)

    // update partial result.
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        // FIXME: cannot assert error since there are cases of error, e.g. JSON documents may not contain NULL member
        _, _ = partialFunc.UpdatePartialResult(ctx, []chunk.Row{row}, partialResult)
    }
    p.messUpChunk(srcChk)
    err = partialFunc.AppendFinalResult2Chunk(ctx, partialResult, resultChk)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    dt := resultChk.GetRow(0).GetDatum(0, p.retType)
    result, err := dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[0], ctor)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.Zero(t, result)

    _, err = finalFunc.MergePartialResult(ctx, partialResult, finalPr)
    require.NoError(t, err)
    partialFunc.ResetPartialResult(partialResult)

    srcChk = p.genSrcChk()
    iter = chunk.NewIterator4Chunk(srcChk)
    iter.Begin()
    iter.Next()
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Next(); row != iter.End(); row = iter.Next() {
        // FIXME: cannot check error
        _, _ = partialFunc.UpdatePartialResult(ctx, []chunk.Row{row}, partialResult)
    }
    p.messUpChunk(srcChk)
    resultChk.Reset()
    err = partialFunc.AppendFinalResult2Chunk(ctx, partialResult, resultChk)
    require.NoError(t, err)
    dt = resultChk.GetRow(0).GetDatum(0, p.retType)
    result, err = dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[1], ctor)
    require.NoError(t, err)
    require.Zero(t, result)
    _, err = finalFunc.MergePartialResult(ctx, partialResult, finalPr)
    require.NoError(t, err)

    resultChk.Reset()
    err = finalFunc.AppendFinalResult2Chunk(ctx, finalPr, resultChk)
    require.NoError(t, err)

    dt = resultChk.GetRow(0).GetDatum(0, p.retType)
    result, err = dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[2], ctor)
    require.NoError(t, err)
    require.Zero(t, result)
}

// for multiple args in aggfuncs such as json_objectagg(c1, c2)
// buildMultiArgsAggTester 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func buildMultiArgsAggTester(funcName string, tps []byte, rt byte, numRows int, results ...any) multiArgsAggTest {
pub fn build_multi_args_agg_tester() {
    fts := make([]*types.FieldType, len(tps))
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for i := range tps {
        fts[i] = types.NewFieldType(tps[i])
    }
    return buildMultiArgsAggTesterWithFieldType(funcName, fts, types.NewFieldType(rt), numRows, results...)
}

// buildMultiArgsAggTesterWithFieldType 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func buildMultiArgsAggTesterWithFieldType(funcName string, fts []*types.FieldType, rt *types.FieldType, numRows int, results ...any) multiArgsAggTest {
pub fn build_multi_args_agg_tester_with_field_type() {
    dataGens := make([]func(i int) types.Datum, len(fts))
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for i := range fts {
        dataGens[i] = getDataGenFunc(fts[i])
    }
    mt := multiArgsAggTest{
        dataTypes: fts,
        retType:   rt,
        numRows:   numRows,
        funcName:  funcName,
        dataGens:  dataGens,
    }
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for _, result := range results {
        mt.results = append(mt.results, types.NewDatum(result))
    }
    return mt
}

// getDataGenFunc 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func getDataGenFunc(ft *types.FieldType) func(i int) types.Datum {
pub fn get_data_gen_func() {
    // switch 分支保留 Go 对类型、函数名或状态枚举的判定顺序。
    switch ft.GetType() {
    case mysql.TypeLonglong:
        return func(i int) types.Datum { return types.NewIntDatum(int64(i)) }
    case mysql.TypeFloat:
        return func(i int) types.Datum { return types.NewFloat32Datum(float32(i)) }
    case mysql.TypeNewDecimal:
        return func(i int) types.Datum { return types.NewDecimalDatum(types.NewDecFromInt(int64(i))) }
    case mysql.TypeDouble:
        return func(i int) types.Datum { return types.NewFloat64Datum(float64(i)) }
    case mysql.TypeString:
        return func(i int) types.Datum { return types.NewStringDatum(fmt.Sprintf("%d", i)) }
    case mysql.TypeDate:
        return func(i int) types.Datum { return types.NewTimeDatum(types.TimeFromDays(int64(i + 365))) }
    case mysql.TypeDuration:
        // Duration 换算保留 Go 时间单位语义，避免迁移时丢失毫秒/秒边界。
        return func(i int) types.Datum { return types.NewDurationDatum(types.Duration{Duration: time.Duration(i)}) }
    case mysql.TypeJSON:
        return func(i int) types.Datum { return types.NewDatum(types.CreateBinaryJSON(int64(i))) }
    case mysql.TypeEnum:
        elems := []string{"e", "d", "c", "b", "a"}
        return func(i int) types.Datum {
            e, _ := types.ParseEnumValue(elems, uint64(i+1))
            return types.NewCollateMysqlEnumDatum(e, ft.GetCollate())
        }
    case mysql.TypeSet:
        elems := []string{"e", "d", "c", "b", "a"}
        return func(i int) types.Datum {
            e, _ := types.ParseSetValue(elems, uint64(i+1))
            return types.NewMysqlSetDatum(e, ft.GetCollate())
        }
    }
    return nil
}

// testParallelDistinctAggFunc 对应 Go 的同名测试/辅助函数，保留 distinct 随机数据、NDV/null 分布和期望值计算语义。
// Go 签名: func testParallelDistinctAggFunc(t *testing.T, p parallelDistinctAggTestCase, multiArgs bool) {
pub fn test_parallel_distinct_agg_func() {
    ctx := mock.NewContext()

    var args []expression.Expression
    var ordinal []int
    if multiArgs {
        args = []expression.Expression{
            &expression.Column{RetType: p.dataTypes[0], Index: 0},
            &expression.Column{RetType: p.dataTypes[1], Index: 1},
        }
        ordinal = []int{0, 1}
    } else {
        args = []expression.Expression{&expression.Column{RetType: p.dataTypes[0], Index: 0}}
        ordinal = []int{0}

        // The second arg is useless, just for avoiding the panic in `desc.Split`
        if p.funcName == ast.AggFuncAvg {
            args = append(args, args...)
            ordinal = append(ordinal, 1)
        }
    }

    if p.funcName == ast.AggFuncGroupConcat {
        args = append(args, &expression.Constant{Value: types.NewStringDatum(separator), RetType: types.NewFieldType(mysql.TypeString)})
        ctx.ExprContext.SetGroupConcatMaxLenForTest(1000000) // Do not truncate
    }
    desc, err := aggregation.NewAggFuncDesc(ctx, p.funcName, args, true)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)

    partialDesc, finalDesc := desc.Split(ordinal)
    partialFunc := aggfuncs.Build(ctx, partialDesc, 0)
    finalFunc := aggfuncs.Build(ctx, finalDesc, 0)

    ctor := collate.GetCollator(finalDesc.RetTp.GetCollate())

    srcChkNum := len(p.srcChks)
    partialPtrs := make([]aggfuncs.PartialResult, 0, srcChkNum)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for range srcChkNum {
        ptr, _ := partialFunc.AllocPartialResult()
        partialPtrs = append(partialPtrs, ptr)
    }

    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for i := range srcChkNum {
        iter := chunk.NewIterator4Chunk(p.srcChks[i])
        // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
        for row := iter.Begin(); row != iter.End(); row = iter.Next() {
            _, err = partialFunc.UpdatePartialResult(ctx, []chunk.Row{row}, partialPtrs[i])
            // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
            require.NoError(t, err)
        }
    }

    for i := 1; i < srcChkNum; i++ {
        finalFunc.MergePartialResult(ctx, partialPtrs[i], partialPtrs[0])
    }

    resultChk := chunk.NewChunkWithCapacity([]*types.FieldType{desc.RetTp}, 1)
    err = finalFunc.AppendFinalResult2Chunk(ctx, partialPtrs[0], resultChk)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    dt := resultChk.GetRow(0).GetDatum(0, desc.RetTp)

    if p.funcName == ast.AggFuncGroupConcat {
        exp := p.result.GetString()
        act := dt.GetString()
        expectRes := strings.Split(exp, separator)
        actualRes := strings.Split(act, separator)
        if len(expectRes) != len(actualRes) {
            // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
            panic(fmt.Sprintf("expect len: %d, actual len: %d", len(expectRes), len(actualRes)))
        }

        slices.Sort(expectRes)
        slices.Sort(actualRes)

        for i := range expectRes {
            if expectRes[i] == actualRes[i] {
                continue
            }
            // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
            panic(fmt.Sprintf("i: %d, expect: %s, actual: %s", i, expectRes[i], actualRes[i]))
        }
        return
    }

    if dt.Kind() == types.KindFloat64 {
        // Truncate the float, as float is imprecise and the tailing numbers may be different
        floatNum := dt.GetFloat64()
        floatStr := fmt.Sprintf("%.2f", floatNum)
        floatNum, err = strconv.ParseFloat(floatStr, 64)
        // 错误分支保留 Go 的显式 err 检查，Rust 接线时应改为 Result 传播或断言。
        if err != nil {
            // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
            panic(err)
        }
        dt = types.NewFloat64Datum(floatNum)

        floatNum = p.result.GetFloat64()
        floatStr = fmt.Sprintf("%.2f", floatNum)
        floatNum, err = strconv.ParseFloat(floatStr, 64)
        // 错误分支保留 Go 的显式 err 检查，Rust 接线时应改为 Result 传播或断言。
        if err != nil {
            // panic 保留 Go 测试中不可达或不支持分支；Rust 接线时应改成明确错误或断言。
            panic(err)
        }
        p.result = types.NewFloat64Datum(floatNum)
    }
    result, err := dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.result, ctor)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    require.Equalf(t, 0, result, "expect: %v, actual: %v", dt.String(), p.result)
}

// testAggFunc 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func testAggFunc(t *testing.T, p aggTest) {
pub fn test_agg_func() {
    srcChk := p.genSrcChk()
    ctx := mock.NewContext()

    args := []expression.Expression{&expression.Column{RetType: p.keyType, Index: 0}}
    ctor := collate.GetCollator(p.keyType.GetCollate())
    if p.funcName == ast.AggFuncGroupConcat {
        args = append(args, &expression.Constant{Value: types.NewStringDatum(separator), RetType: types.NewFieldType(mysql.TypeString)})
    }
    if p.funcName == ast.AggFuncApproxPercentile {
        args = append(args, &expression.Constant{Value: types.NewIntDatum(50), RetType: types.NewFieldType(mysql.TypeLong)})
    }
    desc, err := aggregation.NewAggFuncDesc(ctx, p.funcName, args, false)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    if p.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    finalFunc := aggfuncs.Build(ctx, desc, 0)
    finalPr, _ := finalFunc.AllocPartialResult()
    resultChk := chunk.NewChunkWithCapacity([]*types.FieldType{desc.RetTp}, 1)

    iter := chunk.NewIterator4Chunk(srcChk)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        _, err = finalFunc.UpdatePartialResult(ctx, []chunk.Row{row}, finalPr)
        // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
        require.NoError(t, err)
    }
    p.messUpChunk(srcChk)
    err = finalFunc.AppendFinalResult2Chunk(ctx, finalPr, resultChk)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    dt := resultChk.GetRow(0).GetDatum(0, desc.RetTp)
    result, err := dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[1], ctor)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    require.Equalf(t, 0, result, "%v != %v", dt.String(), p.results[1])

    // test the empty input
    resultChk.Reset()
    finalFunc.ResetPartialResult(finalPr)
    err = finalFunc.AppendFinalResult2Chunk(ctx, finalPr, resultChk)
    require.NoError(t, err)
    dt = resultChk.GetRow(0).GetDatum(0, desc.RetTp)
    result, err = dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[0], ctor)
    require.NoError(t, err)
    require.Equalf(t, 0, result, "%v != %v", dt.String(), p.results[0])

    // test the agg func with distinct
    desc, err = aggregation.NewAggFuncDesc(ctx, p.funcName, args, true)
    require.NoError(t, err)
    if p.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    finalFunc = aggfuncs.Build(ctx, desc, 0)
    finalPr, _ = finalFunc.AllocPartialResult()

    resultChk.Reset()
    srcChk = p.genSrcChk()
    iter = chunk.NewIterator4Chunk(srcChk)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        _, err = finalFunc.UpdatePartialResult(ctx, []chunk.Row{row}, finalPr)
        require.NoError(t, err)
    }
    p.messUpChunk(srcChk)
    srcChk = p.genSrcChk()
    iter = chunk.NewIterator4Chunk(srcChk)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        _, err = finalFunc.UpdatePartialResult(ctx, []chunk.Row{row}, finalPr)
        require.NoError(t, err)
    }
    p.messUpChunk(srcChk)
    err = finalFunc.AppendFinalResult2Chunk(ctx, finalPr, resultChk)
    require.NoError(t, err)
    dt = resultChk.GetRow(0).GetDatum(0, desc.RetTp)
    result, err = dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[1], ctor)
    require.NoError(t, err)
    require.Equalf(t, 0, result, "%v != %v", dt.String(), p.results[1])
}

// testAggFuncWithoutDistinct 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func testAggFuncWithoutDistinct(t *testing.T, p aggTest) {
pub fn test_agg_func_without_distinct() {
    srcChk := p.genSrcChk()

    args := []expression.Expression{&expression.Column{RetType: p.keyType, Index: 0}}
    ctor := collate.GetCollator(p.keyType.GetCollate())
    if p.funcName == ast.AggFuncGroupConcat {
        args = append(args, &expression.Constant{Value: types.NewStringDatum(separator), RetType: types.NewFieldType(mysql.TypeString)})
    }
    if p.funcName == ast.AggFuncApproxPercentile {
        args = append(args, &expression.Constant{Value: types.NewIntDatum(50), RetType: types.NewFieldType(mysql.TypeLong)})
    }
    ctx := mock.NewContext()
    desc, err := aggregation.NewAggFuncDesc(ctx, p.funcName, args, false)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    if p.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    finalFunc := aggfuncs.Build(ctx, desc, 0)
    finalPr, _ := finalFunc.AllocPartialResult()
    resultChk := chunk.NewChunkWithCapacity([]*types.FieldType{desc.RetTp}, 1)

    iter := chunk.NewIterator4Chunk(srcChk)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        _, err = finalFunc.UpdatePartialResult(ctx, []chunk.Row{row}, finalPr)
        // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
        require.NoError(t, err)
    }
    p.messUpChunk(srcChk)
    err = finalFunc.AppendFinalResult2Chunk(ctx, finalPr, resultChk)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    dt := resultChk.GetRow(0).GetDatum(0, desc.RetTp)
    result, err := dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[1], ctor)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    require.Zerof(t, result, "%v != %v", dt.String(), p.results[1])

    // test the empty input
    resultChk.Reset()
    finalFunc.ResetPartialResult(finalPr)
    err = finalFunc.AppendFinalResult2Chunk(ctx, finalPr, resultChk)
    require.NoError(t, err)
    dt = resultChk.GetRow(0).GetDatum(0, desc.RetTp)
    result, err = dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[0], ctor)
    require.NoError(t, err)
    require.Zerof(t, result, "%v != %v", dt.String(), p.results[0])
}

// testAggMemFunc 对应 Go 的同名测试/辅助函数，保留内存增量估算、更新路径和错误返回语义。
// Go 签名: func testAggMemFunc(t *testing.T, p aggMemTest) {
pub fn test_agg_mem_func() {
    srcChk := p.aggTest.genSrcChk()
    ctx := mock.NewContext()

    args := []expression.Expression{&expression.Column{RetType: p.aggTest.keyType, Index: 0}}
    if p.aggTest.funcName == ast.AggFuncGroupConcat {
        args = append(args, &expression.Constant{Value: types.NewStringDatum(separator), RetType: types.NewFieldType(mysql.TypeString)})
    }
    desc, err := aggregation.NewAggFuncDesc(ctx, p.aggTest.funcName, args, p.isDistinct)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    if p.aggTest.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    finalFunc := aggfuncs.Build(ctx, desc, 0)
    finalPr, memDelta := finalFunc.AllocPartialResult()
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.Equal(t, p.allocMemDelta, memDelta)

    updateMemDeltas, err := p.updateMemDeltaGens(updateMemDeltaGensParams{srcChk: srcChk, keyType: p.aggTest.keyType, valType: p.aggTest.valType})
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    iter := chunk.NewIterator4Chunk(srcChk)
    i := 0
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        memDelta, err := finalFunc.UpdatePartialResult(ctx, []chunk.Row{row}, finalPr)
        // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
        require.NoError(t, err)
        require.Equal(t, updateMemDeltas[i], memDelta)
        i++
    }
}

// testMultiArgsAggFunc 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func testMultiArgsAggFunc(t *testing.T, ctx *mock.Context, p multiArgsAggTest) {
pub fn test_multi_args_agg_func() {
    srcChk := p.genSrcChk()

    args := make([]expression.Expression, len(p.dataTypes))
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for k := range p.dataTypes {
        args[k] = &expression.Column{RetType: p.dataTypes[k], Index: k}
    }
    if p.funcName == ast.AggFuncGroupConcat {
        args = append(args, &expression.Constant{Value: types.NewStringDatum(separator), RetType: types.NewFieldType(mysql.TypeString)})
    }

    desc, err := aggregation.NewAggFuncDesc(ctx, p.funcName, args, false)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    if p.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    ctor := collate.GetCollator(args[0].GetType(ctx).GetCollate())
    finalFunc := aggfuncs.Build(ctx, desc, 0)
    finalPr, _ := finalFunc.AllocPartialResult()
    resultChk := chunk.NewChunkWithCapacity([]*types.FieldType{desc.RetTp}, 1)

    iter := chunk.NewIterator4Chunk(srcChk)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        // FIXME: cannot assert error since there are cases of error, e.g. rows were cut by GROUPCONCAT
        _, _ = finalFunc.UpdatePartialResult(ctx, []chunk.Row{row}, finalPr)
    }
    p.messUpChunk(srcChk)
    err = finalFunc.AppendFinalResult2Chunk(ctx, finalPr, resultChk)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    dt := resultChk.GetRow(0).GetDatum(0, desc.RetTp)
    result, err := dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[1], ctor)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.Zerof(t, result, "%v != %v", dt.String(), p.results[1])

    // test the empty input
    resultChk.Reset()
    finalFunc.ResetPartialResult(finalPr)
    err = finalFunc.AppendFinalResult2Chunk(ctx, finalPr, resultChk)
    require.NoError(t, err)
    dt = resultChk.GetRow(0).GetDatum(0, desc.RetTp)
    result, err = dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[0], ctor)
    require.NoError(t, err)
    require.Zerof(t, result, "%v != %v", dt.String(), p.results[0])

    // test the agg func with distinct
    desc, err = aggregation.NewAggFuncDesc(ctx, p.funcName, args, true)
    require.NoError(t, err)
    if p.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    finalFunc = aggfuncs.Build(ctx, desc, 0)
    finalPr, _ = finalFunc.AllocPartialResult()

    resultChk.Reset()
    srcChk = p.genSrcChk()
    iter = chunk.NewIterator4Chunk(srcChk)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        // FIXME: cannot check error
        _, _ = finalFunc.UpdatePartialResult(ctx, []chunk.Row{row}, finalPr)
    }
    p.messUpChunk(srcChk)
    srcChk = p.genSrcChk()
    iter = chunk.NewIterator4Chunk(srcChk)
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        // FIXME: cannot check error
        _, _ = finalFunc.UpdatePartialResult(ctx, []chunk.Row{row}, finalPr)
    }
    p.messUpChunk(srcChk)
    err = finalFunc.AppendFinalResult2Chunk(ctx, finalPr, resultChk)
    require.NoError(t, err)
    dt = resultChk.GetRow(0).GetDatum(0, desc.RetTp)
    result, err = dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[1], ctor)
    require.NoError(t, err)
    require.Zerof(t, result, "%v != %v", dt.String(), p.results[1])

    // test the empty input
    resultChk.Reset()
    finalFunc.ResetPartialResult(finalPr)
    err = finalFunc.AppendFinalResult2Chunk(ctx, finalPr, resultChk)
    require.NoError(t, err)
    dt = resultChk.GetRow(0).GetDatum(0, desc.RetTp)
    result, err = dt.Compare(ctx.GetSessionVars().StmtCtx.TypeCtx(), &p.results[0], ctor)
    require.NoError(t, err)
    require.Zero(t, result)
}

// testMultiArgsAggMemFunc 对应 Go 的同名测试/辅助函数，保留内存增量估算、更新路径和错误返回语义。
// Go 签名: func testMultiArgsAggMemFunc(t *testing.T, p multiArgsAggMemTest) {
pub fn test_multi_args_agg_mem_func() {
    srcChk := p.multiArgsAggTest.genSrcChk()
    ctx := mock.NewContext()

    args := make([]expression.Expression, len(p.multiArgsAggTest.dataTypes))
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for k := range p.multiArgsAggTest.dataTypes {
        args[k] = &expression.Column{RetType: p.multiArgsAggTest.dataTypes[k], Index: k}
    }
    if p.multiArgsAggTest.funcName == ast.AggFuncGroupConcat {
        args = append(args, &expression.Constant{Value: types.NewStringDatum(separator), RetType: types.NewFieldType(mysql.TypeString)})
    }

    desc, err := aggregation.NewAggFuncDesc(ctx, p.multiArgsAggTest.funcName, args, p.isDistinct)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    if p.multiArgsAggTest.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    finalFunc := aggfuncs.Build(ctx, desc, 0)
    finalPr, memDelta := finalFunc.AllocPartialResult()
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.Equal(t, p.allocMemDelta, memDelta)

    updateMemDeltas, err := p.multiArgsUpdateMemDeltaGens(ctx, srcChk, p.multiArgsAggTest.dataTypes, desc.OrderByItems)
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)
    iter := chunk.NewIterator4Chunk(srcChk)
    i := 0
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        memDelta, _ := finalFunc.UpdatePartialResult(ctx, []chunk.Row{row}, finalPr)
        // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
        require.Equal(t, updateMemDeltas[i], memDelta)
        i++
    }
}

// benchmarkAggFunc 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func benchmarkAggFunc(b *testing.B, ctx *mock.Context, p aggTest) {
pub fn benchmark_agg_func() {
    srcChk := chunk.NewChunkWithCapacity([]*types.FieldType{p.keyType}, p.numRows)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for i := range p.numRows {
        dt := p.dataGen(i)
        srcChk.AppendDatum(0, &dt)
    }
    srcChk.AppendDatum(0, &types.Datum{})

    args := []expression.Expression{&expression.Column{RetType: p.keyType, Index: 0}}
    if p.funcName == ast.AggFuncGroupConcat {
        args = append(args, &expression.Constant{Value: types.NewStringDatum(separator), RetType: types.NewFieldType(mysql.TypeString)})
    }
    desc, err := aggregation.NewAggFuncDesc(ctx, p.funcName, args, false)
    // 错误分支保留 Go 的显式 err 检查，Rust 接线时应改为 Result 传播或断言。
    if err != nil {
        b.Fatal(err)
    }
    if p.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    finalFunc := aggfuncs.Build(ctx, desc, 0)
    resultChk := chunk.NewChunkWithCapacity([]*types.FieldType{desc.RetTp}, 1)
    iter := chunk.NewIterator4Chunk(srcChk)
    input := make([]chunk.Row, 0, iter.Len())
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        input = append(input, row)
    }
    b.Run(fmt.Sprintf("%v/%v", p.funcName, p.keyType), func(b *testing.B) {
        baseBenchmarkAggFunc(b, ctx, finalFunc, input, resultChk)
    })

    desc, err = aggregation.NewAggFuncDesc(ctx, p.funcName, args, true)
    // 错误分支保留 Go 的显式 err 检查，Rust 接线时应改为 Result 传播或断言。
    if err != nil {
        b.Fatal(err)
    }
    if p.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    finalFunc = aggfuncs.Build(ctx, desc, 0)
    resultChk.Reset()
    b.Run(fmt.Sprintf("%v(distinct)/%v", p.funcName, p.keyType), func(b *testing.B) {
        baseBenchmarkAggFunc(b, ctx, finalFunc, input, resultChk)
    })
}

// benchmarkMultiArgsAggFunc 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func benchmarkMultiArgsAggFunc(b *testing.B, ctx *mock.Context, p multiArgsAggTest) {
pub fn benchmark_multi_args_agg_func() {
    srcChk := chunk.NewChunkWithCapacity(p.dataTypes, p.numRows)
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for i := range p.numRows {
        // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
        for j := range p.dataGens {
            fdt := p.dataGens[j](i)
            srcChk.AppendDatum(j, &fdt)
        }
    }
    srcChk.AppendDatum(0, &types.Datum{})

    args := make([]expression.Expression, len(p.dataTypes))
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for k := range p.dataTypes {
        args[k] = &expression.Column{RetType: p.dataTypes[k], Index: k}
    }
    if p.funcName == ast.AggFuncGroupConcat {
        args = append(args, &expression.Constant{Value: types.NewStringDatum(separator), RetType: types.NewFieldType(mysql.TypeString)})
    }

    desc, err := aggregation.NewAggFuncDesc(ctx, p.funcName, args, false)
    // 错误分支保留 Go 的显式 err 检查，Rust 接线时应改为 Result 传播或断言。
    if err != nil {
        b.Fatal(err)
    }
    if p.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    finalFunc := aggfuncs.Build(ctx, desc, 0)
    resultChk := chunk.NewChunkWithCapacity([]*types.FieldType{desc.RetTp}, 1)
    iter := chunk.NewIterator4Chunk(srcChk)
    input := make([]chunk.Row, 0, iter.Len())
    for row := iter.Begin(); row != iter.End(); row = iter.Next() {
        input = append(input, row)
    }
    b.Run(fmt.Sprintf("%v/%v", p.funcName, p.dataTypes), func(b *testing.B) {
        baseBenchmarkAggFunc(b, ctx, finalFunc, input, resultChk)
    })

    desc, err = aggregation.NewAggFuncDesc(ctx, p.funcName, args, true)
    // 错误分支保留 Go 的显式 err 检查，Rust 接线时应改为 Result 传播或断言。
    if err != nil {
        b.Fatal(err)
    }
    if p.orderBy {
        desc.OrderByItems = []*util.ByItems{
            {Expr: args[0], Desc: true},
        }
    }
    finalFunc = aggfuncs.Build(ctx, desc, 0)
    resultChk.Reset()
    b.Run(fmt.Sprintf("%v(distinct)/%v", p.funcName, p.dataTypes), func(b *testing.B) {
        baseBenchmarkAggFunc(b, ctx, finalFunc, input, resultChk)
    })
}

// baseBenchmarkAggFunc 对应 Go 的同名测试/辅助函数，保留 Go 辅助函数的参数意图、返回值和主要控制流。
// Go 签名: func baseBenchmarkAggFunc(b *testing.B, ctx aggfuncs.AggFuncUpdateContext, finalFunc aggfuncs.AggFunc, input []chunk.Row, output *chunk.Chunk) {
pub fn base_benchmark_agg_func() {
    finalPr, _ := finalFunc.AllocPartialResult()
    output.Reset()
    b.ResetTimer()
    // range/for 循环保留 Go 表驱动、chunk 行迭代或 benchmark 循环顺序。
    for i := 0; i < b.N; i++ {
        _, err := finalFunc.UpdatePartialResult(ctx, input, finalPr)
        // 错误分支保留 Go 的显式 err 检查，Rust 接线时应改为 Result 传播或断言。
        if err != nil {
            b.Fatal(err)
        }
        b.StopTimer()
        output.Reset()
        b.StartTimer()
    }
}

// TestAggApproxCountDistinctPushDown 对应 Go 的同名测试/辅助函数，保留初始化、fixture、断言和清理顺序。
// Go 签名: func TestAggApproxCountDistinctPushDown(t *testing.T) {
#[test]
pub fn TestAggApproxCountDistinctPushDown() {
    ctx := mock.NewContext()

    args := make([]expression.Expression, 0)
    args = append(args, &expression.Column{
        RetType: types.NewFieldType(mysql.TypeLonglong),
        ID:      1,
        Index:   int(1),
    })

    aggDesc, err := aggregation.NewAggFuncDesc(ctx, ast.AggFuncApproxCountDistinct, args, false)

    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.NoError(t, err)

    // can only pushdown to TiFlash
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.True(t, aggregation.CheckAggPushDown(ctx, aggDesc, kv.TiFlash))
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.False(t, aggregation.CheckAggPushDown(ctx, aggDesc, kv.TiKV))
    // require 断言保留 Go 测试期望；后续可映射为 Rust assert/result 检查。
    require.False(t, aggregation.CheckAggPushDown(ctx, aggDesc, kv.TiDB))
    require.False(t, aggregation.CheckAggPushDown(ctx, aggDesc, kv.UnSpecified))
}
*/

/// 可执行契约测试依赖的生产聚合实现。
use crate::func_avg::FloatAvg;
use crate::func_count::CountAggregator;

/// 验证 Go 通用聚合夹具的核心契约：NULL 跳过、partial/final 合并、
/// 空输入结果、reset 以及 reset 后的状态复用。
#[test]
fn generic_aggregate_partial_final_and_reset_contract_matches_go() {
    let empty = FloatAvg::default();
    assert_eq!(empty.partial_result(), (0, 0.0));
    assert_eq!(empty.result(), None);

    let mut left = FloatAvg::default();
    left.update([Some(1.0), None, Some(3.0)]);
    let mut right = FloatAvg::default();
    right.update([Some(5.0), Some(7.0)]);
    left.merge(&right);
    assert_eq!(left.partial_result(), (4, 16.0));
    assert_eq!(left.result(), Some(4.0));
    left.reset();
    assert_eq!(left.partial_result(), (0, 0.0));
    assert_eq!(left.result(), None);
    left.update_partial([Some((2, 10.0)), None, Some((0, 99.0))]);
    // Go updates the partial sum before adding its count, so a non-null sum
    // remains observable even when the corresponding count is zero.
    assert_eq!(left.partial_result(), (2, 109.0));
    assert_eq!(left.result(), Some(54.5));

    let mut count = CountAggregator::default();
    count.update([Some(1), None, Some(2)]).unwrap();
    assert_eq!(count.value(), 2);
    count.update_partial([Some(3), None]).unwrap();
    assert_eq!(count.value(), 5);

    let mut other_count = CountAggregator::default();
    other_count.update([Some("x"), None, Some("y")]).unwrap();
    count.merge(&other_count).unwrap();
    assert_eq!(count.value(), 7);
    count.reset();
    assert_eq!(count.value(), 0);
    count.update([Some(true), None]).unwrap();
    assert_eq!(count.value(), 1);
}
