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

// `cdcutil` 的嵌入式 etcd 风格单元测试。
//
// 用内存 `TestEtcdClient` 模拟键值，覆盖：按 safe-ts 筛选冲突 changefeed，
// 以及新旧键布局下运行中 changefeed 名称集合的枚举（忽略备份/噪声键）。

#![allow(non_snake_case)]

use std::collections::BTreeMap;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::cdc::{
    CdcError, GetIncompatibleChangefeedsWithSafeTS, GetOptions, GetRunningChangefeeds, KvClient,
    KvPair,
};

/// 测试用内存 etcd：有序 map + 读写锁，实现 `KvClient`。
#[derive(Default)]
struct TestEtcdClient {
    entries: RwLock<BTreeMap<Vec<u8>, Vec<u8>>>,
}

impl TestEtcdClient {
    /// 写入字符串键值。
    async fn put(&self, key: &str, value: &str) {
        self.entries
            .write()
            .await
            .insert(key.as_bytes().to_vec(), value.as_bytes().to_vec());
    }

    /// 删除指定前缀下的全部键。
    async fn delete_prefix(&self, prefix: &str) {
        self.entries
            .write()
            .await
            .retain(|key, _| !key.starts_with(prefix.as_bytes()));
    }
}

#[async_trait]
impl KvClient for TestEtcdClient {
    async fn get(&self, key: &str, options: GetOptions) -> Result<Vec<KvPair>, CdcError> {
        let entries = self.entries.read().await;
        Ok(entries
            .iter()
            .filter(|(entry_key, _)| {
                if options.prefix {
                    entry_key.starts_with(key.as_bytes())
                } else {
                    entry_key.as_slice() == key.as_bytes()
                }
            })
            .map(|(entry_key, value)| KvPair {
                key: entry_key.clone(),
                value: if options.keys_only {
                    Vec::new()
                } else {
                    value.clone()
                },
            })
            .collect())
    }
}

