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

// 多表连接顺序（join order）枚举：动态规划与贪心。
//
// 顶点不超过 `dp_threshold` 时用子集 DP（动态规划）求最低累计代价树；
// 否则从多个起点贪心扩展。无可用边时退化为带代价放大因子的笛卡尔积。

// 操作内存逻辑计划与成本；日志和会话 hint 仍是尚未接通的外部依赖。
//
// use std::collections::{HashMap, HashSet};
//
// JoinOrder 是 DP 与贪心枚举器共享的上下文和连接组基类。
// pub struct JoinOrder { pub ctx: base::PlanContext, pub group: joinGroup }
//
// joinGroup 是一次连接重排的边界，叶节点可各自包含不会跨边界重排的子树。
// pub struct joinGroup {
//     pub root: base::LogicalPlan,
//     pub vertexes: Vec<base::LogicalPlan>,
//     pub leadingHints: Vec<hint::PlanHints>,
//     pub hasUserLeadingHint: bool,
//     pub vertexHints: HashMap<i32, JoinMethodHint>,
//     pub allInnerJoin: bool,
//     pub selConds: HashMap<i32, Vec<expression::Expression>>,
// }
//
// impl joinGroup {
// merge 对应 Go 的切片追加和 maps.Copy，同时合并全内连接标志。
//     pub fn merge(&mut self, other: joinGroup) {
//         self.vertexes.extend(other.vertexes);
//         self.leadingHints.extend(other.leadingHints);
//         self.hasUserLeadingHint |= other.hasUserLeadingHint;
//         self.vertexHints.extend(other.vertexHints);
//         self.allInnerJoin &= other.allInnerJoin;
//         self.selConds.extend(other.selConds);
//     }
// }
//
// makeSingleGroup 把当前计划整体视作一个不可拆分顶点。
// pub fn makeSingleGroup(plan: base::LogicalPlan) -> joinGroup {
//     joinGroup {
//         root: plan.clone(), vertexes: vec![plan], leadingHints: Vec::new(),
//         hasUserLeadingHint: false, vertexHints: HashMap::new(), allInnerJoin: true,
//         selConds: HashMap::new(),
//     }
// }
//
// extractJoinGroup 递归展平允许重排的 Join/Selection，并在边界处退回单顶点组。
// pub fn extractJoinGroup(plan: base::LogicalPlan) -> joinGroup {
//     if let Some(sel) = plan.as_logical_selection() {
//         let vars = plan.SCtx().GetSessionVars();
//         if vars.TiDBOptJoinReorderThroughSel
//             && !sel.Conditions.iter().any(expression::is_mutable_effects_expr)
//         {
//             let mut child = extractJoinGroup(sel.Children()[0].clone());
// Selection 下面必须已有连接，否则没有可供冲突规则约束的连接边。
//             if child.vertexes.len() > 1 {
//                 child.selConds.insert(sel.ID(), sel.Conditions.clone());
//                 child.root = plan;
//                 return child;
//             }
//         }
//         return makeSingleGroup(plan);
//     }
//     let Some(join) = plan.as_logical_join() else { return makeSingleGroup(plan); };
//     let currentLeading = if join.PreferJoinOrder { join.HintInfo.clone() }
//         else if join.InternalPreferJoinOrder { join.InternalHintInfo.clone() } else { None };
//     if join.StraightJoin
//         || !matches!(join.JoinType, base::InnerJoin | base::LeftOuterJoin | base::RightOuterJoin)
//         || (!plan.SCtx().GetSessionVars().EnableOuterJoinReorder && join.JoinType != base::InnerJoin)
//         || (join.PreferJoinType > 0 && !plan.SCtx().GetSessionVars().EnableAdvancedJoinHint)
//         || join.EqualConditions.iter().any(|expr| expr.FuncName.L == ast::NULL_EQ)
//         || (join.JoinType != base::InnerJoin && join.EqualConditions.is_empty())
//     {
//         let mut group = makeSingleGroup(plan);
//         if let Some(info) = currentLeading { group.leadingHints.push(info); }
//         return group;
//     }
//     let mut result = joinGroup {
//         root: plan.clone(), vertexes: Vec::new(), leadingHints: Vec::new(),
//         hasUserLeadingHint: join.PreferJoinOrder, vertexHints: HashMap::new(),
//         allInnerJoin: join.JoinType == base::InnerJoin, selConds: HashMap::new(),
//     };
//     let advanced = plan.SCtx().GetSessionVars().EnableAdvancedJoinHint;
//     let leftHint = advanced && join.LeftPreferJoinType > 0;
//     let rightHint = advanced && join.RightPreferJoinType > 0;
//     if leftHint { result.vertexHints.insert(join.Children()[0].ID(), JoinMethodHint::new(join.LeftPreferJoinType, join.HintInfo.clone())); }
//     if rightHint { result.vertexHints.insert(join.Children()[1].ID(), JoinMethodHint::new(join.RightPreferJoinType, join.HintInfo.clone())); }
//     let preserveLeft = currentLeading.as_ref().is_some_and(|h| IsDerivedTableInLeadingHint(&join.Children()[0], h));
//     let preserveRight = currentLeading.as_ref().is_some_and(|h| IsDerivedTableInLeadingHint(&join.Children()[1], h));
//     result.merge(if leftHint || preserveLeft { makeSingleGroup(join.Children()[0].clone()) } else { extractJoinGroup(join.Children()[0].clone()) });
//     result.merge(if rightHint || preserveRight { makeSingleGroup(join.Children()[1].clone()) } else { extractJoinGroup(join.Children()[1].clone()) });
//     if let Some(info) = currentLeading { result.leadingHints.push(info); }
//     result
// }
//
// Optimize 是连接重排的公开入口。
// pub fn Optimize(plan: base::LogicalPlan) -> Result<base::LogicalPlan, errors::Error> { optimizeRecursive(plan) }
//
// optimizeRecursive 先优化各顶点子树，再对当前多顶点连接组执行枚举。
// pub fn optimizeRecursive(mut plan: base::LogicalPlan) -> Result<base::LogicalPlan, errors::Error> {
//     if plan.is_nil() || plan.is_logical_cte() { return Ok(plan); }
//     let mut group = extractJoinGroup(plan.clone());
//     if group.vertexes.is_empty() { return Err(errors::Errorf(format!("join group has no vertexes, p: {plan:?}"))); }
//     if group.vertexes.len() == 1 {
//         let mut children = Vec::new();
//         for child in plan.Children() { children.push(optimizeRecursive(child.clone())?); }
//         plan.SetChildren(children);
//         if group.hasUserLeadingHint && !group.leadingHints.is_empty() {
//             plan.SCtx().GetSessionVars().StmtCtx.SetHintWarning("leading hint is inapplicable, check the join type or the join algorithm hint");
//         }
//         return Ok(plan);
//     }
//     let mut replacements = HashMap::new();
//     for vertex in &mut group.vertexes {
//         let oldID = vertex.ID();
//         *vertex = optimizeRecursive(vertex.clone())?;
//         replacements.insert(oldID, vertex.clone());
//     }
//     group.root = replaceJoinGroupVertexes(group.root, &replacements);
//     optimizeForJoinGroup(plan.SCtx(), &mut group)
// }
//
// replaceJoinGroupVertexes 把原树叶替换为已递归优化、可能具有新 ID 的计划节点。
// pub fn replaceJoinGroupVertexes(mut root: base::LogicalPlan, vertexMap: &HashMap<i32, base::LogicalPlan>) -> base::LogicalPlan {
//     if let Some(replacement) = vertexMap.get(&root.ID()) { return replacement.clone(); }
//     let children = root.Children().iter().cloned().map(|c| replaceJoinGroupVertexes(c, vertexMap)).collect();
//     root.SetChildren(children);
//     root
// }
//
// optimizeForJoinGroup 按顶点阈值选择 DP 或贪心，并用 Projection 恢复原列顺序。
// pub fn optimizeForJoinGroup(ctx: base::PlanContext, group: &mut joinGroup) -> Result<base::LogicalPlan, errors::Error> {
//     let original = group.root.Schema().clone();
//     let mut plan = if group.vertexes.len() > ctx.GetSessionVars().TiDBOptJoinReorderThreshold as usize {
//         newJoinOrderGreedy(ctx, group.clone()).optimize()?
//     } else { newJoinOrderDP(ctx, group.clone()).optimize()? };
//     if !plan.Schema().Equal(&original) {
//         plan = logicalop::LogicalProjection::new(expression::column_to_exprs(&original.Columns))
//             .Init(plan.SCtx(), plan.QueryBlockOffset()).with_schema(original).with_child(plan);
//     }
//     Ok(plan)
// }
//
// pub struct joinOrderDP { pub JoinOrder: JoinOrder }
// pub fn newJoinOrderDP(ctx: base::PlanContext, group: joinGroup) -> joinOrderDP {
//     joinOrderDP { JoinOrder: JoinOrder { ctx, group } }
// }
//
// impl joinOrderDP {
//     pub fn optimize(&mut self) -> Result<base::LogicalPlan, errors::Error> {
//         if !self.JoinOrder.group.leadingHints.is_empty() {
//             self.JoinOrder.ctx.GetSessionVars().StmtCtx.SetHintWarning("leading hint is inapplicable for the DP join reorder algorithm");
//         }
//         let mut detector = newConflictDetector(self.JoinOrder.ctx.clone());
//         let nodes = detector.Build(&self.JoinOrder.group)?;
//         let (plan, ok) = self.optimizeWithDetector(&mut detector, nodes)?;
//         if ok { Ok(plan.unwrap()) } else {
//             self.JoinOrder.ctx.GetSessionVars().StmtCtx.SetHintWarning("no valid join order found, the original join order will be used");
//             Ok(self.JoinOrder.group.root.clone())
//         }
//     }
//
// optimizeWithDetector 枚举所有子集二分，保留累计成本最低且冲突规则允许的计划。
//     pub fn optimizeWithDetector(&self, detector: &mut ConflictDetector, nodes: Vec<Node>) -> Result<(Option<base::LogicalPlan>, bool), errors::Error> {
//         if nodes.is_empty() { return Err(errors::New("internal error: join group has no nodes")); }
//         if nodes.len() == 1 { return Ok((Some(nodes[0].p.clone()), true)); }
//         if nodes.len() >= 63 { return Err(errors::Errorf(format!("DP join reorder supports at most 62 nodes, got {}", nodes.len()))); }
//         let fullMask = (1_u64 << nodes.len()) - 1;
//         let mut bestPlan: Vec<Option<Node>> = (0..=fullMask).map(|_| None).collect();
//         for node in &nodes { bestPlan[node.bitSet.GetSmallUInt64()? as usize] = Some(node.clone()); }
//         let factor = self.JoinOrder.ctx.GetSessionVars().CartesianJoinOrderThreshold;
//         for subset in 1..=fullMask {
//             if subset.count_ones() == 1 { continue; }
//             let mut left = (subset - 1) & subset;
//             while left > 0 {
//                 let right = subset ^ left;
//                 if left <= right {
//                     if let (Some(l), Some(r)) = (&bestPlan[left as usize], &bestPlan[right as usize]) {
//                         let (check, candidate) = checkConnectionAndMakeJoin(detector, l.clone(), r.clone(), &self.JoinOrder.group.vertexHints, true)?;
//                         if let (Some(check), Some(mut node)) = (check, candidate) {
//                             if check.NoEQEdge() { node.cumCost = applyCartesianFactor(node.cumCost, factor)?; }
//                             if bestPlan[subset as usize].as_ref().is_none_or(|old| node.cumCost < old.cumCost) {
//                                 bestPlan[subset as usize] = Some(node);
//                             }
//                         }
//                     }
//                 }
//                 left = (left - 1) & subset;
//             }
//         }
//         if let Some(finalPlan) = &bestPlan[fullMask as usize] {
//             if finalPlan.cumCost.is_finite() && !detector.HasRemainingEdges(&finalPlan.usedEdges) {
//                 return Ok((Some(finalPlan.p.clone()), true));
//             }
//         }
//         let bushy = buildBushyTreeFromDP(&self.JoinOrder.ctx, detector, &nodes, &bestPlan, &self.JoinOrder.group.vertexHints)?;
//         if let Some(plan) = bushy { if !detector.HasRemainingEdges(&plan.usedEdges) { return Ok((Some(plan.p), true)); } }
//         if let Some(finalPlan) = &bestPlan[fullMask as usize] {
//             if finalPlan.cumCost == f64::INFINITY && !detector.HasRemainingEdges(&finalPlan.usedEdges) {
//                 return Ok((Some(finalPlan.p.clone()), true));
//             }
//         }
//         Ok((None, false))
//     }
// }
//
// pub struct joinOrderGreedy { pub JoinOrder: JoinOrder }
// pub fn newJoinOrderGreedy(ctx: base::PlanContext, group: joinGroup) -> joinOrderGreedy {
//     joinOrderGreedy { JoinOrder: JoinOrder { ctx, group } }
// }
//
// impl joinOrderGreedy {
// buildJoinByHint 通过通用 LeadingTree 构造器消费匹配顶点；失败时完整返回原节点集。
//     pub fn buildJoinByHint(&self, detector: &mut ConflictDetector, nodes: Vec<Node>) -> Result<(Option<Node>, Vec<Node>), errors::Error> {
//         let (leading, different) = CheckAndGenerateLeadingHint(&self.JoinOrder.group.leadingHints);
//         if different && self.JoinOrder.group.hasUserLeadingHint {
//             self.JoinOrder.ctx.GetSessionVars().StmtCtx.SetHintWarning("We can only use one leading hint at most, when multiple leading hints are used, all leading hints will be invalid");
//         }
//         let Some(list) = leading.and_then(|h| h.LeadingList) else { return Ok((None, nodes)); };
//         let original = nodes.clone();
//         let built = BuildLeadingTreeFromList(&list, nodes,
//             |available, table| FindAndRemovePlanByAstHint(&self.JoinOrder.ctx, available, table, |n: &Node| n.p.clone()),
//             |left, right| {
//                 let (_, node) = checkConnectionAndMakeJoin(detector, left, right, &self.JoinOrder.group.vertexHints, true)?;
//                 Ok(node.map(|n| (n, true)).unwrap_or((Node::default(), false)))
//             }, || {} )?;
//         if built.applied { Ok((Some(built.root), built.remaining)) } else { Ok((None, original)) }
//     }
//
//     pub fn optimize(&mut self) -> Result<base::LogicalPlan, errors::Error> {
//         let mut detector = newConflictDetector(self.JoinOrder.ctx.clone());
//         let nodes = detector.Build(&self.JoinOrder.group)?;
//         let (hintNode, mut nodes) = self.buildJoinByHint(&mut detector, nodes)?;
//         if nodes.is_empty() { return Ok(hintNode.unwrap().p); }
//         nodes.sort_by(|a, b| a.cumCost.total_cmp(&b.cumCost));
//         if let Some(node) = hintNode { nodes.insert(0, node); }
//         let factor = self.JoinOrder.ctx.GetSessionVars().CartesianJoinOrderThreshold;
//         let allowNoEQ = factor > 0.0 && self.JoinOrder.group.allInnerJoin;
//         let result = if nodes.len() < 2 { self.optimizeWithStart(&mut detector, &nodes, 0, factor, allowNoEQ)? }
//             else { chooseBestGreedyStart(2, |idx| self.optimizeWithStart(&mut detector, &nodes, idx, factor, allowNoEQ))?.0 };
//         Ok(result.map_or_else(|| self.JoinOrder.group.root.clone(), |node| node.p))
//     }
//
// optimizeWithStart 执行等值优先和必要的第二轮连接，最后把森林拼成 bushy tree。
//     pub fn optimizeWithStart(&self, detector: &mut ConflictDetector, nodes: &[Node], startIdx: usize, mut factor: f64, allowNoEQ: bool) -> Result<Option<Node>, errors::Error> {
//         let mut work = moveGreedyStartToFront(cloneNodesForGreedyStart(nodes), startIdx);
//         work = greedyConnectJoinNodes(detector, work, &self.JoinOrder.group.vertexHints, factor, allowNoEQ)?;
//         let mut used = collectUsedEdges(&work);
//         if !allowNoEQ && detector.HasRemainingEdges(&used) {
//             factor = factor.max(1.0);
//             work = greedyConnectJoinNodes(detector, work, &self.JoinOrder.group.vertexHints, factor, true)?;
//             used = collectUsedEdges(&work);
//         }
//         if detector.HasRemainingEdges(&used) { return Ok(None); }
//         if work.is_empty() { return Err(errors::New("internal error: bushy join tree nodes is empty")); }
//         makeBushyTree(&self.JoinOrder.ctx, detector, work, &self.JoinOrder.group.vertexHints, false).map(Some)
//     }
// }
//
// checkConnectionAndMakeJoin 统一封装冲突检测、可选笛卡尔回退和实际计划构造。
// pub fn checkConnectionAndMakeJoin(detector: &mut ConflictDetector, left: Node, right: Node, hints: &HashMap<i32, JoinMethodHint>, allowNoEQ: bool) -> Result<(Option<CheckConnectionResult>, Option<Node>), errors::Error> {
//     let mut check = detector.CheckConnection(left.clone(), right.clone())?;
//     if !check.Connected() {
//         if !allowNoEQ { return Ok((None, None)); }
//         let Some(cartesian) = detector.TryCreateCartesianCheckResult(left, right) else { return Ok((None, None)); };
//         check = cartesian;
//     }
//     let node = detector.MakeJoin(check.clone(), hints)?;
//     Ok((Some(check), Some(node)))
// }
//
// pub fn cloneNodeForGreedyStart(node: Option<&Node>) -> Option<Node> { node.cloned() }
// pub fn cloneNodesForGreedyStart(nodes: &[Node]) -> Vec<Node> { nodes.to_vec() }
//
// chooseBestGreedyStart 比较多个起点，仅在成本差超过浮点容差时替换最佳项。
// pub fn chooseBestGreedyStart(mut count: usize, mut runner: impl FnMut(usize) -> Result<Option<Node>, errors::Error>) -> Result<(Option<Node>, isize), errors::Error> {
//     let mut best: Option<Node> = None;
//     let mut bestIdx = -1;
//     for idx in 0..count {
//         if let Some(candidate) = runner(idx)? {
//             if best.as_ref().is_none_or(|b| cumCostSignificantlyLess(candidate.cumCost, b.cumCost)) {
//                 best = Some(candidate); bestIdx = idx as isize;
//             }
//         }
//     }
//     Ok((best, bestIdx))
// }
//
// pub fn cumCostSignificantlyLess(cost: f64, bestCost: f64) -> bool {
//     cost < bestCost && bestCost - cost > 1_f64.max(cost.abs()).max(bestCost.abs()) * 1e-12
// }
//
// pub fn moveGreedyStartToFront(mut nodes: Vec<Node>, startIdx: usize) -> Vec<Node> {
//     if startIdx > 0 && startIdx < nodes.len() { let start = nodes.remove(startIdx); nodes.insert(0, start); }
//     nodes
// }
//
// greedyConnectJoinNodes 每轮为当前节点选择最低成本邻居，直到不能继续推进。
// pub fn greedyConnectJoinNodes(detector: &mut ConflictDetector, mut nodes: Vec<Node>, hints: &HashMap<i32, JoinMethodHint>, factor: f64, allowNoEQ: bool) -> Result<Vec<Node>, errors::Error> {
//     while nodes.len() > 1 {
//         let mut progress = false;
//         let mut current = 0;
//         while current + 1 < nodes.len() {
//             let mut best: Option<(usize, Node)> = None;
//             for idx in current + 1..nodes.len() {
//                 let (check, candidate) = checkConnectionAndMakeJoin(detector, nodes[current].clone(), nodes[idx].clone(), hints, allowNoEQ)?;
//                 let (Some(check), Some(mut candidate)) = (check, candidate) else { continue; };
//                 if check.NoEQEdge() {
//                     if !allowNoEQ { continue; }
//                     candidate.cumCost = applyCartesianFactor(candidate.cumCost, factor)?;
//                 }
//                 if best.as_ref().is_none_or(|(_, old)| candidate.cumCost < old.cumCost) { best = Some((idx, candidate)); }
//             }
//             if let Some((idx, node)) = best { nodes[current] = node; nodes.remove(idx); progress = true; }
//             else { current += 1; }
//         }
//         if !progress { break; }
//     }
//     Ok(nodes)
// }
//
// pub fn collectUsedEdges(nodes: &[Node]) -> HashMap<u64, ()> {
//     nodes.iter().flat_map(|n| n.usedEdges.iter().map(|(k, v)| (*k, *v))).collect()
// }
//
// summarizeEdges 生成有限长度的缺失边诊断和当前节点位集合，供警告日志使用。
// pub fn summarizeEdges(detector: &ConflictDetector, usedEdges: &HashMap<u64, ()>, nodes: &[Node], limit: usize) -> (usize, usize, usize, String, String) {
//     let mut total = 0; let mut used = 0; let mut details = Vec::new();
//     for edge in detector.innerEdges.iter().chain(&detector.nonInnerEdges) {
//         if edge.eqConds.is_empty() && edge.nonEQConds.is_empty() { continue; }
//         total += 1;
//         if usedEdges.contains_key(&edge.idx) { used += 1; }
//         else if details.len() < limit { details.push(format!("{{idx:{} type:{:?} eq:{} nonEq:{}}}", edge.idx, edge.joinType, edge.eqConds.len(), edge.nonEQConds.len())); }
//     }
//     let missing = total - used;
//     if missing > limit { details.push(format!("...(+{} more)", missing - limit)); }
//     (total, used, missing, details.join(", "), nodes.iter().map(|n| n.bitSet.String()).collect::<Vec<_>>().join(","))
// }
//
// pub fn applyCartesianFactor(cost: f64, factor: f64) -> Result<f64, errors::Error> {
//     validateCumCost(cost)?;
//     if factor <= 0.0 { return Ok(f64::INFINITY); }
//     if !factor.is_finite() { return Err(errors::Errorf(format!("invalid cartesian factor: {factor}"))); }
//     let adjusted = cost * factor; validateCumCost(adjusted)?; Ok(adjusted)
// }
//
// pub struct dpSubsetCandidate { pub mask: u64, pub node: Node }
//
// buildBushyTreeFromDP 选取最大且成本最低的不相交完整子集，再补齐未覆盖叶节点。
// pub fn buildBushyTreeFromDP(ctx: &base::PlanContext, detector: &mut ConflictDetector, leaves: &[Node], bestPlan: &[Option<Node>], hints: &HashMap<i32, JoinMethodHint>) -> Result<Option<Node>, errors::Error> {
//     let mut candidates: Vec<_> = bestPlan.iter().enumerate().filter_map(|(mask, node)| node.as_ref().filter(|n| n.cumCost.is_finite() && !detector.HasRemainingEdgesInSubset(&n.bitSet, &n.usedEdges)).map(|n| dpSubsetCandidate { mask: mask as u64, node: n.clone() })).collect();
//     candidates.sort_by(|a, b| b.mask.count_ones().cmp(&a.mask.count_ones()).then_with(|| a.node.cumCost.total_cmp(&b.node.cumCost)).then(a.mask.cmp(&b.mask)));
//     let mut forest = Vec::new(); let mut covered = intset::FastIntSet::default();
//     for candidate in candidates { if !candidate.node.bitSet.Intersects(&covered) { covered.UnionWith(&candidate.node.bitSet); forest.push(candidate.node); } }
//     for leaf in leaves { if !leaf.bitSet.Intersects(&covered) { covered.UnionWith(&leaf.bitSet); forest.push(leaf.clone()); } }
//     if forest.is_empty() { Ok(None) } else if forest.len() == 1 { Ok(forest.pop()) }
//     else { makeBushyTree(ctx, detector, forest, hints, false).map(Some) }
// }
//
// pub fn makeJoinWithDetector(detector: &mut ConflictDetector, left: Node, right: Node, hints: &HashMap<i32, JoinMethodHint>) -> Result<Node, errors::Error> {
//     let mut check = detector.CheckConnection(left.clone(), right.clone())?;
//     if !check.Connected() { check = detector.TryCreateCartesianCheckResult(left, right).ok_or_else(|| errors::New("failed to construct bushy tree: no valid join edge found"))?; }
//     detector.MakeJoin(check, hints)
// }
//
// makeBushyTree 两两合并森林；fastPath 只构造最终笛卡尔计划，普通路径保留完整 Node 元数据。
// pub fn makeBushyTree(ctx: &base::PlanContext, detector: &mut ConflictDetector, mut nodes: Vec<Node>, hints: &HashMap<i32, JoinMethodHint>, fastPath: bool) -> Result<Node, errors::Error> {
//     while nodes.len() > 1 {
//         let mut next = Vec::new();
//         let mut iter = nodes.into_iter();
//         while let Some(left) = iter.next() {
//             let Some(right) = iter.next() else { next.push(left); break; };
//             let joined = if fastPath {
//                 Node { p: newCartesianJoin(ctx, base::InnerJoin, left.p, right.p, hints)?.into(), ..Node::default() }
//             } else { makeJoinWithDetector(detector, left, right, hints)? };
//             next.push(joined);
//         }
//         nodes = next;
//     }
//     nodes.pop().ok_or_else(|| errors::New("cannot build bushy tree from empty node list"))
// }
// */
use crate::conflict_detector::{ConflictDetector, Node};
use crate::util::{JoinMethodHint, PlanNode};
use std::collections::{BTreeMap, BTreeSet};

