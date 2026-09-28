// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 连接重排中的冲突检测器（Conflict Detector）。
//
// 将多表连接树拆成顶点（叶关系）与边（原始连接谓词），并为外连接/半连接等
// 生成冲突规则：候选子集触及 `required` 时不得同时包含 `forbidden` 中未绑定部分。
// TES（Total Eligibility Set，总资格集合）思想体现在边的左右顶点与规则约束上。

// use std::collections::HashMap;
//
// ConflictDetector 对应 Go 的冲突检测器：构图阶段收集顶点、边和规则，枚举阶段校验候选连接。
// pub struct ConflictDetector {
//     pub ctx: base::PlanContext,
//     pub groupRoot: base::LogicalPlan,
//     pub groupVertexes: Vec<Node>,
//     pub innerEdges: Vec<Edge>,
//     pub nonInnerEdges: Vec<Edge>,
//     pub allInnerJoin: bool,
// }
//
// Edge 保存连接类型、谓词、总资格集合 TES 以及左右子树产生的冲突规则。
// pub struct Edge {
//     pub idx: u64,
//     pub joinType: base::JoinType,
//     pub eqConds: Vec<expression::ScalarFunction>,
//     pub nonEQConds: expression::CNFExprs,
//     pub tes: intset::FastIntSet,
//     pub rules: Vec<Rule>,
//     pub skipRules: bool,
//     pub leftEdges: Vec<Edge>,
//     pub rightEdges: Vec<Edge>,
//     pub leftVertexes: intset::FastIntSet,
//     pub rightVertexes: intset::FastIntSet,
// }
//
// Rule 表示 from → to：候选集合触及 from 时必须完整包含 to。
// pub struct Rule {
//     pub from: intset::FastIntSet,
//     pub to: intset::FastIntSet,
// }
//
// Node 是叶关系或中间连接结果；usedEdges 防止同一谓词边被重复消费。
// pub struct Node {
//     pub bitSet: intset::FastIntSet,
//     pub p: base::LogicalPlan,
//     pub cumCost: f64,
//     pub usedEdges: HashMap<u64, ()>,
// }
//
// CheckConnectionResult 汇总两个节点间所有可用内连接边以及至多一条非内连接边。
// pub struct CheckConnectionResult {
//     pub node1: Node,
//     pub node2: Node,
//     pub appliedInnerEdges: Vec<Edge>,
//     pub appliedNonInnerEdge: Option<Edge>,
//     pub hasEQCond: bool,
// }
//
// validateCumCost 对应 Go 的成本防御检查；正无穷用于禁用笛卡尔积，因而允许保留。
// pub fn validateCumCost(cost: f64) -> Result<(), errors::Error> {
//     if cost.is_nan() {
//         return Err(errors::New("invalid cumulative cost: NaN"));
//     }
//     if cost == f64::NEG_INFINITY {
//         return Err(errors::New("invalid cumulative cost: -Inf"));
//     }
//     if cost < 0.0 {
//         return Err(errors::Errorf(format!("invalid cumulative cost: negative value {cost}")));
//     }
//     Ok(())
// }
//
// calcCumCost 把当前计划行数与两个子节点的累计成本相加。
// pub fn calcCumCost(p: &base::LogicalPlan, node1: Option<&Node>, node2: Option<&Node>) -> f64 {
//     p.StatsInfo().RowCount
//         + node1.map_or(0.0, |node| node.cumCost)
//         + node2.map_or(0.0, |node| node.cumCost)
// }
//
// calcCumCostByChildren 递归计算原始子树成本，作为叶顶点的初始成本。
// pub fn calcCumCostByChildren(p: &base::LogicalPlan) -> f64 {
//     let mut cost = p.StatsInfo().RowCount;
//     for child in p.Children() {
//         cost += calcCumCostByChildren(child);
//     }
//     cost
// }
//
// impl Node {
//     pub fn checkUsedEdges(&self, edgeIdx: u64) -> bool {
//         self.usedEdges.contains_key(&edgeIdx)
//     }
// }
//
// pub fn newConflictDetector(ctx: base::PlanContext) -> ConflictDetector {
//     ConflictDetector {
//         ctx,
//         groupRoot: Default::default(),
//         groupVertexes: Vec::new(),
//         innerEdges: Vec::new(),
//         nonInnerEdges: Vec::new(),
//         allInnerJoin: false,
//     }
// }
//
// impl ConflictDetector {
// TryCreateCartesianCheckResult 只允许全内连接组生成无等值谓词的回退边。
//     pub fn TryCreateCartesianCheckResult(&mut self, left: Node, right: Node) -> Option<CheckConnectionResult> {
//         if !self.allInnerJoin {
//             return None;
//         }
//         let edge = self.makeEdge(
//             base::InnerJoin,
//             &[],
//             &left.bitSet,
//             &right.bitSet,
//             &[],
//             &[],
//         );
//         Some(CheckConnectionResult {
//             node1: left,
//             node2: right,
//             appliedInnerEdges: vec![edge],
//             appliedNonInnerEdge: None,
//             hasEQCond: false,
//         })
//     }
//
// iterateEdges 保留 Go 的提前停止语义，先遍历内连接边再遍历非内连接边。
//     pub fn iterateEdges(&self, mut visitor: impl FnMut(&Edge) -> bool) {
//         for edge in self.innerEdges.iter().chain(self.nonInnerEdges.iter()) {
//             if !visitor(edge) {
//                 break;
//             }
//         }
//     }
//
// Build 从 joinGroup 生成顶点映射，并自底向上建立边及冲突规则。
//     pub fn Build(&mut self, group: &JoinGroup) -> Result<Vec<Node>, errors::Error> {
//         self.groupRoot = group.root.clone();
//         self.allInnerJoin = group.allInnerJoin;
//         let mut vertexMap = HashMap::new();
//         for (idx, vertex) in group.vertexes.iter().enumerate() {
//             vertex.RecursiveDeriveStats(None)?;
//             let cost = calcCumCostByChildren(vertex);
//             validateCumCost(cost)?;
//             vertexMap.insert(vertex.ID(), Node {
//                 bitSet: intset::new_fast_int_set(idx as i32),
//                 p: vertex.clone(),
//                 cumCost: cost,
//                 usedEdges: HashMap::new(),
//             });
//         }
// 递归调用会按遇到叶节点的次序填充 groupVertexes。
//         self.buildRecursive(group, &group.root, &mut vertexMap)?;
//         Ok(self.groupVertexes.clone())
//     }
//
// buildRecursive 穿透 Selection；其余内部节点必须是 LogicalJoin。
//     pub fn buildRecursive(
//         &mut self,
//         group: &JoinGroup,
//         plan: &base::LogicalPlan,
//         vertexMap: &mut HashMap<i32, Node>,
//     ) -> Result<(Vec<Edge>, intset::FastIntSet), errors::Error> {
//         if let Some(vertex) = vertexMap.remove(&plan.ID()) {
//             let bits = vertex.bitSet.clone();
//             self.groupVertexes.push(vertex);
//             return Ok((Vec::new(), bits));
//         }
//         if let Some(selection) = plan.as_logical_selection() {
//             let (mut childEdges, childVertexes) =
//                 self.buildRecursive(group, &selection.Children()[0], vertexMap)?;
//             let conds = group.selConds.get(&selection.ID()).ok_or_else(||
//                 errors::Errorf(format!("unexpected Selection node (ID: {})", selection.ID()))
//             )?;
// Selection 边没有子边，不参与父子连接类型的冲突规则推导。
//             let mut edge = self.makeEdgeInternal(
//                 base::InnerJoin,
//                 intset::FastIntSet::default(),
//                 intset::FastIntSet::default(),
//                 &[],
//                 &[],
//                 childVertexes.clone(),
//             );
//             edge.nonEQConds = conds.clone();
//             childEdges.push(edge);
//             return Ok((childEdges, childVertexes));
//         }
//         let join = plan.as_logical_join().ok_or_else(||
//             errors::New("unexpected plan type in conflict detector")
//         )?;
//         let (leftEdges, leftVertexes) = self.buildRecursive(group, &join.Children()[0], vertexMap)?;
//         let (rightEdges, rightVertexes) = self.buildRecursive(group, &join.Children()[1], vertexMap)?;
//         let current = if join.JoinType == base::InnerJoin {
//             self.makeInnerEdge(join, &leftVertexes, &rightVertexes, &leftEdges, &rightEdges)?
//         } else {
//             vec![self.makeNonInnerEdge(join, &leftVertexes, &rightVertexes, &leftEdges, &rightEdges)?]
//         };
//         if leftVertexes.Intersects(&rightVertexes) {
//             return Err(errors::New("conflicting join edges detected"));
//         }
//         let mut all = leftEdges;
//         all.extend(rightEdges);
//         all.extend(current);
//         Ok((all, leftVertexes.Union(&rightVertexes)))
//     }
//
// makeInnerEdge 将每个等值或非等值合取项拆成独立边，扩大可枚举空间。
//     pub fn makeInnerEdge(
//         &mut self,
//         join: &logicalop::LogicalJoin,
//         left: &intset::FastIntSet,
//         right: &intset::FastIntSet,
//         leftEdges: &[Edge],
//         rightEdges: &[Edge],
//     ) -> Result<Vec<Edge>, errors::Error> {
//         if !join.NAEQConditions.is_empty() {
//             return Err(errors::New("NAEQConditions not supported in conflict detector yet"));
//         }
//         let mut result = Vec::new();
//         if join.EqualConditions.is_empty()
//             && join.LeftConditions.is_empty()
//             && join.RightConditions.is_empty()
//             && join.OtherConditions.is_empty()
//         {
//             result.push(self.makeEdge(base::InnerJoin, &[], left, right, leftEdges, rightEdges));
//         }
//         for cond in &join.EqualConditions {
//             let mut edge = self.makeEdge(base::InnerJoin, &[cond.as_expression()], left, right, leftEdges, rightEdges);
//             edge.eqConds.push(cond.clone());
//             result.push(edge);
//         }
//         for cond in join.OtherConditions.iter().chain(&join.LeftConditions).chain(&join.RightConditions) {
//             let mut edge = self.makeEdge(base::InnerJoin, &[cond.clone()], left, right, leftEdges, rightEdges);
//             edge.nonEQConds.push(cond.clone());
//             result.push(edge);
//         }
//         Ok(result)
//     }
//
// makeNonInnerEdge 将外连接、半连接和反连接的全部谓词保留在同一原子边中。
//     pub fn makeNonInnerEdge(
//         &mut self,
//         join: &logicalop::LogicalJoin,
//         left: &intset::FastIntSet,
//         right: &intset::FastIntSet,
//         leftEdges: &[Edge],
//         rightEdges: &[Edge],
//     ) -> Result<Edge, errors::Error> {
//         if !join.NAEQConditions.is_empty() {
//             return Err(errors::New("NAEQConditions not supported in conflict detector yet"));
//         }
//         let mut nonEq = join.LeftConditions.clone();
//         nonEq.extend(join.RightConditions.clone());
//         nonEq.extend(join.OtherConditions.clone());
//         let mut conds = expression::scalar_funcs_to_exprs(&join.EqualConditions);
//         conds.extend(nonEq.clone());
//         let mut edge = self.makeEdge(join.JoinType, &conds, left, right, leftEdges, rightEdges);
//         edge.eqConds = join.EqualConditions.clone();
//         edge.nonEQConds = nonEq;
//         Ok(edge)
//     }
//
// makeEdge 先从谓词引用列计算 SES，并以其作为当前 TES。
//     pub fn makeEdge(
//         &mut self,
//         joinType: base::JoinType,
//         conds: &[expression::Expression],
//         left: &intset::FastIntSet,
//         right: &intset::FastIntSet,
//         leftEdges: &[Edge],
//         rightEdges: &[Edge],
//     ) -> Edge {
//         let tes = self.calcSES(conds);
//         self.makeEdgeInternal(joinType, left.clone(), right.clone(), leftEdges, rightEdges, tes)
//     }
//
// makeEdgeInternal 对退化谓词补齐左右关系，并按结合律表生成冲突规则。
//     pub fn makeEdgeInternal(
//         &mut self,
//         joinType: base::JoinType,
//         left: intset::FastIntSet,
//         right: intset::FastIntSet,
//         leftEdges: &[Edge],
//         rightEdges: &[Edge],
//         mut tes: intset::FastIntSet,
//     ) -> Edge {
//         if !tes.Intersects(&left) { tes = tes.Union(&left); }
//         if !tes.Intersects(&right) { tes = tes.Union(&right); }
//         let mut edge = Edge {
//             idx: (self.innerEdges.len() + self.nonInnerEdges.len()) as u64,
//             joinType,
//             eqConds: Vec::new(),
//             nonEQConds: Vec::new(),
//             tes,
//             rules: Vec::new(),
//             skipRules: self.allInnerJoin,
//             leftEdges: leftEdges.to_vec(),
//             rightEdges: rightEdges.to_vec(),
//             leftVertexes: left,
//             rightVertexes: right,
//         };
//         if !self.allInnerJoin {
//             for child in leftEdges {
//                 if !assoc(child, &edge) { edge.rules.push(rightToLeftRule(child)); }
//                 if !leftAsscom(child, &edge) { edge.rules.push(leftToRightRule(child)); }
//             }
//             for child in rightEdges {
//                 if !assoc(&edge, child) { edge.rules.push(leftToRightRule(child)); }
//                 if !rightAsscom(&edge, child) { edge.rules.push(rightToLeftRule(child)); }
//             }
//         }
// Go 将边同时登记到 detector；保留克隆形状，实际所有权以后续模块接线为准。
//         if joinType == base::InnerJoin { self.innerEdges.push(edge.clone()); }
//         else { self.nonInnerEdges.push(edge.clone()); }
//         edge
//     }
//
// calcSES 收集所有被谓词引用的基础关系位集合。
//     pub fn calcSES(&self, conds: &[expression::Expression]) -> intset::FastIntSet {
//         let mut result = intset::FastIntSet::default();
//         for cond in conds {
//             for node in &self.groupVertexes {
//                 if expression::expr_reference_schema(cond, node.p.Schema()) {
//                     result = result.Union(&node.bitSet);
//                 }
//             }
//         }
//         result
//     }
//
// CheckConnection 遍历未使用边；非内连接边必须方向唯一且最多一条。
//     pub fn CheckConnection(&self, mut node1: Node, mut node2: Node) -> Result<CheckConnectionResult, errors::Error> {
//         let mut inner = Vec::new();
//         let mut nonInner = None;
//         let mut hasEq = false;
//         for edge in &self.innerEdges {
//             if !node1.checkUsedEdges(edge.idx) && !node2.checkUsedEdges(edge.idx)
//                 && edge.checkInnerEdgeApplicable(&node1, &node2)
//             {
//                 hasEq |= !edge.eqConds.is_empty();
//                 inner.push(edge.clone());
//             }
//         }
//         let mut swap = false;
//         for edge in &self.nonInnerEdges {
//             if node1.checkUsedEdges(edge.idx) || node2.checkUsedEdges(edge.idx) { continue; }
//             let forward = edge.checkNonInnerEdgeApplicable(&node1, &node2);
//             let reverse = edge.checkNonInnerEdgeApplicable(&node2, &node1);
//             if forward && reverse {
//                 return Err(errors::New("node1 and node2 cannot be connected by non-inner edges of different direction"));
//             }
//             if forward || reverse {
//                 if nonInner.is_some() { return Err(errors::New("multiple non-inner edges applied between two nodes")); }
//                 hasEq |= !edge.eqConds.is_empty();
//                 nonInner = Some(edge.clone());
//                 swap = reverse;
//             }
//         }
//         if swap { std::mem::swap(&mut node1, &mut node2); }
//         Ok(CheckConnectionResult { node1, node2, appliedInnerEdges: inner, appliedNonInnerEdge: nonInner, hasEQCond: hasEq })
//     }
//
// MakeJoin 先应用非内连接边，再把额外内连接边转成 join 谓词或 Selection。
//     pub fn MakeJoin(
//         &self,
//         mut result: CheckConnectionResult,
//         vertexHints: &HashMap<i32, JoinMethodHint>,
//     ) -> Result<Node, errors::Error> {
//         let mut plan = None;
//         if result.appliedNonInnerEdge.is_some() {
//             plan = Some(makeNonInnerJoin(&self.ctx, &mut result, vertexHints)?);
//         }
//         if !result.appliedInnerEdges.is_empty() {
//             plan = Some(makeInnerJoin(&self.ctx, &mut result, plan, vertexHints)?);
//         }
//         let plan = plan.ok_or_else(|| errors::New("failed to make join plan"))?;
//         plan.RecursiveDeriveStats(None)?;
//         let mut used = result.node1.usedEdges.clone();
//         used.extend(result.node2.usedEdges.clone());
//         for edge in &result.appliedInnerEdges { used.insert(edge.idx, ()); }
//         if let Some(edge) = &result.appliedNonInnerEdge { used.insert(edge.idx, ()); }
//         let cost = calcCumCost(&plan, Some(&result.node1), Some(&result.node2));
//         validateCumCost(cost)?;
//         Ok(Node { bitSet: result.node1.bitSet.Union(&result.node2.bitSet), p: plan, cumCost: cost, usedEdges: used })
//     }
//
//     pub fn HasRemainingEdges(&self, used: &HashMap<u64, ()>) -> bool {
//         self.innerEdges.iter().chain(&self.nonInnerEdges).any(|edge|
//             (!edge.eqConds.is_empty() || !edge.nonEQConds.is_empty()) && !used.contains_key(&edge.idx))
//     }
//
// HasRemainingEdgesInSubset 只报告 TES 和原始左右顶点都完整落入子集的真实未用边。
//     pub fn HasRemainingEdgesInSubset(&self, subset: &intset::FastIntSet, used: &HashMap<u64, ()>) -> bool {
//         self.innerEdges.iter().chain(&self.nonInnerEdges).any(|edge| {
//             (!edge.eqConds.is_empty() || !edge.nonEQConds.is_empty())
//                 && !used.contains_key(&edge.idx)
//                 && edge.tes.SubsetOf(subset)
//                 && edge.leftVertexes.Union(&edge.rightVertexes).SubsetOf(subset)
//         })
//     }
// }
//
// impl CheckConnectionResult {
//     pub fn Connected(&self) -> bool {
//         !self.appliedInnerEdges.is_empty() || self.appliedNonInnerEdge.is_some()
//     }
//     pub fn NoEQEdge(&self) -> bool { !self.hasEQCond }
// }
//
// impl Edge {
// 内连接无方向，只要求 TES 横跨两侧且所有冲突规则成立。
//     pub fn checkInnerEdgeApplicable(&self, left: &Node, right: &Node) -> bool {
//         (self.skipRules || self.checkRules(left, right))
//             && self.tes.SubsetOf(&left.bitSet.Union(&right.bitSet))
//             && self.tes.Intersects(&left.bitSet)
//             && self.tes.Intersects(&right.bitSet)
//     }
//
// 非内连接还要求原始左、右关系保持在对应候选输入中。
//     pub fn checkNonInnerEdgeApplicable(&self, left: &Node, right: &Node) -> bool {
//         (self.skipRules || self.checkRules(left, right))
//             && self.leftVertexes.Intersection(&self.tes).SubsetOf(&left.bitSet)
//             && self.rightVertexes.Intersection(&self.tes).SubsetOf(&right.bitSet)
//             && self.tes.Intersects(&left.bitSet)
//             && self.tes.Intersects(&right.bitSet)
//     }
//
//     pub fn checkRules(&self, left: &Node, right: &Node) -> bool {
//         let set = left.bitSet.Union(&right.bitSet);
//         self.rules.iter().all(|rule| !rule.from.Intersects(&set) || rule.to.SubsetOf(&set))
//     }
// }
//
// pub fn rightToLeftRule(child: &Edge) -> Rule {
//     let to = if child.leftVertexes.Intersects(&child.tes) {
//         child.leftVertexes.Intersection(&child.tes)
//     } else { child.leftVertexes.clone() };
//     Rule { from: child.rightVertexes.clone(), to }
// }
//
// pub fn leftToRightRule(child: &Edge) -> Rule {
//     let to = if child.rightVertexes.Intersects(&child.tes) {
//         child.rightVertexes.Intersection(&child.tes)
//     } else { child.rightVertexes.clone() };
//     Rule { from: child.leftVertexes.clone(), to }
// }
//
// joinTypeConvertTable 将 TiDB 七种连接类型压缩到论文规则表的五类。
// pub const JOIN_TYPE_CONVERT_TABLE: [usize; 7] = [0, 1, 2, 3, 4, 3, 4];
//
// pub fn assoc(left: &Edge, right: &Edge) -> bool {
//     ASSOC_RULE_TABLE[JOIN_TYPE_CONVERT_TABLE[left.joinType as usize]][JOIN_TYPE_CONVERT_TABLE[right.joinType as usize]] == 1
// }
// pub fn leftAsscom(left: &Edge, right: &Edge) -> bool {
//     LEFT_ASSCOM_RULE_TABLE[JOIN_TYPE_CONVERT_TABLE[left.joinType as usize]][JOIN_TYPE_CONVERT_TABLE[right.joinType as usize]] == 1
// }
// pub fn rightAsscom(left: &Edge, right: &Edge) -> bool {
//     RIGHT_ASSCOM_RULE_TABLE[JOIN_TYPE_CONVERT_TABLE[left.joinType as usize]][JOIN_TYPE_CONVERT_TABLE[right.joinType as usize]] == 1
// }
//
// alignEQConds 保证等值条件的左右参数与连接输入方向一致；反向时重建表达式。
// pub fn alignEQConds(
//     ctx: &base::PlanContext,
//     mut left: base::LogicalPlan,
//     mut right: base::LogicalPlan,
//     conds: &[expression::ScalarFunction],
// ) -> Result<(base::LogicalPlan, base::LogicalPlan, Vec<expression::ScalarFunction>), errors::Error> {
//     let mut aligned = Vec::with_capacity(conds.len());
//     for cond in conds {
//         let args = cond.GetArgs();
//         if args.len() != 2 { return Err(errors::Errorf(format!("unexpected eq condition args: {}", args.len()))); }
//         if expression::expr_from_schema(&args[0], left.Schema()) && expression::expr_from_schema(&args[1], right.Schema()) {
//             aligned.push(cond.clone());
//         } else if expression::expr_from_schema(&args[1], left.Schema()) && expression::expr_from_schema(&args[0], right.Schema()) {
// 非列表达式需要先注入 Projection，随后再用生成列重建等值条件。
//             let (newLeft, lArg) = logicalop::inject_if_not_column(left, args[1].clone());
//             let (newRight, rArg) = logicalop::inject_if_not_column(right, args[0].clone());
//             left = newLeft;
//             right = newRight;
//             aligned.push(expression::new_function_internal(ctx.GetExprCtx(), &cond.FuncName, cond.GetStaticType(), lArg, rArg));
//         } else {
//             return Err(errors::New("eq condition does not match join sides"));
//         }
//     }
//     Ok((left, right, aligned))
// }
//
// makeNonInnerJoin 恢复外连接两侧方向，并按表达式来源重新分类非等值条件。
// pub fn makeNonInnerJoin(
//     ctx: &base::PlanContext,
//     result: &mut CheckConnectionResult,
//     hints: &HashMap<i32, JoinMethodHint>,
// ) -> Result<base::LogicalPlan, errors::Error> {
//     let edge = result.appliedNonInnerEdge.as_ref().unwrap();
//     let (left, right, eq) = alignEQConds(ctx, result.node1.p.clone(), result.node2.p.clone(), &edge.eqConds)?;
//     let mut join = newCartesianJoin(ctx, edge.joinType, left.clone(), right.clone(), hints)?;
//     join.EqualConditions = eq.into_iter().map(|cond| alignNotNullWithSchema(cond.into(), join.Schema()).0.into_scalar()).collect();
//     for cond in &edge.nonEQConds {
//         let aligned = alignNotNullWithSchema(cond.clone(), join.Schema()).0;
//         if expression::is_mutable_effects_expr(&aligned) { join.OtherConditions.push(aligned); }
//         else if expression::expr_from_schema(&aligned, left.Schema()) { join.LeftConditions.push(aligned); }
//         else if expression::expr_from_schema(&aligned, right.Schema()) { join.RightConditions.push(aligned); }
//         else { join.OtherConditions.push(aligned); }
//     }
//     Ok(join.into())
// }
//
// alignNotNullWithSchema 递归以连接输出 Schema 的列类型同步 NOT_NULL 标志，采用写时复制。
// pub fn alignNotNullWithSchema(expr: expression::Expression, schema: &expression::Schema) -> (expression::Expression, bool) {
//     expression::rewrite_copy_on_write(expr, |column| {
//         let Some(schemaColumn) = schema.RetrieveColumn(column) else { return None; };
//         if mysql::has_not_null_flag(column.RetType.GetFlag()) == mysql::has_not_null_flag(schemaColumn.RetType.GetFlag()) {
//             return None;
//         }
//         let mut cloned = column.clone();
//         cloned.RetType.set_not_null(mysql::has_not_null_flag(schemaColumn.RetType.GetFlag()));
//         Some(cloned)
//     })
// }
//
// makeInnerJoin 在已有非内连接上附加 Selection；否则创建新的内连接并汇总各边条件。
// pub fn makeInnerJoin(
//     ctx: &base::PlanContext,
//     result: &mut CheckConnectionResult,
//     existing: Option<base::LogicalPlan>,
//     hints: &HashMap<i32, JoinMethodHint>,
// ) -> Result<base::LogicalPlan, errors::Error> {
//     if let Some(plan) = existing {
//         let mut conditions = Vec::new();
//         for edge in &result.appliedInnerEdges {
//             conditions.extend(expression::scalar_funcs_to_exprs(&edge.eqConds));
//             conditions.extend(edge.nonEQConds.clone());
//         }
//         return Ok(logicalop::LogicalSelection::new(conditions).Init(ctx, plan.QueryBlockOffset()).with_child(plan));
//     }
//     let mut eq = Vec::new();
//     let mut other = Vec::new();
//     for edge in &result.appliedInnerEdges {
//         let (left, right, aligned) = alignEQConds(ctx, result.node1.p.clone(), result.node2.p.clone(), &edge.eqConds)?;
//         result.node1.p = left;
//         result.node2.p = right;
//         eq.extend(aligned);
//         other.extend(edge.nonEQConds.clone());
//     }
//     let mut join = newCartesianJoin(ctx, result.appliedInnerEdges[0].joinType, result.node1.p.clone(), result.node2.p.clone(), hints)?;
//     join.EqualConditions.extend(eq);
//     join.OtherConditions.extend(other);
//     Ok(join.into())
// }
//
// newCartesianJoin 创建已重排的 LogicalJoin，并合并左右 Schema 与连接方法 hint。
// pub fn newCartesianJoin(
//     ctx: &base::PlanContext,
//     joinType: base::JoinType,
//     left: base::LogicalPlan,
//     right: base::LogicalPlan,
//     hints: &HashMap<i32, JoinMethodHint>,
// ) -> Result<logicalop::LogicalJoin, errors::Error> {
//     let offset = if left.QueryBlockOffset() == right.QueryBlockOffset() { left.QueryBlockOffset() } else { -1 };
//     let mut join = logicalop::LogicalJoin::new(joinType, true).Init(ctx, offset);
//     join.SetSchema(expression::merge_schema(left.Schema(), right.Schema()));
//     join.SetChildren(left, right);
//     join.MergeSchema();
//     SetNewJoinWithHint(&mut join, hints);
//     Ok(join)
// }
//
// ruleTableEntry 保留 Go 的声明名；Rust 非驼峰命名警告暂不处理。
// pub type ruleTableEntry = i32;
//
// 三张表逐项保留 Go 源码值；0 表示需生成冲突规则，1 表示变换恒成立。
// pub const ASSOC_RULE_TABLE: [[ruleTableEntry; 5]; 5] = [
//     [1, 1, 0, 1, 1],
//     [0, 1, 0, 0, 0],
//     [1, 1, 1, 1, 1],
//     [0, 0, 0, 0, 0],
//     [0, 0, 0, 0, 0],
// ];
// */
use crate::util::{
    Expr, JoinMethodHint, JoinType, PlanKind, PlanNode, align_join_edge_args,
    set_new_join_with_hint,
};
use std::collections::{BTreeMap, BTreeSet};

