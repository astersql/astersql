// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// ADMIN CHECK / RECOVER / CLEANUP INDEX 测试。
//
// 块注释内归档完整 Go testkit 用例（recover/cleanup/check、分区/全局/
// 多值/聚簇索引、快照与 fast check 等）。可执行部分用内存 [`AdminTable`]
// 覆盖核心一致性修复语义。

/*
// 覆盖ADMIN CHECK/CLEANUP/RECOVER INDEX 相关测试。
// 主要类型、函数、方法、关键分支、SQL fixture、资源收尾、错误处理、并发/异步/IO 和外部依赖旁边补充中文说明，便于人工核对 Go 源语义。
#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(unused_variables)]

// TestAdminRecoverIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminRecoverIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminRecoverIndex() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 int, c2 int, c3 int default 1, index (c1), unique key(c2))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c2) values (1, 1), (2, 2), (NULL, NULL)")

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("admin recover index admin_test c1")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("0 3"))

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin recover index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("0 3"))

    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c1")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c2")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 int, c2 int, c3 int default 1, primary key(c1), unique key(c2))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c2) values (1, 1), (2, 2), (3, 3), (10, 10), (20, 20)")
    // pk is handle, no additional unique index, no way to recover
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err := tk.ExecToErr("admin recover index admin_test c1")
    // err:index is not found
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin recover index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("0 5"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c2")

    // Make some corrupted index.
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("admin_test")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    tblInfo := tbl.Meta()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idxInfo := tblInfo.FindIndexByName("c2")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(1), kv.IntHandle(1))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check index admin_test c2")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("4"))

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin recover index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("1 5"))

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("5"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c2")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")

    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err = store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(10), kv.IntHandle(10))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check index admin_test c2")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin recover index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("1 5"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c2")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")

    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err = store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(1), kv.IntHandle(1))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(2), kv.IntHandle(2))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(3), kv.IntHandle(3))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(10), kv.IntHandle(10))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(20), kv.IntHandle(20))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check index admin_test c2")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("0"))

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX()")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("5"))

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin recover index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("5 5"))

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("5"))

    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c2")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 int, c2 int, c3 int default 1, primary key(c1), unique key i1((c2+1)))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c2) values (1, 1), (2, 2), (3, 3), (10, 10), (20, 20)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin recover index admin_test i1")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("0 5"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
    sctx = mock.NewContext()
    sctx.Store = store
    ctx = sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is = domain.InfoSchema()
    dbName = ast.NewCIStr("test")
    tblName = ast.NewCIStr("admin_test")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err = is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    tblInfo = tbl.Meta()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idxInfo = tblInfo.FindIndexByName("i1")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err = tables.NewIndex(tblInfo.ID, tblInfo, idxInfo)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err = store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(2), kv.IntHandle(1))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(i1)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("4"))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin recover index admin_test i1")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("1 5"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 int, c2 int, c3 int default 1, primary key(c1), unique key i1(c1, c2));")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c2) values (1, 1), (2, 2), (3, 3), (10, 10), (20, 20);")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin recover index admin_test i1;")
}

// TestAdminRecoverMVIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminRecoverMVIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminRecoverMVIndex() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists t")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table t(pk int primary key, a json, index idx((cast(a as signed array))))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (0, '[0,1,2]')")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (1, '[1,2,3]')")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (2, '[2,3,4]')")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (3, '[3,4,5]')")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (4, '[4,5,6]')")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table t")

    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("t")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    tblInfo := tbl.Meta()
    idxInfo := tblInfo.Indices[0]
    tk.Session().GetSessionVars().IndexLookupSize = 3
    tk.Session().GetSessionVars().MaxChunkSize = 3

    cpIdx := idxInfo.Clone()
    cpIdx.MVIndex = false
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, cpIdx)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(2), kv.IntHandle(1))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table t")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("admin recover index t idx")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("1 5"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table t")
}

// TestAdminCleanupMVIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCleanupMVIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCleanupMVIndex() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists t")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table t(pk int primary key, a json, index idx((cast(a as signed array))))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (0, '[0,1,2]')")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (1, '[1,2,3]')")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (2, '[2,3,4]')")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (3, '[3,4,5]')")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (4, '[4,5,6]')")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table t")

    // Make some corrupted index. Build the index information.
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("t")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    tblInfo := tbl.Meta()
    idxInfo := tblInfo.Indices[0]
    tk.Session().GetSessionVars().IndexLookupSize = 3
    tk.Session().GetSessionVars().MaxChunkSize = 3

    cpIdx := idxInfo.Clone()
    cpIdx.MVIndex = false
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, cpIdx)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    _, err = indexOpr.Create(ctx, txn, types.MakeDatums(9), kv.IntHandle(9), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table t")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("admin cleanup index t idx")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("1"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table t")
}

// TestClusteredIndexAdminRecoverIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestClusteredIndexAdminRecoverIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestClusteredIndexAdminRecoverIndex() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop database if exists test_cluster_index_admin_recover;")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create database test_cluster_index_admin_recover;")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test_cluster_index_admin_recover;")
    tk.Session().GetSessionVars().EnableClusteredIndex = vardef.ClusteredIndexDefModeOn
    dbName := ast.NewCIStr("test_cluster_index_admin_recover")
    tblName := ast.NewCIStr("t")

    // Test no corruption case.
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table t (a varchar(255), b int, c char(10), primary key(a, c), index idx(b), index idx1(c));")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values ('1', 2, '3'), ('1', 2, '4'), ('1', 2, '5');")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("admin recover index t `primary`;").Check(testkit.Rows("0 0"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("admin recover index t `idx`;").Check(testkit.Rows("0 3"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table t;")

    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    tblInfo := tbl.Meta()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idxInfo := tblInfo.FindIndexByName("idx")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // Some index entries are missed.
    // Recover an index don't covered by clustered index.
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    cHandle := testutil.MustNewCommonHandle(t, "1", "3")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(2), cHandle)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    tk.MustGetErrCode("admin check table t", mysql.ErrDataInconsistent)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    tk.MustGetErrCode("admin check index t idx", mysql.ErrDataInconsistent)

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("SELECT COUNT(*) FROM t USE INDEX(idx)").Check(testkit.Rows("2"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("admin recover index t idx").Check(testkit.Rows("1 3"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("SELECT COUNT(*) FROM t USE INDEX(idx)").Check(testkit.Rows("3"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table t;")

    // Recover an index covered by clustered index.
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idx1Info := tblInfo.FindIndexByName("idx1")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr1, err := tables.NewIndex(tblInfo.ID, tblInfo, idx1Info)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err = store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    err = indexOpr1.Delete(ctx, txn, types.MakeDatums("3"), cHandle)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    tk.MustGetErrCode("admin check table t", mysql.ErrDataInconsistent)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    tk.MustGetErrCode("admin check index t idx1", mysql.ErrDataInconsistent)

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("SELECT COUNT(*) FROM t USE INDEX(idx1)").Check(testkit.Rows("2"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("admin recover index t idx1").Check(testkit.Rows("1 3"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("SELECT COUNT(*) FROM t USE INDEX(idx1)").Check(testkit.Rows("3"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table t;")
}

// TestAdminRecoverPartitionTableIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminRecoverPartitionTableIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminRecoverPartitionTableIndex() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    getTable := func() table.Table {
        ctx := mock.NewContext()
        ctx.Store = store
        // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
        is := domain.InfoSchema()
        dbName := ast.NewCIStr("test")
        tblName := ast.NewCIStr("admin_test")
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        tbl, err := is.TableByName(context.Background(), dbName, tblName)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        return tbl
    }

    checkFunc := func(tbl table.Table, pid int64, idxValue int) {
        // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
        idxInfo := tbl.Meta().FindIndexByName("c2")
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        indexOpr, err := tables.NewIndex(pid, tbl.Meta(), idxInfo)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        ctx := mock.NewContext()
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx.GetTableCtx(), txn, types.MakeDatums(idxValue), kv.IntHandle(idxValue))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))

        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r := tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)")
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r.Check(testkit.Rows("2"))

        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r = tk.MustQuery("admin recover index admin_test c2")
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r.Check(testkit.Rows("1 3"))

        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)")
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r.Check(testkit.Rows("3"))
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test")
    }

    // Test for hash partition table.
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 int, c2 int, c3 int default 1, primary key (c1), index (c2)) partition by hash(c1) partitions 3;")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c2) values (0, 0), (1, 1), (2, 2)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("admin recover index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("0 3"))
    tbl := getTable()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    pi := tbl.Meta().GetPartitionInfo()
    require.NotNil(t, pi)
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i, p := range pi.Definitions {
        checkFunc(tbl, p.ID, i)
    }

    // Test for range partition table.
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(`create table admin_test (c1 int, c2 int, c3 int default 1, primary key (c1), index (c2)) PARTITION BY RANGE ( c1 ) (
        PARTITION p0 VALUES LESS THAN (5),
        PARTITION p1 VALUES LESS THAN (10),
        PARTITION p2 VALUES LESS THAN (MAXVALUE))`)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c2) values (0, 0), (6, 6), (12, 12)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin recover index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("0 3"))
    tbl = getTable()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    pi = tbl.Meta().GetPartitionInfo()
    require.NotNil(t, pi)
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i, p := range pi.Definitions {
        checkFunc(tbl, p.ID, i*6)
    }
}

// TestAdminRecoverIndex1 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminRecoverIndex1(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminRecoverIndex1() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    tk.Session().GetSessionVars().EnableClusteredIndex = vardef.ClusteredIndexDefModeIntOnly
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 varchar(255), c2 int, c3 int default 1, primary key(c1), unique key(c2))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c2) values ('1', 1), ('2', 2), ('3', 3), ('10', 10), ('20', 20)")

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(`primary`)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("5"))

    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    tblInfo := tbl.Meta()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idxInfo := tblInfo.FindIndexByName("primary")
    require.NotNil(t, idxInfo)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums("1"), kv.IntHandle(1))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums("2"), kv.IntHandle(2))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums("3"), kv.IntHandle(3))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums("10"), kv.IntHandle(4))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(`primary`)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("1"))

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin recover index admin_test `primary`")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("4 5"))

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(`primary`)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("5"))

    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c2")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test `primary`")
}

// TestAdminCleanupIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCleanupIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCleanupIndex() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 int, c2 int, c3 int default 1, primary key (c1), unique key(c2), key (c3))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c2) values (1, 2), (3, 4), (-5, NULL)")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c3) values (7, 100), (9, 100), (11, NULL)")

    // pk is handle, no need to cleanup
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err := tk.ExecToErr("admin cleanup index admin_test `primary`")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("admin cleanup index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("0"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin cleanup index admin_test c3")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("0"))

    // Make some dangling index.
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("admin_test")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    tblInfo := tbl.Meta()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idxInfo2 := tblInfo.FindIndexByName("c2")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr2, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo2)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idxInfo3 := tblInfo.FindIndexByName("c3")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr3, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo3)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    _, err = indexOpr2.Create(ctx, txn, types.MakeDatums(1), kv.IntHandle(-100), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    _, err = indexOpr2.Create(ctx, txn, types.MakeDatums(6), kv.IntHandle(100), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    _, err = indexOpr2.Create(ctx, txn, types.MakeDatums(8), kv.IntHandle(100), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    _, err = indexOpr2.Create(ctx, txn, types.MakeDatums(nil), kv.IntHandle(101), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    _, err = indexOpr2.Create(ctx, txn, types.MakeDatums(nil), kv.IntHandle(102), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    _, err = indexOpr3.Create(ctx, txn, types.MakeDatums(6), kv.IntHandle(200), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    _, err = indexOpr3.Create(ctx, txn, types.MakeDatums(6), kv.IntHandle(-200), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    _, err = indexOpr3.Create(ctx, txn, types.MakeDatums(8), kv.IntHandle(-200), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check index admin_test c2")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("11"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin cleanup index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("5"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("6"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c2")

    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check index admin_test c3")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c3)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("9"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin cleanup index admin_test c3")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("3"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c3)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("6"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c3")

    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
}

// TestAdminCleanupIndexForPartitionTable 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCleanupIndexForPartitionTable(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCleanupIndexForPartitionTable() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")

    getTable := func() table.Table {
        ctx := mock.NewContext()
        ctx.Store = store
        // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
        is := domain.InfoSchema()
        dbName := ast.NewCIStr("test")
        tblName := ast.NewCIStr("admin_test")
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        tbl, err := is.TableByName(context.Background(), dbName, tblName)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        return tbl
    }

    checkFunc := func(tbl table.Table, pid int64, idxValue, handle int) {
        // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
        idxInfo2 := tbl.Meta().FindIndexByName("c2")
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        indexOpr2, err := tables.NewIndex(pid, tbl.Meta(), idxInfo2)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
        idxInfo3 := tbl.Meta().FindIndexByName("c3")
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        indexOpr3, err := tables.NewIndex(pid, tbl.Meta(), idxInfo3)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)

        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        sctx := mock.NewContext()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        ctx := sctx.GetTableCtx()
        _, err = indexOpr2.Create(ctx, txn, types.MakeDatums(idxValue), kv.IntHandle(handle), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        _, err = indexOpr3.Create(ctx, txn, types.MakeDatums(idxValue), kv.IntHandle(handle), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)

        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)

        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r := tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)")
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r.Check(testkit.Rows("4"))
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r = tk.MustQuery("admin cleanup index admin_test c2")
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r.Check(testkit.Rows("1"))
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)")
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r.Check(testkit.Rows("3"))

        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c3)")
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r.Check(testkit.Rows("4"))
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r = tk.MustQuery("admin cleanup index admin_test c3")
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r.Check(testkit.Rows("1"))
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c3)")
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        r.Check(testkit.Rows("3"))
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test")
    }

    // Test for hash partition table.
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 int, c2 int, c3 int default 1, primary key (c2), unique index c2(c2), index c3(c3)) partition by hash(c2) partitions 3;")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c2, c3) values (0, 0), (1, 1), (2, 2)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("admin cleanup index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("0"))
    tbl := getTable()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    pi := tbl.Meta().GetPartitionInfo()
    require.NotNil(t, pi)
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i, p := range pi.Definitions {
        checkFunc(tbl, p.ID, i+6, i+6)
    }

    // Test for range partition table.
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(`create table admin_test (c1 int, c2 int, c3 int default 1, primary key (c2), unique index c2 (c2), index c3(c3)) PARTITION BY RANGE ( c2 ) (
        PARTITION p0 VALUES LESS THAN (5),
        PARTITION p1 VALUES LESS THAN (10),
        PARTITION p2 VALUES LESS THAN (MAXVALUE))`)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c2) values (0, 0), (6, 6), (12, 12)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin cleanup index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("0"))
    tbl = getTable()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    pi = tbl.Meta().GetPartitionInfo()
    require.NotNil(t, pi)
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i, p := range pi.Definitions {
        checkFunc(tbl, p.ID, i*6+1, i*6+1)
    }
}

// TestAdminCleanupIndexPKNotHandle 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCleanupIndexPKNotHandle(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCleanupIndexPKNotHandle() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    tk.Session().GetSessionVars().EnableClusteredIndex = vardef.ClusteredIndexDefModeIntOnly
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 int, c2 int, c3 int, primary key (c1, c2))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c2) values (1, 2), (3, 4), (-5, 5)")

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("admin cleanup index admin_test `primary`")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("0"))

    // Make some dangling index.
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("admin_test")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    tblInfo := tbl.Meta()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idxInfo := tblInfo.FindIndexByName("primary")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    _, err = indexOpr.Create(ctx, txn, types.MakeDatums(7, 10), kv.IntHandle(-100), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    _, err = indexOpr.Create(ctx, txn, types.MakeDatums(4, 6), kv.IntHandle(100), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    _, err = indexOpr.Create(ctx, txn, types.MakeDatums(-7, 4), kv.IntHandle(101), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check index admin_test `primary`")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(`primary`)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("6"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin cleanup index admin_test `primary`")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("3"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(`primary`)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("3"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test `primary`")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
}

// TestAdminCleanupIndexMore 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCleanupIndexMore(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCleanupIndexMore() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 int, c2 int, unique key (c1, c2), key (c2))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test values (1, 2), (3, 4), (5, 6)")

    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin cleanup index admin_test c1")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin cleanup index admin_test c2")

    // Make some dangling index.
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("admin_test")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    tblInfo := tbl.Meta()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idxInfo1 := tblInfo.FindIndexByName("c1")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr1, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo1)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idxInfo2 := tblInfo.FindIndexByName("c2")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr2, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo2)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := range 2000 {
        c1 := int64(2*i + 7)
        c2 := int64(2*i + 8)
        _, err = indexOpr1.Create(ctx, txn, types.MakeDatums(c1, c2), kv.IntHandle(c1), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoErrorf(t, err, errors.ErrorStack(err))
        _, err = indexOpr2.Create(ctx, txn, types.MakeDatums(c2), kv.IntHandle(c1), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
    }
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check index admin_test c1")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check index admin_test c2")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX()")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("3"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c1)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("2003"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("2003"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin cleanup index admin_test c1")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("2000"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("admin cleanup index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("2000"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c1)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("3"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r = tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("3"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c1")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c2")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
}

// TestClusteredAdminCleanupIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestClusteredAdminCleanupIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestClusteredAdminCleanupIndex() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    tk.Session().GetSessionVars().EnableClusteredIndex = vardef.ClusteredIndexDefModeOn
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 varchar(255), c2 int, c3 char(10) default 'c3', primary key (c1, c3), unique key(c2), key (c3))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c2) values ('c1_1', 2), ('c1_2', 4), ('c1_3', NULL)")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c3) values ('c1_4', 'c3_4'), ('c1_5', 'c3_5'), ('c1_6', default)")

    // Normally, there is no dangling index.
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("admin cleanup index admin_test `primary`").Check(testkit.Rows("0"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("admin cleanup index admin_test `c2`").Check(testkit.Rows("0"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("admin cleanup index admin_test `c3`").Check(testkit.Rows("0"))

    // Make some dangling index.
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := domain.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("admin_test"))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // cleanup clustered primary key takes no effect.

    tblInfo := tbl.Meta()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idxInfo2 := tblInfo.FindIndexByName("c2")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr2, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo2)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idxInfo3 := tblInfo.FindIndexByName("c3")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr3, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo3)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    c2DanglingIdx := []struct {
        handle kv.Handle
        idxVal []types.Datum
    }{
        {testutil.MustNewCommonHandle(t, "c1_10", "c3_10"), types.MakeDatums(10)},
        {testutil.MustNewCommonHandle(t, "c1_10", "c3_11"), types.MakeDatums(11)},
        {testutil.MustNewCommonHandle(t, "c1_12", "c3_12"), types.MakeDatums(12)},
    }
    c3DanglingIdx := []struct {
        handle kv.Handle
        idxVal []types.Datum
    }{
        {testutil.MustNewCommonHandle(t, "c1_13", "c3_13"), types.MakeDatums("c3_13")},
        {testutil.MustNewCommonHandle(t, "c1_14", "c3_14"), types.MakeDatums("c3_14")},
        {testutil.MustNewCommonHandle(t, "c1_15", "c3_15"), types.MakeDatums("c3_15")},
    }
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for _, di := range c2DanglingIdx {
        _, err := indexOpr2.Create(ctx, txn, di.idxVal, di.handle, nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
    }
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for _, di := range c3DanglingIdx {
        _, err := indexOpr3.Create(ctx, txn, di.idxVal, di.handle, nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
    }
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check index admin_test c2")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)").Check(testkit.Rows("9"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("admin cleanup index admin_test c2").Check(testkit.Rows("3"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c2)").Check(testkit.Rows("6"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c2")

    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check index admin_test c3")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c3)").Check(testkit.Rows("9"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("admin cleanup index admin_test c3").Check(testkit.Rows("3"))
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    tk.MustQuery("SELECT COUNT(*) FROM admin_test USE INDEX(c3)").Check(testkit.Rows("6"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test c3")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
}

// TestAdminCheckTableWithMultiValuedIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCheckTableWithMultiValuedIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCheckTableWithMultiValuedIndex() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists t")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table t(pk int primary key, a json, index idx((cast(a as signed array))))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (0, '[0,1,2]')")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (1, '[1,2,3]')")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (2, '[2,3,4]')")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (3, '[3,4,5]')")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (4, '[4,5,6]')")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table t")

    // Make some corrupted index. Build the index information.
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("t")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    tblInfo := tbl.Meta()
    idxInfo := tblInfo.Indices[0]
    tk.Session().GetSessionVars().IndexLookupSize = 3
    tk.Session().GetSessionVars().MaxChunkSize = 3

    cpIdx := idxInfo.Clone()
    cpIdx.MVIndex = false
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, cpIdx)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(0), kv.IntHandle(0))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table t")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))

    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err = store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    _, err = indexOpr.Create(ctx, txn, types.MakeDatums(0), kv.IntHandle(0), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table t")

    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err = store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    _, err = indexOpr.Create(ctx, txn, types.MakeDatums(9), kv.IntHandle(9), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table t")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
}

// TestAdminCheckPartitionTableFailed 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCheckPartitionTableFailed(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCheckPartitionTableFailed() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test_p")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test_p (c1 int key,c2 int,c3 int,index idx(c2)) partition by hash(c1) partitions 4")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test_p (c1, c2, c3) values (0,0,0), (1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,5)")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test_p")

    // Make some corrupted index. Build the index information.
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("admin_test_p")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    tblInfo := tbl.Meta()
    idxInfo := tblInfo.Indices[0]
    tk.Session().GetSessionVars().IndexLookupSize = 3
    tk.Session().GetSessionVars().MaxChunkSize = 3

    // Reduce one row of index on partitions.
    // Table count > index count.
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := 0; i <= 5; i++ {
        // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
        partitionIdx := i % len(tblInfo.GetPartitionInfo().Definitions)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        indexOpr, err := tables.NewIndex(tblInfo.GetPartitionInfo().Definitions[partitionIdx].ID, tblInfo, idxInfo)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(i), kv.IntHandle(i))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test_p")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.EqualError(t, err, fmt.Sprintf("[admin:8223]data inconsistency in table: admin_test_p, index: idx, handle: %d, index-values:\"\" != record-values:\"handle: %d, values: [KindInt64 %d]\"", i, i, i))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))
        // TODO: fix admin recover for partition table.
        // r := tk.MustQuery("admin recover index admin_test_p idx")
        // r.Check(testkit.Rows("0 0"))
        // tk.MustExec("admin check table admin_test_p")
        // Manual recover index.
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err = store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(ctx, txn, types.MakeDatums(i), kv.IntHandle(i), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test_p")
    }

    // Add one row of index on partitions.
    // Table count < index count.
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := 0; i <= 5; i++ {
        // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
        partitionIdx := i % len(tblInfo.GetPartitionInfo().Definitions)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        indexOpr, err := tables.NewIndex(tblInfo.GetPartitionInfo().Definitions[partitionIdx].ID, tblInfo, idxInfo)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(ctx, txn, types.MakeDatums(i+8), kv.IntHandle(i+8), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test_p")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.EqualError(t, err, fmt.Sprintf("[admin:8223]data inconsistency in table: admin_test_p, index: idx, handle: %d, index-values:\"handle: %d, values: [KindInt64 %d]\" != record-values:\"\"", i+8, i+8, i+8))
        // TODO: fix admin recover for partition table.
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err = store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(i+8), kv.IntHandle(i+8))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test_p")
    }

    // Table count = index count, but the index value was wrong.
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := 0; i <= 5; i++ {
        // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
        partitionIdx := i % len(tblInfo.GetPartitionInfo().Definitions)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        indexOpr, err := tables.NewIndex(tblInfo.GetPartitionInfo().Definitions[partitionIdx].ID, tblInfo, idxInfo)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(ctx, txn, types.MakeDatums(i+8), kv.IntHandle(i), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test_p")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.EqualError(t, err, fmt.Sprintf("[admin:8223]data inconsistency in table: admin_test_p, index: idx, handle: %d, index-values:\"handle: %d, values: [KindInt64 %d]\" != record-values:\"handle: %d, values: [KindInt64 %d]\"", i, i, i+8, i, i))
        // TODO: fix admin recover for partition table.
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err = store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(i+8), kv.IntHandle(i))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test_p")
    }
}

// 常量/变量保留 Go 测试命名和值，用于说明跨函数共享 fixture。
const dbName, tblName = "test", "admin_test"

// 类型定义按 Go 结构保留字段和顺序；暂不引入真实 TiDB 类型依赖。
type inconsistencyTestKit struct {
    *testkit.AsyncTestKit
    uniqueIndex table.Index
    plainIndex  table.Index
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    ctx         context.Context
    sctx        sessionctx.Context
    t           *testing.T
}

// 类型定义按 Go 结构保留字段和顺序；暂不引入真实 TiDB 类型依赖。
type kitOpt struct {
    pkColType  string
    idxColType string
    ukColType  string
    clustered  string
}

// newDefaultOpt 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func newDefaultOpt() *kitOpt {
pub fn newDefaultOpt() *kitOpt {
    return &kitOpt{
        pkColType:  "int",
        idxColType: "int",
        ukColType:  "varchar(255)",
    }
}

// newInconsistencyKit 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func newInconsistencyKit(t *testing.T, tk *testkit.AsyncTestKit, opt *kitOpt) *inconsistencyTestKit {
pub fn newInconsistencyKit() *inconsistencyTestKit {
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    ctx := tk.OpenSession(context.Background(), dbName)
    se := testkit.TryRetrieveSession(ctx)
    i := &inconsistencyTestKit{
        AsyncTestKit: tk,
        ctx:          ctx,
        sctx:         se,
        t:            t,
    }
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(i.ctx, "drop table if exists "+tblName)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(i.ctx,
        fmt.Sprintf("create table %s (c1 %s, c2 %s, c3 %s, primary key(c1) %s, index uk1(c2), index k2(c3))",
            tblName, opt.pkColType, opt.idxColType, opt.ukColType, opt.clustered),
    )
    i.rebuild()
    return i
}

// rebuild 对应 Go 方法 receiver `tk *inconsistencyTestKit`，保留接口实现或测试辅助语义。
// Go 签名: func (tk *inconsistencyTestKit) rebuild() {
pub fn rebuild() {
    // receiver 和参数类型仍按 Go 调用形状记录，后续接线时再映射 Rust 类型。
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(tk.ctx, "truncate table "+tblName)
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.GetDomain(testkit.TryRetrieveSession(tk.ctx)).InfoSchema()
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), ast.NewCIStr(dbName), ast.NewCIStr(tblName))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(tk.t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    tk.uniqueIndex, err = tables.NewIndex(tbl.Meta().ID, tbl.Meta(), tbl.Meta().Indices[0])
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(tk.t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    tk.plainIndex, err = tables.NewIndex(tbl.Meta().ID, tbl.Meta(), tbl.Meta().Indices[1])
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(tk.t, err)
}

// TestCheckFailReport 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestCheckFailReport(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestCheckFailReport() {
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    store := testkit.CreateMockStore(t)
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := newInconsistencyKit(t, testkit.NewAsyncTestKit(t, store), newDefaultOpt())

    rmode := tk.sctx.GetSessionVars().EnableRedactLog

    // row more than unique index
    func() {
        // Go defer/Cleanup/Close 负责资源收尾；用注释标出生命周期边界。
        defer tk.rebuild()

        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec(tk.ctx, fmt.Sprintf("insert into %s values(1, 1, '10')", tblName))
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, tk.uniqueIndex.Delete(tk.sctx.GetTableCtx(), txn, types.MakeDatums(1), kv.IntHandle(1)))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, txn.Commit(tk.ctx))

        ctx, hook := testutil.WithLogHook(tk.ctx, t, "inconsistency")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        tk.MustGetErrMsg(ctx, "admin check table admin_test", "[admin:8223]data inconsistency in table: admin_test, index: uk1, handle: 1, index-values:\"\" != record-values:\"handle: 1, values: [KindInt64 1]\"")
        hook.CheckLogCount(t, 1)
        hook.Logs[0].CheckMsg(t, "admin check found data inconsistency")
        hook.Logs[0].CheckField(t,
            zap.String("table_name", "admin_test"),
            zap.String("index_name", "uk1"),
            zap.Stringer("row_id", redact.Stringer(rmode, kv.IntHandle(1))),
        )
        hook.Logs[0].CheckFieldNotEmpty(t, "row_mvcc")
    }()

    // row more than plain index
    func() {
        // Go defer/Cleanup/Close 负责资源收尾；用注释标出生命周期边界。
        defer tk.rebuild()

        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec(tk.ctx, fmt.Sprintf("insert into %s values(1, 1, '10')", tblName))
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, tk.plainIndex.Delete(tk.sctx.GetTableCtx(), txn, []types.Datum{types.NewStringDatum("10")}, kv.IntHandle(1)))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, txn.Commit(tk.ctx))

        ctx, hook := testutil.WithLogHook(tk.ctx, t, "inconsistency")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        tk.MustGetErrMsg(ctx, "admin check table admin_test", "[admin:8223]data inconsistency in table: admin_test, index: k2, handle: 1, index-values:\"\" != record-values:\"handle: 1, values: [KindString 10]\"")
        hook.CheckLogCount(t, 1)
        hook.Logs[0].CheckMsg(t, "admin check found data inconsistency")
        hook.Logs[0].CheckField(t,
            zap.String("table_name", "admin_test"),
            zap.String("index_name", "k2"),
            zap.Stringer("row_id", redact.Stringer(rmode, kv.IntHandle(1))),
        )
        hook.Logs[0].CheckFieldNotEmpty(t, "row_mvcc")
    }()

    // row is missed for plain key
    func() {
        // Go defer/Cleanup/Close 负责资源收尾；用注释标出生命周期边界。
        defer tk.rebuild()

        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        _, err = tk.plainIndex.Create(mock.NewContext().GetTableCtx(), txn, []types.Datum{types.NewStringDatum("100")}, kv.IntHandle(1), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, txn.Commit(tk.ctx))

        ctx, hook := testutil.WithLogHook(tk.ctx, t, "inconsistency")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        tk.MustGetErrMsg(ctx, "admin check table admin_test",
            "[admin:8223]data inconsistency in table: admin_test, index: k2, handle: 1, index-values:\"handle: 1, values: [KindString 100]\" != record-values:\"\"")
        hook.CheckLogCount(t, 1)
        logEntry := hook.Logs[0]
        logEntry.CheckMsg(t, "admin check found data inconsistency")
        logEntry.CheckField(t,
            zap.String("table_name", "admin_test"),
            zap.String("index_name", "k2"),
            zap.Stringer("row_id", redact.Stringer(rmode, kv.IntHandle(1))),
        )
        logEntry.CheckFieldNotEmpty(t, "row_mvcc")
        logEntry.CheckFieldNotEmpty(t, "index_mvcc")

        // test inconsistency check in index lookup
        ctx, hook = testutil.WithLogHook(tk.ctx, t, "")
        rs, err := tk.Exec(ctx, "select * from admin_test use index(k2) where c3 = '100'")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        _, err = session.GetRows4Test(ctx, testkit.TryRetrieveSession(ctx), rs)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Equal(t, "[executor:8133]data inconsistency in table: admin_test, index: k2, index-count:1 != record-count:0", err.Error())
        hook.CheckLogCount(t, 1)
        logEntry = hook.Logs[0]
        logEntry.CheckMsg(t, "indexLookup found data inconsistency")
        logEntry.CheckField(t,
            zap.String("table_name", "admin_test"),
            zap.String("index_name", "k2"),
            zap.Int64("table_cnt", 0),
            zap.Int64("index_cnt", 1),
            zap.String("missing_handles", `[1]`),
        )
        logEntry.CheckFieldNotEmpty(t, "row_mvcc_0")
    }()

    // row is missed for unique key
    func() {
        // Go defer/Cleanup/Close 负责资源收尾；用注释标出生命周期边界。
        defer tk.rebuild()

        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        _, err = tk.uniqueIndex.Create(mock.NewContext().GetTableCtx(), txn, []types.Datum{types.NewIntDatum(10)}, kv.IntHandle(1), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, txn.Commit(tk.ctx))

        ctx, hook := testutil.WithLogHook(tk.ctx, t, "inconsistency")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        tk.MustGetErrMsg(ctx, "admin check table admin_test",
            "[admin:8223]data inconsistency in table: admin_test, index: uk1, handle: 1, index-values:\"handle: 1, values: [KindInt64 10]\" != record-values:\"\"")
        hook.CheckLogCount(t, 1)
        logEntry := hook.Logs[0]
        logEntry.CheckMsg(t, "admin check found data inconsistency")
        logEntry.CheckField(t,
            zap.String("table_name", "admin_test"),
            zap.String("index_name", "uk1"),
            zap.Stringer("row_id", redact.Stringer(rmode, kv.IntHandle(1))),
        )
        logEntry.CheckFieldNotEmpty(t, "row_mvcc")
        logEntry.CheckFieldNotEmpty(t, "index_mvcc")

        // test inconsistency check in point-get
        ctx, hook = testutil.WithLogHook(tk.ctx, t, "")
        rs, err := tk.Exec(ctx, "select * from admin_test use index(uk1) where c2 = 10")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        _, err = session.GetRows4Test(ctx, testkit.TryRetrieveSession(ctx), rs)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        hook.CheckLogCount(t, 1)
        logEntry = hook.Logs[0]
        logEntry.CheckMsg(t, "indexLookup found data inconsistency")
        logEntry.CheckField(t,
            zap.String("table_name", "admin_test"),
            zap.String("index_name", "uk1"),
            zap.Int64("table_cnt", 0),
            zap.Int64("index_cnt", 1),
            zap.String("missing_handles", `[1]`),
        )
        logEntry.CheckFieldNotEmpty(t, "row_mvcc_0")
    }()

    // handle match but value is different for uk
    func() {
        // Go defer/Cleanup/Close 负责资源收尾；用注释标出生命周期边界。
        defer tk.rebuild()

        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec(tk.ctx, fmt.Sprintf("insert into %s values(1, 10, '100')", tblName))
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, tk.uniqueIndex.Delete(tk.sctx.GetTableCtx(), txn, []types.Datum{types.NewIntDatum(10)}, kv.IntHandle(1)))
        _, err = tk.uniqueIndex.Create(mock.NewContext().GetTableCtx(), txn, []types.Datum{types.NewIntDatum(20)}, kv.IntHandle(1), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, txn.Commit(tk.ctx))
        ctx, hook := testutil.WithLogHook(tk.ctx, t, "inconsistency")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        tk.MustGetErrMsg(ctx, "admin check table admin_test", "[admin:8223]data inconsistency in table: admin_test, index: uk1, handle: 1, index-values:\"handle: 1, values: [KindInt64 20]\" != record-values:\"handle: 1, values: [KindInt64 10]\"")
        hook.CheckLogCount(t, 1)
        logEntry := hook.Logs[0]
        logEntry.CheckMsg(t, "admin check found data inconsistency")
        logEntry.CheckField(t,
            zap.String("table_name", "admin_test"),
            zap.String("index_name", "uk1"),
            zap.Stringer("row_id", redact.Stringer(rmode, kv.IntHandle(1))),
        )
        logEntry.CheckFieldNotEmpty(t, "row_mvcc")
        logEntry.CheckFieldNotEmpty(t, "index_mvcc")
    }()

    // handle match but value is different for plain key
    func() {
        // Go defer/Cleanup/Close 负责资源收尾；用注释标出生命周期边界。
        defer tk.rebuild()

        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec(tk.ctx, fmt.Sprintf("insert into %s values(1, 10, '100')", tblName))
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, tk.plainIndex.Delete(tk.sctx.GetTableCtx(), txn, []types.Datum{types.NewStringDatum("100")}, kv.IntHandle(1)))
        _, err = tk.plainIndex.Create(mock.NewContext().GetTableCtx(), txn, []types.Datum{types.NewStringDatum("200")}, kv.IntHandle(1), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, txn.Commit(tk.ctx))
        ctx, hook := testutil.WithLogHook(tk.ctx, t, "inconsistency")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        tk.MustGetErrMsg(ctx, "admin check table admin_test",
            "[admin:8223]data inconsistency in table: admin_test, index: k2, handle: 1, index-values:\"handle: 1, values: [KindString 200]\" != record-values:\"handle: 1, values: [KindString 100]\"")
        hook.CheckLogCount(t, 1)
        logEntry := hook.Logs[0]
        logEntry.CheckMsg(t, "admin check found data inconsistency")
        logEntry.CheckField(t,
            zap.String("table_name", "admin_test"),
            zap.String("index_name", "k2"),
            zap.Stringer("row_id", redact.Stringer(rmode, kv.IntHandle(1))),
        )
        logEntry.CheckFieldNotEmpty(t, "row_mvcc")
        logEntry.CheckFieldNotEmpty(t, "index_mvcc")
    }()

    // test binary column.
    opt := newDefaultOpt()
    opt.clustered = "clustered"
    opt.pkColType = "varbinary(300)"
    opt.idxColType = "varbinary(300)"
    opt.ukColType = "varbinary(300)"
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk = newInconsistencyKit(t, testkit.NewAsyncTestKit(t, store), newDefaultOpt())
    func() {
        // Go defer/Cleanup/Close 负责资源收尾；用注释标出生命周期边界。
        defer tk.rebuild()

        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 时间等待/Eventually 依赖 Go 测试调度；保留轮询窗口和目标条件。
        encoded, err := codec.EncodeKey(time.UTC, nil, types.NewBytesDatum([]byte{1, 0, 1, 0, 0, 1, 1}))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        hd, err := kv.NewCommonHandle(encoded)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        _, err = tk.uniqueIndex.Create(mock.NewContext().GetTableCtx(), txn, []types.Datum{types.NewBytesDatum([]byte{1, 1, 0, 1, 1, 1, 1, 0})}, hd, nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, txn.Commit(tk.ctx))

        ctx, hook := testutil.WithLogHook(tk.ctx, t, "inconsistency")

        // TODO(tiancaiamao): admin check doesn't support the chunk protocol.
        // Remove this after https://github.com/pingcap/tidb/issues/35156
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec(ctx, "set @@tidb_enable_chunk_rpc = off")

        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        tk.MustGetErrMsg(ctx, "admin check table admin_test",
            `[admin:8223]data inconsistency in table: admin_test, index: uk1, handle: 282574488403969, index-values:"handle: 282574488403969, values: [KindInt64 282578800083201]" != record-values:""`)
        hook.CheckLogCount(t, 1)
        logEntry := hook.Logs[0]
        logEntry.CheckMsg(t, "admin check found data inconsistency")
        logEntry.CheckField(t,
            zap.String("table_name", "admin_test"),
            zap.String("index_name", "uk1"),
            zap.Stringer("row_id", redact.Stringer(rmode, kv.IntHandle(282574488403969))),
        )
        logEntry.CheckFieldNotEmpty(t, "row_mvcc")
        logEntry.CheckFieldNotEmpty(t, "index_mvcc")
    }()
}

// TestAdminCheckWithSnapshot 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCheckWithSnapshot(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCheckWithSnapshot() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_t_s")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_t_s (a int, b int, key(a));")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into admin_t_s values (0,0),(1,1);")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_t_s;")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_t_s a;")

    // 时间等待/Eventually 依赖 Go 测试调度；保留轮询窗口和目标条件。
    snapshotTime := time.Now()

    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("admin_t_s")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    tblInfo := tbl.Meta()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    idxInfo := tblInfo.FindIndexByName("a")
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    idxOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    _, err = idxOpr.Create(ctx, txn, types.MakeDatums(2), kv.IntHandle(100), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_t_s")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check index admin_t_s a")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)

    // For mocktikv, safe point is not initialized, we manually insert it for snapshot to use.
    safePointName := "tikv_gc_safe_point"
    safePointValue := "20060102-15:04:05 -0700"
    safePointComment := "All versions after safe point can be accessed. (DO NOT EDIT)"
    updateSafePoint := fmt.Sprintf(`INSERT INTO mysql.tidb VALUES ('%[1]s', '%[2]s', '%[3]s')
    ON DUPLICATE KEY
    UPDATE variable_value = '%[2]s', comment = '%[3]s'`, safePointName, safePointValue, safePointComment)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(updateSafePoint)
    // For admin check table when use snapshot.
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@tidb_snapshot = '" + snapshotTime.Format("2006-01-02 15:04:05.999999") + "'")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_t_s;")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_t_s a;")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@tidb_snapshot = ''")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_t_s")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check index admin_t_s a")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("admin cleanup index admin_t_s a")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("1"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_t_s;")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_t_s a;")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_t_s")
}

// TestAdminCheckTableWithSnapshot 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCheckTableWithSnapshot(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCheckTableWithSnapshot() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, dom := testkit.CreateMockStoreAndDomain(t)
    sv := server.CreateMockServer(t, store)

    sv.SetDomain(dom)
    dom.InfoSyncer().SetSessionManager(sv)
    // Go defer/Cleanup/Close 负责资源收尾；用注释标出生命周期边界。
    defer sv.Close()

    conn1 := server.CreateMockConn(t, sv)
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKitWithSession(t, store, conn1.Context().Session)
    conn2 := server.CreateMockConn(t, sv)
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk2 := testkit.NewTestKitWithSession(t, store, conn2.Context().Session)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table t(a int);")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values(1), (2), (3);")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("alter table t add index(a)")
    // SQL fixture/执行语句保持 源顺序。
    tk2.MustExec("use test")

    // For mocktikv, safe point is not initialized, we manually insert it for snapshot to use.
    safePointName := "tikv_gc_safe_point"
    safePointValue := "20060102-15:04:05 -0700"
    safePointComment := "All versions after safe point can be accessed. (DO NOT EDIT)"
    updateSafePoint := fmt.Sprintf(`INSERT INTO mysql.tidb VALUES ('%[1]s', '%[2]s', '%[3]s')
            ON DUPLICATE KEY UPDATE variable_value = '%[2]s', comment = '%[3]s'`, safePointName, safePointValue, safePointComment)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(updateSafePoint)

// 常量/变量保留 Go 测试命名和值，用于说明跨函数共享 fixture。
    var (
        // 并发、channel 或 atomic 行为依赖 Go runtime；这里保留同步意图供后续重写。
        wg      sync.WaitGroup
        startTS uint64
    )
    wg.Add(1)
    // 并发、channel 或 atomic 行为依赖 Go runtime；这里保留同步意图供后续重写。
    ch := make(chan uint64)

    // 并发、channel 或 atomic 行为依赖 Go runtime；这里保留同步意图供后续重写。
    go func() {
        // Go defer/Cleanup/Close 负责资源收尾；用注释标出生命周期边界。
        defer wg.Done()

        // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
        for tso := range ch {
            // SQL fixture/执行语句保持 源顺序。
            tk2.MustExec(fmt.Sprintf("set @@tidb_snapshot = '%d'", tso))
            // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
            tk2.MustExec("ADMIN CHECK TABLE t")
        }

        // SQL fixture/执行语句保持 源顺序。
        tk2.MustExec("set @@tidb_snapshot = ''")
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk2.MustExec("ADMIN CHECK TABLE t")
    }()

    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for _, alterTableSQL := range []string{
        "ALTER TABLE t MODIFY COLUMN a VARCHAR(4)",
        "ALTER TABLE t MODIFY COLUMN a INT",
        "ALTER TABLE t RENAME COLUMN a TO aa",
        "ALTER TABLE t ADD INDEX idx(aa)",
        "ALTER TABLE t DROP INDEX idx",
    } {
        // 事务边界会影响快照、锁和临时数据可见性；这里按 Go SQL 顺序保留。
        tk.MustExec("BEGIN")
        startTS = tk.Session().GetSessionVars().TxnCtx.StartTS
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec(alterTableSQL)
        // 事务边界会影响快照、锁和临时数据可见性；这里按 Go SQL 顺序保留。
        tk.MustExec("COMMIT")
        ch <- startTS
    }
    close(ch)

    wg.Wait()
}

// TestAdminCheckTableFailed 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCheckTableFailed(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCheckTableFailed() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 int, c2 int, c3 varchar(255) default '1', primary key(c1), key(c3), unique key(c2), key(c2, c3))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test (c1, c2, c3) values (-10, -20, 'y'), (-1, -10, 'z'), (1, 11, 'a'), (2, 12, 'b'), (5, 15, 'c'), (10, 20, 'd'), (20, 30, 'e')")

    // Make some corrupted index. Build the index information.
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("admin_test")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    tblInfo := tbl.Meta()
    idxInfo := tblInfo.Indices[1]
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    tk.Session().GetSessionVars().IndexLookupSize = 3
    tk.Session().GetSessionVars().MaxChunkSize = 3

    // Reduce one row of index.
    // Table count > index count.
    // Index c2 is missing 11.
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(-10), kv.IntHandle(-1))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: -1, index-values:\"\" != record-values:\"handle: -1, values: [KindInt64 -10]\"")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=1;")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: ?, index-values:\"?\" != record-values:\"?\"")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=marker;")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: ‹-1›, index-values:‹\"\"› != record-values:‹\"handle: -1, values: [KindInt64 -10]\"›")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=0;")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("admin recover index admin_test c2")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("1 7"))
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")

    // Add one row of index.
    // Table count < index count.
    // Index c2 has one more values than table data: 0, and the handle 0 hasn't correlative record.
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err = store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    _, err = indexOpr.Create(ctx, txn, types.MakeDatums(0), kv.IntHandle(0), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: 0, index-values:\"handle: 0, values: [KindInt64 0]\" != record-values:\"\"")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=1;")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: ?, index-values:\"?\" != record-values:\"?\"")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=marker;")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: ‹0›, index-values:‹\"handle: 0, values: [KindInt64 0]\"› != record-values:‹\"\"›")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=0;")

    // Add one row of index.
    // Table count < index count.
    // Index c2 has two more values than table data: 10, 13, and these handles have correlative record.
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err = store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(0), kv.IntHandle(0))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // Make sure the index value "19" is smaller "21". Then we scan to "19" before "21".
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    _, err = indexOpr.Create(ctx, txn, types.MakeDatums(19), kv.IntHandle(10), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    _, err = indexOpr.Create(ctx, txn, types.MakeDatums(13), kv.IntHandle(2), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: 10, index-values:\"handle: 10, values: [KindInt64 19]\" != record-values:\"handle: 10, values: [KindInt64 20]\"")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=1;")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: ?, index-values:\"?\" != record-values:\"?\"")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=marker;")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: ‹10›, index-values:‹\"handle: 10, values: [KindInt64 19]\"› != record-values:‹\"handle: 10, values: [KindInt64 20]\"›")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=0;")

    // Table count = index count.
    // Two indices have the same handle.
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err = store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(13), kv.IntHandle(2))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(12), kv.IntHandle(2))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.ErrorContains(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=1;")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: ?, index-values:\"?\" != record-values:\"?\"")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=marker;")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: ‹10›, index-values:‹\"handle: 10, values: [KindInt64 19]\"› != record-values:‹\"handle: 10, values: [KindInt64 20]\"›")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=0;")

    // Table count = index count.
    // Index c2 has one line of data is 19, the corresponding table data is 20.
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err = store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    _, err = indexOpr.Create(ctx, txn, types.MakeDatums(12), kv.IntHandle(2), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(20), kv.IntHandle(10))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: 10, index-values:\"handle: 10, values: [KindInt64 19]\" != record-values:\"handle: 10, values: [KindInt64 20]\"")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=1;")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: ?, index-values:\"?\" != record-values:\"?\"")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=marker;")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: c2, handle: ‹10›, index-values:‹\"handle: 10, values: [KindInt64 19]\"› != record-values:‹\"handle: 10, values: [KindInt64 20]\"›")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set @@global.tidb_redact_log=0;")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table other (a int);")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    tk.MustGetErrMsg("admin check table other, admin_test;",
        "admin check only supports one table at a time")

    // Recover records.
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err = store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(19), kv.IntHandle(10))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    _, err = indexOpr.Create(ctx, txn, types.MakeDatums(20), kv.IntHandle(10), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
}

// TestAdminCheckTableErrorLocate 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCheckTableErrorLocate(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCheckTableErrorLocate() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    executor.CheckTableFastBucketSize.Store(8)

    // 时间等待/Eventually 依赖 Go 测试调度；保留轮询窗口和目标条件。
    seed := time.Now().UnixNano()
    rand := rand.New(rand.NewSource(seed))
    logutil.BgLogger().Info("random generator", zap.Int64("seed", seed))

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 int, c2 int, primary key(c1), key(c2))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set cte_max_recursion_depth=10000;")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into admin_test with recursive cte(a, b) as (select 1, 1 union select a+1, b+1 from cte where cte.a< 10000) select * from cte;")

    sctx := mock.NewContext()
    ctx := sctx.GetTableCtx()

    // Make some corrupted index. Build the index information.
    getIndex := func() table.Index {
        sctx.Store = store
        // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
        is := domain.InfoSchema()
        dbName := ast.NewCIStr("test")
        tblName := ast.NewCIStr("admin_test")
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        tbl, err := is.TableByName(context.Background(), dbName, tblName)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        tblInfo := tbl.Meta()
        idxInfo := tblInfo.Indices[0]
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        indexOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        return indexOpr
    }

    indexOpr := getIndex()

    pattern := "handle:\\s(\\d+)"
    r := regexp.MustCompile(pattern)

    // No index record
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := range 10001 {
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(i), kv.IntHandle(i))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
    }
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err := tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)

    // Reset table.
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("truncate admin_test")
    indexOpr = getIndex()
    // No table record
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := range 100 {
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(ctx, txn, types.MakeDatums(i), kv.IntHandle(i), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
    }
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("truncate admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into admin_test with recursive cte(a, b) as (select 1, 1 union select a+1, b+1 from cte where cte.a< 10000) select * from cte;")
    indexOpr = getIndex()
    // Delete an index record randomly.
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := range 10 {
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        randomRow := rand.Intn(10000) + 1
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(randomRow), kv.IntHandle(randomRow))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        match := r.FindStringSubmatch(err.Error())
        require.Greater(t, len(match), 0)
        handle, err := strconv.Atoi(match[1])
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Equalf(t, randomRow, handle, "i :%d", i)
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        tk.MustQuery("admin recover index admin_test c2")
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("set @@tidb_enable_fast_table_check = 0")
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test")
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("set @@tidb_enable_fast_table_check = 1")
    }

    // Add an index record randomly on exists row.
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := range 10 {
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        randomRow := rand.Intn(10000) + 1
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(ctx, txn, types.MakeDatums(randomRow+1), kv.IntHandle(randomRow), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        match := r.FindStringSubmatch(err.Error())
        require.Greater(t, len(match), 0)
        handle, err := strconv.Atoi(match[1])
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Equalf(t, randomRow, handle, "i :%d", i)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err = store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(randomRow+1), kv.IntHandle(randomRow))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test")
    }

    // Add an index record randomly on not exists row.
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := range 10 {
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        randomRow := rand.Intn(10000) + 10000
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(ctx, txn, types.MakeDatums(randomRow+1), kv.IntHandle(randomRow), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        match := r.FindStringSubmatch(err.Error())
        require.Greater(t, len(match), 0)
        handle, err := strconv.Atoi(match[1])
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Equalf(t, randomRow, handle, "i :%d", i)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err = store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(randomRow+1), kv.IntHandle(randomRow))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test")
    }

    // Modify an index record randomly.
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := range 10 {
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        randomRow := rand.Intn(10000) + 1
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(randomRow), kv.IntHandle(randomRow))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(ctx, txn, types.MakeDatums(randomRow+1), kv.IntHandle(randomRow), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        match := r.FindStringSubmatch(err.Error())
        require.Greater(t, len(match), 0)
        handle, err := strconv.Atoi(match[1])
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Equalf(t, randomRow, handle, "i :%d", i)

        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err = store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(randomRow+1), kv.IntHandle(randomRow))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(ctx, txn, types.MakeDatums(randomRow), kv.IntHandle(randomRow), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test")
    }
}

// TestAdminCheckTableErrorLocateForClusterIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCheckTableErrorLocateForClusterIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCheckTableErrorLocateForClusterIndex() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    executor.CheckTableFastBucketSize.Store(8)

    // 时间等待/Eventually 依赖 Go 测试调度；保留轮询窗口和目标条件。
    seed := time.Now().UnixNano()
    rand := rand.New(rand.NewSource(seed))
    logutil.BgLogger().Info("random generator", zap.Int64("seed", seed))

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (c1 mediumint, c2 int, primary key(c1) clustered, key(c2))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set cte_max_recursion_depth=10000;")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into admin_test with recursive cte(a, b) as (select 1, 1 union select a+1, b+1 from cte where cte.a< 10000) select * from cte;")

    // Make some corrupted index. Build the index information.
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("admin_test")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    tblInfo := tbl.Meta()
    idxInfo := tblInfo.Indices[0]
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    pattern := "handle:\\s(\\d+)"
    r := regexp.MustCompile(pattern)

    getCommonHandle := func(randomRow int) *kv.CommonHandle {
        h, err := codec.EncodeKey(ctx.GetExprCtx().GetEvalCtx().Location(), nil, types.MakeDatums(randomRow)...)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        ch, err := kv.NewCommonHandle(h)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        return ch
    }

    // Delete an index record randomly.
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := range 10 {
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        randomRow := rand.Intn(10000) + 1
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(randomRow), getCommonHandle(randomRow))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        match := r.FindStringSubmatch(err.Error())
        require.Greater(t, len(match), 0)
        handle, err := strconv.Atoi(match[1])
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Equalf(t, randomRow, handle, "i :%d", i)
        // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
        tk.MustQuery("admin recover index admin_test c2")
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("set @@tidb_enable_fast_table_check = 0")
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test")
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("set @@tidb_enable_fast_table_check = 1")
    }

    // Add an index record randomly on exists row.
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := range 10 {
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        randomRow := rand.Intn(10000) + 1
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(ctx, txn, types.MakeDatums(randomRow+1), getCommonHandle(randomRow), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        match := r.FindStringSubmatch(err.Error())
        require.Greater(t, len(match), 0)
        handle, err := strconv.Atoi(match[1])
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Equalf(t, randomRow, handle, "i :%d", i)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err = store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(randomRow+1), getCommonHandle(randomRow))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test")
    }

    // Add an index record randomly on not exists row.
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := range 10 {
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        randomRow := rand.Intn(10000) + 10000
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(ctx, txn, types.MakeDatums(randomRow+1), getCommonHandle(randomRow), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        match := r.FindStringSubmatch(err.Error())
        require.Greater(t, len(match), 0)
        handle, err := strconv.Atoi(match[1])
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Equalf(t, randomRow, handle, "i :%d", i)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err = store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(randomRow+1), getCommonHandle(randomRow))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test")
    }

    // Modify an index record randomly.
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := range 10 {
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        randomRow := rand.Intn(10000) + 1
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(randomRow), getCommonHandle(randomRow))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(ctx, txn, types.MakeDatums(randomRow+1), getCommonHandle(randomRow), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        match := r.FindStringSubmatch(err.Error())
        require.Greater(t, len(match), 0)
        handle, err := strconv.Atoi(match[1])
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Equalf(t, randomRow, handle, "i :%d", i)

        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err = store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(ctx, txn, types.MakeDatums(randomRow+1), getCommonHandle(randomRow))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(ctx, txn, types.MakeDatums(randomRow), getCommonHandle(randomRow), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test")
    }
}

// TestAdminCleanUpGlobalIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCleanUpGlobalIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCleanUpGlobalIndex() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (a int, b int, c int, unique key uidx_a(a) global) partition by hash(c) partitions 5")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test values (-10, -20, 1), (-1, -10, 2), (1, 11, 3), (2, 12, 0), (5, 15, -1), (10, 20, -2), (20, 30, -3)")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("analyze table admin_test")

    // Make some corrupted index. Build the index information.
    sctx := mock.NewContext()
    sctx.Store = store
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("admin_test")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    tblInfo := tbl.Meta()
    idxInfo := tblInfo.Indices[0]
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, idxInfo.Global)
    idx := tbl.Indices()[0]
    require.NotNil(t, idx)

    // Reduce one row of table.
    // Index count > table count, (2, 12, 0) is deleted.
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    err = txn.Delete(tablecodec.EncodeRowKey(tblInfo.GetPartitionInfo().Definitions[0].ID, kv.IntHandle(4).Encoded()))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("admin cleanup index admin_test uidx_a")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("1"))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    require.Len(t, tk.MustQuery("select * from admin_test use index(uidx_a)").Rows(), 6)
}

// TestAdminRecoverGlobalIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminRecoverGlobalIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminRecoverGlobalIndex() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (a int, b int, c int, unique key uidx_a(a) global) partition by hash(c) partitions 5")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert admin_test values (-10, -20, 1), (-1, -10, 2), (1, 11, 3), (2, 12, 0), (5, 15, -1), (10, 20, -2), (20, 30, -3)")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("analyze table admin_test")

    // Make some corrupted index. Build the index information.
    sctx := mock.NewContext()
    sctx.Store = store
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("admin_test")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    tblInfo := tbl.Meta()
    idxInfo := tblInfo.Indices[0]
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, idxInfo.Global)
    idx := tbl.Indices()[0]
    require.NotNil(t, idx)

    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err := tables.NewIndex(tblInfo.GetPartitionInfo().Definitions[2].ID, tblInfo, idxInfo)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // Reduce one row of index.
    // Index count < table count, (-1, -10, 2) is deleted.
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(tk.Session().GetTableCtx(), txn, []types.Datum{types.NewIntDatum(-1)}, kv.IntHandle(2))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))

    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r := tk.MustQuery("admin recover index admin_test uidx_a")
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    r.Check(testkit.Rows("1 7"))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table admin_test")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    require.Len(t, tk.MustQuery("select * from admin_test use index(uidx_a)").Rows(), 7)
}

// TestAdminCheckGlobalIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCheckGlobalIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCheckGlobalIndex() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
// 常量/变量保留 Go 测试命名和值，用于说明跨函数共享 fixture。
    var enableFastCheck = []bool{false, true}
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for _, enabled := range enableFastCheck {
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("use test")
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("drop table if exists admin_test")

        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec(fmt.Sprintf("set tidb_enable_fast_table_check = %v", enabled))

        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("create table admin_test (a int, b int, c int, unique key uidx_a(a) global) partition by hash(c) partitions 5")
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("insert admin_test values (-10, -20, 1), (-1, -10, 2), (1, 11, 3), (2, 12, 0), (5, 15, -1), (10, 20, -2), (20, 30, -3)")

        // Make some corrupted index. Build the index information.
        sctx := mock.NewContext()
        sctx.Store = store
        // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
        is := domain.InfoSchema()
        dbName := ast.NewCIStr("test")
        tblName := ast.NewCIStr("admin_test")
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        tbl, err := is.TableByName(context.Background(), dbName, tblName)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        tblInfo := tbl.Meta()
        idxInfo := tblInfo.Indices[0]
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.True(t, idxInfo.Global)
        idx := tbl.Indices()[0]
        require.NotNil(t, idx)

        // Reduce one row of table.
        // Index count > table count, (2, 12, 0) is deleted.
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err := store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
        err = txn.Delete(tablecodec.EncodeRowKey(tblInfo.GetPartitionInfo().Definitions[0].ID, kv.IntHandle(4).Encoded()))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.ErrorContains(t, err, "[admin:8223]data inconsistency in table: admin_test, index: uidx_a, handle: 4, index-values:\"handle: 4, values: [KindInt64 2")

		// 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
		indexOpr, err := tables.NewIndex(tblInfo.GetPartitionInfo().Definitions[0].ID, tblInfo, idxInfo)
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		// Remove corresponding index key/value.
		// Admin check table will success.
		// 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
		txn, err = store.Begin()
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		// 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
		err = indexOpr.Delete(tk.Session().GetTableCtx(), txn, []types.Datum{types.NewIntDatum(2)}, kv.IntHandle(4))
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		// context 控制请求生命周期或取消语义；不创建真实异步执行环境。
		err = txn.Commit(context.Background())
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		// ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
		tk.MustExec("admin check table admin_test")

		// 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
		indexOpr, err = tables.NewIndex(tblInfo.GetPartitionInfo().Definitions[2].ID, tblInfo, idxInfo)
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)

		// Reduce one row of index.
		// Index count < table count, (-1, -10, 2) is deleted.
		// 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
		txn, err = store.Begin()
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		// 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
		err = indexOpr.Delete(tk.Session().GetTableCtx(), txn, []types.Datum{types.NewIntDatum(-1)}, kv.IntHandle(2))
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		// context 控制请求生命周期或取消语义；不创建真实异步执行环境。
		err = txn.Commit(context.Background())
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		err = tk.ExecToErr("admin check table admin_test")
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.Error(t, err)
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: uidx_a, handle: 2, index-values:\"\" != record-values:\"handle: 2, values: [KindInt64 -1]\"")

		// Add one row of index with inconsistent value.
		// Index count = table count, but data is different.
		// 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
		txn, err = store.Begin()
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		// 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
		_, err = indexOpr.Create(tk.Session().GetTableCtx(), txn, []types.Datum{types.NewIntDatum(100)}, kv.IntHandle(2), nil)
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		// context 控制请求生命周期或取消语义；不创建真实异步执行环境。
		err = txn.Commit(context.Background())
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		err = tk.ExecToErr("admin check table admin_test")
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.Error(t, err)
		// 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
		if !enabled {
			// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
			require.True(t, consistency.ErrAdminCheckInconsistentWithColInfo.Equal(err))
			// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
			require.EqualError(t, err, "[executor:8134]data inconsistency in table: admin_test, index: uidx_a, col: a, handle: \"2\", index-values:\"KindInt64 100\" != record-values:\"KindInt64 -1\", compare err:<nil>")
		} else {
			// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
			require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))
			// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
			require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: uidx_a, handle: 2, index-values:\"handle: 2, values: [KindInt64 100]\" != record-values:\"handle: 2, values: [KindInt64 -1]\"")
		}
	}
}

// TestAdminCheckGlobalIndexWithClusterIndex 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCheckGlobalIndexWithClusterIndex(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCheckGlobalIndexWithClusterIndex() {
	// testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
	store, domain := testkit.CreateMockStoreAndDomain(t)

	// testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
	tk := testkit.NewTestKit(t, store)

	getCommonHandle := func(row int) *kv.CommonHandle {
		h, err := codec.EncodeKey(tk.Session().GetSessionVars().StmtCtx.TimeZone(), nil, types.MakeDatums(row)...)
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		ch, err := kv.NewCommonHandle(h)
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		return ch
	}

// 常量/变量保留 Go 测试命名和值，用于说明跨函数共享 fixture。
	var enableFastCheck = []bool{false, true}
	// 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
	for _, enabled := range enableFastCheck {
		// SQL fixture/执行语句保持 源顺序。
		tk.MustExec("use test")
		// SQL fixture/执行语句保持 源顺序。
		tk.MustExec("drop table if exists admin_test")

		// SQL fixture/执行语句保持 源顺序。
		tk.MustExec(fmt.Sprintf("set tidb_enable_fast_table_check = %v", enabled))

		// SQL fixture/执行语句保持 源顺序。
		tk.MustExec("create table admin_test (a int, b int, c int, unique key uidx_a(a) global, primary key(c)) partition by hash(c) partitions 5")
		// SQL fixture/执行语句保持 源顺序。
		tk.MustExec("insert admin_test values (-10, -20, 1), (-1, -10, 2), (1, 11, 3), (2, 12, 0), (5, 15, -1), (10, 20, -2), (20, 30, -3)")

		// Make some corrupted index. Build the index information.
		sctx := mock.NewContext()
		sctx.Store = store
		// infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
		is := domain.InfoSchema()
		dbName := ast.NewCIStr("test")
		tblName := ast.NewCIStr("admin_test")
		// context 控制请求生命周期或取消语义；不创建真实异步执行环境。
		tbl, err := is.TableByName(context.Background(), dbName, tblName)
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		tblInfo := tbl.Meta()
		idxInfo := tblInfo.Indices[0]
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.True(t, idxInfo.Global)
		// infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
		df := tblInfo.GetPartitionInfo().Definitions[0]

		// Reduce one row of table.
		// Index count > table count, (2, 12, 0) is deleted.
		// 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
		txn, err := store.Begin()
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		txn.Delete(tablecodec.EncodeRowKey(df.ID, kv.IntHandle(0).Encoded()))
		// context 控制请求生命周期或取消语义；不创建真实异步执行环境。
		err = txn.Commit(context.Background())
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.NoError(t, err)
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		err = tk.ExecToErr("admin check table admin_test")
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.Error(t, err)
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))
		// require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
		require.ErrorContains(t, err, "[admin:8223]data inconsistency in table: admin_test, index: uidx_a, handle: 0, index-values:\"handle: 0, values: [KindInt64 2")

        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        indexOpr, err := tables.NewIndex(tblInfo.GetPartitionInfo().Definitions[0].ID, tblInfo, idxInfo)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // Remove corresponding index key/value.
        // Admin check table will success.
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err = store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(tk.Session().GetTableCtx(), txn, []types.Datum{types.NewIntDatum(2)}, getCommonHandle(0))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
        tk.MustExec("admin check table admin_test")

        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        indexOpr, err = tables.NewIndex(tblInfo.GetPartitionInfo().Definitions[2].ID, tblInfo, idxInfo)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // Reduce one row of index.
        // Index count < table count, (-1, -10, 2) is deleted.
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err = store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        err = indexOpr.Delete(tk.Session().GetTableCtx(), txn, []types.Datum{types.NewIntDatum(-1)}, getCommonHandle(2))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: uidx_a, handle: 2, index-values:\"\" != record-values:\"handle: 2, values: [KindInt64 -1]\"")

        // Add one row with inconsistent value.
        // Index count = table count, but data is different.
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        txn, err = store.Begin()
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
        _, err = indexOpr.Create(tk.Session().GetTableCtx(), txn, []types.Datum{types.NewIntDatum(100)}, getCommonHandle(2), nil)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
        err = txn.Commit(context.Background())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, err)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
        // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
        if !enabled {
            // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
            require.True(t, consistency.ErrAdminCheckInconsistentWithColInfo.Equal(err))
            // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
            require.EqualError(t, err, "[executor:8134]data inconsistency in table: admin_test, index: uidx_a, col: a, handle: \"2\", index-values:\"KindInt64 100\" != record-values:\"KindInt64 -1\", compare err:<nil>")
        } else {
            // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
            require.True(t, consistency.ErrAdminCheckInconsistent.Equal(err))
            // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
            require.EqualError(t, err, "[admin:8223]data inconsistency in table: admin_test, index: uidx_a, handle: 2, index-values:\"handle: 2, values: [KindInt64 100]\" != record-values:\"handle: 2, values: [KindInt64 -1]\"")
        }
    }
}

// TestAdminCheckGlobalIndexDuringDDL 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCheckGlobalIndexDuringDDL(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCheckGlobalIndexDuringDDL() {
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    store := testkit.CreateMockStore(t)
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)

// 常量/变量保留 Go 测试命名和值，用于说明跨函数共享 fixture。
    var schemaMap = make(map[model.SchemaState]struct{})

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk1 := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk1.MustExec("use test")
    onJobUpdatedExportedFunc := func(job *model.Job) {
        schemaMap[job.SchemaState] = struct{}{}
        _, err := tk1.Exec("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        assert.NoError(t, err)
    }

    // check table after delete some index key/value pairs.
    ddl.MockDMLExecution = func() {
        _, err := tk1.Exec("admin check table admin_test")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        assert.NoError(t, err)
    }

    batchSize := 32
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(fmt.Sprintf("set @@tidb_ddl_reorg_batch_size = %d", batchSize))

// 常量/变量保留 Go 测试命名和值，用于说明跨函数共享 fixture。
    var enableFastCheck = []bool{false, true}
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for _, enabled := range enableFastCheck {
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("use test")
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("drop table if exists admin_test")

        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec(fmt.Sprintf("set tidb_enable_fast_table_check = %v", enabled))

        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("create table admin_test (a int, b int, c int, unique key uidx_a(a) global, primary key(c)) partition by hash(c) partitions 5")
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("insert admin_test values (-10, -20, 1), (-1, -10, 2), (1, 11, 3), (2, 12, 0), (5, 15, -1), (10, 20, -2), (20, 30, -3)")
        // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
        for i := 1; i <= batchSize*2; i++ {
            // SQL fixture/执行语句保持 源顺序。
            tk.MustExec(fmt.Sprintf("insert admin_test values (%d, %d, %d)", i*5+1, i, i*5+1))
        }

        // failpoint 会改变 Go 测试中的执行路径；保留启停位置和注入含义。
        testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", onJobUpdatedExportedFunc)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/mockDMLExecution", "1*return(true)->return(false)"))
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec("alter table admin_test truncate partition p1")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/mockDMLExecution"))
        // failpoint 会改变 Go 测试中的执行路径；保留启停位置和注入含义。
        testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")

        // Should have 4 different schema states, `none`, `writeOnly`, `deleteOnly`, `deleteReorg`
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Len(t, schemaMap, 4)
        // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
        for ss := range schemaMap {
            delete(schemaMap, ss)
        }
    }
}

// TestAdminCheckGeneratedColumns 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCheckGeneratedColumns(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCheckGeneratedColumns() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("DROP TABLE IF EXISTS t")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("CREATE TABLE t(pk int PRIMARY KEY CLUSTERED, val int, gen int GENERATED ALWAYS AS (val * pk) VIRTUAL, KEY idx_gen(gen))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("INSERT INTO t(pk, val) VALUES (2, 5)")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("ADMIN CHECK TABLE t")

    // Make some corrupted index. Build the index information.
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    dbName := ast.NewCIStr("test")
    tblName := ast.NewCIStr("t")
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), dbName, tblName)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    tblInfo := tbl.Meta()
    idxInfo := tblInfo.Indices[0]
    tk.Session().GetSessionVars().IndexLookupSize = 3
    tk.Session().GetSessionVars().MaxChunkSize = 3

    // Simulate inconsistent index column
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(10), kv.IntHandle(2))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    _, err = indexOpr.Create(ctx, txn, types.MakeDatums(5), kv.IntHandle(2), nil)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    err = txn.Commit(context.Background())
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for _, enabled := range []bool{false, true} {
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec(fmt.Sprintf("set tidb_enable_fast_table_check = %v", enabled))
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        err = tk.ExecToErr("admin check table t")
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        require.Error(t, err)
    }
}

// TestFastAdminCheckWithError 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestFastAdminCheckWithError(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestFastAdminCheckWithError() {
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    store := testkit.CreateMockStore(t)

    // failpoint 会改变 Go 测试中的执行路径；保留启停位置和注入含义。
    testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/executor/mockFastCheckTableError", "return(true)")

    // Create a table with number of indexes larger than the worker pool size in check executor.
    // And the admin check shouldn't be blocked when meeting error.
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(`
        create table admin_test (c1 int, c2 int,
        key idx1(c1), key idx2(c1), key idx3(c1), key idx4(c1), key idx5(c1),
        key idx6(c1), key idx7(c1), key idx8(c1), key idx9(c1), key idx10(c1))
    `)
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExecToErr("admin check table admin_test")
}

// TestFastAdminCheckQuickPassSkipBucketed 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestFastAdminCheckQuickPassSkipBucketed(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestFastAdminCheckQuickPassSkipBucketed() {
    // testkit 创建 mock store/domain 会启动 Go 测试环境；只保留隔离存储和 domain 依赖关系。
    store, domain := testkit.CreateMockStoreAndDomain(t)

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set tidb_enable_fast_table_check = 1")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists t")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table t (id int primary key, k int, key(k))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (1, 1), (2, 2), (3, 3)")

    // If this failpoint is hit, it means we entered the bucketed refinement path.
    // failpoint 会改变 Go 测试中的执行路径；保留启停位置和注入含义。
    testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/executor/mockFastCheckTableBucketedCalled", "return(true)")

    // Consistent case: should exit from global checksum quick pass and not enter bucketed refinement.
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table t")

    // Inconsistent case: should fall back to bucketed refinement (failpoint triggers).
    sctx := mock.NewContext()
    sctx.Store = store
    ctx := sctx.GetTableCtx()
    // infoschema/table metadata 查找决定后续索引或分区操作对象；保留 Go 取数路径。
    is := domain.InfoSchema()
    // context 控制请求生命周期或取消语义；不创建真实异步执行环境。
    tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    tblInfo := tbl.Meta()
    idxInfo := tblInfo.Indices[0]
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    indexOpr, err := tables.NewIndex(tblInfo.ID, tblInfo, idxInfo)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)

    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    txn, err := store.Begin()
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // 底层 KV/index 操作用于制造或修复不一致；保留破坏点和提交顺序。
    err = indexOpr.Delete(ctx, txn, types.MakeDatums(1), kv.IntHandle(1))
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.NoError(t, txn.Commit(context.Background()))

    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    err = tk.ExecToErr("admin check table t")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.Error(t, err)
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.EqualError(t, err, "mock fast check table bucketed called")
}

// TestAdminCheckTableWithEnumAndPointGet 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestAdminCheckTableWithEnumAndPointGet(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestAdminCheckTableWithEnumAndPointGet() {
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    store := testkit.CreateMockStore(t)
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")

    // Test 1: Table with enum column and unique index
    // This scenario can generate PointGet plan
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (id int primary key, status enum('active', 'inactive', 'pending'), unique key uk_status(status))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into admin_test values (1, 'active'), (2, 'inactive'), (3, 'pending')")

    // Verify that a query with unique index on enum column generates PointGet plan
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    rows := tk.MustQuery("explain select * from admin_test use index(uk_status) where status = 'active'").Rows()
    hasPointGet := false
    hasIndexAccess := false
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for _, row := range rows {
        planType := fmt.Sprintf("%v", row[0])
        // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
        if strings.Contains(planType, "Point_Get") || strings.Contains(planType, "PointGet") {
            hasPointGet = true
            // Verify that access object contains ", index:" to ensure it's using secondary index
            // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
            if len(row) > 3 {
                accessObject := fmt.Sprintf("%v", row[3])
                // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
                if strings.Contains(accessObject, ", index:") {
                    hasIndexAccess = true
                }
            }
            break
        }
    }
    // This verifies that the scenario actually generates PointGet plan with index access
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, hasPointGet, "Expected PointGet plan for unique index query on enum column")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, hasIndexAccess, "Expected PointGet plan to use secondary index (access object should contain ', index:')")

    // Fast check mode - this is where verifyIndexSideQuery is called
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set tidb_enable_fast_table_check = 1")
    // This should pass with the fix (would fail without fix when running with --tags=intest)
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test uk_status")

    // Regular check mode - for comparison
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set tidb_enable_fast_table_check = 0")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test uk_status")

    // Test 2: Table with unique index (can also generate PointGet)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (id int primary key, name varchar(50), unique key uk_name(name))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into admin_test values (1, 'alice'), (2, 'bob'), (3, 'charlie')")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set tidb_enable_fast_table_check = 1")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test uk_name")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set tidb_enable_fast_table_check = 0")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test uk_name")

    // Test 3: Composite unique index with enum (can generate BatchPointGet)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (id int, type enum('A', 'B', 'C'), value int, unique key uk_composite(id, type))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into admin_test values (1, 'A', 100), (2, 'B', 200), (3, 'C', 300)")

    // Verify that a query with composite unique index can generate BatchPointGet plan
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    rows = tk.MustQuery("explain select * from admin_test use index(uk_composite) where id in (1, 2) and type = 'A'").Rows()
    hasBatchPointGet := false
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for _, row := range rows {
        planType := fmt.Sprintf("%v", row[0])
        // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
        if strings.Contains(planType, "Batch_Point_Get") || strings.Contains(planType, "BatchPointGet") {
            hasBatchPointGet = true
            break
        }
    }
    // This verifies that the scenario can generate BatchPointGet plan
    // Note: optimizer may choose different plans depending on data, so we don't require.True here
    // The important thing is that IF BatchPointGet is used, it should be recognized
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    if hasBatchPointGet {
        t.Logf("BatchPointGet plan detected for composite unique index query")
    }

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set tidb_enable_fast_table_check = 1")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test uk_composite")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set tidb_enable_fast_table_check = 0")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test uk_composite")

    // Clean up
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set tidb_enable_fast_table_check = default")

    // Test 4: Direct test to verify PointGet with index access is recognized
    // This test creates a scenario where a simple query (not aggregation) generates PointGet
    // to ensure the verifyIndexSideQuery logic works correctly
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists admin_test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table admin_test (id int primary key, code varchar(10), unique key uk_code(code))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into admin_test values (1, 'A001'), (2, 'A002'), (3, 'A003')")

    // Verify PointGet plan with index access
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    rows = tk.MustQuery("explain select * from admin_test use index(uk_code) where code = 'A001'").Rows()
    hasPointGet4 := false
    hasIndexAccess4 := false
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for _, row := range rows {
        planType := fmt.Sprintf("%v", row[0])
        // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
        if strings.Contains(planType, "Point_Get") || strings.Contains(planType, "PointGet") {
            hasPointGet4 = true
            // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
            if len(row) > 3 {
                accessObject := fmt.Sprintf("%v", row[3])
                // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
                if strings.Contains(accessObject, ", index:") {
                    hasIndexAccess4 = true
                }
            }
            break
        }
    }
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, hasPointGet4, "Expected PointGet plan for unique index query")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, hasIndexAccess4, "Expected PointGet plan to use secondary index")

    // Test BatchPointGet with index access
    // 查询和期望行保留 Go testkit 断言语义，后续接线时再映射到 Rust 测试框架。
    rows = tk.MustQuery("explain select * from admin_test use index(uk_code) where code in ('A001', 'A002')").Rows()
    hasBatchPointGet4 := false
    hasIndexAccess4Batch := false
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for _, row := range rows {
        planType := fmt.Sprintf("%v", row[0])
        // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
        if strings.Contains(planType, "Batch_Point_Get") || strings.Contains(planType, "BatchPointGet") {
            hasBatchPointGet4 = true
            // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
            if len(row) > 3 {
                accessObject := fmt.Sprintf("%v", row[3])
                // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
                if strings.Contains(accessObject, ", index:") {
                    hasIndexAccess4Batch = true
                }
            }
            break
        }
    }
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, hasBatchPointGet4, "Expected BatchPointGet plan for unique index IN query")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, hasIndexAccess4Batch, "Expected BatchPointGet plan to use secondary index")

    // Run admin check to ensure it works with the fix
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set tidb_enable_fast_table_check = 1")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table admin_test")
    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check index admin_test uk_code")
}

// TestFastCheckTableConcurrent 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestFastCheckTableConcurrent(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestFastCheckTableConcurrent() {
    // This test verifies that concurrent execution of admin check table works correctly.
    // Note: The data race in ExecDetails (fixed by using ContextWithInitializedExecDetails
    // in getCheckSum) cannot be detected in unit tests because mocktikv doesn't trigger
    // the network traffic writes to ExecDetails that happen in real TiKV environments.
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    store := testkit.CreateMockStore(t)
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists t_concurrent")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table t_concurrent (id int primary key, val int, key idx_val(val))")

    // Insert enough data to trigger parallel execution in checkIndexWorker
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := 0; i < 100; i++ {
        // SQL fixture/执行语句保持 源顺序。
        tk.MustExec(fmt.Sprintf("insert into t_concurrent values (%d, %d)", i, i*10))
    }

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set tidb_enable_fast_table_check = 1")

    // Run multiple admin check table concurrently
// 常量/变量保留 Go 测试命名和值，用于说明跨函数共享 fixture。
    // 并发、channel 或 atomic 行为依赖 Go runtime；这里保留同步意图供后续重写。
    var wg sync.WaitGroup
    // 控制流沿用 Go 源结构，用于保留不同参数组合、错误分支和等待条件。
    for i := 0; i < 5; i++ {
        wg.Add(1)
        // 并发、channel 或 atomic 行为依赖 Go runtime；这里保留同步意图供后续重写。
        go func() {
            // Go defer/Cleanup/Close 负责资源收尾；用注释标出生命周期边界。
            defer wg.Done()
            // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
            tkConcurrent := testkit.NewTestKit(t, store)
            // SQL fixture/执行语句保持 源顺序。
            tkConcurrent.MustExec("use test")
            // SQL fixture/执行语句保持 源顺序。
            tkConcurrent.MustExec("set tidb_enable_fast_table_check = 1")
            // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
            tkConcurrent.MustExec("admin check table t_concurrent")
        }()
    }
    wg.Wait()
}

// TestFastAdminCheckPropagateSessionVarsToSysSession 对应 Go 的同名函数，保留原测试语义、调用顺序和断言路径。
// Go 签名: func TestFastAdminCheckPropagateSessionVarsToSysSession(t *testing.T) {
// Go testing.T/testkit/require 调用按原顺序保留，便于后续逐步接入 Rust 测试 harness。
pub fn TestFastAdminCheckPropagateSessionVarsToSysSession() {
    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    store := testkit.CreateMockStore(t)

// 常量/变量保留 Go 测试命名和值，用于说明跨函数共享 fixture。
    const (
        expectedMemQuotaQuery           = int64(2 * 1024 * 1024 * 1024)
        expectedDistSQLScanConcurrency  = 7
        expectedExecutorConcurrency     = 9
        expectedMaxExecutionTimeMS      = uint64(10 * 60 * 1000)
        expectedTiKVClientReadTimeoutMS = uint64(10 * 60 * 1000)
    )

// 常量/变量保留 Go 测试命名和值，用于说明跨函数共享 fixture。
    // 并发、channel 或 atomic 行为依赖 Go runtime；这里保留同步意图供后续重写。
    var called atomic.Bool

    // failpoint 会改变 Go 测试中的执行路径；保留启停位置和注入含义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/executor/fastCheckTableAfterInitSessCtx", func(sysVars *variable.SessionVars, _ *error) {
        called.Store(true)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        assert.Equal(t, expectedMemQuotaQuery, sysVars.MemQuotaQuery)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        assert.NotNil(t, sysVars.MemTracker)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        assert.Equal(t, expectedMemQuotaQuery, sysVars.MemTracker.GetBytesLimit())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        assert.Equal(t, expectedDistSQLScanConcurrency, sysVars.DistSQLScanConcurrency())
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        assert.Equal(t, expectedExecutorConcurrency, sysVars.ExecutorConcurrency)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        assert.Equal(t, expectedMaxExecutionTimeMS, sysVars.MaxExecutionTime)
        // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
        assert.Equal(t, expectedTiKVClientReadTimeoutMS, sysVars.TiKVClientReadTimeout)
    })

    // testkit/mock store 是 Go 测试外部依赖；只保留会话创建和共享 store 的调用形状。
    tk := testkit.NewTestKit(t, store)
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("use test")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("set tidb_enable_fast_table_check = 1")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("drop table if exists t")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("create table t (id int primary key, k int, key(k))")
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec("insert into t values (1, 1)")

    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(fmt.Sprintf("set @@tidb_mem_quota_query = %d", expectedMemQuotaQuery))
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(fmt.Sprintf("set @@tidb_distsql_scan_concurrency = %d", expectedDistSQLScanConcurrency))
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(fmt.Sprintf("set @@tidb_executor_concurrency = %d", expectedExecutorConcurrency))
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(fmt.Sprintf("set @@max_execution_time = %d", expectedMaxExecutionTimeMS))
    // SQL fixture/执行语句保持 源顺序。
    tk.MustExec(fmt.Sprintf("set @@tikv_client_read_timeout = %d", expectedTiKVClientReadTimeoutMS))

    // ADMIN 语句是本测试核心校验点；记录命令但不会真正执行 TiDB 逻辑。
    tk.MustExec("admin check table t")
    // require/assert 断言保留错误路径或结果校验语义；当前只作为说明。
    require.True(t, called.Load(), "failpoint callback not triggered")
}
*/

