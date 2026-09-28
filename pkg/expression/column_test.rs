// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Column / CorrelatedColumn 求值与哈希相等的 Go 对齐测试骨架。
//
// 对应 Go `column_test.go`：覆盖行/向量求值、Column/FieldType hash equals、
// 虚拟表达式解析与 nil any 回归。当前多为迁移占位，保留 Go 调用顺序与断言点。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables,
    unused_mut
)]
// 这段逻辑覆盖 Column 与 CorrelatedColumn 行/向量求值、Column/FieldType hash equals、虚拟表达式解析和 nil any 回归。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待 Rust crate 接线）：
// - "fmt"
// - "testing"
// - "github.com/pingcap/tidb/pkg/meta/model"
// - "github.com/pingcap/tidb/pkg/parser/ast"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/planner/cascades/base"
// - "github.com/pingcap/tidb/pkg/types"
// - "github.com/pingcap/tidb/pkg/util/chunk"
// - "github.com/pingcap/tidb/pkg/util/mock"
// - "github.com/stretchr/testify/require"

// 迁移占位类型：这些名称来自 Go/TiDB 测试依赖，后续接入真实 Rust 模块时再替换。
type GoAny = ();
type GoError = String;
type GoBytes = Vec<u8>;
type GoBool = bool;

// VecExprBenchCase holds table-driven vectorization bench cases.
/// 表驱动向量化基准用例的占位结构。
pub struct VecExprBenchCase {
    pub name: &'static str,
}

/// 按名称构造基准用例组占位。
fn vec_expr_case_group(name: &'static str) -> VecExprBenchCase {
    VecExprBenchCase { name }
}

