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

// 本文件对照 pkg/ddl/bdr/bdr_test.go，使用已经接线的 parser/model/types 和真实 BDR 逻辑。

// BDR（Bidirectional Replication，双向复制）DDL 拦截逻辑的单元测试。
//
// BDR 指两个集群互为主备并双向同步数据的部署形态。为避免双向同步时
// DDL（数据定义语言，如 CREATE/ALTER/DROP）在两侧产生冲突，集群会被
// 赋予 BDR 角色（Primary 主、Secondary 备、None 未启用），不同角色下
// 部分 DDL 动作会被拒绝执行。
//
// 本文件通过表驱动测试覆盖三个入口：
// - `IsAddColumnDenied`：加列操作是否因列选项（非空、默认值等）被拒绝；
// - `IsModifyColumnDenied`：改列操作是否因类型变化或列选项被拒绝；
// - `IsDenied`：按 DDL 动作类型与 BDR 角色判定的完整拒绝矩阵，
//   并额外覆盖加索引/加主键时依据索引参数（是否唯一）的特判。

use crate::ast::{BDRRole, ColumnOption, ColumnOptionType};
use crate::model_group::{self, ActionType};
use crate::model_job_args::{
    self, GetModifyIndexArgs, IndexArg, Job, JobArgsCompat, JobVersion1, JobVersion2,
    ModifyIndexArgs, OpAddIndex, ast as job_ast, mysql as job_mysql,
};
use crate::{IsAddColumnDenied, IsDenied, IsModifyColumnDenied};
use parser_types::mysql::{TypeLong, TypeVarchar};
use parser_types::types::{FieldType, NewFieldType};

// ColumnOptionCase 对应 Go 中 IsAddColumnDenied 的匿名表格行。
/// 加列（ADD COLUMN）拒绝测试的单个用例：
/// 给定 BDR 角色与列选项组合，断言是否应被拒绝。
#[derive(Debug, Clone, Copy)]
struct ColumnOptionCase {
    /// 用例名称，断言失败时用于定位。
    name: &'static str,
    /// 集群的 BDR 角色（Primary/Secondary/None）。
    role: BDRRole,
    /// 新增列携带的列选项类型（如 NULL、DEFAULT、COMMENT 等）。
    options: &'static [ColumnOptionType],
    /// 预期结果：true 表示该 DDL 应被拒绝。
    expected: bool,
}

/// 加列拒绝判定的用例表：Primary 角色下仅允许可空列或带默认值/注释/
/// 生成列等安全选项的列；出现其他选项（如 CHECK 约束）则拒绝；
/// Secondary/None 角色不做此限制。
const ADD_COLUMN_DENIED_CASES: &[ColumnOptionCase] = &[
    ColumnOptionCase {
        name: "Test with no options(implicit nullable)",
        role: BDRRole::Primary,
        options: &[],
        expected: false,
    },
    ColumnOptionCase {
        name: "Test with nullable option",
        role: BDRRole::Primary,
        options: &[ColumnOptionType::Null],
        expected: false,
    },
    ColumnOptionCase {
        name: "Test with implicit nullable and defaultValue options",
        role: BDRRole::Primary,
        options: &[ColumnOptionType::DefaultValue],
        expected: false,
    },
    ColumnOptionCase {
        name: "Test with nullable and defaultValue options",
        role: BDRRole::Primary,
        options: &[ColumnOptionType::NotNull, ColumnOptionType::DefaultValue],
        expected: false,
    },
    ColumnOptionCase {
        name: "Test with comment options",
        role: BDRRole::Primary,
        options: &[ColumnOptionType::Comment],
        expected: false,
    },
    ColumnOptionCase {
        name: "Test with generated options",
        role: BDRRole::Primary,
        options: &[ColumnOptionType::Generated],
        expected: false,
    },
    ColumnOptionCase {
        name: "Test with comment and generated options",
        role: BDRRole::Primary,
        options: &[ColumnOptionType::Comment, ColumnOptionType::Generated],
        expected: false,
    },
    ColumnOptionCase {
        name: "Test with other options",
        role: BDRRole::Primary,
        options: &[ColumnOptionType::Check],
        expected: true,
    },
    ColumnOptionCase {
        name: "Test with secondary role",
        role: BDRRole::Secondary,
        options: &[ColumnOptionType::Check],
        expected: false,
    },
    ColumnOptionCase {
        name: "Test with none role",
        role: BDRRole::None,
        options: &[ColumnOptionType::Check],
        expected: false,
    },
];

