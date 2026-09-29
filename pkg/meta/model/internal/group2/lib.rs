// Copyright 2026 AsterSQL.
// group2 提供 DDL Job 参数 V1/V2 编解码兼容层。
//
// 本 crate 内嵌 `job_args.rs`；Job / JobVersion / JobState / ActionType 直接复用
// group3 的完整生产身份，参数载荷 DTO 保持 Go V1 数组与 V2 JSON 的字段布局。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

pub use serde;
pub use serde_json;

/// 错误桩：用 String 代替完整 terror，便于参数解码路径单测。
pub mod errors {
    pub type Error = String;

    pub fn Trace(error: impl std::fmt::Display) -> Error {
        error.to_string()
    }

    pub fn type_mismatch() -> Error {
        "job argument cache type mismatch".to_owned()
    }

    pub fn Errorf(message: String) -> Error {
        message
    }
}

/// 内部断言桩，对应 Go intest 包在测试中的硬失败行为。
pub mod intest {
    pub fn Assert(condition: bool, message: &str) {
        assert!(condition, "{message}");
    }
}

// 生成可序列化的默认结构体，字段名保持与 Go 模型一致。
macro_rules! data_type {
    ($(#[$meta:meta])* $name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
        pub struct $name { $(pub $field: $ty),* }
    };
}

/// AST 相关最小类型：大小写不敏感标识、列位置、分区与序列选项等。
pub mod ast {
    data_type!(
        /// 大小写不敏感字符串：O 为原始值，L 为小写形式。
        CIStr {
        O: String,
        L: String
    });

    /// 由原始字符串构造 CIStr，自动填充小写副本。
    pub fn NewCIStr(value: &str) -> CIStr {
        CIStr {
            O: value.to_owned(),
            L: value.to_lowercase(),
        }
    }

    data_type!(
        /// 限定名：Schema + Name。
        Ident {
        Schema: CIStr,
        Name: CIStr
    });
    pub const ColumnPositionNone: i32 = 0;
    pub const ColumnPositionFirst: i32 = 1;
    pub const PartitionTypeRange: i32 = 1;
    pub const SequenceOptionIncrementBy: i32 = 1;
    pub const SequenceCache: i32 = 2;
    pub const TableLockNone: i32 = 0;
    data_type!(ColumnPosition { Tp: i32 });
    data_type!(IndexPartSpecification {
        Column: CIStr,
        Length: i32
    });
    data_type!(IndexOption {});
    data_type!(SequenceOption {
        Tp: i32,
        IntValue: i64
    });
    data_type!(TiFlashReplicaSpec { Count: u64, LocationLabels: Vec<String>, Labels: Vec<String>, Hypo: bool });
}

/// MySQL SQL Mode 桩类型与常用常量。
pub mod mysql {
    pub type SQLMode = u64;
    pub const ModeANSI: SQLMode = 4;
}

/// PD HTTP API 中 Region 标签与规则的最小表示。
/// Region 是 TiKV 的数据分片单位，标签用于放置策略。
pub mod pdhttp {
    data_type!(RegionLabel {
        Key: String,
        Value: String
    });
    data_type!(LabelRule { ID: String, Index: i32, RuleType: String, Labels: Vec<RegionLabel> });
}

data_type!(
    /// 库级元信息桩，仅保留 ID。
    DBInfo { ID: i64 });
/// Job 参数在完整表模型落地前保留未识别的 Go 表字段，避免 V1/V2 往返丢失元数据。
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TableInfo {
    #[serde(rename = "id", alias = "ID")]
    pub ID: i64,
    #[serde(rename = "name", alias = "Name")]
    pub Name: ast::CIStr,
    #[serde(rename = "materialized_view", skip_serializing_if = "Option::is_none")]
    pub MaterializedView: Option<serde_json::Value>,
    #[serde(
        rename = "materialized_view_shadow",
        skip_serializing_if = "Option::is_none"
    )]
    pub MaterializedViewShadow: Option<serde_json::Value>,
    #[serde(flatten)]
    pub Other: std::collections::BTreeMap<String, serde_json::Value>,
}
data_type!(
    /// Placement Policy 引用（ID + 名称）。
    PolicyRefInfo {
    ID: i64,
    Name: ast::CIStr
});
data_type!(
    /// 单个分区定义：ID、名称与 RANGE 上界表达式列表。
    PartitionDefinition { ID: i64, Name: ast::CIStr, LessThan: Vec<String> });
data_type!(
    /// 表分区信息：分区类型与定义列表。
    PartitionInfo { ID: i64, Type: i32, Definitions: Vec<PartitionDefinition> });
