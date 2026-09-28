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

// `fd_graph` 嵌套单元测试：严格/Lax FD、闭包、常量、等价类与公共等价求交。
//
// 通过 `super::*` 访问私有字段，与 Go `fd_graph_test.go` 用例对齐。

// 本文件由 pkg/planner/funcdep/fd_graph_test.go 迁移而来，作为 fd_graph.rs 的嵌套测试模块
// （通过 `#[cfg(test)] #[path = "fd_graph_test.rs"] mod fd_graph_test;` 接入），
// 因此可以借助 `super::*` 直接访问 `FDSet`/`fdEdge` 的私有字段，测试逻辑与 Go 版本保持一致。

use super::*;

/// 由列 unique id 列表构造 `FastIntSet`。
fn set(values: &[i32]) -> FastIntSet {
    NewFastIntSet(values.to_vec())
}

/// 多条同决定端严格 FD 应合并为一条覆盖全部依赖列的边。
#[test]
fn TestAddStrictFunctionalDependency() {
    let mut fd = FDSet::default();
    let fe1 = fdEdge::new(set(&[1, 2]), set(&[3, 4, 5, 6, 7]), true, false); // AB -> CDEFG
    let fe2 = fdEdge::new(set(&[1, 2]), set(&[3, 4]), true, false); // AB -> CD
    let fe3 = fdEdge::new(set(&[1, 2]), set(&[5, 6]), true, false); // AB -> EF

    // fd: AB -> CDEFG implies all of others.
    let assert_f = |fd: &FDSet| {
        assert_eq!(fd.fdEdges.len(), 1);
        let from = fd.fdEdges[0].from.SortedArray();
        assert_eq!(from.len(), 2);
        assert_eq!(from[0], 1);
        assert_eq!(from[1], 2);
        let to = fd.fdEdges[0].to.SortedArray();
        assert_eq!(to.len(), 5);
        assert_eq!(to[0], 3);
        assert_eq!(to[1], 4);
        assert_eq!(to[2], 5);
        assert_eq!(to[3], 6);
        assert_eq!(to[4], 7);
    };

    fd.AddStrictFunctionalDependency(fe1.from.Copy(), fe1.to.Copy());
    fd.AddStrictFunctionalDependency(fe2.from.Copy(), fe2.to.Copy());
    fd.AddStrictFunctionalDependency(fe3.from.Copy(), fe3.to.Copy());
    assert_f(&fd);

    // 打乱插入顺序，合并结果应相同。
    fd.fdEdges.clear();
    fd.AddStrictFunctionalDependency(fe2.from.Copy(), fe2.to.Copy());
    fd.AddStrictFunctionalDependency(fe1.from.Copy(), fe1.to.Copy());
    fd.AddStrictFunctionalDependency(fe3.from.Copy(), fe3.to.Copy());
    assert_f(&fd);
    // TODO:
    // test reduce col
    // test more edges
}

// Preface Notice:
// For test convenience, we add fdEdge to fdSet directly which is not valid in the procedure.
// Because two difference fdEdge in the fdSet may imply each other which is strictly not permitted in the procedure.
// Use `AddStrictFunctionalDependency` to add the fdEdge to the fdSet in the formal way.
/// 严格闭包：从决定列集合反复应用严格边与等价边直到不动点。
#[test]
fn TestFDSet_ClosureOf() {
    let mut fd = FDSet::default();
    let fe1 = fdEdge::new(set(&[1, 2]), set(&[3, 4]), true, false); // AB -> CD
    let fe2 = fdEdge::new(set(&[1, 2]), set(&[5, 6]), true, false); // AB -> EF
    let fe3 = fdEdge::new(set(&[2]), set(&[6, 7]), true, false); // B -> FG
    let fe4 = fdEdge::new(set(&[1]), set(&[4, 5, 8]), true, false); // A -> DEH
    fd.fdEdges
        .extend([fe1.clone(), fe2.clone(), fe3.clone(), fe4.clone()]);

    // A -> ADEH
    let closure = fd.closureOfStrict(&set(&[1])).SortedArray();
    assert_eq!(closure.len(), 4);
    assert_eq!(closure[0], 1);
    assert_eq!(closure[1], 4);
    assert_eq!(closure[2], 5);
    assert_eq!(closure[3], 8);

    // AB -> ABCDEFGH
    fd.fdEdges.extend([fe1, fe2, fe3, fe4]);
    let closure = fd.closureOfStrict(&set(&[1, 2])).SortedArray();
    assert_eq!(closure.len(), 8);
    assert_eq!(closure[0], 1);
    assert_eq!(closure[1], 2);
    assert_eq!(closure[2], 3);
    assert_eq!(closure[3], 4);
    assert_eq!(closure[4], 5);
    assert_eq!(closure[5], 6);
    assert_eq!(closure[6], 7);
    assert_eq!(closure[7], 8);
}

