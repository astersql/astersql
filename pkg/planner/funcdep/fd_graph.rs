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

// 函数依赖图（FD Graph）实现。
//
// 维护列 unique id 之间的严格/松散函数依赖、等价类、常量闭包，以及带 null-constraint
// 的 Cond-FD（`ncEdges`）。供逻辑优化器做列裁剪、唯一键推导与外连接后依赖传播。

#![allow(non_snake_case, non_camel_case_types)]

use crate::intset::{FastIntSet, NewFastIntSet};
use std::collections::HashMap;

/// 空列集合。
fn empty_set() -> FastIntSet {
    NewFastIntSet(Vec::new())
}
/// 单列集合。
fn singleton(value: i32) -> FastIntSet {
    NewFastIntSet(vec![value])
}

/// 一条函数依赖边：`from` 决定 `to`。
///
/// - `strict`：严格 FD（`-->`）；否则为 Lax（`~~>`）。
/// - `equiv`：等价类边（`==`），此时 from/to 应为同一集合。
/// - `conditionNC`：Cond-FD 的 null-constraint 列集；非空时暂存于 `ncEdges`。
#[derive(Clone, Debug)]
pub struct fdEdge {
    pub from: FastIntSet,
    pub to: FastIntSet,
    pub strict: bool,
    pub equiv: bool,
    pub conditionNC: Option<FastIntSet>,
}

impl fdEdge {
    /// 构造无 Cond-FD 条件的普通边。
    fn new(from: FastIntSet, to: FastIntSet, strict: bool, equiv: bool) -> Self {
        Self {
            from,
            to,
            strict,
            equiv,
            conditionNC: None,
        }
    }

    /// 判断本边是否蕴含 `other`（Lax 与严格规则不同）。
    fn implies(&self, other: &fdEdge) -> bool {
        let lhs_lax = !self.equiv && !self.strict;
        let rhs_lax = !other.equiv && !other.strict;
        // Lax：更小决定端 + 相同依赖端 ⇒ 蕴含；严格：更大依赖端 + 不更弱类型。
        if lhs_lax && rhs_lax {
            return self.from.SubsetOf(&other.from) && self.to.Equals(&other.to);
        }
        self.from.SubsetOf(&other.from)
            && other.to.SubsetOf(&self.to)
            && (self.strict || !other.strict)
            && (self.equiv || !other.equiv)
    }

    /// 从决定端去掉 `cols`；若变成常量边则返回 true。
    fn removeColumnsFromSide(&mut self, cols: &FastIntSet) -> bool {
        if self.from.Intersects(cols) {
            self.from.DifferenceWith(cols);
        }
        self.isConstant()
    }

    /// 从依赖端去掉 `cols`；若依赖端变空则返回 true（边可删除）。
    fn removeColumnsToSide(&mut self, cols: &FastIntSet) -> bool {
        if self.to.Intersects(cols) {
            self.to.DifferenceWith(cols);
        }
        self.to.IsEmpty()
    }

    /// 决定端为空 ⇒ 常量 FD `{} → to`。
    fn isConstant(&self) -> bool {
        self.from.IsEmpty()
    }
    /// 等价类边：equiv 且 from==to。
    fn isEquivalence(&self) -> bool {
        self.equiv && self.from.Equals(&self.to)
    }

    /// 打印为 `(cols)==(cols)` / `(a)-->(b)` / `(a)~~>(b)`。
    pub fn String(&self) -> String {
        if self.equiv {
            if !self.strict {
                return "Wrong functional dependency".to_owned();
            }
            format!("{}=={}", self.from, self.to)
        } else if self.strict {
            format!("{}-->{}", self.from, self.to)
        } else {
            format!("{}~~>{}", self.from, self.to)
        }
    }
}

/// 一组 FD 边的集合，附带非空列、表达式 hash→unique id、聚合分组列等元数据。
#[derive(Clone, Debug, Default)]
pub struct FDSet {
    fdEdges: Vec<fdEdge>,
    /// Cond-FD：连接条件依赖，受 null-constraint 列控制可见性。
    ncEdges: Vec<fdEdge>,
    pub NotNullCols: FastIntSet,
    pub HashCodeToUniqueID: HashMap<String, i32>,
    pub GroupByCols: FastIntSet,
    pub HasAggBuilt: bool,
}

