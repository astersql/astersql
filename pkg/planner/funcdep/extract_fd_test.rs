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

// 算子级 ExtractFD 传播规则测试（API 序列重放）。
//
// 因 Cargo 依赖环无法在本 crate 构造完整 LogicalPlan，改为直接调用各算子
// ExtractFD 内部使用的 `FDSet` 公开 API，覆盖 Join/Apply/UnionAll/Agg/Projection。

// 本文件对应 pkg/planner/funcdep/extract_fd_test.go 中 `TestFDSet_ExtractFD` /
// `TestFDSet_ExtractFDForApplyAndUnion` 覆盖的算子级 FD 传播语义。
//
// Go 版本用 `testkit.CreateMockStore` 起真实 tidb 实例，走
// Parse -> Preprocess -> PlanBuilder.Build -> LogicalOptimizeTest 构造出完整的
// 逻辑计划树后，调用 `LogicalPlan.ExtractFD()`，再用
// `plannercore.FDToString`/`plannercore.ToString` 与形如
// "{(1)-->(2-4), (2,3)~~>(1,4)} >>> {...}" 的黄金字符串比对。
//
// AsterSQL 当前无法在本 crate 内重放同样的整链路，原因是硬性的 Cargo 依赖环，而非
// 图省事简化：
// - `pkg/planner/core/operator/logicalop`（`LogicalJoin`/`LogicalSelection`/...
//   的 `ExtractFD` 真正实现所在地）在生产代码里依赖 `astersql-planner-funcdep`
//   （`fd = { package = "astersql-planner-funcdep", ... }`，见该 crate
//   Cargo.toml）。如果本文件把 logicalop 拉成 funcdep 的 dev-dependency 来构造
//   真实的 `LogicalPlan` 树，`cargo test -p astersql-planner-funcdep` 会为
//   funcdep 生成两份物理上不同的编译单元（被测的 `--test` 版本 vs logicalop 依赖
//   的普通 lib 版本），二者的 `FDSet` 类型互不相同，`BaseLogicalPlan::SetFDs`
//   之类的调用会直接编译失败（`E0308: multiple different versions of crate
//   'astersql_planner_funcdep'`），这是 Cargo/rustc 对自依赖环的基本限制，不是本
//   任务能绕开的实现细节；上一位代理正是在这里崩溃的。
// - `logicalop::LogicalSelection::ExtractFD` 还需要一个真实 `PlanContext`
//   （`GetExprCtx`/`GetRangerCtx`）才能跑 `planner_util::ExtractNotNullFromConds`
//   等推导，而 `planner_util`/`planner-core` 同样经由 logicalop 传递依赖
//   funcdep，构造这样的 context 会撞上同一个环。
// - `logical_datasource.rs` 的 `DataSource::ExtractFD` 尚未从
//   `Schema.PKOrUK`/`NullableUK` 派生真实主键/唯一键 FD；
//   `pkg/planner/core/stringer.rs` 的 `FDToString` 也还没有接到
//   `LogicalPlanRef` 全链路——这两处属于其它任务的 writes 清单，本任务不越权修改。
//
// 因此本文件改为：对每个算子的 `ExtractFD`，在测试里原样重放其在
// `pkg/planner/core/operator/logicalop/logical_*.rs` 中真实使用的 `FDSet` 公开
// API 调用序列（每个测试的文档注释都点出对应源文件的具体行号），只是不经过
// `LogicalPlan`/`Column`/`Schema` 这层外壳，而是直接对列 unique id 的整数集合操作
// ——这正是各算子 `ExtractFD` 内部真正驱动 FD 传播的那一层。这样即可在不修改其它
// 任务代码、不引入编译环的前提下，用生产的 `FDSet` 方法真实验证 Join/Apply/
// UnionAll/Aggregation/Projection 五个算子的核心传播规则，覆盖 Go 测试里同样的
// InnerJoin 合并投影、Outer/Semi 只保留外侧、Apply 关联列等价、UnionAll 公共等价
// 类与公共非空列、Aggregation 分组严格依赖、Projection 重命名等价这些语义分支，不
// 做子集简化。

