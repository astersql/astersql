// Copyright 2026 AsterSQL.

// Timer API crate 根模块。
//
// 导出客户端、错误、Hook、内存存储、存储抽象与定时器模型等子模块，
// 并在测试配置下 `include!` 各测试源文件。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

/// 定时器客户端接口与默认实现。
pub mod client;
/// 错误类型与结果别名。
pub mod error;
/// 调度事件 Hook。
pub mod hook;
/// 内存版 TimerStore 实现。
pub mod mem_store;
/// 存储抽象、条件、更新与 Watch。
pub mod store;
/// 定时器记录、调度策略与时间工具。
pub mod timer;

// 再导出各子模块公共 API，便于 `use timer_api::*`。
pub use client::*;
pub use error::*;
pub use hook::*;
pub use mem_store::*;
pub use store::*;
pub use timer::*;

#[cfg(test)]
/// Hook API parity tests.
mod hook_test {
    use super::*;
    include!("hook_test.rs");
}

#[cfg(test)]
/// Aster 侧补充的客户端单元测试。
mod client_1_aster_unit_test {
    use super::*;
    include!("client_1_aster_unit_test.rs");
}

#[cfg(test)]
/// 客户端选项与默认客户端行为测试。
mod client_test {
    use super::*;
    include!("client_test.rs");
}

#[cfg(test)]
/// 间隔与 Cron 调度策略测试。
mod schedule_policy_test {
    use super::*;
    include!("schedule_policy_test.rs");
}

#[cfg(test)]
/// 存储条件、更新与 Watch 测试。
mod store_test {
    use super::*;
    include!("store_test.rs");
}

#[cfg(test)]
/// 定时器模型与策略单元测试。
mod timer_test {
    use super::*;
    include!("timer_test.rs");
}
