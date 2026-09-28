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

// BDR（Bi-Directional Replication，双向复制）DDL 拦截规则的单元测试。
//
// BDR 指两个集群之间互为主备、双向同步数据的部署形态。在这种形态下，
// 部分 DDL（数据定义语句，如建表、加列等）若在主/从集群随意执行，
// 可能导致两侧 schema（表结构）不一致或数据损坏，因此需要按集群角色
// （Primary 主 / Secondary 从 / None 未启用 BDR）对 DDL 动作进行放行或拒绝。
// 本测试文件验证 Rust 迁移版的拦截规则与原 Go(TiDB) 实现完全一致，覆盖：
// - 加列（ADD COLUMN）时列选项的允许/拒绝规则；
// - 改列（MODIFY COLUMN）时类型变更与选项组合的规则；
// - 全量 DDL 动作矩阵（安全/不安全/不受 BDR 管控三类）在各角色下的判定；
// - 添加唯一索引/主键这类特殊动作的额外判定。

// 引入被测模块（bdr 模块本体）以及 DDL 动作常量、索引参数结构。
use super::*;
use model_group as actions;
use model_job_args::{
    self, GetModifyIndexArgs, IndexArg, Job, JobArgsCompat, JobVersion1, JobVersion2,
    ModifyIndexArgs, OpAddIndex, ast as job_ast, mysql as job_mysql,
};

/// 测试辅助函数：构造一个仅设置了选项类型 `tp` 的列选项（ColumnOption），
/// 其余字段取默认值，用于模拟 `ADD/MODIFY COLUMN` 语句中的各种列属性
/// （如 NOT NULL、DEFAULT、COMMENT 等）。
fn option(tp: ast::ColumnOptionType) -> ast::ColumnOption {
    ast::ColumnOption {
        Tp: tp,
        ..Default::default()
    }
}

/// 验证加列（ADD COLUMN）的 BDR 拒绝规则与 Go 版一致。
///
/// 规则要点：在 Primary（主集群）角色下，只有"可空列"或"带默认值的列"等
/// 不影响存量数据回放的加列操作被允许；出现不受支持的列选项（如 CHECK 约束）
/// 则拒绝。Secondary/None 角色对该函数直接放行（由动作矩阵另行管控）。
#[test]
fn add_column_rules_match_go() {
    use self::ast::ColumnOptionType as OptionType;

    // 用例元组含义：(用例名, BDR 角色, 列选项列表, 期望是否被拒绝)。
    let cases = [
        ("implicit nullable", ast::BDRRole::Primary, vec![], false),
        (
            "explicit nullable",
            ast::BDRRole::Primary,
            vec![option(OptionType::Null)],
            false,
        ),
        (
            "implicit nullable with default",
            ast::BDRRole::Primary,
            vec![option(OptionType::DefaultValue)],
            false,
        ),
        (
            "not null with default",
            ast::BDRRole::Primary,
            vec![
                option(OptionType::NotNull),
                option(OptionType::DefaultValue),
            ],
            false,
        ),
        (
            "comment only",
            ast::BDRRole::Primary,
            vec![option(OptionType::Comment)],
            false,
        ),
        (
            "generated only",
            ast::BDRRole::Primary,
            vec![option(OptionType::Generated)],
            false,
        ),
        (
            "comment and generated",
            ast::BDRRole::Primary,
            vec![option(OptionType::Comment), option(OptionType::Generated)],
            false,
        ),
        (
            "unsupported option",
            ast::BDRRole::Primary,
            vec![option(OptionType::Check)],
            true,
        ),
        (
            "secondary bypass",
            ast::BDRRole::Secondary,
            vec![option(OptionType::Check)],
            false,
        ),
        (
            "none bypass",
            ast::BDRRole::None,
            vec![option(OptionType::Check)],
            false,
        ),
    ];

    // 逐用例断言 IsAddColumnDenied 的判定结果与期望一致。
    for (name, role, options, expected) in cases {
        assert_eq!(IsAddColumnDenied(role, &options), expected, "{name}");
    }
}