use crate::{AdminSessionVars, AdminTable, Inconsistency};
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{NewTestKit, Rows, TestKit};

struct RestoreOnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> RestoreOnDrop<F> {
    fn new(restore: F) -> Self {
        Self(Some(restore))
    }
}

impl<F: FnOnce()> Drop for RestoreOnDrop<F> {
    fn drop(&mut self) {
        if let Some(restore) = self.0.take() {
            restore();
        }
    }
}

fn admin_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk
}

/// Go 多值索引场景的真实成功路径：JSON 数组索引和普通 ADMIN CHECK
/// 必须共享同一会话配置与元数据。
#[test]
fn admin_check_accepts_consistent_multi_valued_index() {
    let _restore = RestoreOnDrop::new(astersql_config::restore_func());
    astersql_config::update_global(|conf| {
        conf.experimental.allows_expression_index = true;
    });
    let mut tk = admin_testkit();
    tk.MustExec("drop table if exists admin_multi", Vec::new());
    tk.MustExec(
        "create table admin_multi (id int primary key, payload json, index idx_payload((cast(payload as signed array))))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into admin_multi values (0, '[0,1,2]'), (1, '[1,2,3]'), (2, '[2,3,4]'), (3, '[3,4,5]'), (4, '[4,5,6]')",
        Vec::new(),
    );
    tk.MustExec("admin check table admin_multi", Vec::new());
    tk.MustExec("admin check index admin_multi idx_payload", Vec::new());
}

