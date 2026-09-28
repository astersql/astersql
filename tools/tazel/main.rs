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

//! `tazel` 的入口模块。
//!
//! 该工具会扫描仓库中的 `BUILD.bazel`，只针对首个 `go_test` 规则补齐
//! `timeout`、`flaky` 与 `shard_count` 等属性，使 Rust 版本与 Go 版本的
//! 构建文件修补流程保持一致。
//!
//! 执行顺序也刻意贴近 Go：
//! 1. 先初始化测试计数并预扫源码，建立目录到测试数量的映射；
//! 2. 再确认当前工作目录确实是仓库根，避免误改其它目录中的 BUILD 文件；
//! 3. 最后递归遍历文件系统，把补丁写回每个允许处理的 `BUILD.bazel`。
//!
//! 入口层本身不负责解析测试 AST 或输出 BUILD 文本，这些职责分别委托给
//! `ast`、`stubs::build` 与 `util` 模块，从而保持主流程只描述“何时处理、
//! 处理哪些文件、以及遇到哪些跳过条件”。
//! tazel entry: patch BUILD.bazel go_test attrs (Go `main.go`).

use std::cmp::min;
use std::fs;
use std::io;
use std::path::Path;

use crate::ast::{initCount, test_count_for, walk_from};
use crate::stubs::build;
use crate::util::{skipFlaky, skipShardCount, skipTazel, write};

/// `shard_count` 的上限。
///
/// 这里沿用 Go 常量，避免单个 `go_test` 因测试数量过大而被切成过多分片，
/// 从而让生成结果和原工具的资源预期保持一致。
pub const maxShardCount: u32 = 50;

/// Process entry matching Go `main` (expects CWD = project root).
/// 公开入口只固定根目录为当前目录，把真正可测试的逻辑放到 `run_from`。
pub fn main() {
    run_from(Path::new(".")).unwrap_or_else(|err| panic!("{err}"));
}

/// Injectable root for tests / Go `main` body.
///
/// 这里先做 `initCount` 与 `walk_from`，再校验 `WORKSPACE` 是否存在。
/// 这样的顺序不是偶然的：它保持与 Go 入口一致，确保后续根据目录测试数
/// 决定 `shard_count` 时，`testMap` 已经准备完毕。
pub fn run_from(root: &Path) -> Result<(), String> {
    initCount();
    walk_from(root);

    let workspace = root.join("WORKSPACE");
    if !workspace.exists() {
        return Err("It should run from the project root".into());
    }

    walk_build_files(root).map_err(|e| format!("fail to filepath.Walk: {e}"))
}

/// Apply go_test attr patches to one BUILD file (shared by walk + tests).
///
/// `rel_path` is used for skip* predicates (repo-relative). `abs_build` is the
/// on-disk BUILD path whose parent directory keys into `testMap`.
///
/// 该函数只修改规则属性，不负责落盘；这样遍历逻辑与单元测试都可以复用同一套
/// 补丁行为。实现上只处理首个 `go_test`，因为这里要追随 Go 工具的既有假设，
/// 而不是借机扩展语义。
pub fn patch_go_test_file(
    rel_path: &str,
    abs_build: &Path,
    data: Vec<u8>,
) -> Result<build::File, String> {
    let mut buildfile = build::ParseBuild("BUILD.bazel", data)?;
    {
        let mut gotest = buildfile.Rules("go_test");
        if !gotest.is_empty() {
            let rule = &mut *gotest[0];
            // 缺失 `timeout` 时补成 `short`，保证生成结果和 Go 版本一致。
            if rule.AttrString("timeout").is_empty() {
                rule.SetAttr(
                    "timeout",
                    build::StringExpr {
                        Value: "short".into(),
                    },
                );
            }
            // 只有未命中跳过名单时才补 `flaky = True`，避免覆盖特例目录的人工配置。
            if !skipFlaky(rel_path) && rule.AttrLiteral("flaky").is_empty() {
                rule.SetAttr(
                    "flaky",
                    build::LiteralExpr {
                        Token: "True".into(),
                    },
                );
            }
            // `shard_count` 依赖预扫阶段统计出的目录测试数，因此这里必须拿绝对路径
            // 的父目录作为键，与 Go 版本使用 `filepath.Abs + Dir` 的做法对齐。
            if !skipShardCount(rel_path) {
                let abs = fs::canonicalize(abs_build).map_err(|e| e.to_string())?;
                let dir = abs
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                if let Some(cnt) = test_count_for(&dir) {
                    if cnt > 1 {
                        // 多于一个测试时才设置分片；同时施加统一上限，防止异常膨胀。
                        rule.SetAttr(
                            "shard_count",
                            build::LiteralExpr {
                                Token: min(cnt, maxShardCount).to_string(),
                            },
                        );
                    } else {
                        // 只有一个测试时显式删除旧值，避免残留的历史配置改变执行方式。
                        rule.DelAttr("shard_count");
                    }
                }
            }
        }
    }
    Ok(buildfile)
}

/// 深度优先遍历目录并原地改写符合条件的 `BUILD.bazel`。
///
/// 与 Go 的 `filepath.Walk` 相比，这里手动递归目录，但保留了相同的过滤顺序：
/// 目录直接下钻，非 `BUILD.bazel` 文件跳过，命中 `skipTazel` 的路径也跳过。
fn walk_build_files(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        let path_string = walk_path_string(root, &path);
        let name = entry.file_name().to_string_lossy().to_string();

        if metadata.is_dir() {
            walk_build_files(&path)?;
            continue;
        }

        if name != "BUILD.bazel" || skipTazel(&path_string) {
            continue;
        }

        // 读取、解析、写回都把仓库相对路径带进错误信息，便于和 Go 日志定位对齐。
        let data = fs::read(&path).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("fail to read file, path: {path_string}, err: {err}"),
            )
        })?;
        let mut buildfile = patch_go_test_file(&path_string, &path, data).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("fail to parser BUILD.bazel, path: {path_string}, err: {err}"),
            )
        })?;
        write(&path.to_string_lossy(), &mut buildfile)?;
    }
    Ok(())
}

/// 生成用于跳过规则和报错信息的稳定相对路径字符串。
///
/// 当 `path` 能相对 `root` 计算时，优先返回仓库内路径；否则退回原路径。
/// 同时裁剪掉可能出现的 `./` 前缀，让 Rust 版本与 Go 版本看到的路径格式一致，
/// 这样 `skipTazel`、`skipFlaky` 和 `skipShardCount` 的判断不会因前缀不同而失配。
fn walk_path_string(root: &Path, path: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(root) {
        let raw = rel.to_string_lossy();
        return raw.strip_prefix("./").unwrap_or(&raw).to_string();
    }
    let raw = path.to_string_lossy();
    raw.strip_prefix("./").unwrap_or(&raw).to_string()
}