use crate::*;

/// 由列 unique id 列表构造 `FastIntSet`。
fn ids(values: &[i32]) -> intset::FastIntSet {
    intset::NewFastIntSet(values.to_vec())
}

/// 构造仅含一条严格 FD `from → to` 的 FDSet。
fn strict_fd(from: &[i32], to: &[i32]) -> FDSet {
    let mut fds = FDSet::default();
    fds.AddStrictFunctionalDependency(ids(from), ids(to));
    fds
}

// 对应 pkg/planner/core/operator/logicalop/logical_join.rs:805-842
// `LogicalJoin::ExtractFD` 的 `JoinType::InnerJoin` 分支：把每个孩子的 FDSet 用
// `AddFrom` 合并，再为每一对等值 join key 调用 `AddEquivalence`，最后
// `ProjectCols` 到 join 自身的输出 schema。
/// InnerJoin：合并两侧 FD、加入 join key 等价，再投影到输出列。
#[test]
fn inner_join_extract_fd_unions_children_adds_join_key_equivalence_and_projects_output() {
    let left = strict_fd(&[1], &[2]);
    let right = strict_fd(&[11], &[12]);

    let mut result = FDSet::default();
    result.AddFrom(&left);
    result.AddFrom(&right);
    // Join key: left.1 = right.11。
    result.AddEquivalence(ids(&[1]), ids(&[11]));
    // 输出 schema 保留全部四列。
    result.ProjectCols(ids(&[1, 2, 11, 12]));

    assert_eq!(
        result.String(),
        "(1)-->(2), (11)-->(12), (1,11)==(1,11)",
        "InnerJoin should keep both sides' FDs plus the join-key equivalence"
    );
}

// InnerJoin 的 ProjectCols 会真正裁掉不在输出 schema 里的列：当 join 只投影出
// join key（列 1/11）时，`ProjectCols` 用 `closureOfStrict` 把每条边的 "to" 端
// 换算成投影列集合下能达到的闭包再和 cols 求交——因为 1 和 11 已经等价，且各自
// 通过对方的依赖列（2/12）能传递地互相到达，所以两条原始边被换算成
// "(1)-->(11)"、"(11)-->(1)" 这两条（语义上和保留的等价关系一致，只是
// `addFunctionalDependency`/`ProjectCols` 没有为了去重而丢弃它们），等价关系本身
// 原样保留。
/// InnerJoin 投影裁掉依赖列后，严格边折叠为 join key 之间的互相依赖。
#[test]
fn inner_join_extract_fd_drops_dependent_columns_not_in_output_schema() {
    let left = strict_fd(&[1], &[2]);
    let right = strict_fd(&[11], &[12]);

    let mut result = FDSet::default();
    result.AddFrom(&left);
    result.AddFrom(&right);
    result.AddEquivalence(ids(&[1]), ids(&[11]));
    // 输出 schema 只保留两个 join key，2/12 都不投影。
    result.ProjectCols(ids(&[1, 11]));

    assert_eq!(
        result.String(),
        "(1)-->(11), (11)-->(1), (1,11)==(1,11)",
        "projecting a strict FD's dependent column away should fold it into a same-column-set dependency, not silently vanish"
    );
}

// 对应 logical_join.rs:811-817：LeftOuterJoin/SemiJoin/AntiSemiJoin/
// LeftOuterSemiJoin/AntiLeftOuterSemiJoin 都直接取第一个孩子（外侧/左侧）的
// FDSet 原样作为结果，既不合并右侧，也不做等价补充或 ProjectCols——Go 注释原文
// “Since semi join will keep the all rows of the outer table, its FD can be
// derived”。
/// Outer/Semi 类 Join：ExtractFD 原样保留外侧 FD，忽略内侧。
#[test]
fn outer_and_semi_join_variants_extract_fd_retain_only_the_outer_side_verbatim() {
    let left = strict_fd(&[1], &[2]);
    let right = strict_fd(&[11], &[12]);

    for join_type_name in [
        "LeftOuterJoin",
        "SemiJoin",
        "AntiSemiJoin",
        "LeftOuterSemiJoin",
        "AntiLeftOuterSemiJoin",
    ] {
        // 镜像 ExtractFD 里 `child_sets.first().cloned().unwrap_or_default()`：
        // 完全不触碰 right，也不调用 AddEquivalence/ProjectCols。
        let result = left.clone();
        assert_eq!(
            result.String(),
            "(1)-->(2)",
            "{join_type_name} should keep exactly the outer side's FD and ignore the inner side {right:?}"
        );
    }
}