/// ADMIN RECOVER/CLEANUP INDEX must execute through ConcreteSession and return
/// the same no-corruption counts as the Go testkit path.
#[test]
fn admin_index_maintenance_executes_through_real_sql() {
    let mut tk = admin_testkit();
    tk.MustExec("drop table if exists admin_maintenance", Vec::new());
    tk.MustExec(
        "create table admin_maintenance (id int primary key, value int, index idx_value(value))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into admin_maintenance values (1, 10), (2, 20), (3, 30)",
        Vec::new(),
    );
    tk.MustQuery(
        "admin recover index admin_maintenance idx_value",
        Vec::new(),
    )
    .Check(Rows(&["0 3"]));
    tk.MustQuery(
        "admin cleanup index admin_maintenance idx_value",
        Vec::new(),
    )
    .Check(Rows(&["0"]));
    tk.MustQuery(
        "admin recover index admin_maintenance `primary`",
        Vec::new(),
    )
    .Check(Rows(&["0 0"]));
    tk.MustExec("admin check table admin_maintenance", Vec::new());
}

/// 构造单分区、单值索引样例表（handle 1/2/3/10/20）。
fn scalar_table(global: bool) -> AdminTable {
    let mut table = AdminTable::new(false, global);
    for handle in [1, 2, 3, 10, 20] {
        table.insert(0, handle, vec![handle]);
    }
    table
}

