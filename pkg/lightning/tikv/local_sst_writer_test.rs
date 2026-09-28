// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
// Copyright 2026 AsterSQL.

// 本地 SST 写入器单元测试。
//
// 覆盖 write CF 编码写入、属性（行数/range_index）生成、短值超限拒绝、
// 非法文件头与键乱序错误路径。

use crate::*;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 测试用固定时间戳。
fn init() -> u64 {
    123456789
}

/// 生成带进程与纳秒后缀的临时 SST 路径，避免并发冲突。
fn temp_file(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "astersql-tikv-{name}-{}-{unique}.sst",
        std::process::id()
    ))
}

/// 写入一组 KV 后读回 `LocalSst`，模拟导入服务侧消费。
fn write2ImportService4Test(
    path: &Path,
    key_values: &[(&[u8], &[u8])],
    ts: u64,
) -> Result<LocalSst, TikvError> {
    pebbleWriteSST(path, key_values, ts)?;
    read_local_sst(path)
}

/// 用 `WriteCFWriter` 按序写入并关闭。
fn pebbleWriteSST(path: &Path, key_values: &[(&[u8], &[u8])], ts: u64) -> Result<(), TikvError> {
    let mut writer = newWriteCFWriter(path, ts)?;
    for (key, value) in key_values {
        writer.Set(key, value)?;
    }
    writer.Close()
}

/// 写入固定三元组样例并读回。
fn testPebbleWriteSST(path: &Path) -> Result<LocalSst, TikvError> {
    let cases: &[(&[u8], &[u8])] = &[(b"a", b"1"), (b"b", b"22"), (b"c", b"333")];
    pebbleWriteSST(path, cases, init())?;
    read_local_sst(path)
}

/// 取出记录副本供断言比较。
fn getData2Compare(sst: &LocalSst) -> Vec<(Vec<u8>, Vec<u8>)> {
    sst.records.clone()
}

/// 端到端：写入两条记录后校验行数属性与 range_index 存在。
#[test]
fn TestIntegrationTest() {
    let path = temp_file("integration");
    let sst =
        write2ImportService4Test(&path, &[(b"key1", b"value1"), (b"key2", b"value2")], 42).unwrap();
    assert_eq!(sst.records.len(), 2);
    assert_eq!(
        u64::from_be_bytes(sst.properties["tikv.num_rows"].clone().try_into().unwrap()),
        2
    );
    assert!(sst.properties.contains_key("tikv.range_index"));
    std::fs::remove_file(path).unwrap();
}

/// Go 的 Pebble writer 产出标准 RocksDB BlockBasedTable，而不是私有容器格式。
#[test]
fn TestProducesRocksDbSst() {
    const ROCKSDB_SST_MAGIC: &[u8; 8] = b"\xf7\xcf\xf4\x85\xb7\x41\xe2\x88";

    let path = temp_file("rocksdb-format");
    pebbleWriteSST(&path, &[(b"a", b"1")], 1).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.len() >= ROCKSDB_SST_MAGIC.len());
    assert_eq!(
        &bytes[bytes.len() - ROCKSDB_SST_MAGIC.len()..],
        ROCKSDB_SST_MAGIC
    );
    std::fs::remove_file(path).unwrap();
}

/// 校验编码键有序、MVCC/write 编码正确，以及超长短值被拒绝。
#[test]
fn TestPebbleWriteSST() {
    let path = temp_file("writer");
    let sst = testPebbleWriteSST(&path).unwrap();
    let records = getData2Compare(&sst);
    assert_eq!(records.len(), 3);
    assert!(records.windows(2).all(|pair| pair[0].0 < pair[1].0));
    assert_eq!(records[0].0, encode_mvcc_key(b"a", init()));
    assert_eq!(records[0].1, encode_write_value(b"1", init()));
    assert_eq!(
        encode_mvcc_key(b"a", 1),
        b"za\0\0\0\0\0\0\0\xf8\xff\xff\xff\xff\xff\xff\xff\xfe"
    );
    assert_eq!(encode_write_value(b"1", 1), b"P\x01v\x011");

    // 256 字节超过 u8::MAX，不能写入 write CF 短值。
    let oversized = vec![0; 256];
    let oversized_path = temp_file("oversized");
    let mut writer = newWriteCFWriter(&oversized_path, 1).unwrap();
    assert!(!isShortValue(&oversized));
    let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        writer.Set(b"a", &oversized)
    }));
    assert!(rejected.is_err());
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(oversized_path).unwrap();
}

/// 对齐 Go 第二组 10,000-key 用例，并覆盖 32 KiB 多 data-block 路径。
#[test]
fn TestPebbleWriteSSTManyKeys() {
    let path = temp_file("many-keys");
    let ts = 404411537129996288;
    let mut writer = newWriteCFWriter(&path, ts).unwrap();
    for index in 0..10_000 {
        let key = format!("key{index:09}");
        writer.Set(key.as_bytes(), b"1").unwrap();
    }
    writer.Close().unwrap();

    let sst = read_local_sst(&path).unwrap();
    assert_eq!(sst.records.len(), 10_000);
    assert!(sst.records.windows(2).all(|pair| pair[0].0 < pair[1].0));
    assert_eq!(sst.records[0].0, encode_mvcc_key(b"key000000000", ts));
    assert_eq!(sst.records[9_999].0, encode_mvcc_key(b"key000009999", ts));
    std::fs::remove_file(path).unwrap();
}

/// 非法魔数与键乱序均应失败。
#[test]
fn TestDebugReadSST() {
    let path = temp_file("debug");
    std::fs::write(&path, b"not-an-sst").unwrap();
    assert!(read_local_sst(&path).is_err());
    std::fs::remove_file(&path).unwrap();

    let duplicate_path = temp_file("order");
    let mut writer = newWriteCFWriter(&duplicate_path, 7).unwrap();
    writer.Set(b"b", b"1").unwrap();
    // 先写 b 再写 a 破坏严格递增。
    assert!(writer.Set(b"a", b"2").is_err());
    std::fs::remove_file(duplicate_path).unwrap();
}
