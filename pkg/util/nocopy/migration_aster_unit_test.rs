// Copyright 2026 AsterSQL.
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

// nocopy 迁移回归测试：确认标记为零大小且不可 Clone/Copy，lock/unlock 为空操作。

use super::nocopy::NoCopy;

// 编译期断言：NoCopy 既不实现 Clone 也不实现 Copy。
static_assertions::assert_not_impl_any!(NoCopy: Clone, Copy);

/// 默认构造的 NoCopy 应为零大小（ZST）标记。
#[test]
fn no_copy_is_a_zero_sized_default_marker() {
    let marker = NoCopy::default();

    assert_eq!(std::mem::size_of_val(&marker), 0);
}

/// lock/unlock 可重复调用且无副作用，对齐 Go sync.Locker 空实现。
#[test]
fn lock_and_unlock_are_repeatable_no_ops() {
    let marker = NoCopy::default();

    marker.lock();
    marker.lock();
    marker.unlock();
    marker.unlock();
}