impl FDSet {
    /// 严格闭包的公开入口。
    pub fn ClosureOfStrict(&self, cols: FastIntSet) -> FastIntSet {
        self.closureOfStrict(&cols)
    }

    /// 从 `cols` 出发反复应用严格边与等价边，直到不动点。
    fn closureOfStrict(&self, cols: &FastIntSet) -> FastIntSet {
        let mut result = cols.Copy();
        loop {
            let before = result.Len();
            for fd in &self.fdEdges {
                // 严格：决定端 ⊆ result；等价：与 result 相交即可扩展。
                if (fd.strict && fd.from.SubsetOf(&result))
                    || (fd.equiv && fd.from.Intersects(&result))
                {
                    result.UnionWith(&fd.to);
                }
            }
            if result.Len() == before {
                return result;
            }
        }
    }

    /// Lax 闭包：等价可反复扩展；Lax 边只走一步，并并上严格闭包。
    pub fn ClosureOfLax(&self, cols: FastIntSet) -> FastIntSet {
        let mut reached = cols.Copy();
        let mut i = 0;
        while i < self.fdEdges.len() {
            let fd = &self.fdEdges[i];
            // 等价命中则重置扫描，以继续扩展。
            if fd.equiv && fd.from.Intersects(&reached) && !fd.to.SubsetOf(&reached) {
                reached.UnionWith(&fd.to);
                i = 0;
                continue;
            }
            if !fd.strict && !fd.equiv && fd.from.SubsetOf(&reached) && !fd.to.SubsetOf(&reached) {
                reached.UnionWith(&fd.to);
            }
            i += 1;
        }
        reached.UnionWith(&self.closureOfStrict(&cols));
        reached
    }

    /// 仅沿等价类扩展列集合。
    pub fn ClosureOfEquivalence(&self, cols: FastIntSet) -> FastIntSet {
        let mut result = cols;
        for fd in &self.fdEdges {
            if fd.equiv && fd.from.Intersects(&result) {
                result.UnionWith(&fd.to);
            }
        }
        result
    }

    /// 判断 `to` 是否落在 `from` 的严格闭包内（含 `to ⊆ from`）。
    pub fn InClosure(&self, from: FastIntSet, to: FastIntSet) -> bool {
        to.SubsetOf(&from) || to.SubsetOf(&self.closureOfStrict(&from))
    }

    /// 约简决定列：去掉能由剩余列严格推出的冗余列。
    pub fn ReduceCols(&self, cols: FastIntSet) -> FastIntSet {
        let mut removed = empty_set();
        let mut result = cols.Copy();
        for k in cols.SortedArray() {
            removed.Insert(k);
            result.Remove(k);
            // 若去掉 k 后仍能推出已去掉的列，则 k 冗余。
            if !self.InClosure(result.Copy(), removed.Copy()) {
                removed.Remove(k);
                result.Insert(k);
            }
        }
        result
    }

    /// 添加严格函数依赖 `from → to`。
    pub fn AddStrictFunctionalDependency(&mut self, from: FastIntSet, to: FastIntSet) {
        self.addFunctionalDependency(from, to, true, false)
    }
    /// 添加松散函数依赖 `from ~~> to`。
    pub fn AddLaxFunctionalDependency(&mut self, from: FastIntSet, to: FastIntSet) {
        self.addFunctionalDependency(from, to, false, false)
    }
    /// 添加 Cond-FD（暂存于 ncEdges，待 null-reject 后再提升）。
    pub fn AddNCFunctionalDependency(
        &mut self,
        from: FastIntSet,
        to: FastIntSet,
        nc: FastIntSet,
        strict: bool,
        equiv: bool,
    ) {
        let mut edge = fdEdge::new(from, to, strict, equiv);
        edge.conditionNC = Some(nc);
        self.ncEdges.push(edge);
    }

