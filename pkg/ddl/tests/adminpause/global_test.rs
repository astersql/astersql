// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

use std::time::Duration;

#[test]
fn prepare_domain_matches_go_lease_and_session_lifecycle() {
    let prepared = crate::prepare_domain(&());

    assert_eq!(
        prepared.domain.schema_lease(),
        Duration::from_millis(crate::DB_TEST_LEASE_MILLIS)
    );
    assert!(
        prepared.stmt_kit.ConnectionID() > prepared.admin_command_kit.ConnectionID(),
        "the Go setup session must be replaced after configuring reorg variables"
    );
}