/// ReduceCols：去掉可由剩余列严格闭包推出的冗余决定列。
#[test]
fn TestFDSet_ReduceCols() {
    let mut fd = FDSet::default();
    let fe1 = fdEdge::new(set(&[1]), set(&[3, 4]), true, false); // A -> CD
    let fe2 = fdEdge::new(set(&[3]), set(&[4, 5]), true, false); // C -> DE
    let fe3 = fdEdge::new(set(&[3, 5]), set(&[2]), true, false); // CE -> B
    fd.fdEdges.extend([fe1, fe2, fe3]);
    let res = fd.ReduceCols(set(&[1, 2])).SortedArray();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0], 1);
}

/// InClosure：依赖端可拆分，决定端不可拆分。
#[test]
fn TestFDSet_InClosure() {
    let mut fd = FDSet::default();
    let fe1 = fdEdge::new(set(&[1, 2]), set(&[3, 4]), true, false); // AB -> CD
    let fe2 = fdEdge::new(set(&[1, 2]), set(&[5, 6]), true, false); // AB -> EF
    let fe3 = fdEdge::new(set(&[2]), set(&[6, 7]), true, false); // B -> FG
    fd.fdEdges.extend([fe1, fe2, fe3]);

    // A -> F : false (determinants should not be torn apart)
    assert!(!fd.InClosure(set(&[1]), set(&[6])));
    // B -> G : true (dependency can be torn apart)
    assert!(fd.InClosure(set(&[2]), set(&[7])));
    // AB -> E : true (dependency can be torn apart)
    assert!(fd.InClosure(set(&[1, 2]), set(&[5])));
    // AB -> FG: true (in closure node set)
    assert!(fd.InClosure(set(&[1, 2]), set(&[6, 7])));
    // AB -> DF: true (in closure node set)
    assert!(fd.InClosure(set(&[1, 2]), set(&[4, 6])));
    // AB -> EG: true (in closure node set)
    assert!(fd.InClosure(set(&[1, 2]), set(&[5, 7])));
    // AB -> EGH: false (H is not in closure node set)
    assert!(!fd.InClosure(set(&[1, 2]), set(&[5, 7, 8])));

    let fe4 = fdEdge::new(set(&[2]), set(&[3, 8]), true, false); // B -> CH
    fd.fdEdges.push(fe4);
    // AB -> EGH: true (in closure node set)
    assert!(fd.InClosure(set(&[1, 2]), set(&[5, 7, 8])));
}

/// 常量边 `{} → cols`：扩展常量闭包并约简/消除相关 FD。
#[test]
fn TestFDSet_AddConstant() {
    let mut fd = FDSet::default();
    assert_eq!(fd.ConstantCols().String(), "()");

    fd.AddConstants(set(&[1, 2])); // {} --> {a,b}
    assert_eq!(fd.fdEdges.len(), 1);
    assert!(fd.fdEdges[0].strict);
    assert!(!fd.fdEdges[0].equiv);
    assert_eq!(fd.fdEdges[0].from.String(), "()");
    assert_eq!(fd.fdEdges[0].to.String(), "(1,2)");
    assert_eq!(fd.ConstantCols().String(), "(1,2)");

    fd.AddConstants(set(&[3])); // c, {} --> {a,b,c}
    assert_eq!(fd.fdEdges.len(), 1);
    assert!(fd.fdEdges[0].strict);
    assert!(!fd.fdEdges[0].equiv);
    assert_eq!(fd.fdEdges[0].from.String(), "()");
    assert_eq!(fd.fdEdges[0].to.String(), "(1-3)");
    assert_eq!(fd.ConstantCols().String(), "(1-3)");

    fd.AddStrictFunctionalDependency(set(&[3, 4]), set(&[5, 6])); // {c,d} --> {e,f}
    assert_eq!(fd.fdEdges.len(), 2);
    assert!(fd.fdEdges[0].strict);
    assert!(!fd.fdEdges[0].equiv);
    assert_eq!(fd.fdEdges[0].from.String(), "()");
    assert_eq!(fd.fdEdges[0].to.String(), "(1-3)");
    assert_eq!(fd.ConstantCols().String(), "(1-3)");
    assert!(fd.fdEdges[1].strict);
    assert!(!fd.fdEdges[1].equiv);
    // determinant 3 reduced as constant, leaving FD {d} --> {f,g}.
    assert_eq!(fd.fdEdges[1].from.String(), "(4)");
    assert_eq!(fd.fdEdges[1].to.String(), "(5,6)");

    fd.AddLaxFunctionalDependency(set(&[7]), set(&[5, 6])); // {g} ~~> {e,f}
    assert_eq!(fd.fdEdges.len(), 3);
    assert!(!fd.fdEdges[2].strict);
    assert!(!fd.fdEdges[2].equiv);
    assert_eq!(fd.fdEdges[2].from.String(), "(7)");
    assert_eq!(fd.fdEdges[2].to.String(), "(5,6)");

    // add d, {} --> {a,b,c,d}, and FD {d} --> {f,g} is transferred to constant closure.
    fd.AddConstants(set(&[4]));
    // => {} --> {a,b,c,d,e,f}, for lax FD {g} ~~> {e,f}, dependencies are constants, removed.
    assert_eq!(fd.fdEdges.len(), 1);
    assert!(fd.fdEdges[0].strict);
    assert!(!fd.fdEdges[0].equiv);
    assert_eq!(fd.fdEdges[0].from.String(), "()");
    assert_eq!(fd.fdEdges[0].to.String(), "(1-6)");
    assert_eq!(fd.ConstantCols().String(), "(1-6)");
}