/// 仅写入 info（无 status），用于缺 checkpoint 的场景。
async fn put_lame_changefeed(cli: &TestEtcdClient, name: &str, status: &str, start_ts: u64) {
    cli.put(
        &format!("/tidb/cdc/default/default/changefeed/info/{name}"),
        &format!(r#"{{"state":"{status}", "start-ts": {start_ts}}}"#),
    )
    .await;
}

/// 写入完整的 info + status（含 checkpoint-ts）。
async fn put_changefeed(
    cli: &TestEtcdClient,
    name: &str,
    status: &str,
    start_ts: u64,
    checkpoint_ts: u64,
) {
    put_lame_changefeed(cli, name, status, start_ts).await;
    cli.put(
        &format!("/tidb/cdc/default/default/changefeed/status/{name}"),
        &format!(r#"{{"checkpoint-ts": {checkpoint_ts}}}"#),
    )
    .await;
}

/// 验证 `GetIncompatibleChangefeedsWithSafeTS` 在不同 safe-ts 下的筛选结果。
async fn testGetConflictChangefeeds(cli: &TestEtcdClient) {
    // 先放入 TiCDC 元数据与非 changefeed 噪声键，确保扫描会忽略它们。
    cli.put(
        "/tidb/cdc/default/__cdc_meta__/capture/3ecd5c98-0148-4086-adfd-17641995e71f",
        "",
    )
    .await;
    cli.put("/tidb/cdc/default/__cdc_meta__/meta/meta-version", "")
        .await;
    cli.put(
        "/tidb/cdc/default/__cdc_meta__/meta/ticdc-delete-etcd-key-count",
        "",
    )
    .await;
    cli.put("/tidb/cdc/default/__cdc_meta__/owner/22318498f4dd6639", "")
        .await;
    cli.put("/tidb/cdc/default/default/upstream/7168358383033671922", "")
        .await;

    put_changefeed(cli, "st-ok", "normal", 1, 43).await;
    put_changefeed(cli, "st-fail", "normal", 1, 41).await;
    put_lame_changefeed(cli, "skipped", "finished", 1).await;
    put_changefeed(cli, "not-skipped", "failed", 1, 41).await;
    put_lame_changefeed(cli, "nost-ok", "normal", 43).await;
    put_lame_changefeed(cli, "nost-fail", "normal", 41).await;

    let names = GetIncompatibleChangefeedsWithSafeTS(cli, 42).await.unwrap();
    assert_eq!(
        names.TESTGetChangefeedNames(),
        vec![
            "default/default/nost-fail",
            "default/default/not-skipped",
            "default/default/st-fail",
        ]
    );

    let names = GetIncompatibleChangefeedsWithSafeTS(cli, 40).await.unwrap();
    assert!(names.TESTGetChangefeedNames().is_empty());

    let names = GetIncompatibleChangefeedsWithSafeTS(cli, 48).await.unwrap();
    assert_eq!(
        names.TESTGetChangefeedNames(),
        vec![
            "default/default/nost-fail",
            "default/default/nost-ok",
            "default/default/not-skipped",
            "default/default/st-fail",
            "default/default/st-ok",
        ]
    );
}

/// 验证新旧键布局下 `GetRunningChangefeeds` 的枚举与忽略规则。
async fn testGetCDCChangefeedNameSet(cli: &TestEtcdClient) {
    let names = GetRunningChangefeeds(cli).await.unwrap();
    assert!(names.Empty());

    // TiCDC >= v6.2.
    // 新版布局：cluster/namespace 路径。
    cli.put(
        "/tidb/cdc/default/__cdc_meta__/capture/3ecd5c98-0148-4086-adfd-17641995e71f",
        "",
    )
    .await;
    cli.put("/tidb/cdc/default/__cdc_meta__/meta/meta-version", "")
        .await;
    cli.put(
        "/tidb/cdc/default/__cdc_meta__/meta/ticdc-delete-etcd-key-count",
        "",
    )
    .await;
    cli.put("/tidb/cdc/default/__cdc_meta__/owner/22318498f4dd6639", "")
        .await;
    cli.put(
        "/tidb/cdc/default/default/changefeed/info/test",
        r#"{"state":"normal"}"#,
    )
    .await;
    cli.put(
        "/tidb/cdc/default/default/changefeed/info/test-1",
        r#"{"state":"finished"}"#,
    )
    .await;
    cli.put("/tidb/cdc/default/default/changefeed/status/test-1", "")
        .await;
    cli.put(
        "/tidb/cdc/default/default/task/position/3ecd5c98-0148-4086-adfd-17641995e71f/test-1",
        "",
    )
    .await;
    cli.put("/tidb/cdc/default/default/upstream/7168358383033671922", "")
        .await;

    let names = GetRunningChangefeeds(cli).await.unwrap();
    assert!(!names.Empty());
    assert_eq!(names.TESTGetChangefeedNames(), vec!["default/default/test"]);

    cli.delete_prefix("/tidb/cdc/").await;

    // TiCDC <= v6.1.
    // 旧版扁平路径；命名空间展示为 `<nil>`。
    cli.put("/tidb/cdc/capture/f14cb04d-5ba1-410e-a59b-ccd796920e9d", "")
        .await;
    cli.put("/tidb/cdc/changefeed/info/test", r#"{"state":"stopped"}"#)
        .await;
    cli.put("/tidb/cdc/job/test", "").await;
    cli.put("/tidb/cdc/owner/223184ad80a88b0b", "").await;
    cli.put(
        "/tidb/cdc/task/position/f14cb04d-5ba1-410e-a59b-ccd796920e9d/test",
        "",
    )
    .await;

    let names = GetRunningChangefeeds(cli).await.unwrap();
    assert!(!names.Empty());
    assert_eq!(names.TESTGetChangefeedNames(), vec!["<nil>/test"]);

    cli.delete_prefix("/tidb/cdc/").await;

    // Ignore __backup__ changefeeds and keys containing only a cluster id.
    // 备份键与纯数字 cluster id 键应被忽略。
    cli.put(
        "/tidb/cdc/__backup__/changefeed/info/test",
        r#"{"state":"normal"}"#,
    )
    .await;
    cli.put(
        "/tidb/cdc/5402613591834624000/changefeed/info/test",
        r#"{"state":"normal"}"#,
    )
    .await;

    let names = GetRunningChangefeeds(cli).await.unwrap();
    assert!(names.Empty());
}

/// 总入口：依次跑名称集合与冲突筛选场景。
#[tokio::test]
async fn TestCDCCheckWithEmbedEtcd() {
    let cli = TestEtcdClient::default();

    testGetCDCChangefeedNameSet(&cli).await;
    cli.delete_prefix("").await;
    testGetConflictChangefeeds(&cli).await;
}
