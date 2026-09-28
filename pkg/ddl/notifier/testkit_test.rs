// Copyright 2024 PingCAP, Inc.
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

// DDL Notifier 发布/订阅集成测试。
//
// 使用内存表存储模拟 `tidb_ddl_notifier`，覆盖：基本 pub/sub、投递顺序与清理、
// 多种 SchemaChangeEvent 类型、发布失败恢复、短时双 owner、分页 List、
// 悲观事务（pessimistic txn）错误路径以及提交失败时事件不得丢失等场景。

use crate::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// 构造仅含 ID/名称的简易 TableInfo，供事件工厂使用。
fn table(id: i64, name: &str) -> Option<Box<model::TableInfo>> {
    Some(Box::new(model::TableInfo {
        ID: id,
        Name: ast::NewCIStr(name),
        ..Default::default()
    }))
}

/// 在超时内轮询条件，用于等待异步 handler 完成。
fn eventually(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        condition(),
        "condition was not satisfied within {timeout:?}"
    );
}

/// 在整个观察窗口内持续断言条件成立，对应 Go `require.Never` 的反向契约。
fn consistently(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        assert!(condition(), "condition became false within {timeout:?}");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        condition(),
        "condition became false at the timeout boundary"
    );
}

fn fresh_store(db: &str) -> Arc<dyn Store> {
    static NEXT_STORE_ID: AtomicUsize = AtomicUsize::new(0);
    OpenTableStore(
        db,
        format!(
            "tidb_ddl_notifier_test_{}",
            NEXT_STORE_ID.fetch_add(1, Ordering::Relaxed)
        ),
    )
}

/// 验证向表存储发布多条变更后 List 可读回对应条数。
#[test]
fn test_publish_to_table_store() {
    let store = fresh_store("mysql");
    let session = Session::default();
    PubSchemeChangeToStore(
        &session,
        1,
        -1,
        NewCreateTableEvent(table(1000, "t1")),
        store.as_ref(),
    )
    .unwrap();
    PubSchemeChangeToStore(
        &session,
        2,
        -1,
        NewDropTableEvent(table(1001, "t2")),
        store.as_ref(),
    )
    .unwrap();
    let mut changes = vec![None; 8];
    let (mut result, close) = store.List(Session::default());
    assert_eq!(result.Read(&mut changes).unwrap(), 2);
    close();
}

