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

// DDL（数据定义语言，如 CREATE/ALTER/DROP）数据库级测试模块。
//
// 本文件由 Go(TiDB) 的 `db_test.go` 机械迁移而来，分为两部分：
// - 顶部大段注释块：保留原 Go 测试（时区处理、SchemaValidator 校验器、
//   MDL 元数据锁、DDL job 管理、failpoint 故障注入、系统表元数据检查等场景），
//   带中文说明，等待后续 Rust 化接线；
// - 尾部可运行的 Rust 测试：针对 `crate::executor` 中的 DDL 执行器，
//   覆盖 schema（数据库）的创建、字符集/排序规则修改、放置策略、
//   删除与恢复，以及 DDL job（DDL 任务，异步执行的变更单元）的历史记录顺序。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

/*
// DDL 数据库级测试、时区/SchemaValidator/MDL/DDL job 管理、failpoint 和系统表元数据检查流程。
// 主要类型、函数、测试场景和辅助函数按 Go 文件顺序保留；关键分支、资源收尾、错误路径、并发和外部依赖在附近用中文说明。
// Go imports（保留依赖边界，待 Rust crate 接线）：
// - "context"
// - "fmt"
// - "math"
// - "math/rand"
// - "slices"
// - "strconv"
// - "strings"
// - "sync"
// - "testing"
// - "time"
// - "github.com/pingcap/errors"
// - "github.com/pingcap/failpoint"
// - "github.com/pingcap/tidb/pkg/config"
// - "github.com/pingcap/tidb/pkg/config/kerneltype"
// - "github.com/pingcap/tidb/pkg/ddl"
// - "github.com/pingcap/tidb/pkg/ddl/schemaver"
// - "github.com/pingcap/tidb/pkg/ddl/testutil"
// - ddlutil "github.com/pingcap/tidb/pkg/ddl/util"
// - "github.com/pingcap/tidb/pkg/domain"
// - "github.com/pingcap/tidb/pkg/errno"
// - "github.com/pingcap/tidb/pkg/infoschema"
// - "github.com/pingcap/tidb/pkg/infoschema/validatorapi"
// - "github.com/pingcap/tidb/pkg/kv"
// - "github.com/pingcap/tidb/pkg/meta"
// - "github.com/pingcap/tidb/pkg/meta/model"
// - "github.com/pingcap/tidb/pkg/parser/ast"
// - "github.com/pingcap/tidb/pkg/parser/auth"
// - "github.com/pingcap/tidb/pkg/parser/charset"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/parser/terror"
// - parsertypes "github.com/pingcap/tidb/pkg/parser/types"
// - "github.com/pingcap/tidb/pkg/session/sessmgr"
// - "github.com/pingcap/tidb/pkg/sessionctx/vardef"
// - "github.com/pingcap/tidb/pkg/sessionctx/variable"
// - "github.com/pingcap/tidb/pkg/sessiontxn"
// - "github.com/pingcap/tidb/pkg/testkit"
// - "github.com/pingcap/tidb/pkg/testkit/external"
// - "github.com/pingcap/tidb/pkg/testkit/testfailpoint"
// - "github.com/pingcap/tidb/pkg/util/dbterror"
// - "github.com/pingcap/tidb/pkg/util/mock"
// - "github.com/pingcap/tidb/pkg/util/sqlexec"
// - "github.com/pingcap/tidb/pkg/util/timeutil"
// - "github.com/stretchr/testify/assert"
// - "github.com/stretchr/testify/require"
// - "github.com/tikv/client-go/v2/oracle"
// - "github.com/tikv/client-go/v2/tikv"

// 下列常量沿用 Go 测试配置，保持字面值和声明顺序。
const (
    // waitForCleanDataRound indicates how many times should we check data is cleaned or not.
    waitForCleanDataRound = 150
    // waitForCleanDataInterval is a min duration between 2 check for data clean.
    waitForCleanDataInterval = time.Millisecond * 100
)

// 下列常量沿用 Go 测试配置，保持字面值和声明顺序。
const defaultBatchSize = 1024

// 下列常量沿用 Go 测试配置，保持字面值和声明顺序。
const dbTestLease = 600 * time.Millisecond

// TestGetTimeZone 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestGetTimeZone(t *testing.T) {
#[test]
pub fn TestGetTimeZone() {
    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")

    systemTimeZone := timeutil.SystemLocation().String()

    testCases := []struct {
        tzSQL  string
        tzStr  string
        tzName string
        offset int
        err    string
    }{
        {"set time_zone = '+00:00'", "", "", 0, ""},
        {"set time_zone = '-00:00'", "", "", 0, ""},
        {"set time_zone = 'UTC'", "UTC", "UTC", 0, ""},
        {"set time_zone = '+05:00'", "", "", 18000, ""},
        {"set time_zone = '-08:00'", "", "", -28800, ""},
        {"set time_zone = '+08:00'", "", "", 28800, ""},
        {"set time_zone = 'Asia/Shanghai'", "Asia/Shanghai", "Asia/Shanghai", 0, ""},
        {"set time_zone = 'SYSTEM'", systemTimeZone, systemTimeZone, 0, ""},
        {"set time_zone = DEFAULT", systemTimeZone, systemTimeZone, 0, ""},
        {"set time_zone = 'GMT'", "GMT", "GMT", 0, ""},
        {"set time_zone = 'GMT+1'", "GMT", "GMT", 0, "[variable:1298]Unknown or incorrect time zone: 'GMT+1'"},
        {"set time_zone = 'Etc/GMT+12'", "Etc/GMT+12", "Etc/GMT+12", 0, ""},
        {"set time_zone = 'Etc/GMT-12'", "Etc/GMT-12", "Etc/GMT-12", 0, ""},
        {"set time_zone = 'EST'", "EST", "EST", 0, ""},
        {"set time_zone = 'Australia/Lord_Howe'", "Australia/Lord_Howe", "Australia/Lord_Howe", 0, ""},
    }
    // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
    for _, tc := range testCases {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if tc.err != "" {
            tk.MustGetErrMsg(tc.tzSQL, tc.err)
        } else {
            tk.MustExec(tc.tzSQL)
        }
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, tc.tzStr, tk.Session().GetSessionVars().TimeZone.String(), fmt.Sprintf("sql: %s", tc.tzSQL))
        tz, offset := ddlutil.GetTimeZone(tk.Session())
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, tz, tc.tzName, fmt.Sprintf("sql: %s, offset: %d", tc.tzSQL, offset))
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, offset, tc.offset, fmt.Sprintf("sql: %s", tc.tzSQL))
    }
}

// TestIssue22819 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestIssue22819(t *testing.T) {
#[test]
pub fn TestIssue22819() {
    // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
    if kerneltype.IsNextGen() {
        t.Skip("MDL is always enabled and read only in nextgen")
    }
    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)

    tk1 := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk1.MustExec("set global tidb_enable_metadata_lock=0")
    tk1.MustExec("use test;")
    tk1.MustExec("create table t1 (v int) partition by hash (v) partitions 2")
    tk1.MustExec("insert into t1 values (1)")

    tk2 := testkit.NewTestKit(t, store)
    tk2.MustExec("use test;")
    tk1.MustExec("begin")
    tk1.MustExec("update t1 set v = 2 where v = 1")

    tk2.MustExec("alter table t1 truncate partition p0")

    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    err := tk1.ExecToErr("commit")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Error(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Regexp(t, ".*8028.*Information schema is changed during the execution of the statement.*", err.Error())
}

// TestIssue22307 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestIssue22307(t *testing.T) {
#[test]
pub fn TestIssue22307() {
    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t")
    tk.MustExec("create table t (a int, b int)")
    tk.MustExec("insert into t values(1, 1);")

    var checkErr1, checkErr2 error
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if job.SchemaState != model.StateWriteOnly {
            return
        }
        _, checkErr1 = tk.Exec("update t set a = 3 where b = 1;")
        _, checkErr2 = tk.Exec("update t set a = 3 order by b;")
    })
    done := make(chan error, 1)
    // test transaction on add column.
    // Go 这里启动 goroutine；保留并发触发点，不真正调度异步任务。
    go backgroundExec(store, "test", "alter table t drop column b;", done)
    err := <-done
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.EqualError(t, checkErr1, "[planner:1054]Unknown column 'b' in 'where clause'")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.EqualError(t, checkErr2, "[planner:1054]Unknown column 'b' in 'order clause'")
}

// TestAddExpressionIndexRollback 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestAddExpressionIndexRollback(t *testing.T) {
#[test]
pub fn TestAddExpressionIndexRollback() {
    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t1 (c1 int, c2 int, c3 int, unique key(c1))")
    tk.MustExec("insert into t1 values (20, 20, 20), (40, 40, 40), (80, 80, 80), (160, 160, 160);")

    var checkErr error
    tk1 := testkit.NewTestKit(t, store)
    tk1.MustExec("use test")

    var currJob *model.Job
    ctx := mock.NewContext()
    ctx.Store = store
    times := 0
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if checkErr != nil {
            return
        }
        switch job.SchemaState {
        case model.StateDeleteOnly:
            _, checkErr = tk1.Exec("insert into t1 values (6, 3, 3) on duplicate key update c1 = 10")
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if checkErr == nil {
                _, checkErr = tk1.Exec("update t1 set c1 = 7 where c2=6;")
            }
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if checkErr == nil {
                _, checkErr = tk1.Exec("delete from t1 where c1 = 40;")
            }
        case model.StateWriteOnly:
            _, checkErr = tk1.Exec("insert into t1 values (2, 2, 2)")
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if checkErr == nil {
                _, checkErr = tk1.Exec("update t1 set c1 = 3 where c2 = 80")
            }
        case model.StateWriteReorganization:
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if checkErr == nil && job.SchemaState == model.StateWriteReorganization && times == 0 {
                _, checkErr = tk1.Exec("insert into t1 values (4, 4, 4)")
                // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
                if checkErr != nil {
                    return
                }
                _, checkErr = tk1.Exec("update t1 set c1 = 5 where c2 = 80")
                // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
                if checkErr != nil {
                    return
                }
                currJob = job
                times++
            }
        }
    })

    tk.MustGetErrMsg("alter table t1 add index expr_idx ((pow(c1, c2)));", "[types:1690]DOUBLE value is out of range in 'pow(160, 160)'")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, checkErr)
    tk.MustQuery("select * from t1 order by c1;").Check(testkit.Rows("2 2 2", "4 4 4", "5 80 80", "10 3 3", "20 20 20", "160 160 160"))

    // Check whether the reorg information is cleaned up.
    err := sessiontxn.NewTxn(context.Background(), ctx)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    element, start, end, physicalID, err := ddl.NewReorgHandlerForTest(testkit.NewTestKit(t, store).Session()).GetDDLReorgHandle(currJob)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, meta.ErrDDLReorgElementNotExist.Equal(err))
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Nil(t, element)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Nil(t, start)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Nil(t, end)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, int64(0), physicalID)
}

// TestDropTableOnTiKVDiskFull 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestDropTableOnTiKVDiskFull(t *testing.T) {
#[test]
pub fn TestDropTableOnTiKVDiskFull() {
    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table test_disk_full_drop_table(a int);")
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/mockstore/unistore/rpcTiKVAllowedOnAlmostFull", `return(true)`))
    // Go defer handles resource/config cleanup.
    defer func() {
        // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
        require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/mockstore/unistore/rpcTiKVAllowedOnAlmostFull"))
    }()
    tk.MustExec("drop table test_disk_full_drop_table;")
}

// TestRebaseAutoID 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestRebaseAutoID(t *testing.T) {
#[test]
pub fn TestRebaseAutoID() {
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/meta/autoid/mockAutoIDChange", `return(true)`))
    // Go defer handles resource/config cleanup.
    defer func() {
        // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
        require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/meta/autoid/mockAutoIDChange"))
    }()

    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)
    tk := testkit.NewTestKit(t, store)

    // testkit SQL via mock store/domain.
    tk.MustExec("drop database if exists tidb;")
    tk.MustExec("create database tidb;")
    tk.MustExec("use tidb;")
    tk.MustExec("create table tidb.test (a int auto_increment primary key, b int);")
    tk.MustExec("insert tidb.test values (null, 1);")
    tk.MustQuery("select * from tidb.test").Check(testkit.Rows("1 1"))
    tk.MustExec("alter table tidb.test auto_increment = 6000;")
    tk.MustExec("insert tidb.test values (null, 1);")
    tk.MustQuery("select * from tidb.test").Check(testkit.Rows("1 1", "6000 1"))
    tk.MustExec("alter table tidb.test auto_increment = 5;")
    tk.MustExec("insert tidb.test values (null, 1);")
    tk.MustQuery("select * from tidb.test").Check(testkit.Rows("1 1", "6000 1", "11000 1"))

    // Current range for table test is [11000, 15999].
    // Though it does not have a tuple "a = 15999", its global next auto increment id should be 16000.
    // Anyway it is not compatible with MySQL.
    tk.MustExec("alter table tidb.test auto_increment = 12000;")
    tk.MustExec("insert tidb.test values (null, 1);")
    tk.MustQuery("select * from tidb.test").Check(testkit.Rows("1 1", "6000 1", "11000 1", "16000 1"))

    tk.MustExec("create table tidb.test2 (a int);")
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode("alter table tidb.test2 add column b int auto_increment key, auto_increment=10;", errno.ErrUnsupportedDDLOperation)
}

// TestProcessColumnFlags 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestProcessColumnFlags(t *testing.T) {
#[test]
pub fn TestProcessColumnFlags() {
    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    // check `processColumnFlags()`
    tk.MustExec("create table t(a year(4) comment 'xxx', b year, c bit)")
    // Go defer handles resource/config cleanup.
    defer tk.MustExec("drop table t;")

    check := func(n string, f func(uint) bool) {
        tbl := external.GetTableByName(t, tk, "test", "t")
        // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
        for _, col := range tbl.Cols() {
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if strings.EqualFold(col.Name.L, n) {
                // Go require/assert 断言描述预期结果；不声明当前可运行。
                require.True(t, f(col.GetFlag()))
                break
            }
        }
    }

    yearcheck := func(f uint) bool {
        return mysql.HasUnsignedFlag(f) && mysql.HasZerofillFlag(f) && !mysql.HasBinaryFlag(f)
    }

    tk.MustExec("alter table t modify a year(4)")
    check("a", yearcheck)

    tk.MustExec("alter table t modify a year(4) unsigned")
    check("a", yearcheck)

    tk.MustExec("alter table t modify a year(4) zerofill")

    tk.MustExec("alter table t modify b year")
    check("b", yearcheck)

    tk.MustExec("alter table t modify c bit")
    check("c", func(f uint) bool {
        return mysql.HasUnsignedFlag(f) && !mysql.HasBinaryFlag(f)
    })
}

// TestForbidCacheTableForSystemTable 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestForbidCacheTableForSystemTable(t *testing.T) {
#[test]
pub fn TestForbidCacheTableForSystemTable() {
    store, dom := testkit.CreateMockStoreAndDomainWithSchemaLease(t, dbTestLease)
    tk := testkit.NewTestKit(t, store)
    sysTables := make([]string, 0, 24)
    memOrSysDB := []string{"MySQL", "INFORMATION_SCHEMA", "PERFORMANCE_SCHEMA", "METRICS_SCHEMA", "SYS"}
    // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
    for _, db := range memOrSysDB {
        // testkit SQL via mock store/domain.
        tk.MustExec("use " + db)
        tk.Session().Auth(&auth.UserIdentity{Username: "root", Hostname: "%"}, nil, nil, nil)
        rows := tk.MustQuery("show tables").Rows()
        // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
        for i := range rows {
            sysTables = append(sysTables, rows[i][0].(string))
        }
        // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
        for _, one := range sysTables {
            // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
            err := tk.ExecToErr(fmt.Sprintf("alter table `%s` cache", one))
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if db == "MySQL" || db == "SYS" {
                tbl, err1 := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr(db), ast.NewCIStr(one))
                // Go require/assert 断言描述预期结果；不声明当前可运行。
                require.NoError(t, err1)
                // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
                if tbl.Meta().View != nil {
                    // Go require/assert 断言描述预期结果；不声明当前可运行。
                    require.ErrorIs(t, err, dbterror.ErrWrongObject)
                } else {
                    // Go require/assert 断言描述预期结果；不声明当前可运行。
                    require.EqualError(t, err, "[ddl:8200]ALTER table cache for tables in system database is currently unsupported")
                }
            } else {
                // Go require/assert 断言描述预期结果；不声明当前可运行。
                require.EqualError(t, err, fmt.Sprintf("[planner:1142]ALTER command denied to user 'root'@'%%' for table '%s'", strings.ToLower(one)))
            }
        }
        sysTables = sysTables[:0]
    }
}

// TestAlterShardRowIDBits 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestAlterShardRowIDBits(t *testing.T) {
#[test]
pub fn TestAlterShardRowIDBits() {
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/meta/autoid/mockAutoIDChange", `return(true)`))
    // Go defer handles resource/config cleanup.
    defer func() {
        // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
        require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/meta/autoid/mockAutoIDChange"))
    }()

    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)
    tk := testkit.NewTestKit(t, store)

    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    // Test alter shard_row_id_bits
    tk.MustExec("create table t1 (a int) shard_row_id_bits = 5")
    tk.MustExec(fmt.Sprintf("alter table t1 auto_increment = %d;", 1<<56))
    tk.MustExec("insert into t1 set a=1;")

    // Test increase shard_row_id_bits failed by overflow global auto ID.
    tk.MustGetErrMsg("alter table t1 SHARD_ROW_ID_BITS = 10;", "[autoid:1467]shard_row_id_bits 10 will cause next global auto ID 72057594037932936 overflow")

    // Test reduce shard_row_id_bits will be ok.
    tk.MustExec("alter table t1 SHARD_ROW_ID_BITS = 3;")
    checkShardRowID := func(maxShardRowIDBits, shardRowIDBits uint64) {
        tbl := external.GetTableByName(t, tk, "test", "t1")
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.True(t, tbl.Meta().MaxShardRowIDBits == maxShardRowIDBits)
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.True(t, tbl.Meta().ShardRowIDBits == shardRowIDBits)
    }
    checkShardRowID(5, 3)

    // Test reduce shard_row_id_bits but calculate overflow should use the max record shard_row_id_bits.
    tk.MustExec("drop table if exists t1")
    tk.MustExec("create table t1 (a int) shard_row_id_bits = 10")
    tk.MustExec("alter table t1 SHARD_ROW_ID_BITS = 5;")
    checkShardRowID(10, 5)
    tk.MustExec(fmt.Sprintf("alter table t1 auto_increment = %d;", 1<<56))
    tk.MustGetErrMsg("insert into t1 set a=1;", "[autoid:1467]Failed to read auto-increment value from storage engine")
}

// TestDDLJobErrorCount 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestDDLJobErrorCount(t *testing.T) {
#[test]
pub fn TestDDLJobErrorCount() {
    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("drop table if exists ddl_error_table, new_ddl_error_table")
    tk.MustExec("create table ddl_error_table(a int)")

    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/mockErrEntrySizeTooLarge", `return(true)`))
    // Go defer handles resource/config cleanup.
    defer func() {
        // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
        require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/mockErrEntrySizeTooLarge"))
    }()

    var jobID int64
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", func(job *model.Job) {
        jobID = job.ID
    })

    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode("rename table ddl_error_table to new_ddl_error_table", errno.ErrEntryTooLarge)

    historyJob, err := ddl.GetHistoryJobByID(tk.Session(), jobID)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NotNil(t, historyJob)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, int64(1), historyJob.ErrorCount)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, kv.ErrEntryTooLarge.Equal(historyJob.Error))
    tk.MustQuery("select * from ddl_error_table;").Check(testkit.Rows())
}

// TestAddIndexFailOnCaseWhenCanExit is used to close #19325.
// TestAddIndexFailOnCaseWhenCanExit 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestAddIndexFailOnCaseWhenCanExit(t *testing.T) {
#[test]
pub fn TestAddIndexFailOnCaseWhenCanExit() {
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/MockCaseWhenParseFailure", `return(true)`))
    // Go defer handles resource/config cleanup.
    defer func() {
        // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
        require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/MockCaseWhenParseFailure"))
    }()
    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)
    tk := testkit.NewTestKit(t, store)
    originalVal := vardef.GetDDLErrorCountLimit()
    // testkit SQL via mock store/domain.
    tk.MustExec("set @@global.tidb_ddl_error_count_limit = 1")
    // Go defer handles resource/config cleanup.
    defer tk.MustExec(fmt.Sprintf("set @@global.tidb_ddl_error_count_limit = %d", originalVal))

    tk.MustExec("use test")
    tk.MustExec("drop table if exists t")
    tk.MustExec("create table t(a int, b int)")
    tk.MustExec("insert into t values(1, 1)")
    tk.MustGetErrMsg("alter table t add index idx(b)", "[ddl:-1]job.ErrCount:0, mock unknown type: ast.whenClause.")
    tk.MustExec("drop table if exists t")
}

// TestCreateTableWithIntegerLengthWarning 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestCreateTableWithIntegerLengthWarning(t *testing.T) {
#[test]
pub fn TestCreateTableWithIntegerLengthWarning() {
    // Inject the strict-integer-display-width variable in parser directly.
    parsertypes.TiDBStrictIntegerDisplayWidth = true
    // Go defer handles resource/config cleanup.
    defer func() { parsertypes.TiDBStrictIntegerDisplayWidth = false }()
    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t")

    tk.MustExec("create table t(a tinyint(1))")
    tk.MustQuery("show warnings").Check(testkit.Rows())

    tk.MustExec("drop table if exists t")
    tk.MustExec("create table t(a smallint(2))")
    tk.MustQuery("show warnings").Check(testkit.Rows("Warning 1681 Integer display width is deprecated and will be removed in a future release."))

    tk.MustExec("drop table if exists t")
    tk.MustExec("create table t(a int(2))")
    tk.MustQuery("show warnings").Check(testkit.Rows("Warning 1681 Integer display width is deprecated and will be removed in a future release."))

    tk.MustExec("drop table if exists t")
    tk.MustExec("create table t(a mediumint(2))")
    tk.MustQuery("show warnings").Check(testkit.Rows("Warning 1681 Integer display width is deprecated and will be removed in a future release."))

    tk.MustExec("drop table if exists t")
    tk.MustExec("create table t(a bigint(2))")
    tk.MustQuery("show warnings").Check(testkit.Rows("Warning 1681 Integer display width is deprecated and will be removed in a future release."))

    tk.MustExec("drop table if exists t")
    tk.MustExec("create table t(a integer(2))")
    tk.MustQuery("show warnings").Check(testkit.Rows("Warning 1681 Integer display width is deprecated and will be removed in a future release."))

    tk.MustExec("drop table if exists t")
    tk.MustExec("create table t(a int1(1))") // Note that int1(1) is tinyint(1) which is boolean-ish
    tk.MustQuery("show warnings").Check(testkit.Rows())

    tk.MustExec("drop table if exists t")
    tk.MustExec("create table t(a int2(2))")
    tk.MustQuery("show warnings").Check(testkit.Rows("Warning 1681 Integer display width is deprecated and will be removed in a future release."))

    tk.MustExec("drop table if exists t")
    tk.MustExec("create table t(a int3(2))")
    tk.MustQuery("show warnings").Check(testkit.Rows("Warning 1681 Integer display width is deprecated and will be removed in a future release."))

    tk.MustExec("drop table if exists t")
    tk.MustExec("create table t(a int4(2))")
    tk.MustQuery("show warnings").Check(testkit.Rows("Warning 1681 Integer display width is deprecated and will be removed in a future release."))

    tk.MustExec("drop table if exists t")
    tk.MustExec("create table t(a int8(2))")
    tk.MustQuery("show warnings").Check(testkit.Rows("Warning 1681 Integer display width is deprecated and will be removed in a future release."))

    tk.MustExec("drop table if exists t")
}
// TestShowCountWarningsOrErrors 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestShowCountWarningsOrErrors(t *testing.T) {
#[test]
pub fn TestShowCountWarningsOrErrors() {
    // Inject the strict-integer-display-width variable in parser directly.
    parsertypes.TiDBStrictIntegerDisplayWidth = true
    // Go defer handles resource/config cleanup.
    defer func() { parsertypes.TiDBStrictIntegerDisplayWidth = false }()
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")

    // test sql run work
    tk.MustExec("show count(*) warnings")
    tk.MustExec("show count(*) errors")

    // test count warnings
    tk.MustExec("drop table if exists t1,t2,t3")
    // Warning: Integer display width is deprecated and will be removed in a future release.
    tk.MustExec("create table t(a int8(2));" +
        "create table t1(a int4(2));" +
        "create table t2(a int4(2));")
    tk.MustQuery("show count(*) warnings").Check(tk.MustQuery("select @@session.warning_count").Rows())

    // test count errors
    tk.MustExec("drop table if exists show_errors")
    tk.MustExec("create table show_errors (a int)")
    // Error: Table exist
    _, _ = tk.Exec("create table show_errors (a int)")
    tk.MustQuery("show count(*) errors").Check(tk.MustQuery("select @@session.error_count").Rows())
}

// TestIssue60047 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestIssue60047(t *testing.T) {
#[test]
pub fn TestIssue60047() {
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("drop table if exists t")
    tk.MustExec(`CREATE TABLE t (
        a INT,
        b INT,
        c VARCHAR(10),
        unique key idx(a, c)
    ) partition by range columns(c) (
    partition p0 values less than ('30'),
    partition p1 values less than ('60'),
    partition p2 values less than ('90'));`)

    // initialize the data.
    // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
    for i := range 90 {
        tk.MustExec("insert into t values (?, ?, ?)", i, i, i)
    }

    // parallel execute `insert ... on duplicate key update` and `alter table ... add column after ...`
    var err error
    hookFunc := func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if job.SchemaState == model.StateWriteOnly {
            tk1 := testkit.NewTestKit(t, store)
            tk1.MustExec("use test")
            val := 30 + rand.Intn(60)
            insertSQL := fmt.Sprintf("insert into t(a, b, c) values(%v, %v, %v) on duplicate key update a=values(a), b=values(b), c=values(c)",
                val, rand.Intn(90), strconv.FormatInt(int64(val), 10))
            // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
            err = tk1.ExecToErr(insertSQL)
        }
    }
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", hookFunc)

    tk2 := testkit.NewTestKit(t, store)
    tk2.MustExec("use test")
    ddlSQL := "alter table t add column `d` decimal(20,4) not null default '0'"
    tk2.MustExec(ddlSQL)

    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
}

// Close issue #24172.
// See https://github.com/pingcap/tidb/issues/24172
// TestCancelJobWriteConflict 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestCancelJobWriteConflict(t *testing.T) {
#[test]
pub fn TestCancelJobWriteConflict() {
    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)

    tk1 := testkit.NewTestKit(t, store)
    tk2 := testkit.NewTestKit(t, store)

    // testkit SQL via mock store/domain.
    tk1.MustExec("use test")

    tk1.MustExec("create table t(id int)")

    var cancelErr error
    var rs []sqlexec.RecordSet

    // Test when cancelling cannot be retried and adding index succeeds.
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if job.Type == model.ActionAddIndex && job.State == model.JobStateRunning && job.SchemaState == model.StateWriteReorganization {
            // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
            require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/kv/mockCommitErrorInNewTxn", `return("no_retry")`))
            // Go defer handles resource/config cleanup.
            defer func() {
                // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
                require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/kv/mockCommitErrorInNewTxn"))
            }()
            // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
            require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/mockFailedCommandOnConcurencyDDL", `return(true)`))
            // Go defer handles resource/config cleanup.
            defer func() {
                // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
                require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/mockFailedCommandOnConcurencyDDL"))
            }()

            stmt := fmt.Sprintf("admin cancel ddl jobs %d", job.ID)
            rs, cancelErr = tk2.Session().Execute(context.Background(), stmt)
        }
    })
    tk1.MustExec("alter table t add index (id)")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.EqualError(t, cancelErr, "mock failed admin command on ddl jobs")

    // Test when cancelling is retried only once and adding index is cancelled in the end.
    var jobID int64
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if job.Type == model.ActionAddIndex && job.State == model.JobStateRunning && job.SchemaState == model.StateWriteReorganization {
            jobID = job.ID
            stmt := fmt.Sprintf("admin cancel ddl jobs %d", job.ID)
            // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
            require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/kv/mockCommitErrorInNewTxn", `return("retry_once")`))
            // Go defer handles resource/config cleanup.
            defer func() {
                // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
                require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/kv/mockCommitErrorInNewTxn"))
            }()
            rs, cancelErr = tk2.Session().Execute(context.Background(), stmt)
        }
    })
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk1.MustGetErrCode("alter table t add index (id)", errno.ErrCancelledDDLJob)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, cancelErr)
    result := tk2.ResultSetToResultWithCtx(context.Background(), rs[0], "cancel ddl job fails")
    result.Check(testkit.Rows(fmt.Sprintf("%d successful", jobID)))
}

// TestTxnSavepointWithDDL 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestTxnSavepointWithDDL(t *testing.T) {
#[test]
pub fn TestTxnSavepointWithDDL() {
    // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
    if kerneltype.IsNextGen() {
        t.Skip("MDL is always enabled and read only in nextgen")
    }
    store := testkit.CreateMockStoreWithSchemaLease(t, dbTestLease)
    tk := testkit.NewTestKit(t, store)
    tk2 := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test;")
    tk.MustExec("set global tidb_enable_metadata_lock=0")
    tk2.MustExec("use test;")

    prepareFn := func() {
        tk.MustExec("drop table if exists t1, t2")
        tk.MustExec("create table t1 (c1 int primary key, c2 int)")
        tk.MustExec("create table t2 (c1 int primary key, c2 int)")
    }
    prepareFn()

    tk.MustExec("begin pessimistic")
    tk.MustExec("savepoint s1")
    tk.MustExec("insert t1 values (1, 11)")
    tk.MustExec("rollback to s1")
    tk2.MustExec("alter table t1 add index idx2(c2)")
    tk.MustExec("commit")
    tk.MustQuery("select * from t1").Check(testkit.Rows())
    tk.MustExec("admin check table t1")

    tk.MustExec("begin pessimistic")
    tk.MustExec("savepoint s1")
    tk.MustExec("insert t1 values (1, 11)")
    tk.MustExec("savepoint s2")
    tk.MustExec("insert t2 values (1, 11)")
    tk.MustExec("rollback to s2")
    tk2.MustExec("alter table t2 add index idx2(c2)")
    tk.MustExec("commit")
    tk.MustQuery("select * from t2").Check(testkit.Rows())
    tk.MustExec("admin check table t1")
    tk.MustExec("admin check table t2")

    prepareFn()
    tk.MustExec("truncate table t1")
    tk.MustExec("begin pessimistic")
    tk.MustExec("savepoint s1")
    tk.MustExec("insert t1 values (1, 11)")
    tk.MustExec("savepoint s2")
    tk.MustExec("insert t2 values (1, 11)")
    tk.MustExec("rollback to s2")
    tk2.MustExec("alter table t1 add index idx2(c2)")
    tk2.MustExec("alter table t2 add index idx2(c2)")
    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    err := tk.ExecToErr("commit")
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Error(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Regexp(t, ".*8028.*Information schema is changed during the execution of the statement.*", err.Error())
    tk.MustQuery("select * from t1").Check(testkit.Rows())
    tk.MustExec("admin check table t1")
    tk.MustExec("admin check table t2")
}

// TestSnapshotVersion 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestSnapshotVersion(t *testing.T) {
#[test]
pub fn TestSnapshotVersion() {
    store, dom := testkit.CreateMockStoreAndDomainWithSchemaLease(t, dbTestLease)

    tk := testkit.NewTestKit(t, store)

    dd := dom.DDL()
    ddl.DisableTiFlashPoll(dd)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, dbTestLease, dom.GetSchemaLease())

    snapTS := oracle.GoTimeToTS(time.Now())
    // testkit SQL via mock store/domain.
    tk.MustExec("create database test2")
    tk.MustExec("use test2")
    tk.MustExec("create table t(a int)")

    is := dom.InfoSchema()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NotNil(t, is)

    // For updating the self schema version.
    goCtx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
    sum, err := dd.SchemaSyncer().WaitVersionSynced(goCtx, 0, is.SchemaMetaVersion(), false)
    cancel()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.EqualValues(t, &schemaver.SyncSummary{ServerCount: 1}, sum)

    snapIs, err := dom.GetSnapshotInfoSchema(snapTS)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NotNil(t, snapIs)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)

    // Make sure that the self schema version doesn't be changed.
    goCtx, cancel = context.WithTimeout(context.Background(), 100*time.Millisecond)
    sum, err = dd.SchemaSyncer().WaitVersionSynced(goCtx, 0, is.SchemaMetaVersion(), false)
    cancel()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.EqualValues(t, &schemaver.SyncSummary{ServerCount: 1}, sum)

    // for GetSnapshotInfoSchema
    currSnapTS := oracle.GoTimeToTS(time.Now())
    currSnapIs, err := dom.GetSnapshotInfoSchema(currSnapTS)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NotNil(t, currSnapTS)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, is.SchemaMetaVersion(), currSnapIs.SchemaMetaVersion())

    // for GetSnapshotMeta
    dbInfo, ok := currSnapIs.SchemaByName(ast.NewCIStr("test2"))
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, ok)

    tbl, err := currSnapIs.TableByName(context.Background(), ast.NewCIStr("test2"), ast.NewCIStr("t"))
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)

    m := dom.GetSnapshotMeta(snapTS)

    tblInfo1, err := m.GetTable(dbInfo.ID, tbl.Meta().ID)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, meta.ErrDBNotExists.Equal(err))
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Nil(t, tblInfo1)

    m = dom.GetSnapshotMeta(currSnapTS)

    tblInfo2, err := m.GetTable(dbInfo.ID, tbl.Meta().ID)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, tblInfo2, tbl.Meta())
}

// TestSchemaValidator 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestSchemaValidator(t *testing.T) {
#[test]
pub fn TestSchemaValidator() {
    store, dom := testkit.CreateMockStoreAndDomainWithSchemaLease(t, dbTestLease)

    tk := testkit.NewTestKit(t, store)

    dd := dom.DDL()
    ddl.DisableTiFlashPoll(dd)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, dbTestLease, dom.GetSchemaLease())

    // testkit SQL via mock store/domain.
    tk.MustExec("create table test.t(a int)")

    err := dom.Reload()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    schemaVer := dom.InfoSchema().SchemaMetaVersion()
    ver, err := store.CurrentVersion(kv.GlobalTxnScope)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)

    ts := ver.Ver
    _, res := dom.GetSchemaValidator().Check(ts, schemaVer, nil, true)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, validatorapi.ResultSucc, res)

    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/infoschema/issyncer/ErrorMockReloadFailed", `return(true)`))

    err = dom.Reload()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Error(t, err)
    _, res = dom.GetSchemaValidator().Check(ts, schemaVer, nil, true)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, validatorapi.ResultSucc, res)
    // 时间等待用于暴露异步 DDL 状态；保留等待点但不依赖真实时间推进。
    time.Sleep(dbTestLease)

    ver, err = store.CurrentVersion(kv.GlobalTxnScope)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    ts = ver.Ver
    _, res = dom.GetSchemaValidator().Check(ts, schemaVer, nil, true)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, validatorapi.ResultUnknown, res)

    // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
    require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/infoschema/issyncer/ErrorMockReloadFailed"))
    err = dom.Reload()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)

    _, res = dom.GetSchemaValidator().Check(ts, schemaVer, nil, true)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, validatorapi.ResultSucc, res)

    // For schema check, it tests for getting the result of "ResultUnknown".
    is := dom.InfoSchema()
    schemaChecker := domain.NewSchemaChecker(dom.GetSchemaValidator(), is.SchemaMetaVersion(), nil, true)
    // Make sure it will retry one time and doesn't take a long time.
    domain.SchemaOutOfDateRetryTimes.Store(1)
    domain.SchemaOutOfDateRetryInterval.Store(time.Millisecond * 1)
    dom.GetSchemaValidator().Stop()
    _, err = schemaChecker.Check(uint64(123456))
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.EqualError(t, err, domain.ErrInfoSchemaExpired.Error())
}

// TestLogAndShowSlowLog 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestLogAndShowSlowLog(t *testing.T) {
#[test]
pub fn TestLogAndShowSlowLog() {
    _, dom := testkit.CreateMockStoreAndDomainWithSchemaLease(t, dbTestLease)

    dom.LogSlowQuery(&domain.SlowQueryInfo{SQL: "aaa", Duration: time.Second, Internal: true})
    dom.LogSlowQuery(&domain.SlowQueryInfo{SQL: "bbb", Duration: 3 * time.Second, SessAlias: "alias1"})
    dom.LogSlowQuery(&domain.SlowQueryInfo{SQL: "ccc", Duration: 2 * time.Second})
    // Collecting slow queries is asynchronous, wait a while to ensure it's done.
    // 时间等待用于暴露异步 DDL 状态；保留等待点但不依赖真实时间推进。
    time.Sleep(5 * time.Millisecond)

    result := dom.ShowSlowQuery(&ast.ShowSlow{Tp: ast.ShowSlowTop, Count: 2})
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Len(t, result, 2)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "bbb", result[0].SQL)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "alias1", result[0].SessAlias)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, 3*time.Second, result[0].Duration)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "ccc", result[1].SQL)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, 2*time.Second, result[1].Duration)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Empty(t, result[1].SessAlias)

    result = dom.ShowSlowQuery(&ast.ShowSlow{Tp: ast.ShowSlowTop, Count: 2, Kind: ast.ShowSlowKindInternal})
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Len(t, result, 1)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "aaa", result[0].SQL)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, time.Second, result[0].Duration)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, result[0].Internal)

    result = dom.ShowSlowQuery(&ast.ShowSlow{Tp: ast.ShowSlowTop, Count: 4, Kind: ast.ShowSlowKindAll})
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Len(t, result, 3)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "bbb", result[0].SQL)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, 3*time.Second, result[0].Duration)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "alias1", result[0].SessAlias)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "ccc", result[1].SQL)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, 2*time.Second, result[1].Duration)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Empty(t, result[1].SessAlias)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "aaa", result[2].SQL)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, time.Second, result[2].Duration)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, result[2].Internal)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Empty(t, result[2].SessAlias)

    result = dom.ShowSlowQuery(&ast.ShowSlow{Tp: ast.ShowSlowRecent, Count: 2})
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Len(t, result, 2)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "ccc", result[0].SQL)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, 2*time.Second, result[0].Duration)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Empty(t, result[0].SessAlias)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "bbb", result[1].SQL)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, 3*time.Second, result[1].Duration)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, "alias1", result[1].SessAlias)
}

// TestReportingMinStartTimestamp 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestReportingMinStartTimestamp(t *testing.T) {
#[test]
pub fn TestReportingMinStartTimestamp() {
    _, dom := testkit.CreateMockStoreAndDomainWithSchemaLease(t, dbTestLease)

    infoSyncer := dom.InfoSyncer()
    sm := &testkit.MockSessionManager{
        PS: make([]*sessmgr.ProcessInfo, 0),
    }
    infoSyncer.SetSessionManager(sm)
    beforeTS := oracle.GoTimeToTS(time.Now())
    infoSyncer.ReportMinStartTS(dom.Store(), nil)
    afterTS := oracle.GoTimeToTS(time.Now())
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.False(t, infoSyncer.GetMinStartTS() > beforeTS && infoSyncer.GetMinStartTS() < afterTS)

    now := time.Now()
    validTS := oracle.GoTimeToLowerLimitStartTS(now.Add(time.Minute), tikv.MaxTxnTimeUse)
    lowerLimit := oracle.GoTimeToLowerLimitStartTS(now, tikv.MaxTxnTimeUse)
    sm.PS = []*sessmgr.ProcessInfo{
        {CurTxnStartTS: 0},
        {CurTxnStartTS: math.MaxUint64},
        {CurTxnStartTS: lowerLimit},
        {CurTxnStartTS: validTS},
    }
    infoSyncer.SetSessionManager(sm)
    infoSyncer.ReportMinStartTS(dom.Store(), nil)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, validTS, infoSyncer.GetMinStartTS())
}

// for issue #34931
// TestBuildMaxLengthIndexWithNonRestrictedSqlMode 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestBuildMaxLengthIndexWithNonRestrictedSqlMode(t *testing.T) {
#[test]
pub fn TestBuildMaxLengthIndexWithNonRestrictedSqlMode() {
    store := testkit.CreateMockStore(t)

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")

    maxIndexLength := config.GetGlobalConfig().MaxIndexLength

    tt := []struct {
        ColType           string
        SpecifiedColLen   bool
        SpecifiedIndexLen bool
    }{
        {
            "text",
            false,
            true,
        },
        {
            "blob",
            false,
            true,
        },
        {
            "varchar",
            true,
            false,
        },
        {
            "varbinary",
            true,
            false,
        },
    }

    sqlTemplate := "create table %s (id int, name %s, age int, %s index(name%s%s)) charset=%s;"
    // test character strings for varchar and text
    // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
    for _, tc := range tt {
        // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
        for _, cs := range charset.CharacterSetInfos {
            tableName := fmt.Sprintf("t_%s", cs.Name)
            tk.MustExec(fmt.Sprintf("drop table if exists %s", tableName))
            tk.MustExec("set @@sql_mode=default")

            // test in strict sql mode
            maxLen := cs.Maxlen
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if tc.ColType == "varbinary" || tc.ColType == "blob" {
                maxLen = 1
            }
            expectKeyLength := maxIndexLength / maxLen
            length := 2 * expectKeyLength

            indexLen := ""
            // specify index length for text type
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if tc.SpecifiedIndexLen {
                indexLen = fmt.Sprintf("(%d)", length)
            }

            col := tc.ColType
            // specify column length for varchar type
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if tc.SpecifiedColLen {
                col += fmt.Sprintf("(%d)", length)
            }
            sql := fmt.Sprintf(sqlTemplate,
                tableName, col, "", indexLen, "", cs.Name)
            // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
            tk.MustGetErrCode(sql, errno.ErrTooLongKey)

            tk.MustExec("set @@sql_mode=''")

            // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
            err := tk.ExecToErr(sql)
            // Go require/assert 断言描述预期结果；不声明当前可运行。
            require.NoErrorf(t, err, "exec sql '%s' failed", sql)

            // Go require/assert 断言描述预期结果；不声明当前可运行。
            require.Equal(t, uint16(1), tk.Session().GetSessionVars().StmtCtx.WarningCount())

            warnErr := tk.Session().GetSessionVars().StmtCtx.GetWarnings()[0].Err
            tErr := errors.Cause(warnErr).(*terror.Error)
            sqlErr := terror.ToSQLError(tErr)
            // Go require/assert 断言描述预期结果；不声明当前可运行。
            require.Equal(t, errno.ErrTooLongKey, int(sqlErr.Code))

            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if cs.Name == charset.CharsetBin {
                // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
                if tc.ColType == "varchar" || tc.ColType == "varbinary" {
                    col = fmt.Sprintf("varbinary(%d)", length)
                } else {
                    col = "blob"
                }
            }
            rows := fmt.Sprintf("%s CREATE TABLE `%s` (\n  `id` int(11) DEFAULT NULL,\n  `name` %s DEFAULT NULL,\n  `age` int(11) DEFAULT NULL,\n  KEY `name` (`name`(%d))\n) ENGINE=InnoDB DEFAULT CHARSET=%s",
                tableName, tableName, col, expectKeyLength, cs.Name)
            // add collation for binary charset
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if cs.Name != charset.CharsetBin {
                rows += fmt.Sprintf(" COLLATE=%s", cs.DefaultCollation)
            }

            tk.MustQuery(fmt.Sprintf("show create table %s", tableName)).Check(testkit.Rows(rows))

            ukTable := fmt.Sprintf("t_%s_uk", cs.Name)
            mkTable := fmt.Sprintf("t_%s_mk", cs.Name)
            tk.MustExec(fmt.Sprintf("drop table if exists %s", ukTable))
            tk.MustExec(fmt.Sprintf("drop table if exists %s", mkTable))

            // For a unique index, an error occurs regardless of SQL mode because reducing
            //the index length might enable insertion of non-unique entries that do not meet
            //the specified uniqueness requirement.
            sql = fmt.Sprintf(sqlTemplate, ukTable, col, "unique", indexLen, "", cs.Name)
            // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
            tk.MustGetErrCode(sql, errno.ErrTooLongKey)

            // The multiple column index in which the length sum exceeds the maximum size
            // will return an error instead produce a warning in strict sql mode.
            indexLen = fmt.Sprintf("(%d)", expectKeyLength)
            sql = fmt.Sprintf(sqlTemplate, mkTable, col, "", indexLen, ", age", cs.Name)
            // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
            tk.MustGetErrCode(sql, errno.ErrTooLongKey)
        }
    }
}

// TestTiDBDownBeforeUpdateGlobalVersion 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestTiDBDownBeforeUpdateGlobalVersion(t *testing.T) {
#[test]
pub fn TestTiDBDownBeforeUpdateGlobalVersion() {
    store := testkit.CreateMockStore(t)

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t(a int)")

    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/mockDownBeforeUpdateGlobalVersion", `return(true)`))
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/checkDownBeforeUpdateGlobalVersion", `return(true)`))
    tk.MustExec("alter table t add column b int")
    // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
    require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/mockDownBeforeUpdateGlobalVersion"))
    // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
    require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/checkDownBeforeUpdateGlobalVersion"))
}

// TestDDLBlockedCreateView 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestDDLBlockedCreateView(t *testing.T) {
#[test]
pub fn TestDDLBlockedCreateView() {
    store := testkit.CreateMockStore(t)

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t(a int)")

    first := true
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if job.SchemaState != model.StateWriteOnly {
            return
        }
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if !first {
            return
        }
        first = false
        tk2 := testkit.NewTestKit(t, store)
        tk2.MustExec("use test")
        tk2.MustExec("create view v as select * from t")
    })
    tk.MustExec("alter table t modify column a char(10)")
}

// TestHashPartitionAddColumn 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestHashPartitionAddColumn(t *testing.T) {
#[test]
pub fn TestHashPartitionAddColumn() {
    store := testkit.CreateMockStore(t)

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t(a int, b int) partition by hash(a) partitions 4")

    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if job.SchemaState != model.StateWriteOnly {
            return
        }
        tk2 := testkit.NewTestKit(t, store)
        tk2.MustExec("use test")
        tk2.MustExec("delete from t")
    })
    tk.MustExec("alter table t add column c int")
}

// TestSetInvalidDefaultValueAfterModifyColumn 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestSetInvalidDefaultValueAfterModifyColumn(t *testing.T) {
#[test]
pub fn TestSetInvalidDefaultValueAfterModifyColumn() {
    store := testkit.CreateMockStore(t)

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t(a int, b int)")

    // WaitGroup 相关逻辑描述并发测试同步点，当前不启动真实并发执行器。
    var wg sync.WaitGroup
    var checkErr error
    one := false
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if job.SchemaState != model.StateDeleteOnly {
            return
        }
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if one {
            return
        }
        one = true
        wg.Add(1)
        // Go 这里启动 goroutine；保留并发触发点，不真正调度异步任务。
        go func() {
            tk2 := testkit.NewTestKit(t, store)
            tk2.MustExec("use test")
            _, checkErr = tk2.Exec("alter table t alter column a set default 1")
            wg.Done()
        }()
    })
    tk.MustExec("alter table t modify column a text(100)")
    // WaitGroup 相关逻辑描述并发测试同步点，当前不启动真实并发执行器。
    wg.Wait()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.EqualError(t, checkErr, "[ddl:1101]BLOB/TEXT/JSON column 'a' can't have a default value")
}

// TestMDLTruncateTable 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestMDLTruncateTable(t *testing.T) {
#[test]
pub fn TestMDLTruncateTable() {
    store, dom := testkit.CreateMockStoreAndDomain(t)

    tk := testkit.NewTestKit(t, store)
    tk2 := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t(a int);")
    tbl, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    originalTableID := tbl.Meta().ID
    tk.MustExec("begin")
    tk.MustExec("select * from t for update")

    // WaitGroup 相关逻辑描述并发测试同步点，当前不启动真实并发执行器。
    var wg sync.WaitGroup

    wg.Add(2)
    var timetk2 time.Time
    var timetk3 time.Time
    var errtk2 error
    var errtk3 error

    waitTableIDChanged := func() error {
        deadline := time.Now().Add(5 * time.Second)
        // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
        for time.Now().Before(deadline) {
            tbl, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if err == nil && tbl.Meta().ID != originalTableID {
                return nil
            }
            // 时间等待用于暴露异步 DDL 状态；保留等待点但不依赖真实时间推进。
            time.Sleep(10 * time.Millisecond)
        }
        return errors.New("timed out waiting for truncated table ID to refresh")
    }

    var once sync.Once
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if job.Type != model.ActionTruncateTable {
            return
        }
        once.Do(func() {
            // Go 这里启动 goroutine；保留并发触发点，不真正调度异步任务。
            go func() {
                // Go defer handles resource/config cleanup.
                defer wg.Done()
                // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
                if err := waitTableIDChanged(); err != nil {
                    errtk3 = err
                    return
                }
                tk3 := testkit.NewTestKit(t, store)
                tk3.MustExec("use test")
                // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
                errtk3 = tk3.ExecToErr("truncate table test.t")
                // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
                if errtk3 == nil {
                    timetk3 = time.Now()
                }
            }()
        })
    })

    // Go 这里启动 goroutine；保留并发触发点，不真正调度异步任务。
    go func() {
        // Go defer handles resource/config cleanup.
        defer wg.Done()
        // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
        errtk2 = tk2.ExecToErr("truncate table test.t")
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if errtk2 == nil {
            timetk2 = time.Now()
        }
    }()

    // 时间等待用于暴露异步 DDL 状态；保留等待点但不依赖真实时间推进。
    time.Sleep(2 * time.Second)
    timeMain := time.Now()
    tk.MustExec("commit")
    // WaitGroup 相关逻辑描述并发测试同步点，当前不启动真实并发执行器。
    wg.Wait()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, errtk2)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, errtk3)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, timetk2.After(timeMain))
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, timetk3.After(timeMain))
}

// TestTruncateTableAndSchemaDependence 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestTruncateTableAndSchemaDependence(t *testing.T) {
#[test]
pub fn TestTruncateTableAndSchemaDependence() {
    store := testkit.CreateMockStore(t)

    tk := testkit.NewTestKit(t, store)
    tk2 := testkit.NewTestKit(t, store)
    tk3 := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t(a int);")

    // WaitGroup 相关逻辑描述并发测试同步点，当前不启动真实并发执行器。
    var wg sync.WaitGroup
    wg.Add(2)

    var timetk2 time.Time
    var timetk3 time.Time

    first := false
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", func(job *model.Job) {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if first || job.Type != model.ActionTruncateTable {
            return
        }
        first = true
        // Go 这里启动 goroutine；保留并发触发点，不真正调度异步任务。
        go func() {
            tk3.MustExec("drop database test")
            timetk3 = time.Now()
            wg.Done()
        }()
        // 时间等待用于暴露异步 DDL 状态；保留等待点但不依赖真实时间推进。
        time.Sleep(3 * time.Second)
    })

    // Go 这里启动 goroutine；保留并发触发点，不真正调度异步任务。
    go func() {
        tk2.MustExec("truncate table test.t")
        timetk2 = time.Now()
        wg.Done()
    }()

    // WaitGroup 相关逻辑描述并发测试同步点，当前不启动真实并发执行器。
    wg.Wait()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, timetk3.After(timetk2))
}

// TestInsertIgnore 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestInsertIgnore(t *testing.T) {
#[test]
pub fn TestInsertIgnore() {
    store, dom := testkit.CreateMockStoreAndDomain(t)

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t(a smallint(6) DEFAULT '-13202', b varchar(221) NOT NULL DEFAULT 'duplicatevalue', " +
        "c tinyint(1) NOT NULL DEFAULT '0', PRIMARY KEY (c, b));")

    tk1 := testkit.NewTestKit(t, store)
    tk1.MustExec("use test")

    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", func(job *model.Job) {
        switch job.SchemaState {
        case model.StateDeleteOnly:
            _, err := tk1.Exec("INSERT INTO t VALUES (-18585,'aaa',1), (-18585,'0',1), (-18585,'1',1), (-18585,'duplicatevalue',1);")
            // Go require/assert 断言描述预期结果；不声明当前可运行。
            assert.NoError(t, err)
        case model.StateWriteReorganization:
            idx := testutil.FindIdxInfo(dom, "test", "t", "idx")
            // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
            if idx.BackfillState == model.BackfillStateReadyToMerge {
                _, err := tk1.Exec("insert ignore into `t`  values ( 234,'duplicatevalue',-2028 );")
                // Go require/assert 断言描述预期结果；不声明当前可运行。
                assert.NoError(t, err)
                return
            }
        }
    })

    tk.MustExec("alter table t add unique index idx(b);")
    tk.MustExec("admin check table t;")
}

// TestDDLJobErrEntrySizeTooLarge 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestDDLJobErrEntrySizeTooLarge(t *testing.T) {
#[test]
pub fn TestDDLJobErrEntrySizeTooLarge() {
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)

    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t (a int);")

    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/mockErrEntrySizeTooLarge", `1*return(true)`))
    t.Cleanup(func() {
        // 对应 Go 测试的 failpoint 收尾，避免后续用例被注入状态污染。
        require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/mockErrEntrySizeTooLarge"))
    })

    // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
    tk.MustGetErrCode("rename table t to t1;", errno.ErrEntryTooLarge)
    tk.MustExec("create table t1 (a int);")
    tk.MustExec("alter table t add column b int;") // Should not block.
}

// insertMockJob2Table 对应 Go 的同名辅助函数，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func insertMockJob2Table(tk *testkit.TestKit, job *model.Job) {
pub fn insertMockJob2Table() {
    b, err := job.Encode(false)
    tk.RequireNoError(err)
    sql := fmt.Sprintf("insert into mysql.tidb_ddl_job(job_id, job_meta) values(%s, ?);",
        strconv.FormatInt(job.ID, 10))
    // testkit SQL via mock store/domain.
    tk.MustExec(sql, b)
}

// getJobMetaByID 对应 Go 的同名辅助函数，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func getJobMetaByID(t *testing.T, tk *testkit.TestKit, jobID int64) *model.Job {
pub fn getJobMetaByID() {
    sql := fmt.Sprintf("select job_meta from mysql.tidb_ddl_job where job_id = %s",
        strconv.FormatInt(jobID, 10))
    // testkit SQL via mock store/domain.
    rows := tk.MustQuery(sql)
    res := rows.Rows()
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Len(t, res, 1)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Len(t, res[0], 1)
    jobBinary := []byte(res[0][0].(string))
    job := model.Job{}
    err := job.Decode(jobBinary)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    return &job
}

// deleteJobMetaByID 对应 Go 的同名辅助函数，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func deleteJobMetaByID(tk *testkit.TestKit, jobID int64) {
pub fn deleteJobMetaByID() {
    sql := fmt.Sprintf("delete from mysql.tidb_ddl_job where job_id = %s",
        strconv.FormatInt(jobID, 10))
    // testkit SQL via mock store/domain.
    tk.MustExec(sql)
}

// TestResumeSystemPausedDDLJobWithKVDiskFullReason 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestResumeSystemPausedDDLJobWithKVDiskFullReason(t *testing.T) {
#[test]
pub fn TestResumeSystemPausedDDLJobWithKVDiskFullReason() {
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)

    job := model.Job{
        ID:            1,
        Type:          model.ActionAddIndex,
        State:         model.JobStatePaused,
        AdminOperator: model.AdminCommandBySystem,
    }
    job.SetPauseReason(model.JobPauseReasonKVDiskFull, "TiKV disk full")
    job.Error = dbterror.ErrDDLAutoPausedByKVDiskFull
    insertMockJob2Table(tk, &job)

    // testkit SQL via mock store/domain.
    tk.MustExec("admin resume ddl jobs 1;")
    resumedJob := getJobMetaByID(t, tk, job.ID)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, model.JobStateQueueing, resumedJob.State)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Nil(t, resumedJob.PauseReason)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, resumedJob.HasResumeReason(model.JobResumeReasonKVDiskFull))
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Nil(t, resumedJob.Error)
    deleteJobMetaByID(tk, job.ID)

    systemPausedJob := model.Job{
        ID:            2,
        Type:          model.ActionAddIndex,
        State:         model.JobStatePaused,
        AdminOperator: model.AdminCommandBySystem,
    }
    insertMockJob2Table(tk, &systemPausedJob)
    tk.MustQuery("admin resume ddl jobs 2;").Check(testkit.Rows(
        "2 error: [ddl:8261]Job [2] can't be resumed: job has been paused by [System], should not resumed by [EndUser]"))
    deleteJobMetaByID(tk, systemPausedJob.ID)

    upgradePausedJob := model.Job{
        ID:            3,
        Type:          model.ActionAddIndex,
        State:         model.JobStatePaused,
        AdminOperator: model.AdminCommandBySystem,
    }
    insertMockJob2Table(tk, &upgradePausedJob)

    kvDiskFullPausedJob := model.Job{
        ID:            4,
        Type:          model.ActionAddIndex,
        State:         model.JobStatePaused,
        AdminOperator: model.AdminCommandBySystem,
    }
    kvDiskFullPausedJob.SetPauseReason(model.JobPauseReasonKVDiskFull, "TiKV disk full")
    kvDiskFullPausedJob.Error = dbterror.ErrDDLAutoPausedByKVDiskFull
    insertMockJob2Table(tk, &kvDiskFullPausedJob)

    jobErrs, err := ddl.ResumeAllJobsBySystem(tk.Session())
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Empty(t, jobErrs)

    resumedUpgradeJob := getJobMetaByID(t, tk, upgradePausedJob.ID)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, model.JobStateQueueing, resumedUpgradeJob.State)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Nil(t, resumedUpgradeJob.PauseReason)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Nil(t, resumedUpgradeJob.ResumeReason)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Nil(t, resumedUpgradeJob.Error)

    stillPausedJob := getJobMetaByID(t, tk, kvDiskFullPausedJob.ID)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.True(t, stillPausedJob.IsPausedBySystemForKVDiskFull())
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NotNil(t, stillPausedJob.PauseReason)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NotNil(t, stillPausedJob.Error)
    deleteJobMetaByID(tk, upgradePausedJob.ID)
    deleteJobMetaByID(tk, kvDiskFullPausedJob.ID)
}

// TestAdminAlterDDLJobUpdateSysTable 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestAdminAlterDDLJobUpdateSysTable(t *testing.T) {
#[test]
pub fn TestAdminAlterDDLJobUpdateSysTable() {
    // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
    if kerneltype.IsNextGen() {
        t.Skip("resource params are calculated automatically on nextgen for add-index, we don't support alter them")
    }
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t (a int);")

    // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
    for _, useCloudStorage := range []bool{true, false} {
        job := model.Job{
            ID:   1,
            Type: model.ActionAddIndex,
            ReorgMeta: &model.DDLReorgMeta{
                UseCloudStorage: useCloudStorage,
            },
        }
        job.ReorgMeta.Concurrency.Store(4)
        job.ReorgMeta.BatchSize.Store(128)
        insertMockJob2Table(tk, &job)
        tk.MustExec(fmt.Sprintf("admin alter ddl jobs %d thread = 8;", job.ID))
        j := getJobMetaByID(t, tk, job.ID)
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, 8, j.ReorgMeta.GetConcurrency())

        tk.MustExec(fmt.Sprintf("admin alter ddl jobs %d batch_size = 256;", job.ID))
        j = getJobMetaByID(t, tk, job.ID)
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, 256, j.ReorgMeta.GetBatchSize())

        tk.MustExec(fmt.Sprintf("admin alter ddl jobs %d thread = 16, batch_size = 512;", job.ID))
        j = getJobMetaByID(t, tk, job.ID)
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, 16, j.ReorgMeta.GetConcurrency())
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, 512, j.ReorgMeta.GetBatchSize())
        deleteJobMetaByID(tk, job.ID)
    }
}

// TestAdminAlterDDLJobUnsupportedCases 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestAdminAlterDDLJobUnsupportedCases(t *testing.T) {
#[test]
pub fn TestAdminAlterDDLJobUnsupportedCases() {
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t (a int);")

    // invalid config value
    tk.MustGetErrMsg("admin alter ddl jobs 1 thread = 0;", "the value 0 for thread is out of range [1, 256]")
    tk.MustGetErrMsg("admin alter ddl jobs 1 thread = 257;", "the value 257 for thread is out of range [1, 256]")
    tk.MustGetErrMsg("admin alter ddl jobs 1 thread = 10.5;", "the value for thread is invalid, only integer is allowed")
    tk.MustGetErrMsg("admin alter ddl jobs 1 thread = '16';", "the value for thread is invalid, only integer is allowed")
    tk.MustGetErrMsg("admin alter ddl jobs 1 thread = '';", "the value for thread is invalid, only integer is allowed")
    tk.MustGetErrMsg("admin alter ddl jobs 1 batch_size = 31;", "the value 31 for batch_size is out of range [32, 10240]")
    tk.MustGetErrMsg("admin alter ddl jobs 1 batch_size = 10241;", "the value 10241 for batch_size is out of range [32, 10240]")
    tk.MustGetErrMsg("admin alter ddl jobs 1 batch_size = 321.3;", "the value for batch_size is invalid, only integer is allowed")
    tk.MustGetErrMsg("admin alter ddl jobs 1 batch_size = '512';", "the value for batch_size is invalid, only integer is allowed")
    tk.MustGetErrMsg("admin alter ddl jobs 1 batch_size = '';", "the value for batch_size is invalid, only integer is allowed")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = '2PiB';", "the value 2251799813685248 for max_write_speed is out of range [0, 1125899906842624]")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = -1;", "the value -1 for max_write_speed is out of range [0, 1125899906842624]")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = 1.23;", "the value 1.23 for max_write_speed is invalid")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = 'MiB';", "parse max_write_speed value error: invalid size: 'MiB'")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = 'asd';", "parse max_write_speed value error: invalid size: 'asd'")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = '';", "parse max_write_speed value error: invalid size: ''")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = '20xl';", "parse max_write_speed value error: invalid suffix: 'xl'")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = 1.2.3;", "[parser:1064]You have an error in your SQL syntax; check the manual that corresponds to your TiDB version for the right syntax to use line 1 column 46 near \".3;\" ")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = 20+30;", "[parser:1064]You have an error in your SQL syntax; check the manual that corresponds to your TiDB version for the right syntax to use line 1 column 44 near \"+30;\" ")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = rand();", "[parser:1064]You have an error in your SQL syntax; check the manual that corresponds to your TiDB version for the right syntax to use line 1 column 45 near \"rand();\" ")
    // valid config value
    tk.MustGetErrMsg("admin alter ddl jobs 1 thread = 16;", "ddl job 1 is not running")
    tk.MustGetErrMsg("admin alter ddl jobs 1 batch_size = 64;", "ddl job 1 is not running")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = '0';", "ddl job 1 is not running")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = '64';", "ddl job 1 is not running")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = '2KB';", "ddl job 1 is not running")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = '3MiB';", "ddl job 1 is not running")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = '4 gb';", "ddl job 1 is not running")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = 1;", "ddl job 1 is not running")
    tk.MustGetErrMsg("admin alter ddl jobs 1 max_write_speed = '1.23';", "ddl job 1 is not running")

    // invalid job id
    tk.MustGetErrMsg("admin alter ddl jobs 1 thread = 8;", "ddl job 1 is not running")

    job := model.Job{
        ID:   1,
        Type: model.ActionAddColumn,
    }
    insertMockJob2Table(tk, &job)
    // unsupported job type
    tk.MustGetErrMsg(fmt.Sprintf("admin alter ddl jobs %d thread = 8;", job.ID),
        "unsupported DDL operation: add column. Supported DDL operations are: ADD INDEX, MODIFY COLUMN, and ALTER TABLE REORGANIZE PARTITION")
    deleteJobMetaByID(tk, 1)

    // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
    if kerneltype.IsNextGen() {
        job := model.Job{
            ID:   2,
            Type: model.ActionAddIndex,
        }
        insertMockJob2Table(tk, &job)
        // unsupported job type
        // 这里显式检查 DDL/SQL 错误码或错误对象，保留原错误路径。
        err := tk.ExecToErr(fmt.Sprintf("admin alter ddl jobs %d thread = 8;", job.ID))
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.ErrorIs(t, err, variable.ErrNotSupportedInNextGen)
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.ErrorContains(t, err, "Altering ADD INDEX job")
        deleteJobMetaByID(tk, 2)
    }
}

// TestAdminAlterDDLJobCommitFailed 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestAdminAlterDDLJobCommitFailed(t *testing.T) {
#[test]
pub fn TestAdminAlterDDLJobCommitFailed() {
    // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
    if kerneltype.IsNextGen() {
        t.Skip("resource params are calculated automatically on nextgen for add-index, we don't support alter them")
    }
    store := testkit.CreateMockStore(t)
    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("use test")
    tk.MustExec("create table t (a int);")
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/executor/mockAlterDDLJobCommitFailed", `return(true)`)
    // Go defer handles resource/config cleanup.
    defer testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/executor/mockAlterDDLJobCommitFailed")

    job := model.Job{
        ID:        1,
        Type:      model.ActionAddIndex,
        ReorgMeta: &model.DDLReorgMeta{},
    }
    job.ReorgMeta.Concurrency.Store(4)
    job.ReorgMeta.BatchSize.Store(128)
    insertMockJob2Table(tk, &job)
    tk.MustGetErrMsg(fmt.Sprintf("admin alter ddl jobs %d thread = 8, batch_size = 256;", job.ID),
        "mock commit failed on admin alter ddl jobs")
    j := getJobMetaByID(t, tk, job.ID)
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, j.ReorgMeta, job.ReorgMeta)
    deleteJobMetaByID(tk, job.ID)
}

// TestGetAllTableInfos 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestGetAllTableInfos(t *testing.T) {
#[test]
pub fn TestGetAllTableInfos() {
    store, dom := testkit.CreateMockStoreAndDomain(t)
    tk := testkit.NewTestKit(t, store)

    // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
    for i := range 113 {
        // testkit SQL via mock store/domain.
        tk.MustExec(fmt.Sprintf("create database test%d", i))
        tk.MustExec(fmt.Sprintf("use test%d", i))
        tk.MustExec("create table t1 (a int)")
        tk.MustExec("create table t2 (a int)")
        tk.MustExec("create table t3 (a int)")
    }

    tblInfos1 := make([]*model.TableInfo, 0)
    tblInfos2 := make([]*model.TableInfo, 0)
    dbs := dom.InfoSchema().AllSchemas()
    // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
    for _, db := range dbs {
        // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
        if infoschema.IsSpecialDB(db.Name.L) {
            continue
        }
        info, err := dom.InfoSchema().SchemaTableInfos(context.Background(), db.Name)
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.NoError(t, err)
        tblInfos1 = append(tblInfos1, info...)
    }

    err := meta.IterAllTables(context.Background(), store, oracle.GoTimeToTS(time.Now()), 13, func(tblInfo *model.TableInfo) error {
        tblInfos2 = append(tblInfos2, tblInfo)
        return nil
    })
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, err)

    slices.SortFunc(tblInfos1, func(i, j *model.TableInfo) int {
        return int(i.ID - j.ID)
    })
    slices.SortFunc(tblInfos2, func(i, j *model.TableInfo) int {
        return int(i.ID - j.ID)
    })

    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.Equal(t, len(tblInfos1), len(tblInfos2))
    // 保留 Go 循环/表驱动测试顺序，便于人工核对每个场景。
    for i := range tblInfos1 {
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, tblInfos1[i].ID, tblInfos2[i].ID)
        // Go require/assert 断言描述预期结果；不声明当前可运行。
        require.Equal(t, tblInfos1[i].DBID, tblInfos2[i].DBID)
    }

    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, meta.IterAllTables(context.Background(), store, oracle.GoTimeToTS(time.Now()), 0, func(tblInfo *model.TableInfo) error {
        return nil
    }))
    // Go require/assert 断言描述预期结果；不声明当前可运行。
    require.NoError(t, meta.IterAllTables(context.Background(), store, oracle.GoTimeToTS(time.Now()), -999, func(tblInfo *model.TableInfo) error {
        return nil
    }))
}

// TestGetVersionFailed 对应 Go 的同名测试，保留原控制流、断言和外部依赖调用形状。
// Go 签名: func TestGetVersionFailed(t *testing.T) {
#[test]
pub fn TestGetVersionFailed() {
    // 保留 Go 分支语义，常用于区分错误路径、功能开关或重试退出条件。
    if kerneltype.IsNextGen() {
        t.Skip("MDL is always enabled and read only in nextgen")
    }
    store := testkit.CreateMockStore(t)

    tk := testkit.NewTestKit(t, store)
    // testkit SQL via mock store/domain.
    tk.MustExec("set global tidb_enable_metadata_lock=0")
    tk.MustExec("use test")
    tk.MustExec("create table t(a int)")

    // Simulate the failure of getting the current version twice.
    // failpoint 注入是测试外部依赖边界；这里保留注入点名称和回调语义。
    testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ddl/mockGetCurrentVersionFailed", "2*return(true)")

    tk.MustExec("alter table t add column b int")
}
*/