/// 连接重排优化器：按顶点数在 DP 与贪心之间切换。
pub struct JoinOrder {
    /// 顶点数 ≤ 此阈值走 DP，否则走贪心（对应 TiDB `TiDBOptJoinReorderThreshold`）。
    pub dp_threshold: usize,
    /// 笛卡尔积代价放大因子，抑制无等值边回退。
    pub cartesian_factor: f64,
    /// 顶点级连接方法 hint（如 hash/merge join 偏好）。
    pub vertex_hints: BTreeMap<usize, JoinMethodHint>,
}
impl Default for JoinOrder {
    fn default() -> Self {
        Self {
            dp_threshold: 10,
            cartesian_factor: 10_000.0,
            vertex_hints: BTreeMap::new(),
        }
    }
}

impl JoinOrder {
    /// 优化入口：构图后枚举，并要求所有冲突边被完整消费。
    pub fn optimize(&self, root: PlanNode) -> Result<PlanNode, String> {
        let (detector, nodes) = ConflictDetector::build(&root)?;
        if nodes.len() <= 1 {
            return Ok(root);
        }
        // 小规模精确 DP，大规模贪心近似。
        let result = if nodes.len() <= self.dp_threshold {
            self.optimize_dp(&detector, &nodes)?
        } else {
            self.optimize_greedy(&detector, &nodes)?
        };
        if detector.has_remaining_edges(&result.used_edges) {
            return Err("optimized join tree left conflict edges unused".to_string());
        }
        Ok(result.plan)
    }

