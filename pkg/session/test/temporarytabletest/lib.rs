// Copyright 2026 AsterSQL.

// `temporarytabletest` 临时表测试包入口。
//
// 挂接本地/全局临时表（session 级或事务结束清空的临时关系）相关 harness 与功能测试模块。

#![allow(dead_code)]

/// 测试入口与全局环境准备（对应 Go `TestMain`）。
#[cfg(test)]
mod main_test;
/// 临时表 DDL 解析、点查更新删除谓词与事务语义用例。
#[cfg(test)]
mod temporary_table_test;
