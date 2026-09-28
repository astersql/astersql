// Copyright 2026 AsterSQL.

// 中文总览：本文件承担 RealTiKV 加索引、分布式回填与全局排序 中的 模块入口与共享导出层。
// 中文总览：重点在于模块职责、导出关系和串行化约束。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `serial_guard` 负责 串行守卫。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

extern crate self as astersql_tests_realtikvtest_addindextest3;

use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard};

/// Rust equivalent of the package-level `-full-mode` flag.
pub static FULL_MODE: AtomicBool = AtomicBool::new(false);

static SERIAL: Mutex<()> = Mutex::new(());

/// Process-global RealTiKV/config/failpoint state is serialized like Go's
/// package TestMain lifecycle.
// 该辅助函数负责 串行守卫。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

pub fn serial_guard() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