data_type!(
    /// 资源组元信息桩。
    ResourceGroupInfo {
    ID: i64,
    Name: ast::CIStr
});
data_type!(
    /// 外键元信息桩。
    FKInfo {
    ID: i64,
    Name: ast::CIStr
});
data_type!(
    /// 列元信息桩。
    ColumnInfo {
    ID: i64,
    Name: ast::CIStr
});
data_type!(
    /// 索引元信息桩。
    IndexInfo { ID: i64 });
data_type!(
    /// CHECK 约束元信息。
    ConstraintInfo {
    Name: ast::CIStr,
    Table: ast::CIStr,
    ExprString: String,
    State: i32
});
data_type!(
    /// TTL（Time To Live）表级过期策略信息。
    TTLInfo {
    Enable: bool,
    ColumnName: ast::CIStr,
    IntervalExprStr: String,
    IntervalTimeUnit: i32
});
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
/// 表锁三元组：(表 ID, 会话/服务端标识相关, 锁类型)。
pub struct TableLockTpInfo(pub i64, pub i64, pub i32);
data_type!(
    /// 持锁会话信息。
    SessionInfo { ServerID: String });
/// Schema 对象公开状态（DeleteOnly/WriteOnly/Public 等）；跨版本持久化。
pub type SchemaState = i32;
pub const StateDeleteOnly: SchemaState = 1;
data_type!(
    /// Placement Policy 完整元信息。
    PolicyInfo {
    ID: i64,
    Name: ast::CIStr,
    State: SchemaState
});
data_type!(
    /// 数据脱敏（Masking）策略元信息。
    MaskingPolicyInfo {
    ID: i64,
    Name: ast::CIStr,
    State: SchemaState
});
data_type!(
    /// 表亲和性配置。
    TableAffinityInfo { Affinity: String });
data_type!(
    /// Region 分裂策略配置字符串。
    RegionSplitPolicy { Value: String });

/// 表模式（Normal/Import/Restore 等）。
pub type TableMode = i32;
/// 列式索引类型：向量 / 倒排等；NA 表示不适用。
pub type ColumnarIndexType = i32;
pub const ColumnarIndexTypeNA: ColumnarIndexType = 0;
pub const ColumnarIndexTypeVector: ColumnarIndexType = 1;
pub const ColumnarIndexTypeInverted: ColumnarIndexType = 2;

/// Job、版本、动作与状态统一复用 group3 的完整生产模型和持久化编号。
pub use group_3::{ActionType, Job, JobState, JobVersion};
pub const JobVersion1: JobVersion = JobVersion::V1;
pub const JobVersion2: JobVersion = JobVersion::V2;
pub const JobStateNone: JobState = JobState::None;
pub const JobStateRollingback: JobState = JobState::Rollingback;
pub const JobStateRollbackDone: JobState = JobState::RollbackDone;
pub const ActionAddColumn: ActionType = group_3::ACTION_ADD_COLUMN;
pub const ActionAddColumnarIndex: ActionType = group_3::ACTION_ADD_COLUMNAR_INDEX;
pub const ActionAddIndex: ActionType = group_3::ACTION_ADD_INDEX;
pub const ActionAddPrimaryKey: ActionType = group_3::ACTION_ADD_PRIMARY_KEY;
pub const ActionAddTablePartition: ActionType = group_3::ACTION_ADD_TABLE_PARTITION;
pub const ActionAlterMaskingPolicy: ActionType = group_3::ACTION_ALTER_MASKING_POLICY;
pub const ActionAlterPlacementPolicy: ActionType = group_3::ACTION_ALTER_PLACEMENT_POLICY;
pub const ActionAlterResourceGroup: ActionType = group_3::ACTION_ALTER_RESOURCE_GROUP;
pub const ActionAlterTablePartitionAttributes: ActionType =
    group_3::ACTION_ALTER_TABLE_PARTITION_ATTRIBUTES;
pub const ActionCreateMaskingPolicy: ActionType = group_3::ACTION_CREATE_MASKING_POLICY;
pub const ActionCreatePlacementPolicy: ActionType = group_3::ACTION_CREATE_PLACEMENT_POLICY;
pub const ActionCreateResourceGroup: ActionType = group_3::ACTION_CREATE_RESOURCE_GROUP;
pub const ActionCreateSequence: ActionType = group_3::ACTION_CREATE_SEQUENCE;
pub const ActionCreateTable: ActionType = group_3::ACTION_CREATE_TABLE;
pub const ActionCreateView: ActionType = group_3::ACTION_CREATE_VIEW;
pub const ActionDropColumn: ActionType = group_3::ACTION_DROP_COLUMN;
pub const ActionDropIndex: ActionType = group_3::ACTION_DROP_INDEX;
pub const ActionDropPrimaryKey: ActionType = group_3::ACTION_DROP_PRIMARY_KEY;
pub const ActionDropResourceGroup: ActionType = group_3::ACTION_DROP_RESOURCE_GROUP;
pub const ActionDropTable: ActionType = group_3::ACTION_DROP_TABLE;
pub const ActionDropTablePartition: ActionType = group_3::ACTION_DROP_TABLE_PARTITION;
pub const ActionModifySchemaCharsetAndCollate: ActionType =
    group_3::ACTION_MODIFY_SCHEMA_CHARSET_AND_COLLATE;
