// Copyright 2026 AsterSQL.

// Ingestor 包入口：对外暴露文档模块与迁移测试挂载点。
//
// 本包提供将已编码 KV 整理为 SST 并直接灌入 TiKV 的接口抽象；
// SST（Sorted String Table）是有序键值块文件，物理导入可绕过逐行 SQL 写入。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

pub mod doc;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
