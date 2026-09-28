// Copyright 2026 AsterSQL.

// standby（热备/待命）相关 server 测试 crate 根模块。
//
// 在测试构建下挂载 `main_test` 与 `standby_test`，覆盖待命控制器激活
// 与 Server 监听器生命周期行为。

#![allow(dead_code)]

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod standby_test;
