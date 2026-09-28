// Copyright 2021 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// TiKVDriver 默认配置与路径解析单元测试。
//
// 覆盖从全局配置填充 driver、Option 覆盖安全项但不污染全局、PD Client 选项
// （keep-alive、超时、metrics labels、转发开关），以及 `tikv://` 路径中逗号分隔
// 的 PD 地址与 query 参数解析。

use std::collections::HashMap;

use super::*;
use crate::test_state::global_state_guard;

/// 验证 setDefaultAndOptions：Option 覆盖 security，其余字段取自全局配置。
#[test]
fn TestSetDefaultAndOptions() {
    let _guard = global_state_guard();
    let global = GlobalConfig {
        enable_forwarding: true,
        security: Security {
            cluster_ssl_ca: "original".into(),
            ..Default::default()
        },
        tikv_client: TiKvClientConfig {
            grpc_keep_alive_time: 21,
            grpc_keep_alive_timeout: 8,
        },
        // TxnLocalLatches：本地闩锁，用于降低写冲突时的调度开销。
        txn_local_latches: TxnLocalLatches {
            enabled: true,
            capacity: 4096,
        },
        pd_client: PdClientConfig {
            pd_server_timeout: 17,
        },
        ..Default::default()
    };
    set_global_config(global.clone());

    // WithSecurity 只改 driver 本地 security，不应回写全局配置。
    let security = Security {
        cluster_ssl_ca: "test".into(),
        ..Default::default()
    };
    let mut driver = TiKVDriver::default();
    driver.setDefaultAndOptions(vec![WithSecurity(security.clone())]);

    assert_eq!(driver.security, security);
    assert_eq!(driver.tikv_config, global.tikv_client);
    assert_eq!(driver.txn_local_latches, global.txn_local_latches);
    assert_eq!(driver.pd_config, global.pd_client);
    assert_eq!(get_global_config().security.cluster_ssl_ca, "original");

    // const labels（如 keyspace）会进入 PD 客户端的 metrics 标签。
    set_const_labels([
        ("keyspace_id".to_owned(), "42".to_owned()),
        ("keyspace_name".to_owned(), "ks".to_owned()),
    ]);
    let options = driver.pdClientOptions();
    assert_eq!(options.metrics_labels, get_const_labels());
    assert_eq!(options.max_receive_message_size, i32::MAX);
    assert_eq!(options.keep_alive_time.as_secs(), 21);
    assert_eq!(options.keep_alive_timeout.as_secs(), 8);
    assert_eq!(options.server_timeout.as_secs(), 17);
    assert!(options.enable_forwarding);

    // 清理全局状态，避免影响后续测试。
    set_const_labels(HashMap::new());
    set_global_config(GlobalConfig::default());
}

/// 解析 `tikv://pd1,pd2?...` 时保留逗号分隔的多 PD 地址，并校验非法 scheme/参数。
#[test]
fn parses_driver_path_without_losing_comma_separated_pd_addresses() {
    let parsed =
        parse_path("tikv://pd-1:2379,pd-2:2379?disableGC=true&keyspaceName=analytics").unwrap();
    assert_eq!(parsed.pd_addrs, ["pd-1:2379", "pd-2:2379"]);
    assert!(parsed.disable_gc);
    assert_eq!(parsed.keyspace_name, "analytics");
    assert!(parse_path("mysql://127.0.0.1").is_err());
    assert!(parse_path("tikv://pd:2379?disableGC=not-bool").is_err());
}