// ModifyColumnDeniedCase 对应 Go 中 IsModifyColumnDenied 的匿名表格行。
/// 改列（MODIFY COLUMN）拒绝测试的单个用例：
/// 给定角色、新旧字段类型与列选项，断言是否应被拒绝。
#[derive(Debug, Clone, Copy)]
struct ModifyColumnDeniedCase {
    /// 用例名称。
    name: &'static str,
    /// 集群的 BDR 角色。
    role: BDRRole,
    /// 修改后的字段类型编码（MySQL 类型常量，如 TypeLong）。
    new_field_type: u8,
    /// 修改前的字段类型编码。
    old_field_type: u8,
    /// 改列语句携带的列选项类型。
    options: &'static [ColumnOptionType],
    /// 预期结果：true 表示该 DDL 应被拒绝。
    expected: bool,
}

/// 改列拒绝判定的用例表：Primary 角色下改变列类型会被拒绝（类型变化在
/// 双向复制中可能造成两侧数据不兼容）；仅设置默认值（可附带注释）被视为
/// 安全操作放行；Secondary/None 角色不做此限制。
const MODIFY_COLUMN_DENIED_CASES: &[ModifyColumnDeniedCase] = &[
    ModifyColumnDeniedCase {
        name: "Test when newFieldType and oldFieldType are not equal",
        role: BDRRole::Primary,
        new_field_type: TypeLong,
        old_field_type: TypeVarchar,
        options: &[],
        expected: true,
    },
    ModifyColumnDeniedCase {
        name: "Test when only defaultValue option is provided",
        role: BDRRole::Primary,
        new_field_type: TypeLong,
        old_field_type: TypeLong,
        options: &[ColumnOptionType::DefaultValue],
        expected: false,
    },
    ModifyColumnDeniedCase {
        name: "Test when defaultValue and comment options are provided",
        role: BDRRole::Primary,
        new_field_type: TypeLong,
        old_field_type: TypeLong,
        options: &[ColumnOptionType::DefaultValue, ColumnOptionType::Comment],
        expected: false,
    },
    ModifyColumnDeniedCase {
        name: "Test when other options are provided",
        role: BDRRole::Primary,
        new_field_type: TypeLong,
        old_field_type: TypeLong,
        options: &[ColumnOptionType::Comment],
        expected: true,
    },
    ModifyColumnDeniedCase {
        name: "Test with secondary role",
        role: BDRRole::Secondary,
        new_field_type: TypeLong,
        old_field_type: TypeVarchar,
        options: &[],
        expected: false,
    },
    ModifyColumnDeniedCase {
        name: "Test with none role",
        role: BDRRole::None,
        new_field_type: TypeLong,
        old_field_type: TypeVarchar,
        options: &[],
        expected: false,
    },
];

// ActionDeniedCase 对应 Go TestIsDenied 的主矩阵。
// 三个布尔值分别对应 Primary、Secondary、None 角色，保持 Go 文件中 action 顺序。
/// DDL 动作类型 × BDR 角色的拒绝矩阵中的一行。
#[derive(Debug, Clone, Copy)]
struct ActionDeniedCase {
    /// 动作名称（由宏从常量标识符生成）。
    name: &'static str,
    /// DDL 动作类型（如建表、删列、加索引等）。
    action: ActionType,
    /// Primary 角色下是否应被拒绝。
    expected_primary: bool,
    /// Secondary 角色下是否应被拒绝。
    expected_secondary: bool,
    /// None（未启用 BDR）角色下是否应被拒绝。
    expected_none: bool,
}

