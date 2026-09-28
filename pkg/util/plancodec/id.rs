// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 这段逻辑只描述 plancodec 中物理执行计划类型字符串和稳定数字 ID 的互相映射，
// strconv 只用于 Go 里的 "UnknownPlanID" + strconv.Itoa(id) 拼接；在对应分支用 format! 表达。

// 下列 Type* 常量对应 Go 的第一个 const 块，用于表示执行计划节点的字符串类型名。
// 这些字符串会参与执行计划编码/解码，迁移时保持原始拼写、顺序和英文注释。
// TypeSel is the type of Selection.

// 物理执行计划类型名与稳定数字 ID 的双向映射。
//
// 对应 Go `pkg/util/plancodec` 中的类型字符串常量与 plan id。
// 这些 ID 参与执行计划（optimizer 产出的算子树）的二进制编码/解码，
// 数值一旦发布不可随意改动，否则旧编码无法正确还原。

/// Selection（过滤）算子的类型名字符串。
pub const TypeSel: &str = "Selection";
// TypeSet is the type of Set.
pub const TypeSet: &str = "Set";
// TypeProj is the type of Projection.
pub const TypeProj: &str = "Projection";
// TypeAgg is the type of Aggregation.
pub const TypeAgg: &str = "Aggregation";
// TypeStreamAgg is the type of StreamAgg.
pub const TypeStreamAgg: &str = "StreamAgg";
// TypeHashAgg is the type of HashAgg.
pub const TypeHashAgg: &str = "HashAgg";
// TypeShow is the type of show.
pub const TypeShow: &str = "Show";
// TypeJoin is the type of Join.
pub const TypeJoin: &str = "Join";
// TypeUnion is the type of Union.
pub const TypeUnion: &str = "Union";
// TypePartitionUnion is the type of PartitionUnion
pub const TypePartitionUnion: &str = "PartitionUnion";
// TypeTableScan is the type of TableScan.
pub const TypeTableScan: &str = "TableScan";
// TypeMemTableScan is the type of TableScan.
pub const TypeMemTableScan: &str = "MemTableScan";
// TypeUnionScan is the type of UnionScan.
pub const TypeUnionScan: &str = "UnionScan";
// TypeIdxScan is the type of IndexScan.
pub const TypeIdxScan: &str = "IndexScan";
// TypeSort is the type of Sort.
pub const TypeSort: &str = "Sort";
// TypeTopN is the type of TopN.
pub const TypeTopN: &str = "TopN";
// TypeLimit is the type of Limit.
pub const TypeLimit: &str = "Limit";
// TypeHashJoin is the type of hash join.
pub const TypeHashJoin: &str = "HashJoin";
// TypeExchangeSender is the type of mpp exchanger sender.
pub const TypeExchangeSender: &str = "ExchangeSender";
// TypeExchangeReceiver is the type of mpp exchanger receiver.
pub const TypeExchangeReceiver: &str = "ExchangeReceiver";
// TypeExpand is the type of mpp expand source operator.
pub const TypeExpand: &str = "Expand";
// TypeMergeJoin is the type of merge join.
pub const TypeMergeJoin: &str = "MergeJoin";
// TypeIndexJoin is the type of index look up join.
pub const TypeIndexJoin: &str = "IndexJoin";
// TypeIndexMergeJoin is the type of index look up merge join.
pub const TypeIndexMergeJoin: &str = "IndexMergeJoin";
// TypeIndexHashJoin is the type of index nested loop hash join.
pub const TypeIndexHashJoin: &str = "IndexHashJoin";
// TypeApply is the type of Apply.
pub const TypeApply: &str = "Apply";
// TypeMaxOneRow is the type of MaxOneRow.
pub const TypeMaxOneRow: &str = "MaxOneRow";
// TypeExists is the type of Exists.
pub const TypeExists: &str = "Exists";
// TypeDual is the type of TableDual.
pub const TypeDual: &str = "TableDual";
// TypeLock is the type of SelectLock.
pub const TypeLock: &str = "SelectLock";
// TypeInsert is the type of Insert
pub const TypeInsert: &str = "Insert";
// TypeUpdate is the type of Update.
pub const TypeUpdate: &str = "Update";
// TypeDelete is the type of Delete.
pub const TypeDelete: &str = "Delete";
// TypeIndexLookUp is the type of IndexLookUp.
pub const TypeIndexLookUp: &str = "IndexLookUp";
// TypeLocalIndexLookUp is the type of LocalIndexLookUp.
pub const TypeLocalIndexLookUp: &str = "LocalIndexLookUp";
// TypeTableReader is the type of TableReader.
pub const TypeTableReader: &str = "TableReader";
// TypeIndexReader is the type of IndexReader.
pub const TypeIndexReader: &str = "IndexReader";
// TypeWindow is the type of Window.
pub const TypeWindow: &str = "Window";
// TypeShuffle is the type of Shuffle.
pub const TypeShuffle: &str = "Shuffle";
// TypeShuffleReceiver is the type of Shuffle.
pub const TypeShuffleReceiver: &str = "ShuffleReceiver";
// TypeTiKVSingleGather is the type of TiKVSingleGather.
pub const TypeTiKVSingleGather: &str = "TiKVSingleGather";
// TypeIndexMerge is the type of IndexMergeReader
pub const TypeIndexMerge: &str = "IndexMerge";
// TypePointGet is the type of PointGetPlan.
pub const TypePointGet: &str = "Point_Get";
// TypeShowDDLJobs is the type of show ddl jobs.
pub const TypeShowDDLJobs: &str = "ShowDDLJobs";
// TypeBatchPointGet is the type of BatchPointGetPlan.
pub const TypeBatchPointGet: &str = "Batch_Point_Get";
// TypeClusterMemTableReader is the type of TableReader.
pub const TypeClusterMemTableReader: &str = "ClusterMemTableReader";
// TypeDataSource is the type of DataSource.
pub const TypeDataSource: &str = "DataSource";
// TypeLoadData is the type of LoadData.
pub const TypeLoadData: &str = "LoadData";
// TypeTableSample is the type of TableSample.
pub const TypeTableSample: &str = "TableSample";
// TypeTableFullScan is the type of TableFullScan.
pub const TypeTableFullScan: &str = "TableFullScan";
// TypeTableRangeScan is the type of TableRangeScan.
pub const TypeTableRangeScan: &str = "TableRangeScan";
// TypeTableRowIDScan is the type of TableRowIDScan.
pub const TypeTableRowIDScan: &str = "TableRowIDScan";
// TypeIndexFullScan is the type of IndexFullScan.
pub const TypeIndexFullScan: &str = "IndexFullScan";
// TypeIndexRangeScan is the type of IndexRangeScan.
pub const TypeIndexRangeScan: &str = "IndexRangeScan";
// TypeCTETable is the type of TypeCTETable.
pub const TypeCTETable: &str = "CTETable";
// TypeCTE is the type of CTEFullScan.
pub const TypeCTE: &str = "CTEFullScan";
// TypeCTEDefinition is the type of CTE definition
pub const TypeCTEDefinition: &str = "CTE";
// TypeForeignKeyCheck is the type of FKCheck
pub const TypeForeignKeyCheck: &str = "Foreign_Key_Check";
// TypeForeignKeyCascade is the type of FKCascade
pub const TypeForeignKeyCascade: &str = "Foreign_Key_Cascade";
// TypeImportInto is the type of ImportInto.
pub const TypeImportInto: &str = "ImportInto";
// TypeSequence is the type of Sequence
// Go 源文件只声明了 Sequence 字符串，下面的物理 ID 映射没有为它分配 ID；这里不额外补造映射。
pub const TypeSequence: &str = "Sequence";
// TypeScalarSubQuery is the type of ScalarQuery
pub const TypeScalarSubQuery: &str = "ScalarSubQuery";
// TypePhysicalCTESink is the type of CTE sink.
pub const TypePhysicalCTESink: &str = "PhysicalCTESink";
// TypePhysicalCTESource is the type of CTE source.
pub const TypePhysicalCTESource: &str = "PhysicalCTESource";