use std::time::Duration;

// 引入 DDL 执行器相关类型：
// - Executor：DDL 执行器，负责把 schema 变更转换为 job 并应用到内存元数据；
// - MemoryJobBackend：内存版 job 后端，记录 job 历史，替代真实的 TiKV 存储；
// - OnExist：对象已存在时的处理策略（报错 / 忽略）；
// - SessionContext：会话上下文，携带系统变量与提示信息（notes）。
use crate::executor::{
    Executor, ExecutorError, JobState, MemoryJobBackend, OnExist, SessionContext,
};

/// 构造测试用的 DDL 执行器与默认会话上下文。
/// 使用内存 job 后端和零租约时长（schema lease，schema 版本同步的租约周期），
/// 使测试同步执行、无需等待。
fn executor() -> (Executor<MemoryJobBackend>, SessionContext) {
    (
        Executor::new(MemoryJobBackend::default(), Duration::ZERO),
        SessionContext::default(),
    )
}

/// 验证 CREATE DATABASE：
/// 1) 未显式指定字符集时，采用会话系统变量给出的 utf8mb4 默认排序规则；
/// 2) 同名 schema 已存在时，OnExist::Error 报错、OnExist::Ignore 幂等返回原 id；
/// 3) schema 名大小写不敏感（App/app/APP 视为同一个）。
#[test]
fn create_schema_uses_session_default_collation_and_on_exist_rules() {
    let (mut ddl, mut session) = executor();
    // 通过会话系统变量注入 utf8mb4 的默认排序规则。
    session.system_vars.insert(
        "default_collation_for_utf8mb4".into(),
        "utf8mb4_general_ci".into(),
    );
    let id = ddl
        .create_schema(&mut session, "App", &[], None, OnExist::Error)
        .unwrap();
    let schema = &ddl.schemas["app"];
    assert_eq!(id, schema.id);
    assert_eq!("utf8mb4", schema.charset);
    assert_eq!("utf8mb4_general_ci", schema.collation);
    assert!(matches!(
        ddl.create_schema(&mut session, "app", &[], None, OnExist::Error),
        Err(ExecutorError::SchemaExists(_))
    ));
    assert_eq!(
        id,
        ddl.create_schema(&mut session, "APP", &[], None, OnExist::Ignore)
            .unwrap()
    );
    assert!(matches!(
        ddl.create_schema(&mut session, "app", &[], None, OnExist::Replace),
        Err(ExecutorError::Unsupported(operation)) if operation == "replace schema"
    ));
}

