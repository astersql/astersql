// Copyright 2025 PingCAP, Inc.
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

// 调度器状态（schstatus）与调参因子的迁移回归单元测试。
//
// 对照 Go 行为校验 Status / TTLFlag / TTLTuneFactors 的 JSON 字段名、
// 结构体嵌入展平（flatten）、omitempty，以及 BusyNodes 截断打印不修改原对象。

use super::status::{Node, PauseScaleInFlag, Status, TTLFlag, TTLInfo, Version1};
use super::tune::{GetDefaultTuneFactors, MaxAmplifyFactor, MinAmplifyFactor, TTLTuneFactors};
use std::time::{Duration, SystemTime};

#[test]
/// 验证 Status::String：BusyNodes>5 时截断为 6 项（含说明节点），且不原地修改 Status。
fn status_string_matches_go_json_and_does_not_mutate_busy_nodes() {
    let mut status = Status::default();
    status.Version = Version1;
    // 构造 10 个忙碌节点，触发「保留前 5 个并追加说明节点」的截断逻辑。
    for i in 0..10 {
        status.TiDBWorker.BusyNodes.push(Node {
            ID: format!("tidb-{i}"),
            ..Default::default()
        });
    }

    let encoded = status.String();
    let parsed: Status = serde_json::from_str(&encoded).expect("Status.String returns JSON");
    assert_eq!(parsed.TiDBWorker.BusyNodes.len(), 6);
    assert!(
        parsed.TiDBWorker.BusyNodes[5]
            .ID
            .contains("too many nodes, total 10 busy nodes")
    );
    assert_eq!(status.TiDBWorker.BusyNodes.len(), 10);
    assert_eq!(serde_json::to_value(&parsed).unwrap()["version"], 1);
}

#[test]
/// 验证 serde 字段命名、TTLFlag 嵌入展平，以及零值字段 omitempty。
fn status_json_uses_go_field_names_embedding_and_omitempty() {
    let mut status = Status::default();
    status.Flags.insert(
        PauseScaleInFlag.to_owned(),
        TTLFlag {
            Enabled: true,
            TTLInfo: TTLInfo {
                TTL: Duration::from_secs(2),
                ExpireTime: SystemTime::UNIX_EPOCH,
            },
        },
    );

    let value = serde_json::to_value(&status).unwrap();
    assert_eq!(value["task_queue"], serde_json::json!({}));
    assert_eq!(value["tidb_worker"], serde_json::json!({}));
    assert_eq!(value["tikv_worker"], serde_json::json!({}));
    assert_eq!(value["flags"][PauseScaleInFlag]["enabled"], true);
    assert_eq!(value["flags"][PauseScaleInFlag]["ttl"], 2_000_000_000_i64);
    assert_eq!(
        value["flags"][PauseScaleInFlag]["expire_time"],
        "1970-01-01T00:00:00Z"
    );
    assert!(value.get("Version").is_none());
    assert!(value["flags"][PauseScaleInFlag].get("TTLInfo").is_none());
}

#[test]
/// 验证默认 amplify 因子边界，以及 TTLTuneFactors 嵌入字段展平为顶层 JSON。
fn tune_factors_match_go_defaults_limits_and_flattened_json() {
    let defaults = GetDefaultTuneFactors();
    assert_eq!(defaults.AmplifyFactor, MinAmplifyFactor);
    assert_eq!(MinAmplifyFactor, 1.0);
    assert_eq!(MaxAmplifyFactor, 10.0);

    let factors = TTLTuneFactors {
        TTLInfo: TTLInfo {
            TTL: Duration::from_nanos(15),
            ExpireTime: SystemTime::UNIX_EPOCH,
        },
        TuneFactors: defaults,
    };
    let value: serde_json::Value = serde_json::from_str(&factors.String()).unwrap();
    assert_eq!(value["ttl"], 15);
    assert_eq!(value["expire_time"], "1970-01-01T00:00:00Z");
    assert_eq!(value["amplify_factor"], 1.0);
    assert!(value.get("TTLInfo").is_none());
    assert!(value.get("TuneFactors").is_none());
}