    /// 插入普通 FD：约简决定端、去平凡依赖，并按蕴含/同决定端合并边。
    fn addFunctionalDependency(
        &mut self,
        mut from: FastIntSet,
        mut to: FastIntSet,
        strict: bool,
        equiv: bool,
    ) {
        if to.SubsetOf(&from) {
            return;
        }
        if to.Intersects(&from) {
            to.DifferenceWith(&from);
        }
        from = self.ReduceCols(from);
        let new_fd = fdEdge::new(from.Copy(), to.Copy(), strict, equiv);
        let mut result = Vec::with_capacity(self.fdEdges.len() + 1);
        let mut added = false;
        for mut fd in self.fdEdges.drain(..) {
            if new_fd.implies(&fd) {
                // 新边更强：替换旧边。
                if !added {
                    fd = new_fd.clone();
                    added = true;
                } else {
                    continue;
                }
            } else if !added {
                if fd.implies(&new_fd) {
                    // 旧边已蕴含新边，无需再插。
                    added = true;
                } else if fd.strict && !fd.equiv && fd.from.Equals(&from) {
                    // 同决定端严格边：合并依赖端。
                    fd.to.UnionWith(&to);
                    added = true;
                }
            }
            result.push(fd);
        }
        if !added {
            result.push(new_fd);
        }
        self.fdEdges = result;
    }

    /// 将 `eqs` 并入等价闭包，清理被蕴含的边，并可能扩展常量/非空。
    fn addEquivalence(&mut self, eqs: FastIntSet) {
        let closure = self.ClosureOfEquivalence(eqs);
        self.fdEdges
            .push(fdEdge::new(closure.Copy(), closure.Copy(), true, true));
        let mut add_const = false;
        let last = self.fdEdges.len() - 1;
        let mut kept = Vec::with_capacity(self.fdEdges.len());
        for (i, mut fd) in self.fdEdges.drain(..).enumerate() {
            if i == last {
                kept.push(fd);
                continue;
            }
            let mut remove = false;
            if fd.isConstant() {
                // 常量与新等价类相交 ⇒ 需把等价类也并入常量。
                add_const |= fd.to.Intersects(&closure) && !closure.SubsetOf(&fd.to);
            } else if fd.from.SubsetOf(&closure) {
                if fd.equiv {
                    remove = true;
                } else if fd.removeColumnsToSide(&closure) {
                    remove = true;
                }
            }
            if !remove {
                kept.push(fd);
            }
        }
        self.fdEdges = kept;
        if add_const {
            self.AddConstants(closure.Copy());
        }
        if self.NotNullCols.Intersects(&closure) {
            self.MakeNotNull(closure);
        }
    }

    /// 声明 `from` 与 `to` 等价（取并集加入等价类）。
    pub fn AddEquivalence(&mut self, from: FastIntSet, to: FastIntSet) {
        if !to.SubsetOf(&from) {
            self.addEquivalence(from.Union(&to));
        }
    }
    /// 直接以并集形式添加等价类。
    pub fn AddEquivalenceUnion(&mut self, union: FastIntSet) {
        self.addEquivalence(union);
    }

    /// 声明常量子集 `{} → cons`，并约简/删除被常量吸收的边。
    pub fn AddConstants(&mut self, cons: FastIntSet) {
        if cons.IsEmpty() {
            return;
        }
        let cols = self.closureOfStrict(&cons);
        self.fdEdges
            .push(fdEdge::new(empty_set(), cols.Copy(), true, false));
        let last = self.fdEdges.len() - 1;
        let mut kept = Vec::with_capacity(self.fdEdges.len());
        for (i, mut fd) in self.fdEdges.drain(..).enumerate() {
            if i == last {
                kept.push(fd);
                continue;
            }
            let mut remove = false;
            if !fd.equiv {
                // 决定端变成空 ⇒ 整条边并入常量；依赖端被常量掏空 ⇒ 删除。
                if fd.strict && fd.removeColumnsFromSide(&cols) {
                    remove = true;
                }
                if fd.removeColumnsToSide(&cols) {
                    remove = true;
                }
            }
            if !remove {
                kept.push(fd);
            }
        }
        self.fdEdges = kept;
    }

