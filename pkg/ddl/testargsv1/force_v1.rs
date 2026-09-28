// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 强制测试使用 DDL job V1 参数格式的开关。
//
// TiDB DDL job 参数经历过 V1 → V2 的演进；启用 `ddlargsv1` feature 时，
// 本模块导出 [`FORCE_V1`] = true，让测试夹具始终走 V1 参数编解码路径，
// 以覆盖兼容性与迁移场景。

/// Whether tests must use DDL job V1 arguments.
///
/// 测试是否必须使用 DDL job V1 参数格式（启用 `ddlargsv1` feature 时为 true）。
#[cfg(feature = "ddlargsv1")]
pub const FORCE_V1: bool = true;

/// Go-compatible name retained for translated callers.
///
/// 保留与 Go 源码同名的导出，供机械翻译后的调用方直接引用。
#[cfg(feature = "ddlargsv1")]
#[allow(non_upper_case_globals)]
pub const ForceV1: bool = FORCE_V1;
