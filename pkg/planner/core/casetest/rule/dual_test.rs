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

// TableDual 折叠规则的 casetest。
//
// 对应 Go `dual_test.go`：矛盾谓词、常量投影与 NULL 比较应规划为
// TableDual（空结果节点），且矛盾谓词查询必须返回空结果。

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};

/// 对齐 Go `testkit.RunTestUnderCascades`：两种 planner 模式使用独立存储和会话。
fn run_test_under_cascades<F>(mut test: F)
where
    F: FnMut(&mut TestKit, &str),
{
    for cascades in ["off", "on"] {
        let (store, _domain) = CreateMockStoreAndDomain();
        let mut test_kit = TestKit::new(store);
        test_kit.MustExec("use test", Vec::new());
        test_kit.MustExec(
            &format!("set @@tidb_enable_cascades_planner = {cascades}"),
            Vec::new(),
        );
        test(&mut test_kit, cascades);
    }
}

/// Go `TestDual` 的可执行等价用例。
#[test]
fn test_dual() {
    run_test_under_cascades(|test_kit, cascades| {
        test_kit.MustExec(
            "CREATE TABLE t (id INT PRIMARY KEY AUTO_INCREMENT,d INT);",
            Vec::new(),
        );
        let mut expected_plans = Vec::new();
        let mut actual_plans = Vec::new();

        let column_contradiction =
            "select a from (select d as a from t where d = 0) k where k.a = 5";
        expected_plans.push(Rows(&["TableDual root  rows:0"]));
        actual_plans.push(
            test_kit
                .MustQuery(
                    &format!("explain format = 'plan_tree' {column_contradiction}"),
                    Vec::new(),
                )
                .Rows(),
        );
        test_kit
            .MustQuery(column_contradiction, Vec::new())
            .Check(Rows(&[]));

        let constant_contradiction =
            "select a from (select 1+2 as a from t where d = 0) k where k.a = 5";
        expected_plans.push(Rows(&[
            "Projection root  3->Column",
            "└─TableDual root  rows:0",
        ]));
        actual_plans.push(
            test_kit
                .MustQuery(
                    &format!("explain format = 'plan_tree' {constant_contradiction}"),
                    Vec::new(),
                )
                .Rows(),
        );
        test_kit
            .MustQuery(constant_contradiction, Vec::new())
            .Check(Rows(&[]));

        for operator in ["!=", ">", ">=", "<", "<=", "="] {
            let sql =
                format!("explain format = 'plan_tree' select * from t where d {operator} null;");
            expected_plans.push(Rows(&["TableDual root  rows:0"]));
            actual_plans.push(test_kit.MustQuery(&sql, Vec::new()).Rows());
        }

        assert_eq!(
            expected_plans, actual_plans,
            "Go TestDual plan parity: planner={cascades}"
        );
    });
}
