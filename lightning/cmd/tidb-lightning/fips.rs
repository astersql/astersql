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

//! Go `fips.go` is built only with `//go:build boringcrypto` and blank-imports
//! `crypto/tls/fipsonly`. Rust has no boringcrypto tag; this module is the
//! migration anchor. Enable a future cargo feature to wire a FIPS TLS stack.
//! 这里保留与 Go 条件编译分支对应的接线点，当前 Rust 端未启用
//! FIPS 专用 TLS 提供方，所以只能显式保留一个空入口来表达语义对齐。

/// Go blank-import init side effect. No-op unless a FIPS feature is introduced.
/// 调用方可以无条件保留这一步；只有未来真的接入 FIPS TLS 实现时，
/// 才需要把初始化副作用落在这里，而不是改动现有调用路径。
pub fn init_fips_only_tls_for_boringcrypto_build() {
    // Intentionally empty: matches non-boringcrypto Go builds where fips.go
    // is not compiled. A FIPS-enabled TLS provider would initialize here.
}
