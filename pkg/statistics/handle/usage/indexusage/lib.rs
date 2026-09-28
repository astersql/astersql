// Copyright 2026 AsterSQL.

// 索引使用率（indexusage）crate 入口。
//
// 声明对统计 handle / 元数据模型的模块路径别名，导出 `collector`
// 中的节点/会话/语句级采集 API，并挂载相关单元测试。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 与仓库内 `statistics::handle::usage::collector` 路径对齐的桩模块树。
pub mod statistics {
    pub mod handle {
        pub mod usage {
            pub use usage_collector as collector;
        }
    }
}
/// 元数据模型别名：表/索引信息，供 GC 查找使用。
pub mod meta {
    pub mod model {
        pub use meta_model::group_4::{IndexInfo, TableInfo};
    }
}

mod collector;
pub use collector::*;
pub use meta::model::{IndexInfo, TableInfo};

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "collector_test.rs"]
mod collector_test;
