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

// `SHOW PLACEMENT LABELS` 结果构建逻辑的单元测试。
//
// 覆盖 Store 标签合并、去重、按键排序，以及对非法 JSON 形态的错误处理。

use crate::show_placement::{
    PlacementValue, StoreLabel, StoreLabelsJson, showPlacementLabelsResultBuilder,
};

#[test]
/// 验证空输入、跨 Store 标签合并、去重及与 Go 输出一致的排序。
fn placement_labels_builder_matches_go_cases() {
    let mut builder = showPlacementLabelsResultBuilder::default();

    assert!(builder.BuildRows().is_empty());

    for labels in [
        StoreLabelsJson::Array(vec![
            StoreLabel {
                key: "zone".into(),
                value: "z1".into(),
            },
            StoreLabel {
                key: "rack".into(),
                value: "r3".into(),
            },
            StoreLabel {
                key: "host".into(),
                value: "h1".into(),
            },
        ]),
        StoreLabelsJson::Array(vec![
            StoreLabel {
                key: "zone".into(),
                value: "z1".into(),
            },
            StoreLabel {
                key: "rack".into(),
                value: "r1".into(),
            },
            StoreLabel {
                key: "host".into(),
                value: "h2".into(),
            },
        ]),
        StoreLabelsJson::Array(vec![
            StoreLabel {
                key: "zone".into(),
                value: "z1".into(),
            },
            StoreLabel {
                key: "rack".into(),
                value: "r2".into(),
            },
            StoreLabel {
                key: "host".into(),
                value: "h2".into(),
            },
        ]),
        StoreLabelsJson::Array(vec![
            StoreLabel {
                key: "zone".into(),
                value: "z2".into(),
            },
            StoreLabel {
                key: "rack".into(),
                value: "r1".into(),
            },
            StoreLabel {
                key: "host".into(),
                value: "h2".into(),
            },
        ]),
        StoreLabelsJson::Null,
        StoreLabelsJson::Array(vec![StoreLabel {
            key: "k1".into(),
            value: "v1".into(),
        }]),
    ] {
        builder.AppendStoreLabels(labels).unwrap();
    }

    assert_eq!(
        builder.BuildRows(),
        vec![
            vec![
                PlacementValue::String("host".into()),
                PlacementValue::JsonStringArray(vec!["h1".into(), "h2".into()])
            ],
            vec![
                PlacementValue::String("k1".into()),
                PlacementValue::JsonStringArray(vec!["v1".into()])
            ],
            vec![
                PlacementValue::String("rack".into()),
                PlacementValue::JsonStringArray(vec!["r1".into(), "r2".into(), "r3".into()])
            ],
            vec![
                PlacementValue::String("zone".into()),
                PlacementValue::JsonStringArray(vec!["z1".into(), "z2".into()])
            ],
        ]
    );
}

#[test]
fn placement_labels_reject_non_array_non_null_json() {
    let mut builder = showPlacementLabelsResultBuilder::default();
    builder
        .AppendStoreLabels(StoreLabelsJson::Other)
        .expect_err("only array or null JSON should be accepted");
}
