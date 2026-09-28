// Copyright 2026 AsterSQL.

// sqlsvrapi mock crate 入口：再导出依赖并挂载 mockall 生成的桩。
//
// 通过 `domain::sqlsvrapi::mock` 路径聚合 `ksruntime_mock` / `runtime_mock` /
// `server_mock`，供迁移测试按 Go 包路径风格引用。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]
/// KV 依赖再导出。
pub mod kv {
    pub use sqlsvrapi_dependency::kv::*;
}
/// 元数据模型再导出。
pub mod meta {
    pub mod model {
        pub use sqlsvrapi_dependency::meta::model::*;
    }
}
/// DDL owner 再导出。
pub mod owner {
    pub use sqlsvrapi_dependency::owner::*;
}
/// util（含 session pool）再导出。
pub mod util {
    pub use sqlsvrapi_dependency::util::*;
}
/// KSRuntimeHandle mock。
pub mod ksruntime_mock;
/// Runtime mock。
pub mod runtime_mock;
/// Server mock。
pub mod server_mock;
/// 模拟 Go 包路径 `domain/sqlsvrapi` 与其 mock 子包。
pub mod domain {
    pub mod sqlsvrapi {
        pub use sqlsvrapi_dependency::server::*;
        pub mod mock {
            pub use crate::{ksruntime_mock::*, runtime_mock::*, server_mock::*};
        }
    }
}
/// 测试支持包别名再导出。
pub use sqlsvrapi_dependency::{kv_test_support, owner_test_support, util_test_support};

/// mockall 期望录制与转发行为的迁移期单测。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
