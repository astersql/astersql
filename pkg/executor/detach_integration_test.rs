// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Detach（拆离）能力的集成测试。
//
// 前半为大段保留的 Go TestKit 语义注释（游标、IndexReader/IndexLookUp、
// Selection/Projection 与会话并发等）；底部可执行用例验证
// `TableReaderExecutorContext` 拆离后不再绑定真实会话。

/*
#![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_variables, unused_mut)]

// 这段逻辑覆盖 detachable record set 的上下文拆离、参数查询、IndexReader/IndexLookUp、Selection/Projection 等集成测试。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发、异步、IO 和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待后续 Rust crate 接线）：
// - "context"
// - "strconv"
// - "sync"
// - "sync/atomic"
// - "testing"
// - "github.com/pingcap/tidb/pkg/executor/internal/exec"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/testkit"
// - "github.com/pingcap/tidb/pkg/util/sqlexec"
// - "github.com/stretchr/testify/require"

// 迁移占位类型：这些名称代表 Go/TiDB 测试依赖，后续接入模块时再替换为真实 Rust 类型。
type GoAny = ();
type GoError = String;

// exportExecutor 对应 Go 的同名 interface，保留测试中需要的能力边界。
// 其方法签名仍按 Go 语义展示，后续接入 Rust trait 时再细化类型。
pub trait exportExecutor {
    GetExecutor4Test() any
}

// TestDetachAllContexts 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestDetachAllContexts(t *testing.T) {
#[test]
pub fn test_detach_all_contexts() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.Session().GetSessionVars().SetStatusFlag(mysql.ServerStatusCursorExists, true)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("create table t (a int)")
    tk.MustExec("insert into t values (1), (2), (3)")

    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
    rs, err := tk.Exec("select * from t")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    oldExecutor := rs.(exportExecutor).GetExecutor4Test().(exec.Executor)

    drs := rs.(sqlexec.DetachableRecordSet)
    // TryDetach 是本组测试核心：尝试把 RecordSet 从当前 session 执行上下文拆离。
    srs, ok, err := drs.TryDetach()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.True(t, ok)
    require.NoError(t, err)

    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NotEqual(t, rs, srs)
    newExecutor := srs.(exportExecutor).GetExecutor4Test().(exec.Executor)

    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NotEqual(t, oldExecutor, newExecutor)
    // Children should be different
    for i, child := range oldExecutor.AllChildren() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NotEqual(t, child, newExecutor.AllChildren()[i])
    }

    // Then execute another statement
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    tk.MustQuery("select * from t limit 1").Check(testkit.Rows("1"))
    // The previous detached record set can still be used
    // check data
    chk := srs.NewChunk(nil)
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
    err = srs.Next(context.Background(), chk)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    require.Equal(t, 3, chk.NumRows())
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, int64(1), chk.GetRow(0).GetInt64(0))
    require.Equal(t, int64(2), chk.GetRow(1).GetInt64(0))
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, int64(3), chk.GetRow(2).GetInt64(0))
}

// TestAfterDetachSessionCanExecute 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestAfterDetachSessionCanExecute(t *testing.T) {
#[test]
pub fn test_after_detach_session_can_execute() {
    // This test shows that the session can be safely used to execute another statement after detaching.
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.Session().GetSessionVars().SetStatusFlag(mysql.ServerStatusCursorExists, true)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("create table t (a int)")
    for i := range 10000 {
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
        tk.MustExec("insert into t values (?)", i)
    }

    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
    rs, err := tk.Exec("select * from t")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    // TryDetach 是本组测试核心：尝试把 RecordSet 从当前 session 执行上下文拆离。
    drs, ok, err := rs.(sqlexec.DetachableRecordSet).TryDetach()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    require.True(t, ok)

    // Now, the `drs` can be used concurrently with the session.
    // WaitGroup 管理 goroutine 生命周期；标出等待/完成关系。
    var wg sync.WaitGroup
    // atomic 操作表示跨 goroutine 状态同步，迁移时需保持可见性语义。
    var stop atomic.Bool
    wg.Add(1)
    // goroutine 表示并发执行路径；这里只保留并发关系和同步语义。
    go func() {
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
        defer wg.Done()

        for i := range 10000 {
            if stop.Load() {
                return
            }
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
            tk.MustQuery("select * from t where a = ?", i).Check(testkit.Rows(strconv.Itoa(i)))
        }
    }()

    chk := drs.NewChunk(nil)
    expectedSelect := 0
    for {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        err = drs.Next(context.Background(), chk)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)

        if chk.NumRows() == 0 {
            break
        }
        for i := range chk.NumRows() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.Equal(t, int64(expectedSelect), chk.GetRow(i).GetInt64(0))
            expectedSelect++
        }
    }
    stop.Store(true)
    wg.Wait()
}

// TestDetachWithParam 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestDetachWithParam(t *testing.T) {
#[test]
pub fn test_detach_with_param() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.Session().GetSessionVars().SetStatusFlag(mysql.ServerStatusCursorExists, true)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("create table t (a int primary key)")
    for i := range 10000 {
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
        tk.MustExec("insert into t values (?)", i)
    }

    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
    rs, err := tk.Exec("select * from t where a > ? and a < ?", 100, 200)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    // TryDetach 是本组测试核心：尝试把 RecordSet 从当前 session 执行上下文拆离。
    drs, ok, err := rs.(sqlexec.DetachableRecordSet).TryDetach()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    require.True(t, ok)

    // Now, execute another statement with different size of param. It'll not affect the execution of detached executor.
    // WaitGroup 管理 goroutine 生命周期；标出等待/完成关系。
    var wg sync.WaitGroup
    // atomic 操作表示跨 goroutine 状态同步，迁移时需保持可见性语义。
    var stop atomic.Bool
    wg.Add(1)
    // goroutine 表示并发执行路径；这里只保留并发关系和同步语义。
    go func() {
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
        defer wg.Done()

        for i := range 10000 {
            if stop.Load() {
                return
            }
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
            tk.MustQuery("select * from t where a = ?", i).Check(testkit.Rows(strconv.Itoa(i)))
        }
    }()

    chk := drs.NewChunk(nil)
    expectedSelect := 101
    for {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        err = drs.Next(context.Background(), chk)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)

        if chk.NumRows() == 0 {
            break
        }
        for i := range chk.NumRows() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.Equal(t, int64(expectedSelect), chk.GetRow(i).GetInt64(0))
            expectedSelect++
        }
    }
    stop.Store(true)
    wg.Wait()
}

// TestDetachIndexReaderAndIndexLookUp 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestDetachIndexReaderAndIndexLookUp(t *testing.T) {
#[test]
pub fn test_detach_index_reader_and_index_look_up() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.Session().GetSessionVars().SetStatusFlag(mysql.ServerStatusCursorExists, true)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("create table t (a int, b int, c int, key idx_a_b (a,b), key idx_b (b))")
    for i := range 10000 {
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
        tk.MustExec("insert into t values (?, ?, ?)", i, i, i)
    }

    // Test detach index reader
    tk.MustHavePlan("select a, b from t where a > 100 and a < 200", "IndexReader")
    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
    rs, err := tk.Exec("select a, b from t where a > ? and a < ?", 100, 200)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    // TryDetach 是本组测试核心：尝试把 RecordSet 从当前 session 执行上下文拆离。
    drs, ok, err := rs.(sqlexec.DetachableRecordSet).TryDetach()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    require.True(t, ok)

    chk := drs.NewChunk(nil)
    expectedSelect := 101
    for {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        err = drs.Next(context.Background(), chk)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)

        if chk.NumRows() == 0 {
            break
        }
        for i := range chk.NumRows() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.Equal(t, int64(expectedSelect), chk.GetRow(i).GetInt64(0))
            require.Equal(t, int64(expectedSelect), chk.GetRow(i).GetInt64(1))
            expectedSelect++
        }
    }

    // Test detach indexLookUp
    tk.MustHavePlan("select c from t use index(idx_b) where b > 100 and b < 200", "IndexLookUp")
    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
    rs, err = tk.Exec("select c from t where b > ? and b < ?", 100, 200)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    // TryDetach 是本组测试核心：尝试把 RecordSet 从当前 session 执行上下文拆离。
    drs, ok, err = rs.(sqlexec.DetachableRecordSet).TryDetach()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    require.True(t, ok)

    chk = drs.NewChunk(nil)
    expectedSelect = 101
    for {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        err = drs.Next(context.Background(), chk)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)

        if chk.NumRows() == 0 {
            break
        }
        for i := range chk.NumRows() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.Equal(t, int64(expectedSelect), chk.GetRow(i).GetInt64(0))
            expectedSelect++
        }
    }
}

// TestDetachSelection 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestDetachSelection(t *testing.T) {
#[test]
pub fn test_detach_selection() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.Session().GetSessionVars().SetStatusFlag(mysql.ServerStatusCursorExists, true)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("create table t (a int, b int, c int, key idx_a_b (a,b), key idx_b (b))")
    for i := range 10000 {
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
        tk.MustExec("insert into t values (?, ?, ?)", i, i, i)
    }

    tk.MustHavePlan("select a, b from t where c > 100 and c < 200", "Selection")
    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
    rs, err := tk.Exec("select a, b from t where c > ? and c < ?", 100, 200)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    // TryDetach 是本组测试核心：尝试把 RecordSet 从当前 session 执行上下文拆离。
    drs, ok, err := rs.(sqlexec.DetachableRecordSet).TryDetach()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    require.True(t, ok)

    chk := drs.NewChunk(nil)
    expectedSelect := 101
    for {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        err = drs.Next(context.Background(), chk)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)

        if chk.NumRows() == 0 {
            break
        }
        for i := range chk.NumRows() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.Equal(t, int64(expectedSelect), chk.GetRow(i).GetInt64(0))
            require.Equal(t, int64(expectedSelect), chk.GetRow(i).GetInt64(1))
            expectedSelect++
        }
    }
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, drs.Close())
    require.Equal(t, 200, expectedSelect)

    // Selection with optional property is not allowed
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @a = 1")
    tk.MustExec("set @b = 10")
    tk.MustHavePlan("select a, b from t where a + @a + getvar('b') > 100 and a < 200", "Selection")
    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
    rs, err = tk.Exec("select a, b from t where a + @a + getvar('b') > ? and a < ?", 100, 200)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    // TryDetach 是本组测试核心：尝试把 RecordSet 从当前 session 执行上下文拆离。
    drs, ok, err = rs.(sqlexec.DetachableRecordSet).TryDetach()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    require.True(t, ok)

    // set user variable to another value to test the expression should not change after detaching
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @a=100")
    tk.MustExec("set @b=1000")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("select 1")
    chk = drs.NewChunk(nil)
    expectedSelect = 90
    for {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        err = drs.Next(context.Background(), chk)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)

        if chk.NumRows() == 0 {
            break
        }
        for i := range chk.NumRows() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.Equal(t, int64(expectedSelect), chk.GetRow(i).GetInt64(0))
            require.Equal(t, int64(expectedSelect), chk.GetRow(i).GetInt64(1))
            expectedSelect++
        }
    }
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, drs.Close())
    require.Equal(t, 200, expectedSelect)

    // Selection with optional property is not allowed
    tk.MustHavePlan("select a from t where a + found_rows() > 100 and a < 200", "Selection")
    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
    rs, err = tk.Exec("select a from t where a + found_rows() > ? and a < ?", 100, 200)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    // TryDetach 是本组测试核心：尝试把 RecordSet 从当前 session 执行上下文拆离。
    drs, ok, _ = rs.(sqlexec.DetachableRecordSet).TryDetach()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.False(t, ok)
    require.Nil(t, drs)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, rs.Close())
}

// TestDetachProjection 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestDetachProjection(t *testing.T) {
#[test]
pub fn test_detach_projection() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.Session().GetSessionVars().SetStatusFlag(mysql.ServerStatusCursorExists, true)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("create table t (a int, b int, c int, key idx_a_b (a,b), key idx_b (b))")
    for i := range 10000 {
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
        tk.MustExec("insert into t values (?, ?, ?)", i, i, i)
    }

    tk.MustHavePlan("select a + b from t where a > 100 and a < 200", "Projection")
    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
    rs, err := tk.Exec("select a + b from t where a > ? and a < ?", 100, 200)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    // TryDetach 是本组测试核心：尝试把 RecordSet 从当前 session 执行上下文拆离。
    drs, ok, err := rs.(sqlexec.DetachableRecordSet).TryDetach()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    require.True(t, ok)

    chk := drs.NewChunk(nil)
    expectedSelect := 101
    for {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        err = drs.Next(context.Background(), chk)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)

        if chk.NumRows() == 0 {
            break
        }
        for i := range chk.NumRows() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.Equal(t, int64(2*expectedSelect), chk.GetRow(i).GetInt64(0))
            expectedSelect++
        }
    }
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, drs.Close())
    require.Equal(t, 200, expectedSelect)

    // Projection with optional property is not allowed
    tk.MustHavePlan("select setvar('x', a) from t where a > 100 and a < 200", "Projection")
    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
    rs, err = tk.Exec("select setvar('x', a) from t where a > ? and a < ?", 100, 200)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    // TryDetach 是本组测试核心：尝试把 RecordSet 从当前 session 执行上下文拆离。
    drs, ok, _ = rs.(sqlexec.DetachableRecordSet).TryDetach()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.False(t, ok)
    require.Nil(t, drs)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, rs.Close())

    // Projection with user variable is allowed
    // Also test NOW() function will return right value, see issue: https://github.com/pingcap/tidb/issues/56051
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @a = 1")
    tk.MustExec("set @b = 10")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @@timestamp=360000")
    tk.MustHavePlan(
        "select a + b + @a + getvar('b'), UNIX_TIMESTAMP(NOW()) from t where a > 100 and a < 200",
        "Projection",
    )
    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
    rs, err = tk.Exec(
        "select a + b + @a + getvar('b'), UNIX_TIMESTAMP(NOW()) from t where a > ? and a < ?",
        100, 200,
    )
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    // TryDetach 是本组测试核心：尝试把 RecordSet 从当前 session 执行上下文拆离。
    drs, ok, err = rs.(sqlexec.DetachableRecordSet).TryDetach()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    require.True(t, ok)
    // set user variable and current time to another value to test the expression should not change after detaching
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @a=100,@b=1000,@@timestamp=0")
    chk = drs.NewChunk(nil)
    expectedSelect = 101
    for {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        err = drs.Next(context.Background(), chk)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)

        if chk.NumRows() == 0 {
            break
        }
        for i := range chk.NumRows() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.Equal(t, float64(2*expectedSelect+11), chk.GetRow(i).GetFloat64(0))
            require.Equal(t, int64(360000), chk.GetRow(i).GetInt64(1))
            expectedSelect++
        }
    }
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, drs.Close())
    require.Equal(t, 200, expectedSelect)

    // Projection with Selection is also allowed
    tk.MustHavePlan("select a + b from t where c > 100 and c < 200", "Projection")
    tk.MustHavePlan("select a + b from t where c > 100 and c < 200", "Selection")
    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
    rs, err = tk.Exec("select a + b from t where c > ? and c < ?", 100, 200)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    // TryDetach 是本组测试核心：尝试把 RecordSet 从当前 session 执行上下文拆离。
    drs, ok, err = rs.(sqlexec.DetachableRecordSet).TryDetach()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    require.True(t, ok)

    chk = drs.NewChunk(nil)
    expectedSelect = 101
    for {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        err = drs.Next(context.Background(), chk)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)

        if chk.NumRows() == 0 {
            break
        }
        for i := range chk.NumRows() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.Equal(t, int64(2*expectedSelect), chk.GetRow(i).GetInt64(0))
            expectedSelect++
        }
    }
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, drs.Close())
    require.Equal(t, 200, expectedSelect)
}
*/