/// 冲突规则 `from -> to`：候选集合触及 `from` 时必须完整包含 `to`。
#[derive(Clone, Debug)]
pub struct Rule {
    /// 候选集合只要触及该集合就会触发规则。
    pub from: BTreeSet<usize>,
    /// 触发后候选集合必须完整包含的集合。
    pub to: BTreeSet<usize>,
}
/// 一条原始连接边：左右顶点、谓词、连接类型及派生冲突规则。
#[derive(Clone, Debug)]
pub struct Edge {
    /// 边在检测器中的下标，用于 used_edges 去重消费。
    pub index: usize,
    /// 内连接 / 外连接 / 半连接等连接类型。
    pub join_type: JoinType,
    /// 左边（构建侧）顶点集合。
    pub left: BTreeSet<usize>,
    /// 右边（探测侧）顶点集合。
    pub right: BTreeSet<usize>,
    /// 等值与非等值连接条件表达式。
    pub conditions: Vec<Expr>,
    /// Total Eligibility Set：应用本边所需的最小顶点集合。
    pub tes: BTreeSet<usize>,
    /// 由连接类型推导的冲突规则列表。
    pub rules: Vec<Rule>,
    /// 是否含等值谓词；无等值时可能走笛卡尔积代价放大。
    pub has_equality: bool,
}
/// 枚举中的节点：叶关系或已合并的中间连接结果。
#[derive(Clone, Debug)]
pub struct Node {
    /// 对应的逻辑计划子树。
    pub plan: PlanNode,
    /// 本节点覆盖的叶顶点集合（位集合语义）。
    pub vertexes: BTreeSet<usize>,
    /// 已消费的边下标，防止同一谓词边被重复使用。
    pub used_edges: BTreeSet<usize>,
    /// 累计代价：子节点代价之和加上当前计划估计行数。
    pub cumulative_cost: f64,
}
impl Node {
    /// 由叶计划构造节点，顶点集与初始代价取自计划本身。
    pub fn leaf(plan: PlanNode) -> Self {
        let vertexes = plan.vertexes();
        let cumulative_cost = plan.cumulative_cost;
        Self {
            plan,
            vertexes,
            used_edges: BTreeSet::new(),
            cumulative_cost,
        }
    }
}

