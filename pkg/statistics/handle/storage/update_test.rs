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

use std::sync::Mutex;

#[derive(Default)]
struct RecordingStore {
    statements: Mutex<Vec<String>>,
}

impl crate::SqlStore for RecordingStore {
    fn start_ts(&self) -> Result<u64, crate::Error> {
        Ok(100)
    }

    fn execute(&self, sql: &str) -> Result<Vec<crate::Row>, crate::Error> {
        self.statements.lock().unwrap().push(sql.to_owned());
        Ok(Vec::new())
    }
}

#[test]
fn locked_negative_delta_keeps_its_sign_like_go() {
    let store = RecordingStore::default();
    let updates = [crate::new_delta_update(
        7,
        crate::TableDelta {
            count: 2,
            delta: -3,
        },
        true,
    )];

    crate::update_stats_meta(&store, 42, &updates).unwrap();

    let statements = store.statements.lock().unwrap();
    assert_eq!(
        statements[1],
        "insert into mysql.stats_table_locked (version,table_id,modify_count,count) values (42,7,2,-3) on duplicate key update version=values(version),modify_count=modify_count+values(modify_count),count=count + values(count)"
    );
}
