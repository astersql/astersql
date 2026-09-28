// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

#[test]
fn lock_sql_constants_match_go_public_contract() {
    assert_eq!(
        crate::LockBindInfoSQL,
        "UPDATE mysql.bind_info SET source= 'builtin' WHERE original_sql= 'builtin_pseudo_sql_for_bind_lock'"
    );
    assert_eq!(
        crate::StmtRemoveDuplicatedPseudoBinding,
        "DELETE FROM mysql.bind_info\n       WHERE original_sql='builtin_pseudo_sql_for_bind_lock' AND\n       _tidb_rowid NOT IN ( -- keep one arbitrary pseudo binding\n         SELECT _tidb_rowid FROM mysql.bind_info WHERE original_sql='builtin_pseudo_sql_for_bind_lock' limit 1)"
    );
}
