// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// `generateWildcardPath` 通配路径生成的单元测试。
//
// 覆盖空列表、单文件、Mydumper 命名、前后缀回退、子目录组件内通配，
// 以及冲突文件导致无法生成唯一模式等场景。

use astersql_lightning_mydump as mydump;
use std::collections::HashMap;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn common_prefix_and_suffix_match_go_cases() {
    assert_eq!(
        "s3://bucket/foo/bar/baz",
        crate::pattern::longestCommonPrefix(&strings(&[
            "s3://bucket/foo/bar/baz1",
            "s3://bucket/foo/bar/baz2",
            "s3://bucket/foo/bar/baz",
        ]))
    );
    assert_eq!(
        "",
        crate::pattern::longestCommonPrefix(&strings(&["a", "b"]))
    );
    assert_eq!("", crate::pattern::longestCommonPrefix(&[]));

    assert_eq!(
        "XYZ",
        crate::pattern::longestCommonSuffix(&strings(&["abcXYZ", "defXYZ", "XYZ"]), 0)
    );
    assert_eq!(
        "",
        crate::pattern::longestCommonSuffix(&strings(&["a", "b"]), 0)
    );
    assert_eq!("", crate::pattern::longestCommonSuffix(&[], 0));
    assert_eq!(
        "",
        crate::pattern::longestCommonSuffix(&strings(&["abc", "abc"]), 3)
    );
    assert_eq!(
        "f",
        crate::pattern::longestCommonSuffix(&strings(&["abcdf", "abcef"]), 3)
    );
}

#[test]
fn prefix_suffix_pattern_matches_go_cases() {
    assert_eq!(
        "pre_m*_suf",
        crate::pattern::generatePrefixSuffixPattern(&strings(&["pre_middle_suf", "pre_most_suf"]))
    );
    assert_eq!("", crate::pattern::generatePrefixSuffixPattern(&[]));
    assert_eq!(
        "pre_middle_suf",
        crate::pattern::generatePrefixSuffixPattern(&strings(&["pre_middle_suf"]))
    );
    assert_eq!(
        "*",
        crate::pattern::generatePrefixSuffixPattern(&strings(&["foo", "bar"]))
    );
    assert_eq!(
        "aaa*",
        crate::pattern::generatePrefixSuffixPattern(&strings(&["aaabaaa", "aaa"]))
    );
}

#[test]
fn pattern_validation_matches_go_cases() {
    let table_files = ["a.txt".to_owned(), "b.txt".to_owned()]
        .into_iter()
        .collect();
    let small_all = all_files(&["a.txt", "b.txt"]);
    assert!(crate::pattern::isValidPattern(
        "*.txt",
        &table_files,
        &small_all
    ));
    let full_all = all_files(&["a.txt", "b.txt", "c.txt"]);
    assert!(!crate::pattern::isValidPattern(
        "*.txt",
        &table_files,
        &full_all
    ));
    assert!(!crate::pattern::isValidPattern(
        "*.csv",
        &table_files,
        &small_all
    ));
    assert!(!crate::pattern::isValidPattern(
        "",
        &table_files,
        &small_all
    ));
}

