// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use astersql_config::{get_global_config, store_global_config};
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};

struct GlobalConfigGuard(astersql_config::Config);

impl Drop for GlobalConfigGuard {
    fn drop(&mut self) {
        store_global_config(self.0.clone());
    }
}

/// Go `TestEliminateProjectionSuite` 的可执行等价用例。
#[test]
fn test_eliminate_projection_suite() {
    test_with_apply();
    test_eliminate_projection_with_expression_index();
}

fn new_test_kit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut test_kit = TestKit::new(store);
    test_kit.MustExec("use test", Vec::new());
    test_kit
}

fn test_with_apply() {
    let mut test_kit = new_test_kit();
    for sql in [
        "drop table if exists t11, t22",
        "create table t11(id int, name varchar(20))",
        "create table t22(id int, name varchar(20))",
        "insert into t11(id, name) values(1, 'test')",
        "insert into t22(id, name) values(1, 'test')",
    ] {
        test_kit.MustExec(sql, Vec::new());
    }

    for sql in [
        r#"with temp as (
select id from t22 order by name
)
select (select name from t11 where id = (case when temp.id = 1 then temp.id else temp.id end)) = 'test'
from temp where id = 1"#,
        r#"with temp as (
select id from t22
)
select (select name from t11 where id = (case when temp.id = 1 then temp.id else temp.id end)) = 'test'
from temp where id = 1"#,
    ] {
        test_kit.MustQuery(sql, Vec::new()).Check(Rows(&["1"]));
        let plan = test_kit
            .MustQuery(&format!("explain {sql}"), Vec::new())
            .Rows();
        assert!(
            plan.iter().flatten().any(|cell| cell.contains("Apply")),
            "plan should contain Apply, sql: {sql}, plan: {plan:?}"
        );
    }
}

/// Rust 规则级回归：恒等投影可消除，计算投影必须保留。
#[test]
fn identity_projection_eliminates_but_computed_projection_remains() {
    use astersql_planner_core::rule_eliminate_projection::ProjectionEliminator;
    use astersql_planner_core::task::{Expression, PlanKind, PlanNode};

    let scan = PlanNode::new(PlanKind::TableScan);
    let mut identity = PlanNode::new(PlanKind::Projection).with_children(vec![scan.clone()]);
    identity.expressions = vec![Expression {
        column: Some(0),
        ..Default::default()
    }];
    let mut parent = PlanNode::new(PlanKind::Projection).with_children(vec![identity]);
    parent.expressions = vec![crate::support::expr("a_plus_1", None)];
    let (result, changed) = ProjectionEliminator.Optimize(parent);
    assert!(
        !changed,
        "Go ProjectionEliminator reports planChanged=false"
    );
    assert_eq!(result.kind, PlanKind::Projection);
    assert_eq!(result.children[0].kind, PlanKind::TableScan);

    let mut computed = PlanNode::new(PlanKind::Projection).with_children(vec![scan]);
    computed.expressions = vec![crate::support::expr("a_plus_1", None)];
    let (result, changed) = ProjectionEliminator.Optimize(computed);
    assert!(!changed);
    assert_eq!(result.kind, PlanKind::Projection);
}

fn test_eliminate_projection_with_expression_index() {
    let original = get_global_config().as_ref().clone();
    let _restore = GlobalConfigGuard(original.clone());
    let mut configured = original;
    configured.experimental.allows_expression_index = true;
    store_global_config(configured);

    let mut test_kit = new_test_kit();
    test_kit.MustExec("drop table if exists t1, t2;", Vec::new());
    test_kit.MustExec(
        r#"CREATE TABLE t1 (
		namespace_id char(64) NOT NULL,
		run_id char(64) NOT NULL,
		start_time datetime(6) NOT NULL,
		execution_time datetime(6) NOT NULL,
		workflow_id varchar(255) NOT NULL,
		status int NOT NULL,
		close_time datetime(6) DEFAULT NULL,
		PRIMARY KEY (namespace_id,run_id) /*T![clustered_index] CLUSTERED */,
		KEY by_execution_time (namespace_id,execution_time,(coalesce(close_time, cast(_utf8mb4'9999-12-31 23:59:59' as datetime))),start_time,run_id),
		KEY by_workflow_id (namespace_id,workflow_id,(coalesce(close_time, cast(_utf8mb4'9999-12-31 23:59:59' as datetime))),start_time,run_id)
	) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;"#,
        Vec::new(),
    );
    test_kit.MustExec(
        r#"CREATE TABLE t2 (
		namespace_id CHAR(64) NOT NULL,
		run_id CHAR(64) NOT NULL
	);"#,
        Vec::new(),
    );

    // Go 回归说明该问题需要重复二十次才能稳定复现。
    for _ in 0..20 {
        test_kit
            .MustQuery(
                r#"SELECT close_time
		FROM t1
		LEFT JOIN t2
		USING (namespace_id,run_id)
		ORDER BY coalesce(close_time, CAST('9999-12-31 23:59:59' AS DATETIME));"#,
                Vec::new(),
            )
            .Check(Rows(&[]));
    }
}
