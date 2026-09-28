// Copyright 2026 AsterSQL.

// 计划器 `stats` 子模块入口。
//
// 再导出统计表加载、伪统计（Pseudo Stats）与 UsedStats 记录相关实现。

#![allow(dead_code)]

pub mod stats;

#[cfg(test)]
mod stats_aster_unit_test;
