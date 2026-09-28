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

// 索引剪枝（Index Pruning）辅助规则。
//
// 按 WHERE/ORDER 感兴趣列覆盖度给候选访问路径（Access Path）打分，
// 保留最有希望的索引，降低优化器枚举代价。访问路径指表扫/索引扫等物理读法候选。

// 优化规则的类型、控制流和错误处理形状。
// TiDB planner、expression、logicalop 等外部类型保留为后续模块接线点；本任务不创建跨文件模块连线。
//
//
// Go imports（保留来源依赖，等待后续 Rust 模块接线）：
// 	"fmt"
// 	"slices"
// 	"strings"
// 	"github.com/pingcap/failpoint"
// 	"github.com/pingcap/tidb/pkg/expression"
// 	"github.com/pingcap/tidb/pkg/meta/model"
// 	"github.com/pingcap/tidb/pkg/planner/core/operator/logicalop"
// 	"github.com/pingcap/tidb/pkg/planner/util"
// 	"github.com/pingcap/tidb/pkg/planner/util/fixcontrol"
//
// const (
// defaultMaxIndexes is the default maximum number of indexes to keep when pruning.
// This prevents overly aggressive pruning when the threshold is small.
// TODO: We should add a second pruning phase around the fillIndexPath step,
// where we can refine the indexes further based on the actual column statistics.
// Therefore, if a customer did set tidn_opt_index_prune_threshold < 10, we could
// a minimum of 10 in the first pruning phase, and prune further in the second phase.
//     defaultMaxIndexes = 10
// )
//
// indexWithScore stores an access path along with its coverage scores for ranking.
// indexWithScore 对应 Go 同名类型；字段顺序和迁移阶段的外部依赖形状保持不变。
// type indexWithScore struct {
//     path                 *util.AccessPath
//     interestingCount     int     // Total number of interesting columns covered
//     consecutiveColumnIDs []int64 // IDs of consecutive columns (for detecting different orderings)
// }
// columnRequirements holds the column maps needed for index pruning.
// columnRequirements 对应 Go 同名类型；字段顺序和迁移阶段的外部依赖形状保持不变。
// type columnRequirements struct {
//     interestingColIDs map[int64]struct{}
// }
//
// ShouldPreferIndexMerge returns true if index merge should be preferred, either due to hints or fix control.
// ShouldPreferIndexMerge 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func ShouldPreferIndexMerge(ds *logicalop.DataSource) bool {
//     return len(ds.IndexMergeHints) > 0 || fixcontrol.GetBoolWithDefault(
//         ds.SCtx().GetSessionVars().GetOptimizerFixControlMap(),
//         fixcontrol.Fix52869,
//         false,
//     )
// }
//
// PruneIndexesByWhereAndOrder prunes indexes based on their coverage of interesting columns.
// It keeps the most promising indexes up to the threshold, prioritizing those that:
// 1. Cover more interesting columns
// 2. Have consecutive column matches from the index start (enabling index prefix usage)
// 3. Support single-scan (covering index without table lookups)
// 4. Have different consecutive column orderings (e.g., if interesting columns are A, B, keep both (A,B) and (B,A))
// The threshold controls the behavior:
// threshold = -1: disable pruning (handled by caller)
// threshold = 0: only prune indexes with no interesting columns (score == 0)
// threshold > 0: keep at least threshold indexes (but at least defaultMaxIndexes)
// but if there are fewer than threshold indexes, we will still prune zero-score indexes.
// PruneIndexesByWhereAndOrder 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func PruneIndexesByWhereAndOrder(ds *logicalop.DataSource, paths []*util.AccessPath, interestingColumns []*expression.Column, threshold int) []*util.AccessPath {
//     if len(paths) <= 1 {
//         return paths
//     }
//
//     totalPathCount := len(paths)
//
// If we disabled the prune, return directly.
//     if threshold < 0 {
//         return paths
//     }
// Now the prune must happen.
//
// If threshold is 0 or greater than total paths, we only prune zero-score indexes.
//     onlyPruneZeroScore := threshold == 0 || (threshold > totalPathCount)
//
// Build column ID maps and calculate totals
//     req := buildColumnRequirements(interestingColumns)
//
//     preferredIndexes := make([]indexWithScore, 0, totalPathCount)
//     tablePaths := make([]*util.AccessPath, 0, 1)
//     mvIndexPaths := make([]*util.AccessPath, 0, 1)
//     indexMergeIndexPaths := make([]*util.AccessPath, 0, 1)
//     preferMerge := ShouldPreferIndexMerge(ds)
//
// Check if IndexMerge hints specify specific index names
// We need to import the function from indexmerge_path.go, but since it's in a different package,
// we'll implement the check inline here
//     hasSpecifiedIndexes := false
//     if len(ds.IndexMergeHints) > 0 {
//         for _, hint := range ds.IndexMergeHints {
//             if hint.IndexHint != nil && len(hint.IndexHint.IndexNames) > 0 {
//                 hasSpecifiedIndexes = true
//                 break
//             }
//         }
//     }
//
// Categorize each index path
//     for _, path := range paths {
//         if path.IsTablePath() {
//             tablePaths = append(tablePaths, path)
//             continue
//         }
//
// Always keep multi-value indexes (like table paths)
//         if path.Index != nil && path.Index.MVIndex {
//             mvIndexPaths = append(mvIndexPaths, path)
//             continue
//         }
//
// If we have forced paths, we shouldn't prune any paths
//         if path.Forced {
//             return paths
//         }
//
// Skip paths with nil Index
//         if path.Index == nil {
//             continue
//         }
//
// Calculate coverage for this index
// Use TableInfo.Columns (not ds.Columns) because IndexColumn.Offset refers to TableInfo.Columns
//         var tableColumns []*model.ColumnInfo
//         if ds.TableInfo != nil {
//             tableColumns = ds.TableInfo.Columns
//         }
//         idxScore := scoreIndexPath(ds, path, req, tableColumns)
//
//         if path.FullIdxCols != nil {
//             path.IsSingleScan = ds.IsSingleScan(path.FullIdxCols, path.FullIdxColLens)
//         }
//
// Check if this index is specified in IndexMerge hints
// Note: Indexes specified in USE_INDEX_MERGE(t, idx1, idx2) are NOT marked as "forced"
// (only USE_INDEX/FORCE_INDEX mark indexes as forced), so we need to collect them here
// to ensure they're not pruned. This is only needed when hints specify specific index names.
//         if hasSpecifiedIndexes {
//             indexName := path.Index.Name.L
//             isSpecified := false
//             for _, hint := range ds.IndexMergeHints {
//                 if hint.IndexHint == nil || len(hint.IndexHint.IndexNames) == 0 {
//                     continue
//                 }
//                 for _, hintName := range hint.IndexHint.IndexNames {
// Use case-insensitive comparison like isSpecifiedInIndexMergeHints does
//                     if strings.EqualFold(indexName, hintName.String()) {
//                         isSpecified = true
//                         break
//                     }
//                 }
//                 if isSpecified {
//                     break
//                 }
//             }
//             if isSpecified {
// This index is explicitly specified in IndexMerge hints, keep it
// Add it to indexMergeIndexPaths so it's guaranteed to be included even if it doesn't score well
//                 indexMergeIndexPaths = append(indexMergeIndexPaths, path)
//                 continue
//             }
//         }
//
// If index merge is preferred (via general hints without index names, or fix control),
// keep indexes that have any coverage. Note: When specific indexes are mentioned in hints,
// those are handled above. Here we handle general IndexMerge hints (no specific index names)
// or fix control. We still apply some filtering (len(consecutiveColumnIDs) > 0 OR other coverage)
// to avoid keeping completely useless indexes, but we're more lenient than normal pruning.
//         if preferMerge && !hasSpecifiedIndexes {
// When IndexMerge is preferred without specific index names, keep any index with coverage
//             if len(idxScore.consecutiveColumnIDs) > 0 || path.IsSingleScan || idxScore.interestingCount > 0 {
//                 preferredIndexes = append(preferredIndexes, idxScore)
//                 continue
//             }
//         }
//
// Add to preferred indexes if it has any coverage or is a covering scan
// We'll handle ordering diversity in buildFinalResult to ensure we keep
// different orderings even if they have lower scores
//         if path.IsSingleScan || idxScore.interestingCount > 0 {
//             preferredIndexes = append(preferredIndexes, idxScore)
//         }
//     }
//
// Build final result by sorting and selecting top indexes
//     maxToKeep := max(threshold, defaultMaxIndexes)
//     result := buildFinalResult(tablePaths, mvIndexPaths, indexMergeIndexPaths, preferredIndexes, maxToKeep, onlyPruneZeroScore, req)
//
//     failpoint.InjectCall("InjectCheckForIndexPrune", result)
//
// Safety check: if we ended up with nothing, return the original paths
//     if len(result) == 0 {
//         return paths
//     }
//
// Additional safety: if we only have table paths and MVIndex paths and no regular indexes, keep original
//     if len(result) == len(tablePaths)+len(mvIndexPaths) && len(preferredIndexes) == 0 {
//         return paths
//     }
//
//     return result
// }
//
// buildColumnRequirements builds column ID maps for efficient lookup.
// buildColumnRequirements 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func buildColumnRequirements(interestingColumns []*expression.Column) columnRequirements {
//     req := columnRequirements{
//         interestingColIDs: make(map[int64]struct{}, len(interestingColumns)),
//     }
//
// Build interesting column IDs
//     for _, col := range interestingColumns {
//         req.interestingColIDs[col.ID] = struct{}{}
//     }
//
//     return req
// }
//
// buildOrderingKey creates a string key representing the consecutive column ordering.
// This is used to detect and keep indexes with different orderings.
// buildOrderingKey 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func buildOrderingKey(columnIDs []int64) string {
//     if len(columnIDs) == 0 {
//         return ""
//     }
// Create a simple string representation of the column ID sequence
// Using a format like "1,2,3" for columns with IDs 1, 2, 3
//     var builder strings.Builder
// Pre-allocate capacity: estimate ~4 bytes per ID (for small IDs) + commas
//     builder.Grow(len(columnIDs) * 5)
//     for i, id := range columnIDs {
//         if i > 0 {
//             builder.WriteString(",")
//         }
//         fmt.Fprintf(&builder, "%d", id)
//     }
//     return builder.String()
// }
//
// scoreIndexPath calculates coverage metrics for a single index path.
// When FullIdxCols is nil (e.g., in static pruning mode), it uses path.Index.Columns
// and tableColumns to determine interesting columns. Note that consecutiveColumnIDs
// cannot be determined when FullIdxCols is nil, so it will remain empty.
// scoreIndexPath 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func scoreIndexPath(
//     ds *logicalop.DataSource,
//     path *util.AccessPath,
//     req columnRequirements,
//     tableColumns []*model.ColumnInfo,
// ) indexWithScore {
//     score := indexWithScore{path: path}
//
//     if path.Index != nil && path.Index.ConditionExprString != "" {
//         for _, col := range path.Index.AffectColumn {
// Some columns from the constraint is not found. Then the path can not be selected.
// We mixed the WHERE clause and other clauses like JOIN/ORDER BY to prune the indexes.
// So this check is not the strictest one.
//             if _, found := req.interestingColIDs[ds.TableInfo.Columns[col.Offset].ID]; !found {
//                 return score
//             }
//         }
// Pre check for partial index passed, continue to calculate the score.
//     }
//
//     if path.FullIdxCols != nil {
// Normal path: use FullIdxCols which contains expression.Column with IDs
//         for i, idxCol := range path.FullIdxCols {
//             if idxCol == nil {
//                 continue
//             }
//             idxColID := idxCol.ID
//
// Check if this index column matches an interesting column
//             if _, found := req.interestingColIDs[idxColID]; found {
//                 score.interestingCount++
// Track consecutive columns from the start of the index
//                 if i == len(score.consecutiveColumnIDs) {
//                     score.consecutiveColumnIDs = append(score.consecutiveColumnIDs, idxColID)
//                 }
//             }
// Note: We continue checking all columns to count all interesting columns,
// even if they're not consecutive from the start. The consecutive tracking
// will naturally stop once we hit a non-interesting column, since the condition
// `i == len(score.consecutiveColumnIDs)` will no longer be true.
//         }
//     } else if path.Index != nil && tableColumns != nil {
// Fallback path: use Index.Columns (for static pruning mode when FullIdxCols is nil)
// Map IndexColumn.Offset to column ID via tableColumns
//         for _, idxCol := range path.Index.Columns {
//             if idxCol.Offset < 0 || idxCol.Offset >= len(tableColumns) {
//                 continue
//             }
//             colInfo := tableColumns[idxCol.Offset]
//             if colInfo == nil {
//                 continue
//             }
//             idxColID := colInfo.ID
//
// Check if this index column matches an interesting column
//             if _, found := req.interestingColIDs[idxColID]; found {
//                 score.interestingCount++
// Note: We cannot track consecutiveColumnIDs here because we don't have
// the full column information needed to determine if columns are consecutive
// in the index. This is acceptable as the user indicated.
//             }
//         }
//     }
//
//     return score
// }
//
// buildFinalResult sorts and selects the top indexes to keep, combining table paths,
// multi-value indexes, index merge indexes, and preferred indexes.
// scoredIndex 对应 Go 同名类型；字段顺序和迁移阶段的外部依赖形状保持不变。
// type scoredIndex struct {
//     info             indexWithScore
//     score            int
//     columns          int
//     isSingleScan     bool
//     totalConsecutive int
// }
//
// scoreAndSort 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func scoreAndSort(indexes []indexWithScore, req columnRequirements) []scoredIndex {
//     if len(indexes) == 0 {
//         return nil
//     }
//     scored := make([]scoredIndex, 0, len(indexes))
//     for _, candidate := range indexes {
//         score := calculateScoreFromCoverage(candidate, len(req.interestingColIDs), candidate.path.IsSingleScan)
// Skip indexes with score == 0 as they don't provide any value
//         if score == 0 {
//             continue
//         }
//         cols := len(candidate.path.FullIdxCols)
//         scored = append(scored, scoredIndex{
//             info:             candidate,
//             score:            score,
//             columns:          cols,
//             isSingleScan:     candidate.path.IsSingleScan,
//             totalConsecutive: len(candidate.consecutiveColumnIDs),
//         })
//     }
//     slices.SortFunc(scored, func(a, b scoredIndex) int {
// Tie-breaker: prefer indexes with higher score
//         if a.score != b.score {
//             return b.score - a.score
//         }
// Tie-breaker: prefer indexes with more consecutive columns
//         if a.totalConsecutive != b.totalConsecutive {
//             return b.totalConsecutive - a.totalConsecutive
//         }
// Tie-breaker: prefer indexes with single-scan
//         if a.isSingleScan != b.isSingleScan {
//             if a.isSingleScan {
//                 return -1
//             }
//             return 1
//         }
// Tie-breaker: prefer indexes with fewer columns if
// they have only 1 consecutive column.
//         if a.totalConsecutive == 1 && a.columns != b.columns {
//             return a.columns - b.columns
//         }
// Tie-breaker: use index ID for deterministic ordering when all other criteria are equal
// This ensures stable sorting for functionally identical indexes (e.g., k1 and k2 with same expressions)
//         if a.info.path.Index != nil && b.info.path.Index != nil {
// Use proper three-way comparison to avoid integer overflow
// (a.info.path.Index.ID is int64, casting the difference to int can overflow)
//             if a.info.path.Index.ID < b.info.path.Index.ID {
//                 return -1
//             }
//             if a.info.path.Index.ID > b.info.path.Index.ID {
//                 return 1
//             }
//         }
//         return 0
//     })
//     return scored
// }
//
// buildFinalResult 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func buildFinalResult(tablePaths, mvIndexPaths, indexMergeIndexPaths []*util.AccessPath, preferredIndexes []indexWithScore, maxToKeep int, onlyPruneZeroScore bool, req columnRequirements) []*util.AccessPath {
//     result := make([]*util.AccessPath, 0, len(tablePaths)+len(indexMergeIndexPaths)+len(preferredIndexes))
//
// CRITICAL: Always include table paths - this is mandatory for correctness
//     result = append(result, tablePaths...)
// CRITICAL: Always include multi-value index paths - we do not have sufficient
// information to determine if they should be pruned in this function.
//     result = append(result, mvIndexPaths...)
// CRITICAL: Always include indexes specified in IndexMerge hints - index merge needs them to build partial paths
//     result = append(result, indexMergeIndexPaths...)
//
//     added := make(map[*util.AccessPath]struct{}, len(tablePaths)+len(mvIndexPaths)+len(indexMergeIndexPaths))
//     for _, path := range tablePaths {
//         added[path] = struct{}{}
//     }
//     for _, path := range mvIndexPaths {
//         added[path] = struct{}{}
//     }
//     for _, path := range indexMergeIndexPaths {
//         added[path] = struct{}{}
//     }
//
//     preferredScored := scoreAndSort(preferredIndexes, req)
//
// Prune all the path with 0 score.
//     if onlyPruneZeroScore {
//         for _, entry := range preferredScored {
//             if _, ok := added[entry.info.path]; !ok {
//                 result = append(result, entry.info.path)
//             }
//         }
//         return result
//     }
//
// Apply two-phase selection to limit the number of indexes
//     phase1Limit := maxToKeep / 2
//     selectionState := newIndexSelectionState(phase1Limit, maxToKeep)
//
//     result = selectIndexes(preferredScored, added, req, selectionState, result)
//
//     return result
// }
//
// indexSelectionState tracks state during two-phase index selection
// indexSelectionState 对应 Go 同名类型；字段顺序和迁移阶段的外部依赖形状保持不变。
// type indexSelectionState struct {
//     phase1Limit              int
//     remaining                int
//     hasNonZeroScore          bool
//     phase1Count              int
//     seenConsecutiveColumnIDs map[int64]struct{}
//     seenOrderingKeys         map[string]struct{}
// }
//
// newIndexSelectionState 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func newIndexSelectionState(phase1Limit, maxToKeep int) *indexSelectionState {
//     return &indexSelectionState{
//         phase1Limit:              phase1Limit,
//         remaining:                maxToKeep,
//         seenConsecutiveColumnIDs: make(map[int64]struct{}),
//         seenOrderingKeys:         make(map[string]struct{}),
//     }
// }
//
// selectIndexes performs two-phase selection of indexes
// selectIndexes 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func selectIndexes(preferredScored []scoredIndex, added map[*util.AccessPath]struct{}, req columnRequirements, state *indexSelectionState, result []*util.AccessPath) []*util.AccessPath {
//     for _, entry := range preferredScored {
//         path := entry.info.path
//         if _, ok := added[path]; ok {
//             continue
//         }
//         if state.remaining == 0 {
//             break
//         }
//         if state.hasNonZeroScore && entry.score == 0 {
//             continue
//         }
//
//         shouldAdd := shouldAddIndex(entry, path, req, state)
//         if shouldAdd {
//             result = append(result, path)
//             added[path] = struct{}{}
//             if entry.score > 0 {
//                 state.hasNonZeroScore = true
//             }
//             state.remaining--
//         }
//     }
//     return result
// }
//
// shouldAddIndex determines if an index should be added based on phase and diversity rules
// shouldAddIndex 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func shouldAddIndex(entry scoredIndex, path *util.AccessPath, req columnRequirements, state *indexSelectionState) bool {
//     if state.phase1Count < state.phase1Limit {
// Phase 1: Keep top threshold/2 based solely on score
//         state.phase1Count++
//         recordConsecutiveColumns(entry.info.consecutiveColumnIDs, state)
//         return true
//     }
//
// Phase 2: Apply diversity rules
//     hasConsecutive := len(entry.info.consecutiveColumnIDs) > 0
//     if hasConsecutive {
//         return shouldAddIndexWithConsecutive(entry.info.consecutiveColumnIDs, state)
//     }
//
//     return shouldAddIndexWithoutConsecutive(entry, path, req, state)
// }
//
// recordConsecutiveColumns tracks consecutive column IDs and ordering keys
// recordConsecutiveColumns 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func recordConsecutiveColumns(consecutiveColumnIDs []int64, state *indexSelectionState) {
//     for _, colID := range consecutiveColumnIDs {
//         state.seenConsecutiveColumnIDs[colID] = struct{}{}
//     }
//     if len(consecutiveColumnIDs) > 0 {
//         orderingKey := buildOrderingKey(consecutiveColumnIDs)
//         state.seenOrderingKeys[orderingKey] = struct{}{}
//     }
// }
//
// shouldAddIndexWithConsecutive checks if an index with consecutive columns should be added in phase 2
// shouldAddIndexWithConsecutive 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func shouldAddIndexWithConsecutive(consecutiveColumnIDs []int64, state *indexSelectionState) bool {
//     orderingKey := buildOrderingKey(consecutiveColumnIDs)
//     if _, seen := state.seenOrderingKeys[orderingKey]; seen {
//         return false
//     }
//     state.seenOrderingKeys[orderingKey] = struct{}{}
//     recordConsecutiveColumns(consecutiveColumnIDs, state)
//     return true
// }
//
// shouldAddIndexWithoutConsecutive checks if an index without consecutive columns should be added in phase 2
// shouldAddIndexWithoutConsecutive 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func shouldAddIndexWithoutConsecutive(entry scoredIndex, path *util.AccessPath, req columnRequirements, state *indexSelectionState) bool {
//     if entry.info.interestingCount != 1 {
//         return true
//     }
//
// For single-column indexes, check if the column is already covered by a consecutive column
//     singleColID := findSingleInterestingColumn(path, req)
//     if singleColID < 0 {
//         return true
//     }
//
// Don't add if the column is already in a consecutive column, unless it's a single scan
//     _, covered := state.seenConsecutiveColumnIDs[singleColID]
//     return !covered || path.IsSingleScan
// }
//
// findSingleInterestingColumn finds the single interesting column ID in an index.
// Returns -1 if FullIdxCols is nil (e.g., in static pruning mode) since we cannot
// determine the specific column without FullIdxCols. This causes the caller to
// keep the index, which is the safe default.
// findSingleInterestingColumn 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func findSingleInterestingColumn(path *util.AccessPath, req columnRequirements) int64 {
//     if path.FullIdxCols == nil {
//         return -1
//     }
//     for _, idxCol := range path.FullIdxCols {
//         if idxCol != nil {
//             if _, found := req.interestingColIDs[idxCol.ID]; found {
//                 return idxCol.ID
//             }
//         }
//     }
//     return -1
// }
//
// calculateScoreFromCoverage calculates a ranking score using already-computed coverage information.
// This avoids re-iterating through index columns.
// calculateScoreFromCoverage 对应 Go 同名函数或方法；保留原控制流、参数解析、分支与错误传播语义。
// func calculateScoreFromCoverage(info indexWithScore, totalColumns int, isSingleScan bool) int {
//     score := 0
//
// Score for interesting column coverage
//     score += info.interestingCount * 10
//
// Bonus for consecutive interesting columns from start (critical for index usage)
// Consecutive columns are much more valuable than scattered matches
// Index on (a,b,c,d) with interesting columns a, b, c can use first 3 columns
// But with interesting columns a, d, can only use first 1 column
//     score += len(info.consecutiveColumnIDs) * 10
//
// Bonus if the index is covering all interesting columns
//     if info.interestingCount == totalColumns {
//         score += 10
//     }
//
// Bonus for single-scan (covering index without table lookups)
//     if isSingleScan {
//         score += 20
//     }
//
//     return score
// }
// */
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet};

