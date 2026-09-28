// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// israce 迁移对照单测：验证 `RaceEnabled` 与 Cargo feature 选择一致。

/// 启用 `race` 时为 true，否则为 false，对应 Go 的 race / !race build tag。
#[test]
fn migration_race_enabled_matches_go_build_tag() {
    #[cfg(feature = "race")]
    assert!(super::RaceEnabled);

    #[cfg(not(feature = "race"))]
    assert!(!super::RaceEnabled);
}
