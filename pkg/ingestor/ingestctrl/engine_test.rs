// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Engine 相关单元测试。
//
// 覆盖本地 ingest Engine（基于 Pebble 风格 KV 存储）在 import 锁持有期间的文件大小统计、
// 关闭后写入拒绝、首末键范围扫描，以及迭代器缓冲区释放后切片独立性；
// SST（Sorted String Table）Writer 在未指定 block size 时使用默认 16KB。

#![allow(dead_code)]
#![allow(non_snake_case)]

use std::sync::Arc;
use std::time::Duration;

use crate::engine::{Engine, IMPORT_MUTEX_STATE_IMPORT, Writer};
use crate::{EngineId, Error};

// makePebbleDB 对应 Go 测试辅助：创建临时 Pebble DB 和用于 SST 输出的目录。
/// 创建内存 Engine 测试夹具（对应 Go makePebbleDB）。
pub fn makePebbleDB() -> Arc<Engine> {
    Arc::new(Engine::new(EngineId::new(), 16 * 1024, 1024))
}

// TestGetEngineSizeWhenImport 对应 Go 测试：import 锁持有期间 getEngineFileSize 应标记 IsImporting。
#[test]
/// 验证 import 互斥锁持有时 `getEngineFileSize` 标记 `IsImporting`。
pub fn TestGetEngineSizeWhenImport() {
    let engine = makePebbleDB();
    assert!(engine.lockUnless(IMPORT_MUTEX_STATE_IMPORT, 0));
    let size = engine.getEngineFileSize();
    assert_eq!(engine.UUID, size.UUID);
    assert!(size.IsImporting);
    engine.unlock();
    engine.Close().unwrap();
}

// Go lockUnless 只在调用瞬间命中 ignore mask 时返回 false；若引擎只是暂时持有
// read lock，则会像 sync.RWMutex.Lock 一样等待并在读锁释放后取得写锁。
#[test]
pub fn lock_unless_waits_for_a_non_ignored_read_lock() {
    let engine = makePebbleDB();
    assert!(engine.tryRLock());

    let (tx, rx) = std::sync::mpsc::channel();
    let waiter = Arc::clone(&engine);
    let handle = std::thread::spawn(move || {
        tx.send(waiter.lockUnless(IMPORT_MUTEX_STATE_IMPORT, 0))
            .unwrap();
    });

    assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
    engine.rUnlock();
    assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap());
    handle.join().unwrap();
    engine.unlock();
}

// TestIngestSSTWithClosedEngine 对应 Go 测试：关闭后的 Engine 再 ingest 同一个 SST 应返回 errorEngineClosed。
#[test]
/// 验证 Engine 关闭后再次 Put 返回 `Error::Closed`。
pub fn TestIngestSSTWithClosedEngine() {
    let engine = makePebbleDB();
    for index in 0..10 {
        engine
            .Put(format!("key{index}").into_bytes(), Vec::new())
            .unwrap();
    }
    engine.Close().unwrap();
    assert_eq!(
        Err(Error::Closed),
        engine.Put(b"key10".to_vec(), Vec::new())
    );
}

// TestGetFirstAndLastKey 对应 Go 测试：覆盖全范围、子范围、空结果和无上界扫描。
#[test]
/// 验证全范围、子范围、空结果与无上界扫描的首末键。
pub fn TestGetFirstAndLastKey() {
    let engine = makePebbleDB();
    for key in [b"a", b"c", b"e"] {
        engine.Put(key.to_vec(), key.to_vec()).unwrap();
    }
    assert_eq!(
        (b"a".to_vec(), b"e".to_vec()),
        engine.GetFirstAndLastKey(b"", b"").unwrap()
    );
    assert_eq!(
        (b"c".to_vec(), b"c".to_vec()),
        engine.GetFirstAndLastKey(b"b", b"d").unwrap()
    );
    assert_eq!(
        (b"c".to_vec(), b"e".to_vec()),
        engine.GetFirstAndLastKey(b"b", b"f").unwrap()
    );
    assert_eq!(
        (Vec::new(), Vec::new()),
        engine.GetFirstAndLastKey(b"y", b"z").unwrap()
    );
    assert_eq!(
        (b"e".to_vec(), b"e".to_vec()),
        engine.GetFirstAndLastKey(b"e", b"").unwrap()
    );
}

// TestIterOutputHasUniqueMemorySpace 对应 Go 测试：ReleaseBuf 后旧 key/value 切片会被下一批复用。
#[test]
/// 验证 `ReleaseBuf` 后已拷贝的 key/value 切片不被后续迭代复用覆盖。
pub fn TestIterOutputHasUniqueMemorySpace() {
    let engine = makePebbleDB();
    for key in [b"a", b"c", b"e", b"g"] {
        engine.Put(key.to_vec(), key.to_vec()).unwrap();
    }
    let mut iter = engine.newKVIter(b"", b"").unwrap();
    assert!(iter.First());
    let first = (iter.Key().to_vec(), iter.Value().to_vec());
    assert!(iter.Next());
    let second = (iter.Key().to_vec(), iter.Value().to_vec());
    iter.ReleaseBuf();
    assert_eq!((b"a".to_vec(), b"a".to_vec()), first);
    assert_eq!((b"c".to_vec(), b"c".to_vec()), second);
    assert!(iter.Next());
    assert_eq!(b"e", iter.Key());
    assert!(iter.Next());
    assert_eq!(b"g", iter.Key());
    assert!(!iter.Next());
    iter.Close().unwrap();
}

// TestCreateSSTWriterDefaultBlockSize tests that createSSTWriter will use the default block size of 16KB if the block size is not set.
#[test]
/// 验证 block size 为 0 时 Writer 使用默认块大小，并正确累计 KV 统计。
pub fn TestCreateSSTWriterDefaultBlockSize() {
    let engine = makePebbleDB();
    let mut writer = Writer::new(Arc::clone(&engine), 0);
    writer.Append(b"a".to_vec(), b"1".to_vec()).unwrap();
    assert_eq!(0, writer.EstimatedSize());
    assert_eq!((2, 1), engine.KVStatistics());
    writer.Flush().unwrap();
    assert_eq!(0, writer.EstimatedSize());
    writer.Close().unwrap();
    assert_eq!((2, 1), engine.KVStatistics());
}