/// 对应 Go 创建 schema 时对重复 charset/collation 选项的归并规则：
/// 同值（忽略大小写）可重复指定，互相冲突的值必须在写入元数据前报错。
#[test]
fn create_schema_rejects_conflicting_charset_options_without_side_effects() {
    let (mut ddl, mut session) = executor();
    let options = vec![
        (Some("UTF8".into()), Some("utf8_bin".into())),
        (Some("utf8".into()), Some("UTF8_BIN".into())),
    ];
    ddl.create_schema(&mut session, "consistent", &options, None, OnExist::Error)
        .unwrap();
    assert_eq!("utf8", ddl.schemas["consistent"].charset);
    assert_eq!("utf8_bin", ddl.schemas["consistent"].collation);

    let history_len = ddl.backend().history().len();
    let conflicting = vec![(Some("utf8".into()), None), (Some("latin1".into()), None)];
    assert!(matches!(
        ddl.create_schema(
            &mut session,
            "conflicting",
            &conflicting,
            None,
            OnExist::Error,
        ),
        Err(ExecutorError::InvalidCharsetCollation)
    ));
    assert!(!ddl.schemas.contains_key("conflicting"));
    assert_eq!(history_len, ddl.backend().history().len());
}

/// 验证 ALTER DATABASE ... CHARACTER SET：
/// 修改字符集/排序规则生效；同值重复修改是幂等操作（不产生新的 DDL job）；
/// 排序规则与字符集不匹配（utf8 配 latin1_bin）时报错。
#[test]
fn alter_schema_charset_validates_collation_and_is_idempotent() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.alter_schema_charset(&mut session, "test", "utf8", "utf8_bin")
        .unwrap();
    assert_eq!("utf8", ddl.schemas["test"].charset);
    assert_eq!("utf8_bin", ddl.schemas["test"].collation);
    // 记录 job 历史长度；同值（仅大小写不同）的重复修改不应新增 job。
    let history_len = ddl.backend().history().len();
    ddl.alter_schema_charset(&mut session, "test", "UTF8", "UTF8_BIN")
        .unwrap();
    assert_eq!(history_len, ddl.backend().history().len());
    // 字符集与排序规则不匹配应返回 InvalidCharsetCollation 错误。
    assert!(matches!(
        ddl.alter_schema_charset(&mut session, "test", "utf8", "latin1_bin"),
        Err(ExecutorError::InvalidCharsetCollation)
    ));
}

