// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Executor failpoint（故障注入点）测试。
//
// Failpoint 在关键路径注入延迟、错误或 panic，用于验证可重复读二次读取、
// Shuffle worker 恢复边界、BatchCop 重试，以及 unistore RPC 超时等行为。
//
// 文件前半为大段保留的 Go 机械翻译（块注释内，待后续 harness 接线）；
// 后半为已可运行的 Rust 测试，直接调用生产 failpoint 入口。

/*
#![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_variables, unused_mut)]

// 这段逻辑覆盖 executor failpoint、事务提交模式、region split 超时、coprocessor OOM、UnionExec worker 收尾、死锁表、IndexLookUp pushdown 等测试。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发、异步、IO 和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待后续 Rust crate 接线）：
// - "context"
// - "fmt"
// - "hash/fnv"
// - "math/rand"
// - "strconv"
// - "strings"
// - "sync"
// - "sync/atomic"
// - "testing"
// - "time"
// - "github.com/pingcap/errors"
// - "github.com/pingcap/failpoint"
// - "github.com/pingcap/tidb/pkg/config"
// - "github.com/pingcap/tidb/pkg/config/kerneltype"
// - "github.com/pingcap/tidb/pkg/ddl"
// - "github.com/pingcap/tidb/pkg/executor/internal/exec"
// - "github.com/pingcap/tidb/pkg/executor/unionexec"
// - "github.com/pingcap/tidb/pkg/expression"
// - "github.com/pingcap/tidb/pkg/kv"
// - "github.com/pingcap/tidb/pkg/meta"
// - "github.com/pingcap/tidb/pkg/parser/terror"
// - "github.com/pingcap/tidb/pkg/session"
// - "github.com/pingcap/tidb/pkg/store/copr"
// - "github.com/pingcap/tidb/pkg/store/helper"
// - "github.com/pingcap/tidb/pkg/testkit"
// - "github.com/pingcap/tidb/pkg/util/chunk"
// - "github.com/pingcap/tidb/pkg/util/dbterror/exeerrors"
// - "github.com/pingcap/tidb/pkg/util/deadlockhistory"
// - "github.com/pingcap/tidb/pkg/util/logutil"
// - "github.com/pingcap/tidb/pkg/util/mock"
// - "github.com/pingcap/tidb/pkg/util/sqlkiller"
// - "github.com/stretchr/testify/require"
// - "github.com/tikv/client-go/v2/oracle"
// - "go.uber.org/zap"

// 迁移占位类型：这些名称代表 Go/TiDB 测试依赖，后续接入模块时再替换为真实 Rust 类型。
type GoAny = ();
type GoError = String;

// TestTiDBLastTxnInfoCommitMode 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestTiDBLastTxnInfoCommitMode(t *testing.T) {
#[test]
pub fn test_ti_db_last_txn_info_commit_mode() {
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
    defer config.RestoreFunc()()
    // 全局配置临时改写会影响后续 SQL/执行器行为，必须保留恢复语义。
    config.UpdateGlobal(func(conf *config.Config) {
        conf.TiKVClient.AsyncCommit.SafeWindow = time.Second
    })

    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)

    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.MustExec("create table t (a int primary key, v int)")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("insert into t values (1, 1)")

    tk.MustExec("set @@tidb_enable_async_commit = 1")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @@tidb_enable_1pc = 0")
    tk.MustExec("update t set v = v + 1 where a = 1")
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    rows := tk.MustQuery("select json_extract(@@tidb_last_txn_info, '$.txn_commit_mode'), json_extract(@@tidb_last_txn_info, '$.async_commit_fallback'), json_extract(@@tidb_last_txn_info, '$.one_pc_fallback')").Rows()
    t.Log(rows)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, `"async_commit"`, rows[0][0])
    require.Equal(t, "false", rows[0][1])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, "false", rows[0][2])

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @@tidb_enable_async_commit = 0")
    tk.MustExec("set @@tidb_enable_1pc = 1")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("update t set v = v + 1 where a = 1")
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    rows = tk.MustQuery("select json_extract(@@tidb_last_txn_info, '$.txn_commit_mode'), json_extract(@@tidb_last_txn_info, '$.async_commit_fallback'), json_extract(@@tidb_last_txn_info, '$.one_pc_fallback')").Rows()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, `"1pc"`, rows[0][0])
    require.Equal(t, "false", rows[0][1])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, "false", rows[0][2])

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @@tidb_enable_async_commit = 0")
    tk.MustExec("set @@tidb_enable_1pc = 0")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("update t set v = v + 1 where a = 1")
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    rows = tk.MustQuery("select json_extract(@@tidb_last_txn_info, '$.txn_commit_mode'), json_extract(@@tidb_last_txn_info, '$.async_commit_fallback'), json_extract(@@tidb_last_txn_info, '$.one_pc_fallback')").Rows()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, `"2pc"`, rows[0][0])
    require.Equal(t, "false", rows[0][1])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, "false", rows[0][2])

    // failpoint.Enable 注入外部故障点；不会启用真实 failpoint，只标出作用域。
    require.NoError(t, failpoint.Enable("tikvclient/invalidMaxCommitTS", "return"))
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
    defer func() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, failpoint.Disable("tikvclient/invalidMaxCommitTS"))
    }()

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @@tidb_enable_async_commit = 1")
    tk.MustExec("set @@tidb_enable_1pc = 0")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("update t set v = v + 1 where a = 1")
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    rows = tk.MustQuery("select json_extract(@@tidb_last_txn_info, '$.txn_commit_mode'), json_extract(@@tidb_last_txn_info, '$.async_commit_fallback'), json_extract(@@tidb_last_txn_info, '$.one_pc_fallback')").Rows()
    t.Log(rows)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, `"2pc"`, rows[0][0])
    require.Equal(t, "true", rows[0][1])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, "false", rows[0][2])

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @@tidb_enable_async_commit = 0")
    tk.MustExec("set @@tidb_enable_1pc = 1")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("update t set v = v + 1 where a = 1")
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    rows = tk.MustQuery("select json_extract(@@tidb_last_txn_info, '$.txn_commit_mode'), json_extract(@@tidb_last_txn_info, '$.async_commit_fallback'), json_extract(@@tidb_last_txn_info, '$.one_pc_fallback')").Rows()
    t.Log(rows)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, `"2pc"`, rows[0][0])
    require.Equal(t, "false", rows[0][1])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, "true", rows[0][2])

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @@tidb_enable_async_commit = 1")
    tk.MustExec("set @@tidb_enable_1pc = 1")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("update t set v = v + 1 where a = 1")
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    rows = tk.MustQuery("select json_extract(@@tidb_last_txn_info, '$.txn_commit_mode'), json_extract(@@tidb_last_txn_info, '$.async_commit_fallback'), json_extract(@@tidb_last_txn_info, '$.one_pc_fallback')").Rows()
    t.Log(rows)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, `"2pc"`, rows[0][0])
    require.Equal(t, "true", rows[0][1])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, "true", rows[0][2])
}

// TestPointGetRepeatableRead 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestPointGetRepeatableRead(t *testing.T) {
#[test]
pub fn test_point_get_repeatable_read() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)

    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk1 := testkit.NewTestKit(t, store)
    tk1.MustExec("use test")
    tk1.MustExec(`create table point_get (a int, b int, c int,
            primary key k_a(a),
            unique key k_b(b))`)
    tk1.MustExec("insert into point_get values (1, 1, 1)")
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk2 := testkit.NewTestKit(t, store)
    tk2.MustExec("use test")

    var (
        step1 = "github.com/pingcap/tidb/pkg/executor/pointGetRepeatableReadTest-step1"
        step2 = "github.com/pingcap/tidb/pkg/executor/pointGetRepeatableReadTest-step2"
    )

    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable(step1, "return"))
    // failpoint.Enable 注入外部故障点；不会启用真实 failpoint，只标出作用域。
    require.NoError(t, failpoint.Enable(step2, "pause"))

    updateWaitCh := make(chan struct{})
    // goroutine 表示并发执行路径；这里只保留并发关系和同步语义。
    go func() {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        ctx := context.WithValue(context.Background(), "pointGetRepeatableReadTest", updateWaitCh)
        ctx = failpoint.WithHook(ctx, func(ctx context.Context, fpname string) bool {
            return fpname == step1 || fpname == step2
        })
        rs, err := tk1.Session().Execute(ctx, "select c from point_get where b = 1")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)
        result := tk1.ResultSetToResultWithCtx(ctx, rs[0], "execute sql fail")
        result.Check(testkit.Rows("1"))
    }()

    <-updateWaitCh // Wait `POINT GET` first time `get`
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Disable(step1))
    tk2.MustExec("update point_get set b = 2, c = 2 where a = 1")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Disable(step2))
}

// TestBatchPointGetRepeatableRead 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestBatchPointGetRepeatableRead(t *testing.T) {
#[test]
pub fn test_batch_point_get_repeatable_read() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)

    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk1 := testkit.NewTestKit(t, store)
    tk1.MustExec("use test")
    tk1.MustExec(`create table batch_point_get (a int, b int, c int, unique key k_b(a, b, c))`)
    tk1.MustExec("insert into batch_point_get values (1, 1, 1), (2, 3, 4), (3, 4, 5)")
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk2 := testkit.NewTestKit(t, store)
    tk2.MustExec("use test")

    var (
        step1 = "github.com/pingcap/tidb/pkg/executor/batchPointGetRepeatableReadTest-step1"
        step2 = "github.com/pingcap/tidb/pkg/executor/batchPointGetRepeatableReadTest-step2"
    )

    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable(step1, "return"))
    // failpoint.Enable 注入外部故障点；不会启用真实 failpoint，只标出作用域。
    require.NoError(t, failpoint.Enable(step2, "pause"))

    updateWaitCh := make(chan struct{})
    // goroutine 表示并发执行路径；这里只保留并发关系和同步语义。
    go func() {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        ctx := context.WithValue(context.Background(), "batchPointGetRepeatableReadTest", updateWaitCh)
        ctx = failpoint.WithHook(ctx, func(ctx context.Context, fpname string) bool {
            return fpname == step1 || fpname == step2
        })
        rs, err := tk1.Session().Execute(ctx, "select c from batch_point_get where (a, b, c) in ((1, 1, 1))")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)
        result := tk1.ResultSetToResultWithCtx(ctx, rs[0], "execute sql fail")
        result.Check(testkit.Rows("1"))
    }()

    <-updateWaitCh // Wait `POINT GET` first time `get`
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Disable(step1))
    tk2.MustExec("update batch_point_get set b = 2, c = 2 where a = 1")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Disable(step2))
}

// TestSplitRegionTimeout 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestSplitRegionTimeout(t *testing.T) {
#[test]
pub fn test_split_region_timeout() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)

    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable("tikvclient/injectLiveness", `return("reachable")`))
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
    defer func() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, failpoint.Disable("tikvclient/injectLiveness"))
    }()

    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable("tikvclient/mockSplitRegionTimeout", `return(true)`))
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("create table t(a varchar(100),b int, index idx1(b,a))")
    tk.MustExec(`split table t index idx1 by (10000,"abcd"),(10000000);`)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec(`set @@tidb_wait_split_region_timeout=1`)
    // result 0 0 means split 0 region and 0 region finish scatter regions before timeout.
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    tk.MustQuery(`split table t between (0) and (10000) regions 10`).Check(testkit.Rows("0 0"))
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Disable("tikvclient/mockSplitRegionTimeout"))

    // Test scatter regions timeout.
    // failpoint.Enable 注入外部故障点；不会启用真实 failpoint，只标出作用域。
    require.NoError(t, failpoint.Enable("tikvclient/mockScatterRegionTimeout", `return(true)`))
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    tk.MustQuery(`split table t between (0) and (10000) regions 10`).Check(testkit.Rows("10 1"))
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Disable("tikvclient/mockScatterRegionTimeout"))

    // Test pre-split with timeout.
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("drop table if exists t")
    tk.MustExec("set @@session.tidb_scatter_region='table';")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable("tikvclient/mockScatterRegionTimeout", `return(true)`))
    // atomic 操作表示跨 goroutine 状态同步，迁移时需保持可见性语义。
    atomic.StoreUint32(&ddl.EnableSplitTableRegion, 1)
    start := time.Now()
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("create table t (a int, b int) partition by hash(a) partitions 5;")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Less(t, time.Since(start).Seconds(), 10.0)
    // failpoint.Disable 是 failpoint 资源收尾，必须与启用点成对保留。
    require.NoError(t, failpoint.Disable("tikvclient/mockScatterRegionTimeout"))
}

// TestTSOFail 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestTSOFail(t *testing.T) {
#[test]
pub fn test_tso_fail() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)

    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec(`use test`)
    tk.MustExec(`drop table if exists t`)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec(`create table t(a int)`)

    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/session/mockGetTSFail", "return"))
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
    ctx := failpoint.WithHook(context.Background(), func(ctx context.Context, fpname string) bool {
        return fpname == "github.com/pingcap/tidb/pkg/session/mockGetTSFail"
    })
    _, err := tk.Session().Execute(ctx, `select * from t`)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Error(t, err)
    // failpoint.Disable 是 failpoint 资源收尾，必须与启用点成对保留。
    require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/session/mockGetTSFail"))
}

// TestKillTableReader 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestKillTableReader(t *testing.T) {
#[test]
pub fn test_kill_table_reader() {
    var retry = "tikvclient/mockRetrySendReqToRegion"
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
    defer func() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, failpoint.Disable(retry))
    }()
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)

    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test;")
    tk.MustExec("drop table if exists t")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("create table t (a int)")
    tk.MustExec("insert into t values (1),(2),(3)")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @@tidb_distsql_scan_concurrency=1")
    tk.Session().GetSessionVars().SQLKiller.Reset()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable(retry, `return(true)`))
    // WaitGroup 管理 goroutine 生命周期；标出等待/完成关系。
    wg := &sync.WaitGroup{}
    wg.Add(1)
    // goroutine 表示并发执行路径；这里只保留并发关系和同步语义。
    go func() {
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
        defer wg.Done()
    // Sleep 用于等待异步状态或 race 窗口；不真实等待业务条件。
        time.Sleep(300 * time.Millisecond)
        tk.Session().GetSessionVars().SQLKiller.SendKillSignal(sqlkiller.QueryInterrupted)
    }()
    err := tk.QueryToErr("select * from t")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Error(t, err)
    require.Equal(t, int(exeerrors.ErrQueryInterrupted.Code()), int(terror.ToSQLError(errors.Cause(err).(*terror.Error)).Code))
    wg.Wait()
}

// TestCollectCopRuntimeStats 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestCollectCopRuntimeStats(t *testing.T) {
#[test]
pub fn test_collect_cop_runtime_stats() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)

    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test;")
    tk.MustExec("create table t1 (a int, b int)")
    // Sleep 用于等待异步状态或 race 窗口；不真实等待业务条件。
    time.Sleep(1 * time.Second)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set tidb_enable_collect_execution_info=1;")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable("tikvclient/tikvStoreRespResult", `return(true)`))
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    rows := tk.MustQuery("explain analyze select * from t1").Rows()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Len(t, rows, 2)
    explain := fmt.Sprintf("%v", rows[0])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Regexp(t, ".*num_rpc:.*, .*regionMiss:.*", explain)
    // failpoint.Disable 是 failpoint 资源收尾，必须与启用点成对保留。
    require.NoError(t, failpoint.Disable("tikvclient/tikvStoreRespResult"))
}

// TestCoprocessorOOMTiCase 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestCoprocessorOOMTiCase(t *testing.T) {
#[test]
pub fn test_coprocessor_oom_ti_case() {
    t.Skip("skip")
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.MustExec(`set @@tidb_wait_split_region_finish=1`)
    // create table for non keep-order case
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("drop table if exists t5")
    tk.MustExec("create table t5(id int)")
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    tk.MustQuery(`split table t5 between (0) and (10000) regions 10`).Check(testkit.Rows("9 1"))
    // create table for keep-order case
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("drop table if exists t6")
    tk.MustExec("create table t6(id int, index(id))")
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    tk.MustQuery(`split table t6 between (0) and (10000) regions 10`).Check(testkit.Rows("10 1"))
    tk.MustQuery("split table t6 INDEX id between (0) and (10000) regions 10;").Check(testkit.Rows("10 1"))
    count := 10
    for i := range count {
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
        tk.MustExec(fmt.Sprintf("insert into t5 (id) values (%v)", i))
        tk.MustExec(fmt.Sprintf("insert into t6 (id) values (%v)", i))
    }
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    defer tk.MustExec("SET GLOBAL tidb_mem_oom_action = DEFAULT")
    tk.MustExec("SET GLOBAL tidb_mem_oom_action='LOG'")
    testcases := []struct {
        name string
        sql  string
    }{
        {
            name: "keep Order",
            sql:  "select id from t6 order by id",
        },
        {
            name: "non keep Order",
            sql:  "select id from t5",
        },
    }

    f := func() {
        for _, testcase := range testcases {
            t.Log(testcase.name)
            // larger than one copResponse, smaller than 2 copResponse
            quota := 2*copr.MockResponseSizeForTest - 100
            se, err := session.CreateSession4Test(store)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.NoError(t, err)
            tk.SetSession(se)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
            tk.MustExec("use test")
            tk.MustExec(fmt.Sprintf("set @@tidb_mem_quota_query=%v;", quota))
            var expect []string
            for i := range count {
                expect = append(expect, fmt.Sprintf("%v", i))
            }
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
            tk.MustQuery(testcase.sql).Sort().Check(testkit.Rows(expect...))
            // assert oom action worked by max consumed > memory quota
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.Greater(t, tk.Session().GetSessionVars().StmtCtx.MemTracker.MaxConsumed(), int64(quota))
    // Close 释放 executor/record set 资源；保留收尾调用点。
            se.Close()
        }
    }

    // ticase-4169, trigger oom action twice after workers consuming all the data
    // failpoint.Enable 注入外部故障点；不会启用真实 failpoint，只标出作用域。
    err := failpoint.Enable("github.com/pingcap/tidb/pkg/store/copr/ticase-4169", `return(true)`)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    f()
    // failpoint.Disable 是 failpoint 资源收尾，必须与启用点成对保留。
    err = failpoint.Disable("github.com/pingcap/tidb/pkg/store/copr/ticase-4169")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    /*
        // ticase-4170, trigger oom action twice after iterator receiving all the data.
    // failpoint.Enable 注入外部故障点；不会启用真实 failpoint，只标出作用域。
        err = failpoint.Enable("github.com/pingcap/tidb/pkg/store/copr/ticase-4170", `return(true)`)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)
        f()
    // failpoint.Disable 是 failpoint 资源收尾，必须与启用点成对保留。
        err = failpoint.Disable("github.com/pingcap/tidb/pkg/store/copr/ticase-4170")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)
        // ticase-4171, trigger oom before reading or consuming any data
    // failpoint.Enable 注入外部故障点；不会启用真实 failpoint，只标出作用域。
        err = failpoint.Enable("github.com/pingcap/tidb/pkg/store/copr/ticase-4171", `return(true)`)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)
        f()
    // failpoint.Disable 是 failpoint 资源收尾，必须与启用点成对保留。
        err = failpoint.Disable("github.com/pingcap/tidb/pkg/store/copr/ticase-4171")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)

    */
}

