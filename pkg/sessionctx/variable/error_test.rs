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

use crate::error::{ErrUnknownSystemVar, ErrorClass, ErrorDescriptor};

#[test]
fn format_applies_go_string_precision_in_characters() {
    let long_name = "变量".repeat(40);
    let expected_name = "变量".repeat(32);

    assert_eq!(
        ErrUnknownSystemVar.format(&[&long_name]),
        format!("Unknown system variable '{expected_name}'")
    );
}

#[test]
fn format_preserves_non_ascii_template_text_and_percent_literals() {
    let descriptor = ErrorDescriptor::new(ErrorClass::Variable, 1, "变量 %% '%-.2s'");

    assert_eq!(descriptor.format(&["甲乙丙"]), "变量 % '甲乙'");
}