/// 第一阶段剪枝至少保留的索引数下限（防止阈值过小过度剪枝）。
pub const DEFAULT_MAX_INDEXES: usize = 10;

/// 索引中的一列：在表列中的偏移。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexColumn {
    pub offset: usize,
}

/// 索引元信息：名称、列、是否多值索引及条件表达式等。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexInfo {
    pub id: i64,
    pub name: String,
    pub columns: Vec<IndexColumn>,
    pub multi_value: bool,
    pub condition_expression: Option<String>,
    pub affected_column_offsets: Vec<usize>,
}

/// 候选访问路径：表路径或索引路径及其覆盖列信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessPath {
    /// Stable identity used in place of Go pointer identity.
    pub id: u64,
    pub index: Option<IndexInfo>,
    pub table_path: bool,
    pub forced: bool,
    /// `None` is the static-pruning fallback; individual `None` entries mirror
    /// generated index columns that do not map to a table column.
    pub full_index_columns: Option<Vec<Option<i64>>>,
    pub single_scan: bool,
}

impl AccessPath {
    /// 是否为表主键/句柄扫描路径。
    pub fn is_table_path(&self) -> bool {
        self.table_path
    }
}

/// Index Merge 提示中指定的索引名列表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexMergeHint {
    pub index_names: Vec<String>,
}

