// Copyright 2026 AsterSQL.

// `session/test/common` 测试包入口。
//
// 挂接 session 通用行为（杂项、prepare、affected rows 等）、harness 配置覆盖，
// 以及 prepared statement 去重缓存相关测试模块。

#![allow(dead_code)]

#[cfg(test)]
mod common_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod prepare_dedup_cache_test;
