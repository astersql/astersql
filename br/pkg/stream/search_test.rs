// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! 对应 Go `br/pkg/stream/search_test.go`：前缀比较器、按文件搜索与 CF 合并。
//! 用 MemStorage 注入伪造 DefaultCF/WriteCF 数据文件与 `.meta`，不访问真实外部存储。
//! `Search` 内部会 EncodeBytes(searchKey)，与 Go SearchFromDataFileForTest 编码约定一致。
//! 合并断言：Write 条数 + 1（未配对的 Default）应等于结果长度。
//! aa_/bb_/cc_ 前缀便于肉眼区分搜索与合并场景。
//! 不改动被测实现，仅补充场景与断言意图说明。
//! Sha256 必须与文件字节一致，否则 Search 会在校验阶段失败。
//! 键展示为大写 hex，断言时用 `hex::encode(...).to_uppercase()` 对齐。
//! WriteCF shortValue 编码与 decode_kv/RawWriteCFValue 解析路径配套。
//! 时间戳取自 `now_ts()`，避免固定常量在并发跑测时偶发碰撞。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use astersql_br_pkg_utils_consts::{DefaultCF, WriteCF};
use sha2::{Digest, Sha256};

use crate::decode_kv::EncodeKVEntry;
use crate::search::{NewStartWithComparator, NewStreamBackupSearch, StreamKVInfo};
use crate::stream_mgr::GetStreamBackupMetaPrefix;
use crate::stubs::backuppb::{DataFileInfo, Metadata};
use crate::stubs::codec;
use crate::stubs::{MemStorage, Storage};

/// 对应 Go 前缀比较器单测：命中前缀 / 超前缀 / 完全不同。
#[test]
fn test_start_with_comparator() {
    let comparator = NewStartWithComparator();
    // `aa` 是 `aa_key` 前缀。
    assert!(comparator.Compare(b"aa_key", b"aa"));
    // `aak` 不是前缀（多出字母破坏边界）。
    assert!(!comparator.Compare(b"aa_key", b"aak"));
    // 完全不同前缀。
    assert!(!comparator.Compare(b"aa_key", b"bb"));
}

/// 业务键 + 降序 Ts 后缀，模拟文件内编码键。
fn encode_key(key: &str, ts: i64) -> Vec<u8> {
    let encoded = codec::EncodeBytes(Vec::new(), key.as_bytes());
    codec::EncodeUintDesc(encoded, ts as u64)
}

/// 构造带 shortValue 的 WriteCF 原始值：`P` + uvarint(startTs) + `v` + len + bytes。
fn encode_short_value(val: &str, ts: i64) -> Vec<u8> {
    let mut buff = vec![b'P'];
    buff = codec::EncodeUvarint(buff, ts as u64);
    buff.push(b'v');
    buff.push(val.len() as u8);
    buff.extend_from_slice(val.as_bytes());
    // 无额外 flag，保持最短合法 Put。
    buff
}

/// 构造不带 shortValue 的 WriteCF 值，强制由 DefaultCF 回填大值。
fn encode_long_value(ts: i64) -> Vec<u8> {
    let mut buff = vec![b'P'];
    codec::EncodeUvarint(buff, ts as u64)
}

/// 单条 CF 样例：big key 走 Default，small/配对 key 走 Write。
struct Cf {
    /// 业务键字符串，编码前为 UTF-8 字节。
    key: String,
    /// Default 用 start_ts 挂在键尾；Write 用其回填合并键。
    start_ts: i64,
    /// Write 键尾挂 commit_ts；Default 侧一般为 0。
    commit_ts: i64,
    /// 明文值；写入前按 CF 类型再编码。
    val: String,
}

/// 用当前纳秒作唯一 Ts，避免并行用例键冲突。
fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64
}

/// 伪造 Default/Write 两组：含可合并的 big key 与仅 Write 的 small key。
fn fake_cfs() -> (Vec<Cf>, Vec<Cf>) {
    let default_cfs = vec![
        // Default：大值存 DefaultCF，Write 侧无 shortValue 时回查。
        Cf {
            key: "aa_big_key_1".into(),
            start_ts: now_ts(),
            commit_ts: 0,
            val: "aa_big_val_1".into(),
        },
        Cf {
            key: "bb_big_key_1".into(),
            start_ts: now_ts(),
            commit_ts: 0,
            val: "bb_big_val_1".into(),
        },
        // 无对应 Write 的 Default，合并后应单独保留。
        Cf {
            key: "cc_big_key_1".into(),
            start_ts: now_ts(),
            commit_ts: 0,
            val: "cc_big_val_1".into(),
        },
    ];
    let write_cfs = vec![
        // 仅 Write + shortValue，不依赖 Default。
        Cf {
            key: "aa_small_key_1".into(),
            start_ts: now_ts(),
            commit_ts: now_ts(),
            val: "aa_small_val_1".into(),
        },
        // 与 default_cfs[0] 同 start_ts，供合并回填 Value。
        Cf {
            key: "aa_big_key_1".into(),
            start_ts: default_cfs[0].start_ts,
            commit_ts: now_ts(),
            val: "aa_short_val_1".into(),
        },
        Cf {
            key: "bb_small_key_1".into(),
            start_ts: now_ts(),
            commit_ts: now_ts(),
            val: "bb_small_val_1".into(),
        },
        Cf {
            key: "bb_big_key_1".into(),
            start_ts: default_cfs[1].start_ts,
            commit_ts: now_ts(),
            val: "bb_short_val_1".into(),
        },
    ];
    // 顺序：Default 列表、Write 列表。
    (default_cfs, write_cfs)
}

