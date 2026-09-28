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

//! FIPS-only TLS stand-in for Go `//go:build boringcrypto` + blank import of
//! `crypto/tls/fipsonly`. Default builds leave FIPS-only mode disabled.
//! 中文补充：Rust 默认构建不会像 Go 的 `boringcrypto` 构建标签那样自动切到 FIPS-only 模式，
//! 因此这里显式返回关闭状态，供上层命令按同一语义判断能力边界。

/// Returns whether this build enforces FIPS-only TLS semantics.
///
/// Go only compiles `fips.go` under `boringcrypto`; without that tag the
/// blank import is absent, so FIPS-only mode is off.
/// 中文补充：该函数是能力查询接口，不负责真正切换 TLS 实现。
pub fn fips_only_enabled() -> bool {
    false
}
