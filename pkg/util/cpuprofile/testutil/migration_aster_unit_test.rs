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

// `cpuprofile/testutil` 迁移期单元测试。
//
// 校验 `mock_cpu_load` / `mock_cpu_load_v2` 生成的标签分组与十六进制取值对齐 Go。

use std::time::Duration;

use super::{CancellationToken, CpuLoad, mock_cpu_load, mock_cpu_load_v2};

/// 取出负载对象上的标签集合快照。
fn labels(load: &CpuLoad) -> Vec<Vec<(String, String)>> {
    load.label_sets().to_vec()
}

/// 单标签组、全标签组合并组，以及 hex(`"{label} value"`) 与 Go 一致。
#[test]
fn mock_cpu_load_matches_go_label_groups_and_hex_values() {
    let cancel = CancellationToken::new();
    let load = mock_cpu_load(&cancel, ["sql", "plan_digest"]);

    assert_eq!(
        labels(&load),
        vec![
            vec![("sql".into(), "73716c2076616c7565".into())],
            vec![(
                "plan_digest".into(),
                "706c616e5f6469676573742076616c7565".into(),
            )],
            vec![
                ("sql".into(), "73716c2076616c7565".into()),
                (
                    "plan_digest".into(),
                    "706c616e5f6469676573742076616c7565".into(),
                ),
            ],
        ]
    );

    cancel.cancel();
    assert!(load.join_timeout(Duration::from_secs(1)));
}

/// v2 使用 `sql_global_uid` 键，并同样保留“各值 + 全部组合”三组 worker。
#[test]
fn mock_cpu_load_v2_uses_sql_global_uid_for_each_and_combined_workers() {
    let cancel = CancellationToken::new();
    let load = mock_cpu_load_v2(&cancel, ["0_0", "2_1"]);

    assert_eq!(
        labels(&load),
        vec![
            vec![("sql_global_uid".into(), "0_0".into())],
            vec![("sql_global_uid".into(), "2_1".into())],
            vec![
                ("sql_global_uid".into(), "0_0".into()),
                ("sql_global_uid".into(), "2_1".into()),
            ],
        ]
    );

    cancel.cancel();
    assert!(load.join_timeout(Duration::from_secs(1)));
}

/// 空输入仍启动 Go 侧“全标签”worker，表现为一组空标签。
#[test]
fn empty_input_still_starts_the_go_all_labels_worker() {
    let cancel = CancellationToken::new();
    let load = mock_cpu_load(&cancel, std::iter::empty::<&str>());
    assert_eq!(labels(&load), vec![vec![]]);

    cancel.cancel();
    assert!(load.join_timeout(Duration::from_secs(1)));
}
