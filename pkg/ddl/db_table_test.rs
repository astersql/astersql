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

// DDL（数据定义语言）表相关操作的测试模块。
//
// 覆盖场景：建表（含 ID 分配与重名处理）、列定义校验、truncate（截断表）后
// 表锁迁移、drop view/table 的对象类型区分、以及 DDL 作业（job）状态流转。
// 文件前半部分保留了 Go(TiDB) 原始测试的机械迁移文本（置于块注释中，待接线），
// 后半部分是当前可用的 Rust 测试，基于内存版 `Executor` 与 `MemoryJobBackend`。
//
// 术语说明：DDL 指修改库表结构的语句（如 CREATE/ALTER/DROP）；表锁用于
// 阻止其他会话并发读写；DDL 作业指后台异步执行 DDL 的任务单元。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

/*
// 建表、加列、表锁、批量建表、drop tables、约束和 BR 建表相关测试流程。
// 主要类型、函数、测试场景和辅助函数按 Go 文件顺序保留；关键分支、资源收尾、错误路径、并发和外部依赖在附近用中文说明。
// Go imports（保留依赖边界，待 Rust crate 接线）：
// - "bytes"
// - "context"
// - "fmt"
// - "strconv"
// - "strings"
// - "testing"
// - "time"
// - "github.com/pingcap/errors"
// - "github.com/pingcap/tidb/pkg/config"
// - "github.com/pingcap/tidb/pkg/config/kerneltype"
// - "github.com/pingcap/tidb/pkg/ddl"
// - testddlutil "github.com/pingcap/tidb/pkg/ddl/testutil"
// - "github.com/pingcap/tidb/pkg/domain"
// - "github.com/pingcap/tidb/pkg/errno"
// - "github.com/pingcap/tidb/pkg/infoschema"
// - "github.com/pingcap/tidb/pkg/kv"
// - "github.com/pingcap/tidb/pkg/meta"
// - "github.com/pingcap/tidb/pkg/meta/model"
// - "github.com/pingcap/tidb/pkg/parser/ast"
// - "github.com/pingcap/tidb/pkg/parser/auth"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/parser/terror"
// - "github.com/pingcap/tidb/pkg/sessionctx"
// - "github.com/pingcap/tidb/pkg/store/mockstore"
// - "github.com/pingcap/tidb/pkg/table"
// - "github.com/pingcap/tidb/pkg/table/tables"
// - "github.com/pingcap/tidb/pkg/testkit"
// - "github.com/pingcap/tidb/pkg/testkit/external"
// - "github.com/pingcap/tidb/pkg/testkit/testfailpoint"
// - "github.com/pingcap/tidb/pkg/types"
// - "github.com/pingcap/tidb/pkg/util"
// - "github.com/stretchr/testify/require"

// TestAddNotNullColumn 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestAddNotNullColumn(t *testing.T) {
#[test]
pub fn TestAddNotNullColumn() {
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    // for different databases
    tk.MustExec("create table tnn (c1 int primary key auto_increment, c2 int)")
    tk.MustExec("insert tnn (c2) values (0)" + strings.Repeat(",(0)", 99))
    done := make(chan error, 1)
    testddlutil.SessionExecInGoroutine(store, "test", "alter table tnn add column c3 int not null default 3", done)
    updateCnt := 0
out:
    // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
    for {
        select {
        case err := <-done:
            // Go require/assert 断言描述预期结果；不声明当前可运行。
            require.NoError(t, err)
            break out
        default:
            // Close issue #14636
            // Because add column action is not amendable now, it causes an error when the schema is changed
            // in the process of an insert statement.
            _, err := tk.Exec("update tnn set c2 = c2 + 1 where c1 = 99")
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if err == nil {
                updateCnt++
            }
        }
    }
    expected := fmt.Sprintf("%d %d", updateCnt, 3)
    tk.MustQuery("select c2, c3 from tnn where c1 = 99").Check(testkit.Rows(expected))
    tk.MustExec("drop table tnn")
}

// TestAddNotNullColumnWhileInsertOnDupUpdate 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestAddNotNullColumnWhileInsertOnDupUpdate(t *testing.T) {
#[test]
pub fn TestAddNotNullColumnWhileInsertOnDupUpdate() {
    store := testkit.CreateMockStore(t)
    tk1 := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk1.MustExec("use test")
    tk2 := testkit.NewTestKit(t, store)
    tk2.MustExec("use test")
    closeCh := make(chan bool)
    // WaitGroup 相关逻辑描述并发测试同步点，当前不启动真实并发执行器。
    var wg util.WaitGroupWrapper
    tk1.MustExec("create table nn (a int primary key, b int)")
    tk1.MustExec("insert nn values (1, 1)")
    // WaitGroup 相关逻辑描述并发测试同步点，当前不启动真实并发执行器。
    wg.Run(func() {
        // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
        for {
            select {
            case <-closeCh:
                return
            default:
            }
            tk2.MustExec("insert nn (a, b) values (1, 1) on duplicate key update a = 1, b = values(b) + 1")
        }
    })
    tk1.MustExec("alter table nn add column c int not null default 3 after a")
    close(closeCh)
    // WaitGroup 相关逻辑描述并发测试同步点，当前不启动真实并发执行器。
    wg.Wait()
    tk1.MustQuery("select * from nn").Check(testkit.Rows("1 3 2"))
}

// TestTransactionOnAddDropColumn 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestTransactionOnAddDropColumn(t *testing.T) {
#[test]
pub fn TestTransactionOnAddDropColumn() {
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("set @@global.tidb_max_delta_schema_count= 4096")
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t1")
    tk.MustExec("create table t1 (a int, b int);")
    tk.MustExec("create table t2 (a int, b int);")
    tk.MustExec("insert into t2 values (2,0)")

    transactions := [][]string{
        {
            "begin",
            "insert into t1 set a=1",
            "update t1 set b=1 where a=1",
            "commit",
        },
        {
            "begin",
            "insert into t1 select a,b from t2",
            "update t1 set b=2 where a=2",
            "commit",
        },
    }

    var checkErr error
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if checkErr != nil {
            return
        }
        switch job.SchemaState {
        case model.StateWriteOnly, model.StateWriteReorganization, model.StateDeleteOnly, model.StateDeleteReorganization:
        default:
            return
        }
        // do transaction.
        // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
        for _, transaction := range transactions {
            // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
            for _, sql := range transaction {
                // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
                if _, checkErr = tk.Exec(sql); checkErr != nil {
                    checkErr = errors.Errorf("err: %s, sql: %s, job schema state: %s", checkErr.Error(), sql, job.SchemaState)
                    return
                }
            }
        }
    })
    done := make(chan error, 1)
    // test transaction on add column.
    // Go 这里启动 goroutine；保留并发触发点，不真正调度异步任务。
    go backgroundExec(store, "test", "alter table t1 add column c int not null after a", done)
    err := <-done
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Nil(t, checkErr)
    tk.MustQuery("select a,b from t1 order by a").Check(testkit.Rows("1 1", "1 1", "1 1", "2 2", "2 2", "2 2"))
    tk.MustExec("delete from t1")

    // test transaction on drop column.
    // Go 这里启动 goroutine；保留并发触发点，不真正调度异步任务。
    go backgroundExec(store, "test", "alter table t1 drop column c", done)
    err = <-done
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Nil(t, checkErr)
    tk.MustQuery("select a,b from t1 order by a").Check(testkit.Rows("1 1", "1 1", "1 1", "2 2", "2 2", "2 2"))
}

// TestCreateTableWithSetCol 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestCreateTableWithSetCol(t *testing.T) {
#[test]
pub fn TestCreateTableWithSetCol() {
    store := testkit.CreateMockStore(t, mockstore.WithDDLChecker())

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t_set (a int, b set('e') default '');")
    tk.MustQuery("show create table t_set").Check(testkit.Rows("t_set CREATE TABLE `t_set` (\n" +
        "  `a` int(11) DEFAULT NULL,\n" +
        "  `b` set('e') DEFAULT ''\n" +
        ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))
    tk.MustExec("drop table t_set")
    tk.MustExec("create table t_set (a set('a', 'b', 'c', 'd') default 'a,c,c');")
    tk.MustQuery("show create table t_set").Check(testkit.Rows("t_set CREATE TABLE `t_set` (\n" +
        "  `a` set('a','b','c','d') DEFAULT 'a,c'\n" +
        ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))

    // It's for failure cases.
    // The type of default value is string.
    tk.MustExec("drop table t_set")
    failedSQL := "create table t_set (a set('1', '4', '10') default '3');"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrInvalidDefault)
    failedSQL = "create table t_set (a set('1', '4', '10') default '1,4,11');"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrInvalidDefault)
    // Success when the new collation is enabled.
    tk.MustExec("create table t_set (a set('1', '4', '10') default '1 ,4');")
    // The type of default value is int.
    failedSQL = "create table t_set (a set('1', '4', '10') default 0);"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrInvalidDefault)
    failedSQL = "create table t_set (a set('1', '4', '10') default 8);"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrInvalidDefault)

    // The type of default value is int.
    // It's for successful cases
    tk.MustExec("drop table if exists t_set")
    tk.MustExec("create table t_set (a set('1', '4', '10', '21') default 1);")
    tk.MustQuery("show create table t_set").Check(testkit.Rows("t_set CREATE TABLE `t_set` (\n" +
        "  `a` set('1','4','10','21') DEFAULT '1'\n" +
        ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))
    tk.MustExec("drop table t_set")
    tk.MustExec("create table t_set (a set('1', '4', '10', '21') default 2);")
    tk.MustQuery("show create table t_set").Check(testkit.Rows("t_set CREATE TABLE `t_set` (\n" +
        "  `a` set('1','4','10','21') DEFAULT '4'\n" +
        ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))
    tk.MustExec("drop table t_set")
    tk.MustExec("create table t_set (a set('1', '4', '10', '21') default 3);")
    tk.MustQuery("show create table t_set").Check(testkit.Rows("t_set CREATE TABLE `t_set` (\n" +
        "  `a` set('1','4','10','21') DEFAULT '1,4'\n" +
        ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))
    tk.MustExec("drop table t_set")
    tk.MustExec("create table t_set (a set('1', '4', '10', '21') default 15);")
    tk.MustQuery("show create table t_set").Check(testkit.Rows("t_set CREATE TABLE `t_set` (\n" +
        "  `a` set('1','4','10','21') DEFAULT '1,4,10,21'\n" +
        ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))
    tk.MustExec("insert into t_set value()")
    tk.MustQuery("select * from t_set").Check(testkit.Rows("1,4,10,21"))
}

// TestCreateTableWithEnumCol 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestCreateTableWithEnumCol(t *testing.T) {
#[test]
pub fn TestCreateTableWithEnumCol() {
    store := testkit.CreateMockStore(t, mockstore.WithDDLChecker())

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    // It's for failure cases.
    // The type of default value is string.
    tk.MustExec("drop table if exists t_enum")
    failedSQL := "create table t_enum (a enum('1', '4', '10') default '3');"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrInvalidDefault)
    failedSQL = "create table t_enum (a enum('1', '4', '10') default '');"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrInvalidDefault)
    // The type of default value is int.
    failedSQL = "create table t_enum (a enum('1', '4', '10') default 0);"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrInvalidDefault)
    failedSQL = "create table t_enum (a enum('1', '4', '10') default 8);"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrInvalidDefault)

    // The type of default value is int.
    // It's for successful cases
    tk.MustExec("drop table if exists t_enum")
    tk.MustExec("create table t_enum (a enum('2', '3', '4') default 2);")
    ret := tk.MustQuery("show create table t_enum").Rows()[0][1]
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, strings.Contains(ret.(string), "`a` enum('2','3','4') DEFAULT '3'"))
    tk.MustExec("drop table t_enum")
    tk.MustExec("create table t_enum (a enum('a', 'c', 'd') default 2);")
    ret = tk.MustQuery("show create table t_enum").Rows()[0][1]
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, strings.Contains(ret.(string), "`a` enum('a','c','d') DEFAULT 'c'"))
    tk.MustExec("insert into t_enum value()")
    tk.MustQuery("select * from t_enum").Check(testkit.Rows("c"))
}

// TestCreateTableWithIntegerColWithDefault 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestCreateTableWithIntegerColWithDefault(t *testing.T) {
#[test]
pub fn TestCreateTableWithIntegerColWithDefault() {
    store := testkit.CreateMockStore(t, mockstore.WithDDLChecker())

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    // It's for failure cases.
    tk.MustExec("drop table if exists t1")
    failedSQL := "create table t1 (a tinyint unsigned default -1.25);"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrInvalidDefault)
    failedSQL = "create table t1 (a tinyint default 999999999);"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrInvalidDefault)

    // It's for successful cases
    tk.MustExec("drop table if exists t1")
    tk.MustExec("create table t1 (a tinyint unsigned default 1.25);")
    ret := tk.MustQuery("show create table t1").Rows()[0][1]
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, strings.Contains(ret.(string), "`a` tinyint(3) unsigned DEFAULT '1'"))

    tk.MustExec("drop table t1")
    tk.MustExec("create table t1 (a smallint default -1.25);")
    ret = tk.MustQuery("show create table t1").Rows()[0][1]
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, strings.Contains(ret.(string), "`a` smallint(6) DEFAULT '-1'"))

    tk.MustExec("drop table t1")
    tk.MustExec("create table t1 (a mediumint default 2.8);")
    ret = tk.MustQuery("show create table t1").Rows()[0][1]
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, strings.Contains(ret.(string), "`a` mediumint(9) DEFAULT '3'"))

    tk.MustExec("drop table t1")
    tk.MustExec("create table t1 (a int default -2.8);")
    ret = tk.MustQuery("show create table t1").Rows()[0][1]
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, strings.Contains(ret.(string), "`a` int(11) DEFAULT '-3'"))

    tk.MustExec("drop table t1")
    tk.MustExec("create table t1 (a bigint unsigned default 0.0);")
    ret = tk.MustQuery("show create table t1").Rows()[0][1]
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, strings.Contains(ret.(string), "`a` bigint(20) unsigned DEFAULT '0'"))

    tk.MustExec("drop table t1")
    tk.MustExec("create table t1 (a float default '0012.43');")
    ret = tk.MustQuery("show create table t1").Rows()[0][1]
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, strings.Contains(ret.(string), "`a` float DEFAULT '12.43'"))

    tk.MustExec("drop table t1")
    tk.MustExec("create table t1 (a double default '12.4300');")
    ret = tk.MustQuery("show create table t1").Rows()[0][1]
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, strings.Contains(ret.(string), "`a` double DEFAULT '12.43'"))
}

// TestCreateTableWithInfo 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestCreateTableWithInfo(t *testing.T) {
#[test]
pub fn TestCreateTableWithInfo() {
    store, dom := testkit.CreateMockStoreAndDomain(t)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.Session().SetValue(sessionctx.QueryString, "skip")

    d := dom.DDLExecutor()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NotNil(t, d)
    info := []*model.TableInfo{{
        ID:   42042, // Note, we must ensure the table ID is globally unique!
        Name: ast.NewCIStr("t"),
    }}

    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, d.BatchCreateTableWithInfo(tk.Session(), ast.NewCIStr("test"), info, ddl.WithOnExist(ddl.OnExistError), ddl.WithIDAllocated(true)))
    tk.MustQuery("select tidb_table_id from information_schema.tables where table_name = 't'").Check(testkit.Rows("42042"))
    ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnOthers)

    var id int64
    err := kv.RunInNewTxn(ctx, store, true, func(_ context.Context, txn kv.Transaction) error {
        m := meta.NewMutator(txn)
        var err error
        id, err = m.GenGlobalID()
        return err
    })

    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    info = []*model.TableInfo{{
        ID:   42,
        Name: ast.NewCIStr("tt"),
    }}
    tk.Session().SetValue(sessionctx.QueryString, "skip")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, d.BatchCreateTableWithInfo(tk.Session(), ast.NewCIStr("test"), info, ddl.WithOnExist(ddl.OnExistError)))
    idGen, ok := tk.MustQuery("select tidb_table_id from information_schema.tables where table_name = 'tt'").Rows()[0][0].(string)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, ok)
    idGenNum, err := strconv.ParseInt(idGen, 10, 64)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Greater(t, idGenNum, id)
}

// TestBatchCreateTable 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestBatchCreateTable(t *testing.T) {
#[test]
pub fn TestBatchCreateTable() {
    store, dom := testkit.CreateMockStoreAndDomain(t)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("drop table if exists tables_1")
    tk.MustExec("drop table if exists tables_2")
    tk.MustExec("drop table if exists tables_3")

    d := dom.DDLExecutor()
    infos := []*model.TableInfo{}
    infos = append(infos, &model.TableInfo{
        Name: ast.NewCIStr("tables_1"),
    })
    infos = append(infos, &model.TableInfo{
        Name: ast.NewCIStr("tables_2"),
    })
    infos = append(infos, &model.TableInfo{
        Name: ast.NewCIStr("tables_3"),
    })

    // correct name
    tk.Session().SetValue(sessionctx.QueryString, "skip")
    err := d.BatchCreateTableWithInfo(tk.Session(), ast.NewCIStr("test"), infos, ddl.WithOnExist(ddl.OnExistError))
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)

    tk.MustQuery("show tables like '%tables_%'").Check(testkit.Rows("tables_1", "tables_2", "tables_3"))
    job := tk.MustQuery("admin show ddl jobs").Rows()[0]
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "test", job[1])
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "tables_1,tables_2,tables_3", job[2])
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "create tables", job[3])
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "public", job[4])
    // FIXME: we must change column type to give multiple id
    // c.Assert(job[6], Matches, "[^,]+,[^,]+,[^,]+")

    // duplicated name
    infos[1].Name = ast.NewCIStr("tables_1")
    tk.Session().SetValue(sessionctx.QueryString, "skip")
    err = d.BatchCreateTableWithInfo(tk.Session(), ast.NewCIStr("test"), infos, ddl.WithOnExist(ddl.OnExistError))
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, terror.ErrorEqual(err, infoschema.ErrTableExists))

    newinfo := &model.TableInfo{
        Name: ast.NewCIStr("tables_4"),
    }
    {
        colNum := 2
        cols := make([]*model.ColumnInfo, colNum)
        viewCols := make([]ast.CIStr, colNum)
        var stmtBuffer bytes.Buffer
        stmtBuffer.WriteString("SELECT ")
        // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
        for i := range cols {
            col := &model.ColumnInfo{
                Name:   ast.NewCIStr(fmt.Sprintf("c%d", i+1)),
                Offset: i,
                State:  model.StatePublic,
            }
            cols[i] = col
            viewCols[i] = col.Name
            stmtBuffer.WriteString(cols[i].Name.L + ",")
        }
        stmtBuffer.WriteString("1 FROM t")
        newinfo.Columns = cols
        newinfo.View = &model.ViewInfo{Cols: viewCols, Security: ast.SecurityDefiner, Algorithm: ast.AlgorithmMerge, SelectStmt: stmtBuffer.String(), CheckOption: ast.CheckOptionCascaded, Definer: &auth.UserIdentity{CurrentUser: true}}
    }

    tk.Session().SetValue(sessionctx.QueryString, "skip")
    tk.Session().SetValue(sessionctx.QueryString, "skip")
    err = d.BatchCreateTableWithInfo(tk.Session(), ast.NewCIStr("test"), []*model.TableInfo{newinfo}, ddl.WithOnExist(ddl.OnExistError))
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
}

// port from mysql
// https://github.com/mysql/mysql-server/blob/4f1d7cf5fcb11a3f84cff27e37100d7295e7d5ca/mysql-test/t/tablelock.test
// TestTableLock 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestTableLock(t *testing.T) {
#[test]
pub fn TestTableLock() {
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t1,t2")

    /* Test of lock tables */
    tk.MustExec("create table t1 ( n int auto_increment primary key)")
    tk.MustExec("lock tables t1 write")
    tk.MustExec("insert into t1 values(NULL)")
    tk.MustExec("unlock tables")
    checkTableLock(t, tk, "test", "t1", ast.TableLockNone)

    tk.MustExec("lock tables t1 write")
    tk.MustExec("insert into t1 values(NULL)")
    tk.MustExec("unlock tables")
    checkTableLock(t, tk, "test", "t1", ast.TableLockNone)

    tk.MustExec("drop table if exists t1")

    /* Test of locking and delete of files */
    tk.MustExec("drop table if exists t1,t2")
    tk.MustExec("CREATE TABLE t1 (a int)")
    tk.MustExec("CREATE TABLE t2 (a int)")
    tk.MustExec("lock tables t1 write, t2 write")
    tk.MustExec("drop table t1,t2")

    tk.MustExec("CREATE TABLE t1 (a int)")
    tk.MustExec("CREATE TABLE t2 (a int)")
    tk.MustExec("lock tables t1 write, t2 write")
    tk.MustExec("drop table t2,t1")
}

