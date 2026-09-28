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

//! Go `//go:build boringcrypto` + blank import `_ "crypto/tls/fipsonly"`.
//! Rust has no one-to-one blank import; this hook preserves the side-effect
//! entry point so FIPS-only TLS builds can wire real constraints later.
//! 中文补充：这里保留与 Go FIPS 构建相同的初始化入口，但当前默认实现不直接改写 TLS 全局状态。

/// Called from process init. Go enables FIPS via the boringcrypto-tagged empty
/// import; default builds mirror non-boringcrypto Go (no TLS restriction).
/// 中文补充：调用点只负责显式表达“进入 FIPS-only 初始化阶段”，真正的 TLS 限制仍由后续专门实现承接。
pub fn enable_fips_only() {}
