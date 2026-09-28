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

use crate::group_2::serde_json;
use crate::group_3::{MaskingPolicyInfo, MaskingPolicyStatus};

#[test]
fn masking_policy_json_matches_go_metadata_shape() {
    let mut policy = MaskingPolicyInfo::default();
    policy.Status = MaskingPolicyStatus::MaskingPolicyStatusEnable;

    let json = serde_json::to_value(policy).expect("serialize masking policy");

    assert_eq!(json["status"], serde_json::json!(1));
    assert!(json.get("Status").is_none());
    assert!(json.get("Expression").is_none());
    assert!(json.get("restrict_ops").is_none());
    assert_eq!(
        json["created_at"],
        serde_json::json!("0001-01-01T00:00:00Z")
    );
    assert_eq!(
        json["updated_at"],
        serde_json::json!("0001-01-01T00:00:00Z")
    );
}

#[test]
fn masking_policy_status_round_trips_as_go_byte_value() {
    let enabled: MaskingPolicyStatus = serde_json::from_str("1").expect("decode enabled status");
    assert_eq!(enabled, MaskingPolicyStatus::MaskingPolicyStatusEnable);
    let unknown: MaskingPolicyStatus = serde_json::from_str("2").expect("decode unknown Go byte");
    assert_eq!(unknown.String(), "");
}
