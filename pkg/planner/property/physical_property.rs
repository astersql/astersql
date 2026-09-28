// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 物理属性（PhysicalProperty）：父算子对子计划输出的物理要求。
//
// 涵盖排序项、任务类型（root/cop/mpp）、期望行数、MPP 分区方式与分区列、
// CTE/向量检索/IndexJoin 运行时属性，以及是否禁止下推等。物理属性匹配失败时
// 可插入 Enforcer（如 Sort、Exchange）强制满足。

use std::any::Any;
use std::fmt;
use std::mem;
use std::sync::{Arc, OnceLock};

use crate::{
    CopMultiReadTaskType, CopSingleReadTaskType, MppTaskType, RootTaskType, TaskType, base, codec,
    collate, expression, funcdep, intset, size,
};

/// 根任务可接受的子任务类型全集：单读 Cop、多读 Cop、Root。
pub static wholeTaskTypes: [TaskType; 3] =
    [CopSingleReadTaskType, CopMultiReadTaskType, RootTaskType];

/// 排序项：一列及其升/降序。
#[derive(Clone)]
pub struct SortItem {
    /// 参与排序的列。
    pub Col: expression::Column,
    /// 是否降序。
    pub Desc: bool,
}

impl SortItem {
    /// 将列与升降序写入 Hasher（用于计划指纹）。
    pub fn Hash64(&self, hasher: &mut dyn base::Hasher) {
        hasher.HashByte(base::NotNilFlag);
        self.Col.Hash64(hasher);
        hasher.HashBool(self.Desc);
    }

    /// 比较列与升降序是否均相等。
    pub fn EqualsSortItem(&self, other: &SortItem) -> bool {
        self.Col.Equals(&other.Col) && self.Desc == other.Desc
    }

    /// 格式化为 `{col asc|desc}`。
    pub fn String(&self) -> String {
        format!(
            "{{{} {}}}",
            self.Col.String(),
            if self.Desc { "desc" } else { "asc" }
        )
    }

    /// 深拷贝排序项。
    pub fn Clone(&self) -> SortItem {
        SortItem {
            Col: self.Col.Clone(),
            Desc: self.Desc,
        }
    }

    /// 估算内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        size::SizeOfBool + self.Col.MemoryUsage()
    }
}

impl base::Hash64 for SortItem {
    fn Hash64(&self, hasher: &mut dyn base::Hasher) {
        SortItem::Hash64(self, hasher);
    }
}

impl base::Equals for SortItem {
    fn Equals(&self, other: &dyn Any) -> bool {
        other
            .downcast_ref::<SortItem>()
            .is_some_and(|other| self.EqualsSortItem(other))
    }
}

impl fmt::Debug for SortItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.String())
    }
}

impl fmt::Display for SortItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.String())
    }
}

/// 将窗口/分区的 PARTITION BY 排序项写入 Explain 缓冲区。
pub fn ExplainPartitionBy(
    ctx: &dyn expression::EvalContext,
    buffer: &mut String,
    partition_by: &[SortItem],
    normalized: bool,
) {
    if partition_by.is_empty() {
        return;
    }
    buffer.push_str("partition by ");
    for (index, item) in partition_by.iter().enumerate() {
        if normalized {
            buffer.push_str(expression::Expression::ExplainNormalizedInfo(&item.Col).as_str());
        } else {
            buffer.push_str(expression::Expression::ExplainInfo(&item.Col, ctx).as_str());
        }
        if index + 1 < partition_by.len() {
            buffer.push_str(", ");
        }
    }
}

/// MPP 分区类型：描述 Exchange 如何在各节点间分布数据。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct MPPPartitionType(pub i32);

/// 任意分区（无额外约束）。
pub const AnyType: MPPPartitionType = MPPPartitionType(0);
/// 广播分区（Broadcast）。
pub const BroadcastType: MPPPartitionType = MPPPartitionType(1);
/// 按哈希键分区（Hash / Shuffle）。
pub const HashType: MPPPartitionType = MPPPartitionType(2);
/// 单分区（全部数据集中到一个节点，PassThrough）。
pub const SinglePartitionType: MPPPartitionType = MPPPartitionType(3);

