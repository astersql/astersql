// Copyright 2026 AsterSQL.

// `pkg/executor/test/recovertest` crate 根：挂接 RECOVER / FLASHBACK 相关测试。
//
// RECOVER TABLE 与 FLASHBACK 可把被 DROP/TRUNCATE 的表或库恢复到 GC safe point
//（垃圾回收安全点）之前的版本；本 crate 仅在 `#[cfg(test)]` 下编译测试源。

#![allow(dead_code)]

/// 包级 TestMain：全局配置与 failpoint 约定。
#[cfg(test)]
mod main_test;
/// RECOVER / FLASHBACK 功能与权限、GC 约束用例。
#[cfg(test)]
mod recover_test;