    /// 返回当前常量列集合（若无常量边则为空）。
    pub fn ConstantCols(&self) -> FastIntSet {
        self.fdEdges
            .iter()
            .find(|fd| fd.isConstant())
            .map_or_else(empty_set, |fd| fd.to.Copy())
    }
    /// 返回所有等价类列集合。
    pub fn EquivalenceCols(&self) -> Vec<FastIntSet> {
        self.fdEdges
            .iter()
            .filter(|fd| fd.isEquivalence())
            .map(|fd| fd.from.Copy())
            .collect()
    }

    /// 标记非空列：唤醒命中的 Cond-FD，并将决定端非空的 Lax 提升为严格。
    pub fn MakeNotNull(&mut self, not_null: FastIntSet) {
        let mut combined = not_null;
        combined.UnionWith(&self.NotNullCols);
        let mut not_null_set = self.ClosureOfEquivalence(combined);
        loop {
            let mut changed = false;
            let mut pending = Vec::new();
            let mut hidden = Vec::new();
            // conditionNC 与非空集相交的 Cond-FD 可提升为普通 FD。
            for fd in self.ncEdges.drain(..) {
                if fd
                    .conditionNC
                    .as_ref()
                    .is_some_and(|nc| nc.Intersects(&not_null_set))
                {
                    pending.push(fd);
                } else {
                    hidden.push(fd);
                }
            }
            self.ncEdges = hidden;
            for fd in pending {
                if fd.isConstant() {
                    self.AddConstants(fd.to);
                } else if fd.equiv {
                    self.AddEquivalence(fd.from, fd.to);
                    let expanded = self.ClosureOfEquivalence(not_null_set.Copy());
                    if !expanded.Difference(&not_null_set).IsEmpty() {
                        not_null_set = expanded;
                        changed = true;
                    }
                } else {
                    self.addFunctionalDependency(fd.from, fd.to, fd.strict, fd.equiv);
                }
            }
            if !changed {
                break;
            }
        }
        // 决定端已非空的 Lax → 提升为 Strict。
        loop {
            let lax = self
                .fdEdges
                .iter()
                .find(|fd| !fd.strict && fd.from.SubsetOf(&not_null_set))
                .cloned();
            if let Some(fd) = lax {
                self.AddStrictFunctionalDependency(fd.from, fd.to);
            } else {
                break;
            }
        }
        self.NotNullCols = not_null_set;
    }

    /// 从非空集合中去掉可能为 NULL 的列。
    pub fn MakeNullable(&mut self, nullable: FastIntSet) {
        self.NotNullCols.DifferenceWith(&nullable);
    }

    /// 笛卡尔积合并：并入 rhs 的边与 Cond-FD（常量走 AddConstants）。
    pub fn MakeCartesianProduct(&mut self, rhs: &FDSet) {
        for fd in rhs.fdEdges.clone() {
            if fd.isConstant() {
                self.AddConstants(fd.to);
            } else {
                self.fdEdges.push(fd);
            }
        }
        self.ncEdges.extend(rhs.ncEdges.clone());
    }

    /// 若存在严格非等价边使其闭包覆盖全部列，则其决定端可作为主键候选。
    pub fn FindPrimaryKey(&self) -> Option<FastIntSet> {
        let all = self.AllCols();
        self.fdEdges.iter().find_map(|fd| {
            (fd.strict && !fd.equiv && all.SubsetOf(&self.closureOfStrict(&fd.from)))
                .then(|| fd.from.Copy())
        })
    }

    /// 收集 fdEdges 中出现的全部列（等价边只计 from）。
    pub fn AllCols(&self) -> FastIntSet {
        let mut all = empty_set();
        for fd in &self.fdEdges {
            all.UnionWith(&fd.from);
            if !fd.equiv {
                all.UnionWith(&fd.to);
            }
        }
        all
    }

