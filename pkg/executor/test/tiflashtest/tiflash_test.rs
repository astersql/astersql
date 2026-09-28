// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// MPP Coordinator Manager 缺协调器错误路径测试。
//
// MPP Coordinator 负责协调一次并行查询（query）下的多个 task；当
// `report_status` 找不到对应协调器时，应携带请求中的 `mpp_version` 返回
// `"MppCoordinator not exists"`，便于调用方按协议版本处理。

/// 未注册协调器时，`report_status` 须拒绝并回显请求的 MPP 协议版本。
#[test]
fn mpp_manager_reports_missing_coordinator_with_request_version() {
    use std::time::Duration;

    use astersql_executor_mppcoordmanager::{
        MppCoordinatorManager, ReportTaskMeta, ReportTaskStatusRequest,
    };

    // 仅初始化服务地址，不注册任何 query 的 coordinator。
    let manager = MppCoordinatorManager::new(Duration::from_secs(60));
    manager.init_server_address(true, "127.0.0.1:3930".to_owned());
    assert_eq!(
        manager.server_address(),
        (true, "127.0.0.1:3930".to_owned())
    );

    // 任意未登记的 query_ts/local_query_id 组合都应走“协调器不存在”分支。
    let response = manager.report_status(&ReportTaskStatusRequest {
        meta: ReportTaskMeta {
            query_ts: 42,
            local_query_id: 7,
            server_id: 3,
            gather_id: 11,
            task_id: 9,
            mpp_version: 2,
        },
        data: vec![1, 2, 3],
    });
    let error = response.error.expect("unknown MPP query must be rejected");
    assert_eq!(error.mpp_version, 2);
    assert_eq!(error.message, "MppCoordinator not exists");
}

/// Go `withMockTiFlash`：mock 集群必须保留默认 TiKV store，并为每个请求的
/// TiFlash 节点创建带 `engine=tiflash` 标签的 store 和 Region peer。
#[test]
fn with_mock_tiflash_builds_labeled_stores_and_region_peers() {
    use astersql_store_mockstore::{NewMockStore, StoreType, WithMockTiFlash};

    let store = NewMockStore(vec![WithMockTiFlash(2)]).expect("create mock TiFlash store");
    assert_eq!(store.backend, StoreType::EmbedUnistore);

    let manager = store.cluster.region_manager();
    let mut tiflash_addresses = manager
        .all_stores()
        .into_iter()
        .filter(|store| {
            store
                .labels
                .iter()
                .any(|label| label.key == "engine" && label.value == "tiflash")
        })
        .map(|store| store.address)
        .collect::<Vec<_>>();
    tiflash_addresses.sort();
    assert_eq!(tiflash_addresses, vec!["tiflash0", "tiflash1"]);

    let regions = manager.scan_regions(&[], &[], 1);
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].meta.peers.len(), 3);
    store.close().expect("close mock TiFlash store");
}

