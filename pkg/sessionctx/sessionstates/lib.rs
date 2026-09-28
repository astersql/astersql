// Copyright 2026 AsterSQL.

// 会话状态（sessionstates）子 crate 入口。
//
// 导出会话迁移用的状态快照（`session_states`）与代理鉴权令牌（`session_token`）；
// 测试模块仅在 `cfg(test)` 下编译。

#![allow(non_snake_case, non_upper_case_globals)]

/// 会话状态结构体与序列化辅助（用户/系统变量、预处理语句、DDL 信息等）。
pub mod session_states;
/// 会话令牌：证书签名、校验与轮换，供代理在节点间迁移会话时鉴权。
pub mod session_token;
pub use session_states::*;
pub use session_token::*;

/// 串行化会修改进程级签名证书状态的测试，避免并行测试互相污染。
#[cfg(test)]
pub(crate) static SESSION_TOKEN_TEST_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

/// 迁移期综合单元测试：状态常量、令牌签名算法与宽限期等行为对照 Go。
#[cfg(test)]
#[path = "session_states_1_aster_unit_test.rs"]
mod session_states_1_aster_unit_test;
/// 会话状态集成测试原文与 JSON 形状断言（对应 Go `session_states_test.go`）。
#[cfg(test)]
#[path = "session_states_test.rs"]
mod session_states_test;
/// 会话令牌测试原文与 JSON/错误码断言（对应 Go `session_token_test.go`）。
#[cfg(test)]
#[path = "session_token_test.rs"]
mod session_token_test;