// plan id.
// Attention: for compatibility of encode/decode plan, The plan id shouldn't be changed.
// 这一组常量对应 Go 的第二个 const 块，是执行计划二进制编码兼容性的核心。
// 保持 Go int 的角色，用 isize 表达“机器字长整数”的近似含义；数值不能随意重排或改动。
const typeSelID: isize = 1;
const typeSetID: isize = 2;
const typeProjID: isize = 3;
const typeAggID: isize = 4;
const typeStreamAggID: isize = 5;
const typeHashAggID: isize = 6;
const typeShowID: isize = 7;
const typeJoinID: isize = 8;
const typeUnionID: isize = 9;
const typeTableScanID: isize = 10;
const typeMemTableScanID: isize = 11;
const typeUnionScanID: isize = 12;
const typeIdxScanID: isize = 13;
const typeSortID: isize = 14;
const typeTopNID: isize = 15;
const typeLimitID: isize = 16;
const typeHashJoinID: isize = 17;
const typeMergeJoinID: isize = 18;
const typeIndexJoinID: isize = 19;
const typeIndexMergeJoinID: isize = 20;
const typeIndexHashJoinID: isize = 21;
const typeApplyID: isize = 22;
const typeMaxOneRowID: isize = 23;
const typeExistsID: isize = 24;
const typeDualID: isize = 25;
const typeLockID: isize = 26;
const typeInsertID: isize = 27;
const typeUpdateID: isize = 28;
const typeDeleteID: isize = 29;
const typeIndexLookUpID: isize = 30;
const typeTableReaderID: isize = 31;
const typeIndexReaderID: isize = 32;
const typeWindowID: isize = 33;
const typeTiKVSingleGatherID: isize = 34;
const typeIndexMergeID: isize = 35;
const typePointGet: isize = 36;
const typeShowDDLJobs: isize = 37;
const typeBatchPointGet: isize = 38;
const typeClusterMemTableReader: isize = 39;
const typeDataSourceID: isize = 40;
const typeLoadDataID: isize = 41;
const typeTableSampleID: isize = 42;
const typeTableFullScanID: isize = 43;
const typeTableRangeScanID: isize = 44;
const typeTableRowIDScanID: isize = 45;
const typeIndexFullScanID: isize = 46;
const typeIndexRangeScanID: isize = 47;
const typeExchangeReceiverID: isize = 48;
const typeExchangeSenderID: isize = 49;
const typeCTEID: isize = 50;
const typeCTEDefinitionID: isize = 51;
const typeCTETableID: isize = 52;
const typePartitionUnionID: isize = 53;
const typeShuffleID: isize = 54;
const typeShuffleReceiverID: isize = 55;
const typeForeignKeyCheck: isize = 56;
const typeForeignKeyCascade: isize = 57;
const typeExpandID: isize = 58;
const typeImportIntoID: isize = 59;
/// ScalarSubQuery 的稳定物理计划 ID（对外可见）。
pub const TypeScalarSubQueryID: isize = 60;
const typeLocalIndexLookUpID: isize = 61;
const typePhysicalCTESinkID: isize = 62;
const typePhysicalCTESourceID: isize = 63;

