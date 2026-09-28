// Copyright 2026 AsterSQL.

// 密码管理相关执行器测试 crate 入口。
//
// 对应 Go `pkg/executor/test/passwordtest` 包。覆盖双密码（dual password：
// 主密码与次密码可同时用于认证、便于轮换）、密码历史复用限制，以及
// 权限缓存加载顺序等语义。本 crate 仅在 `#[cfg(test)]` 下编译测试源，
// 不导出生产 API。

#![allow(dead_code)]

/// 双密码保留/丢弃旧凭证与认证匹配用例。
#[cfg(test)]
mod dual_password_test;
/// 包级 TestMain：权限缓存先于会话打开的生命周期顺序冒烟。
/// 密码历史窗口内拒绝复用近期凭证的用例。
#[cfg(test)]
mod password_management_test;