/// 构造指定路径与压缩类型的 `FileInfo` 测试桩。
fn file_at(path: &str, compression: mydump::Compression) -> mydump::FileInfo {
    mydump::FileInfo {
        file_meta: mydump::FileMeta {
            path: path.to_owned(),
            compression,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// 将路径列表转为「全局文件集合」HashMap（无压缩）。
fn all_files(paths: &[&str]) -> HashMap<String, mydump::FileInfo> {
    paths
        .iter()
        .map(|path| ((*path).to_owned(), file_at(path, mydump::Compression::None)))
        .collect()
}

/// Mirrors Go's `TestGenerateWildcardPath` "No files" case: an empty table
/// must fail with the no-data-files sentinel message.
/// 空文件列表应返回「无数据文件」哨兵错误。
#[test]
fn generate_wildcard_path_rejects_empty_file_list() {
    let error = crate::pattern::generateWildcardPath(&[], &HashMap::new(), "db", "tb").unwrap_err();
    assert!(error.to_string().contains("no data files for table"));
}

/// Mirrors Go's `TestGenerateWildcardPath` "Single file" case: a lone data
/// file is returned unchanged, without introducing a wildcard.
/// 单文件直接返回原路径，不引入通配符。
#[test]
fn generate_wildcard_path_returns_single_file_unchanged() {
    let files = vec![file_at("db.tb.0001.sql", mydump::Compression::None)];
    let all = all_files(&["db.tb.0001.sql"]);
    assert_eq!(
        "db.tb.0001.sql",
        crate::pattern::generateWildcardPath(&files, &all, "db", "tb").unwrap()
    );
}

/// Mirrors Go's `TestGenerateWildcardPath` "Mydumper pattern succeeds" case:
/// the Mydumper `db.table.NNNN.ext[.compression]` naming convention produces a
/// pattern that matches only this table's files, including the compressed
/// (`.gz`) suffix.
/// Mydumper 命名应生成 `db.tb.*.sql.gz` 且保留压缩后缀。
#[test]
fn generate_wildcard_path_prefers_mydumper_pattern_when_it_is_specific() {
    let files = vec![
        file_at("db.tb.0001.sql.gz", mydump::Compression::Gz),
        file_at("db.tb.0002.sql.gz", mydump::Compression::Gz),
    ];
    let all = all_files(&["db.tb.0001.sql.gz", "db.tb.0002.sql.gz"]);
    assert_eq!(
        "db.tb.*.sql.gz",
        crate::pattern::generateWildcardPath(&files, &all, "db", "tb").unwrap()
    );
}

#[test]
fn generate_wildcard_path_supports_aurora_partition_directories() {
    let files = vec![
        file_at(
            "export-1/db/db.users/1/part-a.parquet",
            mydump::Compression::None,
        ),
        file_at(
            "export-1/db/db.users/2/part-b.parquet",
            mydump::Compression::None,
        ),
    ];
    let all = all_files(&[
        "export-1/db/db.users/1/part-a.parquet",
        "export-1/db/db.users/2/part-b.parquet",
        "export-1/db/db.orders/1/part-a.parquet",
        "export-1/db2/db2.users/1/part-a.parquet",
    ]);
    assert_eq!(
        "export-1/db/db.users/*/part-*.parquet",
        crate::pattern::generateWildcardPath(&files, &all, "db", "users").unwrap()
    );
}

/// Mirrors Go's `TestGenerateWildcardPath` "Mydumper pattern fails, fallback to
/// prefix/suffix succeeds" case: non-Mydumper filenames fall back to the
/// generic prefix/suffix pattern, and adding a conflicting file that the
/// fallback pattern would also match turns the previously valid pattern into
/// the "cannot generate a unique wildcard pattern" error.
/// 非 Mydumper 名回退到 `*.sql`；再插入冲突文件则应报无法生成唯一模式。
#[test]
fn generate_wildcard_path_falls_back_to_prefix_suffix_pattern() {
    let files = vec![
        file_at("a.sql", mydump::Compression::None),
        file_at("b.sql", mydump::Compression::None),
    ];
    let mut all = all_files(&["a.sql", "b.sql"]);
    assert_eq!(
        "*.sql",
        crate::pattern::generateWildcardPath(&files, &all, "db", "tb").unwrap()
    );

    // 插入会被 `*.sql` 一并匹配的冲突文件，使模式失去特异性。
    all.insert(
        "db-schema.sql".to_owned(),
        file_at("db-schema.sql", mydump::Compression::None),
    );
    let error = crate::pattern::generateWildcardPath(&files, &all, "db", "tb").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot generate a unique wildcard pattern")
    );
}

/// Mirrors Go's `TestGenerateWildcardPath` subdirectory case: when all paths
/// have the same number of `/`-separated components, the fallback pattern is
/// generated component-by-component so a `*` never crosses a `/` boundary.
/// 路径组件数相同时，通配符应限制在各段内（如 `dir/subdir*/*.csv`）。
#[test]
fn generate_wildcard_path_keeps_wildcards_within_path_components() {
    let files = vec![
        file_at("dir/subdir1/a.csv", mydump::Compression::None),
        file_at("dir/subdir1/b.csv", mydump::Compression::None),
        file_at("dir/subdir2/c.csv", mydump::Compression::None),
        file_at("dir/subdir2/d.csv", mydump::Compression::None),
    ];
    let all = all_files(&[
        "dir/subdir1/a.csv",
        "dir/subdir1/b.csv",
        "dir/subdir2/c.csv",
        "dir/subdir2/d.csv",
    ]);
    assert_eq!(
        "dir/subdir*/*.csv",
        crate::pattern::generateWildcardPath(&files, &all, "db", "tb").unwrap()
    );
}

/// Go takes the routed table name from `FileInfo.TableName`, not from the
/// physical filename. Rust receives that routed name from `MDTableMeta`.
#[test]
fn generate_mydumper_pattern_uses_routed_table_name() {
    let file = file_at("incoming/part-0001.csv", mydump::Compression::None);
    assert_eq!(
        "incoming/db.tb.*.csv",
        crate::pattern::generateMydumperPattern(&file, "db", "tb")
    );
}

/// Go delegates validation to `filepath.Match`, so bracket expressions in a
/// generated pattern retain their glob meaning instead of being treated as
/// literal filename bytes.
#[test]
fn generated_pattern_uses_go_filepath_character_class_semantics() {
    let files = vec![
        file_at("a[1].sql", mydump::Compression::None),
        file_at("b[1].sql", mydump::Compression::None),
    ];
    let all = all_files(&["a[1].sql", "b[1].sql"]);

    let error = crate::pattern::generateWildcardPath(&files, &all, "db", "tb").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot generate a unique wildcard pattern")
    );
}
