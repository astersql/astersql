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

use super::{TestStmt, dql_execution_sql};

#[test]
fn dql_execution_matches_go_worker_sequence() {
    let statement = TestStmt {
        normal_stmt: "select * from normal.t where a=1".to_owned(),
        prep_stmt: "prepare st from 'select * from prepared.t where a=?'".to_owned(),
        set_stmt: "set @a=1".to_owned(),
        exec_stmt: "execute st using @a".to_owned(),
    };

    assert_eq!(
        dql_execution_sql(&statement),
        [
            statement.normal_stmt.as_str(),
            statement.prep_stmt.as_str(),
            statement.set_stmt.as_str(),
            statement.exec_stmt.as_str(),
        ]
    );
}
