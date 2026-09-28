// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// Memo 中等价类 Group 的单调 ID 生成器。
//
// Cascades Memo 将语义等价的逻辑表达式归入同一 Group；每个 Group 需要唯一
// `GroupID`，由本生成器在单线程路径上递增分配。

/// Unique identifier of a memo equivalence group.
/// Memo 等价类（equivalence group）的唯一标识符。
pub type GroupID = u64;

/// Monotonic, single-threaded group identifier generator.
/// 单调递增、单线程使用的 Group 标识符生成器。
#[derive(Debug, Default)]
pub struct GroupIDGenerator {
    /// 当前已分配的最大 ID；下一次 `NextGroupID` 会再加一。
    id: GroupID,
}

impl GroupIDGenerator {
    /// 分配并返回下一个 GroupID（wrapping 加法，对齐 Go 行为）。
    pub fn NextGroupID(&mut self) -> GroupID {
        self.id = self.id.wrapping_add(1);
        self.id
    }

    /// 将计数器重置为 0，供 Memo 重建等内部场景使用。
    pub(crate) fn reset(&mut self) {
        self.id = 0;
    }

    /// 测试专用：直接设定当前 ID，便于构造碰撞或确定性场景。
    #[cfg(test)]
    pub(crate) fn set(&mut self, id: GroupID) {
        self.id = id;
    }
}
