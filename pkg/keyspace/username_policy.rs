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

// Keyspace 用户名策略：按部署模式决定是否强制 keyspace 前缀。
//
// Starter 部署下，登录用户名须以 `{keyspace}.` 开头，便于多租户隔离；
// 其他模式使用放行一切的默认策略。本模块定义策略 trait 以及
// `GetUsernamePolicy` 工厂函数。

use crate::deploymode;
use exeerrors_dependency::{errors::SharedError, exeerrors::ErrUserNameNeedPrefix};

use super::GetKeyspaceNameBySettings;

/// 用户名校验、格式检查、变体生成与去前缀接口。
pub trait UsernamePolicy {
    /// 校验用户名是否符合当前策略；失败时返回标准 DDL 用户名错误。
    fn ValidateUsername(&self, username: &str) -> Result<(), SharedError>;
    /// 校验用户名格式（如点号个数），不检查前缀内容。
    fn ValidateUsernameFormat(&self, username: &str) -> bool;
    /// 为缺少前缀的用户名生成候选变体列表。
    fn GetUsernameVariants(&self, username: &str) -> Vec<String>;
    /// 去掉 keyspace 前缀后的原始用户名；不符合则返回空串。
    fn GetOriginalUsername(&self, username: &str) -> String;
}

/// 按部署模式选择策略：Starter → 前缀策略，否则 → 默认放行策略。
pub fn GetUsernamePolicy() -> Box<dyn UsernamePolicy> {
    if deploymode::IsStarter() {
        Box::new(PrefixPolicy {
            user_prefix: GetKeyspaceNameBySettings(),
        })
    } else {
        Box::new(DefaultUsernamePolicy)
    }
}

/// 默认策略：不校验、不生成变体、不去前缀。
struct DefaultUsernamePolicy;

impl UsernamePolicy for DefaultUsernamePolicy {
    fn ValidateUsername(&self, _username: &str) -> Result<(), SharedError> {
        Ok(())
    }

    fn ValidateUsernameFormat(&self, _username: &str) -> bool {
        true
    }

    fn GetUsernameVariants(&self, _username: &str) -> Vec<String> {
        Vec::new()
    }

    fn GetOriginalUsername(&self, _username: &str) -> String {
        String::new()
    }
}

/// Starter 前缀策略：强制用户名以 `{keyspace}.` 开头。
struct PrefixPolicy {
    /// 当前 keyspace 名称，用作用户名前缀。
    user_prefix: String,
}

impl PrefixPolicy {
    /// 拼出带尾部点号的期望前缀，如 `"ks."`。
    fn expected_prefix(&self) -> String {
        format!("{}.", self.user_prefix)
    }
}

impl UsernamePolicy for PrefixPolicy {
    fn ValidateUsername(&self, username: &str) -> Result<(), SharedError> {
        // 已配置前缀且用户名未以 `{prefix}.` 开头时拒绝。
        if !self.user_prefix.is_empty() && !username.starts_with(&self.expected_prefix()) {
            return Err(ErrUserNameNeedPrefix.GenWithStackByArgs(&[
                self.user_prefix.clone().into(),
                self.user_prefix.clone().into(),
                username.into(),
            ]));
        }
        Ok(())
    }

    fn ValidateUsernameFormat(&self, username: &str) -> bool {
        // 恰好一个点号：`prefix.user` 形式。
        username.matches('.').count() == 1
    }

    fn GetUsernameVariants(&self, username: &str) -> Vec<String> {
        // 已有前缀或前缀为空时不生成变体；否则补上 `{prefix}.`。
        if self.user_prefix.is_empty() || username.starts_with(&self.expected_prefix()) {
            Vec::new()
        } else {
            vec![format!("{}.{}", self.user_prefix, username)]
        }
    }

    fn GetOriginalUsername(&self, username: &str) -> String {
        if self.user_prefix.is_empty() {
            return String::new();
        }
        // 成功去掉前缀则返回剩余部分，否则空串。
        username
            .strip_prefix(&self.expected_prefix())
            .unwrap_or_default()
            .to_owned()
    }
}