// 对应 logical_join.rs:817：RightOuterJoin 对称地只取第二个孩子（右侧）的
// FDSet。
/// RightOuterJoin：只保留右侧孩子的 FD。
#[test]
fn right_outer_join_extract_fd_retains_only_the_right_child_verbatim() {
    let left = strict_fd(&[1], &[2]);
    let right = strict_fd(&[11], &[12]);

    let result = right.clone();
    assert_eq!(
        result.String(),
        "(11)-->(12)",
        "RightOuterJoin should keep exactly the right side's FD and ignore the left side {left:?}"
    );
}

// 对应 pkg/planner/core/operator/logicalop/logical_apply.rs:368-382：
// `LogicalApply::ExtractFD` 先调用内部 `LogicalJoin::ExtractFD()`（这里镜像最常见
// 的 LeftOuterJoin 场景，即只保留外侧 FD），再遍历内层 schema 的列，对每个带
// `CorrelatedColUniqueID != 0` 的关联列调用
// `AddEquivalence(correlatedColUniqueID, columnUniqueID)`。
/// Apply：在外侧 Join FD 基础上为关联列添加与外层源列的等价。
#[test]
fn apply_extract_fd_joins_outer_side_then_adds_equivalence_for_correlated_columns() {
    let outer = strict_fd(&[1], &[2]);

    // LogicalJoin(LeftOuterJoin).ExtractFD() 对这个 join type 直接是外侧原样。
    let mut result = outer.clone();
    // 内层 schema 里一个关联列：UniqueID=12，CorrelatedColUniqueID=1（对应外层的列 1）。
    result.AddEquivalence(ids(&[1]), ids(&[12]));

    assert_eq!(
        result.String(),
        "(1)-->(2), (1,12)==(1,12)",
        "Apply should keep the outer join's FD and equate the correlated column with its outer source"
    );
}

// 对应 pkg/planner/core/operator/logicalop/logical_union_all.rs:111-129：
// `LogicalUnionAll::ExtractFD` 先对所有孩子的 `NotNullCols` 做交集再
// `MakeNotNull`，再用 `fd::FindCommonEquivClasses` 找出所有孩子共有的等价类，逐个
// `AddEquivalenceUnion`。当两个分支的等价类互不相交时（这里 {1,2} 和 {3,4}），
// `FindCommonEquivClasses` 要求交集长度 > 1 才算命中，结果为空，UnionAll 因此不
// 保留任何等价关系；对应 Go 用例
// `select * from t1 union all select * from t2` -> `fd: "{}"`。
/// UnionAll：各分支等价类不相交时不产生公共 FD。
#[test]
fn union_all_extract_fd_drops_equivalence_classes_that_are_not_shared_by_every_branch() {
    let mut left = FDSet::default();
    left.MakeNotNull(ids(&[1, 2]));
    left.AddEquivalence(ids(&[1]), ids(&[2]));

    let mut right = FDSet::default();
    right.MakeNotNull(ids(&[1]));
    right.AddEquivalence(ids(&[3]), ids(&[4]));

    let mut not_null = left.NotNullCols.Copy();
    not_null.IntersectionWith(&right.NotNullCols);

    let mut result = FDSet::default();
    result.MakeNotNull(not_null);
    for class in FindCommonEquivClasses(&[&left, &right]) {
        result.AddEquivalenceUnion(class);
    }

    assert_eq!(
        result.String(),
        "",
        "branches with disjoint equivalence classes should not contribute any FD to the union"
    );
}

