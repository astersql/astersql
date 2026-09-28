// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 时间解析 API 用的轻量字符串契约，对齐 Go `types.String`。
//
// 区分稳定来源（`PlainStr`）与可能别名可变缓冲的来源（`HackedStr`），
// 以便错误延迟格式化前由 pingcap/errors 冻结参数，避免后续原地修改污染告警文案。

// String is a lightweight string contract for time parsing APIs.
// It lets callers mark unsafe string sources (for example, zero-copy chunk buffers)
// so error arguments can be frozen by pingcap/errors before deferred formatting.
// String 对应 Go 的接口，只要求调用者能返回稳定的字符串视图。
/// 可导出稳定字符串视图的契约。
pub trait String {
    /// 返回用于错误/展示的拥有型字符串。
    fn String(&self) -> std::string::String;
}

// PlainStr is the default string wrapper for stable string sources.
// PlainStr 对应 Go 的 string 新类型，用于已经稳定的字符串来源。
/// 已稳定字符串来源的默认包装。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct PlainStr(pub std::string::String);

impl String for PlainStr {
    // String returns the string value.
    // Go 这里是零成本 string(s) 转换；用 clone 保留返回拥有值的形状。
    fn String(&self) -> std::string::String {
        self.0.clone()
    }
}

// HackedStr marks strings that may alias mutable buffers. It implements
// errors.HackedStr so pingcap/errors freezes the argument on error construction
// and avoids later mutations showing up in warning/error messages.
// HackedStr 对应 Go 的不安全来源标记，提醒错误构造时需要冻结底层字节。
/// 可能指向可变 chunk buffer 的不安全字符串标记。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct HackedStr(pub std::string::String);

impl String for HackedStr {
    // String returns the string value.
    // Go 返回 string(s)，这里保留同样的显示语义。
    fn String(&self) -> std::string::String {
        self.0.clone()
    }
}

impl HackedStr {
    // FreezeStr clones the string to detach from mutable backing storage.
    // FreezeStr 对应 Go 的 strings.Clone，用于脱离可能复用的 chunk buffer。
    /// 克隆字符串以脱离可能复用的底层缓冲。
    pub fn FreezeStr(&self) -> std::string::String {
        self.0.clone()
    }
}

impl astersql_errors::HackedStr for HackedStr {
    fn FreezeStr(&self) -> std::string::String {
        self.0.clone()
    }
}
