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

use super::assert_common::{AssertArg, assertionFailedMsg};

#[test]
fn missing_format_argument_matches_go_fmt() {
    assert_eq!(
        assertionFailedMsg("", &[AssertArg::from("value=%s")]),
        "assert failed, value=%!s(MISSING)"
    );
    assert_eq!(
        assertionFailedMsg("", &[AssertArg::from("values=%d/%+v"), AssertArg::from(7)]),
        "assert failed, values=7/%!v(MISSING)"
    );
}

#[test]
fn invalid_and_extra_format_arguments_match_go_fmt() {
    assert_eq!(
        assertionFailedMsg("", &[AssertArg::from("value=%d"), AssertArg::from("text")]),
        "assert failed, value=%!d(string=text)"
    );
    assert_eq!(
        assertionFailedMsg(
            "",
            &[
                AssertArg::from("plain"),
                AssertArg::from("text"),
                AssertArg::from(7_i64),
            ]
        ),
        "assert failed, plain%!(EXTRA string=text, int64=7)"
    );
}
