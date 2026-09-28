// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/config/ebs_test.go`.
//!
//! `TestParseConfig` loads the checked-in `ebs_backup.json` fixture via
//! `os.Getwd` + `filepath.Join`; this Rust test mirrors that with
//! `std::env::current_dir` and real filesystem I/O (no mocks).
//!
//! 对照 Go `TestParseConfig`：在真实工作目录下加载同目录 `ebs_backup.json`，
//! 验证 `ConfigFromFile` 能成功反序列化；不做内容字段断言（更细契约见 parity_test）。

use crate::EBSBasedBRMeta;

/// Corresponds to Go `TestParseConfig`.
/// 仅断言加载成功；路径解析失败或 JSON 非法都会使测试失败。
#[test]
fn test_parse_config() {
    let mut cfg = EBSBasedBRMeta::default();
    // 与 Go os.Getwd + filepath.Join("ebs_backup.json") 等价。
    let cur_dir = std::env::current_dir().expect("Go require.NoError: os.Getwd");
    let config_path = cur_dir.join("ebs_backup.json");
    cfg.ConfigFromFile(config_path.to_str().expect("utf-8 path"))
        .expect("Go require.NoError: ConfigFromFile");
}

#[test]
fn config_json_matches_go_field_and_null_semantics() {
    let config: EBSBasedBRMeta = serde_json::from_str(
        r#"{
            "cluster_info":{"version":"ignored","replicas":null},
            "tikv":{"replicas":2147483648,"stores":[{"store_id":1,"volumes":null}]},
            "pd":{"replicas":2147483648},
            "tidb":{"replicas":2147483648},
            "kubernetes":{"pvs":null,"pvcs":null,"options":null},
            "options":null
        }"#,
    )
    .expect("Go encoding/json accepts null slices/maps and 64-bit int values");

    let cluster = config.ClusterInfo.expect("cluster info");
    assert_eq!(
        cluster.Version, "ignored",
        "the checked-in legacy fixture requires the `version` alias"
    );
    assert!(cluster.Replicas.is_empty());
    let tikv = config.TiKVComponent.expect("tikv component");
    assert_eq!(tikv.Replicas, 2_147_483_648);
    assert_eq!(tikv.Stores.len(), 1);
    assert!(tikv.Stores[0].Volumes.is_empty());
    assert_eq!(
        config.PDComponent.expect("pd component").Replicas,
        2_147_483_648
    );
    assert_eq!(
        config.TiDBComponent.expect("tidb component").Replicas,
        2_147_483_648
    );
    let kubernetes = config.KubernetesMeta.expect("kubernetes metadata");
    assert!(kubernetes.PVs.is_empty());
    assert!(kubernetes.PVCs.is_empty());
    assert!(kubernetes.Options.is_empty());
    assert!(config.Options.is_empty());
}
