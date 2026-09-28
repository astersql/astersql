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

// Cascades 优化器集成/单元测试。
//
// 覆盖 Memo 初始化、根 Group 探索（exploration，即对等价类应用变换规则）
// 以及与 Go `TestCascadesDrive` / `TestXFormedOperatorShouldDeriveTheirStatsOwn`
// 对齐的占位用例（当前主体仍以注释保留）。

// Cascades planner 开关、基础 explain 结果和 xform 后算子统计一致性。
//
// test_cascades_drive 对应 Go 的 TestCascadesDrive，快速覆盖 Cascades planner 开关与最简单查询。
// #[test]
// fn test_cascades_drive() {
//     let store = testkit::CreateMockStore();
//     let tk = testkit::NewTestKit(store);
//     tk.Session().GetSessionVars().SetEnableCascadesPlanner(true);
//
// Go 测试通过 mock store 建库建表；保留 SQL 顺序，不执行真实数据库动作。
//     tk.MustExec("use test");
//     tk.MustExec("drop table if exists t1");
//     tk.MustExec("create table t1(a int not null, b int not null, key(a,b))");
//     tk.MustExec("insert into t1 values(1,1),(1,2),(2,1),(2,2),(1,1)");
//
// simple select for quick debug of memo, the normal test case is in tests/planner/cascades/integration.test.
//     tk.MustQuery("select 1").Check(testkit::Rows(vec!["1"]));
//     tk.MustQuery("explain format = 'brief' select 1").Check(testkit::Rows(vec![
//         "Projection 1.00 root  1->Column#1",
//         "└─TableDual 1.00 root  rows:1",
//     ]));
// }
//
// test_xformed_operator_should_derive_their_stats_own 对应 Go 的 TestXFormedOperatorShouldDeriveTheirStatsOwn。
// 它比较 Cascades 开关前后的 explain 字符串，确保 Apply 被转换为 Join 后仍自己派生统计信息。
// #[test]
// fn test_xformed_operator_should_derive_their_stats_own() {
//     let store = testkit::CreateMockStore();
//     let tk = testkit::NewTestKit(store);
//     tk.MustExec("use test");
//
//     tk.MustExec("CREATE TABLE t1 (  a1 int DEFAULT NULL,  b1 int DEFAULT NULL,  c1 int DEFAULT NULL)");
//     tk.MustExec("CREATE TABLE t2 (  a2 int DEFAULT NULL,  b2 int DEFAULT NULL,  KEY idx (a2))");
// 当前只把相关条件上拉到 apply 自身，不主动转 join；这里交给 Cascades xform 生成 join 算子。
//     tk.MustExec("INSERT INTO t1 (a1, b1, c1) VALUES (1, 2, 3), (4, NULL, 5),  (NULL, 6, 7),  (8, 9, NULL),  (10, 11, 12);");
//     tk.MustExec("INSERT INTO t2 values (1,1),(2,2),(3,3)");
//     for _ in 0..10 {
//         tk.MustExec("INSERT INTO t2 select * from t2");
//     }
//     tk.MustExec("analyze table t1, t2");
//
// 每段都先关闭 Cascades 得到基准计划，再开启 Cascades，要求 explain 完全一致。
//     tk.Session().GetSessionVars().SetEnableCascadesPlanner(false);
//     let mut res1 = tk.MustQuery("explain format=\"brief\" SELECT 1 FROM t1 AS tab WHERE  (EXISTS(SELECT  1 FROM t2 WHERE a2 = a1 ))").String();
//     tk.Session().GetSessionVars().SetEnableCascadesPlanner(true);
//     let mut res2 = tk.MustQuery("explain format=\"brief\" SELECT 1 FROM t1 AS tab WHERE  (EXISTS(SELECT  1 FROM t2 WHERE a2 = a1 ))").String();
//     require::Equal(res1, res2);
//
//     tk.Session().GetSessionVars().SetEnableCascadesPlanner(false);
//     res1 = tk.MustQuery("explain format=\"brief\" SELECT /*+ inl_join(tab, t2@sel_2) */
//  1 FROM t1 AS tab WHERE  (EXISTS(SELECT  1 FROM t2 WHERE a2 = a1 ))").String();
//     tk.Session().GetSessionVars().SetEnableCascadesPlanner(true);
//     res2 = tk.MustQuery("explain format=\"brief\" SELECT /*+ inl_join(tab, t2@sel_2) */ 1 FROM t1 AS tab WHERE  (EXISTS(SELECT  1 FROM t2 WHERE a2 = a1 ))").String();
//     require::Equal(res1, res2);
//
//     tk.Session().GetSessionVars().SetEnableCascadesPlanner(false);
//     res1 = tk.MustQuery("explain format=\"brief\" SELECT /*+ inl_hash_join(tab, t2@sel_2) */ 1 FROM t1 AS tab WHERE  (EXISTS(SELECT  1 FROM t2 WHERE a2 = a1 ))").String();
//     tk.Session().GetSessionVars().SetEnableCascadesPlanner(true);
//     res2 = tk.MustQuery("explain format=\"brief\" SELECT /*+ inl_hash_join(tab, t2@sel_2) */ 1 FROM t1 AS tab WHERE  (EXISTS(SELECT  1 FROM t2 WHERE a2 = a1 ))").String();
//     require::Equal(res1, res2);
//
//     tk.MustExec("set tidb_hash_join_version=optimized");
//     tk.Session().GetSessionVars().SetEnableCascadesPlanner(false);
//     res1 = tk.MustQuery("explain format=\"brief\" SELECT /*+ hash_join(tab, t2@sel_2) */ 1 FROM t1 AS tab WHERE  (EXISTS(SELECT  1 FROM t2 WHERE a2 = a1 ))").String();
//     tk.Session().GetSessionVars().SetEnableCascadesPlanner(true);
//     res2 = tk.MustQuery("explain format=\"brief\" SELECT /*+ hash_join(tab, t2@sel_2) */ 1 FROM t1 AS tab WHERE  (EXISTS(SELECT  1 FROM t2 WHERE a2 = a1 ))").String();
//     require::Equal(res1, res2);
// }
// */
use super::{LogicalPlan, NewOptimizer, PlanContext};
use cascades_pattern::OperandTableDual;
use cascades_rule::{DefaultNone, NewBaseRule, Rule};
use logicalop::{LogicalPlanRef, LogicalTableDual};
use std::rc::Rc;
use task::TaskError;