/// 简化版 DataSource：表列 ID、Index Merge 提示与 fix control。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataSource {
    pub table_columns: Vec<i64>,
    pub index_merge_hints: Vec<IndexMergeHint>,
    /// Result of TiDB fix control 52869.
    pub fix_52869: bool,
}

/// 是否因提示或 fix 52869 而偏好 Index Merge。
pub fn should_prefer_index_merge(source: &DataSource) -> bool {
    !source.index_merge_hints.is_empty() || source.fix_52869
}

/// 带覆盖打分的索引路径：感兴趣列数与前缀连续列。
#[derive(Clone, Debug)]
struct IndexWithScore {
    path_index: usize,
    interesting_count: usize,
    consecutive_column_ids: Vec<i64>,
}

/// 排序用的打分结果：分数、列数、是否单扫（覆盖索引）。
#[derive(Clone, Debug)]
struct ScoredIndex {
    info: IndexWithScore,
    score: usize,
    columns: usize,
    single_scan: bool,
}

/// Go-equivalent first-phase index pruning. Table paths, MV indexes, forced
/// paths and explicitly hinted merge paths retain their original guarantees.
pub fn prune_indexes_by_where_and_order(
    source: &DataSource,
    paths: Vec<AccessPath>,
    interesting_columns: &[i64],
    threshold: isize,
) -> Vec<AccessPath> {
    // 路径过少或阈值禁用时不做剪枝。
    if paths.len() <= 1 || threshold < 0 {
        return paths;
    }

    let total_path_count = paths.len();
    let only_prune_zero_score = threshold == 0 || threshold as usize > total_path_count;
    let requirements: BTreeSet<i64> = interesting_columns.iter().copied().collect();
    let prefer_merge = should_prefer_index_merge(source);
    let has_specified_indexes = source
        .index_merge_hints
        .iter()
        .any(|hint| !hint.index_names.is_empty());

    let mut table_paths = Vec::new();
    let mut mv_index_paths = Vec::new();
    let mut index_merge_paths = Vec::new();
    let mut preferred = Vec::new();

    // 分类：表路径与多值索引必留；其余按覆盖打分进入优选池。
    for (path_index, path) in paths.iter().enumerate() {
        if path.is_table_path() {
            table_paths.push(path_index);
            continue;
        }
        let Some(index) = path.index.as_ref() else {
            continue;
        };
        if index.multi_value {
            mv_index_paths.push(path_index);
            continue;
        }
        // Go checks Forced only after table and MV paths have been classified.
        if path.forced {
            return paths;
        }

        let scored = score_index_path(source, path_index, path, &requirements);
        if has_specified_indexes && index_is_explicitly_hinted(index, source) {
            index_merge_paths.push(path_index);
            continue;
        }
        if prefer_merge
            && !has_specified_indexes
            && (!scored.consecutive_column_ids.is_empty()
                || path.single_scan
                || scored.interesting_count > 0)
        {
            preferred.push(scored);
            continue;
        }
        if path.single_scan || scored.interesting_count > 0 {
            preferred.push(scored);
        }
    }

    // 至少保留 defaultMaxIndexes，避免阈值过小剪光。
    let max_to_keep = (threshold.max(0) as usize).max(DEFAULT_MAX_INDEXES);
    let selected = build_final_result(
        &paths,
        table_paths,
        mv_index_paths,
        index_merge_paths,
        preferred.clone(),
        max_to_keep,
        only_prune_zero_score,
        &requirements,
    );

    if selected.is_empty()
        || (selected.len()
            == paths
                .iter()
                .filter(|path| {
                    path.table_path || path.index.as_ref().is_some_and(|i| i.multi_value)
                })
                .count()
            && preferred.is_empty())
    {
        return paths;
    }
    selected
        .into_iter()
        .map(|index| paths[index].clone())
        .collect()
}

