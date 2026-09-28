// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// AST 与物理计划可缓存性检查器（cacheable checker）用例。
//
// 计划缓存入口会先判断语句/计划是否可缓存：`CacheableWithCtx` 面向 AST（可开关
// 允许子查询）；`NonPreparedPlanCacheableWithCtx` 限制非预处理语句的常量个数；
// `isPlanCacheable` 拒绝 Apply、Dual 等不宜复用的物理计划形态。

use astersql_planner_core::{
    AstNode, Cacheable, CacheableWithCtx, NonPreparedPlanCacheableWithCtx, PlanKind, PlanNode,
    isPlanCacheable,
};

fn value(value: &str) -> AstNode {
    AstNode::Value(value.to_owned())
}

fn plan(id: i32, kind: PlanKind, children: Vec<PlanNode>) -> PlanNode {
    PlanNode::New(id, kind, children)
}

/// 对齐 Go `TestCacheable` 的顶层语句白名单、开关和递归 AST 检查。
#[test]
fn cacheable_checks_statement_kind_switches_and_nested_nodes() {
    for statement in [
        AstNode::Select(Vec::new()),
        AstNode::Insert {
            table_id: 1,
            replace: false,
        },
        AstNode::Update { table_id: 1 },
        AstNode::Delete { table_id: 1 },
    ] {
        assert!(Cacheable(&statement));
    }
    assert!(!Cacheable(&AstNode::Show));

    let subquery = AstNode::Select(vec![AstNode::Subquery(vec![value("1")])]);
    assert_eq!(
        CacheableWithCtx(&subquery, false, true),
        (false, "query has sub-queries is un-cacheable".to_owned())
    );
    assert_eq!(
        CacheableWithCtx(&subquery, true, true),
        (true, String::new())
    );

    let nested_subquery = AstNode::Explain(Box::new(AstNode::Other {
        read_only: true,
        children: vec![AstNode::AggregateFunc {
            name: "count".to_owned(),
            args: vec![AstNode::WindowFunc {
                name: "row_number".to_owned(),
                args: vec![AstNode::Subquery(Vec::new())],
            }],
        }],
    }));
    assert!(!CacheableWithCtx(&nested_subquery, false, true).0);
    assert_eq!(
        CacheableWithCtx(
            &AstNode::Select(vec![AstNode::Set { global: true }]),
            true,
            true,
        )
        .1,
        "global SET is un-cacheable"
    );
    assert_eq!(
        CacheableWithCtx(&AstNode::Select(Vec::new()), true, false).1,
        "parameterized limit disabled"
    );
}

/// 对齐 Go `TestNonPreparedPlanCacheable` 的常量上限及嵌套遍历。
#[test]
fn non_prepared_cacheability_counts_all_nested_constants() {
    let constants = AstNode::Select(vec![
        value("1"),
        AstNode::Other {
            read_only: true,
            children: vec![AstNode::AggregateFunc {
                name: "sum".to_owned(),
                args: vec![value("2")],
            }],
        },
    ]);
    assert_eq!(
        NonPreparedPlanCacheableWithCtx(&constants, 1),
        (false, "query has too many constants".to_owned())
    );
    assert_eq!(
        NonPreparedPlanCacheableWithCtx(&constants, 2),
        (true, String::new())
    );
    assert_eq!(
        NonPreparedPlanCacheableWithCtx(
            &AstNode::Select(vec![AstNode::Subquery(Vec::new())]),
            usize::MAX,
        )
        .1,
        "query has sub-queries is un-cacheable"
    );
}

/// 对齐 Go `isPhysicalPlanCacheable` 的危险算子、大小上限及子树传播。
#[test]
fn physical_plan_cacheability_checks_each_hazard_and_children() {
    let safe_scan = plan(
        2,
        PlanKind::TableScan {
            table: "t".to_owned(),
        },
        Vec::new(),
    );
    assert_eq!(
        isPlanCacheable(&safe_scan, 0, i64::MAX),
        (true, String::new())
    );

    let too_small = safe_scan.MemoryUsage() - 1;
    assert_eq!(
        isPlanCacheable(&safe_scan, 0, too_small).1,
        "plan is too large(decided by the variable @@tidb_plan_cache_max_plan_size)"
    );

    let apply = plan(1, PlanKind::Apply, vec![safe_scan.clone()]);
    assert_eq!(
        isPlanCacheable(&apply, 0, i64::MAX).1,
        "PhysicalApply plan is un-cacheable"
    );

    for kind in [
        PlanKind::Shuffle {
            info: "sender".to_owned(),
        },
        PlanKind::ShuffleReceiver {
            info: "receiver".to_owned(),
        },
    ] {
        assert_eq!(
            isPlanCacheable(&plan(3, kind, Vec::new()), 0, i64::MAX).1,
            "get a Shuffle plan"
        );
    }

    let dual = plan(4, PlanKind::Dual, Vec::new());
    assert_eq!(
        isPlanCacheable(&dual, 1, i64::MAX).1,
        "get a TableDual plan"
    );
    assert!(isPlanCacheable(&dual, 0, i64::MAX).0);

    let nested_apply = plan(5, PlanKind::Projection, vec![apply]);
    assert_eq!(
        isPlanCacheable(&nested_apply, 0, i64::MAX).1,
        "PhysicalApply plan is un-cacheable"
    );
}
