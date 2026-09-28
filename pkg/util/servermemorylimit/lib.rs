// Copyright 2026 AsterSQL.

// servermemorylimit crate 根：服务端全局内存上限与会话 kill 治理。
//
// 依赖再导出 memory/sessmgr/sqlkiller 等；核心逻辑在 `servermemorylimit` 模块。
// 测试下挂载迁移回归与 Go 对照单测。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 后台日志工具再导出。
pub mod logutil {
    pub use logutil_dependency::log::*;
}

/// 内存追踪与全局仲裁相关再导出。
pub mod memory {
    pub use memory_dependency::global_arbitrator::{
        HandleGlobalMemArbitratorRuntime, UsingGlobalMemArbitration,
    };
    pub use memory_dependency::memstats::{MemStats, ReadMemStats};
    pub use memory_dependency::tracker::*;
}

/// MySQL 类型常量再导出（排序规则名、Datetime 类型码等）。
pub mod mysql {
    pub use mysql_dependency::charset::DefaultCollationName;
    pub use mysql_dependency::r#type::TypeDatetime;
}

/// 会话管理器接口再导出（ProcessInfo、Manager）。
pub mod sessmgr {
    pub use sessmgr_dependency::*;
}

/// SQL Killer：向会话发送 kill 信号以中止超限查询。
pub mod sqlkiller {
    pub use sqlkiller_dependency::sqlkiller::*;
}

/// Datum / 时间构造再导出，供历史行编码。
pub mod types {
    pub use types_datum::*;
    pub use types_time::FromGoTime;
}

/// 内存上限控制器实现。
pub mod servermemorylimit;
pub use servermemorylimit::*;

#[cfg(test)]
/// AsterSQL 迁移补充回归测试。
mod migration_aster_unit_test {
    use super::servermemorylimit::*;
    include!("migration_aster_unit_test.rs");
}

#[cfg(test)]
/// 对应 Go `servermemorylimit_test.go` 的历史环形缓冲测试。
mod servermemorylimit_test {
    use super::*;
    include!("servermemorylimit_test.rs");
}