// port from mysql
// https://github.com/mysql/mysql-server/blob/4f1d7cf5fcb11a3f84cff27e37100d7295e7d5ca/mysql-test/t/lock_tables_lost_commit.test
// TestTableLocksLostCommit 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestTableLocksLostCommit(t *testing.T) {
#[test]
pub fn TestTableLocksLostCommit() {
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)
    tk2 := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk2.MustExec("use test")

    tk.MustExec("DROP TABLE IF EXISTS t1")
    tk.MustExec("CREATE TABLE t1(a INT)")
    tk.MustExec("LOCK TABLES t1 WRITE")
    tk.MustExec("INSERT INTO t1 VALUES(10)")

    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    err := tk2.ExecToErr("SELECT * FROM t1")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, terror.ErrorEqual(err, infoschema.ErrTableLocked))

    tk.Session().Close()

    tk2.MustExec("SELECT * FROM t1")
    tk2.MustExec("DROP TABLE t1")

    tk.MustExec("unlock tables")
}

// checkTableLock 对应 Go 的同名辅助函数，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func checkTableLock(t *testing.T, tk *testkit.TestKit, dbName, tableName string, lockTp ast.TableLockType) {
pub fn checkTableLock() {
    tb := external.GetTableByName(t, tk, dbName, tableName)
    dom := domain.GetDomain(tk.Session())
    err := dom.Reload()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
    if lockTp != ast.TableLockNone {
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.NotNil(t, tb.Meta().Lock)
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, lockTp, tb.Meta().Lock.Tp)
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, model.TableLockStatePublic, tb.Meta().Lock.State)
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.True(t, len(tb.Meta().Lock.Sessions) == 1)
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, dom.DDL().GetID(), tb.Meta().Lock.Sessions[0].ServerID)
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, tk.Session().GetSessionVars().ConnectionID, tb.Meta().Lock.Sessions[0].SessionID)
    } else {
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Nil(t, tb.Meta().Lock)
    }
}

