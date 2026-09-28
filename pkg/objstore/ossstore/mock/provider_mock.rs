// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// OSS 凭证提供者的 Mock：供单元测试注入假凭证与期望调用。
//
// 对应 Go `gomock` 生成的 CredentialsProvider mock。`Credentials` 携带 AccessKey
// 与可选 STS（Security Token Service）临时令牌；`CredentialsProvider` 抽象从
// 环境/角色链拉取凭证的过程。

use std::fmt;

/// 一组可访问 OSS 的凭证字段（AccessKey 与可选 STS Token）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Credentials {
    /// Access Key ID。
    pub access_key_id: String,
    /// Access Key Secret。
    pub access_key_secret: String,
    /// STS 临时安全令牌；静态长期密钥场景可为空。
    pub security_token: String,
}

/// 拉取凭证失败时的错误包装。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialsError(String);

impl CredentialsError {
    /// 以消息构造凭证错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CredentialsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CredentialsError {}

/// 凭证提供者接口：可并发安全地取凭证并报告提供者名称。
pub trait CredentialsProvider: Send + Sync {
    /// 返回当前可用的凭证。
    fn GetCredentials(&self) -> Result<Credentials, CredentialsError>;
    /// 返回提供者名称（如 `ecs_ram_role`），用于日志与元数据分支。
    fn GetProviderName(&self) -> String;
}

// 用 mockall 生成可设置期望的 MockCredentialsProvider，对齐 Go gomock 用法。
mockall::mock! {
    pub CredentialsProvider {}

    impl CredentialsProvider for CredentialsProvider {
        fn GetCredentials(&self) -> Result<Credentials, CredentialsError>;
        fn GetProviderName(&self) -> String;
    }
}

/// GoMock 的独立 recorder 在 mockall 中由 mock 自身承担；保留公开类型名。
pub type MockCredentialsProviderMockRecorder = MockCredentialsProvider;

/// 构造默认空期望的 Mock 凭证提供者。
pub fn NewMockCredentialsProvider() -> MockCredentialsProvider {
    MockCredentialsProvider::new()
}

impl MockCredentialsProvider {
    /// 兼容 Go mock 的 EXPECT 链式写法入口（当前返回自身以便继续配置）。
    pub fn EXPECT(&mut self) -> &mut Self {
        self
    }

    /// 标记本类型来自 Go mock 迁移，便于测试侧识别。
    pub fn ISGOMOCK(&self) {}
}
