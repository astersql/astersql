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

//! Response contract and query parsing for `GET /tiflash/replica`.

/// Best-effort count of live tables carrying TiFlash replica metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlashReplicaSummary {
    pub keyspace: String,
    pub keyspace_id: u32,
    pub tidb_columnar_storage_enabled: String,
    pub columnar_store_type: String,
    pub can_disable: bool,
    pub table_count: usize,
    pub reloaded: bool,
}

impl FlashReplicaSummary {
    /// Preserve the Go JSON field names without exposing table identities.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "keyspace": self.keyspace,
            "keyspace_id": self.keyspace_id,
            "tidb_columnar_storage_enabled": self.tidb_columnar_storage_enabled,
            "columnar_store_type": self.columnar_store_type,
            "can_disable": self.can_disable,
            "table_count": self.table_count,
            "reloaded": self.reloaded,
        })
    }
}

/// Missing/empty values default to false. Explicit values match Go's
/// `strconv.ParseBool` spellings used by the source endpoint.
pub fn parse_flash_replica_reload_query(raw: Option<&str>) -> Result<bool, String> {
    let raw = raw.unwrap_or_default();
    if raw.is_empty() {
        return Ok(false);
    }
    match raw {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(format!(
            "invalid reload query value {raw:?}, expect true/false/1/0"
        )),
    }
}
