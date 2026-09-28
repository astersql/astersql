// Copyright 2026 AsterSQL.

//! Binary entry for `astersql-tools-patch-go` (Go `check.go` / package main).
//! 该文件只保留最薄的一层二进制入口：对齐 Go 版 `main()` 仅触发一次运行时探针，
//! 并把实际逻辑继续下沉到库侧 `entry()`，避免命令行入口与测试/库调用路径分叉。

/// 与 Go `package main` 的入口职责保持一致，只做单层转发。
fn main() {
    astersql_tools_patch_go::entry();
}
