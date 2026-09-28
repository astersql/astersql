// Copyright 2026 AsterSQL.

// Information Schema（信息模式）执行器测试 crate 入口。
//
// 对应 Go `pkg/executor/test/infoschema`：校验 infoschema 元数据查找
// （表/分区 ID、外键等）及包级 TestMain 对慢日志阈值等全局配置的改写语义。
// 本文件仅在 `#[cfg(test)]` 下挂接测试模块，不导出生产 API。

#![allow(dead_code)]

#[cfg(test)]
/// infoschema 元数据查找与分区/外键字段保留语义的测试。
mod infoschema_test;
#[cfg(test)]
/// 包级 TestMain：慢日志阈值等全局配置的 Go 等价语义冒烟。
mod main_test;
