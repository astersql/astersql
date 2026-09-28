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

// `IsSystemSchema` 单测。
//
// 覆盖常见 MySQL/TiDB 系统库、metrics/巡检库，以及大小写折叠后的判定结果。

use super::IsSystemSchema;

/// 用大小写混合用例验证系统 schema 判定与 Go 测试一致。
#[test]
fn TestIsSystemSchema() {
    // (原始名字, 期望是否为系统库)；实际调用前会转成小写。
    let cases = vec![
        ("information_schema", true),
        ("performance_schema", true),
        ("mysql", true),
        ("sys", true),
        ("INFORMATION_SCHEMA", true),
        ("PERFORMANCE_SCHEMA", true),
        ("MYSQL", true),
        ("SYS", true),
        ("not_system_schema", false),
        ("METRICS_SCHEMA", true),
        ("INSPECTION_SCHEMA", true),
    ];

    for (name, expected) in cases {
        // Go 测试通过 ast.NewCIStr(tt.name).L 转成小写；这里保留大小写折叠后的调用语义。
        let ci_name = name.to_ascii_lowercase();
        assert_eq!(expected, IsSystemSchema(&ci_name), "schema name = {}", name);
    }
}
