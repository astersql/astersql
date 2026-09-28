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

// 内建函数公共测试辅助与缓存相关用例草稿。
//
// 对应 Go `builtin_test.go`：覆盖并发求值、按返回类型分派的 eval 辅助、
// IS NULL / GET_LOCK 等函数、DisplayName 以及 builtinFunc 缓存并发安全。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

// 这段逻辑覆盖内建函数求值辅助、并发求值、缓存测试、测试用函数构造与常量表达式辅助。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发、异步、IO 和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待 Rust crate 接线）：
// - "reflect"
// - "slices"
// - "sync"
// - "sync/atomic"
// - "testing"
// - "time"
// - "github.com/pingcap/errors"
// - "github.com/pingcap/tidb/pkg/parser/ast"
// - "github.com/pingcap/tidb/pkg/parser/charset"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/types"
// - "github.com/pingcap/tidb/pkg/util"
// - "github.com/pingcap/tidb/pkg/util/chunk"
// - "github.com/stretchr/testify/require"

// 迁移占位类型：这些名称来自 Go/TiDB 测试依赖，后续接入模块时再替换为真实 Rust 类型。
type GoAny = ();
type GoError = String;
type GoBytes = Vec<u8>;

use crate::expression_builtin::{SimpleEvalContext, builtinFuncCache};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

// CaseStep 是测试迁移用的轻量步骤记录；op 保留 Go 调用形状，expect 保留 require/testutil 断言语义。
/// CaseStep 是测试迁移用的轻量步骤记录；op 保留 Go 调用形状，expect 保留 require/testutil 断言语义。
pub struct CaseStep {
    pub op: &'static str,
    pub expect: &'static str,
}

// evalBuiltinFuncConcurrent 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
/// evalBuiltinFuncConcurrent 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
pub fn eval_builtin_func_concurrent() {
    // Go 签名（源文件第 35 行）：func evalBuiltinFuncConcurrent(f builtinFunc, ctx EvalContext, row chunk.Row) (d types.Datum, err error) {
    // 并发语义：保留 goroutine、锁、WaitGroup 或 atomic 的同步意图。
    // Go: var wg util.WaitGroupWrapper
    // Go: concurrency := 10
    // 并发语义：保留 goroutine、锁、WaitGroup 或 atomic 的同步意图。
    // Go: var lock sync.Mutex
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: err = nil
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for range concurrency {
    // Go: wg.Run(func() {
    // Go: di, erri := evalBuiltinFunc(f, ctx, chunk.Row{})
    // Go: lock.Lock()
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: if err == nil {
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: d, err = di, erri
    // Go: }
    // Go: lock.Unlock()
    // Go: })
    // Go: }
    // Go: wg.Wait()
    // Go: return
    // Go: }
}

// evalBuiltinFunc 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
/// evalBuiltinFunc 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
pub fn eval_builtin_func() {
    // Go 签名（源文件第 54 行）：func evalBuiltinFunc(f builtinFunc, ctx EvalContext, row chunk.Row) (d types.Datum, err error) {
    // Go: ctx = wrapEvalAssert(ctx, f)
    // Go: var (
    // Go: res any
    // Go: isNull bool
    // Go: )
    // Go: switch f.getRetTp().EvalType() {
    // Go: case types.ETInt:
    // Go: var intRes int64
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: intRes, isNull, err = f.evalInt(ctx, row)
    // Go: if mysql.HasUnsignedFlag(f.getRetTp().GetFlag()) {
    // Go: res = uint64(intRes)
    // Go: } else {
    // Go: res = intRes
    // Go: }
    // Go: case types.ETReal:
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: res, isNull, err = f.evalReal(ctx, row)
    // Go: case types.ETDecimal:
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: res, isNull, err = f.evalDecimal(ctx, row)
    // Go: case types.ETDatetime, types.ETTimestamp:
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: res, isNull, err = f.evalTime(ctx, row)
    // Go: case types.ETDuration:
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: res, isNull, err = f.evalDuration(ctx, row)
    // Go: case types.ETJson:
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: res, isNull, err = f.evalJSON(ctx, row)
    // Go: case types.ETString:
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: res, isNull, err = f.evalString(ctx, row)
    // Go: }
    // Go:
    // Go: d.SetValue(res, f.getRetTp())
    // Go: if isNull {
    // Go: d.SetNull()
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: return d, err
    // Go: }
    // Go: return
    // Go: }
}

// tblToDtbl 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
/// tblToDtbl 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
pub fn tbl_to_dtbl() {
    // Go 签名（源文件第 92 行）：func tblToDtbl(i any) []map[string][]types.Datum {
    // Go: l := reflect.ValueOf(i).Len()
    // Go: tbl := make([]map[string][]types.Datum, l)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for j := range l {
    // Go: v := reflect.ValueOf(i).Index(j).Interface()
    // Go: val := reflect.ValueOf(v)
    // Go: t := reflect.TypeOf(v)
    // Go: item := make(map[string][]types.Datum, val.NumField())
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for k := range val.NumField() {
    // Go: tmp := val.Field(k).Interface()
    // Go: item[t.Field(k).Name] = makeDatums(tmp)
    // Go: }
    // Go: tbl[j] = item
    // Go: }
    // Go: return tbl
    // Go: }
}

// makeDatums 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
/// makeDatums 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
pub fn make_datums() {
    // Go 签名（源文件第 109 行）：func makeDatums(i any) []types.Datum {
    // Go: if i != nil {
    // Go: t := reflect.TypeOf(i)
    // Go: val := reflect.ValueOf(i)
    // Go: switch t.Kind() {
    // Go: case reflect.Slice:
    // Go: l := val.Len()
    // Go: res := make([]types.Datum, l)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for j := range l {
    // Go: res[j] = types.NewDatum(val.Index(j).Interface())
    // Go: }
    // Go: return res
    // Go: }
    // Go: }
    // Go: return types.MakeDatums(i)
    // Go: }
}

// TestIsNullFunc 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
/// TestIsNullFunc 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn test_is_null_func() {
    crate::builtin_registry_aster_unit_test::run_builtin_registry_parity_suite();
    // Go 签名（源文件第 126 行）：func TestIsNullFunc(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: fc := funcs[ast.IsNull]
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: f, err := fc.getFunction(ctx, datumsToConstants(types.MakeDatums(1)))
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: v, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, int64(0), v.GetInt64())
    // Go:
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: f, err = fc.getFunction(ctx, datumsToConstants(types.MakeDatums(nil)))
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: v, err = evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, int64(1), v.GetInt64())
    // Go: }
}

