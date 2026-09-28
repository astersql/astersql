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

// 每日基准（bench daily）汇总流程的单元测试。
//
// 覆盖：缺少日期/输出路径时跳过扫描；递归发现 `bench_daily.json`、
// 合并为带 commit/日期的 `BenchOutput`，并跳过 `.git` 目录。

use super::{BenchOutput, BenchResult, read_bench_result_from_file};
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// 测试辅助 Result，错误统一为可发送的 trait 对象。
type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

/// 将多个输入 JSON 中的基准结果合并写入输出文件。
///
/// 输出为 `BenchOutput`（含 unix 日期、commit hash、结果列表），末尾追加换行。
fn combine_files(
    commit_hash: &str,
    date_in_unix: &str,
    input_files: &[PathBuf],
    output_file: &Path,
) -> TestResult {
    let mut result = Vec::with_capacity(100);
    for file in input_files {
        result.extend(read_bench_result_from_file(file)?);
    }

    let output = BenchOutput {
        date: date_in_unix.to_owned(),
        commit: commit_hash.to_owned(),
        result,
    };
    let mut writer = BufWriter::new(File::create(output_file)?);
    serde_json::to_writer(&mut writer, &output)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

/// 递归查找名为 `bench_daily.json` 的文件；跳过 `.git` 目录。
fn find_bench_results(root: &Path, found: &mut Vec<PathBuf>) -> TestResult {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if entry.file_name() != ".git" {
                find_bench_results(&path, found)?;
            }
        } else if entry.file_name() == "bench_daily.json" {
            found.push(path);
        }
    }
    Ok(())
}

/// 模拟每日汇总入口：日期或 outfile 缺失则直接返回；否则扫描、排序并合并。
fn run_bench_daily(
    date: &str,
    commit_hash: &str,
    outfile: Option<&Path>,
    search_root: &Path,
) -> TestResult {
    // 无日期或无输出路径时不做任何 IO，避免误扫目录。
    let Some(outfile) = outfile.filter(|_| !date.is_empty()) else {
        return Ok(());
    };

    let mut files = Vec::with_capacity(20);
    find_bench_results(search_root, &mut files)?;
    files.sort();
    combine_files(commit_hash, date, &files, outfile)
}

/// 构造仅含名称与 ns/op 的简化 `BenchResult`（alloc/bytes 置 0）。
fn result(name: &str, ns_per_op: i64) -> BenchResult {
    BenchResult {
        name: name.to_owned(),
        ns_per_op,
        allocs_per_op: 0,
        bytes_per_op: 0,
    }
}

/// 验证空日期或空 outfile 时不扫描、不写文件。
#[test]
fn test_bench_daily_skips_without_date_or_outfile() {
    let directory = tempfile::tempdir().unwrap();
    let missing_root = directory.path().join("not-scanned");

    run_bench_daily(
        "",
        "abc123",
        Some(&directory.path().join("out.json")),
        &missing_root,
    )
    .unwrap();
    run_bench_daily("1720742400", "abc123", None, &missing_root).unwrap();

    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

/// 验证合并多路径结果、路径排序，以及 `.git` 下假文件被忽略。
#[test]
fn test_bench_daily_combines_discovered_files_and_skips_git() {
    let directory = tempfile::tempdir().unwrap();
    let first_dir = directory.path().join("a");
    let second_dir = directory.path().join("b/nested");
    let git_dir = directory.path().join(".git/objects");
    fs::create_dir_all(&first_dir).unwrap();
    fs::create_dir_all(&second_dir).unwrap();
    fs::create_dir_all(&git_dir).unwrap();
    fs::write(
        first_dir.join("bench_daily.json"),
        serde_json::to_vec(&vec![result("BenchmarkA", 11)]).unwrap(),
    )
    .unwrap();
    fs::write(
        second_dir.join("bench_daily.json"),
        serde_json::to_vec(&vec![result("BenchmarkB", 22)]).unwrap(),
    )
    .unwrap();
    // `.git` 内故意写入非法 JSON；扫描应跳过该目录。
    fs::write(git_dir.join("bench_daily.json"), b"not json").unwrap();
    let outfile = directory.path().join("combined.json");

    run_bench_daily("1720742400", "abc123", Some(&outfile), directory.path()).unwrap();

    let output: BenchOutput = serde_json::from_slice(&fs::read(&outfile).unwrap()).unwrap();
    assert_eq!(output.date, "1720742400");
    assert_eq!(output.commit, "abc123");
    assert_eq!(
        output.result,
        vec![result("BenchmarkA", 11), result("BenchmarkB", 22)]
    );
    assert!(fs::read(&outfile).unwrap().ends_with(b"\n"));
}
