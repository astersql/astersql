// Copyright 2026 AsterSQL.

// 权限（privilege）子系统 crate 入口。
//
// 本包对应 Go 的 `pkg/privilege/privileges`，负责：
// - 权限错误类型（`errors`）；
// - 从 `mysql.*` 系统表加载并缓存的权限矩阵（`cache`）；
// - 连接鉴权、静态/动态权限校验与 SHOW GRANTS（`privileges`）；
// - TiDB Auth Token（JWT + JWKS）相关逻辑（`tidb_auth_token`）。
//
// 动态权限（Dynamic Privilege）是字符串形式的扩展权限名
// （如 `BACKUP_ADMIN`），与位图静态权限（`SelectPriv` 等）并存。

#![allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    dead_code
)]

mod user_attributes_filter;
pub use user_attributes_filter::*;

mod errors;
pub use errors::*;
mod cache;
#[cfg(test)]
mod errors_test;
pub use cache::*;
mod privileges;
pub use privileges::*;
mod tidb_auth_token;
pub use tidb_auth_token::*;

#[cfg(test)]
mod cache_test;
#[cfg(test)]
mod privileges_test;
#[cfg(test)]
mod tidb_auth_token_test;

#[cfg(test)]
mod user_attributes_filter_test;
