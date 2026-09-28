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

//! 对应 Go `br/pkg/gluetikv/glue_test.go`：校验 TiKV Glue 的版本字符串格式。
//! 断言顺序与 Go `require.Regexp` 一致，避免仅检查子串而忽略前后缀约束。

use astersql_br_pkg_glue::Glue as GlueTrait;

use crate::Glue;

/// 对应 Go `TestGetVersion`：版本须以 `BR` 开头，且依次含 Release Version / Git Commit Hash。
///
/// Go: `require.Regexp(t, "^BR(.|\n)*Release Version(.|\n)*Git Commit Hash(.|\n)*$", g.GetVersion())`
#[test]
fn test_get_version() {
    let g = Glue::new();
    let version = g.GetVersion();

    // 等价于 Go 多行正则：先 BR，再 Release Version，最后 Git Commit Hash。
    assert!(
        version.starts_with("BR"),
        "version must start with BR, got {version:?}"
    );
    let after_br = &version["BR".len()..];
    let rv = after_br
        .find("Release Version")
        .unwrap_or_else(|| panic!("missing Release Version in {version:?}"));
    let after_rv = &after_br[rv..];
    assert!(
        after_rv.contains("Git Commit Hash"),
        "Git Commit Hash must follow Release Version in {version:?}"
    );
}
