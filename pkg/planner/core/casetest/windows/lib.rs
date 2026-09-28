// Copyright 2026 AsterSQL.

// 窗口函数（window function）规划器用例包入口。
//
// 窗口函数在结果集上按分区（PARTITION BY）与排序（ORDER BY）定义滑动窗口，
// 计算排名、累计等。本包聚合窗口下推（push-down）与 EXISTS 子查询解相关
// （decorrelation）相关测试模块。

#![allow(dead_code)]

/// 对齐 Go TestMain：公共初始化与 golden suite 对照说明。
#[cfg(test)]
mod main_test;
/// 窗口计划下推 / TopN 从 Window 派生规则用例。
#[cfg(test)]
mod window_push_down_test;
/// 窗口与 EXISTS/相关子查询组合时的解相关用例。
#[cfg(test)]
mod window_with_exist_subquery_test;