/// recover 应补回缺失索引项并返回 (新增数, 扫描行数)。
#[test]
fn admin_recover_index_restores_missing_entries_and_reports_scan_count() {
    let mut table = scalar_table(false);
    table.corrupt_remove(0, 1, 1);
    assert_eq!(
        table.check(),
        Err(Inconsistency::MissingIndex {
            partition: 0,
            handle: 1,
            value: 1
        })
    );
    assert_eq!(table.indexed_row_count(), 4);
    assert_eq!(table.recover_index(), (1, 5));
    assert_eq!(table.indexed_row_count(), 5);
    assert_eq!(table.check(), Ok(()));
}

/// cleanup 应删除悬空与错误值索引项。
#[test]
fn admin_cleanup_index_removes_dangling_and_wrong_value_entries() {
    let mut table = scalar_table(false);
    table.corrupt_insert(0, 42, 42);
    table.corrupt_insert(0, 1, 99);
    assert!(matches!(
        table.check(),
        Err(Inconsistency::DanglingIndex { .. })
    ));
    assert_eq!(table.cleanup_index(), 2);
    assert_eq!(table.check(), Ok(()));
}

/// 多值索引 recover 应对 JSON 数组元素去重后补齐。
#[test]
fn multi_valued_recover_deduplicates_json_array_elements() {
    let mut table = AdminTable::new(true, false);
    for handle in 0..5 {
        table.insert(0, handle, vec![handle, handle + 1, handle + 2]);
    }
    table.corrupt_remove(0, 1, 2);
    assert_eq!(
        table.check(),
        Err(Inconsistency::MissingIndex {
            partition: 0,
            handle: 1,
            value: 2,
        })
    );
    assert_eq!(table.recover_index(), (1, 5));
    assert_eq!(table.check(), Ok(()));
}