// TypeStringToPhysicalID converts the plan type string to plan id.
// TypeStringToPhysicalID 对应 Go 的同名函数，把计划类型字符串转换成稳定的物理计划 ID。
// 这里不做解析、IO 或外部依赖调用，只按上方常量表执行 Go switch 的等价分支。
/// 将计划类型字符串转换为稳定的物理计划 ID；未知类型返回 0。
pub fn TypeStringToPhysicalID(tp: &str) -> isize {
    // 每个 match guard 对应 Go 源码中的一个 `case TypeXxx`，顺序保持一致，便于审查兼容性 ID。
    match tp {
        x if x == TypeSel => typeSelID,
        x if x == TypeSet => typeSetID,
        x if x == TypeProj => typeProjID,
        x if x == TypeAgg => typeAggID,
        x if x == TypeStreamAgg => typeStreamAggID,
        x if x == TypeHashAgg => typeHashAggID,
        x if x == TypeShow => typeShowID,
        x if x == TypeJoin => typeJoinID,
        x if x == TypeUnion => typeUnionID,
        x if x == TypePartitionUnion => typePartitionUnionID,
        x if x == TypeTableScan => typeTableScanID,
        x if x == TypeMemTableScan => typeMemTableScanID,
        x if x == TypeUnionScan => typeUnionScanID,
        x if x == TypeIdxScan => typeIdxScanID,
        x if x == TypeSort => typeSortID,
        x if x == TypeTopN => typeTopNID,
        x if x == TypeLimit => typeLimitID,
        x if x == TypeHashJoin => typeHashJoinID,
        x if x == TypeMergeJoin => typeMergeJoinID,
        x if x == TypeIndexJoin => typeIndexJoinID,
        x if x == TypeIndexMergeJoin => typeIndexMergeJoinID,
        x if x == TypeIndexHashJoin => typeIndexHashJoinID,
        x if x == TypeApply => typeApplyID,
        x if x == TypeMaxOneRow => typeMaxOneRowID,
        x if x == TypeExists => typeExistsID,
        x if x == TypeDual => typeDualID,
        x if x == TypeLock => typeLockID,
        x if x == TypeInsert => typeInsertID,
        x if x == TypeUpdate => typeUpdateID,
        x if x == TypeDelete => typeDeleteID,
        x if x == TypeIndexLookUp => typeIndexLookUpID,
        x if x == TypeLocalIndexLookUp => typeLocalIndexLookUpID,
        x if x == TypeTableReader => typeTableReaderID,
        x if x == TypeIndexReader => typeIndexReaderID,
        x if x == TypeWindow => typeWindowID,
        x if x == TypeShuffle => typeShuffleID,
        x if x == TypeShuffleReceiver => typeShuffleReceiverID,
        x if x == TypeTiKVSingleGather => typeTiKVSingleGatherID,
        x if x == TypeIndexMerge => typeIndexMergeID,
        x if x == TypePointGet => typePointGet,
        x if x == TypeShowDDLJobs => typeShowDDLJobs,
        x if x == TypeBatchPointGet => typeBatchPointGet,
        x if x == TypeClusterMemTableReader => typeClusterMemTableReader,
        x if x == TypeDataSource => typeDataSourceID,
        x if x == TypeLoadData => typeLoadDataID,
        x if x == TypeTableSample => typeTableSampleID,
        x if x == TypeTableFullScan => typeTableFullScanID,
        x if x == TypeTableRangeScan => typeTableRangeScanID,
        x if x == TypeTableRowIDScan => typeTableRowIDScanID,
        x if x == TypeIndexFullScan => typeIndexFullScanID,
        x if x == TypeIndexRangeScan => typeIndexRangeScanID,
        x if x == TypeExchangeReceiver => typeExchangeReceiverID,
        x if x == TypeExchangeSender => typeExchangeSenderID,
        x if x == TypeCTE => typeCTEID,
        x if x == TypeCTEDefinition => typeCTEDefinitionID,
        x if x == TypeCTETable => typeCTETableID,
        x if x == TypeForeignKeyCheck => typeForeignKeyCheck,
        x if x == TypeForeignKeyCascade => typeForeignKeyCascade,
        x if x == TypeExpand => typeExpandID,
        x if x == TypeImportInto => typeImportIntoID,
        x if x == TypeScalarSubQuery => TypeScalarSubQueryID,
        x if x == TypePhysicalCTESink => typePhysicalCTESinkID,
        x if x == TypePhysicalCTESource => typePhysicalCTESourceID,
        _ => {
            // Should never reach here.
            // Go 源码在未知计划字符串时返回 0；不改成 Result，避免改变原有调用契约。
            0
        }
    }
}