/// 检查两节点是否可通过某条未用边合法连接的结果。
#[derive(Clone, Debug)]
pub struct CheckConnectionResult {
    /// 左侧候选节点。
    pub left: Node,
    /// 右侧候选节点。
    pub right: Node,
    /// 同时适用的全部内连接边。
    pub applied_inner_edges: Vec<Edge>,
    /// 至多一条适用的非内连接边。
    pub applied_non_inner_edge: Option<Edge>,
}
impl CheckConnectionResult {
    /// 是否找到了可用连接边。
    pub fn connected(&self) -> bool {
        !self.applied_inner_edges.is_empty() || self.applied_non_inner_edge.is_some()
    }
    /// 匹配边是否无等值条件（笛卡尔积候选）。
    pub fn no_equality_edge(&self) -> bool {
        !self
            .applied_inner_edges
            .iter()
            .any(|edge| edge.has_equality)
            && self
                .applied_non_inner_edge
                .as_ref()
                .is_none_or(|edge| !edge.has_equality)
    }
}

/// 冲突检测器：持有一组边，供 DP/贪心在枚举时校验与建连。
#[derive(Clone, Debug, Default)]
pub struct ConflictDetector {
    /// 自底向上从原始连接树抽取的全部边。
    pub edges: Vec<Edge>,
}

