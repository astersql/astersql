// Copyright 2026 AsterSQL.
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

// 对照 Go `testflag` 的迁移单元测试。
//
// 覆盖默认值、Go 布尔拼写以及“后者覆盖前者”的 flag 语义。

use super::{Long, long_from_args};

/// 未提供 `-long` 时默认为 false。
#[test]
fn long_defaults_to_false_without_the_flag() {
    assert!(!Long());
    assert!(!long_from_args(["test-binary"]));
}

/// 接受 Go `flag` 包常见的布尔写法（裸标志与 `=true`/`=1`/`=false`/`=0`）。
#[test]
fn long_accepts_go_boolean_flag_forms() {
    assert!(long_from_args(["test-binary", "-long"]));
    assert!(long_from_args(["test-binary", "--long"]));
    assert!(long_from_args(["test-binary", "-long=true"]));
    assert!(long_from_args(["test-binary", "--long=1"]));
    assert!(!long_from_args(["test-binary", "-long=false"]));
    assert!(!long_from_args(["test-binary", "--long=0"]));
}

/// 多次出现时以最后一次为准，与 Go `flag` 行为一致。
#[test]
fn long_uses_the_last_occurrence_like_go_flag() {
    assert!(!long_from_args(["test-binary", "-long", "--long=false",]));
    assert!(long_from_args([
        "test-binary",
        "-long=false",
        "--long=true",
    ]));
}