// test write local lock
// TestWriteLocal 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestWriteLocal(t *testing.T) {
#[test]
pub fn TestWriteLocal() {
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)
    tk2 := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk2.MustExec("use test")
    tk.MustExec("drop table if exists t1")
    tk.MustExec("create table t1 ( n int auto_increment primary key)")

    // Test: allow read
    tk.MustExec("lock tables t1 write local")
    tk.MustExec("insert into t1 values(NULL)")
    tk2.MustQuery("select count(*) from t1")
    tk.MustExec("unlock tables")
    tk2.MustExec("unlock tables")

    // Test: forbid write
    tk.MustExec("lock tables t1 write local")
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    err := tk2.ExecToErr("insert into t1 values(NULL)")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, terror.ErrorEqual(err, infoschema.ErrTableLocked))
    tk.MustExec("unlock tables")
    tk2.MustExec("unlock tables")

    // Test mutex: lock write local first
    tk.MustExec("lock tables t1 write local")
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    err = tk2.ExecToErr("lock tables t1 write local")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, terror.ErrorEqual(err, infoschema.ErrTableLocked))
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    err = tk2.ExecToErr("lock tables t1 write")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, terror.ErrorEqual(err, infoschema.ErrTableLocked))
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    err = tk2.ExecToErr("lock tables t1 read")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, terror.ErrorEqual(err, infoschema.ErrTableLocked))
    tk.MustExec("unlock tables")
    tk2.MustExec("unlock tables")

    // Test mutex: lock write first
    tk.MustExec("lock tables t1 write")
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    err = tk2.ExecToErr("lock tables t1 write local")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, terror.ErrorEqual(err, infoschema.ErrTableLocked))
    tk.MustExec("unlock tables")
    tk2.MustExec("unlock tables")

    // Test mutex: lock read first
    tk.MustExec("lock tables t1 read")
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    err = tk2.ExecToErr("lock tables t1 write local")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, terror.ErrorEqual(err, infoschema.ErrTableLocked))
    tk.MustExec("unlock tables")
    tk2.MustExec("unlock tables")
}

