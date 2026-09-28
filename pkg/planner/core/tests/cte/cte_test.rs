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

// 本文件对应 pkg/planner/core/tests/cte/cte_test.go。Go 版本跨 schema 访问
// SECURITY DEFINER 视图里的非递归 CTE，断言 CTEFullScan / Seed Part plan_tree。

// CTE（公用表表达式）规划相关测试。
//
// 对应 Go `cte_test.go`：覆盖跨 schema 访问 SECURITY DEFINER 视图内嵌的非递归 CTE，
// 并断言 CTEFullScan / Seed Part 的 plan_tree 形状。CTE 即 WITH 子句定义的命名子查询；
// SECURITY DEFINER 表示以定义者权限执行视图。

#![allow(non_snake_case)]

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

// TestCTEWithDifferentSchema 对应 Go 同名测试。
/// 对应 Go `TestCTEWithDifferentSchema`：跨库 DEFINER 视图 + 非递归 CTE 计划形状。
#[test]
fn TestCTEWithDifferentSchema() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());

    // 与 Go 测试保持相同的用户、授权和跨 schema 前置条件。
    tk.MustExec("CREATE USER 'db_a'@'%'", Vec::new());
    tk.MustExec("CREATE USER 'db_b'@'%'", Vec::new());
    tk.MustExec("GRANT ALL PRIVILEGES ON `db_a`.* TO 'db_a'@'%'", Vec::new());
    tk.MustExec("GRANT ALL PRIVILEGES ON `db_b`.* TO 'db_a'@'%'", Vec::new());
    tk.MustExec("GRANT ALL PRIVILEGES ON `db_b`.* TO 'db_b'@'%'", Vec::new());
    tk.MustExec("GRANT ALL PRIVILEGES ON `db_b`.* TO 'db_b'@'%'", Vec::new());
    tk.MustExec("create database db_a", Vec::new());
    tk.MustExec("create database db_b", Vec::new());
    tk.MustExec("use db_a", Vec::new());
    tk.MustExec(
        r#"CREATE TABLE tmp_table1 (
   id decimal(18,0) NOT NULL,
   row_1 varchar(255) DEFAULT NULL,
   PRIMARY KEY (id) /*T![clustered_index] CLUSTERED */
 ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    let view_sql = r#"create ALGORITHM=UNDEFINED DEFINER=db_a@'%' SQL SECURITY DEFINER VIEW view_test_v1 as (
                         with rs1 as(
                            select otn.*
                             from tmp_table1 otn
                          )
                        select ojt.* from rs1 ojt
                        )"#;
    tk.MustExec(view_sql, Vec::new());
    tk.MustExec("use db_b", Vec::new());
    tk.MustQuery(
        "explain format = 'plan_tree' select * from db_a.view_test_v1",
        Vec::new(),
    )
    .Check(
        [
            "CTEFullScan root CTE:rs1 AS ojt data:CTE_0",
            "CTE_0 root  Non-Recursive CTE",
            "└─TableReader(Seed Part) root  data:TableFullScan",
            "  └─TableFullScan cop[tikv] table:otn keep order:false, stats:pseudo",
        ]
        .into_iter()
        .map(|row| row.splitn(4, ' ').map(str::to_owned).collect())
        .collect(),
    );
}