/// 验证 schema 放置策略（placement policy，控制数据副本物理分布的规则）：
/// 可切换到新策略；设置为 "default" 表示恢复默认并清除策略；
/// ignore 模式下不修改策略，只在会话 notes 中记录提示。
#[test]
fn schema_placement_default_and_ignore_clear_the_policy() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(
        &mut session,
        "test",
        &[],
        Some("primary".into()),
        OnExist::Error,
    )
    .unwrap();
    ddl.alter_schema_placement(&mut session, "test", Some("analytics".into()), false)
        .unwrap();
    assert_eq!(
        Some("analytics"),
        ddl.schemas["test"].placement_policy.as_deref()
    );
    ddl.alter_schema_placement(&mut session, "test", Some("default".into()), false)
        .unwrap();
    assert_eq!(None, ddl.schemas["test"].placement_policy);
    ddl.alter_schema_placement(&mut session, "test", Some("ignored".into()), true)
        .unwrap();
    assert_eq!(None, ddl.schemas["test"].placement_policy);
    assert_eq!(vec!["placement is ignored"], session.notes);
}

/// 验证 DROP DATABASE 与 RECOVER（闪回恢复）：
/// 删除后 schema 从元数据消失；重复删除非幂等模式报 SchemaNotFound，
/// if_exists 模式静默成功；恢复后 schema id 与删除前保持一致（元数据身份不变）。
#[test]
fn drop_and_recover_schema_preserve_metadata_identity() {
    let (mut ddl, mut session) = executor();
    let id = ddl
        .create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    // 先保存元数据快照，用于稍后恢复。
    let saved = ddl.schemas["test"].clone();
    ddl.drop_schema(&mut session, "test", false).unwrap();
    assert!(!ddl.schemas.contains_key("test"));
    assert!(matches!(
        ddl.drop_schema(&mut session, "test", false),
        Err(ExecutorError::SchemaNotFound(_))
    ));
    ddl.drop_schema(&mut session, "test", true).unwrap();
    assert_eq!(vec!["schema test does not exist"], session.notes);
    ddl.recover_schema(&mut session, saved).unwrap();
    assert_eq!(id, ddl.schemas["test"].id);
}

