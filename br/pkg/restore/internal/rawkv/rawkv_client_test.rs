// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! 对齐 Go `rawkv_client_test.go` 的单元测试。
//! `FakeRawkvClient` 为内存桩：只记录 BatchPut，无 PD/TiKV/网络。
//! 键用本地 EncodeUintDesc，对应 `pkg/util/codec.EncodeUintDesc`。
//! 覆盖满批 flush、PutRest 残余，以及批内重复键保留更大 TS。

// 本文件对应 Go 同名测试，不扩展额外场景。
// originTs 取自循环下标+1，保证单调便于去重。
// Close 在用例末尾调用，验证资源路径可走通。
// 排序比较消除 map 迭代顺序差异。
// batch_count 固定为 3，与 Go 常量一致。
// 重复键用例期望列表含跨批的 key4 双版本。
// Fake 不检查 options，CF 行为由实现与 parity 覆盖。

// 额外说明：
// - 五元组输入覆盖 flush 边界（3+2）。
// - duplicated 用例验证批内去重与跨批多版本并存。
// - expect 文案保留 Go require.Nil 语义提示。
// - 不引入网络或 PD，保证单测可在 darwin 本地运行。
// - 与 parity_test 互补：本文件贴近 Go 原测试结构。
// - Put 的 originTs 参与去重比较，而非编码进 key 的比较主键。
use std::sync::{Arc, Mutex};

use crate::{Context, Error, NewRawKVBatchClient, RawOption, RawkvClient, Result};

/// 对齐 Go `kv.Entry` 的测试条目。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    key: Vec<u8>,
    value: Vec<u8>,
}

/// 内存 mock：仅累积 BatchPut 的 key/value。
struct FakeRawkvClient {
    kvs: Mutex<Vec<Entry>>,
}

// 构造空假客户端。
fn new_fake_rawkv_client() -> Arc<FakeRawkvClient> {
    Arc::new(FakeRawkvClient {
        kvs: Mutex::new(Vec::new()),
    })
}

impl FakeRawkvClient {
    // 快照已写入条目。
    fn entries(&self) -> Vec<Entry> {
        self.kvs.lock().unwrap().clone()
    }

    // 当前条数，用于断言是否已 flush。
    fn len(&self) -> usize {
        self.kvs.lock().unwrap().len()
    }
}

impl RawkvClient for FakeRawkvClient {
    // 本测试不走 Get/Put/BatchGet，返回空成功即可。
    fn Get(&self, _ctx: &Context, _key: &[u8], _options: &[RawOption]) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn Put(
        &self,
        _ctx: &Context,
        _key: &[u8],
        _value: &[u8],
        _options: &[RawOption],
    ) -> Result<()> {
        Ok(())
    }

    fn BatchGet(
        &self,
        _ctx: &Context,
        _keys: &[Vec<u8>],
        _options: &[RawOption],
    ) -> Result<Vec<Vec<u8>>> {
        Ok(Vec::new())
    }

    fn BatchPut(
        &self,
        _ctx: &Context,
        keys: &[Vec<u8>],
        values: &[Vec<u8>],
        _options: &[RawOption],
    ) -> Result<()> {
        // 与 Go fake 相同：长度不一致则报错。
        if keys.len() != values.len() {
            return Err(Error::new(
                "the length of keys don't equal the length of values",
            ));
        }
        let mut kvs = self.kvs.lock().unwrap();
        for i in 0..keys.len() {
            kvs.push(Entry {
                key: keys[i].clone(),
                value: values[i].clone(),
            });
        }
        Ok(())
    }

    // Close 在此为 no-op，仅满足 trait。
    fn Close(&self) -> Result<()> {
        Ok(())
    }
}

/// 对齐 codec.EncodeUintDesc：前缀 + 取反大端时间戳。
fn encode_uint_desc(prefix: &[u8], v: u64) -> Vec<u8> {
    let mut b = prefix.to_vec();
    b.extend_from_slice(&(!v).to_be_bytes());
    b
}

