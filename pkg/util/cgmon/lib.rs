// Copyright 2026 AsterSQL.

// cgmon 包入口：cgroup CPU/内存监控与 Prometheus 指标上报。
//
// 通过嵌套 `util::cgroup` 依赖真实探测实现，并重新导出 `cgmon` 公开 API；
// 测试配置下挂载迁移回归与行为测试。

/// 将 cgroup 依赖挂到 `crate::util::cgroup`，与实现中的路径一致。
pub mod util {
    pub mod cgroup {
        pub use cgroup_dependency::*;
    }
}

/// cgroup 监控核心实现。
mod cgmon;

/// 对外重新导出监控器与 Start/Stop 入口。
pub use cgmon::*;

/// 迁移期单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// 无 cgroup 时回退默认值的行为测试。
#[cfg(test)]
#[path = "cgmon_test.rs"]
mod cgmon_test;
