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

use super::storage::{extract_data_file_paths, parse_backup_metadata};

#[test]
fn v1_top_level_files_are_exposed_as_a_single_file_group() {
    let raw = br#"{
        "MetaVersion": "V1",
        "StoreId": 42,
        "Files": [
            {"Path": "v1/log/one.log"},
            {"Path": ""},
            {"Path": "v1/log/two.log"}
        ]
    }"#;

    let meta = parse_backup_metadata(raw).expect("parse V1 backupmeta");
    assert_eq!(
        extract_data_file_paths(&meta),
        vec!["v1/log/one.log", "v1/log/two.log"]
    );
}