/// 基本 pub/sub：handler 先返回 NotReady 再成功，最终应按发布顺序看到全部事件。
#[test]
fn test_basic_pub_sub() {
    let store = fresh_store("test");
    let notifier = NewDDLNotifier(
        SessionPool::default(),
        store.clone(),
        Duration::from_millis(5),
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    // 预设错误序列：None 成功、NotReady 重试、最后 EOF 等。
    let errors = Arc::new(Mutex::new(VecDeque::from([
        None,
        Some(ErrNotReadyRetryLater()),
        None,
        Some(ErrNotReadyRetryLater()),
        Some(Error::Message("EOF".to_owned())),
    ])));
    notifier.RegisterHandler(
        TestHandlerID,
        Box::new({
            let seen = seen.clone();
            move |_, event| {
                if let Some(error) = errors.lock().unwrap().pop_front().flatten() {
                    return Err(error);
                }
                seen.lock().unwrap().push(event.clone());
                Ok(())
            }
        }),
    );
    notifier.OnBecomeOwner();
    let session = Session::default();
    let event1 = NewCreateTableEvent(table(1000, "t1"));
    let event2 = NewDropTableEvent(table(1001, "t2#special-char?in'name"));
    let event3 = NewDropTableEvent(table(1002, "t3"));
    for (id, event) in [
        (1, event1.clone()),
        (2, event2.clone()),
        (3, event3.clone()),
    ] {
        PubSchemeChangeToStore(&session, id, -1, event, store.as_ref()).unwrap();
    }
    eventually(Duration::from_secs(1), || seen.lock().unwrap().len() == 3);
    assert_eq!(*seen.lock().unwrap(), vec![event1, event2, event3]);
    notifier.OnRetireOwner();
}

/// 多 handler 各失败一次后再成功：交付顺序一致，且全部处理后存储应清空。
#[test]
fn test_deliver_order_and_cleanup() {
    let store = fresh_store("test");
    let notifier = NewDDLNotifier(
        SessionPool::default(),
        store.clone(),
        Duration::from_millis(5),
    );
    let results: Vec<_> = (0..3).map(|_| Arc::new(Mutex::new(Vec::new()))).collect();
    for (id, result) in [3, 4, 9].into_iter().zip(results.iter().cloned()) {
        let remaining_failures = Arc::new(AtomicUsize::new(5));
        notifier.RegisterHandler(
            id,
            Box::new(move |_, event| {
                if remaining_failures
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok()
                {
                    return Err(ErrNotReadyRetryLater());
                }
                result
                    .lock()
                    .unwrap()
                    .push(event.GetCreateTableInfo().unwrap().ID);
                Ok(())
            }),
        );
    }
    notifier.OnBecomeOwner();
    let session = Session::default();
    for (job, id, name) in [(1, 1000, "t1"), (2, 1001, "t2"), (3, 1002, "t3")] {
        PubSchemeChangeToStore(
            &session,
            job,
            -1,
            NewCreateTableEvent(table(id, name)),
            store.as_ref(),
        )
        .unwrap();
    }
    eventually(Duration::from_secs(1), || store.Count() == 0);
    for result in results {
        assert_eq!(*result.lock().unwrap(), vec![1000, 1001, 1002]);
    }
    notifier.OnRetireOwner();
}

/// 覆盖分区/列/索引/删库等各类 SchemaChangeEvent，校验 handler 收到的类型序列。
#[test]
fn test_pub_sub() {
    let store = fresh_store("test");
    let notifier = NewDDLNotifier(
        SessionPool::default(),
        store.clone(),
        Duration::from_millis(5),
    );
    let types = Arc::new(Mutex::new(Vec::new()));
    notifier.RegisterHandler(
        TestHandlerID,
        Box::new({
            let types = types.clone();
            move |_, event| {
                types.lock().unwrap().push(event.GetType());
                Ok(())
            }
        }),
    );
    notifier.OnBecomeOwner();
    let empty_partition = || Some(Box::new(model::PartitionInfo::default()));
    let events = vec![
        NewCreateTableEvent(table(1, "t")),
        NewAddPartitioningEvent(1, table(2, "t"), empty_partition()),
        NewReorganizePartitionEvent(table(2, "t"), empty_partition(), empty_partition()),
        NewTruncatePartitionEvent(table(2, "t"), empty_partition(), empty_partition()),
        NewDropPartitionEvent(table(2, "t"), empty_partition()),
        NewAddPartitionEvent(table(2, "t"), empty_partition()),
        NewCreateTableEvent(table(3, "t1")),
        NewExchangePartitionEvent(table(2, "t"), empty_partition(), table(3, "t1")),
        NewRemovePartitioningEvent(2, table(4, "t"), empty_partition()),
        NewTruncateTableEvent(table(5, "t"), table(4, "t")),
        NewDropTableEvent(table(3, "t1")),
        NewModifyColumnEvent(
            table(5, "t"),
            vec![Box::new(model::ColumnInfo::default())],
            false,
        ),
        NewAddColumnEvent(table(5, "t"), vec![Box::new(model::ColumnInfo::default())]),
        NewAddIndexEvent(
            table(5, "t"),
            vec![Box::new(model::IndexInfo::default())],
            false,
        ),
        NewCreateTableEvent(table(6, "t1")),
        NewAddColumnEvent(table(6, "t1"), vec![Box::new(model::ColumnInfo::default())]),
        NewAddIndexEvent(
            table(6, "t1"),
            vec![Box::new(model::IndexInfo::default())],
            false,
        ),
        NewDropSchemaEvent(
            &model::DBInfo {
                ID: 1,
                Name: ast::NewCIStr("test"),
                ..Default::default()
            },
            vec![],
        ),
    ];
    let expected: Vec<_> = events.iter().map(SchemaChangeEvent::GetType).collect();
    let session = Session::default();
    for (index, event) in events.into_iter().enumerate() {
        PubSchemeChangeToStore(&session, index as i64 + 1, -1, event, store.as_ref()).unwrap();
    }
    eventually(Duration::from_secs(1), || {
        types.lock().unwrap().len() == expected.len()
    });
    assert_eq!(*types.lock().unwrap(), expected);
    notifier.OnRetireOwner();
}

/// 发布边界首个写入失败时应透传错误且不留事件；解除故障后同一 DDL 可成功发布。
#[test]
fn test_publish_event_error() {
    struct FailFirstInsertStore {
        inner: Arc<dyn Store>,
        fail_next: AtomicBool,
    }
    impl Store for FailFirstInsertStore {
        fn Insert(&self, session: &Session, change: &SchemaChange) -> Result<(), Error> {
            if self.fail_next.swap(false, Ordering::AcqRel) {
                return Err(Error::Message("mock publish event error".to_owned()));
            }
            self.inner.Insert(session, change)
        }

        fn UpdateProcessed(
            &self,
            session: &Session,
            ddl_job_id: i64,
            sub_job_id: i64,
            old_processed_by: u64,
            new_processed_by: u64,
        ) -> Result<(), Error> {
            self.inner.UpdateProcessed(
                session,
                ddl_job_id,
                sub_job_id,
                old_processed_by,
                new_processed_by,
            )
        }

        fn DeleteAndCommit(
            &self,
            session: &Session,
            ddl_job_id: i64,
            sub_job_id: i64,
        ) -> Result<(), Error> {
            self.inner.DeleteAndCommit(session, ddl_job_id, sub_job_id)
        }

        fn List(&self, session: Session) -> (Box<dyn ListResult>, CloseFn) {
            self.inner.List(session)
        }

        fn Count(&self) -> usize {
            self.inner.Count()
        }
    }

    let store = FailFirstInsertStore {
        inner: fresh_store("test"),
        fail_next: AtomicBool::new(true),
    };
    let session = Session::default();
    let event = NewCreateTableEvent(table(1, "t"));
    let error = PubSchemeChangeToStore(&session, 1, -1, event.clone(), &store).unwrap_err();
    assert_eq!(error.to_string(), "mock publish event error");
    assert_eq!(
        store.Count(),
        0,
        "failed publication must not persist an event"
    );

    PubSchemeChangeToStore(&session, 1, -1, event, &store).unwrap();
    assert_eq!(store.Count(), 1, "publication must recover after the fault");
}

/// 模拟短时双 owner：处理中另一侧删除行，CAS 更新失败且本地事务效果不得提交。
#[test]
fn test_2_owner_for_a_short_time() {
    let store = fresh_store("test");
    let notifier = NewDDLNotifier(
        SessionPool::default(),
        store.clone(),
        Duration::from_millis(5),
    );
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let handled_session = Arc::new(Mutex::new(None));
    notifier.RegisterHandler(
        TestHandlerID,
        Box::new({
            let handled_session = handled_session.clone();
            move |session, _| {
                assert!(session.IsPessimistic());
                session.StageEffect("insert result row")?;
                *handled_session.lock().unwrap() = Some(session.clone());
                // 阻塞直到测试线程模拟「另一 owner 已删除该行」。
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            }
        }),
    );
    notifier.OnBecomeOwner();
    PubSchemeChangeToStore(
        &Session::default(),
        1,
        -1,
        NewCreateTableEvent(table(1000, "t1")),
        store.as_ref(),
    )
    .unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    store.DeleteAndCommit(&Session::default(), 1, -1).unwrap();
    release_tx.send(()).unwrap();
    eventually(Duration::from_secs(1), || {
        notifier
            .Errors()
            .iter()
            .any(|error| error.contains("maybe the row has been updated by other owner"))
    });
    assert!(
        handled_session
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .CommittedEffects()
            .is_empty()
    );
    notifier.OnRetireOwner();
}

/// 缩小批大小后分页拉取；一个慢 handler 阻塞删除，放开后仍应处理完并清空存储。
#[test]
fn test_paginated_list() {
    let previous = ProcessEventsBatchSize.swap(3, Ordering::AcqRel);
    struct BatchSizeGuard(usize);
    impl Drop for BatchSizeGuard {
        fn drop(&mut self) {
            ProcessEventsBatchSize.store(self.0, Ordering::Release);
        }
    }
    let _batch_size_guard = BatchSizeGuard(previous);
    let store = fresh_store("test");
    let notifier = NewDDLNotifier(
        SessionPool::default(),
        store.clone(),
        Duration::from_millis(5),
    );
    let names = Arc::new(Mutex::new(Vec::new()));
    notifier.RegisterHandler(
        TestHandlerID,
        Box::new({
            let names = names.clone();
            move |_, event| {
                let name = match event.GetType() {
                    model::ACTION_CREATE_TABLE => event.GetCreateTableInfo().unwrap().Name.O,
                    model::ACTION_ADD_COLUMN => event.GetAddColumnInfo().1[0].Name.O.clone(),
                    other => panic!("unexpected event type: {other}"),
                };
                names.lock().unwrap().push(name);
                Ok(())
            }
        }),
    );
    let blocking = Arc::new(AtomicBool::new(true));
    let count = Arc::new(AtomicUsize::new(0));
    notifier.RegisterHandler(
        10,
        Box::new({
            let blocking = blocking.clone();
            let count = count.clone();
            move |_, _| {
                if blocking.load(Ordering::Acquire) {
                    return Err(ErrNotReadyRetryLater());
                }
                count.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
        }),
    );
    notifier.OnBecomeOwner();
    let session = Session::default();
    for id in 1..=4 {
        PubSchemeChangeToStore(
            &session,
            id,
            -1,
            NewCreateTableEvent(table(id, &format!("t{id}"))),
            store.as_ref(),
        )
        .unwrap();
    }
    for id in 5..=8 {
        let name = format!("c{id}");
        PubSchemeChangeToStore(
            &session,
            id,
            -1,
            NewAddColumnEvent(
                table(1, "t1"),
                vec![Box::new(model::ColumnInfo {
                    Name: ast::NewCIStr(&name),
                    ..Default::default()
                })],
            ),
            store.as_ref(),
        )
        .unwrap();
    }
    eventually(Duration::from_secs(1), || names.lock().unwrap().len() == 8);
    assert_eq!(
        *names.lock().unwrap(),
        vec!["t1", "t2", "t3", "t4", "c5", "c6", "c7", "c8"]
    );
    blocking.store(false, Ordering::Release);
    eventually(Duration::from_secs(1), || {
        count.load(Ordering::Acquire) == 8 && store.Count() == 0
    });
    notifier.OnRetireOwner();
}

/// handler 内会话已是悲观事务，不应再触发「context provider not set」类错误。
#[test]
fn test_begin_twice() {
    let store = fresh_store("test");
    let notifier = NewDDLNotifier(
        SessionPool::default(),
        store.clone(),
        Duration::from_millis(5),
    );
    notifier.RegisterHandler(
        TestHandlerID,
        Box::new(|session, _| {
            assert!(session.IsPessimistic());
            Ok(())
        }),
    );
    notifier.OnBecomeOwner();
    PubSchemeChangeToStore(
        &Session::default(),
        1,
        -1,
        NewCreateTableEvent(table(1000, "t1")),
        store.as_ref(),
    )
    .unwrap();
    eventually(Duration::from_secs(1), || store.Count() == 0);
    assert!(
        notifier
            .Errors()
            .iter()
            .all(|error| !error.contains("context provider not set"))
    );
    notifier.OnRetireOwner();
}

/// 任一 handler 返回致命错误时事件应保留在存储中，供后续重试。
#[test]
fn test_handlers_see_pessimistic_txn_error() {
    let store = fresh_store("test");
    let notifier = NewDDLNotifier(
        SessionPool::default(),
        store.clone(),
        Duration::from_millis(5),
    );
    notifier.RegisterHandler(
        2,
        Box::new(|session, _| {
            assert!(session.IsPessimistic());
            Ok(())
        }),
    );
    notifier.RegisterHandler(
        1,
        Box::new(|session, _| {
            assert!(session.IsPessimistic());
            Err(Error::Message("duplicate key".to_owned()))
        }),
    );
    notifier.OnBecomeOwner();
    PubSchemeChangeToStore(
        &Session::default(),
        1,
        -1,
        NewCreateTableEvent(table(1000, "t1")),
        store.as_ref(),
    )
    .unwrap();
    consistently(Duration::from_secs(1), || store.Count() == 1);
    notifier.OnRetireOwner();
}

/// 会话 Commit 失败（如 information schema changed）时不得删除事件，并应记录错误。
#[test]
fn test_commit_failed() {
    let pool = SessionPool::New(|| {
        let session = Session::default();
        session.FailNextCommit("information schema is changed");
        session
    });
    let store = fresh_store("test");
    let notifier = NewDDLNotifier(pool, store.clone(), Duration::from_millis(5));
    notifier.RegisterHandler(
        TestHandlerID,
        Box::new(|session, _| session.StageEffect("update subscribe_table")),
    );
    notifier.OnBecomeOwner();
    PubSchemeChangeToStore(
        &Session::default(),
        1,
        -1,
        NewCreateTableEvent(table(1000, "t1")),
        store.as_ref(),
    )
    .unwrap();
    consistently(Duration::from_secs(1), || store.Count() == 1);
    assert!(
        notifier
            .Errors()
            .iter()
            .any(|error| error.contains("information schema is changed"))
    );
    notifier.OnRetireOwner();
}
