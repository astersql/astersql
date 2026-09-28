// Copyright 2026 AsterSQL.

// `planner_extstore` crate 入口：外部存储（ExtStorage）抽象的模块组装。
//
// ExtStorage 为 Plan Replayer / 诊断导出等场景提供统一的对象存储或本地路径读写接口。
// 本文件负责依赖重导出、`classic`/`nextgen` 内核特性探测，以及测试子模块挂载；
// 具体实现见 `extstore.rs`。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_planner_extstore;

pub use config;
pub use kerneltype;
pub use objstore;
pub use vardef;

/// 测试环境下探测当前是否为 classic 内核（非 `nextgen` feature）。
#[cfg(test)]
mod root_feature_kerneltype {
    #[allow(non_snake_case)]
    pub const fn IsClassic() -> bool {
        !cfg!(feature = "nextgen")
    }
}

/// ExtStorage 生产实现：创建、读写、目录探测与全局单例。
#[path = "extstore.rs"]
mod root_feature_extstore;
pub use root_feature_extstore::*;

/// 迁移对齐用例：与 Go 行为对照的 ExtStorage 操作序列。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// ExtStorage 功能与本地路径可写性探测测试。
#[cfg(test)]
#[path = "extstore_test.rs"]
mod extstore_test;