// TestCoprocessorBlockIssues56916 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestCoprocessorBlockIssues56916(t *testing.T) {
#[test]
pub fn test_coprocessor_block_issues56916() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/copr/issue56916", `return`))
    // failpoint.Disable 是 failpoint 资源收尾，必须与启用点成对保留。
    defer func() { require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/copr/issue56916")) }()

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t_cooldown")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("create table t_cooldown (id int auto_increment, k int, unique index(id));")
    tk.MustExec("insert into t_cooldown (k) values (1);")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("insert into t_cooldown (k) select id from t_cooldown;")
    tk.MustExec("insert into t_cooldown (k) select id from t_cooldown;")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("insert into t_cooldown (k) select id from t_cooldown;")
    tk.MustExec("insert into t_cooldown (k) select id from t_cooldown;")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("split table t_cooldown by (1),(2),(3),(4),(5),(6),(7),(8),(9),(10);")
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    tk.MustQuery("select * from t_cooldown use index(id) where id > 0 and id < 10").CheckContain("1")
    tk.MustQuery("select * from t_cooldown use index(id) where id between 1 and 10 or id between 124660 and 132790;").CheckContain("1")
}

// TestIssue21441 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestIssue21441(t *testing.T) {
#[test]
pub fn test_issue21441() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/union/issue21441", `return`))
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
    defer func() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/union/issue21441"))
    }()

    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)

    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("create table t(a int)")
    tk.MustExec(`insert into t values(1),(2),(3)`)
    tk.Session().GetSessionVars().InitChunkSize = 1
    tk.Session().GetSessionVars().MaxChunkSize = 1
    sql := `
select a from t union all
select a from t union all
select a from t union all
select a from t union all
select a from t union all
select a from t union all
select a from t union all
select a from t`
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    tk.MustQuery(sql).Sort().Check(testkit.Rows(
        "1", "1", "1", "1", "1", "1", "1", "1",
        "2", "2", "2", "2", "2", "2", "2", "2",
        "3", "3", "3", "3", "3", "3", "3", "3",
    ))

    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    tk.MustQuery("select a from (" + sql + ") t order by a limit 4").Check(testkit.Rows("1", "1", "1", "1"))
    tk.MustQuery("select a from (" + sql + ") t order by a limit 7, 4").Check(testkit.Rows("1", "2", "2", "2"))

    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("set @@tidb_executor_concurrency = 2")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, 2, tk.Session().GetSessionVars().UnionConcurrency())
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    tk.MustQuery("select a from (" + sql + ") t order by a limit 4").Check(testkit.Rows("1", "1", "1", "1"))
    tk.MustQuery("select a from (" + sql + ") t order by a limit 7, 4").Check(testkit.Rows("1", "2", "2", "2"))
}

