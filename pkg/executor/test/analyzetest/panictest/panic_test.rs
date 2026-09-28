// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// ANALYZE worker / 结果处理路径的 panic 转错误测试。
//
// 对应 `pkg/executor/test/analyzetest/panictest/panic_test.go`。
//
// Go 通过 failpoint 强制 `handleResultsError` / analyze worker panic，再要求
// `analyze table` 返回错误而不是泄漏 panic。Rust 复用真实 mock store、TestKit
// 和生产 failpoint，完整执行建表、插入与 ANALYZE 路径。
//
// failpoint：故障注入点，用于在测试中强制触发特定错误路径。

#![allow(non_snake_case)]

use astersql_testkit::{NewTestKit, TestKit, mockstore::CreateMockStoreAndDomain};

/// 非分区表 `tbl_2` 建表 SQL（与 Go panic 用例 fixture 一致）。
const CREATE_TBL_2_SINGLE: &str = "create table tbl_2 ( col_20 decimal default 84232 , col_21 tinyint not null , col_22 int default 80814394 , col_23 mediumint default -8036687 not null , col_24 smallint default 9185 not null , col_25 tinyint unsigned default 65 , col_26 char(115) default 'ZyfroRODMbNDRZnPNRW' not null , col_27 bigint not null , col_28 tinyint not null , col_29 char(130) default 'UMApsVgzHblmY' , primary key idx_14 ( col_28,col_22 ) , unique key idx_15 ( col_24,col_22 ) , key idx_16 ( col_21,col_20,col_24,col_25,col_27,col_28,col_26,col_29 ) , key idx_17 ( col_24,col_25 ) , unique key idx_18 ( col_25,col_23,col_29,col_27,col_26,col_22 ) , key idx_19 ( col_25,col_22,col_26,col_23 ) , unique key idx_20 ( col_22,col_24,col_28,col_29,col_26,col_20 ) , key idx_21 ( col_25,col_24,col_26,col_29,col_27,col_22,col_28 ) );";

/// RANGE 分区表 `tbl_2` 建表 SQL，用于多 worker / 分区 analyze panic 场景。
const CREATE_TBL_2_PARTITIONED: &str = "create table tbl_2 ( col_20 decimal default 84232 , col_21 tinyint not null , col_22 int default 80814394 , col_23 mediumint default -8036687 not null , col_24 smallint default 9185 not null , col_25 tinyint unsigned default 65 , col_26 char(115) default 'ZyfroRODMbNDRZnPNRW' not null , col_27 bigint not null , col_28 tinyint not null , col_29 char(130) default 'UMApsVgzHblmY' , primary key idx_14 ( col_28,col_22 ) , unique key idx_15 ( col_24,col_22 ) , key idx_16 ( col_21,col_20,col_24,col_25,col_27,col_28,col_26,col_29 ) , key idx_17 ( col_24,col_25 ) , unique key idx_18 ( col_25,col_23,col_29,col_27,col_26,col_22 ) , key idx_19 ( col_25,col_22,col_26,col_23 ) , unique key idx_20 ( col_22,col_24,col_28,col_29,col_26,col_20 ) , key idx_21 ( col_25,col_24,col_26,col_29,col_27,col_22,col_28 ) ) partition by range ( col_22 ) ( partition p0 values less than (-1938341588), partition p1 values less than (-1727506184), partition p2 values less than (-1700184882), partition p3 values less than (-1596142809), partition p4 values less than (445165686) );";

/// 单行插入 fixture，用于非分区表填充。
const SINGLE_ROW: &str = "insert ignore into tbl_2 values ( 942,33,-1915007317,3408149,-3699,193,'Trywdis',1876334369465184864,115,null );";

/// Go failpoint：单线程 `handleResultsError` 强制 panic。
const FP_SINGLE_GOROUTINE: &str =
    "github.com/pingcap/tidb/pkg/executor/handleResultsErrorSingleThreadPanic";