// TestLockTables 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestLockTables(t *testing.T) {
#[test]
pub fn TestLockTables() {
    // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
    if kerneltype.IsNextGen() {
        t.Skip("MDL is always enabled and read only in nextgen")
    }
    store := testkit.CreateMockStore(t)
    setTxnTk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    setTxnTk.MustExec("set global tidb_txn_mode=''")
    setTxnTk.MustExec("set global tidb_enable_metadata_lock=0")
    tk := testkit.NewTestKit(t, store)
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t1,t2")
    // Go defer handles resource/config cleanup.
    defer tk.MustExec("drop table if exists t1,t2")
    tk.MustExec("create table t1 (a int)")
    tk.MustExec("create table t2 (a int)")

    // Test lock 1 table.
    tk.MustExec("lock tables t1 write")
    checkTableLock(t, tk, "test", "t1", ast.TableLockWrite)
    // still locked after truncate.
    tk.MustExec("truncate table t1")
    checkTableLock(t, tk, "test", "t1", ast.TableLockWrite)
    // should unlock the new table id.
    tk.MustExec("unlock tables")
    checkTableLock(t, tk, "test", "t1", ast.TableLockNone)
    tk.MustExec("lock tables t1 read")
    checkTableLock(t, tk, "test", "t1", ast.TableLockRead)
    tk.MustExec("lock tables t1 write")
    checkTableLock(t, tk, "test", "t1", ast.TableLockWrite)

    // Test lock multi tables.
    tk.MustExec("lock tables t1 write, t2 read")
    checkTableLock(t, tk, "test", "t1", ast.TableLockWrite)
    checkTableLock(t, tk, "test", "t2", ast.TableLockRead)
    tk.MustExec("lock tables t1 read, t2 write")
    checkTableLock(t, tk, "test", "t1", ast.TableLockRead)
    checkTableLock(t, tk, "test", "t2", ast.TableLockWrite)
    tk.MustExec("lock tables t2 write")
    checkTableLock(t, tk, "test", "t2", ast.TableLockWrite)
    checkTableLock(t, tk, "test", "t1", ast.TableLockNone)
    tk.MustExec("lock tables t1 write")
    checkTableLock(t, tk, "test", "t1", ast.TableLockWrite)
    checkTableLock(t, tk, "test", "t2", ast.TableLockNone)

    tk2 := testkit.NewTestKit(t, store)
    tk2.MustExec("use test")

    // Test read lock.
    tk.MustExec("lock tables t1 read")
    tk.MustQuery("select * from t1")
    tk2.MustQuery("select * from t1")
    tk.MustGetDBError("insert into t1 set a=1", infoschema.ErrTableNotLockedForWrite)
    tk.MustGetDBError("update t1 set a=1", infoschema.ErrTableNotLockedForWrite)
    tk.MustGetDBError("delete from t1", infoschema.ErrTableNotLockedForWrite)

    tk2.MustGetDBError("insert into t1 set a=1", infoschema.ErrTableLocked)
    tk2.MustGetDBError("update t1 set a=1", infoschema.ErrTableLocked)
    tk2.MustGetDBError("delete from t1", infoschema.ErrTableLocked)
    tk2.MustExec("lock tables t1 read")
    tk2.MustGetDBError("insert into t1 set a=1", infoschema.ErrTableNotLockedForWrite)

    // Test write lock.
    tk.MustGetDBError("lock tables t1 write", infoschema.ErrTableLocked)
    tk2.MustExec("unlock tables")
    tk.MustExec("lock tables t1 write")
    tk.MustQuery("select * from t1")
    tk.MustExec("delete from t1")
    tk.MustExec("insert into t1 set a=1")

    tk2.MustGetDBError("select * from t1", infoschema.ErrTableLocked)
    tk2.MustGetDBError("insert into t1 set a=1", infoschema.ErrTableLocked)
    tk2.MustGetDBError("lock tables t1 write", infoschema.ErrTableLocked)

    // Test write local lock.
    tk.MustExec("lock tables t1 write local")
    tk.MustQuery("select * from t1")
    tk.MustExec("delete from t1")
    tk.MustExec("insert into t1 set a=1")

    tk2.MustQuery("select * from t1")
    tk2.MustGetDBError("delete from t1", infoschema.ErrTableLocked)
    tk2.MustGetDBError("insert into t1 set a=1", infoschema.ErrTableLocked)
    tk2.MustGetDBError("lock tables t1 write", infoschema.ErrTableLocked)
    tk2.MustGetDBError("lock tables t1 read", infoschema.ErrTableLocked)

    // Test none unique table.
    tk.MustGetDBError("lock tables t1 read, t1 write", infoschema.ErrNonuniqTable)

    // Test lock table by other session in transaction and commit without retry.
    tk.MustExec("unlock tables")
    tk2.MustExec("unlock tables")
    tk.MustExec("set @@session.tidb_disable_txn_auto_retry=1")
    tk.MustExec("begin")
    tk.MustExec("insert into t1 set a=1")
    tk2.MustExec("lock tables t1 write")
    tk.MustGetErrMsg("commit",
        "previous statement: insert into t1 set a=1: [domain:8028]Information schema is changed during the execution of the statement(for example, table definition may be updated by other DDL ran in parallel). If you see this error often, try increasing `tidb_max_delta_schema_count`. [try again later]")

    // Test lock tables and drop tables
    tk.MustExec("unlock tables")
    tk2.MustExec("unlock tables")
    tk.MustExec("lock tables t1 write, t2 write")
    tk.MustExec("drop table t1")
    tk2.MustExec("create table t1 (a int)")
    tk.MustExec("lock tables t1 write, t2 read")

    // Test lock tables and drop database.
    tk.MustExec("unlock tables")
    tk.MustExec("create database test_lock")
    tk.MustExec("create table test_lock.t3 (a int)")
    tk.MustExec("lock tables t1 write, test_lock.t3 write")
    tk2.MustExec("create table t3 (a int)")
    tk.MustExec("lock tables t1 write, t3 write")
    tk.MustExec("drop table t3")

    // Test lock tables and truncate tables.
    tk.MustExec("unlock tables")
    tk.MustExec("lock tables t1 write, t2 read")
    tk.MustExec("truncate table t1")
    tk.MustExec("insert into t1 set a=1")
    tk2.MustGetDBError("insert into t1 set a=1", infoschema.ErrTableLocked)

    // Test for lock unsupported schema tables.
    tk2.MustGetDBError("lock tables performance_schema.global_status write", infoschema.ErrAccessDenied)
    tk2.MustGetDBError("lock tables information_schema.tables write", infoschema.ErrAccessDenied)
    tk2.MustGetDBError("lock tables mysql.db write", infoschema.ErrAccessDenied)

    // Test create table/view when session is holding the table locks.
    tk.MustExec("unlock tables")
    tk.MustExec("lock tables t1 write, t2 read")
    tk.MustGetDBError("create table t3 (a int)", infoschema.ErrTableNotLocked)
    tk.MustGetDBError("create view v1 as select * from t1;", infoschema.ErrTableNotLocked)

    // Test for locking view was not supported.
    tk.MustExec("unlock tables")
    tk.MustExec("create view v1 as select * from t1;")
    tk.MustGetDBError("lock tables v1 read", table.ErrUnsupportedOp)

    // Test for locking sequence was not supported.
    tk.MustExec("unlock tables")
    tk.MustExec("create sequence seq")
    tk.MustGetDBError("lock tables seq read", table.ErrUnsupportedOp)
    tk.MustExec("drop sequence seq")

    // Test for create/drop/alter database when session is holding the table locks.
    tk.MustExec("unlock tables")
    tk.MustExec("lock table t1 write")
    tk.MustGetDBError("drop database test", table.ErrLockOrActiveTransaction)
    tk.MustGetDBError("create database test_lock", table.ErrLockOrActiveTransaction)
    tk.MustGetDBError("alter database test charset='utf8mb4'", table.ErrLockOrActiveTransaction)
    // Test alter/drop database when other session is holding the table locks of the database.
    tk2.MustExec("create database test_lock2")
    tk2.MustGetDBError("drop database test", infoschema.ErrTableLocked)
    tk2.MustGetDBError("alter database test charset='utf8mb4'", infoschema.ErrTableLocked)

    // Test for admin cleanup table locks.
    tk.MustExec("unlock tables")
    tk.MustExec("lock table t1 write, t2 write")
    tk2.MustGetDBError("lock tables t1 write, t2 read", infoschema.ErrTableLocked)
    tk2.MustExec("admin cleanup table lock t1,t2")
    checkTableLock(t, tk, "test", "t1", ast.TableLockNone)
    checkTableLock(t, tk, "test", "t2", ast.TableLockNone)
    // cleanup unlocked table.
    tk2.MustExec("admin cleanup table lock t1,t2")
    checkTableLock(t, tk, "test", "t1", ast.TableLockNone)
    checkTableLock(t, tk, "test", "t2", ast.TableLockNone)
    tk2.MustExec("lock tables t1 write, t2 read")
    checkTableLock(t, tk2, "test", "t1", ast.TableLockWrite)
    checkTableLock(t, tk2, "test", "t2", ast.TableLockRead)

    tk.MustExec("unlock tables")
    tk2.MustExec("unlock tables")
}