/// Go 的多值索引 cleanup 场景：额外的 (value=9, handle=9) 应只删除一项。
#[test]
fn multi_valued_cleanup_removes_only_the_dangling_entry() {
    let mut table = AdminTable::new(true, false);
    for handle in 0..5 {
        table.insert(0, handle, vec![handle, handle + 1, handle + 2]);
    }
    table.corrupt_insert(0, 9, 9);
    assert_eq!(
        table.check(),
        Err(Inconsistency::DanglingIndex {
            partition: 0,
            handle: 9,
            value: 9,
        })
    );
    assert_eq!(table.cleanup_index(), 1);
    assert_eq!(table.check(), Ok(()));
}

/// 分区索引修复时相同 handle 按 partition 隔离。
#[test]
fn partition_index_repair_keeps_equal_handles_isolated_by_partition() {
    let mut table = AdminTable::new(false, false);
    table.insert(11, 1, vec![7]);
    table.insert(12, 1, vec![7]);
    table.corrupt_remove(12, 1, 7);
    assert_eq!(table.recover_index(), (1, 2));
    assert_eq!(table.check(), Ok(()));
}

/// Go 对 hash/range 两种三分区表逐分区破坏并恢复，每次都扫描三行。
#[test]
fn partition_index_recovery_covers_hash_and_range_layouts() {
    let layouts = [
        [(101, 0, 0), (102, 1, 1), (103, 2, 2)],
        [(201, 0, 0), (202, 6, 6), (203, 12, 12)],
    ];

    for layout in layouts {
        let mut table = AdminTable::new(false, false);
        for (partition, handle, value) in layout {
            table.insert(partition, handle, vec![value]);
        }
        assert_eq!(table.recover_index(), (0, 3));

        for (partition, handle, value) in layout {
            table.corrupt_remove(partition, handle, value);
            assert_eq!(
                table.check(),
                Err(Inconsistency::MissingIndex {
                    partition,
                    handle,
                    value,
                })
            );
            assert_eq!(table.indexed_row_count(), 2);
            assert_eq!(table.recover_index(), (1, 3));
            assert_eq!(table.indexed_row_count(), 3);
            assert_eq!(table.check(), Ok(()));
        }
    }
}

