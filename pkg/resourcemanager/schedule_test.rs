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

// 资源管理器 `Exec` 调容上限相关单元测试。
//
// 验证 Overclock 在达到 `原始并发 + MaxOverclockCount` 后不再继续升容。

#[allow(non_snake_case)]

use std::sync::Arc;

use resourcemanager_test_support::{
    NewResourceManger,
    scheduler::Command,
    util::{DDL, NewMockGPool, PoolContainer},
};

/// 连续两次 Overclock：第一次升到 2，第二次因超上限保持 2。
#[test]
pub fn TestSchedulerOverloadTooMuch() {
    let rm = NewResourceManger();
    let mp = Arc::new(NewMockGPool("test".to_owned(), 1));
    let pool = PoolContainer {
        Pool: mp,
        Component: DDL,
    };

    rm.Exec(&pool, Command::Overclock);
    assert_eq!(2, pool.Pool.Cap());
    rm.Exec(&pool, Command::Overclock);
    assert_eq!(2, pool.Pool.Cap());
}