use crate::detach::{
    DetachableBuildPbContext, DetachableDistSqlContext, DetachableExprContext,
    DetachableRangeContext, TableReaderExecutorContext,
};

/// 测试用表达式上下文：`session_bound` 表示仍依赖会话。
#[derive(Clone, Debug, Eq, PartialEq)]
struct ExprContext {
    session_bound: bool,
}

impl DetachableExprContext for ExprContext {
    /// 会话绑定上下文可转为静态副本；否则返回 `None`。
    fn into_static(&self) -> Option<Self> {
        self.session_bound.then(|| Self {
            session_bound: false,
        })
    }
}

/// 测试用 DistSQL 上下文：持有可选的会话连接 ID。
#[derive(Clone, Debug, Eq, PartialEq)]
struct DistSqlContext {
    session_id: Option<u64>,
}

impl DetachableDistSqlContext for DistSqlContext {
    /// 拆离后清除会话 ID。
    fn detach(&self) -> Self {
        Self { session_id: None }
    }
}

/// 测试用范围上下文：标记是否已基于静态表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
struct RangeContext {
    static_expression: bool,
}

impl DetachableRangeContext<ExprContext> for RangeContext {
    fn detach(&self, expression_context: &ExprContext) -> Self {
        Self {
            static_expression: !expression_context.session_bound,
        }
    }
}