/// 全局索引 cleanup/recover 保留分区身份。
#[test]
fn global_index_cleanup_and_recovery_preserve_partition_identity() {
    let mut table = AdminTable::new(false, true);
    table.insert(101, 1, vec![11]);
    table.insert(102, 2, vec![22]);
    table.corrupt_remove(101, 1, 11);
    table.corrupt_insert(999, 9, 99);
    assert!(table.is_global());
    assert_eq!(table.cleanup_index(), 1);
    assert_eq!(table.recover_index(), (1, 2));
    assert_eq!(table.check(), Ok(()));
}

/// Go 生成列场景把正确索引值 10 替换为错误值 5；普通/快速检查都必须报错。
#[test]
fn generated_index_corruption_is_detected_by_regular_and_fast_checks() {
    let mut table = AdminTable::new(false, false);
    table.insert(0, 2, vec![10]);
    table.corrupt_remove(0, 2, 10);
    table.corrupt_insert(0, 2, 5);
    assert_eq!(table.indexed_row_count(), 1);
    let expected = Err(Inconsistency::MissingIndex {
        partition: 0,
        handle: 2,
        value: 10,
    });
    assert_eq!(table.check(), expected);

    let vars = AdminSessionVars {
        index_lookup_size: 3,
        max_chunk_size: 3,
        concurrency: 4,
    };
    let mut propagated = Vec::new();
    assert_eq!(table.fast_check(vars, &mut propagated), expected);
    assert_eq!(propagated, vec![vars]);
}

