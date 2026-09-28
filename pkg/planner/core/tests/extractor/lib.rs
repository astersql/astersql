// Copyright 2026 AsterSQL.

// planner/core/tests/extractor 测试 crate 入口。
//
// 挂载 infoschema/memtable 谓词抽取相关测试模块。谓词抽取（extractor）从
// WHERE 等条件中提炼 schema/table/index 过滤条件，缩小 infoschema 扫描范围。

#![allow(dead_code)]

/// extractor 测试生命周期约定（规划前安装 infoschema）。
#[cfg(test)]
mod main_test;
/// memtable infoschema 谓词交集抽取测试。
#[cfg(test)]
mod memtable_infoschema_extractor_test;