    /// 子集动态规划：对每个非空子集保留累计代价最低的合法连接树。
    fn optimize_dp(&self, detector: &ConflictDetector, leaves: &[Node]) -> Result<Node, String> {
        if leaves.len() >= usize::BITS as usize {
            return Err("too many join vertices for DP".to_string());
        }
        let full = (1usize << leaves.len()) - 1;
        // best[mask] = 覆盖 mask 中顶点的最优中间结果。
        let mut best: BTreeMap<usize, Node> = leaves
            .iter()
            .enumerate()
            .map(|(index, node)| (1usize << index, node.clone()))
            .collect();
        for size in 2..=leaves.len() {
            for subset in 1usize..=full {
                if subset.count_ones() as usize != size {
                    continue;
                }
                let mut candidate: Option<Node> = None;
                // 固定含最低位的二分，避免左右重复枚举。
                let first_bit = subset & subset.wrapping_neg();
                let mut left_mask = (subset - 1) & subset;
                while left_mask > 0 {
                    if left_mask & first_bit == 0 {
                        left_mask = (left_mask - 1) & subset;
                        continue;
                    }
                    let right_mask = subset ^ left_mask;
                    if let (Some(left), Some(right)) = (best.get(&left_mask), best.get(&right_mask))
                    {
                        let check = detector.check_connection(left, right)?;
                        if check.connected() {
                            let no_equality = check.no_equality_edge();
                            let mut node = detector.make_join(check, &self.vertex_hints)?;
                            if no_equality {
                                node.cumulative_cost = apply_cartesian_factor(
                                    node.cumulative_cost,
                                    self.cartesian_factor,
                                )?;
                                node.plan.cumulative_cost = node.cumulative_cost;
                            }
                            if candidate.as_ref().is_none_or(|old| {
                                significantly_less(node.cumulative_cost, old.cumulative_cost)
                            }) {
                                candidate = Some(node);
                            }
                        }
                    }
                    left_mask = (left_mask - 1) & subset;
                }
                if let Some(candidate) = candidate {
                    best.insert(subset, candidate);
                }
            }
        }
        if let Some(node) = best.remove(&full) {
            return Ok(node);
        }
        // DP 未找到合法全覆盖方案时，退化为灌木状笛卡尔积树。
        self.make_bushy_cartesian(detector, leaves.to_vec())
    }

