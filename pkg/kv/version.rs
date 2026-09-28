// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// KV 版本号类型与 VersionProvider 接口。
//
// Version 包装单调递增的无符号版本，用于 MVCC（多版本并发控制）可见性比较；
// MaxVersion / MinVersion 为边界哨兵而非真实有效版本。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

// VersionProvider 对应 Go 的同名接口，为调用方提供单调递增的当前版本。
// Go 的 error 返回值映射为 Result；具体错误类型留待跨文件模块接线时确定。
/// 提供当前全局版本的供给方接口。
pub trait VersionProvider {
    /// 返回单调递增的当前 Version。
    fn CurrentVersion(&self) -> Result<Version, Box<dyn std::error::Error>>;
}

// Version 是 KV 版本号的轻量包装；字段顺序与 Go 结构体一致。
/// KV 版本号的轻量包装，字段顺序对齐 Go 结构体。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Version {
    /// 原始无符号版本值。
    pub Ver: u64,
}

// MaxVersion 和 MinVersion 对应 Go 包变量，都是边界哨兵而不是有效版本。
/// 版本上界哨兵（u64::MAX），不表示真实已分配版本。
pub const MaxVersion: Version = Version { Ver: u64::MAX };
/// 版本下界哨兵（0），不表示真实已分配版本。
pub const MinVersion: Version = Version { Ver: 0 };

// NewVersion 对应 Go 构造函数，原样保存调用方给出的无符号版本号。
/// 由原始 u64 构造 Version。
pub fn NewVersion(v: u64) -> Version {
    Version { Ver: v }
}

impl Version {
    // Cmp 对应 Go 的三路比较：大于返回 1，小于返回 -1，相等返回 0。
    // 返回 i32 近似 Go int；这里不借用 Rust Ord，以保留源文件显式分支结构。
    /// 三路比较：大于返回 1，小于返回 -1，相等返回 0。
    pub fn Cmp(&self, another: Version) -> i32 {
        if self.Ver > another.Ver {
            return 1;
        } else if self.Ver < another.Ver {
            return -1;
        }
        0
    }
}