// unionEmptyExec 对应 Go 的同名 struct，保留 mock/fixture 字段顺序。
// 字段类型仍按 Go 依赖名展示，当前不构造真实对象。
pub struct unionEmptyExec {
    *exec.BaseExecutor
}

// Open 对应 Go 的同名方法，保留 mock executor 或 PD client 的外部接口语义。
// Go 签名: func (e *unionEmptyExec) Open(context.Context) error {
pub fn Open() {
    return nil
}

// Next 对应 Go 的同名方法，保留 mock executor 或 PD client 的外部接口语义。
// Go 签名: func (e *unionEmptyExec) Next(_ context.Context, req *chunk.Chunk) error {
pub fn Next() {
    req.Reset()
    return nil
}

// Close 对应 Go 的同名方法，保留 mock executor 或 PD client 的外部接口语义。
// Go 签名: func (e *unionEmptyExec) Close() error {
pub fn Close() {
    return nil
}

// unionPanicExec 对应 Go 的同名 struct，保留 mock/fixture 字段顺序。
// 字段类型仍按 Go 依赖名展示，当前不构造真实对象。
pub struct unionPanicExec {
    *exec.BaseExecutor
    nextEntered chan struct{}
    panicCh     <-chan struct{}
}

// Open 对应 Go 的同名方法，保留 mock executor 或 PD client 的外部接口语义。
// Go 签名: func (e *unionPanicExec) Open(context.Context) error {
pub fn Open() {
    return nil
}

// Next 对应 Go 的同名方法，保留 mock executor 或 PD client 的外部接口语义。
// Go 签名: func (e *unionPanicExec) Next(_ context.Context, _ *chunk.Chunk) error {
pub fn Next() {
    close(e.nextEntered)
    <-e.panicCh
    panic("union exec panic during close")
}

// Close 对应 Go 的同名方法，保留 mock executor 或 PD client 的外部接口语义。
// Go 签名: func (e *unionPanicExec) Close() error {
pub fn Close() {
    return nil
}