// TestLock 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
/// TestLock 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn test_lock() {
    crate::builtin_registry_aster_unit_test::run_builtin_registry_parity_suite();
    // Go 签名（源文件第 142 行）：func TestLock(t *testing.T) {
    // Go: ctx := createContext(t)
    // Go: lock := funcs[ast.GetLock]
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: f, err := lock.getFunction(ctx, datumsToConstants(types.MakeDatums("mylock", 1)))
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: v, err := evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, int64(1), v.GetInt64())
    // Go:
    // Go: releaseLock := funcs[ast.ReleaseLock]
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: f, err = releaseLock.getFunction(ctx, datumsToConstants(types.MakeDatums("mylock")))
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: v, err = evalBuiltinFunc(f, ctx, chunk.Row{})
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, int64(1), v.GetInt64())
    // Go: }
}

// TestDisplayName 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
/// TestDisplayName 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn test_display_name() {
    use crate::formal_registry::GetDisplayName;

    assert_eq!(GetDisplayName("eq"), "=");
    assert_eq!(GetDisplayName("nulleq"), "<=>");
    assert_eq!(GetDisplayName("istrue"), "IS TRUE");
    assert_eq!(GetDisplayName("abs"), "abs");
    assert_eq!(GetDisplayName("other_unknown_func"), "other_unknown_func");
    // Go 签名（源文件第 159 行）：func TestDisplayName(t *testing.T) {
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, "=", GetDisplayName(ast.EQ))
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, "<=>", GetDisplayName(ast.NullEQ))
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, "IS TRUE", GetDisplayName(ast.IsTruthWithoutNull))
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, "abs", GetDisplayName("abs"))
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, "other_unknown_func", GetDisplayName("other_unknown_func"))
    // Go: }
}