    /// 从另一 FDSet 合并全部边与元数据（按类型走正规插入路径）。
    pub fn AddFrom(&mut self, fds: &FDSet) {
        for fd in fds.fdEdges.clone() {
            if fd.equiv {
                self.addEquivalence(fd.from);
            } else if fd.isConstant() {
                self.AddConstants(fd.to);
            } else if fd.strict {
                self.AddStrictFunctionalDependency(fd.from, fd.to);
            } else {
                self.AddLaxFunctionalDependency(fd.from, fd.to);
            }
        }
        self.ncEdges.extend(fds.ncEdges.clone());
        self.NotNullCols.UnionWith(&fds.NotNullCols);
        for (key, value) in &fds.HashCodeToUniqueID {
            self.HashCodeToUniqueID.entry(key.clone()).or_insert(*value);
        }
        self.GroupByCols.UnionWith(&fds.GroupByCols);
        self.HasAggBuilt = fds.HasAggBuilt;
    }

    /// 外连接后的 FD 传播：保留/弱化内侧依赖，连接条件记为 Cond-FD。
    pub fn MakeOuterJoin(
        &mut self,
        inner: &FDSet,
        filters: &FDSet,
        outer_cols: FastIntSet,
        inner_cols: FastIntSet,
        opt: &ArgOpts,
    ) {
        let left_pk = self.FindPrimaryKey();
        let right_pk = inner.FindPrimaryKey();
        let left_copy = self.clone();
        let right_copy = inner.clone();
        // 内侧非常量/非等价边：决定端含非空列时保持严格，否则降为 Lax。
        for edge in inner.fdEdges.clone() {
            if edge.isConstant() || edge.equiv {
                continue;
            }
            let strict = edge.strict && edge.from.Intersects(&inner.NotNullCols);
            self.addFunctionalDependency(edge.from, edge.to, strict, edge.equiv);
        }
        self.ncEdges.extend(inner.ncEdges.clone());
        let mut combined_from = empty_set();
        let mut combined_to = empty_set();
        for edge in filters.fdEdges.clone() {
            if edge.isConstant() {
                self.AddNCFunctionalDependency(
                    edge.from,
                    edge.to,
                    inner_cols.Copy(),
                    edge.strict,
                    edge.equiv,
                );
                continue;
            }
            if edge.equiv {
                let right = edge.from.Intersection(&inner_cols);
                let left = edge.from.Intersection(&outer_cols);
                // FD rule 3.3.1：累计左右连接键，稍后合成一条严格边。
                if !opt.SkipFDRule331 && !left.IsEmpty() && !right.IsEmpty() {
                    combined_from.UnionWith(&left);
                    combined_to.UnionWith(&right);
                }
                let right_all = right_copy.AllCols();
                let left_all = left_copy.AllCols();
                // 两侧键都能决定各自全列 ⇒ 键并集严格决定两侧全列。
                if right_all.SubsetOf(&right_copy.closureOfStrict(&right))
                    && left_all.SubsetOf(&left_copy.closureOfStrict(&left))
                {
                    self.addFunctionalDependency(
                        left_copy.ReduceCols(left.Copy()),
                        right_all.Union(&left_all),
                        true,
                        false,
                    );
                }
                // 内侧键 ~~> 外侧键（Lax）；完整等价记为 Cond-FD。
                for i in right.SortedArray() {
                    for j in left.SortedArray() {
                        self.addFunctionalDependency(singleton(i), singleton(j), false, false);
                    }
                }
                self.AddNCFunctionalDependency(left, right, inner_cols.Copy(), true, true);
            }
        }
        if !opt.SkipFDRule331 {
            self.addFunctionalDependency(combined_from, combined_to, true, false);
        }
        // 双侧都有主键时，主键并集严格决定全部输出列。
        if let (Some(left), Some(right)) = (left_pk, right_pk) {
            self.addFunctionalDependency(
                left.Union(&right),
                outer_cols.Union(&inner_cols),
                true,
                false,
            );
        }
        if opt.OnlyInnerFilter {
            if opt.InnerIsFalse {
                self.AddConstants(inner_cols.Copy());
            } else {
                for edge in filters.fdEdges.clone() {
                    if edge.strict && (edge.equiv || edge.from.IsEmpty()) {
                        self.addFunctionalDependency(edge.from, edge.to, edge.strict, edge.equiv);
                    }
                }
                for edge in inner.fdEdges.clone() {
                    self.addFunctionalDependency(edge.from, edge.to, edge.strict, edge.equiv);
                }
            }
        }
        self.NotNullCols.UnionWith(&filters.NotNullCols);
        // 外连接补 NULL：内侧列不再保证非空。
        self.NotNullCols.DifferenceWith(&inner_cols);
        for (key, value) in &inner.HashCodeToUniqueID {
            self.HashCodeToUniqueID.insert(key.clone(), *value);
        }
        self.GroupByCols.UnionWith(&inner.GroupByCols);
        self.HasAggBuilt |= inner.HasAggBuilt;
    }