/// 对齐 Go TestRawKVBatchClient：满批 flush，PutRest 写残余。
#[test]
fn test_raw_kv_batch_client() {
    let fake = new_fake_rawkv_client();
    let batch_count = 3;
    let mut client = NewRawKVBatchClient(fake.clone(), batch_count);
    // 与 Go 用例一致使用 default CF。
    client.SetColumnFamily("default");

    let kvs = vec![
        Entry {
            key: encode_uint_desc(b"key1", 1),
            value: b"v1".to_vec(),
        },
        Entry {
            key: encode_uint_desc(b"key2", 2),
            value: b"v2".to_vec(),
        },
        Entry {
            key: encode_uint_desc(b"key3", 3),
            value: b"v3".to_vec(),
        },
        Entry {
            key: encode_uint_desc(b"key4", 4),
            value: b"v4".to_vec(),
        },
        Entry {
            key: encode_uint_desc(b"key5", 5),
            value: b"v5".to_vec(),
        },
    ];

    let ctx = Context::TODO();
    // 未满批前底层长度为 0。
    for i in 0..batch_count as usize {
        assert_eq!(0, fake.len());
        client
            .Put(&ctx, &kvs[i].key, &kvs[i].value, (i as u64) + 1)
            .expect("Go require.Nil: put before first flush");
    }
    // 第 3 次 Put 触发 flush。
    assert_eq!(batch_count as usize, fake.len());

    // 继续缓冲剩余键，长度仍为上一批大小直至 PutRest。
    for i in batch_count as usize..kvs.len() {
        client
            .Put(&ctx, &kvs[i].key, &kvs[i].value, (i as u64) + 1)
            .expect("Go require.Nil: put remaining kvs");
    }
    assert_eq!(batch_count as usize, fake.len());
    client.PutRest(&ctx).expect("Go require.Nil: flush rest");

    // 排序后应与输入五元组一致。
    let mut got = fake.entries();
    got.sort_by(|a, b| a.key.cmp(&b.key));
    assert_eq!(kvs, got);
    client.Close();
}

/// 对齐 Go TestRawKVBatchClientDuplicated：批内同键保留更大 originTs。
#[test]
fn test_raw_kv_batch_client_duplicated() {
    let fake = new_fake_rawkv_client();
    let batch_count = 3;
    let mut client = NewRawKVBatchClient(fake.clone(), batch_count);
    client.SetColumnFamily("default");

    let kvs = vec![
        Entry {
            key: encode_uint_desc(b"key1", 1),
            value: b"v1".to_vec(),
        },
        Entry {
            key: encode_uint_desc(b"key1", 2),
            value: b"v2".to_vec(),
        },
        Entry {
            key: encode_uint_desc(b"key3", 3),
            value: b"v3".to_vec(),
        },
        Entry {
            key: encode_uint_desc(b"key4", 4),
            value: b"v4".to_vec(),
        },
        Entry {
            key: encode_uint_desc(b"key4", 5),
            value: b"v5".to_vec(),
        },
    ];

    // 批内去重保留大 TS；跨批的 key4 最终可有两个版本。
    // batch_count=3：前三个输入去重后仅 2 键，故尚未发送。
    let expected_kvs = vec![
        Entry {
            key: encode_uint_desc(b"key1", 2),
            value: b"v2".to_vec(),
        },
        Entry {
            key: encode_uint_desc(b"key3", 3),
            value: b"v3".to_vec(),
        },
        Entry {
            key: encode_uint_desc(b"key4", 5),
            value: b"v5".to_vec(),
        },
        Entry {
            key: encode_uint_desc(b"key4", 4),
            value: b"v4".to_vec(),
        },
    ];

    let ctx = Context::TODO();
    for i in 0..batch_count as usize {
        assert_eq!(0, fake.len());
        client
            .Put(&ctx, &kvs[i].key, &kvs[i].value, (i as u64) + 1)
            .expect("Go require.Nil: buffered duplicated put");
    }
    // 两个不同逻辑键仍缓冲中，尚未发往 Fake。
    assert_eq!(0, fake.len());

    // 后续 Put 触发 flush；之后长度保持为 batch_count。
    for i in batch_count as usize..5 {
        client
            .Put(&ctx, &kvs[i].key, &kvs[i].value, (i as u64) + 1)
            .expect("Go require.Nil: put triggers batch flush");
        assert_eq!(batch_count as usize, fake.len());
    }

    client
        .PutRest(&ctx)
        .expect("Go require.Nil: flush duplicated rest");
    // 与 Go expected_kvs 排序比较。
    let mut got = fake.entries();
    got.sort_by(|a, b| a.key.cmp(&b.key));
    assert_eq!(expected_kvs, got);
    client.Close();
}