// TestUnionExecCloseWaitsForWorkers 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestUnionExecCloseWaitsForWorkers(t *testing.T) {
#[test]
pub fn test_union_exec_close_waits_for_workers() {
    fp := "github.com/pingcap/tidb/pkg/executor/unionexec/pauseUnionExecResultPuller"
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable(fp, "pause"))
    fpEnabled := true
    t.Cleanup(func() {
        if fpEnabled {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.NoError(t, failpoint.Disable(fp))
        }
    })

    ctx := mock.NewContext()
    schema := expression.NewSchema()
    childBase := exec.NewBaseExecutor(ctx, schema, 0)
    child := &unionEmptyExec{BaseExecutor: &childBase}
    unionBase := exec.NewBaseExecutor(ctx, schema, 1, child)
    union := &unionexec.UnionExec{
        BaseExecutor: unionBase,
        Concurrency:  1,
    }

    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, exec.Open(context.Background(), union))
    chk := exec.NewFirstChunk(union)

    nextDone := make(chan struct{})
    // goroutine 表示并发执行路径；这里只保留并发关系和同步语义。
    go func() {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        _ = union.Next(context.Background(), chk)
        close(nextDone)
    }()

    select {
    case <-nextDone:
        t.Fatalf("union Next returned before workers paused")
    case <-time.After(100 * time.Millisecond):
    }

    closeDone := make(chan struct{})
    // goroutine 表示并发执行路径；这里只保留并发关系和同步语义。
    go func() {
    // Close 释放 executor/record set 资源；保留收尾调用点。
        _ = union.Close()
        close(closeDone)
    }()

    select {
    case <-closeDone:
        t.Fatalf("union Close returned while workers paused")
    case <-time.After(100 * time.Millisecond):
    }

    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Disable(fp))
    fpEnabled = false

    select {
    case <-closeDone:
    case <-time.After(2 * time.Second):
        t.Fatalf("union Close did not return after workers resumed")
    }

    select {
    case <-nextDone:
    case <-time.After(2 * time.Second):
        t.Fatalf("union Next did not return after Close")
    }
}

// TestUnionExecCloseReturnsAfterWorkerPanicDuringShutdown 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestUnionExecCloseReturnsAfterWorkerPanicDuringShutdown(t *testing.T) {
#[test]
pub fn test_union_exec_close_returns_after_worker_panic_during_shutdown() {
    ctx := mock.NewContext()
    schema := expression.NewSchema()
    panicCh := make(chan struct{})
    nextEntered := make(chan struct{})
    childBase := exec.NewBaseExecutor(ctx, schema, 0)
    child := &unionPanicExec{
        BaseExecutor: &childBase,
        nextEntered:  nextEntered,
        panicCh:      panicCh,
    }
    unionBase := exec.NewBaseExecutor(ctx, schema, 1, child)
    union := &unionexec.UnionExec{
        BaseExecutor: unionBase,
        Concurrency:  1,
    }

    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, exec.Open(context.Background(), union))
    chk := exec.NewFirstChunk(union)

    nextDone := make(chan struct{})
    // goroutine 表示并发执行路径；这里只保留并发关系和同步语义。
    go func() {
    // context 传递取消、超时或请求 hook；不创建真实运行上下文。
        _ = union.Next(context.Background(), chk)
        close(nextDone)
    }()

    select {
    case <-nextEntered:
    case <-time.After(2 * time.Second):
        t.Fatalf("union worker did not enter Next")
    }

    closeDone := make(chan struct{})
    // goroutine 表示并发执行路径；这里只保留并发关系和同步语义。
    go func() {
    // Close 释放 executor/record set 资源；保留收尾调用点。
        _ = union.Close()
        close(closeDone)
    }()

    // Close closes finished before waiting, so once it is blocked here the worker
    // will hit the sendResult(false) path when it panics.
    select {
    case <-closeDone:
        t.Fatalf("union Close returned before worker panic")
    case <-time.After(100 * time.Millisecond):
    }

    close(panicCh)

    select {
    case <-closeDone:
    case <-time.After(2 * time.Second):
        t.Fatalf("union Close did not return after worker panic")
    }

    select {
    case <-nextDone:
    case <-time.After(2 * time.Second):
        t.Fatalf("union Next did not return after worker panic")
    }
}

// TestTxnWriteThroughputSLI 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestTxnWriteThroughputSLI(t *testing.T) {
#[test]
pub fn test_txn_write_throughput_sli() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)

    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    setTxnTk := testkit.NewTestKit(t, store)
    setTxnTk.MustExec("set global tidb_txn_mode=''")
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("create table t (a int key, b int)")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/util/sli/CheckTxnWriteThroughput", "return(true)"))
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
    defer func() {
    // failpoint.Disable 是 failpoint 资源收尾，必须与启用点成对保留。
        err := failpoint.Disable("github.com/pingcap/tidb/pkg/util/sli/CheckTxnWriteThroughput")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)
    }()

    mustExec := func(sql string) {
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
        tk.MustExec(sql)
        tk.Session().GetTxnWriteThroughputSLI().FinishExecuteStmt(time.Second, tk.Session().AffectedRows(), tk.Session().GetSessionVars().InTxn())
    }
    errExec := func(sql string) {
    // Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
        err := tk.ExecToErr(sql)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.Error(t, err)
        tk.Session().GetTxnWriteThroughputSLI().FinishExecuteStmt(time.Second, tk.Session().AffectedRows(), tk.Session().GetSessionVars().InTxn())
    }

    // Test insert in small txn
    mustExec("insert into t values (1,3),(2,4)")
    writeSLI := tk.Session().GetTxnWriteThroughputSLI()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.False(t, writeSLI.IsInvalid())
    require.True(t, writeSLI.IsSmallTxn())
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, "invalid: false, affectRow: 2, writeSize: 58, readKeys: 0, writeKeys: 2, writeTime: 1s", tk.Session().GetTxnWriteThroughputSLI().String())
    tk.Session().GetTxnWriteThroughputSLI().Reset()

    // Test insert ... select ... from
    mustExec("insert into t select b, a from t")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.True(t, writeSLI.IsInvalid())
    require.True(t, writeSLI.IsSmallTxn())
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, "invalid: true, affectRow: 2, writeSize: 58, readKeys: 2, writeKeys: 2, writeTime: 1s", tk.Session().GetTxnWriteThroughputSLI().String())
    tk.Session().GetTxnWriteThroughputSLI().Reset()

    // Test for delete
    mustExec("delete from t")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, "invalid: false, affectRow: 4, writeSize: 76, readKeys: 4, writeKeys: 4, writeTime: 1s", tk.Session().GetTxnWriteThroughputSLI().String())
    tk.Session().GetTxnWriteThroughputSLI().Reset()

    // Test insert not in small txn
    mustExec("begin")
    for i := range 20 {
        mustExec(fmt.Sprintf("insert into t values (%v,%v)", i, i))
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.True(t, writeSLI.IsSmallTxn())
    }
    // The statement which affect rows is 0 shouldn't record into time.
    mustExec("select count(*) from t")
    mustExec("select * from t")
    mustExec("insert into t values (20,20)")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.False(t, writeSLI.IsSmallTxn())
    mustExec("commit")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.False(t, writeSLI.IsInvalid())
    require.Equal(t, "invalid: false, affectRow: 21, writeSize: 609, readKeys: 0, writeKeys: 21, writeTime: 22s", tk.Session().GetTxnWriteThroughputSLI().String())
    tk.Session().GetTxnWriteThroughputSLI().Reset()

    // Test invalid when transaction has replace ... select ... from ... statement.
    mustExec("delete from t")
    tk.Session().GetTxnWriteThroughputSLI().Reset()
    mustExec("begin")
    mustExec("insert into t values (1,3),(2,4)")
    mustExec("replace into t select b, a from t")
    mustExec("commit")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.True(t, writeSLI.IsInvalid())
    require.Equal(t, "invalid: true, affectRow: 4, writeSize: 116, readKeys: 0, writeKeys: 4, writeTime: 3s", tk.Session().GetTxnWriteThroughputSLI().String())
    tk.Session().GetTxnWriteThroughputSLI().Reset()

    // Test clean last failed transaction information.
    // failpoint.Disable 是 failpoint 资源收尾，必须与启用点成对保留。
    err := failpoint.Disable("github.com/pingcap/tidb/pkg/util/sli/CheckTxnWriteThroughput")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, err)
    mustExec("begin")
    mustExec("insert into t values (1,3),(2,4)")
    errExec("commit")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, "invalid: false, affectRow: 0, writeSize: 0, readKeys: 0, writeKeys: 0, writeTime: 0s", tk.Session().GetTxnWriteThroughputSLI().String())

    // failpoint.Enable 注入外部故障点；不会启用真实 failpoint，只标出作用域。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/util/sli/CheckTxnWriteThroughput", "return(true)"))
    mustExec("begin")
    mustExec("insert into t values (5, 6)")
    mustExec("commit")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, "invalid: false, affectRow: 1, writeSize: 29, readKeys: 0, writeKeys: 1, writeTime: 2s", tk.Session().GetTxnWriteThroughputSLI().String())

    // Test for reset
    tk.Session().GetTxnWriteThroughputSLI().Reset()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.Equal(t, "invalid: false, affectRow: 0, writeSize: 0, readKeys: 0, writeKeys: 0, writeTime: 0s", tk.Session().GetTxnWriteThroughputSLI().String())
}