/// 构造 `ActionDeniedCase` 的辅助宏：用 `stringify!` 把动作常量名直接
/// 作为用例名，减少重复书写。参数依次为动作常量、Primary/Secondary/None
/// 三个角色下的预期拒绝结果。
macro_rules! action_case {
    ($action:ident, $primary:expr, $secondary:expr, $none:expr) => {
        ActionDeniedCase {
            name: stringify!($action),
            action: model_group::$action,
            expected_primary: $primary,
            expected_secondary: $secondary,
            expected_none: $none,
        }
    };
}

/// 完整拒绝矩阵：覆盖所有 DDL 动作类型。总体规律为：
/// - None 角色（未启用 BDR）一律放行；
/// - Secondary 角色几乎全部拒绝（备集群不应发起 DDL），仅放行
///   Placement Policy（副本放置策略）与 Resource Group（资源组）类操作；
/// - Primary 角色放行不影响数据兼容性的操作（如建库建表、加可空列、
///   加非唯一索引），拒绝可能破坏双向同步一致性的操作（如删表、删列、
///   改字符集、TRUNCATE 等）。
const IS_DENIED_ACTION_CASES: &[ActionDeniedCase] = &[
    action_case!(ACTION_CREATE_SCHEMA, false, true, false),
    action_case!(ACTION_DROP_SCHEMA, true, true, false),
    action_case!(ACTION_CREATE_TABLE, false, true, false),
    action_case!(ACTION_DROP_TABLE, true, true, false),
    action_case!(ACTION_ADD_COLUMN, false, true, false),
    action_case!(ACTION_DROP_COLUMN, true, true, false),
    action_case!(ACTION_ADD_INDEX, false, true, false),
    action_case!(ACTION_DROP_INDEX, false, true, false),
    action_case!(ACTION_ADD_FOREIGN_KEY, true, true, false),
    action_case!(ACTION_DROP_FOREIGN_KEY, true, true, false),
    action_case!(ACTION_TRUNCATE_TABLE, true, true, false),
    action_case!(ACTION_MODIFY_COLUMN, false, true, false),
    action_case!(ACTION_REBASE_AUTO_ID, true, true, false),
    action_case!(ACTION_RENAME_TABLE, true, true, false),
    action_case!(ACTION_SET_DEFAULT_VALUE, false, true, false),
    action_case!(ACTION_SHARD_ROW_ID, true, true, false),
    action_case!(ACTION_MODIFY_TABLE_COMMENT, false, true, false),
    action_case!(ACTION_RENAME_INDEX, false, true, false),
    action_case!(ACTION_ADD_TABLE_PARTITION, false, true, false),
    action_case!(ACTION_DROP_TABLE_PARTITION, true, true, false),
    action_case!(ACTION_CREATE_VIEW, false, true, false),
    action_case!(ACTION_MODIFY_TABLE_CHARSET_AND_COLLATE, true, true, false),
    action_case!(ACTION_TRUNCATE_TABLE_PARTITION, true, true, false),
    action_case!(ACTION_DROP_VIEW, false, true, false),
    action_case!(ACTION_RECOVER_TABLE, true, true, false),
    action_case!(ACTION_MODIFY_SCHEMA_CHARSET_AND_COLLATE, true, true, false),
    action_case!(ACTION_LOCK_TABLE, true, true, false),
    action_case!(ACTION_UNLOCK_TABLE, true, true, false),
    action_case!(ACTION_REPAIR_TABLE, true, true, false),
    action_case!(ACTION_SET_TIFLASH_REPLICA, true, true, false),
    action_case!(ACTION_UPDATE_TIFLASH_REPLICA_STATUS, true, true, false),
    action_case!(ACTION_ADD_PRIMARY_KEY, true, true, false),
    action_case!(ACTION_DROP_PRIMARY_KEY, false, true, false),
    action_case!(ACTION_CREATE_SEQUENCE, true, true, false),
    action_case!(ACTION_ALTER_SEQUENCE, true, true, false),
    action_case!(ACTION_DROP_SEQUENCE, true, true, false),
    action_case!(ACTION_MODIFY_TABLE_AUTO_ID_CACHE, true, true, false),
    action_case!(ACTION_REBASE_AUTO_RANDOM_BASE, true, true, false),
    action_case!(ACTION_ALTER_INDEX_VISIBILITY, false, true, false),
    action_case!(ACTION_EXCHANGE_TABLE_PARTITION, true, true, false),
    action_case!(ACTION_ADD_CHECK_CONSTRAINT, true, true, false),
    action_case!(ACTION_DROP_CHECK_CONSTRAINT, true, true, false),
    action_case!(ACTION_ALTER_CHECK_CONSTRAINT, true, true, false),
    action_case!(ACTION_RENAME_TABLES, true, true, false),
    action_case!(ACTION_ALTER_TABLE_ATTRIBUTES, true, true, false),
    action_case!(ACTION_ALTER_TABLE_PARTITION_ATTRIBUTES, true, true, false),
    action_case!(ACTION_CREATE_PLACEMENT_POLICY, false, false, false),
    action_case!(ACTION_ALTER_PLACEMENT_POLICY, false, false, false),
    action_case!(ACTION_DROP_PLACEMENT_POLICY, false, false, false),
    action_case!(ACTION_ALTER_TABLE_PARTITION_PLACEMENT, true, true, false),
    action_case!(ACTION_MODIFY_SCHEMA_DEFAULT_PLACEMENT, true, true, false),
    action_case!(ACTION_ALTER_TABLE_PLACEMENT, true, true, false),
    action_case!(ACTION_ALTER_CACHE_TABLE, true, true, false),
    action_case!(ACTION_ALTER_TABLE_STATS_OPTIONS, true, true, false),
    action_case!(ACTION_ALTER_NO_CACHE_TABLE, true, true, false),
    action_case!(ACTION_CREATE_TABLES, false, true, false),
    action_case!(ACTION_MULTI_SCHEMA_CHANGE, true, true, false),
    action_case!(ACTION_FLASHBACK_CLUSTER, true, true, false),
    action_case!(ACTION_RECOVER_SCHEMA, true, true, false),
    action_case!(ACTION_REORGANIZE_PARTITION, true, true, false),
    action_case!(ACTION_ALTER_TTL_INFO, false, true, false),
    action_case!(ACTION_ALTER_TTL_REMOVE, false, true, false),
    action_case!(ACTION_CREATE_RESOURCE_GROUP, false, false, false),
    action_case!(ACTION_ALTER_RESOURCE_GROUP, false, false, false),
    action_case!(ACTION_DROP_RESOURCE_GROUP, false, false, false),
    action_case!(ACTION_ALTER_TABLE_PARTITIONING, true, true, false),
    action_case!(ACTION_REMOVE_PARTITIONING, true, true, false),
];

