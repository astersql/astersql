// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.
// 解析器 AST 侧的元模型枚举与大小写不敏感标识符。
//
// 对照 `model.go`：表锁、视图算法/安全/检查选项、分区类型、主键聚簇类型、
// 索引类型、外键引用选项、Runaway（失控查询）动作与监视类型、列选择策略，
// 以及 `CIStr`（保留原始大小写与小写形式的标识符）。

use serde::{Deserialize, Serialize};
use std::fmt;

pub use crate::{CIStr, NewCIStr};

/// 生成透明包装的整型枚举别名，数值与 Go 常量一一对应。
macro_rules! value_type {
    ($(#[$meta:meta])* $name:ident, $repr:ty) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
        #[repr(transparent)]
        pub struct $name(pub $repr);
    };
}

value_type!(
    /// 表锁类型（LOCK TABLES 的 READ/WRITE 等语义）。
    TableLockType,
    u8
);
/// 无锁。
pub const TableLockNone: TableLockType = TableLockType(0);
/// 共享读锁。
pub const TableLockRead: TableLockType = TableLockType(1);
/// 本地读锁（允许并发本地写）。
pub const TableLockReadLocal: TableLockType = TableLockType(2);
/// 只读锁。
pub const TableLockReadOnly: TableLockType = TableLockType(3);
/// 排他写锁。
pub const TableLockWrite: TableLockType = TableLockType(4);
/// 本地写锁。
pub const TableLockWriteLocal: TableLockType = TableLockType(5);
impl fmt::Display for TableLockType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 未知取值输出空串，与 Go String() 一致。
        f.write_str(match *self {
            TableLockNone => "NONE",
            TableLockRead => "READ",
            TableLockReadLocal => "READ LOCAL",
            TableLockReadOnly => "READ ONLY",
            TableLockWrite => "WRITE",
            TableLockWriteLocal => "WRITE LOCAL",
            _ => "",
        })
    }
}

value_type!(
    /// 视图物化/合并算法。
    ViewAlgorithm,
    i32
);
/// 未指定算法。
pub const AlgorithmUndefined: ViewAlgorithm = ViewAlgorithm(0);
/// MERGE：将视图定义内联到外层查询。
pub const AlgorithmMerge: ViewAlgorithm = ViewAlgorithm(1);
/// TEMPTABLE：先物化为临时表再查询。
pub const AlgorithmTemptable: ViewAlgorithm = ViewAlgorithm(2);
impl ViewAlgorithm {
    pub const Undefined: Self = AlgorithmUndefined;
    pub const Merge: Self = AlgorithmMerge;
    pub const Temptable: Self = AlgorithmTemptable;
}
impl fmt::Display for ViewAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            AlgorithmMerge => "MERGE",
            AlgorithmTemptable => "TEMPTABLE",
            _ => "UNDEFINED",
        })
    }
}

value_type!(
    /// 视图 SQL SECURITY：DEFINER 或 INVOKER。
    ViewSecurity,
    i32
);
/// 以定义者权限执行。
pub const SecurityDefiner: ViewSecurity = ViewSecurity(0);
/// 以调用者权限执行。
pub const SecurityInvoker: ViewSecurity = ViewSecurity(1);
impl ViewSecurity {
    pub const Definer: Self = SecurityDefiner;
    pub const Invoker: Self = SecurityInvoker;
}
impl fmt::Display for ViewSecurity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if *self == SecurityInvoker {
            "INVOKER"
        } else {
            "DEFINER"
        })
    }
}

value_type!(
    /// 视图 WITH CHECK OPTION 作用范围。
    ViewCheckOption,
    i32
);
/// 仅检查本层视图约束。
pub const CheckOptionLocal: ViewCheckOption = ViewCheckOption(0);
/// 级联检查底层视图约束。
pub const CheckOptionCascaded: ViewCheckOption = ViewCheckOption(1);
impl ViewCheckOption {
    pub const Local: Self = CheckOptionLocal;
    pub const Cascaded: Self = CheckOptionCascaded;
}
impl fmt::Display for ViewCheckOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if *self == CheckOptionLocal {
            "LOCAL"
        } else {
            "CASCADED"
        })
    }
}