impl ConflictDetector {
    /// 从连接树根构图，返回检测器与叶节点列表。
    pub fn build(root: &PlanNode) -> Result<(Self, Vec<Node>), String> {
        let mut detector = Self::default();
        let mut leaves = Vec::new();
        detector.build_recursive(root, &mut leaves)?;
        Ok((detector, leaves))
    }
    /// 递归穿透非 Join 为叶；Join 则收集左右顶点并登记边与冲突规则。
    fn build_recursive(
        &mut self,
        plan: &PlanNode,
        leaves: &mut Vec<Node>,
    ) -> Result<(Vec<usize>, BTreeSet<usize>), String> {
        let PlanKind::Join {
            join_type,
            equal_conditions,
            other_conditions,
            ..
        } = &plan.kind
        else {
            leaves.push(Node::leaf(plan.clone()));
            return Ok((Vec::new(), plan.vertexes()));
        };
        if plan.children.len() != 2 {
            return Err("join must have exactly two children".to_string());
        }
        let (left_edges, left) = self.build_recursive(&plan.children[0], leaves)?;
        let (right_edges, right) = self.build_recursive(&plan.children[1], leaves)?;
        if !left.is_disjoint(&right) {
            return Err("conflicting join edges detected".to_string());
        }
        let mut conditions = equal_conditions.clone();
        conditions.extend(other_conditions.clone());
        let condition_vertexes: BTreeSet<_> = conditions.iter().flat_map(Expr::leaf_ids).collect();

        // Go splits inner-join CNF predicates into independent edges so a
        // candidate can consume every predicate separately.  Non-inner joins
        // remain atomic because their predicates cannot be applied in pieces.
        let edge_conditions: Vec<(Vec<Expr>, bool)> = if *join_type == JoinType::Inner {
            if equal_conditions.is_empty() && other_conditions.is_empty() {
                vec![(Vec::new(), false)]
            } else {
                equal_conditions
                    .iter()
                    .cloned()
                    .map(|condition| (vec![condition], true))
                    .chain(
                        other_conditions
                            .iter()
                            .cloned()
                            .map(|condition| (vec![condition], false)),
                    )
                    .collect()
            }
        } else {
            vec![(conditions, !equal_conditions.is_empty())]
        };

        let mut current_edges = Vec::new();
        for (edge_conditions, has_equality) in edge_conditions {
            let mut tes: BTreeSet<_> = edge_conditions.iter().flat_map(Expr::leaf_ids).collect();
            if tes.is_disjoint(&left) {
                tes.extend(&left);
            }
            if tes.is_disjoint(&right) {
                tes.extend(&right);
            }
            let mut rules = Vec::new();
            for &child_index in &left_edges {
                let child = &self.edges[child_index];
                if !assoc(child.join_type, *join_type) {
                    rules.push(right_to_left_rule(child));
                }
                if !left_asscom(child.join_type, *join_type) {
                    rules.push(left_to_right_rule(child));
                }
            }
            for &child_index in &right_edges {
                let child = &self.edges[child_index];
                if !assoc(*join_type, child.join_type) {
                    rules.push(left_to_right_rule(child));
                }
                if !right_asscom(*join_type, child.join_type) {
                    rules.push(right_to_left_rule(child));
                }
            }
            let index = self.edges.len();
            self.edges.push(Edge {
                index,
                join_type: *join_type,
                left: left.clone(),
                right: right.clone(),
                has_equality,
                conditions: edge_conditions,
                tes,
                rules,
            });
            current_edges.push(index);
        }
        let mut all_edges = left_edges;
        all_edges.extend(right_edges);
        all_edges.extend(current_edges);
        let mut all = left;
        all.extend(right);
        all.extend(condition_vertexes);
        Ok((all_edges, all))
    }

