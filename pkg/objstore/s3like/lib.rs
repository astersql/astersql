// Copyright 2026 AsterSQL.

// S3 兼容对象存储公共库入口（s3like）。
//
// 聚合接口、指标、重试、存储实现、IO 适配与权限检查，并向外 re-export。
// 同时内嵌 `backuppb::S3` 配置结构与轻量 `pflag::FlagSet`，对齐 Go 侧
// 备份恢复（BR）配置与命令行标志解析路径。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as s3like;
use std::collections::HashMap;

pub use objectio;

/// 备份协议中的 S3 后端配置字段集合（对齐 backuppb.S3）。
pub mod backuppb {
    /// S3/兼容存储连接与加密相关选项。
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    pub struct S3 {
        /// 服务端点 URL。
        pub Endpoint: String,
        /// 区域名。
        pub Region: String,
        /// 桶名。
        pub Bucket: String,
        /// 对象键公共前缀。
        pub Prefix: String,
        /// 存储类别（如 STANDARD）。
        pub StorageClass: String,
        /// 服务端加密算法标识。
        pub Sse: String,
        /// KMS 密钥 ID（SSE-KMS）。
        pub SseKmsKeyId: String,
        /// 对象 ACL。
        pub Acl: String,
        /// 访问密钥。
        pub AccessKey: String,
        /// 秘密密钥。
        pub SecretAccessKey: String,
        /// 临时会话令牌。
        pub SessionToken: String,
        /// 是否强制 path-style 寻址。
        pub ForcePathStyle: bool,
        /// 扮演角色 ARN。
        pub RoleArn: String,
        /// 外部 ID（STS AssumeRole）。
        pub ExternalId: String,
        /// 提供商标识（aws/oss/ks3 等）。
        pub Provider: String,
        /// 凭证配置文件名。
        pub Profile: String,
        /// 是否启用对象锁定（Object Lock）。
        pub ObjectLockEnabled: bool,
    }
}

/// 轻量命令行标志集合，模拟 Go pflag 中与 S3 相关的字符串标志。
pub mod pflag {
    use super::HashMap;

    /// 字符串标志名到当前值的映射。
    #[derive(Default)]
    pub struct FlagSet {
        values: HashMap<String, String>,
    }

    impl FlagSet {
        /// 定义字符串标志；若已存在则保留原值（对齐 Go 首次定义为准）。
        pub fn String(&mut self, name: &str, value: &str, _usage: &str) {
            self.values
                .entry(name.to_owned())
                .or_insert_with(|| value.to_owned());
        }

        /// 读取已定义标志的字符串值。
        pub fn GetString(&self, name: &str) -> anyhow::Result<String> {
            self.values
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("flag not defined: {name}"))
        }

        /// 覆盖已定义标志的值。
        pub fn Set(&mut self, name: &str, value: &str) -> anyhow::Result<()> {
            let current = self
                .values
                .get_mut(name)
                .ok_or_else(|| anyhow::anyhow!("flag not defined: {name}"))?;
            *current = value.to_owned();
            Ok(())
        }
    }
}

#[path = "interface.rs"]
mod interface;
pub use interface::*;

#[path = "metrics.rs"]
mod metrics;
pub use metrics::*;

#[path = "retry.rs"]
mod retry;
pub use retry::*;

#[path = "store.rs"]
mod store;
pub use store::*;

#[path = "io.rs"]
mod io_impl;
pub use io_impl::*;

#[path = "permission.rs"]
mod permission;
pub use permission::*;

#[path = "mock/lib.rs"]
pub mod mock;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "io_test.rs"]
mod io_test;

#[cfg(test)]
#[path = "metrics_test.rs"]
mod metrics_test;

#[cfg(test)]
#[path = "store_test.rs"]
mod store_test;
