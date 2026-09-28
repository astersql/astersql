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

// FreeBSD 平台的资源限制（rlimit）日志辅助。
//
// 将 `RlimT` 资源限制值包装为结构化日志字段，供 local backend 在校验
// 打开文件数等系统限制时输出诊断信息。

/// FreeBSD 上 rlimit 数值类型别名（对应 C 的 rlim_t）。
pub type RlimT = i64;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 资源限制日志字段：键名与限制值的结构化表示。
pub struct RLimitLogField {
    /// 日志字段名（如 "max-open-files"）。
    pub key: String,
    /// 对应的 rlimit 数值。
    pub value: i64,
}

/// 构造资源限制日志字段，便于 zap/结构化日志输出。
pub fn zapRlimT(key: &str, value: RlimT) -> RLimitLogField {
    RLimitLogField {
        key: key.to_owned(),
        value,
    }
}