impl MPPPartitionType {
    /// 与 `AnyType` 同义。
    pub const Any: Self = AnyType;
    /// 与 `BroadcastType` 同义。
    pub const Broadcast: Self = BroadcastType;
    /// 与 `HashType` 同义。
    pub const Hash: Self = HashType;
    /// 与 `SinglePartitionType` 同义。
    pub const SinglePartition: Self = SinglePartitionType;

    /// 映射为 tipb ExchangeType；Any 视为非法并回退 PassThrough。
    pub fn ToExchangeType(self) -> tipb::ExchangeType {
        match self {
            BroadcastType => tipb::ExchangeType::Broadcast,
            HashType => tipb::ExchangeType::Hash,
            SinglePartitionType => tipb::ExchangeType::PassThrough,
            _ => {
                log::warn!("generate an exchange with any partition type, which is illegal");
                tipb::ExchangeType::PassThrough
            }
        }
    }
}

/// MPP 哈希分区所用的一列及其排序规则 ID。
#[derive(Clone)]
pub struct MPPPartitionColumn {
    /// 分区键列。
    pub Col: expression::Column,
    /// 排序规则 ID；负值表示新 collation 编码。
    pub CollateID: i32,
}

impl MPPPartitionColumn {
    /// 将列下标解析到给定 Schema。
    pub fn ResolveIndices(
        &self,
        schema: &expression::Schema,
    ) -> Result<MPPPartitionColumn, expression::Error> {
        Ok(MPPPartitionColumn {
            Col: self.Col.ResolveIndices(schema)?,
            CollateID: self.CollateID,
        })
    }

    /// 深拷贝。
    pub fn Clone(&self) -> MPPPartitionColumn {
        MPPPartitionColumn {
            Col: self.Col.Clone(),
            CollateID: self.CollateID,
        }
    }

    /// 计算指纹字节：列 HashCode + collation 标记。
    fn hashCode(&self) -> Vec<u8> {
        let mut column = self.Col.Clone();
        let mut hashcode = column.HashCode();
        hashcode = codec::EncodeInt(
            hashcode,
            if self.CollateID < 0 {
                i64::from(self.CollateID)
            } else {
                1
            },
        );
        hashcode
    }

    /// 比较分区列：新 collation 时要求 CollateID 一致，再比列相等。
    pub fn Equal(&self, other: &MPPPartitionColumn) -> bool {
        if self.CollateID < 0 && self.CollateID != other.CollateID {
            return false;
        }
        self.Col.EqualColumn(&other.Col)
    }

    /// 估算内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        size::SizeOfInt32 + self.Col.MemoryUsage()
    }
}

/// 按匹配下标从分区键中选出子集。
pub fn ChoosePartitionKeys(
    keys: &[MPPPartitionColumn],
    matches: &[usize],
) -> Vec<MPPPartitionColumn> {
    matches.iter().map(|&index| keys[index].Clone()).collect()
}

/// 将 MPP 分区列列表格式化为 Explain 用的字节串（含 collation 名）。
pub fn ExplainColumnList(
    ctx: &dyn expression::EvalContext,
    columns: &[MPPPartitionColumn],
) -> Vec<u8> {
    let mut buffer = String::new();
    for (index, column) in columns.iter().enumerate() {
        buffer.push_str("[name: ");
        buffer.push_str(expression::Expression::ExplainInfo(&column.Col, ctx).as_str());
        buffer.push_str(", collate: ");
        if collate::NewCollationEnabled() {
            buffer.push_str(GetCollateNameByIDForPartition(column.CollateID).as_str());
        } else {
            buffer.push_str("N/A");
        }
        buffer.push(']');
        if index + 1 < columns.len() {
            buffer.push_str(", ");
        }
    }
    buffer.into_bytes()
}