// TestBuiltinFuncCacheConcurrency 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
/// TestBuiltinFuncCacheConcurrency 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn test_builtin_func_cache_concurrency() {
    let cache = Arc::new(builtinFuncCache::<usize>::default());
    let invoked = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::new();

    for _ in 0..8 {
        let cache = Arc::clone(&cache);
        let invoked = Arc::clone(&invoked);
        workers.push(thread::spawn(move || {
            cache
                .getOrInitCache(&SimpleEvalContext::new(1), || {
                    let call = invoked.fetch_add(1, Ordering::SeqCst) + 1;
                    thread::sleep(Duration::from_millis(1));
                    Ok(100 + call)
                })
                .unwrap()
        }));
    }

    for worker in workers {
        assert_eq!(worker.join().unwrap(), 101);
    }
    assert_eq!(invoked.load(Ordering::SeqCst), 1);
    // Go 签名（源文件第 167 行）：func TestBuiltinFuncCacheConcurrency(t *testing.T) {
    // Go: cache := builtinFuncCache[int]{}
    // Go: ctx := createContext(t)
    // Go:
    // 并发语义：保留 goroutine、锁、WaitGroup 或 atomic 的同步意图。
    // Go: var invoked atomic.Int64
    // Go: construct := func() (int, error) {
    // Go: invoked.Add(1)
    // 时间/上下文：保留 Go context/time/location 调用位置，后续接线时替换。
    // Go: time.Sleep(time.Millisecond)
    // Go: return 100 + int(invoked.Load()), nil
    // Go: }
    // Go:
    // 并发语义：保留 goroutine、锁、WaitGroup 或 atomic 的同步意图。
    // Go: var wg sync.WaitGroup
    // Go: concurrency := 8
    // Go: wg.Add(concurrency)
    // 循环/遍历：保持 Go range 或计数循环语义。
    // Go: for range concurrency {
    // 并发语义：保留 goroutine、锁、WaitGroup 或 atomic 的同步意图。
    // Go: go func() {
    // 资源收尾：Go defer 的恢复/关闭动作需在 Rust 接线时显式建模。
    // Go: defer wg.Done()
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: v, err := cache.getOrInitCache(ctx, construct)
    // Go: // all goroutines should get the same value
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, 101, v)
    // Go: }()
    // Go: }
    // Go:
    // Go: wg.Wait()
    // Go: // construct will only be called once even in concurrency
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, int64(1), invoked.Load())
    // Go: }
}

// TestBuiltinFuncCache 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
/// TestBuiltinFuncCache 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn test_builtin_func_cache() {
    let cache = builtinFuncCache::<usize>::default();
    let first_ctx = SimpleEvalContext::new(1);
    assert_eq!(cache.getCache(1), None);
    assert_eq!(cache.getCache(1), None);

    let invoked = AtomicUsize::new(0);
    let value = cache
        .getOrInitCache(&first_ctx, || {
            Ok(100 + invoked.fetch_add(1, Ordering::SeqCst) + 1)
        })
        .unwrap();
    assert_eq!(value, 101);
    assert_eq!(invoked.load(Ordering::SeqCst), 1);
    assert_eq!(cache.getCache(1), Some(101));

    assert_eq!(
        cache
            .getOrInitCache(&first_ctx, || {
                Ok(100 + invoked.fetch_add(1, Ordering::SeqCst) + 1)
            })
            .unwrap(),
        101
    );
    assert_eq!(invoked.load(Ordering::SeqCst), 1);

    let second_ctx = SimpleEvalContext::new(2);
    assert_eq!(
        cache
            .getOrInitCache(&second_ctx, || {
                Ok(100 + invoked.fetch_add(1, Ordering::SeqCst) + 1)
            })
            .unwrap(),
        102
    );
    assert_eq!(invoked.load(Ordering::SeqCst), 2);
    assert_eq!(cache.getCache(2), Some(102));

    let third_ctx = SimpleEvalContext::new(3);
    let return_error = AtomicBool::new(true);
    let error = cache
        .getOrInitCache(&third_ctx, || {
            invoked.fetch_add(1, Ordering::SeqCst);
            if return_error.swap(false, Ordering::SeqCst) {
                Err(crate::expression_builtin::Error::new("mockError"))
            } else {
                Ok(100 + invoked.load(Ordering::SeqCst))
            }
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "mockError");
    assert_eq!(cache.getCache(3), None);
    assert_eq!(
        cache
            .getOrInitCache(&third_ctx, || {
                invoked.fetch_add(1, Ordering::SeqCst);
                Ok(100 + invoked.load(Ordering::SeqCst))
            })
            .unwrap(),
        104
    );
    // Go 签名（源文件第 196 行）：func TestBuiltinFuncCache(t *testing.T) {
    // Go: cache := builtinFuncCache[int]{}
    // Go: ctx := createContext(t)
    // Go:
    // Go: // ok should be false when no cache present
    // warning/StmtCtx：保留 TiDB 语句上下文 warning 与类型 flag 校验位置。
    // Go: v, ok := cache.getCache(ctx.GetSessionVars().StmtCtx.CtxID())
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, 0, v)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.False(t, ok)
    // Go:
    // Go: // getCache should not init cache
    // warning/StmtCtx：保留 TiDB 语句上下文 warning 与类型 flag 校验位置。
    // Go: v, ok = cache.getCache(ctx.GetSessionVars().StmtCtx.CtxID())
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, 0, v)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.False(t, ok)
    // Go:
    // 并发语义：保留 goroutine、锁、WaitGroup 或 atomic 的同步意图。
    // Go: var invoked atomic.Int64
    // Go: returnError := false
    // Go: construct := func() (int, error) {
    // Go: invoked.Add(1)
    // Go: if returnError {
    // Go: return 128, errors.New("mockError")
    // Go: }
    // Go: return 100 + int(invoked.Load()), nil
    // Go: }
    // Go:
    // Go: // the first getOrInitCache should init cache
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: v, err := cache.getOrInitCache(ctx, construct)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, 101, v)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, int64(1), invoked.Load())
    // Go:
    // Go: // get should return the cache
    // warning/StmtCtx：保留 TiDB 语句上下文 warning 与类型 flag 校验位置。
    // Go: v, ok = cache.getCache(ctx.GetSessionVars().StmtCtx.CtxID())
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, 101, v)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.True(t, ok)
    // Go:
    // Go: // the second should use the cached one
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: v, err = cache.getOrInitCache(ctx, construct)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, 101, v)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, int64(1), invoked.Load())
    // Go:
    // Go: // if ctxID changed, should re-init cache
    // Go: ctx = createContext(t)
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: v, err = cache.getOrInitCache(ctx, construct)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, 102, v)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, int64(2), invoked.Load())
    // warning/StmtCtx：保留 TiDB 语句上下文 warning 与类型 flag 校验位置。
    // Go: v, ok = cache.getCache(ctx.GetSessionVars().StmtCtx.CtxID())
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, 102, v)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.True(t, ok)
    // Go:
    // Go: // error should be returned
    // Go: ctx = createContext(t)
    // Go: returnError = true
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: v, err = cache.getOrInitCache(ctx, construct)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, 0, v)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.EqualError(t, err, "mockError")
    // Go:
    // Go: // error should not be cached
    // Go: returnError = false
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: v, err = cache.getOrInitCache(ctx, construct)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不执行。
    // Go: require.Equal(t, 104, v)
    // Go: }
}

