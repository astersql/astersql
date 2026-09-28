// Copyright 2026 AsterSQL.

// 阿里云 OSS（Object Storage Service，对象存储服务）客户端 crate 入口。
//
// 聚合凭证刷新、底层 API 抽象、重试策略、高层 Client 与 Store 封装，
// 供备份/导入等路径以统一对象存储接口访问 OSS。

#![allow(non_snake_case, non_upper_case_globals)]

/// OSS 底层 API 请求/响应类型与阿里云实现。
mod interface;
pub use interface::*;
/// 访问密钥提供者与并发安全的凭证刷新器。
mod credential;
pub use credential::*;
/// OSS SDK 日志桥接到仓库 log 门面。
mod logger;
pub use logger::*;
/// 可重试错误判定与退避延迟。
mod retry;
pub use retry::*;
/// 带桶前缀的高层 Client（Get/Put/List/Copy 等）。
mod client;
pub use client::*;
/// Store 工厂、endpoint/region 辅助与后端配置预处理。
mod store;
pub use store::*;

/// 迁移对齐用综合单元测试（权限、请求映射、分片上传等）。
#[cfg(test)]
mod migration_aster_unit_test {
    include!("migration_aster_unit_test.rs");
}

/// OSS retry classification parity tests.
#[cfg(test)]
mod retry_test {
    include!("retry_test.rs");
}

/// OSS SDK 适配层的响应元数据对齐测试。
#[cfg(test)]
mod interface_test {
    include!("interface_test.rs");
}

/// Client 权限检查与对象操作单元测试。
#[cfg(test)]
mod client_test {
    use crate as task_ossstore;
    include!("client_test.rs");
}

/// CredentialRefresher 快照刷新单元测试。
#[cfg(test)]
mod credential_test {
    use crate as task_ossstore;
    include!("credential_test.rs");
}

/// Store 相关单元测试。
#[cfg(test)]
mod store_test {
    use crate as task_ossstore;
    include!("store_test.rs");
}