value_type!(
    /// 表分区类型（RANGE/HASH/LIST/KEY/SYSTEM_TIME）。
    PartitionType,
    i32
);
pub const PartitionTypeNone: PartitionType = PartitionType(0);
pub const PartitionTypeRange: PartitionType = PartitionType(1);
pub const PartitionTypeHash: PartitionType = PartitionType(2);
pub const PartitionTypeList: PartitionType = PartitionType(3);
pub const PartitionTypeKey: PartitionType = PartitionType(4);
/// 按系统时间版本分区（时态表）。
pub const PartitionTypeSystemTime: PartitionType = PartitionType(5);
impl PartitionType {
    pub const None: Self = PartitionTypeNone;
    pub const Range: Self = PartitionTypeRange;
    pub const Hash: Self = PartitionTypeHash;
    pub const List: Self = PartitionTypeList;
    pub const Key: Self = PartitionTypeKey;
    pub const SystemTime: Self = PartitionTypeSystemTime;
}
impl fmt::Display for PartitionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            PartitionTypeNone => "NONE",
            PartitionTypeRange => "RANGE",
            PartitionTypeHash => "HASH",
            PartitionTypeList => "LIST",
            PartitionTypeKey => "KEY",
            PartitionTypeSystemTime => "SYSTEM_TIME",
            _ => "",
        })
    }
}

value_type!(
    /// 主键是否聚簇（CLUSTERED/NONCLUSTERED）。
    PrimaryKeyType,
    i8
);
pub const PrimaryKeyTypeDefault: PrimaryKeyType = PrimaryKeyType(0);
/// 聚簇主键：行数据按主键顺序存放。
pub const PrimaryKeyTypeClustered: PrimaryKeyType = PrimaryKeyType(1);
/// 非聚簇主键：主键仅作二级索引。
pub const PrimaryKeyTypeNonClustered: PrimaryKeyType = PrimaryKeyType(2);
impl fmt::Display for PrimaryKeyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            PrimaryKeyTypeClustered => "CLUSTERED",
            PrimaryKeyTypeNonClustered => "NONCLUSTERED",
            _ => "",
        })
    }
}

value_type!(
    /// 索引物理结构/用途类型。
    IndexType,
    i32
);
pub const IndexTypeInvalid: IndexType = IndexType(0);
pub const IndexTypeBtree: IndexType = IndexType(1);
pub const IndexTypeHash: IndexType = IndexType(2);
pub const IndexTypeRtree: IndexType = IndexType(3);
/// 假设索引（Hypothetical），仅用于优化器实验。
pub const IndexTypeHypo: IndexType = IndexType(4);
pub const IndexTypeVector: IndexType = IndexType(5);
pub const IndexTypeInverted: IndexType = IndexType(6);
/// HNSW：近似最近邻向量索引结构。
pub const IndexTypeHNSW: IndexType = IndexType(7);
pub const IndexTypeFulltext: IndexType = IndexType(8);
impl IndexType {
    pub const Invalid: Self = IndexTypeInvalid;
    pub const Btree: Self = IndexTypeBtree;
    pub const Hash: Self = IndexTypeHash;
    pub const Rtree: Self = IndexTypeRtree;
    pub const Hypo: Self = IndexTypeHypo;
    pub const Vector: Self = IndexTypeVector;
    pub const Inverted: Self = IndexTypeInverted;
    pub const HNSW: Self = IndexTypeHNSW;
    pub const Fulltext: Self = IndexTypeFulltext;
}
impl fmt::Display for IndexType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            IndexTypeBtree => "BTREE",
            IndexTypeHash => "HASH",
            IndexTypeRtree => "RTREE",
            IndexTypeHypo => "HYPO",
            IndexTypeVector => "VECTOR",
            IndexTypeInverted => "INVERTED",
            IndexTypeHNSW => "HNSW",
            IndexTypeFulltext => "FULLTEXT",
            _ => "",
        })
    }
}

