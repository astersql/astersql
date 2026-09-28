// Copyright 2026 AsterSQL.

//! CRR（跨区域复制）测试夹具包入口：装配 stubs/types/builder/仿真器与 harness。
//! 对应 Go `br/pkg/utiltest/crr`，供 stream/checkpoint 相关单测本地搭建上下游。
//! 子模块分工：pd_sim 模拟 PD checkpoint；flush_sim 写元数据；crr_sim 复制事件；harness 组合生命周期。
//! 扁平 `pub use` 降低测试引用路径；stubs 仅提供内存存储抽象，非真实 TiKV。
//! 本文件只声明模块边界与再导出，不含业务逻辑；parity_test 校验与 Go 公开契约一致。
//! builder 负责静态 region 布局；pd_sim_service 暴露可注入的 checkpoint 服务接口。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

#[path = "stubs.rs"]
pub mod stubs;

#[path = "types.rs"]
pub mod types;

#[path = "builder.rs"]
pub mod builder;

#[path = "crr_sim.rs"]
pub mod crr_sim;

#[path = "flush_sim.rs"]
pub mod flush_sim;

#[path = "pd_sim.rs"]
pub mod pd_sim;

#[path = "pd_sim_service.rs"]
pub mod pd_sim_service;

#[path = "harness.rs"]
pub mod harness;

// 再导出仿真核心符号，使 `use utiltest::crr::*` 即可拿到布局/刷盘/复制能力。
pub use builder::*;
pub use crr_sim::*;
pub use flush_sim::*;
pub use harness::*;
pub use pd_sim::*;
// stubs 仅挑选测试常用存储类型，避免把整包桩实现全部暴露到命名空间。
pub use stubs::{
    ArcMemStorage, CancelHandle, Context, Error, LocalStorage, MemStorage, Result, Storage,
};
pub use types::*;

#[cfg(test)]
#[path = "builder_test.rs"]
mod builder_test;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "harness_test.rs"]
mod harness_test;

#[cfg(test)]
#[path = "pd_sim_test.rs"]
mod pd_sim_test;

#[cfg(test)]
#[path = "pd_sim_service_test.rs"]
mod pd_sim_service_test;
