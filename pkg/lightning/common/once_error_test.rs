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

// `OnceError` 单元测试：验证一次性写入语义与跨线程共享行为。

use crate::{CommonError, OnceError};

/// 校验 `OnceError`：初始为空、`None` 不写入、仅首次 `Some` 生效、并发 Clone 安全。
#[test]
fn test_once_error() {
    /// 断言实际错误与期望错误的字符串表示一致。
    fn assert_same_error(actual: Option<&CommonError>, expected: &CommonError) {
        assert_eq!(
            actual.map(|err| err.to_string()),
            Some(expected.to_string())
        );
    }

    let err = OnceError::default();

    // 初始与显式 Set(None) 后均应仍为空
    assert!(err.Get().is_none());
    err.Set(None);
    assert!(err.Get().is_none());

    // 首次写入生效
    let e = CommonError::new("once", "1");
    err.Set(Some(e.clone()));
    assert_same_error(err.Get().as_ref(), &e);

    let e2 = CommonError::new("once", "2");
    err.Set(Some(e2));
    // Go 注释强调这里仍是 e，而不是第二次传入的 e2。
    // 第二次 Set 被忽略，仍保留第一次的 e
    assert_same_error(err.Get().as_ref(), &e);

    // 已有错误后再 Set(None) 不会清空
    err.Set(None);
    assert_same_error(err.Get().as_ref(), &e);

    // 克隆到另一线程 Set(None) 不影响已保存错误
    let (tx, rx) = std::sync::mpsc::sync_channel::<()>(1);
    let err_clone = err.clone();
    std::thread::spawn(move || {
        err_clone.Set(None);
        tx.send(()).expect("notify OnceError goroutine done");
    });
    rx.recv().expect("wait OnceError goroutine done");

    assert_same_error(err.Get().as_ref(), &e);
}
