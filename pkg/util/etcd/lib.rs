// Copyright 2026 AsterSQL.

// etcd 客户端辅助 crate 入口。
//
// 暴露 `etcd` 模块中的命名空间包装与带重试的 key 删除，并挂接对应单元测试。

#![allow(non_snake_case, non_upper_case_globals)]

// 自引用别名，供测试按 Go 包名风格引用本 crate。
extern crate self as util_etcd;

// 核心实现：NamespacedClient、DeleteClient 与 delete_key_from_etcd。
mod etcd;
pub use etcd::*;

#[cfg(test)]
#[path = "etcd_test.rs"]
mod etcd_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