    /// 在两节点间查找尚未消费且不违反冲突规则的边。
    pub fn check_connection(
        &self,
        left: &Node,
        right: &Node,
    ) -> Result<CheckConnectionResult, String> {
        if !left.vertexes.is_disjoint(&right.vertexes) {
            return Err("join nodes overlap".to_string());
        }
        let mut applied_inner_edges = Vec::new();
        let mut applied_non_inner_edge = None;
        let mut swap = false;
        for edge in &self.edges {
            if left.used_edges.contains(&edge.index) || right.used_edges.contains(&edge.index) {
                continue;
            }
            if edge.join_type == JoinType::Inner {
                if edge_applicable(edge, left, right) {
                    applied_inner_edges.push(edge.clone());
                }
                continue;
            }
            let forward = edge_applicable(edge, left, right);
            let reverse = edge_applicable(edge, right, left);
            if forward && reverse {
                return Err(
                    "node1 and node2 cannot be connected by non-inner edges of different direction"
                        .to_string(),
                );
            }
            if forward || reverse {
                if applied_non_inner_edge.is_some() {
                    return Err("multiple non-inner edges applied between two nodes".to_string());
                }
                applied_non_inner_edge = Some(edge.clone());
                swap = reverse;
            }
        }
        let (left, right) = if swap { (right, left) } else { (left, right) };
        Ok(CheckConnectionResult {
            left: left.clone(),
            right: right.clone(),
            applied_inner_edges,
            applied_non_inner_edge,
        })
    }