/// 对应 Go recover database 的同名对象保护：恢复目标已存在时应报错，
/// 且不得覆盖当前 schema 或提交恢复 job。
#[test]
fn recover_schema_rejects_name_conflict_without_overwriting_metadata() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let recover_candidate = ddl.schemas["test"].clone();
    ddl.drop_schema(&mut session, "test", false).unwrap();
    let replacement_id = ddl
        .create_schema(&mut session, "TEST", &[], None, OnExist::Error)
        .unwrap();
    let history_len = ddl.backend().history().len();

    assert!(matches!(
        ddl.recover_schema(&mut session, recover_candidate),
        Err(ExecutorError::SchemaExists(name)) if name == "test"
    ));
    assert_eq!(replacement_id, ddl.schemas["test"].id);
    assert_eq!(history_len, ddl.backend().history().len());
}

/// 验证 DDL job 生命周期：多个 schema 变更提交后，
/// job 历史按提交顺序分配递增 id，且全部到达 Synced（schema 版本已全局同步）终态。
#[test]
fn schema_jobs_reach_synced_history_in_submission_order() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "one", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_schema(&mut session, "two", &[], None, OnExist::Error)
        .unwrap();
    ddl.drop_schema(&mut session, "one", false).unwrap();
    let history = ddl.backend().history();
    assert_eq!(
        vec![1, 2, 3],
        history.iter().map(|job| job.id).collect::<Vec<_>>()
    );
    assert!(history.iter().all(|job| job.state == JobState::Synced));
}
