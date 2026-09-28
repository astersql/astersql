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

//! Non-Unix SIGUSR1 handler — ported from `sigusr1_other.go`.
//!
//!
//! 非 Unix 平台没有 `SIGUSR1` 这个控制通道，因此这里必须显式保持空实现。
//! 保留统一函数签名的目的，是让上层 `Lightning::GoServe()` 不需要再写平台分支。

/// handleSigUsr1 does nothing outside of Unix, since SIGUSR1 does not exist.
pub fn handleSigUsr1<F>(_handler: F)
where
    F: Fn(),
{
}
