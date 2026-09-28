// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc. Licensed under Apache-2.0.
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

//! gluetidb 单测的 Domain 全局状态护栏：串行化并在离开时复位钩子/配置位。
//! 避免并行测试互相污染 `set_domain_hooks_for_test` 与全局 config bits。

use std::sync::{Mutex, MutexGuard};

// 进程级互斥：同一时刻只允许一个测试持有 Domain 相关可变全局。
static DOMAIN_STATE_LOCK: Mutex<()> = Mutex::new(());

/// RAII 护栏：持锁期间独占 Domain 测试状态，Drop 时清钩子并复位配置。
pub(crate) struct DomainStateGuard {
    _lock: MutexGuard<'static, ()>,
}

impl Drop for DomainStateGuard {
    fn drop(&mut self) {
        // 无论测试成败都复位，防止泄漏影响后续用例。
        crate::set_domain_hooks_for_test(None);
        crate::reset_global_config_bits_for_test();
    }
}

/// 获取护栏：先清零状态再返回，保证用例从已知基线开始。
pub(crate) fn domain_state_guard() -> DomainStateGuard {
    let lock = DOMAIN_STATE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::set_domain_hooks_for_test(None);
    crate::reset_global_config_bits_for_test();
    DomainStateGuard { _lock: lock }
}