// TestDeadlocksTable 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestDeadlocksTable(t *testing.T) {
#[test]
pub fn test_deadlocks_table() {
    deadlockhistory.GlobalDeadlockHistory.Clear()
    deadlockhistory.GlobalDeadlockHistory.Resize(10)

    occurTime := time.Date(2021, 5, 10, 1, 2, 3, 456789000, time.Local)
    rec := &deadlockhistory.DeadlockRecord{
        OccurTime:   occurTime,
        IsRetryable: false,
        WaitChain: []deadlockhistory.WaitChainItem{
            {
                TryLockTxn:     101,
                SQLDigest:      "aabbccdd",
                Key:            []byte("k1"),
                AllSQLDigests:  nil,
                TxnHoldingLock: 102,
            },
            {
                TryLockTxn:     102,
                SQLDigest:      "ddccbbaa",
                Key:            []byte("k2"),
                AllSQLDigests:  []string{"sql1"},
                TxnHoldingLock: 101,
            },
        },
    }
    deadlockhistory.GlobalDeadlockHistory.Push(rec)

    occurTime2 := time.Date(2022, 6, 11, 2, 3, 4, 987654000, time.Local)
    rec2 := &deadlockhistory.DeadlockRecord{
        OccurTime:   occurTime2,
        IsRetryable: true,
        WaitChain: []deadlockhistory.WaitChainItem{
            {
                TryLockTxn:     201,
                AllSQLDigests:  []string{},
                TxnHoldingLock: 202,
            },
            {
                TryLockTxn:     202,
                AllSQLDigests:  []string{"sql1", "sql2, sql3"},
                TxnHoldingLock: 203,
            },
            {
                TryLockTxn:     203,
                TxnHoldingLock: 201,
            },
        },
    }
    deadlockhistory.GlobalDeadlockHistory.Push(rec2)

    // `Push` sets the record's ID, and ID in a single DeadlockHistory is monotonically increasing. We must get it here
    // to know what it is.
    id1 := strconv.FormatUint(rec.ID, 10)
    id2 := strconv.FormatUint(rec2.ID, 10)

    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/expression/sqlDigestRetrieverSkipRetrieveGlobal", "return"))
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
    defer func() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/expression/sqlDigestRetrieverSkipRetrieveGlobal"))
    }()

    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)

    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    tk.MustQuery("select * from information_schema.deadlocks").Check(
        testkit.RowsWithSep("/",
            id1+"/2021-05-10 01:02:03.456789/0/101/aabbccdd/<nil>/6B31/<nil>/102",
            id1+"/2021-05-10 01:02:03.456789/0/102/ddccbbaa/<nil>/6B32/<nil>/101",
            id2+"/2022-06-11 02:03:04.987654/1/201/<nil>/<nil>/<nil>/<nil>/202",
            id2+"/2022-06-11 02:03:04.987654/1/202/<nil>/<nil>/<nil>/<nil>/203",
            id2+"/2022-06-11 02:03:04.987654/1/203/<nil>/<nil>/<nil>/<nil>/201",
        ))
}

// TestTiKVClientReadTimeout 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestTiKVClientReadTimeout(t *testing.T) {
#[test]
pub fn test_ti_kv_client_read_timeout() {
    if *testkit.WithTiKV != "" {
        t.Skip("skip test since it's only work for unistore")
    }
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
    store := testkit.CreateMockStore(t)
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
    tk := testkit.NewTestKit(t, store)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
    tk.MustExec("use test")
    tk.MustExec("create table t (a int primary key, b int)")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/mockstore/unistore/unistoreRPCDeadlineExceeded", `return(true)`))
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
    defer func() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/mockstore/unistore/unistoreRPCDeadlineExceeded"))
    }()

    waitUntilReadTSSafe := func(tk *testkit.TestKit, readTime string) {
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
        unixTime, err := strconv.ParseFloat(tk.MustQuery("select unix_timestamp(" + readTime + ")").Rows()[0][0].(string), 64)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
        require.NoError(t, err)
        expectedPhysical := int64(unixTime*1000) + 1
        expectedTS := oracle.ComposeTS(expectedPhysical, 0)
        for {
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
            tk.MustExec("begin")
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
            currentTS, err := strconv.ParseUint(tk.MustQuery("select @@tidb_current_ts").Rows()[0][0].(string), 10, 64)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
            require.NoError(t, err)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
            tk.MustExec("rollback")

            if currentTS >= expectedTS {
                return
            }

    // Sleep 用于等待异步状态或 race 窗口；不真实等待业务条件。
            time.Sleep(5 * time.Millisecond)
        }
    }

    // Test for point_get request
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
    rows := tk.MustQuery("explain analyze select /*+ set_var(tikv_client_read_timeout=1) */
 * from t where a = 1").Rows()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Len(t, rows, 1)
	explain := fmt.Sprintf("%v", rows[0])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Regexp(t, ".*Point_Get.* Get:{num_rpc:2, total_time:.*", explain)

	// Test for batch_point_get request
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
rows = tk.MustQuery("explain analyze select /*+ set_var(tikv_client_read_timeout=1) */
 * from t where a in (1,2)").Rows()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Len(t, rows, 1)
	explain = fmt.Sprintf("%v", rows[0])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Regexp(t, ".*Batch_Point_Get.* BatchGet:{num_rpc:2, total_time:.*", explain)

	// Test for cop request
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
rows = tk.MustQuery("explain analyze select /*+ set_var(tikv_client_read_timeout=1) */
 * from t where b > 1").Rows()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Len(t, rows, 3)
	explain = fmt.Sprintf("%v", rows[0])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Regexp(t, ".*TableReader.* root  time:.*, loops:.* cop_task: {num: 1, .*num_rpc:2.*", explain)

	// Test for stale read.
	if !kerneltype.IsNextGen() {
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
		tk.MustExec("set @a=now(6);")
		waitUntilReadTSSafe(tk, "@a")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
		tk.MustExec("set @@tidb_replica_read='closest-replicas';")
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
rows = tk.MustQuery("explain analyze select /*+ set_var(tikv_client_read_timeout=1) */
 * from t as of timestamp(@a) where b > 1").Rows()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.Len(t, rows, 3)
		explain = fmt.Sprintf("%v", rows[0])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.Regexp(t, ".*TableReader.* root  time:.*, loops:.* cop_task: {num: 1, .*num_rpc:2.*", explain)
	}

	// Test for tikv_client_read_timeout session variable.
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("set @@tikv_client_read_timeout=1;")
	// Test for point_get request
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
	rows = tk.MustQuery("explain analyze select * from t where a = 1").Rows()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Len(t, rows, 1)
	explain = fmt.Sprintf("%v", rows[0])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Regexp(t, ".*Point_Get.* Get:{num_rpc:2, total_time:.*", explain)

	// Test for batch_point_get request
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
	rows = tk.MustQuery("explain analyze select * from t where a in (1,2)").Rows()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Len(t, rows, 1)
	explain = fmt.Sprintf("%v", rows[0])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Regexp(t, ".*Batch_Point_Get.* BatchGet:{num_rpc:2, total_time:.*", explain)

	// Test for cop request
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
	rows = tk.MustQuery("explain analyze select * from t where b > 1").Rows()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Len(t, rows, 3)
	explain = fmt.Sprintf("%v", rows[0])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Regexp(t, ".*TableReader.* root  time:.*, loops:.* cop_task: {num: 1, .*num_rpc:2.*", explain)

	// Test for stale read.
	if !kerneltype.IsNextGen() {
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
		tk.MustExec("set @a=now(6);")
		waitUntilReadTSSafe(tk, "@a")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
		tk.MustExec("set @@tidb_replica_read='closest-replicas';")
    // MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
		rows = tk.MustQuery("explain analyze select * from t as of timestamp(@a) where b > 1").Rows()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.Len(t, rows, 3)
		explain = fmt.Sprintf("%v", rows[0])
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.Regexp(t, ".*TableReader.* root  time:.*, loops:.* cop_task: {num: 1, .*num_rpc:2.*", explain)
	}
}

// TestGetMvccByEncodedKeyRegionError 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestGetMvccByEncodedKeyRegionError(t *testing.T) {
#[test]
pub fn test_get_mvcc_by_encoded_key_region_error() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
	store := testkit.CreateMockStore(t)
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
	tk := testkit.NewTestKit(t, store)
	h := helper.NewHelper(store.(helper.Storage))
	txn, err := store.Begin()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.NoError(t, err)
	m := meta.NewMutator(txn)
	schemaVersion := tk.Session().GetLatestInfoSchema().SchemaMetaVersion()
	key := m.EncodeSchemaDiffKey(schemaVersion)

	resp, err := h.GetMvccByEncodedKey(key)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.NoError(t, err)
	require.NotNil(t, resp.Info)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Equal(t, 1, len(resp.Info.Writes))
	require.Less(t, uint64(0), resp.Info.Writes[0].CommitTs)
	commitTs := resp.Info.Writes[0].CommitTs

    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/mockstore/unistore/epochNotMatch", "2*return(true)"))
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
	defer func() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/mockstore/unistore/epochNotMatch"))
	}()
	resp, err = h.GetMvccByEncodedKey(key)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.NoError(t, err)
	require.NotNil(t, resp.Info)
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.Equal(t, 1, len(resp.Info.Writes))
	require.Equal(t, commitTs, resp.Info.Writes[0].CommitTs)
}

// TestShuffleExit 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestShuffleExit(t *testing.T) {
#[test]
pub fn test_shuffle_exit() {
    // 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
	store := testkit.CreateMockStore(t)
    // 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
	tk := testkit.NewTestKit(t, store)
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("use test")
	tk.MustExec("drop table if exists t1;")
    // MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("create table t1(i int, j int, k int);")
	tk.MustExec("insert into t1 VALUES (1,1,1),(2,2,2),(3,3,3),(4,4,4);")
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/shuffleError", "return(true)"))
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
	defer func() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/shuffleError"))
	}()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/shuffleExecFetchDataAndSplit", "return(true)"))
    // Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
	defer func() {
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/shuffleExecFetchDataAndSplit"))
	}()
    // require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/shuffleWorkerRun", "panic(\"ShufflePanic\")"))
// Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
	defer func() {
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/shuffleWorkerRun"))
	}()
	err := tk.QueryToErr("SELECT SUM(i) OVER W FROM t1 WINDOW w AS (PARTITION BY j ORDER BY i) ORDER BY 1+SUM(i) OVER w;")
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.ErrorContains(t, err, "ShuffleExec.Next error")
}