// IndexDeniedCase 对应 Go TestIsDenied 最后的 ModifyIndexArgs 特例。
// 它验证 primary role 下加主键、加唯一索引会被拒绝，普通非唯一索引不会。
/// 加索引/加主键场景的特判用例：拒绝与否还取决于索引参数（是否唯一）。
#[derive(Debug, Clone, Copy)]
struct IndexDeniedCase {
    /// DDL 动作类型（加主键或加索引）。
    action: ActionType,
    /// 索引是否为唯一索引。唯一约束在双向复制下可能因两侧并发写入
    /// 而冲突，故 Primary 角色下会被拒绝。
    unique: bool,
    /// 预期结果：true 表示应被拒绝。
    expected: bool,
}

/// 索引特判用例表：Primary 角色下加主键、加唯一索引被拒绝，
/// 加普通（非唯一）索引放行。
const INDEX_DENIED_SPECIAL_CASES: &[IndexDeniedCase] = &[
    IndexDeniedCase {
        action: model_group::ACTION_ADD_PRIMARY_KEY,
        unique: true,
        expected: true,
    },
    IndexDeniedCase {
        action: model_group::ACTION_ADD_INDEX,
        unique: true,
        expected: true,
    },
    IndexDeniedCase {
        action: model_group::ACTION_ADD_INDEX,
        unique: false,
        expected: false,
    },
];

/// 把列选项类型列表转换为 `ColumnOption` 结构体列表，
/// 其余字段取默认值，仅用于构造测试输入。
fn column_options(types: &[ColumnOptionType]) -> Vec<ColumnOption> {
    types
        .iter()
        .map(|tp| ColumnOption {
            Tp: *tp,
            ..Default::default()
        })
        .collect()
}