// TestTablesLockDelayClean 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestTablesLockDelayClean(t *testing.T) {
#[test]
pub fn TestTablesLockDelayClean() {
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)
    tk2 := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk2.MustExec("use test")
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t1,t2")
    // Go defer handles resource/config cleanup.
    defer tk.MustExec("drop table if exists t1,t2")
    tk.MustExec("create table t1 (a int)")
    tk.MustExec("create table t2 (a int)")

    tk.MustExec("lock tables t1 write")
    checkTableLock(t, tk, "test", "t1", ast.TableLockWrite)
    config.UpdateGlobal(func(conf *config.Config) {
        conf.DelayCleanTableLock = 100
    })
    // WaitGroup 相关逻辑描述并发测试同步点，当前不启动真实并发执行器。
    var wg util.WaitGroupWrapper
    var startTime time.Time
    // WaitGroup 相关逻辑描述并发测试同步点，当前不启动真实并发执行器。
    wg.Run(func() {
        startTime = time.Now()
        tk.Session().Close()
    })
    // 时间等待用于暴露异步 DDL 状态；保留等待点但不依赖真实时间推进。
    time.Sleep(50 * time.Millisecond)
    checkTableLock(t, tk, "test", "t1", ast.TableLockWrite)
    // WaitGroup 相关逻辑描述并发测试同步点，当前不启动真实并发执行器。
    wg.Wait()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, time.Since(startTime).Seconds() > 0.1)
    checkTableLock(t, tk, "test", "t1", ast.TableLockNone)
    config.UpdateGlobal(func(conf *config.Config) {
        conf.DelayCleanTableLock = 0
    })
}

