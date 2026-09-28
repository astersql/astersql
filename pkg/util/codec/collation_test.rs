// Copyright 2020 PingCAP, Inc.
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

// Collation（字符序）相关编解码与哈希行为的单元测试。
//
// Collation 决定字符串如何比较与排序。本文件验证新旧 collation 开关下
// Encoder 生成的 key/hash，以及 HashGroupKey / HashChunkRow / HashChunkColumns
// 在 ci collation 下对大小写/同形字符的等价性。

// 本文件由 pkg/util/codec/collation_test.go 迁移而来，保留新旧 collation 与 hash 行为。
//

use super::*;
use std::hash::Hasher;

/// 构造两组字符串列：大小写、emoji、重音，用于 collation 等价性断言。
fn prepareCollationData() -> (usize, Box<chunk::Chunk>, Box<chunk::Chunk>) {
    let tp = types::NewFieldType(mysql::TypeString);
    let mut chk1 = chunk::New(vec![(*tp).clone()], 3, 3);
    let mut chk2 = chunk::New(vec![(*tp).clone()], 3, 3);
    chk1.Reset();
    chk2.Reset();
    for (left, right) in [("aaa", "AAA"), ("😜", "😃"), ("À", "A")] {
        chk1.AppendString(0, left);
        chk2.AppendString(0, right);
    }
    (3, chk1, chk2)
}

/// RAII：测试结束时恢复全局新 collation 开关。
struct CollationRestore(bool);
impl Drop for CollationRestore {
    fn drop(&mut self) {
        collate::SetNewCollationEnabledForTest(self.0);
    }
}

/// 验证启用/禁用新 collation 时 Encoder 对大小写不敏感 key 与 HashCode 行为。
#[test]
fn TestEncoderNewCollationEnabled() {
    let _restore = CollationRestore(collate::NewCollationEnabled());
    let lower = types::NewCollationStringDatum("aaa".to_owned(), "utf8_general_ci".to_owned());
    let upper = types::NewCollationStringDatum("AAA".to_owned(), "utf8_general_ci".to_owned());
    let enabled_encoder = NewEncoder(true);
    let disabled_encoder = NewEncoder(false);

    collate::SetNewCollationEnabledForTest(true);
    let enabled_lower = enabled_encoder
        .EncodeKey(time::UTC, Vec::new(), vec![lower.clone()])
        .unwrap();
    let enabled_upper = enabled_encoder
        .EncodeKey(time::UTC, Vec::new(), vec![upper.clone()])
        .unwrap();
    assert_eq!(enabled_lower, enabled_upper);

    let disabled_lower = disabled_encoder
        .EncodeKey(time::UTC, Vec::new(), vec![lower.clone()])
        .unwrap();
    let disabled_upper = disabled_encoder
        .EncodeKey(time::UTC, Vec::new(), vec![upper])
        .unwrap();
    assert_ne!(disabled_lower, disabled_upper);
    assert_eq!(
        enabled_lower,
        EncodeKey(time::UTC, Vec::new(), vec![lower.clone()]).unwrap()
    );

    collate::SetNewCollationEnabledForTest(false);
    assert_eq!(
        disabled_lower,
        EncodeKey(time::UTC, Vec::new(), vec![lower.clone()]).unwrap()
    );
    assert_eq!(
        enabled_encoder.HashCode(Vec::new(), lower.clone()),
        disabled_encoder.HashCode(Vec::new(), lower)
    );
}

/// ci collation 下 HashGroupKey 应对大小写/同形字符给出相同 group key。
#[test]
fn TestHashGroupKeyCollation() {
    let mut tp = types::NewFieldType(mysql::TypeString);
    let (n, mut chk1, mut chk2) = prepareCollationData();
    for collate_name in ["utf8_general_ci", "utf8_unicode_ci"] {
        tp.SetCollate(collate_name.to_owned());
        let buf1 = HashGroupKey(
            time::UTC,
            n,
            &mut chk1.columns[0],
            vec![Vec::new(); n],
            &mut *tp,
        )
        .unwrap();
        let buf2 = HashGroupKey(
            time::UTC,
            n,
            &mut chk2.columns[0],
            vec![Vec::new(); n],
            &mut *tp,
        )
        .unwrap();
        assert_eq!(buf1, buf2);
    }
}

/// HashChunkRow：binary 应区分大小写，ci collation 应视为相等。
#[test]
fn TestHashChunkRowCollation() {
    let type_ctx = types::DefaultStmtNoWarningContext.WithLocation(time::UTC);
    let mut tp = types::NewFieldType(mysql::TypeString);
    let (n, chk1, chk2) = prepareCollationData();
    for (collate_name, should_equal) in [
        ("binary", false),
        ("utf8_general_ci", true),
        ("utf8_unicode_ci", true),
    ] {
        tp.SetCollate(collate_name.to_owned());
        for i in 0..n {
            let mut encoded1 = Vec::new();
            let mut encoded2 = Vec::new();
            HashChunkRow(
                type_ctx.clone(),
                &mut encoded1,
                chk1.GetRow(i),
                vec![&mut *tp],
                vec![0],
                vec![0],
            )
            .unwrap();
            HashChunkRow(
                type_ctx.clone(),
                &mut encoded2,
                chk2.GetRow(i),
                vec![&mut *tp],
                vec![0],
                vec![0],
            )
            .unwrap();
            assert_eq!(
                encoded1 == encoded2,
                should_equal,
                "collation={collate_name}, row={i}"
            );
        }
    }
}

/// HashChunkColumns：与 HashChunkRow 相同的 collation 等价性期望。
#[test]
fn TestHashChunkColumnsCollation() {
    let type_ctx = types::DefaultStmtNoWarningContext.WithLocation(time::UTC);
    let mut tp = types::NewFieldType(mysql::TypeString);
    let (n, mut chk1, mut chk2) = prepareCollationData();
    for (collate_name, should_equal) in [
        ("binary", false),
        ("utf8_general_ci", true),
        ("utf8_unicode_ci", true),
    ] {
        tp.SetCollate(collate_name.to_owned());
        let mut h1s: Vec<Box<dyn Hasher>> = (0..n)
            .map(|_| Box::new(fnv::FnvHasher::default()) as Box<dyn Hasher>)
            .collect();
        let mut h2s: Vec<Box<dyn Hasher>> = (0..n)
            .map(|_| Box::new(fnv::FnvHasher::default()) as Box<dyn Hasher>)
            .collect();
        let mut is_null = vec![false; n];
        HashChunkColumns(
            type_ctx.clone(),
            &mut h1s,
            &mut *chk1,
            &mut *tp,
            0,
            vec![0],
            &mut is_null,
        )
        .unwrap();
        let mut is_null = vec![false; n];
        HashChunkColumns(
            type_ctx.clone(),
            &mut h2s,
            &mut *chk2,
            &mut *tp,
            0,
            vec![0],
            &mut is_null,
        )
        .unwrap();
        for i in 0..n {
            assert_eq!(
                h1s[i].finish() == h2s[i].finish(),
                should_equal,
                "collation={collate_name}, row={i}"
            );
        }
    }
}
