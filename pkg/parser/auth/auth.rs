// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 用户/角色身份 AST 数据结构，对照 `pkg/parser/auth/auth.go`。
//
// 保存登录身份与权限系统匹配后的认证身份，并提供 SQL 还原与显示字符串规则。
// 本模块不执行真实认证握手，只承载解析与权限元数据形状。

// 本文件对照 pkg/parser/auth/auth.go，保留用户/角色身份的数据形状、SQL 还原和显示规则。

use crate::parser::format;

// 用户名和主机名长度上限与 Go 常量保持一致。
/// 用户名最大长度（字符数），与 Go 常量一致。
pub const UserNameMaxLength: usize = 32;
/// 主机名最大长度（字符数），与 Go 常量一致。
pub const HostNameMaxLength: usize = 255;

// UserIdentity 同时保存登录身份和权限系统实际匹配到的认证身份。
/// 用户身份：同时保留客户端登录身份与权限系统匹配到的 AuthIdentity。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UserIdentity {
    /// 登录用户名。
    #[serde(rename = "Username")]
    pub username: String,
    /// 登录主机名。
    #[serde(rename = "Hostname")]
    pub hostname: String,
    /// 是否表示 CURRENT_USER 语法。
    #[serde(rename = "CurrentUser")]
    pub current_user: bool,
    // AuthUsername/AuthHostname 可能来自通配符权限记录，不一定等于客户端提交值。
    /// 权限系统匹配到的用户名（可能来自通配符规则）。
    #[serde(rename = "AuthUsername")]
    pub auth_username: String,
    /// 权限系统匹配到的主机名。
    #[serde(rename = "AuthHostname")]
    pub auth_hostname: String,
    // AuthPlugin 只记录握手选定的插件；这里不会调用插件执行认证。
    /// 握手选定的认证插件名（仅记录，不在此执行认证）。
    #[serde(rename = "AuthPlugin")]
    pub auth_plugin: String,
}

impl UserIdentity {
    // restore 对应 Go Node.Restore：CURRENT_USER 不附带 @host，其余身份分别按名称转义。
    /// 将用户身份还原为 SQL 文本（CURRENT_USER 或 `user`@`host`）。
    pub fn restore(&self, ctx: &mut format::RestoreCtx<'_>) -> std::io::Result<()> {
        if self.current_user {
            ctx.WriteKeyWord("CURRENT_USER")?;
        } else {
            ctx.WriteName(&self.username)?;
            ctx.WritePlain("@")?;
            ctx.WriteName(&self.hostname)?;
        }
        Ok(())
    }

    // identity_string 对应 Go String；优先展示权限表匹配身份，未匹配时回退到登录身份。
    /// 显示身份字符串：优先 AuthIdentity，否则回退登录身份。
    pub fn identity_string(&self) -> String {
        // 源 Go 注明未来仍需完善用户名和主机名的显示转义；这里保持原始字符串拼接语义。
        if !self.auth_username.is_empty() {
            return format!("{}@{}", self.auth_username, self.auth_hostname);
        }
        format!("{}@{}", self.username, self.hostname)
    }

    // login_string 始终返回客户端登录身份，不使用权限系统匹配后的 AuthIdentity。
    /// 始终返回客户端登录身份 `user@host`。
    pub fn login_string(&self) -> String {
        format!("{}@{}", self.username, self.hostname)
    }
}

impl std::fmt::Display for UserIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.identity_string())
    }
}

// Go 允许在 nil *UserIdentity 上调用 String/LoginString 并得到空串；Rust 用 Option 辅助函数显式保留该语义。
/// 对应 Go 在 nil `*UserIdentity` 上调用 String 得到空串的语义。
pub fn optional_identity_string(user: Option<&UserIdentity>) -> String {
    user.map(UserIdentity::identity_string).unwrap_or_default()
}
/// 对应 Go 在 nil `*UserIdentity` 上调用 LoginString 得到空串的语义。
pub fn optional_login_string(user: Option<&UserIdentity>) -> String {
    user.map(UserIdentity::login_string).unwrap_or_default()
}

// RoleIdentity 保存角色名及可选主机名；还原 SQL 时只有非空主机才输出 @。
/// 角色身份：角色名与可选主机名。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RoleIdentity {
    /// 角色名。
    #[serde(rename = "Username")]
    pub username: String,
    /// 可选主机名；为空时 SQL 还原不输出 `@`。
    #[serde(rename = "Hostname")]
    pub hostname: String,
}
impl RoleIdentity {
    /// 将角色身份还原为 SQL 文本；主机为空时仅输出角色名。
    pub fn restore(&self, ctx: &mut format::RestoreCtx<'_>) -> std::io::Result<()> {
        ctx.WriteName(&self.username)?;
        if !self.hostname.is_empty() {
            ctx.WritePlain("@")?;
            ctx.WriteName(&self.hostname)?;
        }
        Ok(())
    }

    // role_string 保留 Go String 的反引号包裹格式，包括空主机名。
    /// 显示字符串，保留 Go 的反引号包裹格式（含空主机）。
    pub fn role_string(&self) -> String {
        format!("`{}`@`{}`", self.username, self.hostname)
    }
}

impl std::fmt::Display for RoleIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.role_string())
    }
}