/// 写入两份数据文件并返回带 Sha256 的 DataFileInfo。
/// StartKey/EndKey 拉满，确保 resolveMetaData 不会因区间过滤丢弃。
fn fake_data_file(s: &MemStorage) -> (DataFileInfo, DataFileInfo) {
    let (default_cfs, write_cfs) = fake_cfs();
    let mut default_buf = Vec::new();
    for c in &default_cfs {
        // 按 EventIterator 编码拼接多条 KV。
        default_buf.extend(EncodeKVEntry(
            &encode_key(&c.key, c.start_ts),
            c.val.as_bytes(),
        ));
    }
    s.WriteFile("default_cf", &default_buf).unwrap();
    let default_sum = Sha256::digest(&default_buf);
    let default_file = DataFileInfo {
        Path: "default_cf".into(),
        Cf: DefaultCF.into(),
        Sha256: default_sum.to_vec(),
        StartKey: vec![],
        // 宽松上界避免键区间过滤。
        EndKey: vec![0xff; 128],
        ..Default::default()
    };

    let mut write_buf = Vec::new();
    for c in &write_cfs {
        // small 值内联 shortValue；big 值留给 DefaultCF 跨文件回填。
        let value = if c.key.contains("_big_") {
            encode_long_value(c.start_ts)
        } else {
            encode_short_value(&c.val, c.start_ts)
        };
        write_buf.extend(EncodeKVEntry(&encode_key(&c.key, c.commit_ts), &value));
    }
    s.WriteFile("write_cf", &write_buf).unwrap();
    let write_sum = Sha256::digest(&write_buf);
    let write_file = DataFileInfo {
        Path: "write_cf".into(),
        Cf: WriteCF.into(),
        Sha256: write_sum.to_vec(),
        StartKey: vec![],
        EndKey: vec![0xff; 128],
        ..Default::default()
    };
    // 调用方需把两者挂入同一 Metadata.Files。
    (default_file, write_file)
}

/// 对应 Go `TestSearchFromDataFile`：经公开 `Search()` 走完整路径。
/// 搜索 `aa_big_key_1` 应命中 Default+Write 合并相关的两条。
#[test]
fn test_search_from_data_file() {
    let s = Arc::new(MemStorage::new());
    let (default_file, write_file) = fake_data_file(&s);
    let meta = Metadata {
        Files: vec![default_file, write_file],
        ..Default::default()
    };
    let raw = serde_json::to_vec(&meta).unwrap();
    // meta 放在流备份约定前缀下，供 ListFiles 发现。
    let meta_path = format!("{}/0001.meta", GetStreamBackupMetaPrefix());
    s.WriteFile(&meta_path, &raw).unwrap();

    let comparator = NewStartWithComparator();
    let search_key = b"aa_big_key_1".to_vec();
    // Search 内部会再 EncodeBytes；与 Go 测试编码路径一致。
    let bs = NewStreamBackupSearch(s, comparator, search_key.clone());
    // 端到端：列 meta → 校验和 → 迭代 → 合并。
    let out = bs.Search().unwrap();

    // 结果键为大写 hex 业务键前缀。
    let hex_search = hex::encode(&search_key).to_uppercase();
    let mut count = 0;
    for kv in &out {
        assert!(
            kv.Key.starts_with(&hex_search),
            "{} vs {}",
            kv.Key,
            hex_search
        );
        count += 1;
    }
    // Go Search 会跨数据文件汇集两个 CF，再把 aa_big Default 合入 Write。
    assert_eq!(count, 1);
    assert_eq!(out[0].Value, "YWFfYmlnX3ZhbF8x");
    assert!(out[0].ShortValue.is_empty());
}

/// 直接测 MergeCFEntriesForTest：Write 全保留 + 未合并 Default 一条。
#[test]
fn test_merge_cf_entries() {
    let (default_cfs, write_cfs) = fake_cfs();
    let mut default_entries = HashMap::new();
    // 手工装填，绕过文件 IO。
    let mut write_entries = HashMap::new();
    for c in &default_cfs {
        // Map 键使用完整编码键 hex。
        let encoded = hex::encode(encode_key(&c.key, c.start_ts));
        default_entries.insert(
            encoded.clone(),
            StreamKVInfo {
                Key: hex::encode(c.key.as_bytes()),
                EncodedKey: encoded,
                StartTs: c.start_ts as u64,
                CFName: DefaultCF.into(),
                Value: c.val.clone(),
                ..Default::default()
            },
        );
    }
    for c in &write_cfs {
        let encoded = hex::encode(encode_key(&c.key, c.commit_ts));
        write_entries.insert(
            encoded.clone(),
            StreamKVInfo {
                Key: hex::encode(c.key.as_bytes()),
                EncodedKey: encoded,
                StartTs: c.start_ts as u64,
                CommitTs: c.commit_ts as u64,
                CFName: WriteCF.into(),
                Value: c.val.clone(),
                ..Default::default()
            },
        );
    }
    // 本测不读盘，Storage 仅满足构造签名。
    let s = Arc::new(MemStorage::new()) as Arc<dyn Storage>;
    let bs = NewStreamBackupSearch(s, NewStartWithComparator(), vec![]);
    let kv_entries = bs.MergeCFEntriesForTest(default_entries, write_entries);
    // write 全量 + 未被配对的 cc Default = write.len()+1。
    assert_eq!(write_cfs.len() + 1, kv_entries.len());
}
