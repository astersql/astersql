// Copyright 2026 AsterSQL.

// DDL 模式加载器的公共抽象。
//
// DDL 调度器通过该接口触发 InfoSchema 重载，而具体加载过程由注入的实现负责，
// 从而将调度逻辑与元数据存储及缓存刷新细节解耦。

use std::fmt;

/// DDL 调度器触发模式重载时返回的统一错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaLoaderError(String);

impl SchemaLoaderError {
    /// 保留底层加载器的诊断信息，供调用方展示或继续传递。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for SchemaLoaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SchemaLoaderError {}

/// DDL 变更后重新加载生产 InfoSchema 的能力。
///
/// 实现需要可在线程间共享，以便 DDL 调度组件持有并调用同一加载器实例。
pub trait SchemaLoader: Send + Sync {
    /// 从元数据存储重新加载模式信息，使 InfoSchema 与最新 DDL 结果一致。
    fn reload(&self) -> Result<(), SchemaLoaderError>;
}