// test_is_add_column_denied 对应 Go 的 TestIsAddColumnDenied。
/// 遍历加列用例表，逐一断言 `IsAddColumnDenied` 的判定结果。
#[test]
fn test_is_add_column_denied() {
    for case in ADD_COLUMN_DENIED_CASES {
        let options = column_options(case.options);
        let result = IsAddColumnDenied(case.role, &options);
        assert_eq!(case.expected, result, "{}", case.name);
    }
}

// test_is_modify_column_denied 对应 Go 的 TestIsModifyColumnDenied。
/// 遍历改列用例表，构造新旧字段类型后断言 `IsModifyColumnDenied` 的结果。
#[test]
fn test_is_modify_column_denied() {
    for case in MODIFY_COLUMN_DENIED_CASES {
        let new_field_type: FieldType = NewFieldType(case.new_field_type);
        let old_field_type: FieldType = NewFieldType(case.old_field_type);
        let options = column_options(case.options);
        let result = IsModifyColumnDenied(case.role, &new_field_type, &old_field_type, &options);
        assert_eq!(case.expected, result, "{}", case.name);
    }
}

// test_is_denied 对应 Go 的 TestIsDenied。
/// 验证 `IsDenied` 的两部分行为：
/// 1. 主矩阵：每个 DDL 动作在三种 BDR 角色下的拒绝结果（不带索引参数）；
/// 2. 索引特判：构造带 `ModifyIndexArgs` 的 DDL Job（DDL 任务的内部表示），
///    经编码/解码往返后验证唯一索引与主键的拒绝逻辑，且覆盖两个 Job
///    参数版本（V1/V2 序列化格式）。
#[test]
fn test_is_denied() {
    // 第一部分：不带索引参数，逐动作校验三种角色的拒绝矩阵。
    for case in IS_DENIED_ACTION_CASES {
        assert_eq!(
            case.expected_primary,
            IsDenied(BDRRole::Primary, case.action, None),
            "primary {}",
            case.name
        );
        assert_eq!(
            case.expected_secondary,
            IsDenied(BDRRole::Secondary, case.action, None),
            "secondary {}",
            case.name
        );
        assert_eq!(
            case.expected_none,
            IsDenied(BDRRole::None, case.action, None),
            "none {}",
            case.name
        );
    }

    // 第二部分：加索引/加主键特判，需携带索引参数并覆盖两个 Job 版本。
    for case in INDEX_DENIED_SPECIAL_CASES {
        for version in [JobVersion1, JobVersion2] {
            // 根据用例动作选择对应的 Job 动作常量。
            let job_action = if case.action == model_group::ACTION_ADD_PRIMARY_KEY {
                model_job_args::ActionAddPrimaryKey
            } else {
                model_job_args::ActionAddIndex
            };
            // 构造包含单个索引定义的参数：是否唯一/是否主键由用例决定。
            let index_args = ModifyIndexArgs {
                IndexArgs: vec![IndexArg {
                    Unique: case.unique,
                    IsPK: case.action == model_group::ACTION_ADD_PRIMARY_KEY,
                    IndexName: job_ast::NewCIStr("idx1"),
                    IndexPartSpecifications: vec![job_ast::IndexPartSpecification {
                        Length: 2,
                        ..Default::default()
                    }],
                    IndexOption: Some(Box::new(job_ast::IndexOption::default())),
                    SQLMode: job_mysql::ModeANSI,
                    IndexID: 1,
                    ..Default::default()
                }],
                OpType: OpAddIndex,
                ..Default::default()
            };
            let mut job = Job {
                version,
                tp: job_action,
                ..Default::default()
            };
            // 先填充参数并编码，再解码取回参数，模拟 Job 在存储中的
            // 序列化/反序列化往返，确保两个版本格式都能正确解析。
            job.FillArgs(index_args);
            job.Encode(true).expect("encode modify-index job");
            let args = GetModifyIndexArgs(&mut job).expect("decode modify-index job arguments");
            assert_eq!(
                case.expected,
                IsDenied(BDRRole::Primary, case.action, Some(&args)),
                "role: BDRRolePrimary, action: {}, version: {}",
                case.action,
                version,
            );
        }
    }
}