    /// 根据匹配边构造新的中间连接节点，合并 used_edges 与列集合。
    pub fn make_join(
        &self,
        result: CheckConnectionResult,
        hints: &BTreeMap<usize, JoinMethodHint>,
    ) -> Result<Node, String> {
        let mut applied = result.applied_inner_edges;
        if let Some(edge) = result.applied_non_inner_edge {
            applied.insert(0, edge);
        }
        let join_edge = applied
            .first()
            .ok_or_else(|| "cannot make join without edge".to_string())?;
        let mut equal_conditions = Vec::new();
        let mut other_conditions = Vec::new();
        // 按左右列对齐等值条件；对齐失败则归入非等值条件。
        for candidate in &applied {
            for condition in &candidate.conditions {
                if candidate.has_equality {
                    let condition = align_join_edge_args(
                        condition,
                        &result.left.plan.columns,
                        &result.right.plan.columns,
                    )
                    .ok_or_else(|| "eq condition does not match join sides".to_string())?;
                    equal_conditions.push(condition);
                } else {
                    other_conditions.push(condition.clone());
                }
            }
        }
        // 内连接用选择率启发式；外连接等取两侧行数上界。
        let estimated_rows = match join_edge.join_type {
            JoinType::Inner => (result.left.plan.estimated_rows
                * result.right.plan.estimated_rows
                * if equal_conditions.is_empty() {
                    1.0
                } else {
                    0.1
                })
            .max(1.0),
            _ => result
                .left
                .plan
                .estimated_rows
                .max(result.right.plan.estimated_rows),
        };
        let mut columns = result.left.plan.columns.clone();
        columns.extend(&result.right.plan.columns);
        let cumulative_cost =
            result.left.cumulative_cost + result.right.cumulative_cost + estimated_rows;
        if cumulative_cost.is_nan() || cumulative_cost == f64::NEG_INFINITY || cumulative_cost < 0.0
        {
            return Err("invalid cumulative join cost".to_string());
        }
        let mut plan = PlanNode {
            id: result.left.plan.id.min(result.right.plan.id),
            kind: PlanKind::Join {
                join_type: join_edge.join_type,
                equal_conditions,
                other_conditions,
                hint: JoinMethodHint::default(),
            },
            children: vec![result.left.plan, result.right.plan],
            columns,
            estimated_rows,
            cumulative_cost,
        };
        set_new_join_with_hint(&mut plan, hints);
        let mut used_edges = result.left.used_edges;
        used_edges.extend(result.right.used_edges);
        for applied_edge in applied {
            used_edges.insert(applied_edge.index);
        }
        let vertexes = plan.vertexes();
        Ok(Node {
            plan,
            vertexes,
            used_edges,
            cumulative_cost,
        })
    }