    /// 最多一行语义：保留相关等价，并加入 `{} → cols` 常量边。
    pub fn MaxOneRow(&mut self, cols: FastIntSet) {
        let mut edges = Vec::new();
        for fd in &self.fdEdges {
            if fd.equiv && cols.Intersects(&fd.from) {
                edges.push(fdEdge::new(
                    fd.from.Intersection(&cols),
                    fd.to.Intersection(&cols),
                    true,
                    true,
                ));
            }
        }
        if !cols.IsEmpty() {
            edges.push(fdEdge::new(empty_set(), cols, true, false));
        }
        self.fdEdges = edges;
    }

    /// 投影到 `cols`：裁剪边、用等价替换被删决定列，并过滤 Cond-FD。
    pub fn ProjectCols(&mut self, cols: FastIntSet) {
        let mut const_cols = empty_set();
        let mut det_cols = empty_set();
        let mut equiv_cols = empty_set();
        for fd in &self.fdEdges {
            if fd.isConstant() {
                const_cols = fd.to.Copy();
            }
            if !fd.equiv && !fd.from.SubsetOf(&cols) {
                det_cols.UnionWith(&fd.from.Difference(&cols));
            }
            if fd.equiv && fd.from.Intersects(&cols) {
                equiv_cols.UnionWith(&fd.from);
            }
        }
        let original = self.clone();
        // 严格边依赖端超出投影列时，先扩到闭包再裁剪。
        for fd in &mut self.fdEdges {
            if !fd.to.SubsetOf(&cols) && !fd.equiv && fd.strict {
                let closure_input = fd.to.Union(&fd.from);
                fd.to = original.closureOfStrict(&closure_input);
                fd.to.DifferenceWith(&fd.from);
            }
        }
        det_cols.IntersectionWith(&equiv_cols);
        let equiv_map = self.makeEquivMap(det_cols, cols.Copy());
        if !const_cols.IsEmpty() {
            self.AddConstants(const_cols.Copy());
        }
        let mut retained = Vec::new();
        let mut substituted = Vec::new();
        for mut fd in self.fdEdges.drain(..) {
            if !fd.to.SubsetOf(&cols) {
                if fd.equiv {
                    fd.to.IntersectionWith(&cols);
                    fd.from.IntersectionWith(&cols);
                } else if fd.strict {
                    fd.to.IntersectionWith(&cols);
                } else {
                    // Lax：仅当被删依赖端是常量或非空时才可安全裁剪。
                    let deleted = fd.to.Difference(&cols);
                    if deleted.SubsetOf(&const_cols) || deleted.SubsetOf(&self.NotNullCols) {
                        fd.to.IntersectionWith(&cols);
                    } else {
                        continue;
                    }
                }
                if !fd.isConstant() && fd.removeColumnsToSide(&const_cols) {
                    continue;
                }
                if !fd.isEquivalence() {
                    let from = fd.from.Copy();
                    if fd.removeColumnsToSide(&from) {
                        continue;
                    }
                }
            }
            if !fd.from.SubsetOf(&cols) {
                // 决定端有被投影掉的列：尝试用等价映射替换。
                let deleted = fd.from.Difference(&cols);
                let mut replacements = empty_set();
                let mut found = true;
                for c in deleted.SortedArray() {
                    if let Some(id) = equiv_map.get(&c) {
                        replacements.Insert(*id);
                    } else {
                        found = false;
                        break;
                    }
                }
                if found {
                    fd.from.UnionWith(&replacements);
                    fd.from.DifferenceWith(&deleted);
                    substituted.push(fd);
                }
                continue;
            }
            retained.push(fd);
        }
        self.fdEdges = retained;
        for fd in substituted {
            if fd.equiv {
                self.addEquivalence(fd.from);
            } else if fd.isConstant() {
                self.AddConstants(fd.to);
            } else if fd.strict {
                self.AddStrictFunctionalDependency(fd.from, fd.to);
            } else {
                self.AddLaxFunctionalDependency(fd.from, fd.to);
            }
        }
        // Cond-FD：与 Go 一致，null-constraint 与投影列无交时保持隐藏边不变。
        self.ncEdges.retain_mut(|fd| {
            if fd
                .conditionNC
                .as_ref()
                .is_none_or(|nc| !nc.Intersects(&cols))
            {
                return true;
            }
            if fd.isConstant() {
                fd.to.IntersectionWith(&cols);
                return !fd.to.IsEmpty();
            }
            if fd.equiv {
                fd.from.IntersectionWith(&cols);
                fd.to.IntersectionWith(&cols);
                return !fd.from.IsEmpty();
            }
            true
        });
    }

