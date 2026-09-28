// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

use crate::runtime::split_statement_sql;

#[test]
fn double_dash_requires_mysql_comment_whitespace() {
    assert_eq!(
        split_statement_sql("SELECT 1--2; SELECT 3"),
        vec!["SELECT 1--2".to_owned(), "SELECT 3".to_owned()]
    );
    assert_eq!(
        split_statement_sql("SELECT 1-- comment;\n; SELECT 3"),
        vec!["SELECT 1-- comment;".to_owned(), "SELECT 3".to_owned()]
    );
}