/// 验证改列（MODIFY COLUMN）的 BDR 拒绝规则与 Go 版一致。
///
/// 规则要点：在 Primary 角色下，改变列类型（如 LONG 改 VARCHAR）会被拒绝，
/// 因为类型变更可能需要重写数据、破坏双向同步；仅修改默认值
///（可附带 COMMENT）被允许；只改 COMMENT 而不改默认值也会被拒绝。
/// Secondary/None 角色同样直接放行。
#[test]
fn modify_column_rules_match_go() {
    use self::ast::ColumnOptionType as OptionType;

    // 构造两种字段类型（FieldType 描述列的 MySQL 类型信息）：
    // long 为整型 TypeLong，varchar 为变长字符串 TypeVarchar，
    // 用于模拟"类型未变"与"类型改变"两种场景。
    let mut long = types::FieldType::default();
    long.SetType(parser_types::mysql::TypeLong);
    let mut varchar = types::FieldType::default();
    varchar.SetType(parser_types::mysql::TypeVarchar);

    // 用例元组含义：(用例名, BDR 角色, 新类型, 旧类型, 列选项列表, 期望是否被拒绝)。
    let cases = [
        (
            "changed type",
            ast::BDRRole::Primary,
            &long,
            &varchar,
            vec![],
            true,
        ),
        (
            "default only",
            ast::BDRRole::Primary,
            &long,
            &long,
            vec![option(OptionType::DefaultValue)],
            false,
        ),
        (
            "default and comment",
            ast::BDRRole::Primary,
            &long,
            &long,
            vec![
                option(OptionType::DefaultValue),
                option(OptionType::Comment),
            ],
            false,
        ),
        (
            "comment only",
            ast::BDRRole::Primary,
            &long,
            &long,
            vec![option(OptionType::Comment)],
            true,
        ),
        (
            "secondary bypass",
            ast::BDRRole::Secondary,
            &long,
            &varchar,
            vec![],
            false,
        ),
        (
            "none bypass",
            ast::BDRRole::None,
            &long,
            &varchar,
            vec![],
            false,
        ),
    ];

    // 逐用例断言 IsModifyColumnDenied 的判定结果与期望一致。
    for (name, role, new_type, old_type, options, expected) in cases {
        assert_eq!(
            IsModifyColumnDenied(role, new_type, old_type, &options),
            expected,
            "{name}"
        );
    }
}

/// BDR 场景下被视为"安全"的 DDL 动作集合：
/// 这些动作（如建库、建表、加列、加/删索引等）不会破坏双向复制的一致性，
/// 在 Primary 角色下允许执行；但在 Secondary（从集群）上仍会被拒绝，
/// 因为 BDR 要求所有 schema 变更统一从主集群发起。
const SAFE_ACTIONS: &[actions::ActionType] = &[
    actions::ACTION_CREATE_SCHEMA,
    actions::ACTION_CREATE_TABLE,
    actions::ACTION_ADD_COLUMN,
    actions::ACTION_ADD_INDEX,
    actions::ACTION_DROP_INDEX,
    actions::ACTION_MODIFY_COLUMN,
    actions::ACTION_SET_DEFAULT_VALUE,
    actions::ACTION_MODIFY_TABLE_COMMENT,
    actions::ACTION_RENAME_INDEX,
    actions::ACTION_ADD_TABLE_PARTITION,
    actions::ACTION_CREATE_VIEW,
    actions::ACTION_DROP_VIEW,
    actions::ACTION_DROP_PRIMARY_KEY,
    actions::ACTION_ALTER_INDEX_VISIBILITY,
    actions::ACTION_CREATE_TABLES,
    actions::ACTION_ALTER_TTL_INFO,
    actions::ACTION_ALTER_TTL_REMOVE,
    actions::ACTION_ALTER_TABLE_AFFINITY,
];

