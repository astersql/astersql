// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 对应 Go `TestIsReadOnlyVarInNextGen` 的运行时只读变量测试。
//
// Classic 内核下直接跳过；NextGen 下按用例表断言 `IsReadOnlyVarInNextGen`。

use astersql_sessionctx_vardef::*;

// Mirrors TestIsReadOnlyVarInNextGen. The task harness enables the nextgen
// feature, while retaining Go's build-mode guard for direct reuse.
#[test]
/// 镜像 Go 测试：在 NextGen 下校验只读变量集合与大小写变体。
fn test_is_read_only_var_in_next_gen() {
    // Classic 部署无 NextGen 只读约束，与 Go build-mode 守卫一致。
    if kerneltype::IsClassic() {
        return;
    }

    let cases = [
        ("abc", false),
        (TiDBEnableMDL, true),
        ("TIDB_ENABLE_METADATA_LOCK", true),
        (TiDBMaxDistTaskNodes, true),
        (TiDBDDLReorgMaxWriteSpeed, true),
        (TiDBDDLDiskQuota, true),
        (TiDBDDLEnableFastReorg, true),
        (TiDBEnableDistTask, true),
    ];

    for (name, expected) in cases {
        assert_eq!(
            IsReadOnlyVarInNextGen(name),
            expected,
            "unexpected next-gen read-only result for {name}"
        );
    }
}
