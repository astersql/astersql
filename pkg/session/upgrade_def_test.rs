// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[test]
fn upgrade_functions_match_go_order_and_current_version() {
    let functions = &*crate::upgrade_def::upgradeToVerFunctions;
    assert_eq!(functions.len(), 176);

    let mut previous = 0;
    for entry in functions {
        assert!(entry.version > previous);
        previous = entry.version;
    }
    assert_eq!(
        previous,
        // SAFETY: the test only reads the compatibility variable.
        unsafe { crate::upgrade_def::currentBootstrapVersion },
    );
    assert_eq!(previous, 317);
}