/// 分区场景下：排序规则名 → 经 rewrite 的 collation ID。
pub fn GetCollateIDByNameForPartition(collation: &str) -> i32 {
    collate::RewriteNewCollationIDIfNeeded(collate::CollationName2ID(collation))
}

/// 分区场景下：collation ID → 名称（先 restore 再查表）。
pub fn GetCollateNameByIDForPartition(collation_id: i32) -> String {
    collate::CollationID2Name(collate::RestoreCollationIDIfNeeded(collation_id))
}

/// CTE producer 在 MPP 下的可达状态。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct cteProducerStatus(pub i32);

/// 无 CTE，或所有 producer 均可 MPP。
pub const NoCTEOrAllProducerCanMPP: cteProducerStatus = cteProducerStatus(0);
/// 部分 CTE 无法走 MPP。
pub const SomeCTEFailedMpp: cteProducerStatus = cteProducerStatus(1);
/// 所有 CTE 均可 MPP。
pub const AllCTECanMpp: cteProducerStatus = cteProducerStatus(2);

/// 物理属性匹配结果。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct PhysicalPropMatchResult(pub i32);

/// 未匹配。
pub const PropNotMatched: PhysicalPropMatchResult = PhysicalPropMatchResult(0);
/// 完全匹配。
pub const PropMatched: PhysicalPropMatchResult = PhysicalPropMatchResult(1);
/// 匹配但需 MergeSort 合并有序流。
pub const PropMatchedNeedMergeSort: PhysicalPropMatchResult = PhysicalPropMatchResult(2);

impl PhysicalPropMatchResult {
    /// 是否视为已匹配（含需 MergeSort）。
    pub fn Matched(self) -> bool {
        self == PropMatched || self == PropMatchedNeedMergeSort
    }
}

/// 部分有序信息：索引扫描等可提供的前缀有序性。
#[derive(Clone, Debug, Default)]
pub struct PartialOrderInfo {
    /// 已知有序的排序项前缀。
    pub SortItems: Vec<SortItem>,
}

impl PartialOrderInfo {
    /// 返回 (是否全部同向, 若同向则是否降序)。
    pub fn AllSameOrder(&self) -> (bool, bool) {
        let Some(first) = self.SortItems.first() else {
            return (true, false);
        };
        if self
            .SortItems
            .windows(2)
            .any(|items| items[0].Desc != items[1].Desc)
        {
            return (false, false);
        }
        (true, first.Desc)
    }

    /// 估算内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        mem::size_of::<PartialOrderInfo>() as i64
            + self.SortItems.capacity() as i64 * size::SizeOfPointer
            + self
                .SortItems
                .iter()
                .map(SortItem::MemoryUsage)
                .sum::<i64>()
    }
}

/// 部分有序匹配结果：是否匹配、前缀列与前缀长度。
#[derive(Clone, Default)]
pub struct PartialOrderMatchResult {
    /// 是否匹配成功。
    pub Matched: bool,
    /// 匹配前缀的末列（可选）。
    pub PrefixCol: Option<expression::Column>,
    /// 匹配前缀长度。
    pub PrefixLen: isize,
}

impl PartialOrderMatchResult {
    /// 估算内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        mem::size_of::<PartialOrderMatchResult>() as i64
            + self
                .PrefixCol
                .as_ref()
                .map_or(0, expression::Column::MemoryUsage)
    }
}

/// IndexJoin 运行时属性：连接条件、内外键与平均内表行数等。
#[derive(Clone)]
pub struct IndexJoinRuntimeProp {
    /// 非等值连接条件。
    pub OtherConditions: Vec<expression::ExprBox>,
    /// 外表连接键。
    pub OuterJoinKeys: Vec<expression::Column>,
    /// 内表连接键。
    pub InnerJoinKeys: Vec<expression::Column>,
    /// 每个外表行对应的平均内表行数。
    pub AvgInnerRowCnt: f64,
    /// 内表是否走 TableRangeScan。
    pub TableRangeScan: bool,
}

