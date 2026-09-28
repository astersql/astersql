// Copyright 2019 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// KV 校验和模块单元测试。
//
// 覆盖固定样例的 CRC XOR 聚合、JSON 序列化、分组合并，以及 Add/Sub 可逆运算。

use crate::*;

/// 由字符串构造 `KvPair`。
fn pair(key: &str, value: &str) -> KvPair {
    KvPair {
        key: key.as_bytes().to_vec(),
        val: value.as_bytes().to_vec(),
    }
}

#[derive(Default)]
struct RecordingLogEncoder {
    uints: Vec<(String, u64)>,
    object_error: Option<&'static str>,
}

impl LogEncoder for RecordingLogEncoder {
    type Error = &'static str;

    fn AddUint64(&mut self, key: &str, value: u64) {
        self.uints.push((key.to_owned(), value));
    }

    fn AddObject(&mut self, _key: &str, _value: &KVChecksum) -> Result<(), Self::Error> {
        match self.object_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// 验证 Update 后 Sum/SumKVS/SumSize，以及重复 Update 使 XOR 归零。
#[test]
fn TestChecksum() {
    let mut checksum = NewKVChecksum();
    assert_eq!(checksum.Sum(), 0);
    checksum.Update(&[]);
    assert_eq!(checksum.Sum(), 0);
    let pairs = [
        pair("Cop", "PingCAP"),
        pair(
            "Introduction",
            "Inspired by Google Spanner/F1, PingCAP develops TiDB.",
        ),
    ];
    checksum.Update(&pairs);
    assert_eq!(checksum.Sum(), 4_850_203_904_608_948_940);
    assert_eq!(checksum.SumKVS(), 2);
    assert_eq!(checksum.SumSize(), 75);
    // 再混入相同集合，XOR 自逆为 0。
    checksum.Update(&pairs);
    assert_eq!(checksum.Sum(), 0);
    assert_eq!(checksum.SumKVS(), 4);
    assert_eq!(checksum.SumSize(), 150);
}

/// 验证 MarshalJSON 紧凑字段顺序。
#[test]
fn TestChecksumJSON() {
    let checksum = MakeKVChecksum(123, 456, 7890);
    assert_eq!(
        checksum.MarshalJSON(),
        br#"{"checksum":7890,"size":123,"kvs":456}"#
    );
    assert_eq!(
        checksum.String(),
        r#"{"checksum":7890,"size":123,"kvs":456}"#
    );
    assert_eq!(checksum.to_string(), checksum.String());
}

/// 验证日志字段形状，并保留 Go AddObject 错误的传播契约。
#[test]
fn TestChecksumMarshalLogObject() {
    let mut encoder = RecordingLogEncoder::default();
    MakeKVChecksum(123, 456, 7890)
        .MarshalLogObject(&mut encoder)
        .unwrap();
    assert_eq!(
        encoder.uints,
        [
            ("cksum".to_owned(), 7890),
            ("size".to_owned(), 123),
            ("kvs".to_owned(), 456)
        ]
    );

    let mut encoder = RecordingLogEncoder {
        object_error: Some("encode object failed"),
        ..Default::default()
    };
    let error = NewKVGroupChecksumForAdd()
        .MarshalLogObject(&mut encoder)
        .unwrap_err();
    assert_eq!(error, "encode object failed");
}

/// 验证数据组与索引组合并后的条数/字节统计。
#[test]
fn TestGroupChecksum() {
    let mut checksum = NewKVGroupChecksumWithKeyspace(&[]);
    checksum.UpdateOneDataKV(&pair("key", "val"));
    checksum.UpdateOneIndexKV(1, &pair("key2", "val2"));
    let inner = checksum.GetInnerChecksums();
    assert_eq!(inner.len(), 2);
    assert_eq!(inner[&1].SumKVS(), 1);
    assert_eq!(inner[&DataKVGroupID].SumKVS(), 1);

    let mut detached = inner.clone();
    detached
        .get_mut(&1)
        .unwrap()
        .UpdateOne(&pair("extra", "kv"));
    assert_eq!(checksum.GetInnerChecksums()[&1].SumKVS(), 1);

    let mut keyspace_checksum = NewKVGroupChecksumWithKeyspace(b"keyspace");
    keyspace_checksum.UpdateOneDataKV(&pair("key", "val"));
    keyspace_checksum.UpdateOneIndexKV(1, &pair("key2", "val2"));
    assert_ne!(inner, keyspace_checksum.GetInnerChecksums());

    let mut other = NewKVGroupChecksumWithKeyspace(&[]);
    other.UpdateOneIndexKV(1, &pair("key", "val"));
    other.UpdateOneIndexKV(2, &pair("key2", "val2"));
    checksum.Add(&other);
    let inner = checksum.GetInnerChecksums();
    assert_eq!(inner.len(), 3);
    assert_eq!(inner[&1].SumKVS(), 2);
    assert_eq!(inner[&2].SumKVS(), 1);
    assert_eq!(inner[&DataKVGroupID].SumKVS(), 1);
    assert_eq!(checksum.DataAndIndexSumKVS(), (1, 3));
    assert_eq!(checksum.DataAndIndexSumSize(), (6, 22));
    assert_eq!(checksum.MergedChecksum().SumKVS(), 4);
    assert_eq!(checksum.MergedChecksum().SumSize(), 28);
}

/// 验证 Add/Sub 对校验和与统计量的影响。
#[test]
fn TestKVChecksumOperation() {
    let mut checksum = NewKVChecksum();
    checksum.Add(&MakeKVChecksum(100, 100, 100));
    assert_eq!(checksum.Sum(), 100);
    assert_eq!(checksum.SumSize(), 100);
    assert_eq!(checksum.SumKVS(), 100);
    checksum.Sub(&MakeKVChecksum(10, 20, 30));
    assert_eq!(checksum.Sum(), 100 ^ 30);
    assert_eq!(checksum.SumSize(), 90);
    assert_eq!(checksum.SumKVS(), 80);
}

/// Go 的 uint64 统计运算按模 2^64 回绕，Rust 调试构建也必须保持一致。
#[test]
fn TestKVChecksumWrappingArithmetic() {
    let mut checksum = MakeKVChecksum(u64::MAX, u64::MAX, 1);
    checksum.Add(&MakeKVChecksum(1, 1, 2));
    assert_eq!(checksum.SumSize(), 0);
    assert_eq!(checksum.SumKVS(), 0);
    assert_eq!(checksum.Sum(), 3);

    checksum.Sub(&MakeKVChecksum(1, 1, 2));
    assert_eq!(checksum.SumSize(), u64::MAX);
    assert_eq!(checksum.SumKVS(), u64::MAX);
    assert_eq!(checksum.Sum(), 1);

    let mut updated = MakeKVChecksumWithKeyspace(b"k", u64::MAX, u64::MAX, 0);
    updated.UpdateOne(&pair("", ""));
    assert_eq!(updated.SumSize(), 0);
    assert_eq!(updated.SumKVS(), 0);

    let mut groups = NewKVGroupChecksumForAdd();
    groups.AddRawGroup(1, u64::MAX, u64::MAX, 1);
    groups.AddRawGroup(2, 1, 1, 2);
    assert_eq!(groups.DataAndIndexSumSize(), (0, 0));
    assert_eq!(groups.DataAndIndexSumKVS(), (0, 0));
    assert_eq!(groups.MergedChecksum().Sum(), 3);
}

/// 直接构造的 keyspace 状态与“带 keyspace 空状态 + 原始统计”保持等价。
#[test]
fn TestMakeKVChecksumWithKeyspace() {
    let mut direct = MakeKVChecksumWithKeyspace(b"keyspace", 10, 20, 30);
    let mut composed = NewKVChecksumWithKeyspace(b"keyspace");
    composed.Add(&MakeKVChecksum(10, 20, 30));

    let pair = pair("key", "value");
    direct.UpdateOne(&pair);
    composed.UpdateOne(&pair);
    assert_eq!(direct, composed);
}
