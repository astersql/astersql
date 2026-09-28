// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// taskkey 迁移对齐单元测试：校验 NextGen / classic 两种内核下 task key 格式。
//
// NextGen 下 key 带 keyspace 前缀；classic 忽略 keyspace 参数，仅保留 `ImportInto/{jobID}`。
// 负 job ID 与含 `/` 的 keyspace 名须按 Go 字符串拼接语义原样保留。

use astersql_dxf_importinto_taskkey::{config, kerneltype, taskkey};

/// 覆盖：NextGen 带配置/显式 keyspace；classic 固定旧格式且忽略传入 keyspace。
#[test]
fn task_keys_match_the_active_go_kernel_mode() {
    if kerneltype::IsNextGen() {
        // NextGen：ForJob 使用全局配置的 keyspace；ForJobInKeyspace 使用显式参数。
        config::set_global_keyspace_name("configured-ks");
        assert_eq!(taskkey::ForJob(42), "configured-ks/ImportInto/42");
        assert_eq!(
            taskkey::ForJobInKeyspace("explicit-ks".to_owned(), -7),
            "explicit-ks/ImportInto/-7"
        );
    } else {
        // classic：不带 keyspace；负 job ID 原样出现在路径末段。
        assert_eq!(taskkey::ForJob(42), "ImportInto/42");
        assert_eq!(
            taskkey::ForJobInKeyspace("tenant-a".to_owned(), -7),
            "ImportInto/-7"
        );
    }
}

/// 覆盖：keyspace 含 `/` 与 `i64::MIN` 的十进制渲染，与 Go 格式化一致。
#[test]
fn task_key_format_preserves_go_string_and_integer_rendering() {
    assert_eq!(
        taskkey::ForJobInKeyspace("tenant/child".to_owned(), i64::MIN),
        if kerneltype::IsNextGen() {
            "tenant/child/ImportInto/-9223372036854775808"
        } else {
            "ImportInto/-9223372036854775808"
        }
    );
}
