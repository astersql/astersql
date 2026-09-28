// Copyright 2026 AsterSQL.

// 实例级资源管理器（Resource Manager）crate 入口。
//
// 负责注册各组件的协程池（Goroutine Pool），按 CPU 观测与调度策略
// 对池容量做升/降档（Overclock / Downclock / Hold），以平衡 DDL、
// DistTask 等后台任务与前台负载的并发资源。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// CPU 利用率观测：为调度器提供升降档依据。
pub mod cpu {
    pub use cpu_dependency::{NewCPUObserver, Observer};
}
/// 调度命令与调度器实现（Hold / Downclock / Overclock）。
pub mod scheduler {
    pub use scheduler_dependency::*;
}
/// 分片池映射内部实现。
#[path = "util/shard_pool_map.rs"]
mod shard_pool_map;
/// 工具类型：分片池映射、组件枚举、池容器等。
pub mod util {
    pub use crate::shard_pool_map::{NewShardPoolMap, PoolMapError, ShardPoolMap};
    pub use scheduler_dependency::util::*;
}
/// 资源管理器核心：注册/注销池、周期调度与 Exec。
pub mod rm;
/// 调度循环与后台生命周期（Start / Stop）。
mod schedule;
/// WaitGroup 包装，便于测试与优雅停机。
#[path = "../util/wait_group_wrapper.rs"]
pub mod wait_group_wrapper;
pub use rm::{InstanceResourceManager, NewResourceManger, RandomName, ResourceManager};

/// 测试用 Mock 协程池。
#[cfg(test)]
#[path = "util/mock_gpool.rs"]
mod mock_gpool;

/// 调度相关测试的依赖聚合命名空间（对应 Go 测试包路径习惯）。
#[cfg(test)]
pub mod resourcemanager_test_support {
    pub use crate::{NewResourceManger, scheduler};
    pub mod util {
        pub use crate::mock_gpool::{MockGPool, NewMockGPool};
        pub use crate::util::*;
    }
}

/// 调度器行为单测。
#[cfg(test)]
mod schedule_test {
    include!("schedule_test.rs");
    use crate::resourcemanager_test_support;
}

/// 迁移对照单测：注册、调度守卫、Exec 边界与生命周期。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
