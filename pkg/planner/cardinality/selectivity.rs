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

// 谓词选择率（selectivity）估算。
//
// 选择率是满足谓词的行占比（0~1）。本模块组合列/索引统计、范围提取、
// TopN 辅助评估与默认选择率，供代价模型估算过滤后行数；含贪心覆盖、
// DNF（析取范式）递归与 MV Index（多值索引）路径。

use crate::*;

// 优化器组合列/索引统计、范围提取、TopN 辅助评估和默认选择率。
// cmp、maps、slices、expression、statistics、ranger、chunk、codec、collate 等跨包依赖保留为后续模块接线占位。

/// out-of-range 等值估算中 NDV 的下限，避免分母过小导致选择率虚高。
pub static mut outOfRangeBetweenRate: i64 = 100;

/// Selectivity 对应 Go 的主入口：计算 CNF 表达式在 HistColl 上的选择率。
/// 该函数按原有顺序处理相关列、列统计、索引统计、贪心覆盖和未覆盖谓词默认回退。
pub fn Selectivity(
    ctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    exprs: &[expression::ExprBox],
    filledPaths: &[&planutil::AccessPath],
) -> Result<f64, errors::Error> {
    // 空表或空条件等价于 100% 选择率。
    if coll.RealtimeCount == 0 || exprs.is_empty() {
        return Ok(1.0);
    }
    let mut ret = 1.0;
    let sc = ctx.GetExprCtx().GetEvalCtx();
    let tableID = coll.PhysicalID;
    if exprs.len() > 63 || (coll.ColNum() == 0 && coll.IdxNum() == 0) {
        ret = pseudoSelectivity(ctx, coll, exprs);
        ctx.GetSessionVars()
            .RecordRelevantOptVar(vardef::TiDBOptSelectivityFactor);
        return Ok(ret);
    }

    let mut nodes: Vec<StatsNode> = Vec::new();
    let mut remainedExprs = Vec::<expression::ExprBox>::with_capacity(exprs.len());
    // Go 先单独处理 col = correlated_col，避免它进入 ranger 范围提取。
    for expr in exprs {
        let c = isColEqCorCol(expr.as_ref());
        if c.is_none() {
            remainedExprs.push(expr.clone());
            continue;
        }
        let c = c.unwrap();
        let colHist = coll.GetCol(c.UniqueID);
        let sel = if statistics::ColumnStatsIsInvalid(colHist, coll.Pseudo) {
            1.0 / pseudoEqualRate
        } else if colHist.unwrap().Histogram.NDV > 0 {
            1.0 / colHist.unwrap().Histogram.NDV as f64
        } else {
            1.0 / pseudoEqualRate
        };
        ret *= sel;
    }

    let mut extractedCols = expression::ExtractColumnsMapFromExpressions(|_| true, &remainedExprs)
        .into_values()
        .collect::<Vec<_>>();
    extractedCols.sort_by_key(|c| c.ID);
    extractedCols.dedup_by_key(|c| c.ID);
    for col in &extractedCols {
        if col.IsHidden && col.VirtualExpr.is_some() {
            // 表达式索引只使用索引统计，不使用隐藏虚拟列的列统计。
            continue;
        }
        let id = col.UniqueID;
        let colStats = coll.GetCol(id);
        if colStats.is_some() {
            let (maskCovered, ranges, _, _, _) = getMaskAndRanges(
                ctx,
                &remainedExprs,
                ranger::ColumnRangeType,
                &[],
                None,
                std::slice::from_ref(col),
            )?;
            let mut node = StatsNode {
                Tp: ColType,
                ID: id,
                mask: maskCovered,
                Ranges: ranges.clone(),
                numCols: 1,
                ..Default::default()
            };
            let cntEst = if colStats.unwrap().IsHandle {
                node.Tp = PkType;
                let range_refs = ranges.iter().collect::<Vec<_>>();
                GetRowCountByColumnRanges(ctx, coll, id, &range_refs, true)?
            } else {
                let range_refs = ranges.iter().collect::<Vec<_>>();
                GetRowCountByColumnRanges(ctx, coll, id, &range_refs, false)?
            };
            node.Selectivity = cntEst.Est / coll.RealtimeCount as f64;
            nodes.push(node);
        } else if !col.IsHidden {
            // 兼容异步统计加载：没有列统计时仍记录 used stats 状态。
            statistics::ColumnStatsIsInvalid(None, coll.Pseudo);
            recordUsedItemStatsStatus(ctx, UsedStatsItem::Column(None), tableID, col.ID);
        }
    }

    let mut id2Paths = std::collections::HashMap::<i64, &planutil::AccessPath>::new();
    for path in filledPaths {
        if path.Index.is_some() {
            id2Paths.insert(path.Index.as_ref().unwrap().ID, *path);
        }
    }
    let mut idxIDs = Vec::<i64>::with_capacity(coll.IdxNum());
    coll.ForEachIndexImmutable(|id, _| {
        idxIDs.push(id);
        false
    });
    idxIDs.sort();
    for id in idxIDs {
        let idxStats = coll
            .GetIdx(id)
            .expect("ForEachIndexImmutable 返回的索引应存在");
        let idxInfo = idxStats.InfoRef();
        if idxInfo.MVIndex {
            let (totalSelectivity, mask, ok) =
                getMaskAndSelectivityForMVIndex(ctx, coll, id, &remainedExprs);
            if !ok {
                continue;
            }
            nodes.push(StatsNode {
                Tp: IndexType,
                ID: id,
                mask,
                numCols: idxInfo.Columns.len(),
                Selectivity: totalSelectivity,
                ..Default::default()
            });
            continue;
        }
        let idxCols = findPrefixOfIndexByCol(
            ctx,
            &extractedCols,
            &coll.Idx2ColUniqueIDs[&id],
            id2Paths.get(&idxStats.ID).copied(),
        );
        if !idxCols.is_empty() {
            let mut lengths = Vec::<i32>::with_capacity(idxCols.len());
            for i in 0..idxCols.len().min(idxStats.InfoRef().Columns.len()) {
                lengths.push(idxStats.InfoRef().Columns[i].Length);
            }
            if idxCols.len() > idxStats.InfoRef().Columns.len() {
                lengths.push(types::UnspecifiedLength);
            }
            let (maskCovered, ranges, partCover, minAccessCondsForDNFCond, _) = getMaskAndRanges(
                ctx,
                &remainedExprs,
                ranger::IndexRangeType,
                &lengths,
                id2Paths.get(&idxStats.ID).copied(),
                &idxCols,
            )?;
            let range_refs = ranges.iter().collect::<Vec<_>>();
            let idx_col_refs = idxCols.iter().collect::<Vec<_>>();
            let mut countResult =
                GetRowCountByIndexRanges(ctx, coll, id, &range_refs, &idx_col_refs)?;
            countResult.DivideAll(coll.RealtimeCount as f64);
            nodes.push(StatsNode {
                Tp: IndexType,
                ID: id,
                mask: maskCovered,
                Ranges: ranges,
                numCols: idxStats.InfoRef().Columns.len(),
                Selectivity: countResult.Est,
                MinSelectivity: countResult.MinEst,
                MaxSelectivity: countResult.MaxEst,
                partCover,
                minAccessCondsForDNFCond,
            });
        }
    }

    let usedSets = GetUsableSetsByGreedy(&mut nodes);
    let mut mask = (1_i64 << remainedExprs.len() as u64) - 1;
    for set in &usedSets {
        mask &= !set.mask;
        ret *= set.Selectivity;
        if set.partCover {
            // DNF 仅部分转为 access condition 时，剩余部分按默认选择率补乘。
            ret *= ctx.GetSessionVars().SelectivityFactor;
        }
    }

    let mut notCoveredConstants = std::collections::HashMap::<usize, expression::Constant>::new();
    let mut notCoveredDNF = std::collections::HashMap::<usize, &expression::ScalarFunction>::new();
    let mut notCoveredStrMatch = std::collections::HashMap::<usize, expression::ExprBox>::new();
    let mut notCoveredNegateStrMatch =
        std::collections::HashMap::<usize, expression::ExprBox>::new();
    let mut notCoveredOtherExpr = std::collections::HashMap::<usize, expression::ExprBox>::new();
    if mask > 0 {
        for (i, expr) in remainedExprs.iter().enumerate() {
            if mask & (1_i64 << i as u64) == 0 {
                continue;
            }
            if let Some(c) = expr.as_constant() {
                notCoveredConstants.insert(i, c.clone());
                continue;
            }
            if let Some(x) = expr.as_scalar_function() {
                match x.FuncName.L.as_str() {
                    ast::LogicOr => {
                        notCoveredDNF.insert(i, x);
                        continue;
                    }
                    ast::Like | ast::Ilike | ast::Regexp | ast::RegexpLike => {
                        notCoveredStrMatch.insert(i, expr.clone());
                        continue;
                    }
                    ast::FTSMysqlMatchAgainst => {
                        // MATCH AGAINST 不能直接 EvalReal；Go 尝试改写为 ILIKE 后再走 TopN 辅助估算。
                        if let Ok(substitute) =
                            expression::BuildFTSToILikeExpressionFromBuiltin(ctx.GetExprCtx(), x)
                        {
                            if let Some(sub) = substitute.as_scalar_function() {
                                notCoveredStrMatch.insert(i, substitute);
                                continue;
                            }
                            if let Some(sub) = substitute.as_constant() {
                                notCoveredConstants.insert(i, sub.clone());
                                continue;
                            }
                        }
                        notCoveredStrMatch.insert(i, expr.clone());
                        continue;
                    }
                    ast::UnaryNot => {
                        let inner = expression::GetExprInsideIsTruth(x.GetArgs()[0].clone());
                        if let Some(innerSF) = inner.as_scalar_function() {
                            match innerSF.FuncName.L.as_str() {
                                ast::Like | ast::Ilike | ast::Regexp | ast::RegexpLike => {
                                    notCoveredNegateStrMatch.insert(i, expr.clone());
                                    continue;
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            notCoveredOtherExpr.insert(i, expr.clone());
        }
    }

    // 常量表达式可直接判断 true/false/null，false/null 让总选择率归零。
    let constantKeys = notCoveredConstants.keys().copied().collect::<Vec<_>>();
    for i in constantKeys {
        let c = notCoveredConstants.get(&i).unwrap();
        if expression::MaybeOverOptimized4PlanCache(ctx.GetExprCtx(), c) {
            continue;
        }
        if c.Value.IsNull() {
            ret *= 0.0;
            mask &= !(1_i64 << i as u64);
            notCoveredConstants.remove(&i);
        } else if let Ok(isTrue) = c.Value.ToBool(sc.TypeCtx()) {
            if isTrue == 0 {
                ret *= 0.0;
            }
            mask &= !(1_i64 << i as u64);
            notCoveredConstants.remove(&i);
        }
    }

    // 未覆盖 DNF 用独立性公式 sel(A or B)=sel(A)+sel(B)-sel(A)*sel(B) 递归估算。
    let dnfKeys = notCoveredDNF.keys().copied().collect::<Vec<_>>();
    'outer: for i in dnfKeys {
        let scalarCond = notCoveredDNF.get(&i).unwrap();
        let cols = expression::ExtractColumns(*scalarCond);
        for col in &cols {
            if coll.GetCol(col.UniqueID).is_none() {
                continue 'outer;
            }
        }
        let mut dnfItems = expression::FlattenDNFConditions(*scalarCond);
        dnfItems = ranger::MergeDNFItems4Col(ctx.GetRangerCtx(), dnfItems);
        if dnfItems.len() <= 1 {
            continue;
        }
        let mut selectivity = 0.0;
        for cond in dnfItems {
            if cond.as_correlated_column().is_some() {
                continue;
            }
            let cnfItems = if cond
                .as_scalar_function()
                .is_some_and(|sf| sf.FuncName.L == ast::LogicAnd)
            {
                expression::FlattenCNFConditions(cond.as_scalar_function().unwrap())
            } else {
                vec![cond]
            };
            let curSelectivity = match Selectivity(ctx, coll, &cnfItems, &[]) {
                Ok(v) => v,
                Err(err) => {
                    logutil::log::background_logger()
                        .debug(format!("selectivity fallback after error: {err}"));
                    ctx.GetSessionVars().SelectivityFactor
                }
            };
            selectivity = selectivity + curSelectivity - selectivity * curSelectivity;
        }
        if selectivity != 0.0 {
            ret *= selectivity;
            mask &= !(1_i64 << i as u64);
            notCoveredDNF.remove(&i);
        }
    }

    // 字符串匹配可选用 TopN/Histogram 辅助评估；错误只追加 warning，不终止主选择率估算。
    if ctx.GetSessionVars().EnableEvalTopNEstimationForStrMatch() {
        let keys = notCoveredStrMatch.keys().copied().collect::<Vec<_>>();
        for i in keys {
            let scalarCond = notCoveredStrMatch.get(&i).unwrap();
            match GetSelectivityByFilter(ctx, coll, scalarCond.clone()) {
                Ok((true, sel)) => {
                    ret *= sel;
                    mask &= !(1_i64 << i as u64);
                    notCoveredStrMatch.remove(&i);
                }
                Ok((false, _)) => {}
                Err(err) => sc.AppendWarning(expression::contextutil::errors::NewNoStackError(
                    format!("Error when using TopN-assisted estimation: {}", err),
                )),
            }
        }
        let keys = notCoveredNegateStrMatch.keys().copied().collect::<Vec<_>>();
        for i in keys {
            let scalarCond = notCoveredNegateStrMatch.get(&i).unwrap();
            match GetSelectivityByFilter(ctx, coll, scalarCond.clone()) {
                Ok((true, sel)) => {
                    ret *= sel;
                    mask &= !(1_i64 << i as u64);
                    notCoveredNegateStrMatch.remove(&i);
                }
                Ok((false, _)) => {}
                Err(err) => sc.AppendWarning(expression::contextutil::errors::NewNoStackError(
                    format!("Error when using TopN-assisted estimation: {}", err),
                )),
            }
        }
    }

    if mask > 0 {
        let mut minSelectivity: f64 = 1.0;
        if !notCoveredConstants.is_empty()
            || !notCoveredDNF.is_empty()
            || !notCoveredOtherExpr.is_empty()
        {
            minSelectivity = minSelectivity.min(ctx.GetSessionVars().SelectivityFactor);
        }
        if !notCoveredStrMatch.is_empty() {
            minSelectivity =
                minSelectivity.min(ctx.GetSessionVars().GetStrMatchDefaultSelectivity());
        }
        if !notCoveredNegateStrMatch.is_empty() {
            minSelectivity =
                minSelectivity.min(ctx.GetSessionVars().GetNegateStrMatchDefaultSelectivity());
        }
        ret *= minSelectivity;
        ctx.GetSessionVars()
            .RecordRelevantOptVar(vardef::TiDBOptSelectivityFactor);
    }
    Ok(ret.max(1.0 / coll.RealtimeCount as f64))
}

/// 将统计过滤表达式中的实际列统一到单列 chunk 的索引 0。
///
/// Go 直接修改表达式树中的列并在函数返回前恢复；Rust 的列提取 API
/// 返回副本，因此必须递归修改传入的表达式副本，修改提取出的列副本并
/// 不会影响 VectorizedFilter 的实际求值对象。
pub(crate) fn prepareFilterForStatsEvaluation(expr: &mut dyn expression::Expression) {
    if let Some(column) = expr.as_any_mut().downcast_mut::<expression::Column>() {
        column.Index = 0;
        return;
    }
    if let Some(function) = expr
        .as_any_mut()
        .downcast_mut::<expression::ScalarFunction>()
    {
        for argument in function.GetArgsMut() {
            prepareFilterForStatsEvaluation(argument.as_mut());
        }
    }
}

/// CalcTotalSelectivityForMVIdxPath 对应 Go 的 MV index partial path 总选择率估算。
/// intersection 使用乘法，union 使用容斥公式；单个 partial path 的分母按是否触及虚拟列选择表或索引行数。
pub fn CalcTotalSelectivityForMVIdxPath(
    coll: &statistics::HistColl,
    partialPaths: &[&planutil::AccessPath],
    isIntersection: bool,
) -> f64 {
    let mut selectivities = Vec::<f64>::with_capacity(partialPaths.len());
    for path in partialPaths {
        let mut realtimeCount = coll.RealtimeCount;
        if !path.IsTablePath() && path.Index.as_ref().is_some_and(|index| index.MVIndex) {
            let index = path
                .Index
                .as_ref()
                .expect("non-table MV path requires index metadata");
            let mut virtualCol: Option<expression::Column> = None;
            let mut access_columns =
                expression::ExtractColumnsMapFromExpressions(|_| true, &path.AccessConds);
            for col_id in &coll.MVIdx2Columns[&index.ID] {
                if let Some(col) = access_columns.remove(col_id)
                    && col.VirtualExpr.is_some()
                {
                    virtualCol = Some(col);
                    break;
                }
            }
            let cols = expression::ExtractColumnsMapFromExpressions(|_| true, &path.AccessConds);
            if !virtualCol
                .as_ref()
                .is_some_and(|v| cols.contains_key(&v.UniqueID))
            {
                let (cnt, _) = coll.GetScaledRealtimeAndModifyCnt(coll.GetIdx(index.ID).unwrap());
                realtimeCount = cnt;
            }
        }
        let sel = mathutil::Clamp(path.CountAfterAccess / realtimeCount as f64, 0.0, 1.0);
        selectivities.push(sel);
    }
    if isIntersection {
        selectivities.into_iter().fold(1.0, |acc, sel| acc * sel)
    } else {
        selectivities
            .into_iter()
            .fold(0.0, |acc, sel| (sel + acc) - acc * sel)
    }
}

/// StatsNode 对应 Go struct，用于贪心选择覆盖谓词的列统计或索引统计。
#[derive(Clone, Default)]
pub struct StatsNode {
    pub Ranges: ranger::Ranges,
    pub Tp: i32,
    pub ID: i64,
    pub mask: i64,
    pub Selectivity: f64,
    pub MinSelectivity: f64,
    pub MaxSelectivity: f64,
    pub numCols: usize,
    pub partCover: bool,
    pub minAccessCondsForDNFCond: i32,
}

/// 统计节点类型：索引统计。
pub const IndexType: i32 = 0;
/// 统计节点类型：主键 handle 列。
pub const PkType: i32 = 1;
/// 统计节点类型：普通列统计。
pub const ColType: i32 = 2;

/// compareType 保留 Go 对 StatsNode 类型的排序优先级：列、索引/主键之间按原逻辑稳定排序。
pub fn compareType(l: i32, r: i32) -> i32 {
    if l == r {
        return 0;
    }
    if l == ColType {
        return -1;
    }
    if l == PkType {
        return 1;
    }
    if r == ColType {
        return 1;
    }
    -1
}

/// 无法识别列 ID 时的哨兵值。
pub const unknownColumnID: i64 = i64::MIN;
/// 最后桶末值 Repeat 相对平均行数过低时判定为“可疑低估”的阈值。
pub const staleLastBucketThreshold: f64 = 0.3;
/// 分析后新增行相对平均值达到该倍数时，才启用最后桶末值启发式。
pub const valueAwareRowAddedThreshold: f64 = 0.5;

/// IsLastBucketEndValueUnderrepresented 对应 Go 的最后桶末值低估启发式。
/// 当分析后新增行较多且待估值正好是最后桶上界时，过小 Repeat 会被视为可疑。
pub fn IsLastBucketEndValueUnderrepresented(
    sctx: &dyn planctx::PlanContext,
    hg: &statistics::Histogram,
    val: types::Datum,
    histCnt: f64,
    histNDV: f64,
    realtimeRowCount: i64,
    modifyCount: i64,
) -> bool {
    if modifyCount <= 0 || hg.Buckets.is_empty() || histNDV <= 0.0 {
        return false;
    }
    let newRowsAdded = hg.AbsRowCountDifference(realtimeRowCount);
    let avgValueCount = hg.NotNullCount() / histNDV;
    if newRowsAdded < avgValueCount * valueAwareRowAddedThreshold {
        return false;
    }
    let (_, bucketIdx, inBucket, matchLastValue) = hg.LocateBucket(&val);
    let isLastBucketEndValue = bucketIdx == hg.Buckets.len() - 1 && inBucket && matchLastValue;
    if !isLastBucketEndValue {
        return false;
    }
    histCnt < avgValueCount * staleLastBucketThreshold
}

/// getConstantColumnID 对应 Go 的两参数表达式识别：一侧列、一侧常量时返回列 ID。
pub fn getConstantColumnID(e: &[expression::ExprBox]) -> i64 {
    if e.len() != 2 {
        return unknownColumnID;
    }
    if let (Some(col), Some(_)) = (e[0].as_column(), e[1].as_constant()) {
        return col.ID;
    }
    if let (Some(col), Some(_)) = (e[1].as_column(), e[0].as_constant()) {
        return col.ID;
    }
    unknownColumnID
}

/// GetUsableSetsByGreedy 对应 Go 的贪心集合选择。
/// 每轮选择完整覆盖当前剩余 mask 且更“好”的统计节点，然后从 mask 中删除其覆盖位。
pub fn GetUsableSetsByGreedy(nodes: &mut [StatsNode]) -> Vec<StatsNode> {
    nodes.sort_by(|i, j| {
        let r = compareType(i.Tp, j.Tp);
        if r != 0 {
            return r.cmp(&0);
        }
        i.ID.cmp(&j.ID)
    });
    let mut newBlocks = Vec::<StatsNode>::new();
    let mut marked = vec![false; nodes.len()];
    let mut mask = i64::MAX;
    loop {
        let mut bestMask = 0_i64;
        let mut best = statsNodeForGreedyChoice {
            StatsNode: StatsNode {
                Tp: ColType,
                Selectivity: 0.0,
                numCols: 0,
                partCover: true,
                minAccessCondsForDNFCond: -1,
                ..Default::default()
            },
            idx: -1,
            coverCount: 0,
        };
        for (i, set) in nodes.iter().enumerate() {
            if marked[i] {
                continue;
            }
            let curMask = set.mask & mask;
            if curMask != set.mask {
                marked[i] = true;
                continue;
            }
            let bits = curMask.count_ones() as i32;
            if bits == 0 {
                marked[i] = true;
                continue;
            }
            let current = statsNodeForGreedyChoice {
                StatsNode: set.clone(),
                idx: i as isize,
                coverCount: bits,
            };
            if current.isBetterThan(&best) {
                best = current;
                bestMask = curMask;
            }
        }
        if best.coverCount == 0 {
            break;
        }
        mask &= !bestMask;
        newBlocks.push(nodes[best.idx as usize].clone());
        marked[best.idx as usize] = true;
    }
    newBlocks
}

#[derive(Clone)]
/// 贪心选择过程中的候选包装：附带节点下标与当前覆盖谓词位数。
pub struct statsNodeForGreedyChoice {
    pub StatsNode: StatsNode,
    pub idx: isize,
    pub coverCount: i32,
}

impl statsNodeForGreedyChoice {
    /// isBetterThan 保留 Go 的六条优先级：类型、覆盖数、DNF 完整性、DNF 最少 access 数、列数、选择率。
    pub fn isBetterThan(&self, other: &statsNodeForGreedyChoice) -> bool {
        if self.StatsNode.Tp != ColType && other.StatsNode.Tp == ColType {
            return true;
        }
        if self.coverCount > other.coverCount {
            return true;
        }
        if self.coverCount != other.coverCount {
            return false;
        }
        if !self.StatsNode.partCover && other.StatsNode.partCover {
            return true;
        }
        if self.StatsNode.partCover != other.StatsNode.partCover {
            return false;
        }
        if self.StatsNode.minAccessCondsForDNFCond > other.StatsNode.minAccessCondsForDNFCond {
            return true;
        }
        if self.StatsNode.minAccessCondsForDNFCond != other.StatsNode.minAccessCondsForDNFCond {
            return false;
        }
        if self.StatsNode.numCols < other.StatsNode.numCols {
            return true;
        }
        if self.StatsNode.numCols != other.StatsNode.numCols {
            return false;
        }
        self.StatsNode.Selectivity < other.StatsNode.Selectivity
    }
}

/// isColEqCorCol 判断表达式是否为普通列与 correlated column 的等值条件。
pub fn isColEqCorCol(filter: &dyn expression::Expression) -> Option<expression::Column> {
    let f = filter.as_scalar_function()?;
    if f.FuncName.L != ast::EQ {
        return None;
    }
    if let Some(c) = f.GetArgs()[0].as_column() {
        if f.GetArgs()[1].as_correlated_column().is_some() {
            return Some(c.clone());
        }
    }
    if let Some(c) = f.GetArgs()[1].as_column() {
        if f.GetArgs()[0].as_correlated_column().is_some() {
            return Some(c.clone());
        }
    }
    None
}

/// findPrefixOfIndex 按 unique id 查找输入列在索引中的连续前缀。
pub fn findPrefixOfIndex(
    cols: &[expression::Column],
    idxColIDs: &[i64],
) -> Vec<expression::Column> {
    let mut retCols = Vec::with_capacity(idxColIDs.len());
    'idLoop: for id in idxColIDs {
        for col in cols {
            if col.UniqueID == *id {
                retCols.push(col.clone());
                continue 'idLoop;
            }
        }
        return retCols;
    }
    retCols
}

/// findPrefixOfIndexByCol 对应 Go 的索引前缀查找，优先使用 cachedPath 中已解析的表达式索引列。
pub fn findPrefixOfIndexByCol(
    ctx: &dyn planctx::PlanContext,
    cols: &[expression::Column],
    idxColIDs: &[i64],
    cachedPath: Option<&planutil::AccessPath>,
) -> Vec<expression::Column> {
    if let Some(path) = cachedPath {
        let evalCtx = ctx.GetExprCtx().GetEvalCtx();
        let mut retCols = Vec::with_capacity(path.IdxCols.len());
        'idLoop: for idCol in &path.IdxCols {
            for col in cols {
                if col.EqualByExprAndID(evalCtx, idCol) {
                    retCols.push(col.clone());
                    continue 'idLoop;
                }
            }
            return retCols;
        }
        return retCols;
    }
    findPrefixOfIndex(cols, idxColIDs)
}

/// getMaskAndRanges 对应 Go 的 range detach/build 辅助函数。
/// 它返回哪些表达式被 access condition 覆盖，以及 DNF 是否只有部分覆盖。
pub fn getMaskAndRanges(
    ctx: &dyn planctx::PlanContext,
    exprs: &[expression::ExprBox],
    rangeType: ranger::RangeType,
    lengths: &[i32],
    cachedPath: Option<&planutil::AccessPath>,
    cols: &[expression::Column],
) -> Result<(i64, ranger::Ranges, bool, i32, ()), errors::Error> {
    let mut mask = 0_i64;
    let mut ranges = ranger::Ranges::default();
    let mut accessConds = Vec::<expression::ExprBox>::new();
    let mut remainedConds = Vec::<expression::ExprBox>::new();
    let mut isDNF = false;
    let mut minAccessCondsForDNFCond = 0;
    match rangeType {
        ranger::ColumnRangeType => {
            accessConds = ranger::ExtractAccessConditionsForColumn(
                ctx.GetRangerCtx(),
                exprs.iter().cloned().collect(),
                cols[0].clone(),
            );
            let Some(ret_type) = cols[0].RetType.as_ref() else {
                return Ok((0, ranges, false, 0, ()));
            };
            let mut ranger_ctx = ctx.GetRangerCtx().clone();
            let built = ranger::BuildColumnRange(
                accessConds,
                &mut ranger_ctx,
                ret_type,
                types::UnspecifiedLength,
                ctx.GetSessionVars().RangeMaxSize,
            )?;
            ranges = built.0;
            accessConds = built.1;
        }
        ranger::IndexRangeType => {
            if let Some(path) = cachedPath {
                ranges = ranger::Ranges(path.Ranges.clone());
                accessConds = path.AccessConds.clone();
                remainedConds = path.TableFilters.clone();
                isDNF = path.IsDNFCond;
                minAccessCondsForDNFCond = path.MinAccessCondsForDNFCond;
            } else {
                let res = ranger::DetachCondAndBuildRangeForIndex(
                    ctx.GetRangerCtx(),
                    exprs.iter().cloned().collect(),
                    cols.to_vec(),
                    lengths.to_vec(),
                    ctx.GetSessionVars().RangeMaxSize,
                )?;
                ranges = res.Ranges;
                accessConds = res.AccessConds;
                remainedConds = res.RemainedConds;
                isDNF = res.IsDNFCond;
                minAccessCondsForDNFCond = res.MinAccessCondsForDNFCond as usize;
            }
        }
        _ => panic!("should never be here"),
    }
    if isDNF && !accessConds.is_empty() {
        mask |= 1;
        return Ok((
            mask,
            ranges,
            !remainedConds.is_empty(),
            minAccessCondsForDNFCond as i32,
            (),
        ));
    }
    for (i, expr) in exprs.iter().enumerate() {
        for accessCond in &accessConds {
            if expr.Equal(ctx.GetExprCtx().GetEvalCtx(), accessCond.as_ref()) {
                mask |= 1_i64 << i as u64;
                break;
            }
        }
    }
    Ok((mask, ranges, false, 0, ()))
}

/// getMaskAndSelectivityForMVIndex 对应 Go 的 MV index access condition 收集与 partial path 选择率估算。
pub fn getMaskAndSelectivityForMVIndex(
    ctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    id: i64,
    exprs: &[expression::ExprBox],
) -> (f64, i64, bool) {
    let col_ids = &coll.MVIdx2Columns[&id];
    let cols = expression::ExtractColumnsMapFromExpressions(|_| true, exprs)
        .into_values()
        .filter(|column| col_ids.contains(&column.UniqueID))
        .collect::<Vec<_>>();
    if cols.is_empty() {
        return (1.0, 0, false);
    }
    let Some(collect_filters) = (unsafe { CollectFilters4MVIndex }) else {
        return (1.0, 0, false);
    };
    let Some(build_partial_paths) = (unsafe { BuildPartialPaths4MVIndex }) else {
        return (1.0, 0, false);
    };
    let (accessConds, _, _) = collect_filters(ctx, exprs, &cols);
    let stats_info = coll.GetIdx(id).unwrap().InfoRef();
    let model_info = model::IndexInfo {
        ID: stats_info.ID,
        Unique: stats_info.Unique,
        MVIndex: stats_info.MVIndex,
        ConditionExprString: stats_info.ConditionExprString.clone(),
        ..Default::default()
    };
    let res = build_partial_paths(ctx, &accessConds, &cols, &model_info, coll);
    if res.err.is_some() || !res.ok {
        return (1.0, 0, false);
    }
    let totalSelectivity =
        CalcTotalSelectivityForMVIdxPath(coll, &res.partialPaths, res.isIntersection);
    let mut mask = 0_i64;
    for (i, expr) in exprs.iter().enumerate() {
        for accessCond in &accessConds {
            if expr.Equal(ctx.GetExprCtx().GetEvalCtx(), accessCond.as_ref()) {
                mask |= 1_i64 << i as u64;
                break;
            }
        }
    }
    (totalSelectivity, mask, true)
}

/// GetSelectivityByFilter 对应 Go 的 TopN/Histogram/NULL 辅助表达式评估。
/// 只处理安全、单列、且可从统计值恢复的过滤表达式。
pub fn GetSelectivityByFilter(
    sctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    mut filters: expression::ExprBox,
) -> Result<(bool, f64), errors::Error> {
    prepareFilterForStatsEvaluation(filters.as_mut());
    if expression::IsMutableEffectsExpr(filters.as_ref())
        || expression::ContainCorrelatedColumn(&[filters.clone()])
    {
        return Ok((false, 0.0));
    }
    let cols = expression::ExtractColumnsMapFromExpressions(|_| true, &[filters.clone()]);
    if cols.len() != 1 {
        return Ok((false, 0.0));
    }
    let col = cols.into_values().next().unwrap();
    let Some(tp) = col.RetType.clone() else {
        return Ok((false, 0.0));
    };
    if types::IsString(tp.GetType())
        && collate::NewCollationEnabled()
        && !collate::IsBinCollation(tp.GetCollate())
    {
        return Ok((false, 0.0));
    }

    let (isIndex, i) = findAvailableStatsForCol(sctx, coll, col.UniqueID);
    if i < 0 {
        return Ok((false, 0.0));
    }
    let (statsVer, nullCnt, hist, topn) = if isIndex {
        let stats = coll.GetIdx(i).unwrap();
        (
            stats.StatsVer,
            stats.Histogram.NullCount,
            &stats.Histogram,
            stats.TopN.as_ref(),
        )
    } else {
        let stats = coll.GetCol(i).unwrap();
        (
            stats.StatsVer,
            stats.Histogram.NullCount,
            &stats.Histogram,
            stats.TopN.as_ref(),
        )
    };
    if statsVer != statistics::Version2 as i64 {
        return Ok((false, 0.0));
    }
    let topnTotalCnt = topn.TotalCount();
    let histTotalCnt = hist.NotNullCount();
    let totalCnt = topnTotalCnt as f64 + histTotalCnt + nullCnt as f64;

    // 传入表达式副本已在函数入口完成列索引归一化；后续 chunk 求值使用
    // 的是该副本，而不是列提取 API 返回的元数据副本。
    let topNLen = topn.map_or(0, |topn| topn.TopN.len());
    let histBucketsLen = hist.Len();
    let mut c = chunk::NewChunkWithCapacity(vec![tp.clone()], std::cmp::max(1, topNLen));
    let mut selected = Vec::<bool>::with_capacity(std::cmp::max(histBucketsLen, topNLen));
    let vecEnabled = sctx.GetSessionVars().EnableVectorizedExpression;

    let mut topNSelectedCnt = 0_u64;
    if let Some(topn) = topn {
        for item in &topn.TopN {
            let (_, val) = codec::DecodeOne(&item.Encoded)?;
            c.AppendDatum(0, &val);
        }
        let mut iterator = chunk::NewIterator4Chunk(c.clone());
        selected = expression::VectorizedFilter(
            sctx.GetExprCtx().GetEvalCtx(),
            vecEnabled,
            &[filters.clone()],
            &mut iterator,
            selected,
        )?;
        for (idx, isTrue) in selected.iter().enumerate() {
            if *isTrue {
                topNSelectedCnt += topn.TopN[idx].Count;
            }
        }
    }
    let topNSel = topNSelectedCnt as f64 / totalCnt;

    let mut histSel = 0.0;
    if histTotalCnt > 0.0 {
        selected.clear();
        // Bounds 是 stats-cache chunk；Go 复制浅层 header 并清空 Sel，避免改写共享状态。
        let mut histBounds = chunk::NewChunkWithCapacity(vec![tp.clone()], hist.Bounds.len());
        for bound in &hist.Bounds {
            histBounds.AppendDatum(0, bound);
        }
        let mut iterator = chunk::NewIterator4Chunk(histBounds);
        selected = expression::VectorizedFilter(
            sctx.GetExprCtx().GetEvalCtx(),
            vecEnabled,
            &[filters.clone()],
            &mut iterator,
            selected,
        )?;
        let mut bucketRepeatTotalCnt = 0_i64;
        let mut bucketRepeatSelectedCnt = 0_i64;
        let mut lowerBoundMatchCnt = 0_i64;
        for i in 0..hist.Buckets.len() {
            bucketRepeatTotalCnt += hist.Buckets[i].Repeat;
            if selected.len() < 2 * i {
                break;
            }
            if selected[2 * i] {
                lowerBoundMatchCnt += 1;
            }
            if selected[2 * i + 1] {
                bucketRepeatSelectedCnt += hist.Buckets[i].Repeat;
            }
        }
        let upperBoundsRatio = (bucketRepeatTotalCnt as f64 / histTotalCnt).min(1.0);
        let lowerBoundsRatio = 1.0 - upperBoundsRatio;
        let mut upperBoundsSel = 0.0;
        if bucketRepeatTotalCnt > 0 {
            upperBoundsSel = bucketRepeatSelectedCnt as f64 / bucketRepeatTotalCnt as f64;
        }
        let lowerBoundsSel = lowerBoundMatchCnt as f64 / histBucketsLen as f64;
        histSel = lowerBoundsSel * lowerBoundsRatio + upperBoundsSel * upperBoundsRatio;
        histSel *= histTotalCnt / totalCnt;
    }

    c.Reset();
    c.AppendNull(0);
    selected.clear();
    let mut iterator = chunk::NewIterator4Chunk(c.clone());
    selected = expression::VectorizedFilter(
        sctx.GetExprCtx().GetEvalCtx(),
        vecEnabled,
        &[filters],
        &mut iterator,
        selected,
    )?;
    let nullSel = if selected.len() != 1 || !selected[0] {
        0.0
    } else {
        nullCnt as f64 / totalCnt
    };
    Ok((true, topNSel + histSel + nullSel))
}

/// findAvailableStatsForCol 优先找可用列统计，再找无前缀长度的单列索引统计。
pub fn findAvailableStatsForCol(
    sctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    uniqueID: i64,
) -> (bool, i64) {
    if let Some(colStats) = coll.GetCol(uniqueID) {
        if !statistics::ColumnStatsIsInvalid(Some(colStats), coll.Pseudo) && colStats.IsFullLoad() {
            return (false, uniqueID);
        }
    }
    for (idxStatsIdx, cols) in &coll.Idx2ColUniqueIDs {
        if cols.len() == 1 && cols[0] == uniqueID {
            let idxStats = coll.GetIdx(*idxStatsIdx).unwrap();
            if !statistics::IndexStatsIsInvalid(Some(idxStats), coll.Pseudo)
                && idxStats.InfoRef().Columns[0].Length == types::UnspecifiedLength
                && idxStats.IsFullLoad()
            {
                return (true, *idxStatsIdx);
            }
        }
    }
    (false, -1)
}

/// getEqualCondSelectivity 对应 Go 的索引等值条件选择率估算。
pub fn getEqualCondSelectivity(
    sctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    idx: &statistics::Index,
    bytes: Vec<u8>,
    usedColsLen: usize,
    idxPointRange: &ranger::Range,
) -> Result<f64, errors::Error> {
    let coverAll = idx.InfoRef().Columns.len() == usedColsLen;
    if idx.InfoRef().Unique && coverAll {
        return Ok(1.0 / idx.TotalRowCount());
    }
    let val = types::NewBytesDatum(bytes.clone());
    if outOfRangeOnIndex(idx, val) {
        let (realtimeCnt, _) = coll.GetScaledRealtimeAndModifyCnt(idx);
        if idx.NDV > 0 && coverAll {
            return Ok(outOfRangeEQSelectivity(
                sctx,
                idx.NDV,
                realtimeCnt,
                idx.TotalRowCount() as i64,
            ));
        }
        let colIDs = &coll.Idx2ColUniqueIDs[&idx.ID];
        let mut ndv = 0_i64;
        for (i, colID) in colIDs.iter().enumerate() {
            if i >= usedColsLen {
                break;
            }
            if let Some(col) = coll.GetCol(*colID) {
                ndv = ndv.max(col.Histogram.NDV);
            }
        }
        return Ok(outOfRangeEQSelectivity(
            sctx,
            ndv,
            realtimeCnt,
            idx.TotalRowCount() as i64,
        ));
    }
    let (minRowCount, crossValidSelectivity) =
        crossValidationSelectivity(sctx, coll, idx, usedColsLen, idxPointRange)?;
    let idxCount = idx.QueryBytes(&bytes) as f64;
    if minRowCount < idxCount {
        return Ok(crossValidSelectivity);
    }
    Ok(idxCount / idx.TotalRowCount())
}

/// outOfRangeEQSelectivity 对应 Go 的 out-of-range 等值启发式选择率。
pub fn outOfRangeEQSelectivity(
    _sctx: &dyn planctx::PlanContext,
    mut ndv: i64,
    realtimeRowCount: i64,
    columnRowCount: i64,
) -> f64 {
    let increaseRowCount = realtimeRowCount - columnRowCount;
    if increaseRowCount <= 0 {
        return 0.0;
    }
    unsafe {
        if ndv < outOfRangeBetweenRate {
            ndv = outOfRangeBetweenRate;
        }
    }
    let mut selectivity = 1.0 / ndv as f64;
    if selectivity * columnRowCount as f64 > increaseRowCount as f64 {
        selectivity = increaseRowCount as f64 / columnRowCount as f64;
    }
    selectivity
}

/// outOfRangeFullNDV 对应 Go 的 TopN 覆盖全部 NDV 时的未命中估算。
pub fn outOfRangeFullNDV(
    mut ndv: f64,
    origRowCount: f64,
    mut notNullCount: f64,
    realtimeRowCount: f64,
    increaseFactor: f64,
    modifyCount: i64,
) -> f64 {
    if modifyCount == 0 {
        return 0.0;
    }
    let mut newRows = realtimeRowCount - origRowCount;
    if notNullCount <= 0.0 {
        notNullCount = origRowCount.min(realtimeRowCount);
    }
    if newRows < 0.0 {
        newRows = notNullCount.min(realtimeRowCount);
    }
    if ndv <= 0.0 {
        ndv = notNullCount.max(realtimeRowCount).sqrt();
    } else {
        // 调用者随后会乘 increaseFactor，这里先同步放大 NDV，避免重复放大结果。
        ndv *= increaseFactor;
    }
    unsafe {
        ndv = ndv.max(outOfRangeBetweenRate as f64);
    }
    1.0_f64.max(newRows / ndv)
}

/// crossValidationSelectivity 通过逐列点范围交叉验证多列索引等值选择率。
pub fn crossValidationSelectivity(
    sctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    idx: &statistics::Index,
    usedColsLen: usize,
    idxPointRange: &ranger::Range,
) -> Result<(f64, f64), errors::Error> {
    let mut minRowCount = f64::MAX;
    let cols = &coll.Idx2ColUniqueIDs[&idx.ID];
    let mut crossValidationSelectivity = 1.0;
    let totalRowCount = idx.TotalRowCount();
    for (i, colID) in cols.iter().enumerate() {
        if i >= usedColsLen {
            break;
        }
        let col = coll.GetCol(*colID);
        if statistics::ColumnStatsIsInvalid(col, coll.Pseudo) {
            continue;
        }
        // 点范围必须强制闭区间，否则 getColumnRowCount 会按开边界得到 0。
        let rang = ranger::Range {
            LowVal: vec![idxPointRange.LowVal[i].clone()],
            LowExclude: false,
            HighVal: vec![idxPointRange.HighVal[i].clone()],
            HighExclude: false,
            Collators: vec![collate::GetCollator(&idxPointRange.LowVal[i].Collation())],
            ..Default::default()
        };
        let rowCountEst = getColumnRowCount(
            sctx,
            col.unwrap(),
            &[&rang],
            coll.RealtimeCount,
            coll.ModifyCount,
            col.unwrap().IsHandle,
        )?;
        let rowCount = rowCountEst.Est;
        crossValidationSelectivity *= rowCount / totalRowCount;
        if rowCount < minRowCount {
            minRowCount = rowCount;
        }
    }
    Ok((minRowCount, crossValidationSelectivity))
}

/// CollectFilters4MVIndex 和 BuildPartialPaths4MVIndex 在 Go 中由 planner/core 注入，用于避免 import cycle。
// / Rust 保留函数变量形状，后续跨模块迁移时再接入真实实现。
pub static mut CollectFilters4MVIndex: Option<
    fn(
        &dyn planctx::PlanContext,
        &[expression::ExprBox],
        &[expression::Column],
    ) -> (Vec<expression::ExprBox>, Vec<expression::ExprBox>, i32),
> = None;

/// 由 planner/core 注入的 MV Index partial path 构建回调，形状与 Go 注入函数一致。
pub static mut BuildPartialPaths4MVIndex: Option<
    fn(
        &dyn planctx::PlanContext,
        &[expression::ExprBox],
        &[expression::Column],
        &model::IndexInfo,
        &statistics::HistColl,
    ) -> BuildPartialPaths4MVIndexResult,
> = None;

/// BuildPartialPaths4MVIndexResult 是 Go 多返回值的临时承载结构。
pub struct BuildPartialPaths4MVIndexResult {
    pub partialPaths: Vec<&'static planutil::AccessPath>,
    pub isIntersection: bool,
    pub ok: bool,
    pub err: Option<errors::Error>,
}
