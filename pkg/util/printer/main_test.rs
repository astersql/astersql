// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

use crate::mysql;
use crate::testsetup;
use serial_test::serial;

#[test]
fn test_main_preserves_common_setup_and_go_leak_allowlist() {
    testsetup::SetupForCommonTest();
}

/// Go 始终只规范化固定的旧占位版本，不会把构建时注入的版本误判为占位符。
#[test]
#[serial]
fn normalize_release_version_uses_the_fixed_legacy_placeholder() {
    let original = unsafe { mysql::TiDBReleaseVersion };
    unsafe { mysql::TiDBReleaseVersion = "v9.0.0" };

    let normalized_placeholder =
        mysql::NormalizeTiDBReleaseVersionForNextGen("v8.4.0-this-is-a-placeholder");
    let normalized_injected = mysql::NormalizeTiDBReleaseVersionForNextGen("v9.0.0");
    unsafe { mysql::TiDBReleaseVersion = original };

    assert_eq!("v26.3.0-this-is-a-placeholder", normalized_placeholder);
    assert_eq!("v9.0.0", normalized_injected);
}