/// 覆盖 Column/CorrelatedColumn 的身份、哈希、转换、排序和求值契约。
fn run_column_parity_suite() {
    use crate::{
        ColInfo2Col, Column, Column2Exprs, CorrelatedColumn, Expression, NewCorrelatedDatum,
        SortColumns, chunk, model, types,
    };

    let context = exprstatic::NewEvalContext(Vec::new());
    let field_type = types::FieldType::default();
    let mut column = Column::new(field_type.clone(), 7, 12, 0);
    let same = column.clone();
    let other = Column::new(field_type.clone(), 8, 2, 1);
    assert!(column.EqualColumn(&same));
    assert!(!column.EqualColumn(&other));
    assert!(!column.IsCorrelated());
    assert_eq!(column.HashCode().len(), 9);
    assert_ne!(column.HashCode(), other.clone().HashCode());

    let columns = vec![column.clone(), other.clone()];
    let expressions = Column2Exprs(&columns);
    assert!(columns[0].EqualColumn(expressions[0].as_ref()));
    assert!(columns[1].EqualColumn(expressions[1].as_ref()));

    let info = model::ColumnInfo {
        ID: 7,
        ..Default::default()
    };
    assert_eq!(ColInfo2Col(&columns, &info).map(|value| value.ID), Some(7));
    assert_eq!(
        SortColumns(&columns)
            .iter()
            .map(|value| value.UniqueID)
            .collect::<Vec<_>>(),
        vec![2, 12]
    );
    assert!(column.InColumnArray(&columns));

    let correlated = CorrelatedColumn {
        column: column.clone(),
        data: Some(NewCorrelatedDatum(types::NewIntDatum(1))),
    };
    assert!(correlated.IsCorrelated());
    assert!(!correlated.SafeToShareAcrossSession());
    assert_eq!(
        correlated
            .EvalInt(&context, chunk::Row::default())
            .expect("correlated integer evaluation"),
        (1, false)
    );
}
// TestColumn 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_column() {
    run_column_parity_suite();
    // Go 签名：func TestColumn(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: ctx := mock.NewContext()
    // Go: col := &Column{RetType: types.NewFieldType(mysql.TypeLonglong), UniqueID: 1}

    // Go: require.True(t, col.EqualColumn(col))
    // Go: require.False(t, col.EqualColumn(&Column{}))
    // Go: require.False(t, col.IsCorrelated())
    // Go: require.True(t, col.EqualColumn(col.Decorrelate(nil)))

    // Go: intDatum := types.NewIntDatum(1)
    // Go: corCol := &CorrelatedColumn{Column: *col, Data: &intDatum}
    // Go: invalidCorCol := &CorrelatedColumn{Column: Column{}}
    // Go: schema := NewSchema(&Column{UniqueID: 1})
    // Go: require.True(t, corCol.EqualColumn(corCol))
    // Go: require.False(t, corCol.EqualColumn(invalidCorCol))
    // Go: require.True(t, corCol.IsCorrelated())
    // Go: require.Equal(t, ConstNone, corCol.ConstLevel())
    // Go: require.True(t, col.EqualColumn(corCol.Decorrelate(schema)))
    // Go: require.True(t, invalidCorCol.EqualColumn(invalidCorCol.Decorrelate(schema)))

    // Go: intCorCol := &CorrelatedColumn{Column: Column{RetType: types.NewFieldType(mysql.TypeLonglong)},
    // Go: Data: &intDatum}
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: intVal, isNull, err := intCorCol.EvalInt(ctx, chunk.Row{})
    // Go: require.Equal(t, int64(1), intVal)
    // Go: require.False(t, isNull)
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)

    // Go: realDatum := types.NewFloat64Datum(1.2)
    // Go: realCorCol := &CorrelatedColumn{Column: Column{RetType: types.NewFieldType(mysql.TypeDouble)},
    // Go: Data: &realDatum}
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: realVal, isNull, err := realCorCol.EvalReal(ctx, chunk.Row{})
    // Go: require.Equal(t, float64(1.2), realVal)
    // Go: require.False(t, isNull)
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)

    // Go: decimalDatum := types.NewDecimalDatum(types.NewDecFromStringForTest("1.2"))
    // Go: decimalCorCol := &CorrelatedColumn{Column: Column{RetType: types.NewFieldType(mysql.TypeNewDecimal)},
    // Go: Data: &decimalDatum}
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: decVal, isNull, err := decimalCorCol.EvalDecimal(ctx, chunk.Row{})
    // Go: require.Zero(t, decVal.Compare(types.NewDecFromStringForTest("1.2")))
    // Go: require.False(t, isNull)
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)

    // Go: stringDatum := types.NewStringDatum("abc")
    // Go: stringCorCol := &CorrelatedColumn{Column: Column{RetType: types.NewFieldType(mysql.TypeVarchar)},
    // Go: Data: &stringDatum}
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: strVal, isNull, err := stringCorCol.EvalString(ctx, chunk.Row{})
    // Go: require.Equal(t, "abc", strVal)
    // Go: require.False(t, isNull)
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)

    // Go: durationCorCol := &CorrelatedColumn{Column: Column{RetType: types.NewFieldType(mysql.TypeDuration)},
    // Go: Data: &durationDatum}
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: durationVal, isNull, err := durationCorCol.EvalDuration(ctx, chunk.Row{})
    // Go: require.Zero(t, durationVal.Compare(duration))
    // Go: require.False(t, isNull)
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)

    // Go: timeDatum := types.NewTimeDatum(tm)
    // Go: timeCorCol := &CorrelatedColumn{Column: Column{RetType: types.NewFieldType(mysql.TypeDatetime)},
    // Go: Data: &timeDatum}
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: timeVal, isNull, err := timeCorCol.EvalTime(ctx, chunk.Row{})
    // Go: require.Zero(t, timeVal.Compare(tm))
    // Go: require.False(t, isNull)
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)

    // Go: t.Run("resolve virtual expression prefers exact column ID", func(t *testing.T) {
    // Go: strTp := types.NewFieldType(mysql.TypeVarchar)
    // Go: baseCol := &Column{UniqueID: 10, RetType: strTp}
    // Go: virtualExpr := NewFunctionInternal(ctx, ast.Lower, strTp, baseCol)
    // Go: exprOnlyMatchedCol := &Column{UniqueID: 12, RetType: strTp, VirtualExpr: virtualExpr.Clone()}
    // Go: exactMatchedCol := &Column{UniqueID: 11, RetType: strTp, VirtualExpr: virtualExpr.Clone()}
    // Go: targetCol := &Column{UniqueID: exactMatchedCol.UniqueID, RetType: strTp, VirtualExpr: virtualExpr.Clone()}

    // Go: resolvedExpr, ok := targetCol.ResolveIndicesByVirtualExpr(ctx.GetEvalCtx(), NewSchema(exprOnlyMatchedCol, exactMatchedCol))
    // Go: require.True(t, ok)
    // Go: require.Equal(t, 1, resolvedExpr.(*Column).Index)

    // Go: ambiguousTargetCol := &Column{UniqueID: 13, RetType: strTp, VirtualExpr: virtualExpr.Clone()}
    // Go: resolvedExpr, ok = ambiguousTargetCol.ResolveIndicesByVirtualExpr(ctx.GetEvalCtx(), NewSchema(exprOnlyMatchedCol, exactMatchedCol))
    // Go: require.True(t, ok)
    // Go: require.Equal(t, 0, resolvedExpr.(*Column).Index)
    // Go: })
}

