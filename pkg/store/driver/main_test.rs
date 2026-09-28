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

// store driver 测试辅助：内存 KV 快照准备与清理。
//
// 对应 Go TestMain 周边的 store 数据初始化工具：向 BTreeMap 写入键值并构造
// `tikvSnapshot`，以及清空存储后验证 Get 返回 not-found。

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use astersql_store_driver_txn as txn;

#[derive(Clone, Copy)]
enum ByteInput<'a> {
    Nil,
    String(&'a str),
    Bytes(&'a [u8]),
}

/// 将 nil、字符串或原始字节转为可选字节键/值（对齐 Go makeBytes）。
fn make_bytes(value: ByteInput<'_>) -> Option<Vec<u8>> {
    match value {
        ByteInput::String(value) => Some(value.as_bytes().to_vec()),
        ByteInput::Bytes(value) => Some(value.to_vec()),
        ByteInput::Nil => None,
    }
}

/// 清空底层 store 映射，使已有快照读到 not-found。
fn clear_store_data(storage: &Arc<RwLock<BTreeMap<txn::Key, txn::ValueEntry>>>) {
    storage.write().unwrap().clear();
}

/// 写入若干键值并返回指向同一存储的快照（start_ts 固定为 1）。
fn prepare_snapshot(
    storage: &Arc<RwLock<BTreeMap<txn::Key, txn::ValueEntry>>>,
    data: &[(ByteInput<'_>, ByteInput<'_>)],
) -> txn::tikvSnapshot {
    {
        let mut map = storage.write().unwrap();
        for (key, value) in data {
            // ValueEntry::new(value, start_ts)：MVCC 版本条目，此处用固定 ts=1。
            map.insert(
                make_bytes(*key).unwrap_or_default(),
                txn::ValueEntry::new(make_bytes(*value).unwrap_or_default(), 1),
            );
        }
    }
    txn::NewSnapshot(Arc::clone(storage))
}

/// 准备快照可读，清空后再 Get 应变为 not-found。
#[test]
fn TestMainHelpersPrepareAndClearStoreData() {
    let storage = Arc::new(RwLock::new(BTreeMap::new()));
    storage.write().unwrap().insert(
        b"existing".to_vec(),
        txn::ValueEntry::new(b"kept".to_vec(), 1),
    );
    let snap = prepare_snapshot(
        &storage,
        &[
            (ByteInput::String("a"), ByteInput::String("1")),
            (ByteInput::Bytes(b"b"), ByteInput::Bytes(b"2")),
        ],
    );
    assert_eq!(snap.Get(b"a", &[]).unwrap().value, b"1");
    assert_eq!(snap.Get(b"existing", &[]).unwrap().value, b"kept");
    clear_store_data(&storage);
    assert!(snap.Get(b"a", &[]).unwrap_err().is_not_found());
    assert!(storage.read().unwrap().is_empty());
}

/// make_bytes 保留字符串/字节载荷，并将 Go nil 区分于空切片。
#[test]
fn makeBytes_preserves_string_bytes() {
    assert_eq!(make_bytes(ByteInput::String("k1")).unwrap(), b"k1");
    assert_eq!(make_bytes(ByteInput::String("")).unwrap(), b"");
    assert_eq!(make_bytes(ByteInput::Bytes(b"\0raw")).unwrap(), b"\0raw");
    assert_eq!(make_bytes(ByteInput::Nil), None);
}
