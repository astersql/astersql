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

//! Non-POSIX dynamic pprof listener stub from `br/pkg/utils/dyn_pprof_other.go`.
//! 非 POSIX 平台的动态 pprof 监听空实现；有信号支持的平台走另一编译单元。

use astersql_util::security::TLS;

/// No-op on platforms without POSIX signal support.
/// 无 SIGUSR 等信号能力时启动监听为空操作，避免链接失败。
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd", unix)))]
pub fn StartDynamicPProfListener(_tls: Option<&TLS>) {}