/// 索引是否出现在 Index Merge 提示的显式名称列表中。
fn index_is_explicitly_hinted(index: &IndexInfo, source: &DataSource) -> bool {
    source.index_merge_hints.iter().any(|hint| {
        hint.index_names
            .iter()
            .any(|name| unicode_equal_fold(name, &index.name))
    })
}

/// Match Go's Unicode-aware `strings.EqualFold` for index hint names.
fn unicode_equal_fold(left: &str, right: &str) -> bool {
    let mut left = left.chars();
    let mut right = right.chars();
    loop {
        match (left.next(), right.next()) {
            (Some(left), Some(right)) => {
                if left != right
                    && !left.to_lowercase().eq(right.to_lowercase())
                    && !left.to_uppercase().eq(right.to_uppercase())
                {
                    return false;
                }
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

/// 计算索引对感兴趣列的覆盖：连续前缀与总命中数。
fn score_index_path(
    source: &DataSource,
    path_index: usize,
    path: &AccessPath,
    requirements: &BTreeSet<i64>,
) -> IndexWithScore {
    let mut result = IndexWithScore {
        path_index,
        interesting_count: 0,
        consecutive_column_ids: Vec::new(),
    };
    let Some(index) = path.index.as_ref() else {
        return result;
    };

    if index
        .condition_expression
        .as_ref()
        .is_some_and(|condition| !condition.is_empty())
        && index.affected_column_offsets.iter().any(|offset| {
            source
                .table_columns
                .get(*offset)
                .is_none_or(|column| !requirements.contains(column))
        })
    {
        return result;
    }

    if let Some(columns) = &path.full_index_columns {
        for (position, column) in columns.iter().enumerate() {
            let Some(column) = column else {
                continue;
            };
            if requirements.contains(column) {
                result.interesting_count += 1;
                if position == result.consecutive_column_ids.len() {
                    result.consecutive_column_ids.push(*column);
                }
            }
        }
    } else {
        for column in &index.columns {
            let Some(column_id) = source.table_columns.get(column.offset) else {
                continue;
            };
            if requirements.contains(column_id) {
                result.interesting_count += 1;
            }
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
/// 合并必留路径与打分优选结果，生成最终保留的路径下标。
fn build_final_result(
    paths: &[AccessPath],
    table_paths: Vec<usize>,
    mv_index_paths: Vec<usize>,
    index_merge_paths: Vec<usize>,
    preferred: Vec<IndexWithScore>,
    max_to_keep: usize,
    only_prune_zero_score: bool,
    requirements: &BTreeSet<i64>,
) -> Vec<usize> {
    let mut result = Vec::new();
    let mut added = HashSet::new();
    for index in table_paths
        .into_iter()
        .chain(mv_index_paths)
        .chain(index_merge_paths)
    {
        if added.insert(paths[index].id) {
            result.push(index);
        }
    }

    let scored = score_and_sort(paths, preferred, requirements.len());
    // threshold==0 或大于路径数：只丢掉零分索引，其余全留。
    if only_prune_zero_score {
        for entry in scored {
            if added.insert(paths[entry.info.path_index].id) {
                result.push(entry.info.path_index);
            }
        }
        return result;
    }

    let mut state = IndexSelectionState::new(max_to_keep / 2, max_to_keep);
    for entry in scored {
        let path = &paths[entry.info.path_index];
        if added.contains(&path.id) || state.remaining == 0 {
            continue;
        }
        if state.has_non_zero_score && entry.score == 0 {
            continue;
        }
        if should_add_index(&entry, path, requirements, &mut state) {
            result.push(entry.info.path_index);
            added.insert(path.id);
            state.has_non_zero_score |= entry.score > 0;
            state.remaining -= 1;
        }
    }
    result
}

/// 过滤零分候选并按分数/前缀/单扫等键排序。
fn score_and_sort(
    paths: &[AccessPath],
    candidates: Vec<IndexWithScore>,
    total_columns: usize,
) -> Vec<ScoredIndex> {
    let mut scored: Vec<_> = candidates
        .into_iter()
        .filter_map(|info| {
            let path = &paths[info.path_index];
            let score = calculate_score_from_coverage(&info, total_columns, path.single_scan);
            (score != 0).then(|| ScoredIndex {
                columns: path.full_index_columns.as_ref().map_or(0, Vec::len),
                single_scan: path.single_scan,
                info,
                score,
            })
        })
        .collect();
    scored.sort_by(|left, right| compare_scored(paths, left, right));
    scored
}

/// 打分结果比较器：高分优先，其次连续前缀长度与单扫。
fn compare_scored(paths: &[AccessPath], left: &ScoredIndex, right: &ScoredIndex) -> Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| {
            right
                .info
                .consecutive_column_ids
                .len()
                .cmp(&left.info.consecutive_column_ids.len())
        })
        .then_with(|| right.single_scan.cmp(&left.single_scan))
        .then_with(|| {
            if left.info.consecutive_column_ids.len() == 1 {
                left.columns.cmp(&right.columns)
            } else {
                Ordering::Equal
            }
        })
        .then_with(|| {
            let left_id = paths[left.info.path_index]
                .index
                .as_ref()
                .map(|index| index.id);
            let right_id = paths[right.info.path_index]
                .index
                .as_ref()
                .map(|index| index.id);
            left_id.cmp(&right_id)
        })
}

/// 两阶段选取状态：第一阶段名额、剩余名额与已见前缀序。
#[derive(Default)]
struct IndexSelectionState {
    phase_one_limit: usize,
    remaining: usize,
    has_non_zero_score: bool,
    phase_one_count: usize,
    seen_consecutive_columns: BTreeSet<i64>,
    seen_ordering_keys: BTreeSet<String>,
}

impl IndexSelectionState {
    /// 构造选取状态：phase_one_limit 通常为 max_to_keep/2。
    fn new(phase_one_limit: usize, max_to_keep: usize) -> Self {
        Self {
            phase_one_limit,
            remaining: max_to_keep,
            ..Self::default()
        }
    }

    /// 记录已选索引的连续列集合与次序键，避免重复次序。
    fn record_consecutive_columns(&mut self, columns: &[i64]) {
        self.seen_consecutive_columns
            .extend(columns.iter().copied());
        if !columns.is_empty() {
            self.seen_ordering_keys.insert(build_ordering_key(columns));
        }
    }
}

/// 第二阶段是否再收录该索引（新前缀序或未覆盖的单列兴趣列）。
fn should_add_index(
    entry: &ScoredIndex,
    path: &AccessPath,
    requirements: &BTreeSet<i64>,
    state: &mut IndexSelectionState,
) -> bool {
    // 第一阶段：按排序直接收下前 phase_one_limit 个。
    if state.phase_one_count < state.phase_one_limit {
        state.phase_one_count += 1;
        state.record_consecutive_columns(&entry.info.consecutive_column_ids);
        return true;
    }
    if !entry.info.consecutive_column_ids.is_empty() {
        let key = build_ordering_key(&entry.info.consecutive_column_ids);
        if state.seen_ordering_keys.contains(&key) {
            return false;
        }
        state.record_consecutive_columns(&entry.info.consecutive_column_ids);
        return true;
    }
    if entry.info.interesting_count != 1 {
        return true;
    }
    let Some(column) = find_single_interesting_column(path, requirements) else {
        return true;
    };
    !state.seen_consecutive_columns.contains(&column) || path.single_scan
}

/// 在全索引列中找第一个属于感兴趣集合的列。
fn find_single_interesting_column(path: &AccessPath, requirements: &BTreeSet<i64>) -> Option<i64> {
    path.full_index_columns
        .as_ref()?
        .iter()
        .flatten()
        .copied()
        .find(|column| requirements.contains(column))
}

/// 由覆盖统计计算综合分：感兴趣列、连续前缀、全覆盖、单扫加权。
fn calculate_score_from_coverage(
    coverage: &IndexWithScore,
    total_columns: usize,
    single_scan: bool,
) -> usize {
    let interesting = coverage.interesting_count;
    let consecutive = coverage.consecutive_column_ids.len();
    interesting * 10
        + consecutive * 10
        + usize::from(interesting == total_columns) * 10
        + usize::from(single_scan) * 20
}

/// 把连续列 ID 序列编码为次序键字符串。
pub fn build_ordering_key(column_ids: &[i64]) -> String {
    column_ids
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
