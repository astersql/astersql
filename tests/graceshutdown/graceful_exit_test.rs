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

use astersql_tests_graceshutdown::{GRACEFUL_SIGINT_EXIT_CODE, classify_stop_wait};

#[test]
fn graceful_exit_accepts_success_and_sigint_but_preserves_other_wait_errors() {
    assert_eq!(classify_stop_wait(None, 0), Ok(()));
    assert_eq!(
        classify_stop_wait(Some("exit status: 130"), GRACEFUL_SIGINT_EXIT_CODE),
        Ok(())
    );
    assert_eq!(
        classify_stop_wait(Some("exit status: 1"), 1),
        Err("exit status: 1".to_owned())
    );
}
