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

use crate::{FlashReplicaSummary, parse_flash_replica_reload_query};

#[test]
fn reload_query_matches_go_parse_bool_contract() {
    for value in [None, Some(""), Some("0"), Some("false"), Some("False")] {
        assert_eq!(Ok(false), parse_flash_replica_reload_query(value));
    }
    for value in [Some("1"), Some("true"), Some("TRUE"), Some("T")] {
        assert_eq!(Ok(true), parse_flash_replica_reload_query(value));
    }
    assert!(parse_flash_replica_reload_query(Some("maybe")).is_err());
}

#[test]
fn summary_json_has_only_operator_fields() {
    let value = FlashReplicaSummary {
        keyspace: "ks".into(),
        keyspace_id: 7,
        tidb_columnar_storage_enabled: "OFF".into(),
        columnar_store_type: "columnar".into(),
        can_disable: false,
        table_count: 2,
        reloaded: true,
    }
    .to_json();
    assert_eq!(value["table_count"], 2);
    assert!(value.get("tables").is_none());
}