/// Go failpoint：analyze worker 路径强制 panic。
const FP_ANALYZE_WORKER: &str = "github.com/pingcap/tidb/pkg/executor/handleAnalyzeWorkerPanic";

/// 分区表多行插入 SQL 列表（含 `SINGLE_ROW`），与 Go fixture 行数一致。
fn partitioned_rows() -> Vec<&'static str> {
    vec![
        SINGLE_ROW,
        "insert ignore into tbl_2 values ( null,55,-388460319,-2292918,10130,162,'UqjDlYvdcNY',4872802276956896607,-51,'ORBQjnumcXP' );",
        "insert ignore into tbl_2 values ( 42,-19,-9677826,-1168338,16904,79,'TzOqH',8173610791128879419,65,'lNLcvOZDcRzWvDO' );",
        "insert ignore into tbl_2 values ( 2,26,369867543,-6773303,-24953,41,'BvbdrKTNtvBgsjjnxt',5996954963897924308,-95,'wRJYPBahkIGDfz' );",
        "insert ignore into tbl_2 values ( 6896,3,444460824,-2070971,-13095,167,'MvWNKbaOcnVuIrtbT',6968339995987739471,-5,'zWipNBxGeVmso' );",
        "insert ignore into tbl_2 values ( 58761,112,-1535034546,-5837390,-14204,157,'',-8319786912755096816,15,'WBjsozfBfrPPHmKv' );",
        "insert ignore into tbl_2 values ( 84923,113,-973946646,406140,25040,51,'THQdwkQvppWZnULm',5469507709881346105,94,'oGNmoxLLgHkdyDCT' );",
        "insert ignore into tbl_2 values ( 0,-104,-488745187,-1941015,-2646,39,'jyKxfs',-5307175470406648836,46,'KZpfjFounVgFeRPa' );",
        "insert ignore into tbl_2 values ( 4,97,2105289255,1034363,28385,192,'',4429378142102752351,8,'jOk' );",
    ]
}

/// 创建隔离的真实 mock store / TestKit，并执行 Go 用例共同的 `USE test`。
fn new_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = NewTestKit(store);
    testkit.MustExec("use test", Vec::new());
    testkit
}

/// 对应 Go `TestPanicInHandleResultErrorWithSingleGoroutine`。
/// 单线程结果处理 panic 应转为 `AnalyzeWorkerPanic`，而不是向上传播。
#[test]
fn TestPanicInHandleResultErrorWithSingleGoroutine() {
    let mut testkit = new_testkit();
    testkit.MustExec(CREATE_TBL_2_SINGLE, Vec::new());
    testkit.MustExec(SINGLE_ROW, Vec::new());
    let failpoint = astersql_testkit_testfailpoint::enable(
        FP_SINGLE_GOROUTINE,
        "panic(TestPanicInHandleResultErrorWithSingleGoroutine)",
    );

    let error = testkit.ExecToErr("analyze table tbl_2;");
    assert!(
        error.message().contains("analyze worker panic"),
        "unexpected analyze error: {error}"
    );
    drop(failpoint);
    testkit.MustExec("analyze table tbl_2;", Vec::new());
}

/// 对应 Go `TestPanicInHandleAnalyzeWorkerPanic`。
/// analyze worker panic 与 OOM panic 走不同错误种类分支。
#[test]
fn TestPanicInHandleAnalyzeWorkerPanic() {
    let mut testkit = new_testkit();
    testkit.MustExec(CREATE_TBL_2_PARTITIONED, Vec::new());
    for row in partitioned_rows() {
        testkit.MustExec(row, Vec::new());
    }
    let failpoint = astersql_testkit_testfailpoint::enable(
        FP_ANALYZE_WORKER,
        "panic(TestPanicInHandleAnalyzeWorkerPanic)",
    );

    let error = testkit.ExecToErr("analyze table tbl_2;");
    assert!(
        error.message().contains("analyze worker panic"),
        "unexpected analyze error: {error}"
    );
    drop(failpoint);
    testkit.MustExec("analyze table tbl_2;", Vec::new());
}
