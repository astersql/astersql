// Copyright 2026 AsterSQL.

// config crate 的根模块（对应 TiDB 的 `pkg/config` 包）。
//
// 职责：集中管理数据库服务端的全局配置，包括配置结构体定义、
// 配置文件加载与校验工具、各类配置常量，以及 TiFlash（列式存储副本）、
// 存储引擎（store）、外部负载感知等子系统的配置项。
//
// 组织方式：各子模块分别定义于独立文件，并在此统一 `pub use` 重导出，
// 使外部 crate 可以直接通过 `config::Xxx` 访问全部配置项。

// 允许死代码与 Go 风格命名（驼峰函数名、非大写全局常量），
// 因为本仓库是 Go(TiDB) 到 Rust 的机械迁移基线，保留了原始命名。
#![allow(dead_code, non_snake_case, non_upper_case_globals)]

// 为本 crate 声明自引用别名，便于迁移代码内部以统一路径引用自身。
extern crate self as astersql_config;

/// 核心配置模块：定义服务端主配置结构体及其默认值、加载逻辑。
pub mod config;
pub use config::*;

/// TiKV 客户端配置的精简桩模块。
///
/// TiKV 是 TiDB 体系中的分布式键值存储层；此处仅提供获取
/// 事务作用域（txn scope）的桩实现，固定返回 "global"（全局事务）。
pub mod tikvcfg {
    pub fn GetTxnScopeFromConfig() -> String {
        crate::get_global_config()
            .labels
            .get("zone")
            .cloned()
            .unwrap_or_else(|| "global".to_owned())
    }
}

// 配置工具模块：提供配置项解析、合并与校验等辅助函数。
mod config_util;
pub use config_util::*;
// 配置常量模块：源文件名为 const.rs（const 是 Rust 关键字，
// 无法直接作为模块名，故通过 #[path] 重命名为 config_const）。
#[path = "const.rs"]
mod config_const;
pub use config_const::*;
// 外部负载模块：感知外部工作负载（如后台任务）对资源的影响。
mod external_workload;
pub use external_workload::*;
// keyspace 可观测性模块：keyspace 是多租户场景下的键空间隔离单位，
// 本模块提供其监控/观测相关配置。
mod keyspace_observability;
pub use keyspace_observability::*;
// 存储配置模块：定义存储引擎类型（如 TiKV、单机存储）相关配置。
mod store;
pub use store::*;
// TiFlash 配置模块：TiFlash 是 TiDB 的列式存储副本，用于加速分析型查询（HTAP）。
mod tiflash;
pub use tiflash::*;

// 以下均为测试模块：通过 #[path] 挂载同目录下的测试文件，
// 仅在 cfg(test) 下编译，不影响正常构建。
#[cfg(test)]
#[path = "config_2_aster_unit_test.rs"]
mod config_2_aster_unit_test;
#[cfg(test)]
#[path = "config_test.rs"]
mod config_test;
#[cfg(test)]
#[path = "config_util_1_aster_unit_test.rs"]
mod config_util_1_aster_unit_test;
#[cfg(test)]
#[path = "config_util_test.rs"]
mod config_util_test;
#[cfg(test)]
#[path = "const_3_aster_unit_test.rs"]
mod const_3_aster_unit_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "store_test.rs"]
mod store_test;
