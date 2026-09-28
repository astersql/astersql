// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// tidbvar 键名与 Go 兼容别名的迁移单元测试。
//
// 确认 Rust 常量字面量与 Go `mysql.tidb` 键一致，且 PascalCase 别名指向同一字符串。

use super::{DXF_SCHEDULE_PAUSE_SCALE_IN, DXFSchedulePauseScaleIn};

/// 规范常量的字符串值须与 Go 侧键名完全一致。
#[test]
fn dxf_schedule_pause_scale_in_matches_go_key() {
    assert_eq!(DXF_SCHEDULE_PAUSE_SCALE_IN, "dxf_schedule_pause_scale_in");
}

/// Go 风格别名 `DXFSchedulePauseScaleIn` 应与规范常量同一地址语义（同值）。
#[test]
fn go_compatible_name_uses_the_canonical_rust_constant() {
    assert_eq!(DXFSchedulePauseScaleIn, DXF_SCHEDULE_PAUSE_SCALE_IN);
}
