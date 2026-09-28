// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// Unix 通用平台下 rlimit 相关类型别名与日志字段。
//
// 与 FreeBSD 等变体分离：此处将 `RlimT` 定为 `u64`，并提供
// `zapRlimT` 以便结构化日志输出打开文件数限制。

/// 进程资源限制数值类型（Unix 通用：无符号 64 位）。
pub type RlimT = u64;

/// 结构化日志中表示 rlimit 键值对的字段。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RLimitLogField {
    /// 日志字段名（如 `"rlimit"`）。
    pub key: String,
    /// 对应的 rlimit 数值。
    pub value: u64,
}

/// 构造用于 zap/结构化日志的 rlimit 字段。
pub fn zapRlimT(key: &str, value: RlimT) -> RLimitLogField {
    RLimitLogField {
        key: key.to_owned(),
        value,
    }
}
