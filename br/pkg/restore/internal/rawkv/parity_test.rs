// Copyright 2026 AsterSQL.

//! 与 Go rawkv 包对齐的契约测试：批量刷盘、去重、错误与资源清理。
//! 使用 FakeRawkvClient / RecordingDialer，不连真实 PD/TiKV。
//! 覆盖：满批 flush、PutRest、同键保留更大 originTs、BatchPut 失败、Close。
//! 指标观测与 10s 超时转发亦在此断言。

// 场景清单：
// 1) 满批自动 flush，并校验 SetColumnFamily 出现在 options。
// 2) PutRest 刷出未满批的残余键值。
// 3) 批内同逻辑键仅保留更大 originTs。
// 4) 跨批同键可出现多版本（与 Go 行为一致）。
// 5) 更小 originTs 不得覆盖缓冲中的新值。
// 6) BatchPut 错误文案原样回传，且无脏写。
// 7) 默认 dialer 未配置时 NewRawkvClient 失败。
// 8) WithDialer 转发 RAWKV_CUSTOM_TIMEOUT 与 PD 地址。
// 9) Close 恰好关闭底层客户端一次。
// 10) 指标观测记录 default CF 的 batch size 与耗时。
// 比较前对 got/want 按 key 排序，规避 HashMap 无序。
// FakeRawkvClient 可注入 batch_put_err 模拟 TiKV 失败。
// RecordingDialer 捕获 timeout/addrs 供契约断言。
// encode_uint_desc 构造带降序 TS 后缀的测试键。
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// 本文件聚合正常/边界/错误/清理四类场景，对应 Go 多用例。
// Fake 仅实现测试需要的 RawkvClient 方法，其余返回空成功。
// encode_uint_desc 保证不同 TS 共享相同逻辑前缀，便于去重断言。
// 排序比较消除 HashMap 迭代顺序不确定性。
// RecordingDialer 验证 PD 地址列表原样转发。
// batch_count=3 取自 Go TestRawKVBatchClient 固定容量。
// SetColumnFamily 必须在 Put 前调用，否则 CF option 为空串。
// 跨批重复键可并存多个版本，与批内去重策略不同。
use crate::{
    Context, Error, NewRawKVBatchClient, NewRawkvClient, NewRawkvClientWithDialer, PdRawkvDialer,
    RAWKV_CUSTOM_TIMEOUT, RawKVBatchClient, RawOption, RawkvClient, Result, Security,
    SetColumnFamily, metrics,
};

// 测试用 KV 条目，对齐 Go kv.Entry。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    key: Vec<u8>,
    value: Vec<u8>,
}

// 内存 Fake：记录 BatchPut、可注入错误、统计 Close。
struct FakeRawkvClient {
    kvs: Mutex<Vec<Entry>>,
    last_options: Mutex<Vec<RawOption>>,
    batch_put_err: Mutex<Option<Error>>,
    closed: AtomicBool,
    close_calls: AtomicUsize,
}

impl FakeRawkvClient {
    // 默认无错误、未关闭。
    fn new() -> Arc<Self> {
        Arc::new(Self {
            kvs: Mutex::new(Vec::new()),
            last_options: Mutex::new(Vec::new()),
            batch_put_err: Mutex::new(None),
            closed: AtomicBool::new(false),
            close_calls: AtomicUsize::new(0),
        })
    }

    // 注入 BatchPut 失败，用于错误路径。
    fn set_batch_put_err(&self, err: Option<Error>) {
        *self.batch_put_err.lock().unwrap() = err;
    }

    // 返回已写入条目快照。
    fn entries(&self) -> Vec<Entry> {
        self.kvs.lock().unwrap().clone()
    }
}