impl Default for IndexJoinRuntimeProp {
    fn default() -> Self {
        Self {
            OtherConditions: Vec::new(),
            OuterJoinKeys: Vec::new(),
            InnerJoinKeys: Vec::new(),
            AvgInnerRowCnt: 0.0,
            TableRangeScan: false,
        }
    }
}

impl IndexJoinRuntimeProp {
    /// 克隆关键字段（当前实现为整结构 clone）。
    pub fn CloneEssentialFields(&self) -> IndexJoinRuntimeProp {
        self.clone()
    }
}

/// 向量检索信息：距离函数、protobuf 签名、查询向量与列。
#[derive(Clone)]
pub struct VectorSearchInfo {
    /// 距离函数名。
    pub DistanceFnName: String,
    /// tipb 标量函数签名。
    pub FnPbCode: tipb::ScalarFuncSig,
    /// 查询向量。
    pub Vec: Arc<expression::types::VectorFloat32>,
    /// 向量列。
    pub Column: expression::Column,
}

/// 向量属性：可选的向量检索信息与 TopK。
#[derive(Clone, Default)]
pub struct VectorProperty {
    /// 向量检索详情。
    pub VSInfo: Option<VectorSearchInfo>,
    /// Top-K 截断。
    pub TopK: u32,
}

/// 物理属性：父算子对子计划的排序、任务、分区等要求。
#[derive(Clone)]
pub struct PhysicalProperty {
    /// 要求的排序项。
    pub SortItems: Vec<SortItem>,
    /// 要求的任务类型。
    pub TaskTp: TaskType,
    /// 期望输出行数（用于 Limit 下推等）。
    pub ExpectedCnt: f64,
    /// 缓存的属性指纹。
    pub(crate) hashcode: OnceLock<Vec<u8>>,
    /// 是否允许插入 Enforcer（如 Sort）来满足本属性。
    pub CanAddEnforcer: bool,
    /// MPP 哈希分区列。
    pub MPPPartitionCols: Vec<MPPPartitionColumn>,
    /// MPP 分区类型。
    pub MPPPartitionTp: MPPPartitionType,
    /// 分区相关的排序项（窗口等）。
    pub SortItemsForPartition: Vec<SortItem>,
    /// CTE producer 的 MPP 可达状态。
    pub CTEProducerStatus: cteProducerStatus,
    /// 向量检索相关属性。
    pub VectorProp: VectorProperty,
    /// IndexJoin 运行时属性。
    pub IndexJoinProp: Option<IndexJoinRuntimeProp>,
    /// 禁止下推到 Cop/MPP。
    pub NoCopPushDown: bool,
    /// 部分有序信息。
    pub PartialOrderInfo: Option<PartialOrderInfo>,
    /// 建议性排序项（不强制，仅提示）。
    pub AdvisorySortItems: Vec<SortItem>,
    /// 标量聚合等可从 TiFlash 向量化执行获益时优先枚举 TiFlash。
    pub PreferTiFlash: bool,
}

impl Default for PhysicalProperty {
    fn default() -> Self {
        Self {
            SortItems: Vec::new(),
            TaskTp: RootTaskType,
            ExpectedCnt: 0.0,
            hashcode: OnceLock::new(),
            CanAddEnforcer: false,
            MPPPartitionCols: Vec::new(),
            MPPPartitionTp: AnyType,
            SortItemsForPartition: Vec::new(),
            CTEProducerStatus: NoCTEOrAllProducerCanMPP,
            VectorProp: VectorProperty::default(),
            IndexJoinProp: None,
            NoCopPushDown: false,
            PartialOrderInfo: None,
            AdvisorySortItems: Vec::new(),
            PreferTiFlash: false,
        }
    }
}

/// 由任务类型、排序列、期望行数与是否可强制构造物理属性。
pub fn NewPhysicalProperty(
    task_type: TaskType,
    columns: &[expression::Column],
    desc: bool,
    expected_count: f64,
    enforced: bool,
) -> Box<PhysicalProperty> {
    Box::new(PhysicalProperty {
        SortItems: SortItemsFromCols(columns, desc),
        TaskTp: task_type,
        ExpectedCnt: expected_count,
        CanAddEnforcer: enforced,
        ..PhysicalProperty::default()
    })
}