/// Lax FD 蕴含：依赖端不同则不互相蕴含；同依赖端时更小决定端蕴含更大决定端。
#[test]
fn TestFDSet_LaxImplies() {
    let mut fd = FDSet::default();
    fd.AddLaxFunctionalDependency(set(&[1]), set(&[2, 3]));
    fd.AddLaxFunctionalDependency(set(&[1]), set(&[2]));
    // lax FD won't imply each other once they have the different to side.
    assert_eq!(fd.String(), "(1)~~>(2,3), (1)~~>(2)");

    let mut fd = FDSet::default();
    fd.AddLaxFunctionalDependency(set(&[1]), set(&[2]));
    fd.AddLaxFunctionalDependency(set(&[1]), set(&[2, 3]));
    assert_eq!(fd.String(), "(1)~~>(2), (1)~~>(2,3)");

    let mut fd = FDSet::default();
    fd.AddLaxFunctionalDependency(set(&[1]), set(&[3]));
    fd.AddLaxFunctionalDependency(set(&[1, 2]), set(&[3]));
    // lax FD can imply each other once they have the same to side. {1,2} ~~> {3} implies {1} ~~> {3}
    assert_eq!(fd.String(), "(1)~~>(3)");

    let mut fd = FDSet::default();
    fd.AddLaxFunctionalDependency(set(&[1]), set(&[3, 4]));
    fd.AddLaxFunctionalDependency(set(&[1, 2]), set(&[3]));
    // lax FD won't imply each other once they have the different to side. {1,2} ~~> {3} implies {1} ~~> {3}
    assert_eq!(fd.String(), "(1)~~>(3,4), (1,2)~~>(3)");
}