impl RawkvClient for FakeRawkvClient {
    // Get/Put/BatchGet 在本测试中不使用，返回空成功。
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
        options: &[RawOption],
    ) -> Result<()> {
        // 长度不一致直接失败，模拟 Go 侧校验。
        if keys.len() != values.len() {
            return Err(Error::new(
                "the length of keys don't equal the length of values",
            ));
        }
        // 优先返回注入错误，不写入。
        if let Some(err) = self.batch_put_err.lock().unwrap().clone() {
            return Err(err);
        }
        // 记录 options（含 SetColumnFamily）供断言。
        *self.last_options.lock().unwrap() = options.to_vec();
        let mut kvs = self.kvs.lock().unwrap();
        for i in 0..keys.len() {
            kvs.push(Entry {
                key: keys[i].clone(),
                value: values[i].clone(),
            });
        }
        Ok(())
    }

    // Close 置位并计数，验证 batch.Close 透传。
    fn Close(&self) -> Result<()> {
        self.closed.store(true, Ordering::SeqCst);
        self.close_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// 对齐 `codec.EncodeUintDesc`：前缀 + 取反大端 u64，用作带 TS 的 key。
fn encode_uint_desc(prefix: &[u8], v: u64) -> Vec<u8> {
    let mut b = prefix.to_vec();
    b.extend_from_slice(&(!v).to_be_bytes());
    b
}

// 记录 NewClient 收到的 timeout/addrs，验证自定义 10s 超时转发。
struct RecordingDialer {
    last_timeout: Mutex<Option<Duration>>,
    last_addrs: Mutex<Vec<String>>,
    client: Arc<FakeRawkvClient>,
}

impl PdRawkvDialer for RecordingDialer {
    fn NewClient(
        &self,
        _ctx: &Context,
        pd_addrs: &[String],
        _security: &Security,
        timeout: Duration,
    ) -> Result<Arc<dyn RawkvClient>> {
        *self.last_timeout.lock().unwrap() = Some(timeout);
        *self.last_addrs.lock().unwrap() = pd_addrs.to_vec();
        Ok(self.client.clone())
    }
}

#[test]
fn go_rust_public_contract_matches() {
    // 清理指标，避免与其他用例交叉污染。
    metrics::clear_observations();

    // --- 正常：满批 flush + PutRest 刷残余（对齐 TestRawKVBatchClient）---
    let fake = FakeRawkvClient::new();
    let batch_count = 3;
    let mut client = NewRawKVBatchClient(fake.clone(), batch_count);
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
    // 前 batch_count-1 次 Put 不应触发 BatchPut。
    for i in 0..batch_count as usize {
        assert_eq!(fake.entries().len(), 0);
        client
            .Put(&ctx, &kvs[i].key, &kvs[i].value, (i as u64) + 1)
            .unwrap();
    }
    // 第 batch_count 次后应已 flush。
    assert_eq!(fake.entries().len(), batch_count as usize);
    let opts = fake.last_options.lock().unwrap().clone();
    // CF option 必须为 default。
    assert_eq!(opts, vec![SetColumnFamily("default")]);

    // 继续 Put：缓冲未满前底层条目数不变。
    for i in batch_count as usize..kvs.len() {
        client
            .Put(&ctx, &kvs[i].key, &kvs[i].value, (i as u64) + 1)
            .unwrap();
    }
    assert_eq!(fake.entries().len(), batch_count as usize);
    // PutRest 刷出残余 2 条。
    client.PutRest(&ctx).unwrap();

    let mut got = fake.entries();
    got.sort_by(|a, b| a.key.cmp(&b.key));
    let mut want = kvs.clone();
    want.sort_by(|a, b| a.key.cmp(&b.key));
    assert_eq!(got, want);

    // 指标：至少一次 batch size=3、且记录了 default CF 耗时。
    let batch_obs = metrics::take_batch_size_observations();
    assert!(
        batch_obs
            .iter()
            .any(|o| o.cf == "default" && o.value == 3.0)
    );
    let dur_obs = metrics::take_duration_observations();
    assert!(dur_obs.iter().any(|o| o.cf == "default"));

    // --- 边界：空 PutRest 为 no-op；批内重复键保留更大 originTs ---
    let fake2 = FakeRawkvClient::new();
    let mut client2 = NewRawKVBatchClient(fake2.clone(), batch_count);
    client2.SetColumnFamily("default");
    // 空缓冲 PutRest 不产生写入。
    client2.PutRest(&ctx).unwrap();
    assert_eq!(fake2.entries().len(), 0);

    // key1/key4 各有两个 TS；批内去重后期望保留较大 TS。
    let dup_kvs = vec![
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
    let expected = vec![
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

    for i in 0..batch_count as usize {
        assert_eq!(fake2.entries().len(), 0);
        client2
            .Put(&ctx, &dup_kvs[i].key, &dup_kvs[i].value, (i as u64) + 1)
            .unwrap();
    }
    // 仅两个不同逻辑键缓冲，未满批故尚未发送。
    assert_eq!(fake2.entries().len(), 0);

    // 后续 Put 触发 flush；跨批的 key4 可保留两个版本。
    for i in batch_count as usize..5 {
        client2
            .Put(&ctx, &dup_kvs[i].key, &dup_kvs[i].value, (i as u64) + 1)
            .unwrap();
        assert_eq!(fake2.entries().len(), batch_count as usize);
    }
    client2.PutRest(&ctx).unwrap();
    let mut got2 = fake2.entries();
    got2.sort_by(|a, b| a.key.cmp(&b.key));
    let mut want2 = expected;
    want2.sort_by(|a, b| a.key.cmp(&b.key));
    assert_eq!(got2, want2);

    // 较小 originTs 不得覆盖缓冲中较新的值。
    let fake3 = FakeRawkvClient::new();
    let mut client3 = NewRawKVBatchClient(fake3.clone(), 10);
    let k = encode_uint_desc(b"same", 9);
    client3.Put(&ctx, &k, b"new", 20).unwrap();
    client3
        .Put(&ctx, &encode_uint_desc(b"same", 1), b"old", 5)
        .unwrap();
    client3.PutRest(&ctx).unwrap();
    assert_eq!(
        fake3.entries(),
        vec![Entry {
            key: k,
            value: b"new".to_vec()
        }]
    );

    // --- 错误：BatchPut 失败经 Trace 回传到 Put ---
    let fake_err = FakeRawkvClient::new();
    fake_err.set_batch_put_err(Some(Error::new("batch put failed")));
    let mut client_err = NewRawKVBatchClient(fake_err.clone(), 1);
    client_err.SetColumnFamily("write");
    let err = client_err
        .Put(&ctx, &encode_uint_desc(b"e", 1), b"v", 1)
        .unwrap_err();
    assert_eq!(err.msg, "batch put failed");
    // 失败时不应有成功写入。
    assert!(fake_err.entries().is_empty());

    // 未配置 dialer 的 NewRawkvClient 必须失败。
    let bare = NewRawkvClient(&ctx, &["127.0.0.1:2379".into()], &Security::default());
    assert!(bare.is_err());

    // WithDialer 必须转发 10s 超时（Go WithCustomTimeoutOption）。
    let dial_fake = FakeRawkvClient::new();
    let dialer = RecordingDialer {
        last_timeout: Mutex::new(None),
        last_addrs: Mutex::new(Vec::new()),
        client: dial_fake.clone(),
    };
    let built = NewRawkvClientWithDialer(
        &ctx,
        &["pd1:2379".into(), "pd2:2379".into()],
        &Security::default(),
        &dialer,
    )
    .unwrap();
    assert_eq!(
        *dialer.last_timeout.lock().unwrap(),
        Some(RAWKV_CUSTOM_TIMEOUT)
    );
    assert_eq!(
        *dialer.last_addrs.lock().unwrap(),
        vec!["pd1:2379".to_string(), "pd2:2379".to_string()]
    );
    // 行使返回客户端的 trait 表面（Put/BatchPut）。
    built.Put(&ctx, b"k", b"v", &[]).unwrap();
    built
        .BatchPut(&ctx, &[b"a".to_vec()], &[b"b".to_vec()], &[])
        .unwrap();

    // --- 资源清理：Close 关闭底层 rawkv client ---
    let fake_close = FakeRawkvClient::new();
    let batch: RawKVBatchClient = NewRawKVBatchClient(fake_close.clone(), 2);
    assert!(!fake_close.closed.load(Ordering::SeqCst));
    batch.Close();
    assert!(fake_close.closed.load(Ordering::SeqCst));
    // 恰好调用一次 Close。
    assert_eq!(fake_close.close_calls.load(Ordering::SeqCst), 1);
}