    /// 无可用边时构造笛卡尔积内连接，并用 `factor` 放大代价以抑制滥用。
    pub fn cartesian_join(&self, left: Node, right: Node, factor: f64) -> Result<Node, String> {
        if factor.is_nan() || factor.is_infinite() {
            return Err("invalid cartesian factor".to_string());
        }
        let estimated_rows = left.plan.estimated_rows * right.plan.estimated_rows;
        let cost = left.cumulative_cost + right.cumulative_cost + estimated_rows * factor;
        if cost.is_nan() || cost == f64::NEG_INFINITY || cost < 0.0 {
            return Err("invalid cartesian cost".to_string());
        }
        let mut columns = left.plan.columns.clone();
        columns.extend(&right.plan.columns);
        let plan = PlanNode {
            id: left.plan.id.min(right.plan.id),
            kind: PlanKind::Join {
                join_type: JoinType::Inner,
                equal_conditions: Vec::new(),
                other_conditions: Vec::new(),
                hint: JoinMethodHint::default(),
            },
            children: vec![left.plan, right.plan],
            columns,
            estimated_rows,
            cumulative_cost: cost,
        };
        let mut used_edges = left.used_edges;
        used_edges.extend(right.used_edges);
        let vertexes = plan.vertexes();
        Ok(Node {
            plan,
            vertexes,
            used_edges,
            cumulative_cost: cost,
        })
    }

