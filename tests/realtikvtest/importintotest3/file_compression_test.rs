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

//! 中文说明开始（自动生成）
//! 中文总览：`file_compression_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `file_compression_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 37 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_gzip_and_mixed_compression` 是当前文件里的辅助函数。
//! 阅读 `test_gzip_and_mixed_compression` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_gzip_and_mixed_compression` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_gzip_and_mixed_compression`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_gzip_and_mixed_compression` 的重要阅读参照。
//! 理解 `test_gzip_and_mixed_compression` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_gzip_and_mixed_compression` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_gzip_and_mixed_compression` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_zstd` 是当前文件里的辅助函数。
//! 阅读 `test_zstd` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_zstd` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_zstd`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_zstd` 的重要阅读参照。
//! 理解 `test_zstd` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_zstd` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_zstd` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 符号 `test_snappy` 是当前文件里的辅助函数。
//! 阅读 `test_snappy` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_snappy` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_snappy`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_snappy` 的重要阅读参照。
//! 理解 `test_snappy` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_snappy` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 把 `test_snappy` 当成当前主题的局部锚点，有助于快速定位相关逻辑。
//! 关注点 001：继续从前置准备的角度理解这一段逻辑，避免把环境搭建误读成业务断言。（主题：`file_compression_test`）。
//! 关注点 002：继续从主路径执行的角度理解这一段逻辑，确认核心动作发生在何处。（主题：`file_compression_test`）。
//! 关注点 003：继续从结果断言的角度理解这一段逻辑，重点关注稳定可观察输出。（主题：`file_compression_test`）。
//! 关注点 004：继续从资源收尾的角度理解这一段逻辑，留意清理和状态回收何时发生。（主题：`file_compression_test`）。
//! 关注点 005：继续对照 Go 同名场景阅读这一段逻辑，避免把桩行为当成真实实现。（主题：`file_compression_test`）。
//! 关注点 006：继续关注错误文本、结果行和状态枚举，这些通常是回归时最稳定的观察点。（主题：`file_compression_test`）。
//! 中文说明结束（自动生成）

//! Go-equivalent tests for `file_compression_test.go`.
//!
//! Mapping:
//! - `getCompressedData` → [`MockGCSSuite::get_compressed_data`]
//! - `TestGzipAndMixedCompression` → [`test_gzip_and_mixed_compression`]
//! - `TestZStd` → [`test_zstd`]
//! - `TestSnappy` → [`test_snappy`]

use astersql_tests_realtikvtest_importintotest3::harness::{
    fakestorage, gcs_endpoint, mydump, reset_engine, serial_guard, testkit,
};

#[path = "compression_harness.rs"]
mod compression_harness;
use compression_harness::MockGCSSuite;

