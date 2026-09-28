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

// FDSet 核心 API 与 Go 行为对齐的迁移单元测试。
//
// 覆盖严格闭包与决定端约简、Lax 一步闭包与等价扩展、常量/等价/非空提升、
// 投影后传递依赖保留，以及多 FDSet 公共等价类求交。

use astersql_planner_funcdep::funcdep::{FDSet, FindCommonEquivClasses};
use astersql_planner_funcdep::intset::{FastIntSet, NewFastIntSet};

/// 由列 unique id 列表构造 `FastIntSet`。
fn set(values: &[i32]) -> FastIntSet {
    NewFastIntSet(values.to_vec())
}

/// 严格 FD 闭包应传递闭合，且 `ReduceCols` 能去掉可由闭包推出的冗余决定列。
#[test]
fn strict_closure_and_determinant_reduction_match_go() {
    let mut fd = FDSet::default();
    // 1→{3,4}、3→{4,5}、{3,5}→2：从 1 应推出全部列。
    fd.AddStrictFunctionalDependency(set(&[1]), set(&[3, 4]));
    fd.AddStrictFunctionalDependency(set(&[3]), set(&[4, 5]));
    fd.AddStrictFunctionalDependency(set(&[3, 5]), set(&[2]));

    assert_eq!(
        fd.ClosureOfStrict(set(&[1])).SortedArray(),
        vec![1, 2, 3, 4, 5]
    );
    assert_eq!(fd.ReduceCols(set(&[1, 2])).SortedArray(), vec![1]);
}

/// Lax 闭包默认只走一步，但中间插入等价类后可继续扩展依赖端。
#[test]
fn lax_dependencies_are_one_step_but_equivalence_extends_the_step() {
    let mut fd = FDSet::default();
    fd.AddLaxFunctionalDependency(set(&[2]), set(&[3]));
    fd.AddLaxFunctionalDependency(set(&[1]), set(&[2]));
    // 仅一步：1 ~~> 2，尚不能到 3。
    assert_eq!(fd.ClosureOfLax(set(&[1])).SortedArray(), vec![1, 2]);

    // 2≡4 后，4 ~~> 5 可通过等价把 1 的 Lax 闭包扩到 3/4/5。
    fd.AddEquivalence(set(&[2]), set(&[4]));
    fd.AddLaxFunctionalDependency(set(&[4]), set(&[5]));
    assert_eq!(
        fd.ClosureOfLax(set(&[1])).SortedArray(),
        vec![1, 2, 3, 4, 5]
    );
}

/// 常量经等价传播，且决定端非空可将 Lax FD 提升为 Strict。
#[test]
fn constants_equivalence_and_not_null_promotion_match_go() {
    let mut fd = FDSet::default();
    fd.AddConstants(set(&[4, 5]));
    fd.AddEquivalence(set(&[3]), set(&[4]));
    // 等价把 3 纳入常量闭包。
    assert_eq!(fd.ConstantCols().String(), "(3-5)");

    fd.AddLaxFunctionalDependency(set(&[7]), set(&[8]));
    fd.MakeNotNull(set(&[7]));
    assert_eq!(fd.String(), "(3,4)==(3,4), ()-->(3-5), (7)-->(8)");
}

/// 投影去掉中间列后，传递严格依赖仍应可通过闭包查询得到。
#[test]
fn projection_preserves_transitive_dependencies() {
    let mut fd = FDSet::default();
    fd.AddStrictFunctionalDependency(set(&[1]), set(&[2]));
    fd.AddStrictFunctionalDependency(set(&[2]), set(&[3]));
    // 只保留 1、3：中间列 2 被裁掉，但 1→3 传递语义仍在。
    fd.ProjectCols(set(&[1, 3]));
    assert!(fd.InClosure(set(&[1]), set(&[3])));
    assert_eq!(fd.AllCols().SortedArray(), vec![1, 3]);
}

/// `FindCommonEquivClasses` 对多个 FDSet 求等价类交集（长度须 > 1）。
#[test]
fn common_equivalence_classes_match_go_intersections() {
    let mut fd1 = FDSet::default();
    fd1.AddEquivalenceUnion(set(&[1, 2, 3]));
    let mut fd2 = FDSet::default();
    fd2.AddEquivalenceUnion(set(&[2, 3, 4]));
    let common = FindCommonEquivClasses(&[&fd1, &fd2]);
    assert_eq!(common.len(), 1);
    assert_eq!(common[0].String(), "(2,3)");
}