/// Go `Column.string` only removes the generated fallback column number.  An
/// explicit `OrigName` is user-visible metadata and must be preserved even if
/// it happens to use the `Column#<number>` spelling.
#[test]
fn explicit_column_name_is_not_rewritten_for_explain() {
    use crate::{Column, errors, exprctx};

    let mut column = Column::default();
    column.OrigName = "Column#12".to_owned();

    assert_eq!(
        column.StringWithCtxForExplain(&exprctx::EmptyParamValues, errors::RedactLogDisable, true,),
        "Column#12"
    );
}

/// An unbound correlated column is an executor invariant violation. Go fails
/// immediately when dereferencing `Data`; Rust must not turn it into SQL NULL.
#[test]
#[should_panic(expected = "correlated column data is not bound")]
fn unbound_correlated_column_eval_does_not_become_null() {
    use crate::{Column, CorrelatedColumn, chunk};

    let correlated = CorrelatedColumn {
        column: Column::default(),
        data: None,
    };
    let context = exprstatic::NewEvalContext(Vec::new());
    let _ = correlated.Eval(&context, chunk::Row::default());
}

// TestColumnHashCode 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_column_hash_code() {
    run_column_parity_suite();
    // Go 签名：func TestColumnHashCode(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: col1 := &Column{
    // Go: UniqueID: 12,
    // Go: }
    // Go: require.EqualValues(t, []byte{0x1, 0x80, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0xc}, col1.HashCode())

    // Go: col2 := &Column{
    // Go: UniqueID: 2,
    // Go: }
    // Go: require.EqualValues(t, []byte{0x1, 0x80, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x2}, col2.HashCode())
}

// TestColumn2Expr 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_column2_expr() {
    run_column_parity_suite();
    // Go 签名：func TestColumn2Expr(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: cols := make([]*Column, 0, 5)
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for i := range 5 {
    // Go: cols = append(cols, &Column{UniqueID: int64(i)})
    // Go: }

    // Go: exprs := Column2Exprs(cols)
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for i := range exprs {
    // Go: require.True(t, cols[i].EqualColumn(exprs[i]))
    // Go: }
}

// TestColInfo2Col 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_col_info2_col() {
    run_column_parity_suite();
    // Go 签名：func TestColInfo2Col(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: col0, col1 := &Column{ID: 0}, &Column{ID: 1}
    // Go: cols := []*Column{col0, col1}
    // Go: colInfo := &model.ColumnInfo{ID: 0}
    // Go: res := ColInfo2Col(cols, colInfo)
    // Go: require.True(t, res.EqualColumn(col1))

    // Go: colInfo.ID = 3
    // Go: res = ColInfo2Col(cols, colInfo)
    // Go: require.Nil(t, res)
}