/// 等价类合并、与常量闭包互相扩展，并消除被常量约掉的严格边。
#[test]
fn TestFDSet_AddEquivalence() {
    let mut fd = FDSet::default();
    assert_eq!(fd.EquivalenceCols().len(), 0);

    fd.AddEquivalence(set(&[1]), set(&[2])); // {a} == {b}
    assert_eq!(fd.fdEdges.len(), 1); // res: {a} == {b}
    assert_eq!(fd.EquivalenceCols().len(), 1);
    assert!(fd.fdEdges[0].strict);
    assert!(fd.fdEdges[0].equiv);
    assert_eq!(fd.fdEdges[0].from.String(), "(1,2)");
    assert_eq!(fd.fdEdges[0].to.String(), "(1,2)");
    assert_eq!(fd.EquivalenceCols()[0].String(), "(1,2)");

    fd.AddEquivalence(set(&[3]), set(&[4])); // {c} == {d}
    assert_eq!(fd.fdEdges.len(), 2); // res: {a,b} == {a,b}, {c,d} == {c,d}
    assert_eq!(fd.EquivalenceCols().len(), 2);
    assert!(fd.fdEdges[0].strict);
    assert!(fd.fdEdges[0].equiv);
    assert_eq!(fd.fdEdges[0].from.String(), "(1,2)");
    assert_eq!(fd.fdEdges[0].to.String(), "(1,2)");
    assert_eq!(fd.EquivalenceCols()[0].String(), "(1,2)");
    assert!(fd.fdEdges[1].strict);
    assert!(fd.fdEdges[1].equiv);
    assert_eq!(fd.fdEdges[1].from.String(), "(3,4)");
    assert_eq!(fd.fdEdges[1].to.String(), "(3,4)");
    assert_eq!(fd.EquivalenceCols()[1].String(), "(3,4)");

    fd.AddConstants(set(&[4, 5])); // {} --> {d,e}
    assert_eq!(fd.fdEdges.len(), 3); // res: {a,b} == {a,b}, {c,d} == {c,d},{} --> {c,d,e}
    assert!(fd.fdEdges[2].strict); // explain: constant closure is extended by equivalence {c,d} == {c,d}
    assert!(!fd.fdEdges[2].equiv);
    assert_eq!(fd.fdEdges[2].from.String(), "()");
    assert_eq!(fd.fdEdges[2].to.String(), "(3-5)");
    assert_eq!(fd.ConstantCols().String(), "(3-5)");

    fd.AddStrictFunctionalDependency(set(&[2, 3]), set(&[5, 6])); // {b,c} --> {e,f}
    // res: {a,b} == {a,b}, {c,d} == {c,d},{} --> {c,d,e}, {b} --> {e,f}
    assert_eq!(fd.fdEdges.len(), 4);
    // explain: strict FD's from side c is eliminated by constant closure.
    assert!(fd.fdEdges[3].strict);
    assert!(!fd.fdEdges[3].equiv);
    assert_eq!(fd.fdEdges[3].from.String(), "(2)");
    assert_eq!(fd.fdEdges[3].to.String(), "(5,6)");

    fd.AddEquivalence(set(&[2]), set(&[3])); // {b} == {d}
    // res: {a,b,c,d} == {a,b,c,d}, {} --> {a,b,c,d,e,f}
    // explain:
    // b = d build the connection between {a,b} == {a,b}, {c,d} == {c,d}, make the superset of equivalence closure.
    // the superset equivalence closure extend the existed constant closure in turn, resulting {} --> {a,b,c,d,e}
    // the superset constant closure eliminate existed strict FD, since determinants is constant,
    // so the dependencies must be constant as well.
    // so extending the current constant closure as to {} --> {a,b,c,d,e,f}
    assert_eq!(fd.fdEdges.len(), 2);
    assert_eq!(fd.EquivalenceCols().len(), 1);
    assert_eq!(fd.EquivalenceCols()[0].String(), "(1-4)");
    assert_eq!(fd.ConstantCols().String(), "(1-6)");
}

/// FindCommonEquivClasses：多 FDSet 间交集长度 > 1 的公共等价类。
#[test]
fn TestFindCommonEquivClasses() {
    let mut fd1 = FDSet::default();
    // fd1 is with equivalence classes for {1,2} and {3,4}
    fd1.addEquivalence(set(&[1, 2]));
    fd1.addEquivalence(set(&[3, 4]));

    let mut fd2 = FDSet::default();
    // fd2 is with equivalence classes for {1,3} and {2,4}
    fd2.addEquivalence(set(&[1, 3]));
    fd2.addEquivalence(set(&[2, 4]));

    let mut fd3 = FDSet::default();
    // fd3 is with equivalence classes for {1} and {3,4}
    fd3.addEquivalence(set(&[1]));
    fd3.addEquivalence(set(&[3, 4]));

    // find common equivalence classes between fd1 and fd2.
    let res = FindCommonEquivClasses(&[&fd1, &fd2]);
    assert_eq!(res.len(), 0);

    // find common equivalence classes between fd2 and fd3.
    let res = FindCommonEquivClasses(&[&fd2, &fd3]);
    assert_eq!(res.len(), 0);

    // find common equivalence classes between fd1 and fd3.
    let res = FindCommonEquivClasses(&[&fd1, &fd3]);
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].String(), "(3,4)");

    // find common equivalence classes between fd1, fd2 and fd3.
    let res = FindCommonEquivClasses(&[&fd1, &fd2, &fd3]);
    assert_eq!(res.len(), 0);
}

/// Go `ProjectCols` leaves a hidden Cond-FD untouched when its null constraint
/// has no projected column, because that branch continues without deleting it.
#[test]
fn TestProjectColsKeepsCondFDWithUnprojectedCondition() {
    let mut fd = FDSet::default();
    fd.AddNCFunctionalDependency(set(&[1]), set(&[2]), set(&[9]), true, false);

    fd.ProjectCols(set(&[1, 2]));

    assert_eq!(fd.ncEdges.len(), 1);
    assert_eq!(fd.ncEdges[0].conditionNC.as_ref().unwrap().String(), "(9)");
}
