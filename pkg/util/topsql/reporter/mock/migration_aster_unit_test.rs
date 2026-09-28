// Copyright 2026 AsterSQL.
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

// Mock TopSQL Agent / PubSub 的迁移补充单元测试。
//
// 验证：回环绑定与干净停止、多种上报流（TopSQL/TopRU/SQL Meta/Plan Meta）的收集语义，
// 以及 Hang 窗口对接收延迟的影响。对应 Go mock 测试的关键行为。

use std::time::{Duration, Instant};

use tokio_stream::iter;
use topsql_mock::pubsub::NewMockPubSubServer;
use topsql_mock::server::StartMockAgentServer;
use topsql_mock::tipb::{
    PlanMeta, SqlMeta, TopRuRecord, TopSqlRecord, top_sql_agent_client::TopSqlAgentClient,
};

/// 验证 mock PubSub 绑定 127.0.0.1 动态端口，且重复 Stop 安全。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pubsub_binds_loopback_and_stops_cleanly() {
    let mut server = NewMockPubSubServer().expect("bind pubsub mock");
    assert!(server.Address().starts_with("127.0.0.1:"));
    assert_ne!(server.Address(), "127.0.0.1:0");
    server.Serve().expect("serve pubsub mock");
    server.Stop();
    server.Stop();
}

/// 上报四类流后，校验计数、按 digest 阻塞查询与 GetLatest* 一次性取走语义。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_collects_all_stream_kinds_and_preserves_go_lookup_semantics() {
    let mut server = StartMockAgentServer().await.expect("start agent");
    let endpoint = format!("http://{}", server.Address());
    let mut client = TopSqlAgentClient::connect(endpoint).await.expect("connect");

    // 依次上报 TopSQL / TopRU / SqlMeta / PlanMeta，再断言 mock 侧聚合结果。
    client
        .report_top_sql_records(iter(vec![
            TopSqlRecord {
                sql_digest: b"sql-a".to_vec(),
                plan_digest: b"plan-a".to_vec(),
            },
            TopSqlRecord {
                sql_digest: b"sql-b".to_vec(),
                plan_digest: b"plan-b".to_vec(),
            },
        ]))
        .await
        .expect("report SQL records");
    client
        .report_top_ru_records(iter(vec![TopRuRecord {
            sql_digest: b"sql-a".to_vec(),
            ru: 3.5,
        }]))
        .await
        .expect("report RU records");
    client
        .report_sql_meta(iter(vec![
            SqlMeta {
                sql_digest: vec![0xff, 0x00],
                normalized_sql: "select ?".into(),
            },
            SqlMeta {
                sql_digest: b"sql-b".to_vec(),
                normalized_sql: "select ? + ?".into(),
            },
        ]))
        .await
        .expect("report SQL metas");
    client
        .report_plan_meta(iter(vec![PlanMeta {
            plan_digest: vec![0xfe, 0x01],
            normalized_plan: "TableReader".into(),
        }]))
        .await
        .expect("report plan meta");

    server.WaitCollectCnt(0, 1, Duration::from_millis(100));
    server.WaitCollectCntOfSQLMeta(0, 2, Duration::from_millis(100));
    assert_eq!(server.RecordsCnt(), 1);
    assert_eq!(server.RURecordsCnt(), 1);
    assert_eq!(server.SQLMetaCnt(), 2);

    let (meta, exists) = server.GetSQLMetaByDigestBlocking(&[0xff, 0x00], Duration::ZERO);
    assert!(exists);
    assert_eq!(meta.normalized_sql, "select ?");
    let (plan, exists) = server.GetPlanMetaByDigestBlocking(&[0xfe, 0x01], Duration::ZERO);
    assert!(exists);
    assert_eq!(plan, "TableReader");
    assert_eq!(server.GetTotalSQLMetas().len(), 2);

    let latest = server.GetLatestRecords().expect("latest SQL batch");
    assert_eq!(latest.len(), 2);
    assert!(server.GetLatestRecords().is_none());
    let latest_ru = server.GetLatestRURecords().expect("latest RU batch");
    assert_eq!(latest_ru[0].ru, 3.5);
    assert!(server.GetLatestRURecords().is_none());
    server.Stop();
}

/// HangFromNow 使流式接收在窗口结束前阻塞，对齐 Go 侧模拟慢下游。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hang_delays_stream_receive_until_window_ends() {
    let mut server = StartMockAgentServer().await.expect("start agent");
    let endpoint = format!("http://{}", server.Address());
    let mut client = TopSqlAgentClient::connect(endpoint).await.expect("connect");
    server.HangFromNow(Duration::from_millis(45));

    let started = Instant::now();
    client
        .report_top_sql_records(iter(vec![TopSqlRecord::default()]))
        .await
        .expect("report after hang");
    assert!(started.elapsed() >= Duration::from_millis(35));
    server.Stop();
}