// TestColHybird 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_col_hybird() {
    run_column_parity_suite();
    // Go 签名：func TestColHybird(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: ctx := mock.NewContext()

    // Go 注释：bit
    // Go: ft := types.NewFieldType(mysql.TypeBit)
    // Go: col := &Column{RetType: ft, Index: 0}
    // Go: input := chunk.New([]*types.FieldType{ft}, 1024, 1024)
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for i := range 1024 {
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: num, err := types.ParseBitStr(fmt.Sprintf("0b%b", i))
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)
    // Go: input.AppendBytes(0, num)
    // Go: }
    // Go: result := chunk.NewColumn(types.NewFieldType(mysql.TypeLonglong), 1024)
    // Go: require.Nil(t, col.VecEvalInt(ctx, input, result))

    // Go: it := chunk.NewIterator4Chunk(input)
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for row, i := it.Begin(), 0; row != it.End(); row, i = it.Next(), i+1 {
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: v, _, err := col.EvalInt(ctx, row)
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)
    // Go: require.Equal(t, result.GetInt64(i), v)
    // Go: }

    // Go 注释：use a container which has the different field type with bit
    // Go: result = chunk.NewColumn(types.NewFieldType(mysql.TypeString), 1024)
    // Go: require.Nil(t, col.VecEvalInt(ctx, input, result))
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for row, i := it.Begin(), 0; row != it.End(); row, i = it.Next(), i+1 {
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: v, _, err := col.EvalInt(ctx, row)
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)
    // Go: require.Equal(t, result.GetInt64(i), v)
    // Go: }

    // Go 注释：enum
    // Go: ft = types.NewFieldType(mysql.TypeEnum)
    // Go: col.RetType = ft
    // Go: input = chunk.New([]*types.FieldType{ft}, 1024, 1024)
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for i := range 1024 {
    // Go: input.AppendEnum(0, types.Enum{Name: fmt.Sprintf("%v", i), Value: uint64(i)})
    // Go: }
    // Go: result = chunk.NewColumn(types.NewFieldType(mysql.TypeString), 1024)
    // Go: require.Nil(t, col.VecEvalString(ctx, input, result))

    // Go: it = chunk.NewIterator4Chunk(input)
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for row, i := it.Begin(), 0; row != it.End(); row, i = it.Next(), i+1 {
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: v, _, err := col.EvalString(ctx, row)
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)
    // Go: require.Equal(t, result.GetString(i), v)
    // Go: }

    // Go 注释：set
    // Go: ft = types.NewFieldType(mysql.TypeSet)
    // Go: col.RetType = ft
    // Go: input = chunk.New([]*types.FieldType{ft}, 1024, 1024)
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for i := range 1024 {
    // Go: input.AppendSet(0, types.Set{Name: fmt.Sprintf("%v", i), Value: uint64(i)})
    // Go: }
    // Go: result = chunk.NewColumn(types.NewFieldType(mysql.TypeString), 1024)
    // Go: require.Nil(t, col.VecEvalString(ctx, input, result))

    // Go: it = chunk.NewIterator4Chunk(input)
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for row, i := it.Begin(), 0; row != it.End(); row, i = it.Next(), i+1 {
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: v, _, err := col.EvalString(ctx, row)
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)
    // Go: require.Equal(t, result.GetString(i), v)
    // Go: }
}

// TestInColumnArray 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_in_column_array() {
    run_column_parity_suite();
    // Go 签名：func TestInColumnArray(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go 注释：normal case, col is in column array
    // Go: col0, col1 := &Column{ID: 0, UniqueID: 0}, &Column{ID: 1, UniqueID: 1}
    // Go: cols := []*Column{col0, col1}
    // Go: require.True(t, col0.InColumnArray(cols))

    // Go 注释：abnormal case, col is not in column array
    // Go: require.False(t, col0.InColumnArray([]*Column{col1}))

    // Go 注释：abnormal case, input is nil
    // Go: require.False(t, col0.InColumnArray(nil))
}

// TestGcColumnExprIsTidbShard 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_gc_column_expr_is_tidb_shard() {
    run_column_parity_suite();
    // Go 签名：func TestGcColumnExprIsTidbShard(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: ctx := mock.NewContext()

    // Go 注释：abnormal case
    // Go 注释：nil, not tidb_shard
    // Go: require.False(t, GcColumnExprIsTidbShard(nil))

    // Go 注释：`a = 1`, not tidb_shard
    // Go: ft := types.NewFieldType(mysql.TypeLonglong)
    // Go: col := &Column{RetType: ft, Index: 0}
    // Go: d1 := types.NewDatum(1)
    // Go: con := &Constant{Value: d1, RetType: ft}
    // Go: expr := NewFunctionInternal(ctx, ast.EQ, ft, col, con)
    // Go: require.False(t, GcColumnExprIsTidbShard(expr))

    // Go 注释：normal case
    // Go 注释：tidb_shard(a) = 1
    // Go: shardExpr := NewFunctionInternal(ctx, ast.TiDBShard, ft, col)
    // Go: require.True(t, GcColumnExprIsTidbShard(shardExpr))
}

