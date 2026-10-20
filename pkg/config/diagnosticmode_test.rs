// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::diagnosticmode::{Enabled, ErrDDLNotAllowed, Initialize, SetForTest, reset_for_test};

#[test]
fn diagnostic_mode_matches_go_initialize_and_test_override() {
    let original = reset_for_test();

    assert!(!Enabled());
    Initialize(false).expect("initialize diagnostic mode as disabled");
    assert!(!Enabled());
    Initialize(false).expect("reinitializing with the same value is allowed");
    let error = Initialize(true).expect_err("initialized mode must be immutable");
    assert!(error.to_string().contains("cannot be changed to enabled"));
    assert!(!Enabled());

    drop(reset_for_test());
    Initialize(true).expect("initialize diagnostic mode as enabled");
    assert!(Enabled());
    Initialize(true).expect("reinitializing with the same value is allowed");
    let error = Initialize(false).expect_err("initialized mode must be immutable");
    assert!(error.to_string().contains("cannot be changed to disabled"));
    assert!(Enabled());

    drop(reset_for_test());
    let restore = SetForTest(true);
    assert!(Enabled());
    restore();
    assert!(!Enabled());

    assert_eq!(
        ErrDDLNotAllowed.to_string(),
        "DDL operations are not allowed in diagnostic mode"
    );

    original();
}