// TestAddColumn2 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestAddColumn2(t *testing.T) {
#[test]
pub fn TestAddColumn2() {
    store, dom := testkit.CreateMockStoreAndDomain(t)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t1")
    tk.MustExec("create table t1 (a int key, b int);")
    // Go defer handles resource/config cleanup.
    defer tk.MustExec("drop table if exists t1, t2")

    var writeOnlyTable table.Table
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if job.SchemaState == model.StateWriteOnly {
            writeOnlyTable, _ = dom.InfoSchema().TableByID(context.Background(), job.TableID)
        }
    })
    done := make(chan error, 1)
    // test transaction on add column.
    // Go 这里启动 goroutine；保留并发触发点，不真正调度异步任务。
    go backgroundExec(store, "test", "alter table t1 add column c int not null", done)
    err := <-done
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)

    tk.MustExec("insert into t1 values (1,1,1)")
    tk.MustQuery("select a,b,c from t1").Check(testkit.Rows("1 1 1"))

    // mock for outdated tidb update record.
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NotNil(t, writeOnlyTable)
    ctx := context.Background()
    txn, err := newTxn(tk.Session())
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    oldRow, err := tables.RowWithCols(writeOnlyTable, tk.Session(), kv.IntHandle(1), writeOnlyTable.WritableCols())
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, 3, len(oldRow))
    err = writeOnlyTable.RemoveRecord(tk.Session().GetTableCtx(), txn, kv.IntHandle(1), oldRow)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    _, err = writeOnlyTable.AddRecord(tk.Session().GetTableCtx(), txn, types.MakeDatums(oldRow[0].GetInt64(), 2, oldRow[2].GetInt64()), table.IsUpdate)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    tk.Session().StmtCommit(ctx)
    err = tk.Session().CommitTxn(ctx)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)

    tk.MustQuery("select a,b,c from t1").Check(testkit.Rows("1 2 1"))

    // Test for _tidb_rowid
    var re *testkit.Result
    tk.MustExec("create table t2 (a int);")
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if job.SchemaState != model.StateWriteOnly {
            return
        }
        // allow write _tidb_rowid first
        tk2 := testkit.NewTestKit(t, store)
        tk2.MustExec("use test")
        tk2.MustExec("set @@tidb_opt_write_row_id=1")
        tk2.MustExec("begin")
        tk2.MustExec("insert into t2 (a,_tidb_rowid) values (1,2);")
        re = tk2.MustQuery(" select a,_tidb_rowid from t2;")
        tk2.MustExec("commit")
    })

    // Go 这里启动 goroutine；保留并发触发点，不真正调度异步任务。
    go backgroundExec(store, "test", "alter table t2 add column b int not null default 3", done)
    err = <-done
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    re.Check(testkit.Rows("1 2"))
    tk.MustQuery("select a,b,_tidb_rowid from t2").Check(testkit.Rows("1 3 2"))
    // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
    testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep")
}