    /// 贪心：最多尝试两个起点，先尽量消费真实边，再按 Go 的规则回退到笛卡尔积。
    fn optimize_greedy(
        &self,
        detector: &ConflictDetector,
        leaves: &[Node],
    ) -> Result<Node, String> {
        // The Go implementation compares two starts; keeping the same bound
        // prevents the quadratic plan-cloning work from dominating large joins.
        let starts = leaves.len().min(2);
        let all_inner = detector
            .edges
            .iter()
            .all(|edge| edge.join_type == crate::util::JoinType::Inner);
        let mut best = None;
        for start in 0..starts {
            let mut nodes = leaves.to_vec();
            let start_node = nodes.remove(start);
            nodes.insert(0, start_node);
            let allow_no_equality = all_inner && self.cartesian_factor > 0.0;
            nodes = greedy_connect(
                detector,
                nodes,
                &self.vertex_hints,
                self.cartesian_factor,
                allow_no_equality,
                all_inner,
            )?;
            if !allow_no_equality && detector.has_remaining_edges(&collect_used_edges(&nodes)) {
                nodes = greedy_connect(
                    detector,
                    nodes,
                    &self.vertex_hints,
                    self.cartesian_factor.max(1.0),
                    true,
                    all_inner,
                )?;
            }
            if detector.has_remaining_edges(&collect_used_edges(&nodes)) {
                continue;
            }
            let current = self.make_bushy_cartesian(detector, nodes)?;
            if best.as_ref().is_none_or(|old: &Node| {
                significantly_less(current.cumulative_cost, old.cumulative_cost)
            }) {
                best = Some(current);
            }
        }
        best.ok_or_else(|| "cannot optimize empty join group".to_string())
    }

