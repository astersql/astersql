// Copyright 2026 AsterSQL.

// 统计 handle util 测试夹具库。
//
// 提供精简的 `Context`、错误与 kv 请求源辅助，并导出 `CtxMatcher` 供单元测试断言事务来源。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 测试用上下文，可携带可选的请求源信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Context {
    request_source: Option<option_impl::RequestSource>,
}
impl Context {
    /// 返回当前绑定的请求源（若有）。
    pub fn RequestSource(&self) -> Option<&option_impl::RequestSource> {
        self.request_source.as_ref()
    }
    /// 设置请求源后返回自身（建造者模式，供 kv 辅助函数链式调用）。
    fn with_request_source(mut self, request_source: option_impl::RequestSource) -> Self {
        self.request_source = Some(request_source);
        self
    }
}
/// 简单错误包装，兼容 Go `errors.New` 风格。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error(String);
/// 错误构造辅助模块。
pub mod errors {
    use crate::Error;

    /// Shared error type used by the included transaction option contracts.
    pub type SharedError = Error;

    /// 用消息字符串构造 `Error`。
    pub fn New(message: impl Into<String>) -> Error {
        Error(message.into())
    }
}
/// 再导出 `Context`，模拟 Go `context` 包路径。
pub mod context {
    pub use crate::Context;
}
#[path = "../../../../kv/option.rs"]
mod option_impl;
/// 再导出统计相关内部事务源常量与注入函数。
pub mod kv {
    pub use crate::option_impl::{
        InternalTxnStats, InternalTxnStatsForegroundPriority, WithInternalSourceAndTaskType,
        WithInternalSourceType,
    };
}
mod ctx_matcher;
pub use ctx_matcher::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
