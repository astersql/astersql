// Copyright 2026 AsterSQL.

// store driver 请求选项（replica read 等）crate 入口。
//
// 提供轻量 Error 占位、内嵌 `kv/option.rs`，以及携带 RequestSource 的 Context，
// 并导出 `options` 模块中的副本读策略映射（TiKVReplicaReadType）。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 本 crate 内简化错误类型（字符串消息）。
pub type Error = String;
/// 构造错误字符串的辅助模块。
pub mod errors {
    /// Shared error type used by the included transaction option contracts.
    pub type SharedError = crate::Error;

    /// 由任意可转 String 的消息创建错误。
    pub fn New(message: impl Into<String>) -> String {
        message.into()
    }
}
/// KV 请求选项定义（内嵌 `kv/option.rs`，含 ReplicaReadType 等）。
pub mod kv {
    include!("../../../kv/option.rs");
}
/// RPC / 存储请求上下文，可附带 RequestSource（请求来源标签）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Context {
    request_source: Option<kv::RequestSource>,
}
impl Context {
    /// 返回当前请求来源（若已设置）。
    pub fn RequestSource(&self) -> Option<&kv::RequestSource> {
        self.request_source.as_ref()
    }
    /// 构建时附带 RequestSource。
    fn with_request_source(mut self, request_source: kv::RequestSource) -> Self {
        self.request_source = Some(request_source);
        self
    }
}
/// 副本读策略等选项实现。
mod options;
pub use options::*;

/// Aster 迁移对照的 options 单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