/// 将列列表转为统一升降序的 SortItem 向量。
pub fn SortItemsFromCols(columns: &[expression::Column], desc: bool) -> Vec<SortItem> {
    columns
        .iter()
        .map(|column| SortItem {
            Col: column.Clone(),
            Desc: desc,
        })
        .collect()
}

impl PhysicalProperty {
    /// 若本属性的 MPP 分区列是 keys 的子集，返回各列在 keys 中的下标；否则 None。
    pub fn IsSubsetOf(&self, keys: &[MPPPartitionColumn]) -> Option<Vec<usize>> {
        if self.MPPPartitionCols.len() > keys.len() {
            return None;
        }
        let mut matches = Vec::with_capacity(keys.len());
        for partition_column in &self.MPPPartitionCols {
            let index = keys.iter().position(|key| partition_column.Equal(key))?;
            matches.push(index);
        }
        Some(matches)
    }

    /// 基于函数依赖等价类判断是否仍需 MPP Exchange。
    /// 若当前分区列无法被要求的分区列（及其等价闭包）覆盖，则需要 Exchange。
    pub fn NeedMPPExchangeByEquivalence(
        &self,
        current_partition_columns: &[MPPPartitionColumn],
        dependencies: &funcdep::FDSet,
    ) -> bool {
        // 为每个要求的分区列计算等价闭包，供后续匹配。
        let alternatives = self
            .MPPPartitionCols
            .iter()
            .map(|column| {
                (
                    column,
                    dependencies.ClosureOfEquivalence(intset::NewFastIntSet(vec![
                        column.Col.UniqueID as i32,
                    ])),
                )
            })
            .collect::<Vec<_>>();

        for key in current_partition_columns {
            if !alternatives
                .iter()
                .any(|(partition_column, set)| checkEquivalence(set, key, partition_column))
            {
                return true;
            }
        }
        false
    }

    /// 排序列是否全部来自给定 Schema。
    pub fn AllColsFromSchema(&self, schema: &expression::Schema) -> bool {
        self.SortItems
            .iter()
            .all(|item| schema.ColumnIndex(&item.Col).is_some())
    }

    /// 是否为 Flash/MPP 属性（TaskTp == Mpp）。
    pub fn IsFlashProp(&self) -> bool {
        self.TaskTp == MppTaskType
    }

    /// 根任务可接受全部子任务类型；否则仅接受自身 TaskTp。
    pub fn GetAllPossibleChildTaskTypes(&self) -> Vec<TaskType> {
        if self.TaskTp == RootTaskType {
            wholeTaskTypes.to_vec()
        } else {
            vec![self.TaskTp]
        }
    }

    /// 本属性的排序项是否为 other 的前缀（列与升降序均一致）。
    pub fn IsPrefix(&self, other: &PhysicalProperty) -> bool {
        self.SortItems.len() <= other.SortItems.len()
            && self
                .SortItems
                .iter()
                .zip(&other.SortItems)
                .all(|(left, right)| left.Col.EqualColumn(&right.Col) && left.Desc == right.Desc)
    }

    /// SortItemsForPartition 是否与 SortItems 完全一致。
    pub fn IsSortItemAllForPartition(&self) -> bool {
        self.SortItemsForPartition.len() == self.SortItems.len()
            && self
                .SortItemsForPartition
                .iter()
                .zip(&self.SortItems)
                .all(|(left, right)| left.Col.EqualColumn(&right.Col) && left.Desc == right.Desc)
    }

    /// 是否无排序要求。
    pub fn IsSortItemEmpty(&self) -> bool {
        self.SortItems.is_empty()
    }

    /// 是否需要保持有序（有排序项或有部分有序信息）。
    pub fn NeedKeepOrder(&self) -> bool {
        !self.IsSortItemEmpty() || self.PartialOrderInfo.is_some()
    }

