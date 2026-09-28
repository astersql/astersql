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

// 默认（非 `ddlargsv1`）构建下的 DDL 参数版本开关。
//
// 当未启用 `ddlargsv1` feature 时由本模块提供 `FORCE_V1`/`ForceV1`，
// 固定为 `false`，表示测试使用当前默认的 DDL Job 参数格式（V2）。
// 与 `force_v1` 模块互斥：后者在启用 feature 时导出为 `true`。

/// Whether tests must use DDL job V1 arguments.
///
/// DDL args V2 is the default since TiDB 8.4.0. The feature-gated variant
/// keeps the V1 compatibility path covered by a second test run.
///
/// 是否强制测试使用 DDL Job V1 参数；本模块恒为 `false`。
#[cfg(not(feature = "ddlargsv1"))]
pub const FORCE_V1: bool = false;

/// Go-compatible name retained for translated callers.
///
/// 与 Go 侧 `ForceV1` 同名的别名，便于机械迁移后的调用方直接引用。
#[cfg(not(feature = "ddlargsv1"))]
#[allow(non_upper_case_globals)]
pub const ForceV1: bool = FORCE_V1;