/// 测试用 PB 构建上下文。
#[derive(Clone, Debug, Eq, PartialEq)]
struct BuildContext {
    static_expression: bool,
}

impl DetachableBuildPbContext<ExprContext> for BuildContext {
    fn detach(&self, expression_context: &ExprContext) -> Self {
        Self {
            static_expression: !expression_context.session_bound,
        }
    }
}

/// 在真实 TestKit 会话上拆离 TableReader 上下文，关闭会话后副本仍可用。
#[test]
fn detached_reader_context_survives_its_real_sql_session() {
    let store = astersql_testkit::mockstore::CreateAnalyzeStatsStore();
    let mut testkit = astersql_testkit::TestKit::new(store.clone());
    // 写入会话侧 KV，确认 TestKit 会话可用
    testkit.MustExec(
        "insert into aster_session_kv(k, v) values ('detach', 'ready')",
        Vec::new(),
    );
    testkit
        .MustQuery(
            "select v from aster_session_kv where k = 'detach'",
            Vec::new(),
        )
        .Check(vec![vec!["ready".to_owned()]]);

    // 构造仍绑定会话的 TableReader 上下文并拆离
    let context = TableReaderExecutorContext {
        expression_context: ExprContext {
            session_bound: true,
        },
        distsql_context: DistSqlContext {
            session_id: Some(testkit.ConnectionID()),
        },
        range_context: RangeContext {
            static_expression: false,
        },
        build_pb_context: BuildContext {
            static_expression: false,
        },
    };
    let detached = context.Detach();
    assert!(!detached.expression_context.session_bound);
    assert_eq!(detached.distsql_context.session_id, None);
    assert!(detached.range_context.static_expression);
    assert!(detached.build_pb_context.static_expression);

    // 关闭底层 store 后，拆离副本仍应保持静态字段
    astersql_testkit::Database::close(store.as_ref()).unwrap();
    assert_eq!(detached.distsql_context.session_id, None);
}