    /// 按 Go 森林顺序逐轮两两合并，生成灌木状（bushy）回退树。
    pub(crate) fn make_bushy_cartesian(
        &self,
        detector: &ConflictDetector,
        mut nodes: Vec<Node>,
    ) -> Result<Node, String> {
        while nodes.len() > 1 {
            let mut next = Vec::with_capacity(nodes.len().div_ceil(2));
            let mut iter = nodes.into_iter();
            while let Some(left) = iter.next() {
                let Some(right) = iter.next() else {
                    next.push(left);
                    break;
                };
                let check = detector.check_connection(&left, &right)?;
                let node = if check.connected() {
                    let no_equality = check.no_equality_edge();
                    let mut node = detector.make_join(check, &self.vertex_hints)?;
                    if no_equality {
                        node.cumulative_cost = apply_cartesian_factor(
                            node.cumulative_cost,
                            self.cartesian_factor.max(1.0),
                        )?;
                        node.plan.cumulative_cost = node.cumulative_cost;
                    }
                    node
                } else {
                    make_cartesian_candidate(detector, left, right, self.cartesian_factor.max(1.0))?
                };
                next.push(node);
            }
            nodes = next;
        }
        nodes
            .pop()
            .ok_or_else(|| "cannot build bushy tree from empty node list".to_string())
    }
}

