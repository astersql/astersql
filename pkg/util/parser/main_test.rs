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

// parser 包测试入口 harness。
//
// 确保测试入口只初始化一次。

use std::sync::OnceLock;
use std::thread::{self, ThreadId};

/// 公共测试 setup 所在线程 ID，确保只初始化一次。
static COMMON_TEST_SETUP_THREAD: OnceLock<ThreadId> = OnceLock::new();

/// 记录并返回首次调用时的当前线程 ID（模拟 Go 侧公共 setup）。
fn setup_for_common_test() -> ThreadId {
    *COMMON_TEST_SETUP_THREAD.get_or_init(|| thread::current().id())
}
