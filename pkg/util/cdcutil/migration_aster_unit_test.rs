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

// cdcutil 迁移回归测试（内存 KV，无网络）。
//
// 对齐 Go 行为：同时枚举新旧键版本并忽略噪声；按 checkpoint/start-ts 过滤；
// JSON 解析失败时错误信息携带 changefeed 上下文。

use std::collections::BTreeMap;

use async_trait::async_trait;

use super::cdc::{
    CdcError, GetIncompatibleChangefeedsWithSafeTS, GetOptions, GetRunningChangefeeds, KvClient,
    KvPair,
};

/// 同步内存 KV，实现 `KvClient` 供迁移测试使用。
#[derive(Default)]
struct MemoryKv {
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl MemoryKv {
    /// 链式写入一条键值后返回自身。
    fn with(mut self, key: &str, value: &str) -> Self {
        self.entries
            .insert(key.as_bytes().to_vec(), value.as_bytes().to_vec());
        self
    }
}

#[async_trait]
impl KvClient for MemoryKv {
    async fn get(&self, key: &str, options: GetOptions) -> Result<Vec<KvPair>, CdcError> {
        let mut result = Vec::new();
        for (entry_key, value) in &self.entries {
            let matches = if options.prefix {
                entry_key.starts_with(key.as_bytes())
            } else {
                entry_key.as_slice() == key.as_bytes()
            };
            if matches {
                result.push(KvPair {
                    key: entry_key.clone(),
                    value: if options.keys_only {
                        Vec::new()
                    } else {
                        value.clone()
                    },
                });
            }
        }
        Ok(result)
    }
}

/// 同时识别新版/旧版路径，并忽略 `__backup__` 与仅含 cluster 的键。
#[tokio::test]
async fn migration_enumerates_both_key_versions_and_ignores_noise() {
    let empty = GetRunningChangefeeds(&MemoryKv::default()).await.unwrap();
    assert!(empty.Empty());
    assert!(empty.changefeed_names().is_empty());

    let kv = MemoryKv::default()
        .with(
            "/tidb/cdc/default/default/changefeed/info/current",
            r#"{"state":"normal"}"#,
        )
        .with(
            "/tidb/cdc/default/default/changefeed/info/finished",
            r#"{"state":"finished"}"#,
        )
        .with("/tidb/cdc/changefeed/info/legacy", r#"{"state":"stopped"}"#)
        .with(
            "/tidb/cdc/__backup__/changefeed/info/ignored",
            r#"{"state":"normal"}"#,
        )
        .with(
            "/tidb/cdc/cluster-only/changefeed/info/ignored",
            r#"{"state":"normal"}"#,
        )
        .with("/tidb/cdc/default/default/upstream/42", "");

    let names = GetRunningChangefeeds(&kv).await.unwrap();

    assert_eq!(
        names.changefeed_names(),
        vec!["<nil>/legacy", "default/default/current"]
    );
    assert!(!names.Empty());
    assert!(names.MessageToUser().contains("found CDC changefeed(s): "));
}

/// 有效 checkpoint（status 与 start-ts 取 max）小于 safe-ts 者入选。
#[tokio::test]
async fn migration_filters_with_checkpoint_and_start_ts_like_go() {
    let kv = MemoryKv::default()
        .with(
            "/tidb/cdc/default/default/changefeed/info/st-fail",
            r#"{"state":"normal","start-ts":1}"#,
        )
        .with(
            "/tidb/cdc/default/default/changefeed/status/st-fail",
            r#"{"checkpoint-ts":41}"#,
        )
        .with(
            "/tidb/cdc/default/default/changefeed/info/st-ok",
            r#"{"state":"normal","start-ts":1}"#,
        )
        .with(
            "/tidb/cdc/default/default/changefeed/status/st-ok",
            r#"{"checkpoint-ts":43}"#,
        )
        .with(
            "/tidb/cdc/default/default/changefeed/info/not-skipped",
            r#"{"state":"failed","start-ts":1}"#,
        )
        .with(
            "/tidb/cdc/default/default/changefeed/status/not-skipped",
            r#"{"checkpoint-ts":41}"#,
        )
        .with(
            "/tidb/cdc/default/default/changefeed/info/skipped",
            r#"{"state":"finished","start-ts":1}"#,
        )
        .with(
            "/tidb/cdc/default/default/changefeed/info/nost-fail",
            r#"{"state":"normal","start-ts":41}"#,
        )
        .with(
            "/tidb/cdc/default/default/changefeed/info/nost-ok",
            r#"{"state":"normal","start-ts":43}"#,
        )
        .with(
            "/tidb/cdc/default/default/changefeed/info/unknown",
            r#"{"state":"mystery","start-ts":1}"#,
        );

    let names = GetIncompatibleChangefeedsWithSafeTS(&kv, 42).await.unwrap();

    assert_eq!(
        names.changefeed_names(),
        vec![
            "default/default/nost-fail",
            "default/default/not-skipped",
            "default/default/st-fail",
        ]
    );

    let none = GetIncompatibleChangefeedsWithSafeTS(&kv, 40).await.unwrap();
    assert!(none.changefeed_names().is_empty());

    let all_active = GetIncompatibleChangefeedsWithSafeTS(&kv, 48).await.unwrap();
    assert_eq!(
        all_active.changefeed_names(),
        vec![
            "default/default/nost-fail",
            "default/default/nost-ok",
            "default/default/not-skipped",
            "default/default/st-fail",
            "default/default/st-ok",
        ]
    );
}

/// JSON 损坏时应包装为带 changefeed 名的 `CheckChangefeed` 错误。
#[tokio::test]
async fn migration_propagates_json_errors_with_changefeed_context() {
    let kv = MemoryKv::default().with(
        "/tidb/cdc/default/default/changefeed/info/broken",
        "not-json",
    );

    let error = GetRunningChangefeeds(&kv).await.unwrap_err().to_string();

    assert!(error.contains("failed to check changefeed"));
    assert!(error.contains("broken"));
}