// PhysicalIDToTypeString converts the plan id to plan type string.
// PhysicalIDToTypeString 对应 Go 的同名函数，把兼容性物理计划 ID 转回计划类型字符串。
// 由于 Go 默认分支会拼接未知 ID，返回 String，以同时表达静态常量和动态 fallback。
/// 将物理计划 ID 转回类型字符串；未知 ID 返回 `UnknownPlanID{id}`。
pub fn PhysicalIDToTypeString(id: isize) -> String {
    // 每个分支对应 Go 的 `case typeXxxID`，保持原顺序以降低人工核对 ID 映射时的认知负担。
    match id {
        x if x == typeSelID => TypeSel.to_string(),
        x if x == typeSetID => TypeSet.to_string(),
        x if x == typeProjID => TypeProj.to_string(),
        x if x == typeAggID => TypeAgg.to_string(),
        x if x == typeStreamAggID => TypeStreamAgg.to_string(),
        x if x == typeHashAggID => TypeHashAgg.to_string(),
        x if x == typeShowID => TypeShow.to_string(),
        x if x == typeJoinID => TypeJoin.to_string(),
        x if x == typeUnionID => TypeUnion.to_string(),
        x if x == typePartitionUnionID => TypePartitionUnion.to_string(),
        x if x == typeTableScanID => TypeTableScan.to_string(),
        x if x == typeMemTableScanID => TypeMemTableScan.to_string(),
        x if x == typeUnionScanID => TypeUnionScan.to_string(),
        x if x == typeIdxScanID => TypeIdxScan.to_string(),
        x if x == typeSortID => TypeSort.to_string(),
        x if x == typeTopNID => TypeTopN.to_string(),
        x if x == typeLimitID => TypeLimit.to_string(),
        x if x == typeHashJoinID => TypeHashJoin.to_string(),
        x if x == typeMergeJoinID => TypeMergeJoin.to_string(),
        x if x == typeIndexJoinID => TypeIndexJoin.to_string(),
        x if x == typeIndexMergeJoinID => TypeIndexMergeJoin.to_string(),
        x if x == typeIndexHashJoinID => TypeIndexHashJoin.to_string(),
        x if x == typeApplyID => TypeApply.to_string(),
        x if x == typeMaxOneRowID => TypeMaxOneRow.to_string(),
        x if x == typeExistsID => TypeExists.to_string(),
        x if x == typeDualID => TypeDual.to_string(),
        x if x == typeLockID => TypeLock.to_string(),
        x if x == typeInsertID => TypeInsert.to_string(),
        x if x == typeUpdateID => TypeUpdate.to_string(),
        x if x == typeDeleteID => TypeDelete.to_string(),
        x if x == typeIndexLookUpID => TypeIndexLookUp.to_string(),
        x if x == typeLocalIndexLookUpID => TypeLocalIndexLookUp.to_string(),
        x if x == typeTableReaderID => TypeTableReader.to_string(),
        x if x == typeIndexReaderID => TypeIndexReader.to_string(),
        x if x == typeWindowID => TypeWindow.to_string(),
        x if x == typeShuffleID => TypeShuffle.to_string(),
        x if x == typeShuffleReceiverID => TypeShuffleReceiver.to_string(),
        x if x == typeTiKVSingleGatherID => TypeTiKVSingleGather.to_string(),
        x if x == typeIndexMergeID => TypeIndexMerge.to_string(),
        x if x == typePointGet => TypePointGet.to_string(),
        x if x == typeShowDDLJobs => TypeShowDDLJobs.to_string(),
        x if x == typeBatchPointGet => TypeBatchPointGet.to_string(),
        x if x == typeClusterMemTableReader => TypeClusterMemTableReader.to_string(),
        x if x == typeDataSourceID => TypeDataSource.to_string(),
        x if x == typeLoadDataID => TypeLoadData.to_string(),
        x if x == typeTableSampleID => TypeTableSample.to_string(),
        x if x == typeTableFullScanID => TypeTableFullScan.to_string(),
        x if x == typeTableRangeScanID => TypeTableRangeScan.to_string(),
        x if x == typeTableRowIDScanID => TypeTableRowIDScan.to_string(),
        x if x == typeIndexFullScanID => TypeIndexFullScan.to_string(),
        x if x == typeIndexRangeScanID => TypeIndexRangeScan.to_string(),
        x if x == typeExchangeReceiverID => TypeExchangeReceiver.to_string(),
        x if x == typeExchangeSenderID => TypeExchangeSender.to_string(),
        x if x == typeCTEID => TypeCTE.to_string(),
        x if x == typeCTEDefinitionID => TypeCTEDefinition.to_string(),
        x if x == typeCTETableID => TypeCTETable.to_string(),
        x if x == typeForeignKeyCheck => TypeForeignKeyCheck.to_string(),
        x if x == typeForeignKeyCascade => TypeForeignKeyCascade.to_string(),
        x if x == typeExpandID => TypeExpand.to_string(),
        x if x == typeImportIntoID => TypeImportInto.to_string(),
        x if x == TypeScalarSubQueryID => TypeScalarSubQuery.to_string(),
        x if x == typePhysicalCTESinkID => TypePhysicalCTESink.to_string(),
        x if x == typePhysicalCTESourceID => TypePhysicalCTESource.to_string(),
        _ => {
            // Should never reach here.
            // Go 使用 strconv.Itoa(id) 拼接未知 ID；这里用 format! 保留同样的可观察字符串形状。
            format!("UnknownPlanID{}", id)
        }
    }
}