// TestHandleForeignKeyCascadePanic 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestHandleForeignKeyCascadePanic(t *testing.T) {
#[test]
pub fn test_handle_foreign_key_cascade_panic() {
// Test no goroutine leak.
// 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
	store := testkit.CreateMockStore(t)
// 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
	tk := testkit.NewTestKit(t, store)
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("use test")
	tk.MustExec("drop table if exists t1, t2;")
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("create table t1 (id int key, a int, index (a));")
	tk.MustExec("create table t2 (id int key, a int, index (a), constraint fk_1 foreign key (a) references t1(a));")
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("alter table t2 drop foreign key fk_1;")
	tk.MustExec("alter table t2 add constraint fk_1 foreign key (a) references t1(a) on delete set null;")
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("replace into t1 values (1, 1);")
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/handleForeignKeyCascadeError", "return(true)"))
// Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
	defer func() {
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/handleForeignKeyCascadeError"))
	}()
// Exec 返回 record set 或错误；保留错误处理和后续 record set 形状。
	err := tk.ExecToErr("replace into t1 values (1, 2);")
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.ErrorContains(t, err, "handleForeignKeyCascadeError")
}

// TestBuildProjectionForIndexJoinPanic 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestBuildProjectionForIndexJoinPanic(t *testing.T) {
#[test]
pub fn test_build_projection_for_index_join_panic() {
// Test no goroutine leak.
// 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
	store := testkit.CreateMockStore(t)
// 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
	tk := testkit.NewTestKit(t, store)
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("use test")
	tk.MustExec("drop table if exists t1, t2;")
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("create table t1(a int, b varchar(8));")
	tk.MustExec("insert into t1 values(1,'1');")
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("create table t2(a int , b varchar(8) GENERATED ALWAYS AS (c) VIRTUAL, c varchar(8), PRIMARY KEY (a));")
	tk.MustExec("insert into t2(a) values(1);")
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/buildProjectionForIndexJoinPanic", "return(true)"))
// Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
	defer func() {
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/buildProjectionForIndexJoinPanic"))
	}()
	err := tk.QueryToErr("select /*+ tidb_inlj(t2) */ t2.b, t1.b from t1 join t2 ON t2.a=t1.a;")
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.ErrorContains(t, err, "buildProjectionForIndexJoinPanic")
}

// IndexLookUpPushDownRunVerifier 对应 Go 的同名 struct，保留 mock/fixture 字段顺序。
// 字段类型仍按 Go 依赖名展示，当前不构造真实对象。
pub struct IndexLookUpPushDownRunVerifier {
	*testing.T
	tk          *testkit.TestKit
	tableName   string
	indexName   string
	primaryRows []int
	hitRate     any
	msg         string
}

// RunSelectWithCheckResult 对应 Go 的同名 struct，保留 mock/fixture 字段顺序。
// 字段类型仍按 Go 依赖名展示，当前不构造真实对象。
pub struct RunSelectWithCheckResult {
	SQL         string
	Rows        [][]any
	AnalyzeRows [][]any
}

// RunSelectWithCheck 对应 Go 的同名方法，保留 mock executor 或 PD client 的外部接口语义。
// Go 签名: func (t *IndexLookUpPushDownRunVerifier) RunSelectWithCheck(where string, skip, limit int) RunSelectWithCheckResult {
pub fn RunSelectWithCheck() {
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.NotNil(t, t.tk)
	require.NotEmpty(t, t.tableName)
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.NotEmpty(t, t.indexName)
	require.NotEmpty(t, t.primaryRows)
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.GreaterOrEqual(t, skip, 0)
	if skip > 0 {
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.GreaterOrEqual(t, limit, 0)
	}

	var hitRate int
	if r, ok := t.hitRate.(*rand.Rand); ok {
		hitRate = r.Intn(11)
	} else {
		hitRate, ok = t.hitRate.(int)
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.True(t, ok)
	}

	message := fmt.Sprintf("%s, hitRate: %d, where: %s, limit: %d", t.msg, hitRate, where, limit)
	injectHandleFilter := func(h kv.Handle) bool {
		if hitRate >= 10 {
			return true
		}
		h64a := fnv.New64a()
		_, err := h64a.Write(h.Encoded())
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.NoError(t, err)
		return h64a.Sum64()%10 < uint64(hitRate)
	}
// atomic 操作表示跨 goroutine 状态同步，迁移时需保持可见性语义。
	var injectCalled atomic.Bool
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.NoError(t, failpoint.EnableCall("github.com/pingcap/tidb/pkg/store/mockstore/unistore/cophandler/inject-index-lookup-handle-filter", func(f *func(kv.Handle) bool) {
		*f = injectHandleFilter
		injectCalled.Store(true)
	}))
// Go defer 负责恢复配置、关闭资源或释放 failpoint；这里保留收尾边界。
	defer func() {
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/mockstore/unistore/cophandler/inject-index-lookup-handle-filter"))
	}()
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("select /*+ index_lookup_pushdown(%s, %s)*/ * from %s where ", t.tableName, t.indexName, t.tableName))
	sb.WriteString(where)
	if skip > 0 {
		sb.WriteString(fmt.Sprintf(" limit %d, %d", skip, limit))
	} else if limit >= 0 {
		sb.WriteString(fmt.Sprintf(" limit %d", limit))
	}

// make sure the query uses index lookup
	analyzeSQL := "explain analyze " + sb.String()
	injectCalled.Store(false)
// MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
	analyzeResult := t.tk.MustQuery(analyzeSQL)
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.True(t, injectCalled.Load(), message)
	require.Contains(t, analyzeResult.String(), "LocalIndexLookUp", analyzeSQL+"\n"+analyzeResult.String())

// get actual result
	injectCalled.Store(false)
// MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
	rs := t.tk.MustQuery(sb.String())
	actual := rs.Rows()
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.True(t, injectCalled.Load(), message)
	idSets := make(map[string]struct{}, len(actual))
	for _, row := range actual {
		var primaryKey strings.Builder
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.Greater(t, len(t.primaryRows), 0)
		for i, idx := range t.primaryRows {
			if i > 0 {
				primaryKey.WriteString("#")
			}
			primaryKey.WriteString(row[idx].(string))
		}
		id := primaryKey.String()
		_, dup := idSets[id]
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.False(t, dup, "dupID: "+id+", "+message)
		idSets[row[0].(string)] = struct{}{}
	}

// use table scan
// MustQuery/Check 对应结果集断言，保留查询文本和期望结果方便后续核对。
	matchCondList := t.tk.MustQuery(fmt.Sprintf("select /*+ use_index(%s) */* from %s where "+where, t.tableName, t.tableName)).Rows()
	if limit == 0 || skip >= len(matchCondList) {
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.Len(t, actual, 0, message)
	} else if limit < 0 {
// no limit two results should have same members
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.ElementsMatch(t, matchCondList, actual, message)
	} else {
		expectRowCnt := limit
		if skip+limit > len(matchCondList) {
			expectRowCnt = len(matchCondList) - skip
		}
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
		require.Len(t, actual, expectRowCnt, message)
		require.Subset(t, matchCondList, actual, message)
	}

// check in analyze the index is lookup locally
	message = fmt.Sprintf("%s\n%s\n%s", message, analyzeSQL, analyzeResult.String())
	analyzeVerified := false
	localIndexLookUpIndex := -1
	totalIndexScanCnt := -1
	localIndexLookUpRowCnt := -1
	analyzeRows := analyzeResult.Rows()
	metTableRowIDScan := false
	for i, row := range analyzeRows {
		if strings.Contains(row[0].(string), "LocalIndexLookUp") {
			localIndexLookUpIndex = i
			continue
		}

		if strings.Contains(row[0].(string), "TableRowIDScan") && strings.Contains(row[3].(string), "cop[tikv]") {
			var err error
			if !metTableRowIDScan {
				localIndexLookUpRowCnt, err = strconv.Atoi(row[2].(string))
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
				require.NoError(t, err, message)
				require.GreaterOrEqual(t, localIndexLookUpRowCnt, 0)
				if hitRate == 0 {
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
					require.Zero(t, localIndexLookUpRowCnt, message)
				}
// check actRows for LocalIndexLookUp
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
				require.Equal(t, analyzeRows[localIndexLookUpIndex][2], row[2], message)
// get index scan row count
				totalIndexScanCnt, err = strconv.Atoi(analyzeRows[localIndexLookUpIndex+1][2].(string))
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
				require.NoError(t, err, message)
				require.GreaterOrEqual(t, totalIndexScanCnt, localIndexLookUpRowCnt)
				if hitRate >= 10 {
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
					require.Equal(t, localIndexLookUpRowCnt, totalIndexScanCnt)
				}
				metTableRowIDScan = true
				continue
			}

			tidbIndexLookUpRowCnt, err := strconv.Atoi(row[2].(string))
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
			require.NoError(t, err, message)
			if limit < 0 {
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
				require.Equal(t, totalIndexScanCnt, localIndexLookUpRowCnt+tidbIndexLookUpRowCnt, message)
			} else {
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
				require.LessOrEqual(t, localIndexLookUpRowCnt+tidbIndexLookUpRowCnt, totalIndexScanCnt, message)
			}
			analyzeVerified = true
			break
		}
	}
// require 断言保留 Go 测试的错误路径、相等性、长度或对象身份语义。
	require.True(t, analyzeVerified, analyzeResult.String())
	return RunSelectWithCheckResult{
		SQL:         sb.String(),
		Rows:        actual,
		AnalyzeRows: analyzeRows,
	}
}

// TestIndexLookUpPushDownExec 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestIndexLookUpPushDownExec(t *testing.T) {
#[test]
pub fn test_index_look_up_push_down_exec() {
// 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
	store := testkit.CreateMockStore(t)
// 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
	tk := testkit.NewTestKit(t, store)
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("use test")
	tk.MustExec("create table t(id bigint primary key, a bigint, b bigint, index a(a))")
	seed := time.Now().UnixNano()
	logutil.BgLogger().Info("Run TestIndexLookUpPushDownExec with seed", zap.Int64("seed", seed))
	r := rand.New(rand.NewSource(seed))
	v := &IndexLookUpPushDownRunVerifier{
		T:           t,
		tk:          tk,
		tableName:   "t",
		indexName:   "a",
		primaryRows: []int{0},
		hitRate:     r,
		msg:         fmt.Sprintf("seed: %d", seed),
	}

	batch := 100
	total := batch * 20
	indexValEnd := 100
	randIndexVal := func() int {
		return r.Intn(indexValEnd)
	}
	for i := 0; i < total; i += batch {
		values := make([]string, 0, batch)
		for j := 0; j < batch; j++ {
			values = append(values, fmt.Sprintf("(%d, %d, %d)", i+j, randIndexVal(), r.Int63()))
		}
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
		tk.MustExec("insert into t values " + strings.Join(values, ","))
	}

	v.RunSelectWithCheck("1", 0, -1)
	v.RunSelectWithCheck("1", 0, r.Intn(total*2))
	v.RunSelectWithCheck("1", total/2, r.Intn(total))
	v.RunSelectWithCheck("1", total-10, 20)
	v.RunSelectWithCheck("1", total, 10)
	v.RunSelectWithCheck("1", 10, 0)
	v.RunSelectWithCheck(fmt.Sprintf("a = %d", randIndexVal()), 0, -1)
	v.RunSelectWithCheck(fmt.Sprintf("a = %d", randIndexVal()), 0, 25)
	v.RunSelectWithCheck(fmt.Sprintf("a < %d", randIndexVal()), 0, -1)
	v.RunSelectWithCheck(fmt.Sprintf("a < %d", randIndexVal()), 0, r.Intn(100)+1)
	v.RunSelectWithCheck(fmt.Sprintf("a > %d", randIndexVal()), 0, -1)
	v.RunSelectWithCheck(fmt.Sprintf("a > %d", randIndexVal()), 0, r.Intn(100)+1)
	start := randIndexVal()
	v.RunSelectWithCheck(fmt.Sprintf("a >= %d and a < %d", start, start+r.Intn(5)+1), 0, -1)
	start = randIndexVal()
	v.RunSelectWithCheck(fmt.Sprintf("a >= %d and a < %d", start, start+r.Intn(5)+1), 0, r.Intn(50)+1)
	v.RunSelectWithCheck(fmt.Sprintf("a > %d and b < %d", randIndexVal(), r.Int63()), 0, -1)
	v.RunSelectWithCheck(fmt.Sprintf("a > %d and b < %d", randIndexVal(), r.Int63()), 0, r.Intn(50)+1)
}