// TestDropTables 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestDropTables(t *testing.T) {
#[test]
pub fn TestDropTables() {
    store := testkit.CreateMockStore(t, mockstore.WithDDLChecker())

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t1;")

    failedSQL := "drop table t1;"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrBadTable)
    failedSQL = "drop table test2.t1;"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrBadTable)

    tk.MustExec("create table t1 (a int);")
    tk.MustExec("drop table if exists t1, t2;")

    tk.MustExec("create table t1 (a int);")
    tk.MustExec("drop table if exists t2, t1;")

    // Without IF EXISTS, the statement drops all named tables that do exist, and returns an error indicating which
    // nonexisting tables it was unable to drop.
    // https://dev.mysql.com/doc/refman/5.7/en/drop-table.html
    tk.MustExec("create table t1 (a int);")
    failedSQL = "drop table t1, t2;"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrBadTable)

    tk.MustExec("create table t1 (a int);")
    failedSQL = "drop table t2, t1;"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrBadTable)

    failedSQL = "show create table t1;"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrNoSuchTable)
}

// TestCreateConstraintForTable 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestCreateConstraintForTable(t *testing.T) {
#[test]
pub fn TestCreateConstraintForTable() {
    store := testkit.CreateMockStore(t, mockstore.WithDDLChecker())

    tk := testkit.NewTestKit(t, store)

    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("DROP TABLE IF EXISTS t1, t2")
    tk.MustExec("set @@global.tidb_enable_check_constraint = 1")
    tk.MustExec("CREATE TABLE t1 (id INT PRIMARY KEY, CONSTRAINT c1 CHECK (id<50))")
    failedSQL := "CREATE TABLE t2 (id INT PRIMARY KEY, CONSTRAINT c1 CHECK (id<50))"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrCheckConstraintDupName)

    tk.MustExec("CREATE TABLE t2 (id INT PRIMARY KEY)")
    failedSQL = "ALTER TABLE t2 ADD CONSTRAINT c1 CHECK (id<50)"
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode(failedSQL, errno.ErrCheckConstraintDupName)

    tk.MustExec("DROP DATABASE IF EXISTS test2")
    tk.MustExec("CREATE DATABASE test2")
    tk.MustExec("CREATE TABLE test2.t1 (id INT PRIMARY KEY, CONSTRAINT c1 CHECK (id<50))")
    rs, err := tk.Exec("SHOW TABLES FROM test2 LIKE 't1'")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, tk.ResultSetToResult(rs, "").Rows()[0][0], "t1")
}

// TestCreateTableHandleAutoIDOnce 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestCreateTableHandleAutoIDOnce(t *testing.T) {
#[test]
pub fn TestCreateTableHandleAutoIDOnce() {
    store := testkit.CreateMockStore(t)

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")

    count := 0
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/handleAutoIncID", func() {
        count++
    })

    tk.MustExec("create table t1(id int) AUTO_INCREMENT 1000")

    // For normal DDL, rebase should be called only once.
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, 1, count)
    rs := tk.MustQuery("show table test.t1 next_row_id").Rows()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
require.Equal(t, "1000", rs[0][3])
}
*/

use std::collections::BTreeSet;
use std::time::Duration;

use crate::executor::{
    ColumnInfo, ColumnKind, Executor, ExecutorError, Ident, MemoryJobBackend, OnExist,
    SessionContext, TableInfo, TableLockType,
};

/// 构造一个整数类型的测试列定义。
///
/// id 置 0 表示由执行器在建表时统一分配；charset/collation 使用 binary
/// 表示按原始字节比较，不做字符集转换。
fn column(name: &str) -> ColumnInfo {
    ColumnInfo {
        id: 0,
        name: name.into(),
        kind: ColumnKind::Integer,
        charset: "binary".into(),
        collation: "binary".into(),
        nullable: true,
        hidden: false,
        generated_dependencies: BTreeSet::new(),
        masking_policy: None,
    }
}

/// 构造只含一个 `id` 列的最小测试表定义。
///
/// 各种可选特性（分区、TTL、TiFlash 副本、放置策略等）均取空值/默认值，
/// id 与 schema_id 置 0 表示由执行器分配全局唯一 ID。
fn table(name: &str) -> TableInfo {
    TableInfo {
        id: 0,
        schema_id: 0,
        name: name.into(),
        charset: "utf8mb4".into(),
        collation: "utf8mb4_bin".into(),
        columns: vec![column("id")],
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        partitions: Vec::new(),
        auto_increment: 0,
        auto_random_bits: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        temporary: false,
        view: false,
        sequence: false,
        cached: false,
        tiflash_replica_count: 0,
        tiflash_available_ids: BTreeSet::new(),
        placement_policy: None,
        affinity: None,
        ttl_column: None,
        table_lock: None,
    }
}

/// 创建基于内存作业后端的 DDL 执行器与默认会话上下文。
///
/// `Duration::ZERO` 表示 DDL 作业无人工延迟，测试同步完成；
/// `SessionContext` 记录会话级状态（如当前持有的表锁）。
fn executor() -> (Executor<MemoryJobBackend>, SessionContext) {
    (
        Executor::new(MemoryJobBackend::default(), Duration::ZERO),
        SessionContext::default(),
    )
}

/// 验证建表会分配正整数表 ID，并遵循 OnExist（同名表存在时）策略：
/// Error 模式返回 TableExists 错误，Ignore 模式返回已有表的 ID。
#[test]
fn create_table_allocates_ids_and_honours_on_exist() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let id = ddl
        .create_table(&mut session, "test", table("t"), OnExist::Error)
        .unwrap();
    assert!(id > 0);
    assert!(ddl.table_exists(&Ident::new("test", "t")));
    // 重复建同名表：Error 策略必须报 TableExists。
    assert!(matches!(
        ddl.create_table(&mut session, "test", table("t"), OnExist::Error),
        Err(ExecutorError::TableExists(_))
    ));
    // Ignore 策略应静默返回原表 ID，不新建。
    assert_eq!(
        id,
        ddl.create_table(&mut session, "test", table("t"), OnExist::Ignore)
            .unwrap()
    );
}

