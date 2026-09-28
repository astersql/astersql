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

// 一次性错误容器：在并发场景下只保留首次写入的错误。
//
// Lightning 导入任务中多个 worker 可能同时失败；本模块用互斥锁保护的
// `Option` 保证只记录第一次非空错误，后续写入被忽略，便于统一向上汇报。

use crate::CommonError;
use std::sync::{Arc, Mutex};

/// OnceError is an error value which can be assigned once.
///
/// The zero value is ready for use.
///
/// 一次性错误：内部用 `Arc<Mutex<Option<CommonError>>>` 共享，可 Clone 后跨线程使用。
/// 零值（Default）即可直接使用，初始无错误。
#[derive(Clone, Default)]
pub struct OnceError(Arc<Mutex<Option<CommonError>>>);

impl OnceError {
    /// Set assigns an error to this instance, if `error` is `Some`.
    ///
    /// If this method is called multiple times, only the first call is effective.
    ///
    /// 若 `error` 为 `Some` 则尝试写入；已有错误时忽略后续写入（仅首次生效）。
    /// 传入 `None` 时直接返回，不改变已有状态。
    pub fn Set(&self, error: Option<CommonError>) {
        // 空错误不覆盖；已有值时保持第一次写入的结果
        let Some(error) = error else {
            return;
        };
        let mut stored = self.0.lock().expect("once error lock poisoned");
        if stored.is_none() {
            *stored = Some(error);
        }
    }

    /// Get returns the first error value stored in this instance.
    ///
    /// 返回已保存的首次错误副本；若尚未设置则返回 `None`。
    pub fn Get(&self) -> Option<CommonError> {
        self.0.lock().expect("once error lock poisoned").clone()
    }
}
