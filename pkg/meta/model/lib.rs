// Copyright 2026 AsterSQL.

// `meta/model` 包根：聚合 group1–group4 并再导出完整 Go 模型身份。
//
// Group1 拥有 column/index/table 等正式定义；Group4 仅为兼容再导出。
// Group2/Group3 提供 Job 参数与 Job/Placement/Reorg 等编译边界。
// 测试模块通过 `#[path]` 挂到本 crate，覆盖 BDR、列、索引、Job 与 Placement 等。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// Group1：完整表模型身份（列、索引、表等）。
pub mod group_1 {
    pub use ::group_1::*;
}

/// Group2：Job 参数 V1/V2 兼容适配；Job 身份复用 Group3 完整模型。
pub mod group_2 {
    pub use ::group_2::*;
}

/// Group3：Job / Placement / Reorg / ResourceGroup / TableMode。
pub mod group_3 {
    pub use ::group_3::*;
}

/// Group4：对 Group1 的兼容再导出，不维护第二套元数据实现。
pub mod group_4 {
    pub use ::group_4::*;
}

// Group 1 owns the complete Go model identity: the real `column.rs`,
// `index.rs`, and `table.rs` compile together in one crate. Group 4 is now a
// one-way compatibility re-export of the same definitions, never a second
// metadata implementation.
// Group1 拥有完整 Go 模型身份：真实的 column/index/table 同 crate 编译。
// Group4 现为同一套定义的单向兼容再导出，绝不是第二套元数据实现。
pub use ::group_1::*;

#[cfg(test)]
#[path = "bdr_1_aster_unit_test.rs"]
mod bdr_1_aster_unit_test;
#[cfg(test)]
#[path = "bdr_test.rs"]
mod bdr_test;
#[cfg(test)]
#[path = "column_test.rs"]
mod column_test;
#[cfg(test)]
#[path = "db_test.rs"]
mod db_test;
#[cfg(test)]
#[path = "dependency_tests.rs"]
mod dependency_tests;
#[cfg(test)]
#[path = "go_merge_15_test.rs"]
mod go_merge_15_test;
#[cfg(test)]
#[path = "go_merge_18_test.rs"]
mod go_merge_18_test;
#[cfg(test)]
#[path = "index_test.rs"]
mod index_test;
#[cfg(test)]
#[path = "job_3_aster_unit_test.rs"]
mod job_3_aster_unit_test;
#[cfg(test)]
#[path = "job_args_test.rs"]
mod job_args_test;
#[cfg(test)]
#[path = "job_test.rs"]
mod job_test;
#[cfg(test)]
#[path = "masking_policy_test.rs"]
mod masking_policy_test;
#[cfg(test)]
#[path = "model_identity_aster_unit_test.rs"]
mod model_identity_aster_unit_test;
#[cfg(test)]
#[path = "placement_test.rs"]
mod placement_test;
#[cfg(test)]
#[path = "reorg_test.rs"]
mod reorg_test;
#[cfg(test)]
#[path = "resource_group_test.rs"]
mod resource_group_test;
#[cfg(test)]
#[path = "table_4_aster_unit_test.rs"]
mod table_4_aster_unit_test;
#[cfg(test)]
#[path = "table_mode_test.rs"]
mod table_mode_test;
#[cfg(test)]
#[path = "table_test.rs"]
mod table_test;
