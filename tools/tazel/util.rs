// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! BUILD write helpers and skip predicates (Go `util.go`).
//!
//! 这个模块只承载两类很小但高频的公共逻辑：
//! 一类是把 buildtools 生成的 AST 重写、格式化并原子地落回 BUILD 文件；
//! 另一类是维护少量与 Go 版一致的跳过名单，避免 `tazel` 对已知特殊路径误加补丁。
//! 这里故意把名单写死在函数内部，目的是继续保持与 Go `util.go` 的最小语义镜像，
//! 让调用方只关心“该路径是否应跳过”，而不需要理解额外的配置层。

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::stubs::build;

/// Go `write`.
/// 先执行 `Rewrite` 归一化 AST，再用 `Format` 输出文本，最后以 `0644` 权限覆盖目标文件。
/// 这个顺序要和 Go 版保持一致，避免仅因序列化细节不同而产生无意义的 BUILD diff。
pub fn write(path: &str, f: &mut build::File) -> io::Result<()> {
    build::Rewrite(f);
    let out = build::Format(f);

    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o644);

    let mut file = options.open(path)?;
    file.write_all(&out)
}

/// Go `skipFlaky`.
/// 标记已知不稳定的 BUILD 文件；命中后上层逻辑会跳过自动改写，避免把 flaky 用例纳入本轮处理。
pub fn skipFlaky(path: &str) -> bool {
    let mut pmap: HashSet<&str> = HashSet::new();
    pmap.insert("tests/realtikvtest/addindextest/BUILD.bazel");
    pmap.contains(path)
}

/// Go `skipTazel`.
/// 跳过 `tazel` 自身不应接管的 BUILD 文件，防止工具把生成规则反向补丁到其基础构建目录。
pub fn skipTazel(path: &str) -> bool {
    let mut pmap: HashSet<&str> = HashSet::new();
    pmap.insert("build/BUILD.bazel");
    pmap.contains(path)
}

/// Go `skipShardCount`.
/// 这些路径虽然仍会被扫描，但不会参与测试分片数统计。
/// 规则分成两段：`tests/readonlytest` 整体跳过，以及 `pkg/util` 默认跳过但保留若干白名单子目录，
/// 这样可以继续复用 Go 版对测试体量和目录特性的经验判断。
pub fn skipShardCount(path: &str) -> bool {
    path.starts_with("tests/readonlytest")
        || (path.starts_with("pkg/util")
            && !path.starts_with("pkg/util/admin")
            && !path.starts_with("pkg/util/chunk")
            && !path.starts_with("pkg/util/topsql")
            && !path.starts_with("pkg/util/stmtsummary")
            && !path.starts_with("pkg/util/workloadrepo"))
}