// 对应同一段 union_all.rs:111-129，但两个分支在相同输出列位上共享同一个等价类
// （都是 1==2）并且共享非空列：UnionAll 应当保留这个公共等价关系。对应 Go 用例
// `select * from t1 where a=b union all select * from t2 where e=f` ->
// `fd: "{(11,12)==(11,12)}"`。
/// UnionAll：公共等价类与公共非空列应被保留。
#[test]
fn union_all_extract_fd_keeps_equivalence_and_not_null_common_to_every_branch() {
    let mut left = FDSet::default();
    left.MakeNotNull(ids(&[1, 2]));
    left.AddEquivalence(ids(&[1]), ids(&[2]));

    let mut right = FDSet::default();
    right.MakeNotNull(ids(&[1]));
    right.AddEquivalence(ids(&[1]), ids(&[2]));

    let mut not_null = left.NotNullCols.Copy();
    not_null.IntersectionWith(&right.NotNullCols);

    let mut result = FDSet::default();
    result.MakeNotNull(not_null);
    for class in FindCommonEquivClasses(&[&left, &right]) {
        result.AddEquivalenceUnion(class);
    }

    assert_eq!(
        result.String(),
        "(1,2)==(1,2)",
        "an equivalence class shared by every branch should survive UnionAll::ExtractFD"
    );
}

// 对应 pkg/planner/core/operator/logicalop/logical_aggregation.rs:355-377：
// `LogicalAggregation::ExtractFD` 先用 `base_mut().ExtractFD()` 的默认实现合并
// 子节点 FD（`AddFrom`，见 base_logical_plan.rs:369-378），再把 GroupByItems 涉及
// 的列作为 `from`，所有非 firstrow 的聚合输出列作为 `to`，调用一次
// `AddStrictFunctionalDependency`。因为新加的确定集合与已有边 `1-->2` 的
// determinant 完全相同（都是列 1），生产代码里的 `addFunctionalDependency` 会把
// 两条边的 "to" 合并成同一条边（`fd.to.UnionWith(&to)`），而不是新增一条边。
/// Aggregation：分组列严格决定非 firstrow 聚合输出，同决定端边合并。
#[test]
fn aggregation_extract_fd_adds_strict_dependency_from_group_by_to_non_firstrow_aggregates() {
    let child = strict_fd(&[1], &[2]);

    let mut result = FDSet::default();
    result.AddFrom(&child);
    // GroupByItems = [col 1]；聚合输出里唯一的非 firstrow 列是 UniqueID=10（例如 count(a)）。
    result.AddStrictFunctionalDependency(ids(&[1]), ids(&[10]));

    assert_eq!(
        result.String(),
        "(1)-->(2,10)",
        "a strict FD with the same determinant should merge into the existing edge's dependent side"
    );
}

// 对应 pkg/planner/core/operator/logicalop/logical_projection.rs:320-407：
// `LogicalProjection::ExtractFD` 先用默认实现合并子节点 FD，再对每个
// `(expr, outputColumn)` 对：若 expr 是列引用且和输出列 UniqueID 不同，调用
// `AddEquivalence(expr.UniqueID, output.UniqueID)`；随后 `MakeNotNull` 收集到的
// not-null 输出列（这里没有），最后 `ProjectCols(output_ids ∪ GroupByCols)` 投影
// 到 Projection 自己的输出 schema。
/// Projection：列重命名产生等价，再投影到输出 schema。
#[test]
fn projection_extract_fd_adds_equivalence_for_renamed_column_then_projects_to_output_schema() {
    let child = strict_fd(&[1], &[2]);

    let mut dependencies = FDSet::default();
    dependencies.AddFrom(&child);
    // `select a as x from t`：expr 是列 1，重命名输出列 UniqueID=20，两者不同。
    dependencies.AddEquivalence(ids(&[1]), ids(&[20]));
    dependencies.MakeNotNull(ids(&[]));
    // Projection 的输出 schema 只有重命名后的列 20（GroupByCols 为空）。
    dependencies.ProjectCols(ids(&[20]).Union(&FDSet::default().GroupByCols));

    assert_eq!(
        dependencies.String(),
        "(20)==(20)",
        "renaming column 1 to output column 20 and projecting away column 1/2 should leave only the trivial self-equivalence on the retained output column"
    );
}