    /// 为被投影掉的决定列找等价替换：映射到仍保留的等价类成员。
    fn makeEquivMap(&self, det_cols: FastIntSet, projected: FastIntSet) -> HashMap<i32, i32> {
        let mut result = HashMap::new();
        for col in det_cols.SortedArray() {
            let mut closure = self.ClosureOfEquivalence(singleton(col));
            closure.IntersectionWith(&projected);
            if let (id, true) = closure.Next(0) {
                result.insert(col, id);
            }
        }
        result
    }

    /// 将全部 fdEdges 格式化为逗号分隔字符串。
    pub fn String(&self) -> String {
        self.fdEdges
            .iter()
            .map(fdEdge::String)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// 注册表达式 hash_code → 列 unique_id（首次写入生效）。
    pub fn RegisterUniqueID(&mut self, hash_code: String, unique_id: i32) {
        if !hash_code.is_empty() {
            self.HashCodeToUniqueID
                .entry(hash_code)
                .or_insert(unique_id);
        }
    }
    /// 查询 hash_code 是否已注册；未注册返回 `(-1, false)`。
    pub fn IsHashCodeRegistered(&self, hash_code: &str) -> (i32, bool) {
        self.HashCodeToUniqueID
            .get(hash_code)
            .map_or((-1, false), |id| (*id, true))
    }
}

/// `MakeOuterJoin` 的行为开关（对应论文/实现中的可选规则）。
#[derive(Clone, Copy, Debug, Default)]
pub struct ArgOpts {
    /// 跳过 FD rule 3.3.1（左右键合成严格边）。
    pub SkipFDRule331: bool,
    /// 仅存在内侧过滤时的特殊传播。
    pub OnlyInnerFilter: bool,
    /// 内侧过滤恒假 ⇒ 内侧列可视为常量（全 NULL 补行语义）。
    pub InnerIsFalse: bool,
}

#[cfg(test)]
#[path = "fd_graph_test.rs"]
mod fd_graph_test;

/// 求多个 FDSet 共有的等价类：逐集合取交集，仅保留长度 > 1 的结果。
pub fn FindCommonEquivClasses(fd_sets: &[&FDSet]) -> Vec<FastIntSet> {
    let Some(first) = fd_sets.first() else {
        return Vec::new();
    };
    let mut result = first.EquivalenceCols();
    for fd_set in &fd_sets[1..] {
        let mut next = Vec::new();
        for class in &result {
            for edge in &fd_set.fdEdges {
                if edge.equiv {
                    let intersection = class.Intersection(&edge.from);
                    if intersection.Len() > 1 {
                        next.push(intersection);
                    }
                }
            }
        }
        result = next;
        if result.is_empty() {
            break;
        }
    }
    result
}
