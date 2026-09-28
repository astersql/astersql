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

//! 对齐 Go `//go:build boringcrypto` 版本里匿名导入 `crypto/tls/fipsonly` 的文件。
//!
//! Go 文件只在 `boringcrypto` 构建中存在。Rust 的常规构建因此不启用 FIPS；
//! FIPS 构建由 `ASTERSQL_FIPS_ONLY` 编译环境标记请求，并且必须链接经验证的
//! rustls AWS-LC FIPS provider，否则启动立即失败，禁止静默降级。

/// 对齐 Go 中 `_ "crypto/tls/fipsonly"` 的初始化副作用入口。
pub fn enable_fips_only() {
    enable_fips_only_for_build(option_env!("ASTERSQL_FIPS_ONLY").is_some())
        .unwrap_or_else(|error| panic!("FIPS-only initialization failed: {error}"));
}

/// 可测试的构建策略：普通构建与 Go 未选中 `fips.go` 一致；FIPS 构建 fail-closed。
pub fn enable_fips_only_for_build(requested: bool) -> Result<(), String> {
    if requested {
        astersql_server::server::install_fips_crypto_provider()
    } else {
        Ok(())
    }
}
