// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use crate::ddl::Job;
use crate::schema_version::{
    RESERVED_GLOBAL_ID_LOWER_BOUND, RESERVED_GLOBAL_ID_UPPER_BOUND,
    should_check_assumed_server_for_kernel,
};

#[test]
fn test_should_check_assumed_server_matches_go_kernel_and_id_boundaries() {
    for table_id in [
        100,
        RESERVED_GLOBAL_ID_LOWER_BOUND,
        RESERVED_GLOBAL_ID_LOWER_BOUND + 1,
        RESERVED_GLOBAL_ID_UPPER_BOUND,
        RESERVED_GLOBAL_ID_UPPER_BOUND + 1,
    ] {
        let job = Job::new(1, 2, table_id, "alter table t add column c int");
        assert!(!should_check_assumed_server_for_kernel(&job, true));
    }

    for (table_id, expected) in [
        (100, false),
        (RESERVED_GLOBAL_ID_LOWER_BOUND, false),
        (RESERVED_GLOBAL_ID_LOWER_BOUND + 1, true),
        (RESERVED_GLOBAL_ID_UPPER_BOUND, true),
        (RESERVED_GLOBAL_ID_UPPER_BOUND + 1, false),
    ] {
        let job = Job::new(1, 2, table_id, "alter table t add column c int");
        assert_eq!(
            should_check_assumed_server_for_kernel(&job, false),
            expected,
            "table ID {table_id}"
        );
    }
}