    /// 是否仍有边未被 `used` 消费（优化结果不完整的信号）。
    pub fn has_remaining_edges(&self, used: &BTreeSet<usize>) -> bool {
        self.edges
            .iter()
            .any(|edge| !edge.conditions.is_empty() && !used.contains(&edge.index))
    }

    /// 子集中是否还有尚未消费、且其 TES 与原始两侧均完整落入子集的真实边。
    pub fn has_remaining_edges_in_subset(
        &self,
        subset: &BTreeSet<usize>,
        used: &BTreeSet<usize>,
    ) -> bool {
        self.edges.iter().any(|edge| {
            if edge.conditions.is_empty()
                || used.contains(&edge.index)
                || !edge.tes.is_subset(subset)
            {
                return false;
            }
            edge.left
                .union(&edge.right)
                .all(|vertex| subset.contains(vertex))
        })
    }
}

/// Compute the total eligibility set for an edge.  A predicate that references
/// only one original side is degenerate; as in Go, both original sides are
/// added so it cannot connect an unrelated subgraph prematurely.
fn edge_tes(edge: &Edge) -> BTreeSet<usize> {
    edge.tes.clone()
}

/// Check the TES, side orientation, and local conflict rules for a candidate.
fn edge_applicable(edge: &Edge, left: &Node, right: &Node) -> bool {
    let union = left
        .vertexes
        .union(&right.vertexes)
        .copied()
        .collect::<BTreeSet<_>>();

    // The common no-predicate edge is hot in the large-scale benchmark.  Its
    // TES is exactly both original sides, so avoid allocating a second set.
    if edge.conditions.is_empty() {
        if !edge.left.is_subset(&union)
            || !edge.right.is_subset(&union)
            || (edge.left.is_disjoint(&left.vertexes) && edge.right.is_disjoint(&left.vertexes))
            || (edge.left.is_disjoint(&right.vertexes) && edge.right.is_disjoint(&right.vertexes))
        {
            return false;
        }
        if edge.join_type != JoinType::Inner
            && (!edge.left.is_subset(&left.vertexes) || !edge.right.is_subset(&right.vertexes))
        {
            return false;
        }
        return edge
            .rules
            .iter()
            .all(|rule| rule.from.is_disjoint(&union) || rule.to.is_subset(&union));
    }
    let tes = edge_tes(edge);
    if !tes.is_subset(&union) || tes.is_disjoint(&left.vertexes) || tes.is_disjoint(&right.vertexes)
    {
        return false;
    }

    let forward = if edge.join_type == JoinType::Inner {
        true
    } else {
        edge.left
            .intersection(&tes)
            .all(|vertex| left.vertexes.contains(vertex))
            && edge
                .right
                .intersection(&tes)
                .all(|vertex| right.vertexes.contains(vertex))
    };
    let reverse = edge.join_type == JoinType::Inner;
    if !(forward || reverse) {
        return false;
    }

    // Go conflict rules use implication semantics: touching `from` requires
    // the complete `to` set to be present in the candidate union.
    edge.rules
        .iter()
        .all(|rule| rule.from.is_disjoint(&union) || rule.to.is_subset(&union))
}

fn right_to_left_rule(child: &Edge) -> Rule {
    let to = if !child.left.is_disjoint(&child.tes) {
        child.left.intersection(&child.tes).copied().collect()
    } else {
        child.left.clone()
    };
    Rule {
        from: child.right.clone(),
        to,
    }
}

fn left_to_right_rule(child: &Edge) -> Rule {
    let to = if !child.right.is_disjoint(&child.tes) {
        child.right.intersection(&child.tes).copied().collect()
    } else {
        child.right.clone()
    };
    Rule {
        from: child.left.clone(),
        to,
    }
}

fn join_type_index(join_type: JoinType) -> usize {
    match join_type {
        JoinType::Inner => 0,
        JoinType::LeftOuter => 1,
        JoinType::RightOuter | JoinType::FullOuter => 2,
        JoinType::Semi => 3,
        JoinType::AntiSemi => 4,
    }
}

fn assoc(left: JoinType, right: JoinType) -> bool {
    ASSOC_RULE_TABLE[join_type_index(left)][join_type_index(right)] == 1
}

fn left_asscom(left: JoinType, right: JoinType) -> bool {
    LEFT_ASSCOM_RULE_TABLE[join_type_index(left)][join_type_index(right)] == 1
}

fn right_asscom(left: JoinType, right: JoinType) -> bool {
    RIGHT_ASSCOM_RULE_TABLE[join_type_index(left)][join_type_index(right)] == 1
}

/// 结合律/交换律规则表条目：0 需生成冲突规则，1 表示变换恒成立。
type RuleTableEntry = u8;
/// 结合律合法性查找表。
pub const ASSOC_RULE_TABLE: [[RuleTableEntry; 5]; 5] = [
    [1, 1, 0, 1, 1],
    [0, 1, 0, 0, 0],
    [1, 1, 1, 1, 1],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
];
/// 左结合交换（left assoc/comm）合法性查找表，行/列为连接类型编码。
pub const LEFT_ASSCOM_RULE_TABLE: [[RuleTableEntry; 5]; 5] = [
    [1, 1, 0, 1, 1],
    [1, 1, 1, 1, 1],
    [0, 1, 0, 0, 0],
    [1, 1, 0, 1, 1],
    [1, 1, 0, 1, 1],
];
/// 右结合交换（right assoc/comm）合法性查找表。
pub const RIGHT_ASSCOM_RULE_TABLE: [[RuleTableEntry; 5]; 5] = [
    [1, 0, 1, 0, 0],
    [0, 0, 1, 0, 0],
    [1, 1, 1, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
];