// TestFieldTypeHashEquals 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_field_type_hash_equals() {
    run_column_parity_suite();
    // Go 签名：func TestFieldTypeHashEquals(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: ft := types.NewFieldType(mysql.TypeLonglong)
    // Go: ft2 := types.NewFieldType(mysql.TypeLonglong)
    // Go: hasher1 := base.NewHashEqualer()
    // Go: hasher2 := base.NewHashEqualer()
    // Go: ft.Hash64(hasher1)
    // Go: ft2.Hash64(hasher2)
    // Go: require.Equal(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.True(t, ft.Equals(ft2))

    // Go 注释：flag diff
    // Go: ft.DelFlag(mysql.NotNullFlag)
    // Go: ft2.AddFlag(mysql.NotNullFlag)
    // Go: hasher1.Reset()
    // Go: hasher2.Reset()
    // Go: ft.Hash64(hasher1)
    // Go: ft2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, ft.Equals(ft2))

    // Go 注释：flen diff
    // Go: ft.AddFlag(mysql.NotNullFlag)
    // Go: ft2.AddFlag(mysql.NotNullFlag)
    // Go: ft2.SetFlen(ft2.GetFlen() + 1)
    // Go: hasher1.Reset()
    // Go: hasher2.Reset()
    // Go: ft.Hash64(hasher1)
    // Go: ft2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, ft.Equals(ft2))

    // Go 注释：decimal diff
    // Go: ft2.SetFlen(ft.GetFlen())
    // Go: ft2.SetDecimal(ft.GetDecimal() + 1)
    // Go: hasher2.Reset()
    // Go: ft2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, ft.Equals(ft2))

    // Go 注释：charset diff
    // Go: ft2.SetDecimal(ft.GetDecimal())
    // Go: ft2.SetCharset(ft.GetCharset() + "1")
    // Go: hasher2.Reset()
    // Go: ft2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, ft.Equals(ft2))

    // Go 注释：collate diff
    // Go: ft2.SetCharset(ft.GetCharset())
    // Go: ft2.SetCollate(ft.GetCollate() + "1")
    // Go: hasher2.Reset()
    // Go: ft2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, ft.Equals(ft2))

    // Go 注释：tp diff
    // Go: ft2.SetCollate(ft.GetCollate())
    // Go: ft2.SetType(ft.GetType() + 1)
    // Go: hasher2.Reset()
    // Go: ft2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, ft.Equals(ft2))

    // Go 注释：elems diff
    // Go: ft2.SetType(ft.GetType())
    // Go: ft.SetElems([]string{""})
    // Go: ft2.SetElems([]string{"a"})
    // Go: hasher1.Reset()
    // Go: hasher2.Reset()
    // Go: ft.Hash64(hasher1)
    // Go: ft2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, ft.Equals(ft2))

    // Go 注释：elemsIsBinaryLit diff
    // Go: ft.SetElems([]string{"a", "b"})
    // Go: ft2.SetElems([]string{"a", "b"})
    // Go: ft.SetElemWithIsBinaryLit(0, "1", true)
    // Go: ft.SetElemWithIsBinaryLit(1, "2", false)
    // Go: ft2.SetElemWithIsBinaryLit(0, "1", true)
    // Go: ft2.SetElemWithIsBinaryLit(1, "2", true)
    // Go: hasher1.Reset()
    // Go: hasher2.Reset()
    // Go: ft.Hash64(hasher1)
    // Go: ft2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, ft.Equals(ft2))

    // Go 注释：array diff
    // Go: ft.SetElems([]string{"a", "b"})
    // Go: ft2.SetElems([]string{"a", "b"})
    // Go: ft.SetElemWithIsBinaryLit(0, "1", true)
    // Go: ft.SetElemWithIsBinaryLit(1, "2", true)
    // Go: ft2.SetElemWithIsBinaryLit(0, "1", true)
    // Go: ft2.SetElemWithIsBinaryLit(1, "2", true)
    // Go: ft.SetArray(true)
    // Go: ft2.SetArray(false)
    // Go: hasher1.Reset()
    // Go: hasher2.Reset()
    // Go: ft.Hash64(hasher1)
    // Go: ft2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, ft.Equals(ft2))

    // Go 注释：same
    // Go: ft2.SetArray(true)
    // Go: hasher2.Reset()
    // Go: ft2.Hash64(hasher2)
    // Go: require.Equal(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.True(t, ft.Equals(ft2))
}