pub const ActionRecoverTable: ActionType = group_3::ACTION_RECOVER_TABLE;
pub const ActionRenameIndex: ActionType = group_3::ACTION_RENAME_INDEX;
pub const ActionTruncateTable: ActionType = group_3::ACTION_TRUNCATE_TABLE;
pub const ActionCreateTables: ActionType = group_3::ACTION_CREATE_TABLES;
pub const ActionDropSchema: ActionType = group_3::ACTION_DROP_SCHEMA;
pub const ActionModifySchemaDefaultPlacement: ActionType =
    group_3::ACTION_MODIFY_SCHEMA_DEFAULT_PLACEMENT;
pub const ActionTruncateTablePartition: ActionType = group_3::ACTION_TRUNCATE_TABLE_PARTITION;
pub const ActionCreateSchema: ActionType = group_3::ACTION_CREATE_SCHEMA;
pub const ActionDropView: ActionType = group_3::ACTION_DROP_VIEW;
pub const ActionDropSequence: ActionType = group_3::ACTION_DROP_SEQUENCE;
pub const ActionAlterTablePartitioning: ActionType = group_3::ACTION_ALTER_TABLE_PARTITIONING;
pub const ActionRemovePartitioning: ActionType = group_3::ACTION_REMOVE_PARTITIONING;
pub const ActionReorganizePartition: ActionType = group_3::ACTION_REORGANIZE_PARTITION;
pub const ActionExchangeTablePartition: ActionType = group_3::ACTION_EXCHANGE_TABLE_PARTITION;
pub const ActionAlterTablePartitionPlacement: ActionType =
    group_3::ACTION_ALTER_TABLE_PARTITION_PLACEMENT;
pub const ActionRenameTable: ActionType = group_3::ACTION_RENAME_TABLE;
pub const ActionRenameTables: ActionType = group_3::ACTION_RENAME_TABLES;
pub const ActionAlterSequence: ActionType = group_3::ACTION_ALTER_SEQUENCE;
pub const ActionRebaseAutoID: ActionType = group_3::ACTION_REBASE_AUTO_ID;
pub const ActionRebaseAutoRandomBase: ActionType = group_3::ACTION_REBASE_AUTO_RANDOM_BASE;
pub const ActionModifyTableComment: ActionType = group_3::ACTION_MODIFY_TABLE_COMMENT;
pub const ActionAlterIndexVisibility: ActionType = group_3::ACTION_ALTER_INDEX_VISIBILITY;
pub const ActionAddForeignKey: ActionType = group_3::ACTION_ADD_FOREIGN_KEY;
pub const ActionModifyTableAutoIDCache: ActionType = group_3::ACTION_MODIFY_TABLE_AUTO_ID_CACHE;
pub const ActionShardRowID: ActionType = group_3::ACTION_SHARD_ROW_ID;
pub const ActionDropForeignKey: ActionType = group_3::ACTION_DROP_FOREIGN_KEY;
pub const ActionAlterTTLInfo: ActionType = group_3::ACTION_ALTER_TTL_INFO;
pub const ActionAddCheckConstraint: ActionType = group_3::ACTION_ADD_CHECK_CONSTRAINT;
pub const ActionDropCheckConstraint: ActionType = group_3::ACTION_DROP_CHECK_CONSTRAINT;
pub const ActionAlterTablePlacement: ActionType = group_3::ACTION_ALTER_TABLE_PLACEMENT;
pub const ActionSetTiFlashReplica: ActionType = group_3::ACTION_SET_TIFLASH_REPLICA;
pub const ActionUpdateTiFlashReplicaStatus: ActionType =
    group_3::ACTION_UPDATE_TIFLASH_REPLICA_STATUS;
pub const ActionLockTable: ActionType = group_3::ACTION_LOCK_TABLE;
pub const ActionUnlockTable: ActionType = group_3::ACTION_UNLOCK_TABLE;
pub const ActionRepairTable: ActionType = group_3::ACTION_REPAIR_TABLE;
pub const ActionRecoverSchema: ActionType = group_3::ACTION_RECOVER_SCHEMA;
pub const ActionDropPlacementPolicy: ActionType = group_3::ACTION_DROP_PLACEMENT_POLICY;
pub const ActionDropMaskingPolicy: ActionType = group_3::ACTION_DROP_MASKING_POLICY;
pub const ActionSetDefaultValue: ActionType = group_3::ACTION_SET_DEFAULT_VALUE;
pub const ActionFlashbackCluster: ActionType = group_3::ACTION_FLASHBACK_CLUSTER;
pub const ActionAlterTableAttributes: ActionType = group_3::ACTION_ALTER_TABLE_ATTRIBUTES;
pub const ActionModifyColumn: ActionType = group_3::ACTION_MODIFY_COLUMN;

