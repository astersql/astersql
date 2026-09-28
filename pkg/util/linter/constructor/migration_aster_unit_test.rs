// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `Constructor` 标记类型的迁移补充单元测试。
//
// 验证零大小空结构体与 Go 空 struct 语义一致：可 `Copy`、字面量等于
// `Default`，且 `size_of` 为 0。

use super::Constructor;

/// 编译期约束：要求 `T: Copy`，用于确认 `Constructor` 可按值复制。
fn assert_copy<T: Copy>(value: T) -> T {
    value
}

/// 断言 `Constructor` 可 Copy，且占用 0 字节（对齐 Go 空 struct）。
#[test]
fn constructor_marker_matches_go_empty_struct_semantics() {
    let marker = Constructor::default();
    let copied = assert_copy(marker);

    assert!(marker == copied);
    assert_eq!(std::mem::size_of::<Constructor>(), 0);
}

/// 断言结构体字面量 `Constructor {}` 与 `Default` 零值相等。
#[test]
fn constructor_literal_matches_the_zero_value() {
    assert!(Constructor {} == Constructor::default());
}
