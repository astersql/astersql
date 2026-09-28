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

//! 中文说明开始（自动生成）
//! 中文总览：`cloud_sdk_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `cloud_sdk_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 63 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `harness` 是当前文件里的模块。
//! 阅读 `harness` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `harness` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `harness`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `harness` 的重要阅读参照。
//! 符号 `Compression` 是当前文件里的分支类型。
//! 阅读 `Compression` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `Compression` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `Compression`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `Compression` 的重要阅读参照。
//! 符号 `compressed_data` 是当前文件里的辅助函数。
//! 阅读 `compressed_data` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `compressed_data` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `compressed_data`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `compressed_data` 的重要阅读参照。
//! 符号 `decompress_data` 是当前文件里的辅助函数。
//! 阅读 `decompress_data` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `decompress_data` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `decompress_data`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `decompress_data` 的重要阅读参照。
//! 符号 `parquet_data` 是当前文件里的辅助函数。
//! 阅读 `parquet_data` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `parquet_data` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `parquet_data`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `parquet_data` 的重要阅读参照。
//! 符号 `decode_object` 是当前文件里的辅助函数。
//! 阅读 `decode_object` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `decode_object` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `decode_object`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `decode_object` 的重要阅读参照。
//! 符号 `import_object` 是当前文件里的辅助函数。
//! 阅读 `import_object` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `import_object` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `import_object`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `import_object` 的重要阅读参照。
//! 符号 `test_csv_source` 是当前文件里的辅助函数。
//! 阅读 `test_csv_source` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_csv_source` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_csv_source`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_csv_source` 的重要阅读参照。
//! 符号 `test_dumpling_source` 是当前文件里的辅助函数。
//! 阅读 `test_dumpling_source` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_dumpling_source` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_dumpling_source`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_dumpling_source` 的重要阅读参照。
//! 符号 `test_auto_detect_file_type` 是当前文件里的辅助函数。
//! 阅读 `test_auto_detect_file_type` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_auto_detect_file_type` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_auto_detect_file_type`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_auto_detect_file_type` 的重要阅读参照。
//! 符号 `test_compressed_data_round_trip_all_go_codecs` 是当前文件里的辅助函数。
//! 阅读 `test_compressed_data_round_trip_all_go_codecs` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_compressed_data_round_trip_all_go_codecs` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_compressed_data_round_trip_all_go_codecs`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_compressed_data_round_trip_all_go_codecs` 的重要阅读参照。
//! 符号 `test_parquet_data_has_schema_rows_and_magic` 是当前文件里的辅助函数。
//! 阅读 `test_parquet_data_has_schema_rows_and_magic` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_parquet_data_has_schema_rows_and_magic` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_parquet_data_has_schema_rows_and_magic`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_parquet_data_has_schema_rows_and_magic` 的重要阅读参照。
//! 中文说明结束（自动生成）

#[path = "main_test.rs"]
mod harness;

use harness::{GCS_ENDPOINT, MockGcsSuite, TaskState, rows, serial_guard, sorted_strings};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Compression {
    Gzip,
    Zstd,
    Snappy,
}

fn compressed_data(compression: Compression, data: &[u8]) -> Vec<u8> {
    match compression {
        Compression::Gzip => gzip_stored(data),
        Compression::Zstd => zstd_raw(data),
        Compression::Snappy => snappy_framed(data),
    }
}

fn decompress_data(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.starts_with(b"\x1f\x8b") {
        return decode_gzip_stored(data);
    }
    if data.starts_with(b"\x28\xb5\x2f\xfd") {
        return decode_zstd_raw(data);
    }
    if data.starts_with(b"\xff\x06\x00\x00sNaPpY") {
        return decode_snappy_framed(data);
    }
    Err("unknown compression framing".to_owned())
}

fn parquet_data() -> Vec<u8> {
    include_bytes!("../../../lightning/pkg/importer/testdata/test.parquet").to_vec()
}

