// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 字段标志位 helper 的单元测试。
//
// 对应 Go 的 `type_test.go`，确认各 `Has*Flag` 能识别自身常量位。

// 本文件对照 pkg/parser/mysql/type_test.go 迁移，保留 Go 测试结构与行为。
// 验证 MySQL 字段 flag helper 对相应 bit 位返回 true。
// test_flags 对应 Go 的 TestFlags，逐个确认 flag helper 能识别自己的标志位。
use crate::r#type::*;

#[test]
fn test_flags() {
    // 每个断言用「标志常量自身」作为输入，验证对应 Has* 返回 true。
    assert!(HasNotNullFlag(NotNullFlag));
    assert!(HasUniKeyFlag(UniqueKeyFlag));
    // Go 源测试重复检查 NotNullFlag；保留这个重复断言，方便对照源文件。
    assert!(HasNotNullFlag(NotNullFlag));
    assert!(HasNoDefaultValueFlag(NoDefaultValueFlag));
    assert!(HasAutoIncrementFlag(AutoIncrementFlag));
    assert!(HasUnsignedFlag(UnsignedFlag));
    assert!(HasZerofillFlag(ZerofillFlag));
    assert!(HasBinaryFlag(BinaryFlag));
    assert!(HasPriKeyFlag(PriKeyFlag));
    assert!(HasMultipleKeyFlag(MultipleKeyFlag));
    assert!(HasTimestampFlag(TimestampFlag));
    assert!(HasOnUpdateNowFlag(OnUpdateNowFlag));
}
