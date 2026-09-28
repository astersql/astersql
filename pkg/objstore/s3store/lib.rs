// Copyright 2026 AsterSQL.

// AWS S3 / 兼容 S3 对象存储客户端 crate 入口。
//
// 聚合底层 API 抽象、重试策略、日志桥接、高层 Client 与 Store/KS3 封装，
// 供备份、导入等路径以统一对象存储接口访问 S3 协议后端。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as s3store;

pub use objectio;
pub use s3like;
pub use storeapi;

/// 备份相关 protobuf 中的 S3 后端配置（复用 s3like 定义）。
pub mod backuppb {
    pub use s3like::backuppb::S3;
}

/// S3 API 请求/响应类型与 AWS SDK 实现。
#[path = "interface.rs"]
mod interface;
pub use interface::*;
#[cfg(test)]
#[path = "interface_test.rs"]
mod interface_test;

/// 带桶前缀的高层 Client（Get/Put/List/Copy/分片上传等）。
#[path = "client.rs"]
mod client;
pub use client::*;

/// S3 客户端重试策略与凭证/区域辅助。
#[path = "retry.rs"]
mod retry;
pub use retry::*;

/// AWS SDK 日志桥接到 tracing。
#[path = "logger.rs"]
mod logger;
pub use logger::*;

/// Store 工厂与后端配置预处理。
#[path = "store.rs"]
mod store;
pub use store::*;

/// 金山云 KS3（兼容 S3）后端封装。
#[path = "ks3.rs"]
mod ks3;
pub use ks3::*;
#[cfg(test)]
#[path = "ks3_test.rs"]
mod ks3_test;

/// Aster 迁移对齐用 Client 综合单元测试。
#[cfg(test)]
#[path = "client_1_aster_unit_test.rs"]
mod client_aster_unit_test;
/// Client 权限检查与对象操作单元测试。
#[cfg(test)]
#[path = "client_test.rs"]
mod client_test;
/// 可注入的 S3API Mock 子模块。
#[path = "mock/lib.rs"]
pub mod mock;
/// 重试策略单元测试。
#[cfg(test)]
#[path = "retry_test.rs"]
mod retry_test;
/// S3 命令行 Flag 解析单元测试。
#[cfg(test)]
#[path = "s3_flags_test.rs"]
mod s3_flags_test;
