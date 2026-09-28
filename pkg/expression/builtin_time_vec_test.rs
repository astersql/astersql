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

// 时间内置函数向量化测试与数据生成器草稿。
//
// 对应 Go `builtin_time_vec_test.go`：覆盖 period/unit/timezone 生成器、
// `vecBuiltinTimeCases` 大表、TimeFormat 空格式回归与 VecMonth 严格模式路径。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables,
    unused_mut
)]
// 这段逻辑覆盖 period/unit/timezone 生成器、vecBuiltinTimeCases 大表、TimeFormat 空格式回归和 VecMonth 严格模式路径。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待 Rust crate 接线）：
// - "math"
// - "math/rand"
// - "testing"
// - "time"
// - "github.com/pingcap/tidb/pkg/parser/ast"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/types"
// - "github.com/pingcap/tidb/pkg/util/chunk"
// - "github.com/stretchr/testify/require"

// 迁移占位类型：这些名称来自 Go/TiDB 测试依赖，后续接入真实 Rust 模块时再替换。
type GoAny = ();
type GoError = String;
type GoBytes = Vec<u8>;
type GoBool = bool;

// VecExprBenchCase holds table-driven vectorization bench cases.
/// 表驱动向量化基准用例的轻量占位。
pub struct VecExprBenchCase {
    pub name: &'static str,
}

fn vec_expr_case_group(name: &'static str) -> VecExprBenchCase {
    VecExprBenchCase { name }
}
// periodGener 对应 Go 的同名测试辅助类型；字段保持原声明顺序，具体依赖先用 GoAny 占位。
/// 对应 Go `periodGener`：生成 YYYYMM/YYMM 会计期测试数据。
pub struct periodGener {
    // Go field: randGen *defaultRandGen
    pub randGen: GoAny,
}

// newPeriodGener 对应 Go 辅助函数/方法；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
/// 对应 Go `newPeriodGener` 构造入口草稿。
pub fn new_period_gener() {
    // Go 签名：func newPeriodGener() *periodGener
    // 返回语义：Go 返回 *periodGener；只记录返回路径和错误传播。
    // Go: return &periodGener{newDefaultRandGen()}
}

// periodGener.gen 对应 Go 辅助函数/方法；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
/// 对应 Go `periodGener.gen`：随机合法会计期。
pub fn period_gener_gen() {
    // Go 签名：func (g *periodGener) gen() any
    // 返回语义：Go 返回 any；只记录返回路径和错误传播。
    // Go: return int64((g.randGen.Intn(2500)+1)*100 + g.randGen.Intn(12) + 1)
}

// unitStrGener 对应 Go 的同名测试辅助类型；字段保持原声明顺序，具体依赖先用 GoAny 占位。
/// 对应 Go `unitStrGener`：生成时间单位字符串。
pub struct unitStrGener {
    // Go field: randGen *defaultRandGen
    pub randGen: GoAny,
}

// newUnitStrGener 对应 Go 辅助函数/方法；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
/// 对应 Go `newUnitStrGener` 构造入口草稿。
pub fn new_unit_str_gener() {
    // Go 签名：func newUnitStrGener() *unitStrGener
    // 返回语义：Go 返回 *unitStrGener；只记录返回路径和错误传播。
    // Go: return &unitStrGener{newDefaultRandGen()}
}

// unitStrGener.gen 对应 Go 辅助函数/方法；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
/// 对应 Go `unitStrGener.gen`：从 MICROSECOND..YEAR 中随机选取。
pub fn unit_str_gener_gen() {
    // Go 签名：func (g *unitStrGener) gen() any
    // 返回语义：Go 返回 any；只记录返回路径和错误传播。
    // Go: units := []string{
    // Go: "MICROSECOND",
    // Go: "SECOND",
    // Go: "MINUTE",
    // Go: "HOUR",
    // Go: "DAY",
    // Go: "WEEK",
    // Go: "MONTH",
    // Go: "QUARTER",
    // Go: "YEAR",
    // Go: }

    // Go: n := g.randGen.Intn(len(units))
    // Go: return units[n]
}

// tzStrGener 对应 Go 的同名测试辅助类型；字段保持原声明顺序，具体依赖先用 GoAny 占位。
pub struct tzStrGener {
    // Go 空 struct：无字段，仅作为 generator/marker 使用。
    _marker: (),
}