value_type!(
    /// 外键 ON DELETE/UPDATE 引用动作。
    ReferOptionType,
    i32
);
pub const ReferOptionNoOption: ReferOptionType = ReferOptionType(0);
pub const ReferOptionRestrict: ReferOptionType = ReferOptionType(1);
pub const ReferOptionCascade: ReferOptionType = ReferOptionType(2);
pub const ReferOptionSetNull: ReferOptionType = ReferOptionType(3);
pub const ReferOptionNoAction: ReferOptionType = ReferOptionType(4);
pub const ReferOptionSetDefault: ReferOptionType = ReferOptionType(5);
impl ReferOptionType {
    pub const None: Self = ReferOptionNoOption;
    pub const Restrict: Self = ReferOptionRestrict;
    pub const Cascade: Self = ReferOptionCascade;
    pub const SetNull: Self = ReferOptionSetNull;
    pub const NoAction: Self = ReferOptionNoAction;
    pub const SetDefault: Self = ReferOptionSetDefault;
}
impl fmt::Display for ReferOptionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            ReferOptionRestrict => "RESTRICT",
            ReferOptionCascade => "CASCADE",
            ReferOptionSetNull => "SET NULL",
            ReferOptionNoAction => "NO ACTION",
            ReferOptionSetDefault => "SET DEFAULT",
            _ => "",
        })
    }
}

value_type!(
    /// Runaway（失控查询）触发后的处置动作。
    RunawayActionType,
    i32
);
pub const RunawayActionNone: RunawayActionType = RunawayActionType(0);
pub const RunawayActionDryRun: RunawayActionType = RunawayActionType(1);
/// 降速冷却。
pub const RunawayActionCooldown: RunawayActionType = RunawayActionType(2);
/// 终止会话/查询。
pub const RunawayActionKill: RunawayActionType = RunawayActionType(3);
/// 切换资源组。
pub const RunawayActionSwitchGroup: RunawayActionType = RunawayActionType(4);
impl fmt::Display for RunawayActionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 未识别动作默认展示为 DRYRUN。
        f.write_str(match *self {
            RunawayActionCooldown => "COOLDOWN",
            RunawayActionKill => "KILL",
            RunawayActionSwitchGroup => "SWITCH_GROUP",
            _ => "DRYRUN",
        })
    }
}

value_type!(
    /// Runaway 监视匹配粒度：精确 SQL、相似 SQL、或执行计划。
    RunawayWatchType,
    i32
);
pub const WatchNone: RunawayWatchType = RunawayWatchType(0);
pub const WatchExact: RunawayWatchType = RunawayWatchType(1);
pub const WatchSimilar: RunawayWatchType = RunawayWatchType(2);
pub const WatchPlan: RunawayWatchType = RunawayWatchType(3);
impl fmt::Display for RunawayWatchType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            WatchExact => "EXACT",
            WatchSimilar => "SIMILAR",
            WatchPlan => "PLAN",
            _ => "NONE",
        })
    }
}

value_type!(
    /// Runaway 选项类别：规则 / 动作 / 监视。
    RunawayOptionType,
    i32
);
pub const RunawayRule: RunawayOptionType = RunawayOptionType(0);
pub const RunawayAction: RunawayOptionType = RunawayOptionType(1);
pub const RunawayWatch: RunawayOptionType = RunawayOptionType(2);

value_type!(
    /// ANALYZE / 统计相关列选择策略。
    ColumnChoice,
    u8
);
pub const DefaultChoice: ColumnChoice = ColumnChoice(0);
pub const AllColumns: ColumnChoice = ColumnChoice(1);
/// 仅谓词涉及的列。
pub const PredicateColumns: ColumnChoice = ColumnChoice(2);
/// 显式列清单。
pub const ColumnList: ColumnChoice = ColumnChoice(3);
impl ColumnChoice {
    pub const Default: Self = DefaultChoice;
    pub const All: Self = AllColumns;
    pub const Predicate: Self = PredicateColumns;
    pub const List: Self = ColumnList;
}
impl fmt::Display for ColumnChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            AllColumns => "ALL",
            PredicateColumns => "PREDICATE",
            ColumnList => "LIST",
            _ => "DEFAULT",
        })
    }
}

/// 语句优先级数值：LOW / MEDIUM / HIGH。
pub const LowPriorityValue: u64 = 1;
pub const MediumPriorityValue: u64 = 8;
pub const HighPriorityValue: u64 = 16;
/// 将优先级数值映射为展示名；未知值归为 MEDIUM。
pub fn PriorityValueToName(value: u64) -> &'static str {
    match value {
        LowPriorityValue => "LOW",
        HighPriorityValue => "HIGH",
        _ => "MEDIUM",
    }
}
