// Copyright 2026 AsterSQL.

// BDR（Bidirectional Replication，双向复制）场景下的 DDL 检查库。
//
// BDR 指两个集群互为主从、双向同步数据的部署模式。在这种模式下，
// 某些 DDL（数据定义语言，如建表、加索引等修改表结构的语句）可能
// 导致两个集群的表结构不一致，进而引发复制冲突或数据损坏。
// 本 crate 负责根据集群所扮演的 BDR 角色（PRIMARY/SECONDARY）判断
// 一条 DDL 是否允许执行。
//
// 该 crate 是从 Go(TiDB) 机械迁移而来，本文件主要负责：
// - 重新导出（re-export）依赖 crate 中的类型，模拟原 Go 包的引用路径；
// - 声明核心逻辑模块 `bdr` 及其单元测试模块。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 重新导出元数据模型分组 1，对应原 Go 代码中的 `meta/model` 包路径。
pub use meta_model::group_1 as model_group;
/// 重新导出元数据模型分组 2（DDL Job 参数相关），对应原 Go 代码中的 job args 部分。
pub use meta_model::group_2 as model_job_args;

/// AST（抽象语法树）相关类型的兼容命名空间，模拟原 Go 的 `parser/ast` 包路径。
pub mod ast {
    /// BDR 角色类型：标识集群在双向复制中是主集群还是从集群。
    pub use parser_ast::misc::{BDRRole, BdrRole};
    /// 列选项类型：描述列上的约束/属性（如 NOT NULL、DEFAULT、AUTO_INCREMENT 等）。
    pub use parser_ast::{ColumnOption, ColumnOptionType};
}
/// 元数据模型的兼容命名空间，模拟原 Go 的 `meta/model` 包路径。
pub mod model {
    // ActionType 是 DDL 动作类型枚举；ActionBDRMap 是 DDL 动作到 BDR 类别的映射表；
    // SafeDDL / UnmanagementDDL 表示在 BDR 下安全或不受管控的 DDL 类别；
    // ACTION_ADD_INDEX / ACTION_ADD_PRIMARY_KEY 是加索引/加主键的动作常量。
    pub use meta_model::group_1::{
        ACTION_ADD_INDEX, ACTION_ADD_PRIMARY_KEY, ActionBDRMap, ActionType, SafeDDL,
        UnmanagementDDL,
    };
    /// 修改索引类 DDL Job 的参数结构。
    pub use meta_model::group_2::ModifyIndexArgs;
}
/// 字段类型的兼容命名空间，模拟原 Go 的 `parser/types` 包路径。
pub mod types {
    /// 字段类型：描述列的数据类型、长度、标志位等信息。
    pub use parser_types::types::FieldType;
}

/// BDR DDL 检查的核心逻辑实现模块。
mod bdr;
// 将 bdr 模块的公开项直接暴露到 crate 根，供外部以扁平路径引用。
pub use bdr::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "bdr_test.rs"]
mod bdr_test;
