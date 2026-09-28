// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// structure 包测试入口冒烟用例。
//
// Cargo 负责 Rust 测试进程生命周期；本文件验证各测试用例拿到独立的内存后端，
// 避免用例间共享状态污染。

use super::migration_aster_unit_test::writable;

// Cargo owns the Rust test-process lifecycle. This checks the corresponding
// package invariant: separate test cases receive independent backing stores.
/// 验证两次 `writable` 得到互不影响的存储：写入第一个不影响第二个。
#[test]
fn TestMainUsesIsolatedStores() {
    let (mut first, _) = writable(&[1]);
    let (second, _) = writable(&[1]);
    first.Set(b"key", b"value").unwrap();
    assert_eq!(Some(b"value".to_vec()), first.Get(b"key").unwrap());
    assert_eq!(None, second.Get(b"key").unwrap());
}