/// 浮点代价比较：绝对差与相对差双阈值，避免噪声导致抖动。
fn significantly_less(cost: f64, best: f64) -> bool {
    cost < best && best - cost > cost.abs().max(best.abs()).max(1.0) * 1e-12
}

fn apply_cartesian_factor(cost: f64, factor: f64) -> Result<f64, String> {
    if cost.is_nan() || cost == f64::NEG_INFINITY || cost < 0.0 {
        return Err("invalid cumulative cost".to_string());
    }
    if factor.is_nan() || factor.is_infinite() {
        return Err("invalid cartesian factor".to_string());
    }
    if factor <= 0.0 {
        return Ok(f64::INFINITY);
    }
    let adjusted = cost * factor;
    if adjusted.is_nan() || adjusted == f64::NEG_INFINITY || adjusted < 0.0 {
        return Err("invalid cumulative cost".to_string());
    }
    Ok(adjusted)
}

fn collect_used_edges(nodes: &[Node]) -> BTreeSet<usize> {
    nodes
        .iter()
        .flat_map(|node| node.used_edges.iter().copied())
        .collect()
}

fn greedy_connect(
    detector: &ConflictDetector,
    mut nodes: Vec<Node>,
    hints: &BTreeMap<usize, JoinMethodHint>,
    factor: f64,
    allow_no_equality: bool,
    allow_cartesian: bool,
) -> Result<Vec<Node>, String> {
    while nodes.len() > 1 {
        let mut progress = false;
        let mut current_index = 0;
        while current_index + 1 < nodes.len() {
            let mut choice: Option<(usize, Node)> = None;
            for candidate_index in current_index + 1..nodes.len() {
                let left = &nodes[current_index];
                let right = &nodes[candidate_index];
                let check = detector.check_connection(left, right)?;
                let candidate = if check.connected() {
                    let no_equality = check.no_equality_edge();
                    if no_equality && !allow_no_equality {
                        None
                    } else {
                        let mut candidate = detector.make_join(check, hints)?;
                        if no_equality {
                            candidate.cumulative_cost =
                                apply_cartesian_factor(candidate.cumulative_cost, factor)?;
                            candidate.plan.cumulative_cost = candidate.cumulative_cost;
                        }
                        Some(candidate)
                    }
                } else if allow_cartesian {
                    Some(make_cartesian_candidate(
                        detector,
                        left.clone(),
                        right.clone(),
                        factor,
                    )?)
                } else {
                    None
                };
                let Some(candidate) = candidate else {
                    continue;
                };
                if choice.as_ref().is_none_or(|(_, old)| {
                    significantly_less(candidate.cumulative_cost, old.cumulative_cost)
                }) {
                    choice = Some((candidate_index, candidate));
                }
            }
            if let Some((candidate_index, candidate)) = choice {
                nodes[current_index] = candidate;
                nodes.remove(candidate_index);
                progress = true;
            } else {
                current_index += 1;
            }
        }
        if !progress {
            break;
        }
    }
    Ok(nodes)
}

fn make_cartesian_candidate(
    detector: &ConflictDetector,
    left: Node,
    right: Node,
    factor: f64,
) -> Result<Node, String> {
    let mut node = detector.cartesian_join(left, right, 1.0)?;
    node.cumulative_cost = apply_cartesian_factor(node.cumulative_cost, factor)?;
    node.plan.cumulative_cost = node.cumulative_cost;
    Ok(node)
}
