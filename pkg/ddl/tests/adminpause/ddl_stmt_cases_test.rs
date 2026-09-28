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

use crate::ddl_stmt_cases::{ALTER_COLUMN_PREFIX, AutoIncrsedID, column_ddl_stmt_case};

#[test]
fn modify_column_reorganization_sql_matches_go_exactly() {
    let mut ai = AutoIncrsedID { idx: 0 };
    let cases = column_ddl_stmt_case(&mut ai);
    let expected = format!("{}char(10);", ALTER_COLUMN_PREFIX);
    let reorganization_cases = cases
        .iter()
        .filter(|case| case.stmt.ends_with("char(10);"))
        .collect::<Vec<_>>();

    assert_eq!(reorganization_cases.len(), 5);
    assert!(
        reorganization_cases
            .iter()
            .all(|case| case.stmt == expected),
        "Go concatenates alterColumnPrefix directly with `char(10);`"
    );
}