/// BDR 场景下被视为"不安全"的 DDL 动作集合：
/// 这些动作（如删库、删表、删列、TRUNCATE、交换分区、锁表等）会直接影响
/// 存量数据或复制拓扑，在 Primary 与 Secondary 角色下都会被拒绝，
/// 只有未启用 BDR（None 角色）时才放行。
const UNSAFE_ACTIONS: &[actions::ActionType] = &[
    actions::ACTION_DROP_SCHEMA,
    actions::ACTION_DROP_TABLE,
    actions::ACTION_DROP_COLUMN,
    actions::ACTION_ADD_FOREIGN_KEY,
    actions::ACTION_DROP_FOREIGN_KEY,
    actions::ACTION_TRUNCATE_TABLE,
    actions::ACTION_REBASE_AUTO_ID,
    actions::ACTION_RENAME_TABLE,
    actions::ACTION_SHARD_ROW_ID,
    actions::ACTION_DROP_TABLE_PARTITION,
    actions::ACTION_MODIFY_TABLE_CHARSET_AND_COLLATE,
    actions::ACTION_TRUNCATE_TABLE_PARTITION,
    actions::ACTION_RECOVER_TABLE,
    actions::ACTION_MODIFY_SCHEMA_CHARSET_AND_COLLATE,
    actions::ACTION_LOCK_TABLE,
    actions::ACTION_UNLOCK_TABLE,
    actions::ACTION_REPAIR_TABLE,
    actions::ACTION_SET_TIFLASH_REPLICA,
    actions::ACTION_UPDATE_TIFLASH_REPLICA_STATUS,
    actions::ACTION_ADD_PRIMARY_KEY,
    actions::ACTION_CREATE_SEQUENCE,
    actions::ACTION_ALTER_SEQUENCE,
    actions::ACTION_DROP_SEQUENCE,
    actions::ACTION_MODIFY_TABLE_AUTO_ID_CACHE,
    actions::ACTION_REBASE_AUTO_RANDOM_BASE,
    actions::ACTION_EXCHANGE_TABLE_PARTITION,
    actions::ACTION_ADD_CHECK_CONSTRAINT,
    actions::ACTION_DROP_CHECK_CONSTRAINT,
    actions::ACTION_ALTER_CHECK_CONSTRAINT,
    actions::ACTION_RENAME_TABLES,
    actions::ACTION_ALTER_TABLE_ATTRIBUTES,
    actions::ACTION_ALTER_TABLE_PARTITION_ATTRIBUTES,
    actions::ACTION_ALTER_TABLE_PARTITION_PLACEMENT,
    actions::ACTION_MODIFY_SCHEMA_DEFAULT_PLACEMENT,
    actions::ACTION_ALTER_TABLE_PLACEMENT,
    actions::ACTION_ALTER_CACHE_TABLE,
    actions::ACTION_ALTER_TABLE_STATS_OPTIONS,
    actions::ACTION_ALTER_NO_CACHE_TABLE,
    actions::ACTION_MULTI_SCHEMA_CHANGE,
    actions::ACTION_FLASHBACK_CLUSTER,
    actions::ACTION_RECOVER_SCHEMA,
    actions::ACTION_REORGANIZE_PARTITION,
    actions::ACTION_ALTER_TABLE_PARTITIONING,
    actions::ACTION_REMOVE_PARTITIONING,
    actions::ACTION_ADD_COLUMNAR_INDEX,
    actions::ACTION_MODIFY_ENGINE_ATTRIBUTE,
    actions::ACTION_ALTER_TABLE_MODE,
    actions::ACTION_REFRESH_META,
    actions::ACTION_MODIFY_SCHEMA_READ_ONLY,
    actions::ACTION_MODIFY_SCHEMA_SOFT_DELETE_AND_ACTIVE_ACTIVE,
    actions::ACTION_ALTER_TABLE_SOFT_DELETE_INFO,
    actions::ACTION_ALTER_TABLE_SET_REGION_SPLIT_POLICY,
];

/// 不受 BDR 管控的 DDL 动作集合：
/// 放置策略（Placement Policy，控制数据副本物理分布）、资源组
///（Resource Group，资源隔离与限流）、脱敏策略（Masking Policy）等
/// 属于集群本地配置，不参与双向复制，因此在任何角色下都放行。
const UNMANAGED_ACTIONS: &[actions::ActionType] = &[
    actions::ACTION_CREATE_PLACEMENT_POLICY,
    actions::ACTION_ALTER_PLACEMENT_POLICY,
    actions::ACTION_DROP_PLACEMENT_POLICY,
    actions::ACTION_CREATE_RESOURCE_GROUP,
    actions::ACTION_ALTER_RESOURCE_GROUP,
    actions::ACTION_DROP_RESOURCE_GROUP,
    actions::ACTION_CREATE_MASKING_POLICY,
    actions::ACTION_ALTER_MASKING_POLICY,
    actions::ACTION_DROP_MASKING_POLICY,
];

