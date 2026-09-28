// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

use crate::group_1::*;

#[test]
fn db_info_json_matches_go_field_names_and_omissions() {
    let mut database = DBInfo {
        ID: 42,
        Name: ast::NewCIStr("TeSt"),
        Charset: "utf8mb4".to_owned(),
        Collate: "utf8mb4_bin".to_owned(),
        State: StatePublic,
        PlacementPolicyRef: Some(PolicyRefInfo {
            ID: 7,
            Name: ast::NewCIStr("policy"),
        }),
        ..Default::default()
    };
    database.TableName2ID.insert("hidden".to_owned(), 9);
    database.Deprecated.Tables.push(Default::default());

    let encoded = ast::metadata_json::encode(&database).unwrap();
    let json = String::from_utf8(encoded).unwrap();

    assert!(json.contains(r#""id":42"#), "{json}");
    assert!(
        json.contains(r#""db_name":{"O":"TeSt","L":"test"}"#),
        "{json}"
    );
    assert!(json.contains(r#""charset":"utf8mb4""#), "{json}");
    assert!(json.contains(r#""collate":"utf8mb4_bin""#), "{json}");
    assert!(json.contains(r#""state":5"#), "{json}");
    assert!(json.contains(r#""policy_ref_info""#), "{json}");
    assert!(json.contains(r#""Deprecated":{}"#), "{json}");
    assert!(!json.contains("TableName2ID"), "{json}");
    assert!(!json.contains("Tables"), "{json}");

    let decoded: DBInfo = ast::metadata_json::decode(b"{}").unwrap();
    assert_eq!(decoded.ID, 0);
    assert!(decoded.Name.L.is_empty());
    assert!(decoded.Deprecated.Tables.is_empty());
    assert!(decoded.TableName2ID.is_empty());
}
