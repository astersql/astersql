// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// Cascades 优化器上下文契约（context）与规则开关掩码。
//
// Cascades 是一种基于 Memo（记忆化搜索空间）的代价优化框架：等价逻辑表达式
// 归入同一 Group，再通过任务调度逐步做规则变换与物理实现。本模块拆出独立的
// `Context` trait，打断与具体优化器实现之间的导入环（import cycle），并提供
// `RuleMask` 控制哪些变换规则（transformation rule）可被应用。

use std::collections::BTreeSet;

/// Rule enablement mask used by the cascades context boundary.
/// 规则启用掩码：用有序集合记录已启用的规则下标，供 Cascades 上下文边界查询。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuleMask(BTreeSet<usize>);

impl RuleMask {
    /// 启用指定下标的规则。
    pub fn Set(&mut self, index: usize) {
        self.0.insert(index);
    }

    /// 测试指定下标的规则是否已启用。
    pub fn Test(&self, index: usize) -> bool {
        self.0.contains(&index)
    }

    /// 清除（禁用）指定下标的规则。
    pub fn Clear(&mut self, index: usize) {
        self.0.remove(&index);
    }
}

/// Import-cycle-breaking optimizer context contract.
/// 打断导入环的优化器上下文契约：暴露调度器、Memo 与规则掩码的只读/可变访问。
pub trait Context {
    /// 销毁上下文持有的调度器与 Memo 等资源。
    fn Destroy(&mut self);
    /// 取得任务调度器（Scheduler）的只读引用。
    fn GetScheduler(&self) -> &dyn cascades_base::Scheduler;
    /// 取得任务调度器的可变引用，用于入队等写操作。
    fn GetSchedulerMut(&mut self) -> &mut dyn cascades_base::Scheduler;
    /// 将优化任务压入调度器；默认实现转发到 `GetSchedulerMut().PushTask`。
    fn PushTask(&mut self, task: Box<dyn cascades_base::Task>) {
        self.GetSchedulerMut().PushTask(task);
    }
    /// 取得 Memo（等价计划搜索空间）的只读引用。
    fn GetMemo(&self) -> &memo::Memo;
    /// 取得 Memo 的可变引用。
    fn GetMemoMut(&mut self) -> &mut memo::Memo;
    /// 取得规则启用掩码的只读引用。
    fn GetRuleMask(&self) -> &RuleMask;
    /// 取得规则启用掩码的可变引用。
    fn GetRuleMaskMut(&mut self) -> &mut RuleMask;
}