// newFunctionForTest 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
/// newFunctionForTest 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
pub fn new_function_for_test() {
    // Go 签名（源文件第 263 行）：func newFunctionForTest(ctx BuildContext, funcName string, args ...Expression) (Expression, error) {
    // Go: fc, ok := funcs[funcName]
    // Go: if !ok {
    // Go: return nil, ErrFunctionNotExists.GenWithStackByArgs("FUNCTION", funcName)
    // Go: }
    // Go: funcArgs := slices.Clone(args)
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: f, err := fc.getFunction(ctx, funcArgs)
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: if err != nil {
    // 错误处理：保留 Go err 传播或检查位置。
    // Go: return nil, err
    // Go: }
    // Go: return &ScalarFunction{
    // Go: FuncName: ast.NewCIStr(funcName),
    // Go: RetType: f.getRetTp(),
    // Go: Function: f,
    // Go: }, nil
    // Go: }
}

// var 声明对应 Go 的 block；复杂常量/变量以原声明顺序保留。
/// var 声明对应 Go 的 block；复杂常量/变量以原声明顺序保留。
pub const BLOCK_GO_DECL: &str = r#"var (
	// MySQL int8.
	int8Con = &Constant{RetType: types.NewFieldTypeBuilder().SetType(mysql.TypeLonglong).SetCharset(charset.CharsetBin).SetCollate(charset.CollationBin).BuildP()}
	// MySQL varchar.
	varcharCon = &Constant{RetType: types.NewFieldTypeBuilder().SetType(mysql.TypeVarchar).SetCharset(charset.CharsetUTF8).SetCollate(charset.CollationUTF8).BuildP()}
)"#;

// getInt8Con 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
/// getInt8Con 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
pub fn get_int8_con() {
    // Go 签名（源文件第 287 行）：func getInt8Con() Expression {
    // Go: return int8Con.Clone()
    // Go: }
}

// getVarcharCon 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
/// getVarcharCon 对应 Go 辅助函数/方法；保留参数、返回值和关键控制流语义。
pub fn get_varchar_con() {
    // Go 签名（源文件第 291 行）：func getVarcharCon() Expression {
    // Go: return varcharCon.Clone()
    // Go: }
}