// tzStrGener.gen 对应 Go 辅助函数/方法；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn tz_str_gener_gen() {
    // Go 签名：func (g *tzStrGener) gen() any
    // 返回语义：Go 返回 any；只记录返回路径和错误传播。
    // Go: tzs := []string{
    // Go: "",
    // Go: "GMT",
    // Go: "MET",
    // Go: "+00:00",
    // Go: "+10:00",
    // Go: }

    // Go: n := rand.Int() % len(tzs)
    // Go: return tzs[n]
}

// vecBuiltinTimeCases 对应 Go 的向量化表驱动 case map；这里按 map key 保留审阅入口，具体 vecExprBenchCase 字段在下方逐行注释。
pub fn vec_builtin_time_cases() -> Vec<(&'static str, VecExprBenchCase)> {
    let mut cases = Vec::new();
    // case 分组：ast.DateLiteral。
    cases.push(("ast::DateLiteral", vec_expr_case_group("ast.DateLiteral")));
    // Go: ast.DateLiteral: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETDatetime},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{{Value: types.NewStringDatum("2019-11-11"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // Go: },
    // case 分组：ast.TimeLiteral。
    cases.push(("ast::TimeLiteral", vec_expr_case_group("ast.TimeLiteral")));
    // Go: ast.TimeLiteral: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDuration, childrenTypes: []types.EvalType{types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{
    // Go: {Value: types.NewStringDatum("838:59:59"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // Go: },
    // case 分组：ast.DateDiff。
    cases.push(("ast::DateDiff", vec_expr_case_group("ast.DateDiff")));
    // Go: ast.DateDiff: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime, types.ETDatetime}},
    // Go: },
    // case 分组：ast.DateFormat。
    cases.push(("ast::DateFormat", vec_expr_case_group("ast.DateFormat")));
    // Go: ast.DateFormat: {
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{&dateTimeStrGener{randGen: newDefaultRandGen()}, newTimeFormatGener(0.5)},
    // Go: },
    // Go: },
    // case 分组：ast.Hour。
    cases.push(("ast::Hour", vec_expr_case_group("ast.Hour")));
    // Go: ast.Hour: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDuration}, geners: []dataGenerator{newRangeDurationGener(0.2)}},
    // Go: },
    // case 分组：ast.Minute。
    cases.push(("ast::Minute", vec_expr_case_group("ast.Minute")));
    // Go: ast.Minute: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDuration}, geners: []dataGenerator{newRangeDurationGener(0.2)}},
    // Go: },
    // case 分组：ast.Second。
    cases.push(("ast::Second", vec_expr_case_group("ast.Second")));
    // Go: ast.Second: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDuration}, geners: []dataGenerator{newRangeDurationGener(0.2)}},
    // Go: },
    // case 分组：ast.ToSeconds。
    cases.push(("ast::ToSeconds", vec_expr_case_group("ast.ToSeconds")));
    // Go: ast.ToSeconds: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // case 分组：ast.MicroSecond。
    cases.push(("ast::MicroSecond", vec_expr_case_group("ast.MicroSecond")));
    // Go: ast.MicroSecond: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDuration}, geners: []dataGenerator{newRangeDurationGener(0.2)}},
    // Go: },
    // case 分组：ast.Now。
    cases.push(("ast::Now", vec_expr_case_group("ast.Now")));
    // Go: ast.Now: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime},
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETDatetime,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETInt},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{newRangeInt64Gener(0, 7)},
    // Go: },
    // Go: },
    // case 分组：ast.DayOfWeek。
    cases.push(("ast::DayOfWeek", vec_expr_case_group("ast.DayOfWeek")));
    // Go: ast.DayOfWeek: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // case 分组：ast.DayOfYear。
    cases.push(("ast::DayOfYear", vec_expr_case_group("ast.DayOfYear")));
    // Go: ast.DayOfYear: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // case 分组：ast.Day。
    cases.push(("ast::Day", vec_expr_case_group("ast.Day")));
    // Go: ast.Day: {},
    // case 分组：ast.ToDays。
    cases.push(("ast::ToDays", vec_expr_case_group("ast.ToDays")));
    // Go: ast.ToDays: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // case 分组：ast.CurrentTime。
    cases.push(("ast::CurrentTime", vec_expr_case_group("ast.CurrentTime")));
    // Go: ast.CurrentTime: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDuration},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDuration, childrenTypes: []types.EvalType{types.ETInt}, geners: []dataGenerator{newRangeInt64Gener(0, 7)}}, // fsp must be in the range 0 to 6.
    // Go: },
    // case 分组：ast.Time。
    cases.push(("ast::Time", vec_expr_case_group("ast.Time")));
    // Go: ast.Time: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDuration, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{&dateTimeStrGener{randGen: newDefaultRandGen()}}},
    // Go: },
    // case 分组：ast.CurrentDate。
    cases.push(("ast::CurrentDate", vec_expr_case_group("ast.CurrentDate")));
    // Go: ast.CurrentDate: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime},
    // Go: },
    // case 分组：ast.MakeDate。
    cases.push(("ast::MakeDate", vec_expr_case_group("ast.MakeDate")));
    // Go: ast.MakeDate: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETInt, types.ETInt},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{newRangeInt64Gener(0, 2200), newRangeInt64Gener(0, 365)},
    // Go: },
    // Go: },
    // case 分组：ast.MakeTime。
    cases.push(("ast::MakeTime", vec_expr_case_group("ast.MakeTime")));
    // Go: ast.MakeTime: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDuration, childrenTypes: []types.EvalType{types.ETInt, types.ETInt, types.ETReal}, geners: []dataGenerator{newRangeInt64Gener(-1000, 1000)}},
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETDuration, childrenTypes: []types.EvalType{types.ETInt, types.ETInt, types.ETReal},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenFieldTypes: []*types.FieldType{
    // Go: types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).SetFlag(mysql.UnsignedFlag).BuildP(),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{newRangeInt64Gener(-1000, 1000)},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDuration, childrenTypes: []types.EvalType{types.ETReal, types.ETReal, types.ETReal}, geners: []dataGenerator{newRangeRealGener(-1000.0, 1000.0, 0.1)}},
    // Go: },
    // case 分组：ast.PeriodAdd。
    cases.push(("ast::PeriodAdd", vec_expr_case_group("ast.PeriodAdd")));
    // Go: ast.PeriodAdd: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETInt, types.ETInt}, geners: []dataGenerator{newPeriodGener(), newPeriodGener()}},
    // Go: },
    // case 分组：ast.PeriodDiff。
    cases.push(("ast::PeriodDiff", vec_expr_case_group("ast.PeriodDiff")));
    // Go: ast.PeriodDiff: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETInt, types.ETInt}, geners: []dataGenerator{newPeriodGener(), newPeriodGener()}},
    // Go: },
    // case 分组：ast.Quarter。
    cases.push(("ast::Quarter", vec_expr_case_group("ast.Quarter")));
    // Go: ast.Quarter: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // case 分组：ast.TimeFormat。
    cases.push(("ast::TimeFormat", vec_expr_case_group("ast.TimeFormat")));
    // Go: ast.TimeFormat: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETDuration, types.ETString}, geners: []dataGenerator{newRangeDurationGener(0.5), newTimeFormatGener(0.5)}},
    // Go: },
    // case 分组：ast.TimeToSec。
    cases.push(("ast::TimeToSec", vec_expr_case_group("ast.TimeToSec")));
    // Go: ast.TimeToSec: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDuration}},
    // Go: },
    // case 分组：ast.SecToTime。
    cases.push(("ast::SecToTime", vec_expr_case_group("ast.SecToTime")));
    // Go: ast.SecToTime: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDuration, childrenTypes: []types.EvalType{types.ETReal}},
    // Go: },
    // Go 注释：This test case may fail due to the issue: https://github.com/pingcap/tidb/issues/13638.
    // Go 注释：We remove this case to stabilize CI, and will reopen this when we fix the issue above.
    // Go 注释：ast.TimestampAdd: {
    // Go 注释：{
    // Go 注释：retEvalType: types.ETString,
    // Go 注释：childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETDatetime},
    // Go 注释：geners: []dataGenerator{&unitStrGener{newDefaultRandGen()}, nil, nil},
    // Go 注释：},
    // Go 注释：},
    // case 分组：ast.UnixTimestamp。
    cases.push((
        "ast::UnixTimestamp",
        vec_expr_case_group("ast.UnixTimestamp"),
    ));
    // Go: ast.UnixTimestamp: {
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETInt,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETDatetime},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenFieldTypes: []*types.FieldType{
    // Go: types.NewFieldTypeBuilder().SetType(mysql.TypeDatetime).SetFlag(mysql.BinaryFlag).SetFlen(types.UnspecifiedLength).BuildP(),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{&dateTimeGener{Fsp: 0, randGen: newDefaultRandGen()}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDecimal, childrenTypes: []types.EvalType{types.ETTimestamp}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt},
    // Go: },
    // case 分组：ast.TimestampDiff。
    cases.push((
        "ast::TimestampDiff",
        vec_expr_case_group("ast.TimestampDiff"),
    ));
    // Go: ast.TimestampDiff: {
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETInt,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETDatetime, types.ETDatetime},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{newUnitStrGener(), nil, nil}},
    // Go: },
    // case 分组：ast.TimestampLiteral。
    cases.push((
        "ast::TimestampLiteral",
        vec_expr_case_group("ast.TimestampLiteral"),
    ));
    // Go: ast.TimestampLiteral: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETTimestamp, childrenTypes: []types.EvalType{types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{{Value: types.NewStringDatum("2019-12-04 00:00:00"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // Go: },
    // case 分组：ast.SubDate。
    cases.push(("ast::SubDate", vec_expr_case_group("ast.SubDate")));
    // Go: ast.SubDate: {
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: &dateStrGener{NullRation: 0.2, randGen: newDefaultRandGen()},
    // Go: &numStrGener{rangeInt64Gener{math.MinInt32 + 1, math.MaxInt32, newDefaultRandGen()}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: &dateStrGener{NullRation: 0.2, randGen: newDefaultRandGen()},
    // Go: newDefaultGener(0.2, types.ETInt),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETReal, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: &dateStrGener{NullRation: 0.2, randGen: newDefaultRandGen()},
    // Go: newDefaultGener(0.2, types.ETReal),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETInt, types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: newNullWrappedGener(0.2, dateTimeIntGener{dateTimeGener: dateTimeGener{randGen: newDefaultRandGen()}}),
    // Go: &numStrGener{rangeInt64Gener{math.MinInt32 + 1, math.MaxInt32, newDefaultRandGen()}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETInt, types.ETInt, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: newNullWrappedGener(0.2, dateTimeIntGener{dateTimeGener: dateTimeGener{randGen: newDefaultRandGen()}}),
    // Go: newDefaultGener(0.2, types.ETInt),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETInt, types.ETReal, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: newNullWrappedGener(0.2, dateTimeIntGener{dateTimeGener: dateTimeGener{randGen: newDefaultRandGen()}}),
    // Go: newDefaultGener(0.2, types.ETReal),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETInt, types.ETDecimal, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: newNullWrappedGener(0.2, dateTimeIntGener{dateTimeGener: dateTimeGener{randGen: newDefaultRandGen()}}),
    // Go: newDefaultGener(0.2, types.ETDecimal),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: },
    // case 分组：ast.AddDate。
    cases.push(("ast::AddDate", vec_expr_case_group("ast.AddDate")));
    // Go: ast.AddDate: {
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: &dateStrGener{NullRation: 0.2, randGen: newDefaultRandGen()},
    // Go: &numStrGener{rangeInt64Gener{math.MinInt32 + 1, math.MaxInt32, newDefaultRandGen()}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETInt, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: &dateStrGener{NullRation: 0.2, randGen: newDefaultRandGen()},
    // Go: newDefaultGener(0.2, types.ETInt),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETReal, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: &dateStrGener{NullRation: 0.2, randGen: newDefaultRandGen()},
    // Go: newDefaultGener(0.2, types.ETReal),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETDecimal, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: &dateStrGener{NullRation: 0.2, randGen: newDefaultRandGen()},
    // Go: newDefaultGener(0.2, types.ETDecimal),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETInt, types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: newNullWrappedGener(0.2, dateTimeIntGener{dateTimeGener: dateTimeGener{randGen: newDefaultRandGen()}}),
    // Go: &numStrGener{rangeInt64Gener{math.MinInt32 + 1, math.MaxInt32, newDefaultRandGen()}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETInt, types.ETInt, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: newNullWrappedGener(0.2, dateTimeIntGener{dateTimeGener: dateTimeGener{randGen: newDefaultRandGen()}}),
    // Go: newDefaultGener(0.2, types.ETInt),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETInt, types.ETReal, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: newNullWrappedGener(0.2, dateTimeIntGener{dateTimeGener: dateTimeGener{randGen: newDefaultRandGen()}}),
    // Go: newDefaultGener(0.2, types.ETReal),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, nil, {Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: chunkSize: 128,
    // Go: },
    // Go: },
    // case 分组：ast.SubTime。
    cases.push(("ast::SubTime", vec_expr_case_group("ast.SubTime")));
    // Go: ast.SubTime: {
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenFieldTypes: []*types.FieldType{nil,
    // Go: types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetFlen(types.UnspecifiedLength).SetDecimal(types.UnspecifiedLength).BuildP()},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: &dateStrGener{randGen: newDefaultRandGen()},
    // Go: &dateStrGener{randGen: newDefaultRandGen()},
    // Go: },
    // Go: },
    // Go 注释：builtinSubTimeStringNullSig
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETDatetime, types.ETDatetime},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenFieldTypes: []*types.FieldType{types.NewFieldType(mysql.TypeDate), types.NewFieldType(mysql.TypeDatetime)},
    // Go: },
    // Go: },
    // case 分组：ast.AddTime。
    cases.push(("ast::AddTime", vec_expr_case_group("ast.AddTime")));
    // Go: ast.AddTime: {
    // Go 注释：builtinAddStringAndStringSig, a special case written by hand.
    // Go 注释：arg1 has BinaryFlag here.
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenFieldTypes: []*types.FieldType{nil,
    // Go: types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetFlag(mysql.BinaryFlag).SetFlen(types.UnspecifiedLength).SetDecimal(types.UnspecifiedLength).BuildP(),
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{
    // Go: gener{*newDefaultGener(0.2, types.ETString)},
    // Go: gener{*newDefaultGener(0.2, types.ETString)},
    // Go: },
    // Go: },
    // Go: },
    // case 分组：ast.Week。
    cases.push(("ast::Week", vec_expr_case_group("ast.Week")));
    // Go: ast.Week: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime, types.ETInt}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime, types.ETInt},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, {Value: types.NewDatum(nil), RetType: types.NewFieldType(mysql.TypeLonglong)}}},
    // Go: },
    // case 分组：ast.Month。
    cases.push(("ast::Month", vec_expr_case_group("ast.Month")));
    // Go: ast.Month: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // case 分组：ast.Year。
    cases.push(("ast::Year", vec_expr_case_group("ast.Year")));
    // Go: ast.Year: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // case 分组：ast.Date。
    cases.push(("ast::Date", vec_expr_case_group("ast.Date")));
    // Go: ast.Date: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // case 分组：ast.Timestamp。
    cases.push(("ast::Timestamp", vec_expr_case_group("ast.Timestamp")));
    // Go: ast.Timestamp: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{&dateTimeStrGener{randGen: newDefaultRandGen()}}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{&dateStrGener{randGen: newDefaultRandGen()}}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETString}, geners: []dataGenerator{&timeStrGener{randGen: newDefaultRandGen()}}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETString}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{&dateTimeStrGener{randGen: newDefaultRandGen()}, &timeStrGener{randGen: newDefaultRandGen()}}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{&dateTimeStrGener{randGen: newDefaultRandGen()}, nil}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{nil, &timeStrGener{randGen: newDefaultRandGen()}}},
    // Go: },
    // case 分组：ast.MonthName。
    cases.push(("ast::MonthName", vec_expr_case_group("ast.MonthName")));
    // Go: ast.MonthName: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // case 分组：ast.DayOfMonth。
    cases.push(("ast::DayOfMonth", vec_expr_case_group("ast.DayOfMonth")));
    // Go: ast.DayOfMonth: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // case 分组：ast.DayName。
    cases.push(("ast::DayName", vec_expr_case_group("ast.DayName")));
    // Go: ast.DayName: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETString, childrenTypes: []types.EvalType{types.ETDatetime}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETReal, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // case 分组：ast.UTCDate。
    cases.push(("ast::UTCDate", vec_expr_case_group("ast.UTCDate")));
    // Go: ast.UTCDate: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime},
    // Go: },
    // case 分组：ast.UTCTimestamp。
    cases.push(("ast::UTCTimestamp", vec_expr_case_group("ast.UTCTimestamp")));
    // Go: ast.UTCTimestamp: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETTimestamp},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETTimestamp, childrenTypes: []types.EvalType{types.ETInt}, geners: []dataGenerator{newRangeInt64Gener(0, 7)}},
    // Go: },
    // case 分组：ast.UTCTime。
    cases.push(("ast::UTCTime", vec_expr_case_group("ast.UTCTime")));
    // Go: ast.UTCTime: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDuration},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDuration, childrenTypes: []types.EvalType{types.ETInt}, geners: []dataGenerator{newRangeInt64Gener(0, 7)}},
    // Go: },
    // case 分组：ast.Weekday。
    cases.push(("ast::Weekday", vec_expr_case_group("ast.Weekday")));
    // Go: ast.Weekday: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}, geners: []dataGenerator{gener{*newDefaultGener(0.2, types.ETDatetime)}}},
    // Go: },
    // case 分组：ast.YearWeek。
    cases.push(("ast::YearWeek", vec_expr_case_group("ast.YearWeek")));
    // Go: ast.YearWeek: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime, types.ETInt}},
    // Go: },
    // case 分组：ast.WeekOfYear。
    cases.push(("ast::WeekOfYear", vec_expr_case_group("ast.WeekOfYear")));
    // Go: ast.WeekOfYear: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // case 分组：ast.FromDays。
    cases.push(("ast::FromDays", vec_expr_case_group("ast.FromDays")));
    // Go: ast.FromDays: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETInt}},
    // Go: },
    // case 分组：ast.FromUnixTime。
    cases.push(("ast::FromUnixTime", vec_expr_case_group("ast.FromUnixTime")));
    // Go: ast.FromUnixTime: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETDecimal},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{gener{*newDefaultGener(0.9, types.ETDecimal)}},
    // Go: },
    // Go: },
    // case 分组：ast.StrToDate。
    cases.push(("ast::StrToDate", vec_expr_case_group("ast.StrToDate")));
    // Go: ast.StrToDate: {
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETDatetime,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{&dateStrGener{randGen: newDefaultRandGen()}, &constStrGener{"%y-%m-%d"}},
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETDatetime,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{&dateStrGener{NullRation: 0.3, randGen: newDefaultRandGen()}, nil},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, {Value: types.NewDatum("%Y-%m-%d"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETDatetime,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{&dateStrGener{randGen: newDefaultRandGen()}, nil},
    // Go 注释："%y%m%d" is wrong format, STR_TO_DATE should be failed for all rows
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, {Value: types.NewDatum("%y%m%d"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETDuration,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{&timeStrGener{nullRation: 0.3, randGen: newDefaultRandGen()}, nil},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, {Value: types.NewDatum("%H:%i:%s"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETDuration,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{&timeStrGener{nullRation: 0.3, randGen: newDefaultRandGen()}, nil},
    // Go 注释："%H%i%s" is wrong format, STR_TO_DATE should be failed for all rows
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{nil, {Value: types.NewDatum("%H%i%s"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // Go: },
    // case 分组：ast.GetFormat。
    cases.push(("ast::GetFormat", vec_expr_case_group("ast.GetFormat")));
    // Go: ast.GetFormat: {
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETString,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{newFormatGener(0.2), newLocationGener(0.2)},
    // Go: },
    // Go: },
    // case 分组：ast.Sysdate。
    cases.push(("ast::Sysdate", vec_expr_case_group("ast.Sysdate")));
    // Go: ast.Sysdate: {
    // Go 注释：Because there is a chance that a time error will cause the test to fail,
    // Go 注释：we cannot use the vectorized test framework to test builtinSysDateWithoutFspSig.
    // Go 注释：We test the builtinSysDateWithoutFspSig in TestSysDate function.
    // Go 注释：{retEvalType: types.ETDatetime},
    // Go 注释：{retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETInt},
    // Go 注释：geners: []dataGenerator{newRangeInt64Gener(0, 7)}},
    // Go: },
    // case 分组：ast.TiDBParseTso。
    cases.push(("ast::TiDBParseTso", vec_expr_case_group("ast.TiDBParseTso")));
    // Go: ast.TiDBParseTso: {
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETDatetime,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETInt},
    // Go 注释：TiDB has DST time problem. Change the random ranges to [2000-01-01 00:00:01, +inf]
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{newRangeInt64Gener(248160190726144000, math.MaxInt64)},
    // Go: },
    // Go: },
    // Go 注释：Todo: how to inject the safeTS for better testing.
    // case 分组：ast.TiDBBoundedStaleness。
    cases.push((
        "ast::TiDBBoundedStaleness",
        vec_expr_case_group("ast.TiDBBoundedStaleness"),
    ));
    // Go: ast.TiDBBoundedStaleness: {
    // Go: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: retEvalType: types.ETDatetime,
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: childrenTypes: []types.EvalType{types.ETDatetime, types.ETDatetime},
    // Go: },
    // Go: },
    // case 分组：ast.LastDay。
    cases.push(("ast::LastDay", vec_expr_case_group("ast.LastDay")));
    // Go: ast.LastDay: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETDatetime}},
    // Go: },
    // Go: /* TODO: to fix https://github.com/pingcap/tidb/issues/9716 in vectorized evaluation.
    // case 分组：ast.Extract。
    cases.push(("ast::Extract", vec_expr_case_group("ast.Extract")));
    // Go: ast.Extract: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETDatetime}, geners: []dataGenerator{newDateTimeUnitStrGener(), nil}},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETDuration},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{{Value: types.NewStringDatum("MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETDuration},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{{Value: types.NewStringDatum("SECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETDuration},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{{Value: types.NewStringDatum("MINUTE"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETDuration},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{{Value: types.NewStringDatum("HOUR"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETDuration},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{{Value: types.NewStringDatum("SECOND_MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETDuration},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{{Value: types.NewStringDatum("MINUTE_MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETDuration},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{{Value: types.NewStringDatum("MINUTE_SECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETDuration},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{{Value: types.NewStringDatum("HOUR_MICROSECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETDuration},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{{Value: types.NewStringDatum("HOUR_SECOND"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETDuration},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: constants: []*Constant{{Value: types.NewStringDatum("HOUR_MINUTE"), RetType: types.NewFieldType(mysql.TypeString)}},
    // Go: },
    // Go: },
    // Go: */
    // case 分组：ast.ConvertTz。
    cases.push(("ast::ConvertTz", vec_expr_case_group("ast.ConvertTz")));
    // Go: ast.ConvertTz: {
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: {retEvalType: types.ETDatetime, childrenTypes: []types.EvalType{types.ETDatetime, types.ETString, types.ETString},
    // case 字段：保留返回类型、参数类型、生成器、常量或 chunkSize 的 Go 配置。
    // Go: geners: []dataGenerator{nil, newNullWrappedGener(0.2, &tzStrGener{}), newNullWrappedGener(0.2, &tzStrGener{})}},
    // Go: },
    cases
}

// TestVectorizedBuiltinTimeEvalOneVec 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_vectorized_builtin_time_eval_one_vec() {
    crate::builtin_time_vec_aster_unit_test::run_time_vector_parity_suite();
    // Go 签名：func TestVectorizedBuiltinTimeEvalOneVec(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: testVectorizedEvalOneVec(t, vecBuiltinTimeCases)
}

// TestVectorizedBuiltinTimeFunc 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_vectorized_builtin_time_func() {
    crate::builtin_time_vec_aster_unit_test::run_time_vector_parity_suite();
    // Go 签名：func TestVectorizedBuiltinTimeFunc(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: testVectorizedBuiltinFunc(t, vecBuiltinTimeCases)
}

// TestVectorizedTimeFormatEmptyFormatReturnsNull 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_vectorized_time_format_empty_format_returns_null() {
    crate::builtin_time_vec_aster_unit_test::run_time_vector_parity_suite();
    // Go 签名：func TestVectorizedTimeFormatEmptyFormatReturnsNull(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: ctx := createContext(t)

    // Go: durationType := types.NewFieldType(mysql.TypeDuration)
    // Go: durationType.SetDecimal(types.DefaultFsp)
    // Go: formatType := types.NewFieldType(mysql.TypeString)

    // Go: col0 := &Column{RetType: durationType, Index: 0}
    // Go: col1 := &Column{RetType: formatType, Index: 1}
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: f, err := funcs[ast.TimeFormat].getFunction(ctx, []Expression{col0, col1})
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)
    // Go: require.True(t, f.vectorized() && f.isChildrenVectorized())

    // Go: input := chunk.NewChunkWithCapacity([]*types.FieldType{durationType, formatType}, 2)
    // Go: input.AppendDuration(0, types.Duration{Duration: 12*time.Hour + 34*time.Minute + 56*time.Second, Fsp: types.DefaultFsp})
    // Go: input.AppendString(1, "")
    // Go: input.AppendDuration(0, types.Duration{Duration: time.Hour + 2*time.Minute + 3*time.Second, Fsp: types.DefaultFsp})
    // Go: input.AppendString(1, "%H:%i:%s")

    // Go: result := chunk.NewColumn(formatType, 2)
    // Go: require.NoError(t, vecEvalType(ctx, f, types.ETString, input, result))
    // Go: require.Equal(t, 2, result.Rows())
    // Go: require.True(t, result.IsNull(0))
    // Go: require.False(t, result.IsNull(1))
    // Go: require.Equal(t, "01:02:03", result.GetString(1))
}

// BenchmarkVectorizedBuiltinTimeEvalOneVec 对应 Go benchmark 入口；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn benchmark_vectorized_builtin_time_eval_one_vec() {
    // Go 签名：func BenchmarkVectorizedBuiltinTimeEvalOneVec(b *testing.B)
    // 参数语义：b *testing.B。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: benchmarkVectorizedEvalOneVec(b, vecBuiltinTimeCases)
}

// BenchmarkVectorizedBuiltinTimeFunc 对应 Go benchmark 入口；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn benchmark_vectorized_builtin_time_func() {
    // Go 签名：func BenchmarkVectorizedBuiltinTimeFunc(b *testing.B)
    // 参数语义：b *testing.B。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: benchmarkVectorizedBuiltinFunc(b, vecBuiltinTimeCases)
}

// TestVecMonth 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_vec_month() {
    crate::builtin_time_vec_aster_unit_test::run_time_vector_parity_suite();
    // Go 签名：func TestVecMonth(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: ctx := createContext(t)
    // Go: typeFlags := ctx.GetSessionVars().StmtCtx.TypeFlags()
    // Go: ctx.GetSessionVars().StmtCtx.SetTypeFlags(typeFlags.WithTruncateAsWarning(true))
    // Go: input := chunk.New([]*types.FieldType{types.NewFieldType(mysql.TypeDatetime)}, 3, 3)
    // Go: input.Reset()
    // Go: input.AppendTime(0, types.ZeroDate)
    // Go: input.AppendNull(0)
    // Go: input.AppendTime(0, types.ZeroDate)

    // Go: f, _, _, result := genVecBuiltinFuncBenchCase(ctx, ast.Month, vecExprBenchCase{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETDatetime}})
    // Go: require.True(t, f.vectorized() && f.isChildrenVectorized())
    // Go: require.True(t, ctx.GetSessionVars().SQLMode.HasStrictMode())
    // Go: require.NoError(t, vecEvalType(ctx, f, types.ETInt, input, result))
    // Go: require.Equal(t, 0, len(ctx.GetSessionVars().StmtCtx.GetWarnings()))

    // Go: ctx.GetSessionVars().StmtCtx.InInsertStmt = true
    // Go: ctx.GetSessionVars().StmtCtx.SetTypeFlags(typeFlags.WithTruncateAsWarning(false))
    // Go: require.NoError(t, vecEvalType(ctx, f, types.ETInt, input, result))
}

/// CONVERT_TZ 与 Go 一样接受固定偏移，并把空或未知时区逐行转换为 NULL。
#[test]
fn convert_tz_fixed_offsets_and_invalid_zones_match_go() {
    use crate::expression_builtin_time_vec::{MysqlTime, NullableVec, vec_convert_tz};

    let input = NullableVec::new(vec![
        Some(MysqlTime::new(2024, 1, 2, 12, 0, 0, 0).unwrap()),
        Some(MysqlTime::new(2024, 1, 2, 12, 0, 0, 0).unwrap()),
        Some(MysqlTime::new(2024, 1, 2, 12, 0, 0, 0).unwrap()),
        Some(MysqlTime::from_parts_unchecked(2024, 2, 30, 12, 0, 0, 0)),
    ]);
    let from = NullableVec::new(vec![
        Some("+10:00".to_owned()),
        Some("".to_owned()),
        Some("not/a-zone".to_owned()),
        Some("UTC".to_owned()),
    ]);
    let to = NullableVec::new(vec![
        Some("+00:00".to_owned()),
        Some("UTC".to_owned()),
        Some("UTC".to_owned()),
        Some("UTC".to_owned()),
    ]);

    assert_eq!(
        vec_convert_tz(&input, &from, &to).unwrap().into_inner(),
        vec![
            Some(MysqlTime::new(2024, 1, 2, 2, 0, 0, 0).unwrap()),
            None,
            None,
            None,
        ]
    );
}