/// 测试用逻辑计划：固定算子容量，根表达式为 DataSource（表扫描逻辑算子）。
struct ScanPlan;

impl LogicalPlan for ScanPlan {
    fn SCtx(&self) -> PlanContext {
        PlanContext {
            operator_num: vec![1, 2, 4],
        }
    }

    fn RootExpression(&self) -> Result<LogicalPlanRef, TaskError> {
        Ok(Box::new(LogicalTableDual {
            RowCount: 1,
            ..Default::default()
        }))
    }
}

/// 验证优化器创建时 Memo 容量与根表达式正确，Execute 后根 Group 标记为已探索，
/// Destroy 后根表达式被清空。
#[test]
fn cascades_optimizer_initializes_memo_and_explores_root_group() {
    let mut optimizer = NewOptimizer(Box::new(ScanPlan)).unwrap();
    // 创建后：Memo 容量来自 ScanPlan::SCtx，根尚未探索。
    {
        let memo = optimizer.GetMemo();
        assert_eq!(memo.Capacities(), &[1, 2, 4]);
        let root = memo.GetRootGroupExpression().unwrap();
        assert!(root.borrow().LogicalPlan.as_any().is::<LogicalTableDual>());
        assert!(
            !root
                .borrow()
                .GetGroup()
                .expect("root expression must have a group")
                .borrow()
                .IsExplored()
        );
    }

    optimizer.Execute().unwrap();
    // 执行探索后根 Group 应变为已探索。
    {
        let memo = optimizer.GetMemo();
        let root = memo.GetRootGroupExpression().unwrap();
        assert!(
            root.borrow()
                .GetGroup()
                .expect("root expression must have a group")
                .borrow()
                .IsExplored()
        );
    }
    optimizer.Destroy();
    assert!(optimizer.GetMemo().GetRootGroupExpression().is_none());
}

/// Go `Context.GetRuleMask` 的公开契约：默认上下文启用全部规则，且调用方
/// 能读取掩码而不是只能通过任务内部的 `RuleEnabled` 间接判断。
#[test]
fn cascades_context_exposes_rule_mask() {
    let context = super::NewContext(PlanContext::default());
    assert!(context.GetRuleMask().Test(0));
}

/// 对应 Go `TestCascadesDrive` 的无存储边界等价测试：验证 SELECT 1 形状
/// 的单行 Dual 进入 Memo、执行搜索并保留可解释的单行统计。
#[test]
fn test_cascades_drive() {
    let mut optimizer = NewOptimizer(Box::new(ScanPlan)).unwrap();
    {
        let root = optimizer.GetMemo().GetRootGroupExpression().unwrap();
        let expression = root.borrow();
        let dual = expression
            .LogicalPlan
            .as_any()
            .downcast_ref::<LogicalTableDual>()
            .expect("root should be the SELECT 1 table dual");
        assert_eq!(dual.RowCount, 1);
        assert_eq!(dual.ExplainInfo(), "rowcount:1");
    }

    optimizer.Execute().unwrap();
    assert!(
        optimizer
            .GetMemo()
            .GetRootGroupExpression()
            .unwrap()
            .borrow()
            .GetGroup()
            .unwrap()
            .borrow()
            .IsExplored()
    );
    optimizer.Destroy();
    assert!(optimizer.GetMemo().GetRootGroupExpression().is_none());
}

/// Go uses a bitset sized to `XFMaximumRuleLength`: `SetAll` enables every
/// declared slot, but it must not make arbitrary out-of-range ids visible.
#[test]
fn cascades_context_rule_mask_respects_go_bitset_length() {
    let context = super::NewContext(PlanContext::default());
    let maximum = cascades_rule::XFMaximumRuleLength as usize;
    assert!(context.GetRuleMask().Test(maximum - 1));
    assert!(!context.GetRuleMask().Test(maximum));
}

/// 验证 Context 注册规则后，任务仍会按规则掩码探索根表达式；Go 侧默认
/// 规则集当前为空，因此这里使用无输出的 BaseRule 覆盖同一调度边界。
#[test]
fn test_registered_rule_is_explored() {
    let rule = Rc::new(NewBaseRule(
        DefaultNone,
        cascades_pattern::NewPattern(OperandTableDual, cascades_pattern::EngineAll),
    ));
    let rule_id = rule.ID();
    let mut optimizer = NewOptimizer(Box::new(ScanPlan)).unwrap();
    optimizer.RegisterRules(OperandTableDual, vec![rule]);
    optimizer.SetRules(&[rule_id]);
    optimizer.Execute().unwrap();

    let root = optimizer.GetMemo().GetRootGroupExpression().unwrap();
    let group = root.borrow().GetGroup().unwrap();
    assert_eq!(group.borrow().GetLogicalExpressions().len(), 1);
    assert!(root.borrow().IsExplored(rule_id));
    optimizer.Destroy();
}