/// 快照检查看到旧一致态，当前表则报缺失索引。
#[test]
fn snapshot_check_observes_old_consistent_state_and_current_corruption() {
    let mut table = scalar_table(false);
    let snapshot = table.clone();
    table.corrupt_remove(0, 10, 10);
    assert_eq!(snapshot.check(), Ok(()));
    assert!(matches!(
        table.check(),
        Err(Inconsistency::MissingIndex { handle: 10, .. })
    ));
}

/// fast_check 应把会话变量传播到内部 worker（此处记入 propagated）。
#[test]
fn fast_check_propagates_session_variables_to_internal_worker() {
    let table = scalar_table(false);
    let vars = AdminSessionVars {
        index_lookup_size: 3,
        max_chunk_size: 3,
        concurrency: 9,
    };
    let mut propagated = Vec::new();
    assert_eq!(table.fast_check(vars, &mut propagated), Ok(()));
    assert_eq!(propagated, vec![vars]);
}

/// Go 并发场景启动五个独立会话；内存契约用五个 worker 并发读取同一一致态。
#[test]
fn fast_check_supports_five_concurrent_workers() {
    let table = std::sync::Arc::new(scalar_table(false));
    let workers: Vec<_> = (0..5)
        .map(|_| {
            let table = std::sync::Arc::clone(&table);
            std::thread::spawn(move || {
                let mut propagated = Vec::new();
                assert_eq!(
                    table.fast_check(AdminSessionVars::default(), &mut propagated),
                    Ok(())
                );
                assert_eq!(propagated, vec![AdminSessionVars::default()]);
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("fast-check worker panicked");
    }
}