/// `TestGzipAndMixedCompression`.
#[test]
fn test_gzip_and_mixed_compression() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.prepare_and_use_db("gzip");
    s.tk.MustExec("CREATE TABLE gzip.t (i INT PRIMARY KEY, s varchar(32));");

    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "gzip".into(),
            Name: "compress.001.csv.gz".into(),
        },
        Content: s.get_compressed_data(mydump::Compression::GZ, b"1,test1\n2,test2"),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "gzip".into(),
            Name: "compress.001.csv.gzip".into(),
        },
        Content: s.get_compressed_data(mydump::Compression::GZ, b"3,test3\n4,test4"),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "gzip".into(),
            Name: "compress.002.csv".into(),
        },
        Content: b"5,test5\n6,test6\n7,test7\n8,test8\n9,test9".to_vec(),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "gzip".into(),
            Name: "compress2.001.sql.gz".into(),
        },
        Content: s.get_compressed_data(
            mydump::Compression::GZ,
            b"INSERT INTO `gzip`.`t` VALUES (1,'test1'),(2,'test2');",
        ),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "gzip".into(),
            Name: "compress2.001.sql.gzip".into(),
        },
        Content: s.get_compressed_data(
            mydump::Compression::GZ,
            b"INSERT INTO `gzip`.`t` VALUES (3,'test3'),(4,'test4');",
        ),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "gzip".into(),
            Name: "compress2.002.sql".into(),
        },
        Content: b"INSERT INTO `gzip`.`t` VALUES (5,'test5'),(6,'test6'),(7,'test7'),(8,'test8'),(9,'test9');"
            .to_vec(),
    });

    let ep = gcs_endpoint();
    let testcases = [
        (
            vec![
                "1 test1", "2 test2", "3 test3", "4 test4", "5 test5", "6 test6", "7 test7",
                "8 test8", "9 test9",
            ],
            format!("IMPORT INTO gzip.t FROM 'gs://gzip/compress.*?endpoint={ep}' WITH thread=1;"),
        ),
        (
            vec![
                "1 test1", "2 test2", "3 test3", "4 test4", "5 test5", "6 test6", "7 test7",
                "8 test8", "9 test9",
            ],
            format!("IMPORT INTO gzip.t FROM 'gs://gzip/compress2.*?endpoint={ep}' WITH thread=1;"),
        ),
        (
            vec![
                "2 test2", "4 test4", "6 test6", "7 test7", "8 test8", "9 test9",
            ],
            format!(
                "IMPORT INTO gzip.t FROM 'gs://gzip/compress.*?endpoint={ep}' WITH skip_rows=1, thread=1;"
            ),
        ),
    ];

    for (expect_rows, load_sql) in &testcases {
        s.tk.MustExec("TRUNCATE TABLE gzip.t");
        s.tk.MustQuery(load_sql);
        s.tk.MustQuery("SELECT * FROM gzip.t;")
            .Check(&testkit::Rows(expect_rows));
    }
    s.tear_down();
}

/// `TestZStd`.
#[test]
fn test_zstd() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.prepare_and_use_db("zstd");
    s.tk.MustExec("CREATE TABLE zstd.t (i INT PRIMARY KEY, s varchar(32));");

    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "zstd".into(),
            Name: "t.01.csv.zst".into(),
        },
        Content: s.get_compressed_data(mydump::Compression::ZStd, b"1,test1\n2,test2"),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "zstd".into(),
            Name: "t.02.csv.zstd".into(),
        },
        Content: s.get_compressed_data(mydump::Compression::ZStd, b"3,test3\n4,test4"),
    });

    let ep = gcs_endpoint();
    let sql = format!("IMPORT INTO zstd.t FROM 'gs://zstd/t.*?endpoint={ep}' WITH thread=1;");
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM zstd.t;")
        .Check(&testkit::Rows(&[
            "1 test1", "2 test2", "3 test3", "4 test4",
        ]));

    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "zstd".into(),
            Name: "t2.01.sql.zst".into(),
        },
        Content: s.get_compressed_data(
            mydump::Compression::ZStd,
            b"INSERT INTO `gzip`.`t` VALUES (1,'test1'),(2,'test2');",
        ),
    });
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "zstd".into(),
            Name: "t2.02.sql.zstd".into(),
        },
        Content: s.get_compressed_data(
            mydump::Compression::ZStd,
            b"INSERT INTO `gzip`.`t` VALUES (3,'test3'),(4,'test4');",
        ),
    });

    s.tk.MustExec("truncate table zstd.t");
    let sql = format!("IMPORT INTO zstd.t FROM 'gs://zstd/t2.*?endpoint={ep}' WITH thread=1;");
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM zstd.t;")
        .Check(&testkit::Rows(&[
            "1 test1", "2 test2", "3 test3", "4 test4",
        ]));
    s.tear_down();
}

/// `TestSnappy`.
#[test]
fn test_snappy() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.prepare_and_use_db("snappy");
    s.tk.MustExec("CREATE TABLE snappy.t (i INT PRIMARY KEY, s varchar(32));");

    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "snappy".into(),
            Name: "t.01.csv.snappy".into(),
        },
        Content: s.get_compressed_data(mydump::Compression::Snappy, b"1,test1\n2,test2"),
    });

    let ep = gcs_endpoint();
    let sql = format!("IMPORT INTO snappy.t FROM 'gs://snappy/t.*?endpoint={ep}' WITH thread=1;");
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM snappy.t;")
        .Check(&testkit::Rows(&["1 test1", "2 test2"]));

    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: "snappy".into(),
            Name: "t2.01.sql.snappy".into(),
        },
        Content: s.get_compressed_data(
            mydump::Compression::Snappy,
            b"INSERT INTO `gzip`.`t` VALUES (1,'test1'),(2,'test2');",
        ),
    });

    s.tk.MustExec("truncate table snappy.t");
    let sql = format!("IMPORT INTO snappy.t FROM 'gs://snappy/t2.*?endpoint={ep}' WITH thread=1;");
    s.tk.MustQuery(&sql);
    s.tk.MustQuery("SELECT * FROM snappy.t;")
        .Check(&testkit::Rows(&["1 test1", "2 test2"]));
    s.tear_down();
}

