// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! Provider trait matching `br/pkg/kms/kms.go`.
//
// 定义 KMS 提供方统一接口：解密 data key、报告厂商名、释放资源；对齐 Go `Provider`。

use tokio_util::sync::CancellationToken;

/// Cancellation context propagated into an in-flight KMS request.
#[derive(Clone, Debug, Default)]
pub struct Context {
    cancellation: CancellationToken,
}

impl Context {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub(crate) fn token(&self) -> &CancellationToken {
        &self.cancellation
    }
}

/// Provider is an interface for key management service providers.
// AWS/GCP 等后端均通过此 trait 被 master_key 层统一调用。
pub trait Provider {
    fn DecryptDataKey(&self, ctx: &Context, dataKey: &[u8]) -> Result<Vec<u8>, String>;
    fn Name(&self) -> &str;
    fn Close(&mut self);
}
