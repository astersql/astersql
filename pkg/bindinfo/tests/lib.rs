// Copyright 2026 AsterSQL.

// bindinfo 集成测试入口模块。
//
// bindinfo（SQL 绑定信息）是数据库中用于把某条 SQL 语句与指定执行计划
// （执行计划：优化器为 SQL 生成的具体执行步骤，如索引选择、连接顺序）
// 绑定在一起的机制，可在不修改业务 SQL 的情况下固定或干预优化器的选择。
// 本文件作为集成测试 crate 的根，仅负责声明并组织各个测试子模块。

// 允许存在未被使用的代码，避免测试辅助函数在部分配置下触发编译告警。
#![allow(dead_code)]

// 以下各子模块均仅在测试配置（cfg(test)）下编译。

/// SQL 绑定基础功能测试：验证创建、匹配与使用绑定的核心流程。
#[cfg(test)]
mod bind_test;
/// 绑定使用信息测试：验证绑定的使用统计（如最近使用时间）被正确记录。
#[cfg(test)]
mod bind_usage_info_test;
/// 跨库绑定测试：验证使用通配库名的绑定能作用于不同数据库中的同构 SQL。
#[cfg(test)]
mod cross_db_binding_test;
/// 测试主入口模块：承载测试套件级别的初始化与公共设置逻辑。
#[cfg(test)]
mod main_test;