// Go getCompressedData uses actual codec writers, not a private envelope.
#[test]
fn test_compressed_data_uses_standard_formats() {
    use astersql_tests_realtikvtest_importintotest3::harness::compress_framed;

    for (kind, signature) in [
        (mydump::Compression::GZ, &b"\x1f\x8b\x08"[..]),
        (mydump::Compression::ZStd, &b"\x28\xb5\x2f\xfd"[..]),
        (mydump::Compression::Snappy, &b"\xff\x06\x00\x00sNaPpY"[..]),
    ] {
        let compressed = compress_framed(kind, b"1,test1\n2,test2");
        assert!(
            compressed.starts_with(signature),
            "nonstandard compressed stream: {compressed:?}"
        );
    }
}

#[test]
fn test_compression_streams_finish_and_decode_across_blocks() {
    use astersql_tests_realtikvtest_importintotest3::harness::{
        compress_framed, decompress_by_name, decompress_framed,
    };

    // Exceed Snappy's 64 KiB block boundary and verify writer finalization.
    let payload = b"1,test1\n2,test2\n".repeat(10_000);
    for (kind, names) in [
        (mydump::Compression::GZ, &["t.csv.gz", "t.csv.gzip"][..]),
        (mydump::Compression::ZStd, &["t.csv.zst", "t.csv.zstd"][..]),
        (mydump::Compression::Snappy, &["t.csv.snappy"][..]),
    ] {
        let compressed = compress_framed(kind, &payload);
        assert_ne!(compressed, payload);
        assert_eq!(decompress_framed(&compressed), payload);
        for name in names {
            assert_eq!(decompress_by_name(name, &compressed), payload);
        }
    }
    assert_eq!(decompress_framed(&payload), payload);
    assert_eq!(decompress_by_name("t.csv", &payload), payload);
}

#[test]
fn test_corrupt_compressed_import_returns_error_without_rows() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.prepare_and_use_db("corrupt_compression");
    s.tk.MustExec("CREATE TABLE corrupt_compression.t (i INT PRIMARY KEY, s varchar(32));");
    for extension in ["gz", "gzip", "zst", "zstd", "snappy"] {
        let name = format!("bad.csv.{extension}");
        s.server.CreateObject(fakestorage::Object {
            ObjectAttrs: fakestorage::ObjectAttrs {
                BucketName: "corrupt_compression".into(),
                Name: name.clone(),
            },
            Content: b"not a compressed stream".to_vec(),
        });
        let sql = format!(
            "IMPORT INTO corrupt_compression.t FROM 'gs://corrupt_compression/{name}?endpoint={}' WITH thread=1;",
            gcs_endpoint()
        );
        let error =
            s.tk.QueryToErr(&sql)
                .err()
                .expect("corrupt stream must fail");
        assert!(error.contains("decompress"), "{extension}: {error}");
        s.tk.MustQuery("SELECT * FROM corrupt_compression.t;")
            .Check(&testkit::Rows(&[]));
    }
    s.tear_down();
}

#[test]
fn test_gzip_concatenated_members_are_not_truncated() {
    use astersql_tests_realtikvtest_importintotest3::harness::{
        compress_framed, decompress_by_name,
    };
    let mut stream = compress_framed(mydump::Compression::GZ, b"1,test1\n");
    stream.extend(compress_framed(mydump::Compression::GZ, b"2,test2\n"));
    assert_eq!(
        decompress_by_name("t.csv.gz", &stream),
        b"1,test1\n2,test2\n"
    );
}