/// 验证 DDL 动作矩阵在三种 BDR 角色下的判定与 Go 版完全一致：
/// - 安全动作：Primary 放行、Secondary 拒绝、None 放行；
/// - 不安全动作：Primary/Secondary 均拒绝、None 放行；
/// - 不受管控动作：三种角色均放行；
/// - 未知动作：按最保守策略处理（Primary/Secondary 拒绝）。
#[test]
fn ddl_action_matrix_matches_go() {
    // 安全动作：仅 Secondary 拒绝。
    for &action in SAFE_ACTIONS {
        assert!(
            !IsDenied(ast::BDRRole::Primary, action, None),
            "primary safe action {action}"
        );
        assert!(
            IsDenied(ast::BDRRole::Secondary, action, None),
            "secondary safe action {action}"
        );
        assert!(
            !IsDenied(ast::BDRRole::None, action, None),
            "none safe action {action}"
        );
    }
    // 不安全动作：Primary 与 Secondary 均拒绝。
    for &action in UNSAFE_ACTIONS {
        assert!(
            IsDenied(ast::BDRRole::Primary, action, None),
            "primary unsafe action {action}"
        );
        assert!(
            IsDenied(ast::BDRRole::Secondary, action, None),
            "secondary unsafe action {action}"
        );
        assert!(
            !IsDenied(ast::BDRRole::None, action, None),
            "none unsafe action {action}"
        );
    }
    // 不受管控动作：任何角色都放行。
    for &action in UNMANAGED_ACTIONS {
        assert!(
            !IsDenied(ast::BDRRole::Primary, action, None),
            "primary unmanaged action {action}"
        );
        assert!(
            !IsDenied(ast::BDRRole::Secondary, action, None),
            "secondary unmanaged action {action}"
        );
        assert!(
            !IsDenied(ast::BDRRole::None, action, None),
            "none unmanaged action {action}"
        );
    }

    // 未知动作编号（250 不在任何已知动作集合内）：
    // 采取保守策略，Primary/Secondary 一律拒绝，None 放行。
    let unknown_action = 250;
    assert!(IsDenied(ast::BDRRole::Primary, unknown_action, None));
    assert!(IsDenied(ast::BDRRole::Secondary, unknown_action, None));
    assert!(!IsDenied(ast::BDRRole::None, unknown_action, None));
}

/// 验证唯一索引/主键相关的特殊判定与 Go 版一致：
/// 添加主键（ADD PRIMARY KEY）或唯一索引（unique=true 的 ADD INDEX）
/// 在 Primary 角色下会被拒绝——唯一性约束可能与从集群回放的数据冲突；
/// 普通（非唯一）索引则允许添加。与 Go 测试相同，两个 Job 参数版本都要
/// 经 FillArgs/Encode/GetModifyIndexArgs 往返后再调用被测函数。
#[test]
fn unique_index_special_cases_match_go() {
    // 用例元组含义：(DDL 动作, 是否唯一索引, 期望是否被拒绝)。
    let cases = [
        (actions::ACTION_ADD_PRIMARY_KEY, true, true),
        (actions::ACTION_ADD_INDEX, true, true),
        (actions::ACTION_ADD_INDEX, false, false),
    ];

    for (action, unique, expected) in cases {
        for version in [JobVersion1, JobVersion2] {
            let job_action = if action == actions::ACTION_ADD_PRIMARY_KEY {
                model_job_args::ActionAddPrimaryKey
            } else {
                model_job_args::ActionAddIndex
            };
            let args = ModifyIndexArgs {
                IndexArgs: vec![IndexArg {
                    Unique: unique,
                    IsPK: action == actions::ACTION_ADD_PRIMARY_KEY,
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
            job.FillArgs(args);
            job.Encode(true).expect("encode modify-index job");
            let decoded = GetModifyIndexArgs(&mut job).expect("decode modify-index job arguments");
            assert_eq!(
                IsDenied(ast::BDRRole::Primary, action, Some(&decoded)),
                expected,
                "action {action}, unique {unique}, version {version}"
            );
        }
    }
}

/// Go's string-backed BDRRole allows unknown values and treats them as bypass.
#[test]
fn unknown_bdr_role_bypasses_all_guards() {
    let unknown = ast::BDRRole::Unknown;
    let field_type = types::FieldType::default();
    assert!(!IsAddColumnDenied(unknown, &[]));
    assert!(!IsModifyColumnDenied(
        unknown,
        &field_type,
        &field_type,
        &[]
    ));
    assert!(!IsDenied(unknown, actions::ACTION_DROP_TABLE, None));
}