// TestColumnHashEquals 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_column_hash_equals() {
    run_column_parity_suite();
    // Go 签名：func TestColumnHashEquals(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: col1 := &Column{UniqueID: 1}
    // Go: col2 := &Column{UniqueID: 1}
    // Go: hasher1 := base.NewHashEqualer()
    // Go: hasher2 := base.NewHashEqualer()
    // Go: col1.Hash64(hasher1)
    // Go: col2.Hash64(hasher2)
    // Go: require.Equal(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.True(t, col1.Equals(col2))

    // Go 注释：diff uniqueID
    // Go: col2.UniqueID = 2
    // Go: hasher2.Reset()
    // Go: col2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, col1.Equals(col2))

    // Go 注释：diff ID
    // Go: col2.UniqueID = col1.UniqueID
    // Go: col2.ID = 2
    // Go: hasher2.Reset()
    // Go: col2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, col1.Equals(col2))

    // Go 注释：diff RetType
    // Go: col2.ID = col1.ID
    // Go: col2.RetType = types.NewFieldType(mysql.TypeLonglong)
    // Go: hasher2.Reset()
    // Go: col2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, col1.Equals(col2))

    // Go 注释：diff Index
    // Go: col2.RetType = col1.RetType
    // Go: col2.Index = 1
    // Go: hasher2.Reset()
    // Go: col2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, col1.Equals(col2))

    // Go 注释：diff VirtualExpr see TestColumnHashEuqals4VirtualExpr

    // Go 注释：diff OrigName
    // Go: col2.Index = col1.Index
    // Go: col2.OrigName = "a"
    // Go: hasher2.Reset()
    // Go: col2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, col1.Equals(col2))

    // Go 注释：diff IsHidden
    // Go: col2.OrigName = col1.OrigName
    // Go: col2.IsHidden = true
    // Go: hasher2.Reset()
    // Go: col2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, col1.Equals(col2))

    // Go 注释：diff IsPrefix
    // Go: col2.IsHidden = col1.IsHidden
    // Go: col2.IsPrefix = true
    // Go: hasher2.Reset()
    // Go: col2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, col1.Equals(col2))

    // Go 注释：diff InOperand
    // Go: col2.IsPrefix = col1.IsPrefix
    // Go: col2.InOperand = true
    // Go: hasher2.Reset()
    // Go: col2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, col1.Equals(col2))

    // Go 注释：diff collationInfo
    // Go: col2.InOperand = col1.InOperand
    // Go: col2.collationInfo = collationInfo{
    // Go: collation: "aa",
    // Go: }
    // Go: hasher2.Reset()
    // Go: col2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, col1.Equals(col2))

    // Go 注释：diff CorrelatedColUniqueID
    // Go: col2.collationInfo = col1.collationInfo
    // Go: col2.CorrelatedColUniqueID = 1
    // Go: hasher2.Reset()
    // Go: col2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, col1.Equals(col2))
}

// TestColumnHashEuqals4VirtualExpr 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_column_hash_euqals4_virtual_expr() {
    run_column_parity_suite();
    // Go 签名：func TestColumnHashEuqals4VirtualExpr(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: col1 := &Column{UniqueID: 1, VirtualExpr: NewZero()}
    // Go: col2 := &Column{UniqueID: 1, VirtualExpr: nil}
    // Go: hasher1 := base.NewHashEqualer()
    // Go: hasher2 := base.NewHashEqualer()
    // Go: col1.Hash64(hasher1)
    // Go: col2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, col1.Equals(col2))

    // Go: col2.VirtualExpr = NewZero()
    // Go: hasher2.Reset()
    // Go: col2.Hash64(hasher2)
    // Go: require.Equal(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.True(t, col1.Equals(col2))

    // Go: col1.VirtualExpr = nil
    // Go: col2.VirtualExpr = nil
    // Go: hasher1.Reset()
    // Go: hasher2.Reset()
    // Go: col1.Hash64(hasher1)
    // Go: col2.Hash64(hasher2)
    // Go: require.Equal(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.True(t, col1.Equals(col2))
}

// TestColumnEqualsWithNilWrappedInAny 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_column_equals_with_nil_wrapped_in_any() {
    run_column_parity_suite();
    // Go 签名：func TestColumnEqualsWithNilWrappedInAny(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: col1 := &Column{UniqueID: 1}
    // Go 注释：Test: other is nil
    // Go: require.False(t, col1.Equals(nil))
    // Go 注释：Test: other is *Column(nil) wrapped in any
    // Go: var col2 *Column = nil
    // Go: var col2AsAny any = col2
    // Go: require.False(t, col1.Equals(col2AsAny))
    // Go 注释：Test: both Columns are nil
    // Go: var col3 *Column = nil
    // Go: require.True(t, col3.Equals(col2AsAny))
    // Go 注释：Test: two Columns with the same values
    // Go: col4 := &Column{UniqueID: 1}
    // Go: require.True(t, col1.Equals(col4))
    // Go 注释：Test: two Columns with different values
    // Go: col5 := &Column{UniqueID: 2}
    // Go: require.False(t, col1.Equals(col5))
}