/// Go `TestTiFlashComputeDispatchPolicy`：合法策略名必须双向映射，未知名
/// 必须报错，非法枚举必须稳定显示为 `invalid`。
#[test]
fn tiflash_dispatch_policy_round_trips_canonical_names() {
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;
    use astersql_testkit::{NewTestKit, Rows};
    use astersql_util_tiflashcompute::{
        DispatchPolicyConsistentHash, DispatchPolicyInvalid, DispatchPolicyRR, GetDispatchPolicy,
        GetDispatchPolicyByStr, GetValidDispatchPolicy,
    };

    assert_eq!(
        GetValidDispatchPolicy(),
        vec!["consistent_hash", "round_robin"]
    );
    assert_eq!(
        GetDispatchPolicyByStr("consistent_hash"),
        Ok(DispatchPolicyConsistentHash)
    );
    assert_eq!(GetDispatchPolicyByStr("round_robin"), Ok(DispatchPolicyRR));
    assert_eq!(
        GetDispatchPolicyByStr("random")
            .expect_err("unknown policy must fail")
            .to_string(),
        "unexpected tiflash_compute dispatch policy, expect [consistent_hash round_robin], got random"
    );
    assert_eq!(GetDispatchPolicy(DispatchPolicyInvalid), "invalid");

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec("use test", Vec::new());

    // Go 默认值及 session 赋值路径：合法值可回读，非法值返回精确错误。
    tk.MustQuery("select @@tiflash_compute_dispatch_policy", Vec::new())
        .Check(Rows(&["consistent_hash"]));
    tk.MustExec(
        "set @@session.tiflash_compute_dispatch_policy = 'consistent_hash'",
        Vec::new(),
    );
    tk.MustQuery("select @@tiflash_compute_dispatch_policy", Vec::new())
        .Check(Rows(&["consistent_hash"]));
    tk.MustExec(
        "set @@session.tiflash_compute_dispatch_policy = 'round_robin'",
        Vec::new(),
    );
    tk.MustQuery("select @@tiflash_compute_dispatch_policy", Vec::new())
        .Check(Rows(&["round_robin"]));
    assert_eq!(
        tk.ExecToErr("set @@session.tiflash_compute_dispatch_policy = 'error_dispatch_policy'")
            .message(),
        "unexpected tiflash_compute dispatch policy, expect [consistent_hash round_robin], got error_dispatch_policy"
    );
    tk.MustQuery("select @@tiflash_compute_dispatch_policy", Vec::new())
        .Check(Rows(&["round_robin"]));

    // Go global 路径拒绝字符串、空串和数字非法值；合法值由新会话继承。
    for (sql, message) in [
        (
            "set global tiflash_compute_dispatch_policy = 'error_dispatch_policy'",
            "unexpected tiflash_compute dispatch policy, expect [consistent_hash round_robin], got error_dispatch_policy",
        ),
        (
            "set global tiflash_compute_dispatch_policy = ''",
            "unexpected tiflash_compute dispatch policy, expect [consistent_hash round_robin], got ",
        ),
        (
            "set global tiflash_compute_dispatch_policy = 100",
            "unexpected tiflash_compute dispatch policy, expect [consistent_hash round_robin], got 100",
        ),
    ] {
        assert_eq!(tk.ExecToErr(sql).message(), message, "sql={sql:?}");
    }
    tk.MustQuery(
        "select @@global.tiflash_compute_dispatch_policy",
        Vec::new(),
    )
    .Check(Rows(&["consistent_hash"]));

    tk.MustExec(
        "set @@session.tiflash_compute_dispatch_policy = 'consistent_hash'",
        Vec::new(),
    );
    tk.MustExec(
        "set global tiflash_compute_dispatch_policy = 'round_robin'",
        Vec::new(),
    );
    tk.MustQuery("select @@tiflash_compute_dispatch_policy", Vec::new())
        .Check(Rows(&["consistent_hash"]));
    tk.MustQuery(
        "select @@global.tiflash_compute_dispatch_policy",
        Vec::new(),
    )
    .Check(Rows(&["round_robin"]));
    let tk1 = NewTestKit(store);
    tk1.MustQuery("select @@tiflash_compute_dispatch_policy", Vec::new())
        .Check(Rows(&["round_robin"]));
}

/// Go MPP 管理器的 Run/Stop 必须可重复调用，并只在 Run 后安装有限生命周期。
#[test]
fn mpp_manager_run_stop_is_idempotent_and_sets_lifetime() {
    use std::time::Duration;

    use astersql_executor_mppcoordmanager::MppCoordinatorManager;

    let manager = MppCoordinatorManager::new(Duration::from_millis(1));
    assert_eq!(manager.coordinator_count(), 0);
    assert_eq!(manager.max_lifetime_nanos(), 0);
    manager.run();
    let lifetime = manager.max_lifetime_nanos();
    assert!(lifetime > 0);
    manager.run();
    assert_eq!(manager.max_lifetime_nanos(), lifetime);
    manager.stop();
    manager.stop();
    assert_eq!(manager.coordinator_count(), 0);
}

/// Go `TestNonsupportCharsetTable` 的错误分类：含 GBK 字符集的表不能设置
/// TiFlash 副本，错误文本必须保留 DDL 错误码和原因。
#[test]
fn tiflash_replica_rejects_gbk_table_with_go_error() {
    use astersql_testkit::NewTestKit;
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists tiflash_gbk", Vec::new());
    tk.MustExec(
        "create table tiflash_gbk(a int, b char(10) charset gbk collate gbk_bin)",
        Vec::new(),
    );
    let error = tk.ExecToErr("alter table tiflash_gbk set tiflash replica 1");
    assert_eq!(
        error.message(),
        "[ddl:8200]Unsupported `set TiFlash replica` settings for table contains gbk charset"
    );

    tk.MustExec("drop table tiflash_gbk", Vec::new());
    tk.MustExec(
        "create table tiflash_utf8(a int, b char(10) charset utf8)",
        Vec::new(),
    );
    tk.MustExec("alter table tiflash_utf8 set tiflash replica 1", Vec::new());
}