// TestIndexLookUpPushDownPartitionExec 对应 Go 的同名测试，保留初始化、SQL、断言和清理顺序。
// Go testing.T/testkit/require/failpoint 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
// Go 签名: func TestIndexLookUpPushDownPartitionExec(t *testing.T) {
#[test]
pub fn test_index_look_up_push_down_partition_exec() {
// 创建 mock store 对应 Go 测试存储依赖；这里只保留测试环境创建顺序。
	store := testkit.CreateMockStore(t)
// 创建 TestKit 会话 harness；后续 SQL 调用只保留 Go 测试语义，不连接真实数据库。
	tk := testkit.NewTestKit(t, store)
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("use test")
// int handle
	tk.MustExec("create table tp1 (\n" +
		"    a varchar(32),\n" +
		"    b int,\n" +
		"    c int,\n" +
		"    d int,\n" +
		"    primary key(b) CLUSTERED,\n" +
		"    index c(c)\n" +
		")\n" +
		"PARTITION BY RANGE (b) (\n" +
		"    PARTITION p0 VALUES LESS THAN (100),\n" +
		"    PARTITION p1 VALUES LESS THAN (200),\n" +
		"    PARTITION p2 VALUES LESS THAN (300),\n" +
		"    PARTITION p3 VALUES LESS THAN MAXVALUE\n" +
		")")

// common handle
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("create table tp2 (\n" +
		"    a varchar(32),\n" +
		"    b int,\n" +
		"    c int,\n" +
		"    d int,\n" +
		"    primary key(a, b) CLUSTERED,\n" +
		"    index c(c)\n" +
		")\n" +
		"PARTITION BY RANGE COLUMNS (a) (\n" +
		"    PARTITION p0 VALUES LESS THAN ('c'),\n" +
		"    PARTITION p1 VALUES LESS THAN ('e'),\n" +
		"    PARTITION p2 VALUES LESS THAN ('g'),\n" +
		"    PARTITION p3 VALUES LESS THAN MAXVALUE\n" +
		")")

// extra handle
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
	tk.MustExec("create table tp3 (\n" +
		"    a varchar(32),\n" +
		"    b int,\n" +
		"    c int,\n" +
		"    d int,\n" +
		"    primary key(a, b) NONCLUSTERED,\n" +
		"    index c(c)\n" +
		")\n" +
		"PARTITION BY RANGE COLUMNS (a) (\n" +
		"    PARTITION p0 VALUES LESS THAN ('c'),\n" +
		"    PARTITION p1 VALUES LESS THAN ('e'),\n" +
		"    PARTITION p2 VALUES LESS THAN ('g'),\n" +
		"    PARTITION p3 VALUES LESS THAN MAXVALUE\n" +
		")")

	tableNames := []string{"tp1", "tp2", "tp3"}
// prepare data
	for _, tableName := range tableNames {
// MustExec 对应必须成功的 SQL 执行断言，保留原 SQL 文本和执行顺序。
		tk.MustExec("insert into " + tableName + " values " +
			"('a', 10, 1, 100), " +
			"('b', 20, 2, 200), " +
			"('c', 110, 3, 300), " +
			"('d', 120, 4, 400), " +
			"('e', 210, 5, 500), " +
			"('f', 220, 6, 600), " +
			"('g', 330, 5, 700), " +
			"('h', 340, 5, 800), " +
			"('i', 450, 5, 900), " +
			"('j', 550, 6, 1000) ",
		)

		v := &IndexLookUpPushDownRunVerifier{
			T:           t,
			tk:          tk,
			tableName:   tableName,
			indexName:   "c",
			primaryRows: []int{0, 1},
			msg:         tableName,
		}

		if tableName == "tp1" {
			v.primaryRows = []int{1}
		}

		for _, hitRate := range []int{0, 5, 10} {
			v.hitRate = hitRate
			v.RunSelectWithCheck("1", 0, -1)
		}
	}
}
*/

// ----- 以下为已接线的 Rust failpoint 测试 -----

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use astersql_store_copr::{
    Backoffer, BatchError, BatchRequest, BatchResult, CancellationToken, RegionBatchRequestSender,
    RegionFailureHandler, RegionInfo, RegionStore, RpcClient, RpcContext, RpcResponse,
};

#[test]
/// PointGet 可重复读路径：step1/step2 两个 failpoint 必须在二次读边界各触发一次。
fn point_get_failpoints_are_consumed_at_the_canonical_second_read_boundary() {
    let calls = Arc::new(AtomicUsize::new(0));
    let step_one = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/executor/pointGetRepeatableReadTest-step1",
        {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::AcqRel);
            }
        },
    );
    let step_two = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/executor/pointGetRepeatableReadTest-step2",
        {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::AcqRel);
            }
        },
    );
    // 触发生产路径上的两个 step failpoint。
    crate::point_get::point_get_repeatable_read_failpoint();
    assert_eq!(calls.load(Ordering::Acquire), 2);
    drop((step_one, step_two));
}

#[test]
/// BatchPointGet：索引快照读后的 step1/step2 failpoint 各消费一次。
fn batch_point_get_failpoints_are_consumed_after_index_snapshot_read() {
    let calls = Arc::new(AtomicUsize::new(0));
    let step_one = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/executor/batchPointGetRepeatableReadTest-step1",
        {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::AcqRel);
            }
        },
    );
    let step_two = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/executor/batchPointGetRepeatableReadTest-step2",
        {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::AcqRel);
            }
        },
    );
    // 触发 BatchPointGet 可重复读边界上的两个 step failpoint。
    crate::batch_point_get::batch_point_get_repeatable_read_failpoint();
    assert_eq!(calls.load(Ordering::Acquire), 2);
    drop((step_one, step_two));
}

#[test]
/// Shuffle worker 在可恢复边界内 panic，外层 catch_unwind 应捕获。
fn shuffle_worker_failpoint_panics_inside_the_recovered_worker_boundary() {
    let guard = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/shuffleWorkerRun",
        "panic(shuffle-worker)",
    );
    // failpoint 配置为 panic(shuffle-worker)，应在 worker 恢复边界内爆发。
    let panic = std::panic::catch_unwind(crate::shuffle::shuffle_worker_failpoint);
    assert!(panic.is_err());
    drop(guard);
}

/// 故意 panic 的 RPC 客户端：注入响应必须绕过真实传输层。
struct UnexpectedRpcClient;

impl RpcClient for UnexpectedRpcClient {
    fn send_request(
        &self,
        _address: &str,
        _request: &BatchRequest,
        _timeout: Duration,
        _cancellation: &CancellationToken,
    ) -> BatchResult<RpcResponse> {
        panic!("injected BatchCop response must bypass the transport")
    }
}

#[derive(Default)]
/// 记录 BatchCop 发送失败回调次数，用于断言重试路径。
struct FailureRecorder {
    calls: AtomicUsize,
}

impl RegionFailureHandler for FailureRecorder {
    /// 断言 reload_region 且错误为 OtherResponse，并累加调用计数。
    fn on_send_fail_for_batch_regions(
        &self,
        _store: Option<&RegionStore>,
        _regions: &[RegionInfo],
        reload_region: bool,
        error: &BatchError,
    ) {
        assert!(reload_region);
        assert!(matches!(error, BatchError::OtherResponse(_)));
        self.calls.fetch_add(1, Ordering::AcqRel);
    }
}

#[test]
/// mockBatchCopResponseError 注入后，sender 走真实重试并回调 FailureRecorder。
fn batch_cop_response_failpoint_uses_the_real_sender_retry_path() {
    let guard = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/store/copr/mockBatchCopResponseError",
        "return(true)",
    );
    let recorder = Arc::new(FailureRecorder::default());
    let mut sender = RegionBatchRequestSender::new(
        recorder.clone(),
        Arc::new(UnexpectedRpcClient),
        true,
        false,
        Arc::new(AtomicBool::new(false)),
    );
    let mut request = BatchRequest::default();
    let mut backoff = Backoffer::new(1);
    // UnexpectedRpcClient 不应被调用；响应由 failpoint 注入。
    let result = sender.send_req_to_addr(
        &mut backoff,
        &RpcContext {
            address: "tiflash0".to_owned(),
            ..Default::default()
        },
        &[RegionInfo::default()],
        &mut request,
        Duration::from_secs(1),
    );
    assert!(result.retry);
    assert!(result.response.is_none());
    assert_eq!(recorder.calls.load(Ordering::Acquire), 1);
    assert!(matches!(
        sender.last_rpc_error,
        Some(BatchError::OtherResponse(_))
    ));
    drop(guard);
}

#[test]
/// unistoreRPCDeadlineExceeded 注入后返回真实 Deadline is exceeded RPC 错误。
fn unistore_deadline_failpoint_returns_the_real_rpc_error() {
    use astersql_store_mockstore_unistore::{Request, RpcError};

    let (client, _, _) =
        astersql_store_mockstore_unistore::New("", Vec::new(), 0, Vec::new()).unwrap();
    let guard = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/store/mockstore/unistore/unistoreRPCDeadlineExceeded",
        "return(true)",
    );
    let error = match client.send_request("unused", Request::Empty, Duration::from_millis(10)) {
        Err(error) => error,
        Ok(_) => panic!("deadline failpoint unexpectedly returned a response"),
    };
    assert_eq!(error, RpcError::Server("Deadline is exceeded".to_owned()));
    drop(guard);
    client.close().unwrap();
}