    /// 为保持有序选取的升降序：优先 PartialOrderInfo，否则用 SortItems。
    pub fn GetSortDescForKeepOrder(&self) -> bool {
        if let Some(partial) = self
            .PartialOrderInfo
            .as_ref()
            .filter(|partial| !partial.SortItems.is_empty())
        {
            return partial.AllSameOrder().1;
        }
        self.AllSameOrder().1
    }

    /// 为保持有序选取的排序项列表。
    pub fn GetSortItemsForKeepOrder(&self) -> Vec<SortItem> {
        self.PartialOrderInfo
            .as_ref()
            .filter(|partial| !partial.SortItems.is_empty())
            .map(|partial| partial.SortItems.clone())
            .unwrap_or_else(|| self.SortItems.clone())
    }

    /// 返回属性指纹（懒计算并缓存）。
    pub fn HashCode(&self) -> Vec<u8> {
        self.hashcode.get_or_init(|| self.buildHashCode()).clone()
    }

    /// 编码区分属性的各字段到指纹字节流。
    fn buildHashCode(&self) -> Vec<u8> {
        let mut hashcode = Vec::new();
        hashcode = codec::EncodeInt(hashcode, i64::from(self.CanAddEnforcer));
        hashcode = codec::EncodeInt(hashcode, i64::from(self.TaskTp.0));
        hashcode = codec::EncodeFloat(hashcode, self.ExpectedCnt);
        for item in &self.SortItems {
            let mut column = item.Col.Clone();
            hashcode.extend(column.HashCode());
            hashcode = codec::EncodeInt(hashcode, i64::from(item.Desc));
        }
        // MPP 任务额外编码分区类型、分区列与向量检索列。
        if self.TaskTp == MppTaskType {
            hashcode = codec::EncodeInt(hashcode, i64::from(self.MPPPartitionTp.0));
            for column in &self.MPPPartitionCols {
                hashcode.extend(column.hashCode());
            }
            if let Some(vector) = &self.VectorProp.VSInfo {
                let mut column = vector.Column.Clone();
                hashcode.extend(column.HashCode());
                hashcode = codec::EncodeInt(hashcode, i64::from(vector.FnPbCode as i32));
            }
        }
        hashcode = codec::EncodeInt(hashcode, i64::from(self.CTEProducerStatus.0));
        if let Some(index_join) = &self.IndexJoinProp {
            for condition in &index_join.OtherConditions {
                hashcode.extend(condition.HashCode());
            }
            for column in &index_join.OuterJoinKeys {
                let mut column = column.Clone();
                hashcode.extend(column.HashCode());
            }
            for column in &index_join.InnerJoinKeys {
                let mut column = column.Clone();
                hashcode.extend(column.HashCode());
            }
            hashcode = codec::EncodeFloat(hashcode, index_join.AvgInnerRowCnt);
            hashcode = codec::EncodeInt(hashcode, i64::from(index_join.TableRangeScan));
        }
        hashcode = codec::EncodeInt(hashcode, i64::from(self.NoCopPushDown));
        hashcode = codec::EncodeInt(hashcode, i64::from(self.PreferTiFlash));
        if let Some(partial) = &self.PartialOrderInfo {
            hashcode = codec::EncodeInt(hashcode, 1);
            for item in &partial.SortItems {
                let mut column = item.Col.Clone();
                hashcode.extend(column.HashCode());
                hashcode = codec::EncodeInt(hashcode, i64::from(item.Desc));
            }
        } else {
            hashcode = codec::EncodeInt(hashcode, 0);
        }
        for item in &self.AdvisorySortItems {
            let mut column = item.Col.Clone();
            hashcode.extend(column.HashCode());
            hashcode = codec::EncodeInt(hashcode, i64::from(item.Desc));
        }
        hashcode
    }

    /// 调试用摘要字符串。
    pub fn String(&self) -> String {
        format!(
            "Prop{{cols: {:?}, TaskTp: {}, expectedCount: {}}}",
            self.SortItems, self.TaskTp, self.ExpectedCnt
        )
    }