/// 将 JSON 值序列解码到目标字段；支持单值与元组目标。
pub trait DecodeArgs {
    fn decode(self, values: &[serde_json::Value]) -> Result<(), errors::Error>;
}

impl<'a, T> DecodeArgs for &'a mut T
where
    T: serde::de::DeserializeOwned,
{
    fn decode(self, values: &[serde_json::Value]) -> Result<(), errors::Error> {
        if let Some(value) = values.first() {
            if !value.is_null() {
                *self = serde_json::from_value(value.clone()).map_err(errors::Trace)?;
            }
        }
        Ok(())
    }
}

// 为可变引用元组实现 DecodeArgs，按下标从 JSON 数组取值。
macro_rules! impl_decode_tuple {
    ($($name:ident : $index:tt),+ $(,)?) => {
        impl<'a, $($name),+> DecodeArgs for ($(&'a mut $name,)+)
        where
            $($name: serde::de::DeserializeOwned,)+
        {
            fn decode(self, values: &[serde_json::Value]) -> Result<(), errors::Error> {
                $(
                    if let Some(value) = values.get($index) {
                        if !value.is_null() {
                            *self.$index = serde_json::from_value(value.clone()).map_err(errors::Trace)?;
                        }
                    }
                )+
                Ok(())
            }
        }
    };
}

impl_decode_tuple!(A:0, B:1);
impl_decode_tuple!(A:0, B:1, C:2);
impl_decode_tuple!(A:0, B:1, C:2, D:3);
impl_decode_tuple!(A:0, B:1, C:2, D:3, E:4);
impl_decode_tuple!(A:0, B:1, C:2, D:3, E:4, F:5);
impl_decode_tuple!(A:0, B:1, C:2, D:3, E:4, F:5, G:6);
impl_decode_tuple!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7);
impl_decode_tuple!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7, I:8);
impl_decode_tuple!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7, I:8, J:9);

/// 为完整 Job 提供 Go 风格参数编解码方法名，保持现有调用方兼容。
pub trait JobArgsCompat {
    fn decodeArgs<T: DecodeArgs>(&self, output: T) -> Result<(), errors::Error>;
    fn FillArgs<T>(&mut self, args: T)
    where
        T: JobArgs + Clone + serde::Serialize + 'static;
    fn FillFinishedArgs<T>(&mut self, args: T)
    where
        T: FinishedJobArgs + Clone + serde::Serialize + 'static;
    fn Encode(&mut self, update_raw_args: bool) -> Result<Vec<u8>, errors::Error>;
    fn Decode(&mut self, bytes: &[u8]) -> Result<(), errors::Error>;
    fn IsRollingback(&self) -> bool;
}

impl JobArgsCompat for Job {
    fn decodeArgs<T: DecodeArgs>(&self, output: T) -> Result<(), errors::Error> {
        let value: serde_json::Value =
            serde_json::from_slice(&self.raw_args).map_err(errors::Trace)?;
        match value {
            serde_json::Value::Array(values) => output.decode(&values),
            serde_json::Value::Null => output.decode(&[]),
            value => output.decode(&[value]),
        }
    }

    fn FillArgs<T>(&mut self, args: T)
    where
        T: JobArgs + Clone + serde::Serialize + 'static,
    {
        self.args = if self.version == JobVersion1 {
            args.getArgsV1(self)
        } else {
            vec![serde_json::to_value(&args).expect("serialize V2 job args")]
        };
    }

    fn FillFinishedArgs<T>(&mut self, args: T)
    where
        T: FinishedJobArgs + Clone + serde::Serialize + 'static,
    {
        self.args = if self.version == JobVersion1 {
            args.getFinishedArgsV1(self)
        } else {
            vec![serde_json::to_value(&args).expect("serialize V2 finished args")]
        };
    }

    fn Encode(&mut self, update_raw_args: bool) -> Result<Vec<u8>, errors::Error> {
        self.encode(update_raw_args).map_err(errors::Trace)
    }

    fn Decode(&mut self, bytes: &[u8]) -> Result<(), errors::Error> {
        *self = Job::decode(bytes).map_err(errors::Trace)?;
        Ok(())
    }

    fn IsRollingback(&self) -> bool {
        self.is_rollingback()
    }
}

include!("../../job_args.rs");

#[cfg(test)]
#[path = "../../job_args_2_aster_unit_test.rs"]
mod job_args_2_aster_unit_test;
