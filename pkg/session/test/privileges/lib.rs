// Copyright 2026 AsterSQL.

// `privileges` 会话权限测试包入口。
//
// 挂接权限 harness 与 Session.Auth / SkipWithGrant 等权限行为测试模块。

#![allow(dead_code)]

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod privileges_test;
