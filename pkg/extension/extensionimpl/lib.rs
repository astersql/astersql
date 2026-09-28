// Copyright 2026 AsterSQL.

// 扩展实现子包（extensionimpl）的 crate 根。
//
// 当前导出 bootstrap 子模块，承载扩展引导期 SQL 执行与会话池协作逻辑。

#![allow(dead_code, non_snake_case, non_camel_case_types)]

/// 扩展 bootstrap（引导）实现。
pub mod bootstrap;
