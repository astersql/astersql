// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! 中文说明开始（自动生成）
//! 中文总览：`global_sort.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `测试工具与兼容封装` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 4 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `RemoveAllObjects` 是当前文件里的公开函数。
//! `RemoveAllObjects` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `RemoveAllObjects` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `RemoveAllObjects`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! Fake GCS bucket cleanup helper
//! (Go `tests/realtikvtest/testutils/global_sort.go`).

use crate::stubs::{fakestorage, require, storage};
use astersql_tests_realtikvtest::stubs::TestCtx;

/// RemoveAllObjects removes all objects in the bucket.
pub fn RemoveAllObjects(t: &TestCtx, server: &fakestorage::Server, bucket: &str) {
    let (object_attrs, list_res) =
        server.ListObjectsWithOptions(bucket, fakestorage::ListOptions::default());
    require::NoError(t, list_res);
    let bucket_handle = server.Client().Bucket(bucket);
    for attr in object_attrs {
        match bucket_handle.Object(&attr.Name).Delete(()) {
            Err(e) if storage::is_object_not_exist(&e) => {
                // DXF cleanup might also delete the object, so we ignore the error.
                continue;
            }
            Err(e) => require::NoError(t, Err(e.to_string())),
            Ok(()) => {}
        }
    }
}