    /// 克隆用于匹配/比较的关键字段（不含 hash 缓存、向量与 IndexJoin 等）。
    pub fn CloneEssentialFields(&self) -> PhysicalProperty {
        PhysicalProperty {
            SortItems: self.SortItems.clone(),
            SortItemsForPartition: self.SortItemsForPartition.clone(),
            TaskTp: self.TaskTp,
            ExpectedCnt: self.ExpectedCnt,
            MPPPartitionTp: self.MPPPartitionTp,
            MPPPartitionCols: self.MPPPartitionCols.clone(),
            CTEProducerStatus: self.CTEProducerStatus,
            NoCopPushDown: self.NoCopPushDown,
            PartialOrderInfo: self.PartialOrderInfo.clone(),
            AdvisorySortItems: self.AdvisorySortItems.clone(),
            PreferTiFlash: self.PreferTiFlash,
            ..PhysicalProperty::default()
        }
    }

    /// SortItems 是否全部同向；返回 (同向?, 若同向则是否降序)。
    pub fn AllSameOrder(&self) -> (bool, bool) {
        let Some(first) = self.SortItems.first() else {
            return (true, false);
        };
        if self
            .SortItems
            .windows(2)
            .any(|items| items[0].Desc != items[1].Desc)
        {
            return (false, false);
        }
        (true, first.Desc)
    }

    /// 估算本属性结构的内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        mem::size_of::<PhysicalProperty>() as i64
            + self
                .hashcode
                .get()
                .map_or(0, |hashcode| hashcode.capacity() as i64)
            + self
                .SortItems
                .iter()
                .map(SortItem::MemoryUsage)
                .sum::<i64>()
            + self
                .SortItemsForPartition
                .iter()
                .map(SortItem::MemoryUsage)
                .sum::<i64>()
            + self
                .MPPPartitionCols
                .iter()
                .map(MPPPartitionColumn::MemoryUsage)
                .sum::<i64>()
            + self
                .AdvisorySortItems
                .iter()
                .map(SortItem::MemoryUsage)
                .sum::<i64>()
            + self
                .PartialOrderInfo
                .as_ref()
                .map_or(0, PartialOrderInfo::MemoryUsage)
    }
}

impl fmt::Display for PhysicalProperty {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.String())
    }
}

/// 检查 key 是否落在等价集内，且 collation 约束满足。
fn checkEquivalence(
    equivalence_set: &intset::FastIntSet,
    key: &MPPPartitionColumn,
    partition_column: &MPPPartitionColumn,
) -> bool {
    equivalence_set.Has(key.Col.UniqueID as i32)
        && ((key.CollateID < 0 && partition_column.CollateID == key.CollateID)
            || key.CollateID >= 0)
}

/// 判断已提供的分区是否满足要求的物理属性；不满足则需强制加 Exchange。
pub fn NeedEnforceExchanger(
    supplied_partition_type: MPPPartitionType,
    supplied_hash_columns: &[MPPPartitionColumn],
    property: &PhysicalProperty,
    dependencies: Option<&funcdep::FDSet>,
) -> bool {
    match property.MPPPartitionTp {
        AnyType => false,
        BroadcastType => true,
        SinglePartitionType => supplied_partition_type != SinglePartitionType,
        _ => {
            // Hash：类型或列集合不匹配则需 Exchange；有 FD 时按等价类判断。
            if supplied_partition_type != HashType {
                return true;
            }
            if let Some(dependencies) = dependencies.filter(|_| !supplied_hash_columns.is_empty()) {
                return property.NeedMPPExchangeByEquivalence(supplied_hash_columns, dependencies);
            }
            if property.MPPPartitionCols.len() != supplied_hash_columns.len() {
                return true;
            }
            property
                .MPPPartitionCols
                .iter()
                .zip(supplied_hash_columns)
                .any(|(required, supplied)| !required.Equal(supplied))
        }
    }
}