/// 对应 Go `TestCreateTableWithInfo` 的 ID 契约：调用方已分配的全局表 ID
/// 必须原样写入目录；未指定 ID 的表仍由执行器分配非零 ID。
#[test]
fn create_table_preserves_preallocated_id() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();

    let mut preallocated = table("preallocated");
    preallocated.id = 42_042;
    assert_eq!(
        42_042,
        ddl.create_table(&mut session, "test", preallocated, OnExist::Error)
            .unwrap()
    );

    let generated = ddl
        .create_table(&mut session, "test", table("generated"), OnExist::Error)
        .unwrap();
    assert_ne!(0, generated);
    assert_ne!(42_042, generated);
}

/// 对应 Go `TestBatchCreateTable`：成功批量创建全部表；批内名称按
/// 大小写不敏感规则查重，并在提交任何表之前拒绝整批请求。
#[test]
fn batch_create_tables_creates_all_and_rejects_duplicate_names_atomically() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();

    let ids = ddl
        .batch_create_tables(
            &mut session,
            "test",
            vec![table("tables_1"), table("tables_2"), table("tables_3")],
            OnExist::Error,
        )
        .unwrap();
    assert_eq!(3, ids.len());
    assert!(ids.iter().all(|id| *id > 0));
    assert!(ddl.table_exists(&Ident::new("test", "tables_1")));
    assert!(ddl.table_exists(&Ident::new("test", "tables_2")));
    assert!(ddl.table_exists(&Ident::new("test", "tables_3")));

    assert!(matches!(
        ddl.batch_create_tables(
            &mut session,
            "test",
            vec![table("new_table"), table("NEW_TABLE")],
            OnExist::Error,
        ),
        Err(ExecutorError::TableExists(_))
    ));
    assert!(!ddl.table_exists(&Ident::new("test", "new_table")));
}

/// 验证列名查重不区分大小写：`id` 与 `ID` 视为重复列，建表应报 ColumnExists。
/// 这与 MySQL 列名大小写不敏感的语义一致。
#[test]
fn table_definition_rejects_case_insensitive_duplicate_columns() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    // table("...") 已含小写 "id" 列，再追加大写 "ID" 构成大小写冲突。
    let mut invalid = table("duplicate_columns");
    invalid.columns.push(column("ID"));
    assert!(matches!(
        ddl.create_table(&mut session, "test", invalid, OnExist::Error),
        Err(ExecutorError::ColumnExists(_))
    ));
}

/// 验证 truncate（截断表，等价于删表重建）后会话持有的表锁迁移：
/// 截断会分配新的表 ID，旧 ID 上的锁应转移到新 ID 上，锁类型保持不变。
#[test]
fn truncate_table_moves_session_lock_to_new_table_id() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let old_id = ddl
        .create_table(&mut session, "test", table("t"), OnExist::Error)
        .unwrap();
    // 模拟当前会话在旧表 ID 上持有读锁。
    session.locked_tables.insert(old_id, TableLockType::Read);
    let new_id = ddl
        .truncate_table(&mut session, &Ident::new("test", "t"))
        .unwrap();
    // 截断必须生成新表 ID，且锁记录随之迁移。
    assert_ne!(old_id, new_id);
    assert!(!session.locked_tables.contains_key(&old_id));
    assert_eq!(
        Some(&TableLockType::Read),
        session.locked_tables.get(&new_id)
    );
}

/// 验证删除对象前的类型检查：对普通表执行 DROP VIEW（is_view=true）
/// 应报 Unsupported 且不删除元数据；按表删除（is_view=false）才能成功。
#[test]
fn drop_view_checks_object_kind_before_removing_metadata() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_table(&mut session, "test", table("t"), OnExist::Error)
        .unwrap();
    let ident = Ident::new("test", "t");
    // 以视图身份删除普通表：应被拒绝，表元数据保持不变。
    assert!(matches!(
        ddl.drop_table(&mut session, &ident, false, true),
        Err(ExecutorError::Unsupported(_))
    ));
    assert!(ddl.table_exists(&ident));
    // 以表身份删除：成功并移除元数据。
    ddl.drop_table(&mut session, &ident, false, false).unwrap();
    assert!(!ddl.table_exists(&ident));
}

/// 验证 DDL 作业生命周期：建库 + 建表共产生两条历史作业记录，
/// 且全部到达 Synced（已同步）终态，表示各节点元数据版本已一致。
#[test]
fn created_tables_are_public_and_jobs_reach_synced_history() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let mut info = table("t");
    info.columns[0].hidden = false;
    ddl.create_table(&mut session, "test", info, OnExist::Error)
        .unwrap();
    // 两条作业：create schema 与 create table，均应进入历史队列。
    assert_eq!(2, ddl.backend().history().len());
    assert!(
        ddl.backend()
            .history()
            .iter()
            .all(|job| job.state == crate::executor::JobState::Synced)
    );
}

/*
// TestCreateTableWithBR 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestCreateTableWithBR(t *testing.T) {
#[test]
pub fn TestCreateTableWithBR() {
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ddl/mockBRStartMode", "return(true)")
    store, dom := testkit.CreateMockStoreAndDomain(t)

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")

    count := 0
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/handleAutoIncID", func() {
        count++
    })

    tblInfo := &model.TableInfo{
        ID:   42043,
        Name: ast.NewCIStr("t1"),
        Columns: []*model.ColumnInfo{
            {
                ID:        1,
                Name:      ast.NewCIStr("id"),
                Offset:    0,
                State:     model.StatePublic,
                FieldType: *types.NewFieldType(mysql.TypeLonglong),
            },
        },
        State:     model.StatePublic,
        AutoIncID: 1000,
    }

    involvingRef := []model.InvolvingSchemaInfo{{
        Database: "test",
        Table:    "t1",
        Mode:     model.SharedInvolving,
    }}

    // Mock BR scenario, rebase should be called twice.
    count = 0
    se := tk.Session()
    se.SetValue(sessionctx.QueryString, "skip")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, dom.DDLExecutor().CreateTableWithInfo(
        se, ast.NewCIStr("test"), tblInfo, involvingRef,
        ddl.WithOnExist(ddl.OnExistError)))

    // For BR execution, rebase should be called twice. And this won't affect the rebase result.
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, 2, count)
    rs := tk.MustQuery("show table test.t1 next_row_id").Rows()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "1000", rs[0][3])
}
*/
