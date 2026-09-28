// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// GroupIDGenerator 单元测试。
//
// 验证等价组（Group）标识生成器的单调递增、手动设置起点，
// 以及 `u64` 回绕（wrapping）行为：`MAX` 之后下一号回到 `0`。

use crate::{GroupID, GroupIDGenerator};

/// 覆盖默认起点、`set` 跳号，以及 `wrapping_add` 溢出回绕。
#[test]
fn TestGroupIDGenerator_NextGroupID() {
    let mut generator = GroupIDGenerator::default();
    // 默认从 0 起，第一次 Next 得到 1
    assert_eq!(generator.NextGroupID(), 1 as GroupID);
    assert_eq!(generator.NextGroupID(), 2);
    assert_eq!(generator.NextGroupID(), 3);
    // 手动把内部计数设到 100，下一号应为 101
    generator.set(100);
    assert_eq!(generator.NextGroupID(), 101);
    assert_eq!(generator.NextGroupID(), 102);
    assert_eq!(generator.NextGroupID(), 103);
    // 设到 u64::MAX 后 wrapping_add 回到 0，再 Next 得到 1
    generator.set(u64::MAX);
    assert_eq!(generator.NextGroupID(), 0);
    assert_eq!(generator.NextGroupID(), 1);
}