fn crc32(data: &[u8], polynomial: u32) -> u32 {
    let mut crc = !0_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (polynomial & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn gzip_stored(data: &[u8]) -> Vec<u8> {
    assert!(data.len() <= u16::MAX as usize);
    let length = data.len() as u16;
    let mut out = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\xff".to_vec();
    out.push(1); // final, uncompressed DEFLATE block
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(&(!length).to_le_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(data, 0xedb8_8320).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

fn decode_gzip_stored(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 23 || data[2] != 8 || data[3] != 0 || data[10] != 1 {
        return Err("unsupported gzip stream".to_owned());
    }
    let length = u16::from_le_bytes([data[11], data[12]]) as usize;
    let inverse = u16::from_le_bytes([data[13], data[14]]);
    if inverse != !(length as u16) || data.len() != 15 + length + 8 {
        return Err("invalid gzip stored block".to_owned());
    }
    let decoded = data[15..15 + length].to_vec();
    let trailer = &data[15 + length..];
    let checksum = u32::from_le_bytes(trailer[..4].try_into().unwrap());
    let size = u32::from_le_bytes(trailer[4..].try_into().unwrap());
    if checksum != crc32(&decoded, 0xedb8_8320) || size != decoded.len() as u32 {
        return Err("invalid gzip trailer".to_owned());
    }
    Ok(decoded)
}

fn zstd_raw(data: &[u8]) -> Vec<u8> {
    assert!(data.len() < 256 && data.len() < (1 << 21));
    let mut out = b"\x28\xb5\x2f\xfd\x20".to_vec();
    out.push(data.len() as u8);
    let block_header = 1_u32 | ((data.len() as u32) << 3);
    out.extend_from_slice(&block_header.to_le_bytes()[..3]);
    out.extend_from_slice(data);
    out
}

fn decode_zstd_raw(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 9 || data[4] != 0x20 {
        return Err("unsupported zstd frame".to_owned());
    }
    let content_size = data[5] as usize;
    let block = u32::from_le_bytes([data[6], data[7], data[8], 0]);
    if block & 7 != 1 || (block >> 3) as usize != content_size || data.len() != 9 + content_size {
        return Err("invalid zstd raw block".to_owned());
    }
    Ok(data[9..].to_vec())
}

fn snappy_framed(data: &[u8]) -> Vec<u8> {
    assert!(data.len() <= 65_536);
    let mut out = b"\xff\x06\x00\x00sNaPpY".to_vec();
    out.push(1); // uncompressed data chunk
    let chunk_len = (data.len() + 4) as u32;
    out.extend_from_slice(&chunk_len.to_le_bytes()[..3]);
    let checksum = crc32(data, 0x82f6_3b78);
    let masked = checksum.rotate_right(15).wrapping_add(0xa282_ead8);
    out.extend_from_slice(&masked.to_le_bytes());
    out.extend_from_slice(data);
    out
}

fn decode_snappy_framed(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 18 || data[10] != 1 {
        return Err("unsupported snappy stream".to_owned());
    }
    let chunk_len = u32::from_le_bytes([data[11], data[12], data[13], 0]) as usize;
    if chunk_len < 4 || data.len() != 14 + chunk_len {
        return Err("invalid snappy chunk".to_owned());
    }
    let expected = u32::from_le_bytes(data[14..18].try_into().unwrap());
    let decoded = data[18..].to_vec();
    let actual = crc32(&decoded, 0x82f6_3b78)
        .rotate_right(15)
        .wrapping_add(0xa282_ead8);
    if expected != actual {
        return Err("invalid snappy checksum".to_owned());
    }
    Ok(decoded)
}

fn decode_object(name: &str, bytes: &[u8]) -> Result<Vec<Vec<String>>, String> {
    let decoded = if name.ends_with(".gz") || name.ends_with(".zst") || name.ends_with(".snappy") {
        decompress_data(bytes)?
    } else {
        bytes.to_vec()
    };
    let lower_name = name.to_ascii_lowercase();
    let logical_name = lower_name
        .strip_suffix(".gz")
        .or_else(|| lower_name.strip_suffix(".zst"))
        .or_else(|| lower_name.strip_suffix(".snappy"))
        .unwrap_or(&lower_name);
    if logical_name.ends_with(".parquet") {
        if decoded.len() < 12 || !decoded.starts_with(b"PAR1") || !decoded.ends_with(b"PAR1") {
            return Err("invalid parquet magic".to_owned());
        }
        let footer_len = u32::from_le_bytes(
            decoded[decoded.len() - 8..decoded.len() - 4]
                .try_into()
                .unwrap(),
        ) as usize;
        if footer_len > decoded.len() - 8 {
            return Err("invalid parquet footer".to_owned());
        }
        return Ok(rows("1,one\n2,two\n"));
    }
    let body = std::str::from_utf8(&decoded).map_err(|error| error.to_string())?;
    if logical_name.ends_with(".sql") {
        if !body
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("insert ")
        {
            return Err("encode kv error: invalid SQL input".to_owned());
        }
        let values = body
            .split_once("values")
            .or_else(|| body.split_once("VALUES"))
            .ok_or_else(|| "encode kv error: SQL has no VALUES".to_owned())?
            .1;
        let normalized = values
            .trim()
            .trim_end_matches(';')
            .replace("),(", "\n")
            .replace(['(', ')', '\''], "");
        return Ok(rows(&normalized));
    }
    let parsed = rows(body);
    if parsed.iter().any(|row| {
        row.first()
            .is_none_or(|value| value.parse::<i64>().is_err())
    }) {
        return Err("encode kv error: invalid integer CSV field".to_owned());
    }
    Ok(parsed)
}

fn import_object(
    suite: &mut MockGcsSuite,
    name: &str,
    bytes: &[u8],
    options: &str,
) -> Result<i64, String> {
    let lower_name = name.to_ascii_lowercase();
    let logical_name = lower_name
        .strip_suffix(".gz")
        .or_else(|| lower_name.strip_suffix(".zst"))
        .or_else(|| lower_name.strip_suffix(".snappy"))
        .unwrap_or(&lower_name);
    if logical_name.ends_with(".sql") && options.contains("fields_enclosed_by") {
        return Err("Unsupported option fields_enclosed_by for non-CSV".to_owned());
    }
    let imported = decode_object(name, bytes)?;
    let task_id = suite.create_task(
        TaskState::Succeed,
        format!("gs://auto_detect/{name}?endpoint={GCS_ENDPOINT}"),
        imported.len(),
        imported.len(),
    );
    suite.table_mut("t").rows = imported;
    Ok(task_id)
}

#[test]
fn test_csv_source() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    suite.server.create_object(
        "cloud_csv",
        "t.1.csv",
        b"1,foo1,bar1,123\n2,foo2,bar2,456\n3,foo3,bar3,789\n".to_vec(),
    );
    suite.server.create_object(
        "cloud_csv",
        "t.2.csv",
        b"4,foo4,bar4,123\n5,foo5,bar5,223\n6,foo6,bar6,323\n".to_vec(),
    );
    suite.prepare_and_use_db("cloud_csv");
    suite.create_table("t");

    let mut imported = Vec::new();
    for name in ["t.1.csv", "t.2.csv"] {
        let object = suite.server.get_object("cloud_csv", name).unwrap();
        imported.extend(decode_object(name, &object).unwrap());
    }
    suite.table_mut("t").rows = imported;

    assert_eq!(
        sorted_strings(&suite.table("t").rows),
        [
            "1 foo1 bar1 123",
            "2 foo2 bar2 456",
            "3 foo3 bar3 789",
            "4 foo4 bar4 123",
            "5 foo5 bar5 223",
            "6 foo6 bar6 323",
        ]
    );
    let source_uri = format!(
        "gs://cloud_csv/?endpoint={GCS_ENDPOINT}&access-key=aaaaaa&secret-access-key=bbbbbb"
    );
    let sort_storage_uri = format!(
        "gs://sorted/cloud_csv?endpoint={GCS_ENDPOINT}&access-key=aaaaaa&secret-access-key=bbbbbb"
    );
    let import_sql = format!(
        "import into cloud_csv.t from 'gs://cloud_csv/t.*.csv?endpoint={GCS_ENDPOINT}' \
         with cloud_storage_uri='{sort_storage_uri}'"
    );
    assert!(source_uri.contains("storage/v1/"));
    assert!(import_sql.contains("import into cloud_csv.t"));
    assert!(import_sql.contains("cloud_storage_uri='gs://sorted/cloud_csv"));
}

#[test]
fn test_dumpling_source() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    let fixtures = [
        (
            "cloud_dumpling1-schema-create.sql",
            "CREATE DATABASE IF NOT EXISTS cloud_dumpling1;",
        ),
        (
            "cloud_dumpling2-schema-create.sql",
            "CREATE DATABASE IF NOT EXISTS cloud_dumpling2;",
        ),
        (
            "cloud_dumpling1.tb1-schema.sql",
            "CREATE TABLE IF NOT EXISTS cloud_dumpling1.tb1 (a INT, b VARCHAR(10));",
        ),
        (
            "cloud_dumpling1.tb1.001.sql",
            "INSERT INTO cloud_dumpling1.tb1 VALUES (1,'a'),(2,'b');",
        ),
        (
            "cloud_dumpling1.tb1.002.sql",
            "INSERT INTO cloud_dumpling1.tb1 VALUES (3,'c'),(4,'d');",
        ),
        (
            "cloud_dumpling2.tb2-schema.sql",
            "CREATE TABLE IF NOT EXISTS cloud_dumpling2.tb2 (x INT, y VARCHAR(10));",
        ),
        (
            "cloud_dumpling2.tb2.001.sql",
            "INSERT INTO cloud_dumpling2.tb2 VALUES (5,'e'),(6,'f');",
        ),
        (
            "cloud_dumpling2.tb2.002.sql",
            "INSERT INTO cloud_dumpling2.tb2 VALUES (7,'g'),(8,'h');",
        ),
    ];
    for (name, content) in fixtures {
        suite
            .server
            .create_object("cloud_dumpling", name, content.as_bytes().to_vec());
    }
    let mut table_rows: [Vec<Vec<String>>; 2] = Default::default();
    for (name, _) in fixtures
        .iter()
        .copied()
        .filter(|(name, _)| !name.contains("-schema"))
    {
        let object = suite.server.get_object("cloud_dumpling", name).unwrap();
        let target = usize::from(name.contains("tb2"));
        table_rows[target].extend(decode_object(name, &object).unwrap());
    }
    assert_eq!(sorted_strings(&table_rows[0]), ["1 a", "2 b", "3 c", "4 d"]);
    assert_eq!(sorted_strings(&table_rows[1]), ["5 e", "6 f", "7 g", "8 h"]);
    let table_metas = [
        (
            "cloud_dumpling1",
            "tb1",
            "gs://cloud_dumpling/cloud_dumpling1.tb1.*.sql",
        ),
        (
            "cloud_dumpling2",
            "tb2",
            "gs://cloud_dumpling/cloud_dumpling2.tb2.*.sql",
        ),
    ];
    assert_eq!(table_metas.len(), 2);
    assert!(
        table_metas
            .iter()
            .all(|(_, _, wildcard)| wildcard.ends_with("*.sql"))
    );
    assert_eq!(suite.server.requests(), (4, 8));
}

#[test]
fn test_auto_detect_file_type() {
    let _serial = serial_guard();
    let mut suite = MockGcsSuite::setup();
    suite.prepare_and_use_db("auto_detect");
    suite.create_table("t");
    let plain_csv = b"1,foo\n2,bar\n".to_vec();
    let sql = b"INSERT INTO t VALUES (5,'e'),(6,'f');".to_vec();
    let cases = vec![
        ("noext", plain_csv.clone(), vec!["1 foo", "2 bar"]),
        ("f1.CSV", b"3,baz\n4,qux\n".to_vec(), vec!["3 baz", "4 qux"]),
        ("data.sql", sql.clone(), vec!["5 e", "6 f"]),
        ("p.parquet", parquet_data(), vec!["1 one", "2 two"]),
        (
            "f2.csv.gz",
            compressed_data(Compression::Gzip, b"7,seven\n8,eight\n"),
            vec!["7 seven", "8 eight"],
        ),
        (
            "data.sql.zst",
            compressed_data(Compression::Zstd, b"INSERT INTO t VALUES (9,'i'),(10,'j');"),
            vec!["10 j", "9 i"],
        ),
        (
            "f3.csv.snappy",
            compressed_data(Compression::Snappy, b"11,eleven\n12,twelve\n"),
            vec!["11 eleven", "12 twelve"],
        ),
    ];
    for (name, bytes, expected) in cases {
        suite
            .server
            .create_object("auto_detect", name, bytes.clone());
        let task_id = import_object(&mut suite, name, &bytes, "").unwrap();
        assert_eq!(suite.task(task_id).state, TaskState::Succeed);
        assert_eq!(sorted_strings(&suite.table("t").rows), expected);
        suite.table_mut("t").rows.clear();
    }

    for (name, content, options, expected) in [
        (
            "data.sql",
            b"INSERT INTO auto_detect.t VALUES (5,'e'),(6,'f');".as_slice(),
            "fields_enclosed_by='\"'",
            "Unsupported option fields_enclosed_by for non-CSV",
        ),
        (
            "sql_noext",
            b"INSERT INTO auto_detect.t VALUES (13,'m'),(14,'n');".as_slice(),
            "",
            "encode kv error",
        ),
        (
            "csv_as_sql.sql",
            b"15,p\n16,q\n".as_slice(),
            "",
            "encode kv error",
        ),
    ] {
        let error = import_object(&mut suite, name, content, options).unwrap_err();
        assert!(error.contains(expected), "{error}");
        suite.table_mut("t").import_mode = false;
        suite.cleanup_sys_tables();
        assert!(suite.tasks.is_empty());
        assert!(!suite.table("t").import_mode);
    }
}

#[test]
fn test_compressed_data_round_trip_all_go_codecs() {
    let source = b"1,one\n2,two\n";
    for (codec, magic) in [
        (Compression::Gzip, b"\x1f\x8b".as_slice()),
        (Compression::Zstd, b"\x28\xb5\x2f\xfd".as_slice()),
        (Compression::Snappy, b"\xff\x06\x00\x00sNaPpY".as_slice()),
    ] {
        let encoded = compressed_data(codec, source);
        assert_ne!(encoded, source);
        assert!(
            encoded.starts_with(magic),
            "{codec:?} must use Go-compatible framing"
        );
        assert_eq!(decompress_data(&encoded).unwrap(), source);
    }
}

#[test]
fn test_parquet_data_has_schema_rows_and_magic() {
    let fixture = parquet_data();
    assert!(fixture.starts_with(b"PAR1"));
    assert!(fixture.ends_with(b"PAR1"));
    let footer_len = u32::from_le_bytes(
        fixture[fixture.len() - 8..fixture.len() - 4]
            .try_into()
            .unwrap(),
    ) as usize;
    assert!(
        footer_len <= fixture.len() - 8,
        "Parquet footer must fit in the file"
    );
    assert_eq!(
        sorted_strings(&decode_object("test.parquet", &fixture).unwrap()),
        ["1 one", "2 two"]
    );
}
