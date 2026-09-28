// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// worker 包单元测试：对齐 Go `TestApplyRecycle` 的借还与 nil 保护。

use crate::{NewPool, metric};
use std::sync::Arc;

// test_apply_recycle 对应 Go 的 TestApplyRecycle。
// Go 使用容量为 3 的 pool，先取尽 worker，再按回收顺序检查 Apply 返回同一对象。
/// 取尽三个 worker 后按 Recycle 顺序复用同一 Arc，并对 None 触发 panic。
#[test]
fn test_apply_recycle() {
    let pool = NewPool(&metric::MetricContext::background(), 3, "test".to_owned());

    let w1 = pool.Apply();
    let w2 = pool.Apply();
    let w3 = pool.Apply();
    assert_eq!(1_i64, w1.ID);
    assert_eq!(2_i64, w2.ID);
    assert_eq!(3_i64, w3.ID);
    assert_eq!(false, pool.HasWorker());

    // 回收后的 worker 应立即可用，并且 Apply 返回的对象与刚回收的对象相同。
    pool.Recycle(Some(Arc::clone(&w3)));
    assert_eq!(true, pool.HasWorker());
    assert!(Arc::ptr_eq(&w3, &pool.Apply()));
    pool.Recycle(Some(Arc::clone(&w2)));
    assert!(Arc::ptr_eq(&w2, &pool.Apply()));
    pool.Recycle(Some(Arc::clone(&w1)));
    assert!(Arc::ptr_eq(&w1, &pool.Apply()));

    assert_eq!(false, pool.HasWorker());

    // Go 的 require.PanicsWithValue 检查 nil worker 会 panic "invalid restore worker"。
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.Recycle(None)))
        .expect_err("Recycle(None) must panic");
    let message = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str));
    assert_eq!(message, Some("invalid restore worker"));
}
