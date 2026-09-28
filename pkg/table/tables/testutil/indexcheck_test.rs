// Copyright 2026 AsterSQL.

// 索引 KV 数量检查工具的单元测试。
//
// 通过记录调用顺序的内存运行时，验证扫描边界、资源关闭与事务提交的 Go 兼容语义，
// 避免依赖真实存储或会话环境。

use super::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// 可控制键序列与关闭结果的快照迭代器，并记录资源操作顺序。
struct MockIterator {
    keys: Vec<Vec<u8>>,
    position: usize,
    close_error: Option<IndexCheckError>,
    calls: Rc<RefCell<Vec<String>>>,
}

impl SnapshotIterator for MockIterator {
    fn valid(&self) -> bool {
        self.position < self.keys.len()
    }

    fn key(&self) -> &[u8] {
        &self.keys[self.position]
    }

    fn next(&mut self) -> Result<(), IndexCheckError> {
        self.calls.borrow_mut().push("next".into());
        self.position += 1;
        Ok(())
    }

    fn close(&mut self) -> Result<(), IndexCheckError> {
        self.calls.borrow_mut().push("close".into());
        self.close_error.clone().map_or(Ok(()), Err)
    }
}

/// 为索引检查注入固定元数据、解码结果和事务结果的内存运行时。
struct MockRuntime {
    calls: Rc<RefCell<Vec<String>>>,
    iterator: Option<Box<dyn SnapshotIterator>>,
    decoded_ids: HashMap<Vec<u8>, Option<i64>>,
    commit_error: Option<IndexCheckError>,
}

impl MockRuntime {
    fn new(
        iterator: Box<dyn SnapshotIterator>,
        decoded_ids: HashMap<Vec<u8>, Option<i64>>,
        calls: Rc<RefCell<Vec<String>>>,
    ) -> Self {
        Self {
            calls,
            iterator: Some(iterator),
            decoded_ids,
            commit_error: None,
        }
    }
}

impl IndexCheckRuntime for MockRuntime {
    fn index_meta(
        &mut self,
        database: &str,
        table: &str,
        index: &str,
    ) -> Result<IndexMeta, IndexCheckError> {
        self.calls
            .borrow_mut()
            .push(format!("index_meta:{database}.{table}.{index}"));
        Ok(IndexMeta {
            table_id: 1,
            index_id: 42,
            minimum_key: b"first".to_vec(),
        })
    }

    fn begin(&mut self) -> Result<(), IndexCheckError> {
        self.calls.borrow_mut().push("begin".into());
        Ok(())
    }

    fn commit(&mut self) -> Result<(), IndexCheckError> {
        self.calls.borrow_mut().push("commit".into());
        self.commit_error.clone().map_or(Ok(()), Err)
    }

    fn snapshot_iter(
        &mut self,
        start: &[u8],
    ) -> Result<Box<dyn SnapshotIterator>, IndexCheckError> {
        self.calls
            .borrow_mut()
            .push(format!("snapshot_iter:{}", String::from_utf8_lossy(start)));
        Ok(self
            .iterator
            .take()
            .expect("iterator should be requested once"))
    }

    fn decode_index_id(&self, key: &[u8]) -> Result<Option<i64>, IndexCheckError> {
        self.calls
            .borrow_mut()
            .push(format!("decode:{}", String::from_utf8_lossy(key)));
        Ok(*self.decoded_ids.get(key).unwrap_or(&None))
    }
}

#[test]
// Go 的 `defer iter.Close()` 不检查返回值，关闭失败不能覆盖成功的扫描结果。
fn check_index_kv_count_ignores_iterator_close_error_like_go_defer() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let iterator = MockIterator {
        keys: vec![b"first".to_vec()],
        position: 0,
        close_error: Some(IndexCheckError("close failed".into())),
        calls: calls.clone(),
    };
    let mut runtime = MockRuntime::new(
        Box::new(iterator),
        HashMap::from([(b"first".to_vec(), Some(42))]),
        calls.clone(),
    );

    CheckIndexKVCount(&mut runtime, "orders", "by_customer", 1)
        .expect("Go ignores the deferred iterator Close result");
    assert_eq!(
        *calls.borrow(),
        [
            "index_meta:test.orders.by_customer",
            "begin",
            "snapshot_iter:first",
            "decode:first",
            "next",
            "close",
            "commit",
        ]
    );
}

#[test]
// 扫描到其它索引 ID 时必须停下，不能把相邻索引前缀的键计入结果。
fn check_index_kv_count_stops_at_the_next_index_prefix() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let iterator = MockIterator {
        keys: vec![
            b"first".to_vec(),
            b"second".to_vec(),
            b"other-index".to_vec(),
        ],
        position: 0,
        close_error: None,
        calls: calls.clone(),
    };
    let mut runtime = MockRuntime::new(
        Box::new(iterator),
        HashMap::from([
            (b"first".to_vec(), Some(42)),
            (b"second".to_vec(), Some(42)),
            (b"other-index".to_vec(), Some(43)),
        ]),
        calls.clone(),
    );

    CheckIndexKVCount(&mut runtime, "orders", "by_customer", 2).unwrap();
    assert_eq!(
        *calls.borrow(),
        [
            "index_meta:test.orders.by_customer",
            "begin",
            "snapshot_iter:first",
            "decode:first",
            "next",
            "decode:second",
            "next",
            "decode:other-index",
            "close",
            "commit",
        ]
    );
}

#[test]
// 数量不符也应先关闭迭代器并提交事务，再向调用方返回计数错误。
fn check_index_kv_count_reports_count_mismatch_after_close_and_commit() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let iterator = MockIterator {
        keys: vec![b"first".to_vec()],
        position: 0,
        close_error: None,
        calls: calls.clone(),
    };
    let mut runtime = MockRuntime::new(
        Box::new(iterator),
        HashMap::from([(b"first".to_vec(), Some(42))]),
        calls.clone(),
    );

    let error = CheckIndexKVCount(&mut runtime, "orders", "by_customer", 2).unwrap_err();
    assert_eq!(
        error.to_string(),
        "index orders.by_customer contains 1 KV pairs, expected 2"
    );
    assert_eq!(
        calls.borrow().as_slice(),
        [
            "index_meta:test.orders.by_customer",
            "begin",
            "snapshot_iter:first",
            "decode:first",
            "next",
            "close",
            "commit",
        ]
    );
}

#[test]
// 扫描成功不应吞掉提交错误，提交阶段失败仍需原样返回。
fn check_index_kv_count_returns_commit_error_after_a_successful_scan() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let iterator = MockIterator {
        keys: vec![],
        position: 0,
        close_error: None,
        calls: calls.clone(),
    };
    let mut runtime = MockRuntime::new(Box::new(iterator), HashMap::new(), calls.clone());
    runtime.commit_error = Some(IndexCheckError("commit failed".into()));

    let error = CheckIndexKVCount(&mut runtime, "orders", "by_customer", 0).unwrap_err();
    assert_eq!(error, IndexCheckError("commit failed".into()));
    assert_eq!(
        calls.borrow().as_slice(),
        [
            "index_meta:test.orders.by_customer",
            "begin",
            "snapshot_iter:first",
            "close",
            "commit",
        ]
    );
}
