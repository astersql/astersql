// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// DDL 会话池（session pool）的单元测试。
//
// 会话池为 DDL worker 提供可复用的内部会话（session）。
// 本文件用 Mock 会话与计数型资源池验证：借还会话、内部会话 ID 登记、
// 悲观事务（pessimistic transaction，加行锁后提交前阻塞冲突写）下的并发阻塞，
// 以及 Destroyable / SlotCounting 两类资源池在 destroy 时的不同回收路径。

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::{
    ExecutionContext, RecordSet, Resource, ResourcePool, ResourcePoolKind, Row, Session,
    SessionContext, SessionError, SessionVariables, SqlValue, Transaction, TransactionMode,
    internal_session_ids, new_session_pool,
};

/// 测试用结果集：可预置行，并记录是否已关闭。
#[derive(Default)]
struct MockRecordSet {
    rows: Vec<Row>,
    closed: bool,
}

impl RecordSet for MockRecordSet {
    fn drain(&mut self, _batch_size: usize) -> Result<Vec<Row>, SessionError> {
        Ok(std::mem::take(&mut self.rows))
    }

    fn close(&mut self) -> Result<(), SessionError> {
        self.closed = true;
        Ok(())
    }
}

/// 测试用会话上下文：模拟事务起止、内部 SQL 执行与共享行锁。
struct MockSessionContext {
    id: u64,
    variables: Arc<SessionVariables>,
    txn: Mutex<Option<Transaction>>,
    next_start_ts: AtomicU64,
    row_lock: Arc<(Mutex<bool>, Condvar)>,
    closed: Mutex<bool>,
}

/// 全局递增的 Mock 会话 ID 分配器。
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

impl MockSessionContext {
    /// 分配新会话 ID，并绑定共享行锁（用于悲观事务测试）。
    fn new(row_lock: Arc<(Mutex<bool>, Condvar)>) -> Arc<Self> {
        let id = NEXT_SESSION_ID.fetch_add(1, Ordering::AcqRel);
        Arc::new(Self {
            id,
            variables: Arc::new(SessionVariables::default()),
            txn: Mutex::new(None),
            next_start_ts: AtomicU64::new(1000 + id * 100),
            row_lock,
            closed: Mutex::new(false),
        })
    }
}

impl SessionContext for MockSessionContext {
    fn session_id(&self) -> u64 {
        self.id
    }

    fn session_variables(&self) -> Arc<SessionVariables> {
        Arc::clone(&self.variables)
    }

    fn enter_new_transaction(&self, _mode: TransactionMode) -> Result<(), SessionError> {
        let start_ts = self.next_start_ts.fetch_add(1, Ordering::AcqRel);
        *self.txn.lock().unwrap() = Some(Transaction {
            start_ts,
            valid: true,
        });
        Ok(())
    }

    fn statement_commit(&self, _context: &ExecutionContext) {}

    fn commit_transaction(&self, _context: &ExecutionContext) -> Result<(), SessionError> {
        if let Some(txn) = self.txn.lock().unwrap().as_mut() {
            txn.valid = false;
        }
        // release row lock for pessimistic tests
        let (lock, cv) = &*self.row_lock;
        let mut held = lock.lock().unwrap();
        *held = false;
        cv.notify_all();
        Ok(())
    }

    fn transaction(&self, _activate: bool) -> Result<Option<Transaction>, SessionError> {
        Ok(self.txn.lock().unwrap().clone())
    }

    fn statement_rollback(&self, _context: &ExecutionContext, _pessimistic_retry: bool) {}

    fn rollback_transaction(&self, _context: &ExecutionContext) {
        if let Some(txn) = self.txn.lock().unwrap().as_mut() {
            txn.valid = false;
        }
    }

    fn execute_internal(
        &self,
        _context: &ExecutionContext,
        query: &str,
        _arguments: &[SqlValue],
    ) -> Result<Option<Box<dyn RecordSet>>, SessionError> {
        let q = query.trim().to_ascii_lowercase();
        // 固定返回常量行，供 test_session_pool 断言 execute 结果。
        if q.starts_with("select 2") {
            return Ok(Some(Box::new(MockRecordSet {
                rows: vec![Row {
                    values: vec![SqlValue::Integer(2)],
                }],
                closed: false,
            })));
        }
        // 模拟悲观行锁：若锁已被持有则 Condvar 等待，拿到后置位阻止并发写。
        if q.starts_with("update test.t") {
            let (lock, cv) = &*self.row_lock;
            let mut held = lock.lock().unwrap();
            while *held {
                held = cv.wait(held).unwrap();
            }
            *held = true;
            return Ok(Some(Box::new(MockRecordSet::default())));
        }
        Ok(Some(Box::new(MockRecordSet::default())))
    }

    fn close(&self) {
        *self.closed.lock().unwrap() = true;
    }
}

/// 可计数的 Mock 资源池：跟踪 put/destroy 次数，并按容量复用会话。
struct CountingPool {
    put_cnt: AtomicI64,
    destroy_cnt: AtomicI64,
    capacity: usize,
    stored: Mutex<Vec<Arc<dyn SessionContext>>>,
    row_lock: Arc<(Mutex<bool>, Condvar)>,
    kind: ResourcePoolKind,
}

impl CountingPool {
    /// 构造指定容量与池类型的计数资源池。
    fn new(capacity: usize, kind: ResourcePoolKind) -> Arc<Self> {
        Arc::new(Self {
            put_cnt: AtomicI64::new(0),
            destroy_cnt: AtomicI64::new(0),
            capacity,
            stored: Mutex::new(Vec::new()),
            row_lock: Arc::new((Mutex::new(false), Condvar::new())),
            kind,
        })
    }

    /// 创建绑定本池共享行锁的新 Mock 会话。
    fn make_session(&self) -> Arc<dyn SessionContext> {
        MockSessionContext::new(Arc::clone(&self.row_lock))
    }
}

impl ResourcePool for CountingPool {
    fn get(&self) -> Result<Resource, SessionError> {
        // 优先复用已归还会话，否则新建。
        let mut stored = self.stored.lock().unwrap();
        if let Some(session) = stored.pop() {
            return Ok(Resource::Session(session));
        }
        Ok(Resource::Session(self.make_session()))
    }

    fn put(&self, resource: Option<Arc<dyn SessionContext>>) {
        self.put_cnt.fetch_add(1, Ordering::AcqRel);
        if let Some(session) = resource {
            let mut stored = self.stored.lock().unwrap();
            // 未满则入池复用，已满则关闭会话释放资源。
            if stored.len() < self.capacity {
                stored.push(session);
            } else {
                session.close();
            }
        }
    }

    fn destroy(&self, resource: Arc<dyn SessionContext>) {
        self.destroy_cnt.fetch_add(1, Ordering::AcqRel);
        resource.close();
    }

    fn close(&self) {
        self.stored.lock().unwrap().clear();
    }

    fn kind(&self) -> ResourcePoolKind {
        self.kind
    }
}

/// 对应 Go 的 TestSessionPool：借出会话、执行 SQL、登记内部 ID，归还后应移除登记。
// test_session_pool 对应 Go 的 TestSessionPool。
#[test]
fn test_session_pool() {
    let resource_pool = CountingPool::new(4, ResourcePoolKind::SlotCounting);
    let pool = new_session_pool(resource_pool.clone() as Arc<dyn ResourcePool>);
    let sess_ctx = pool.get().unwrap();
    let se = Session::new(Arc::clone(&sess_ctx));
    se.begin(&ExecutionContext::default()).unwrap();
    let start_ts = se.transaction().unwrap().unwrap().start_ts;
    assert_ne!(0, start_ts);

    let rows = se
        .execute(&ExecutionContext::default(), "select 2;", "test", &[])
        .unwrap()
        .unwrap();
    assert_eq!(1, rows.len());
    assert_eq!(SqlValue::Integer(2), rows[0].values[0]);

    // 借出期间会话应出现在内部会话 ID 列表中。
    let ids = internal_session_ids();
    assert!(
        ids.contains(&sess_ctx.session_id()),
        "internal sessions should contain borrowed session"
    );

    se.commit(&ExecutionContext::default()).unwrap();
    pool.put(sess_ctx.clone()).unwrap();
    // 归还后应从内部列表移除，避免泄漏。
    let ids = internal_session_ids();
    assert!(
        !ids.contains(&sess_ctx.session_id()),
        "put should remove session from internal list"
    );
}

/// 对应 Go 的 TestPessimisticTxn：第二事务在行锁释放前应阻塞，提交后才能继续。
// test_pessimistic_txn 对应 Go 的 TestPessimisticTxn。
#[test]
fn test_pessimistic_txn() {
    let resource_pool = CountingPool::new(4, ResourcePoolKind::SlotCounting);
    let pool = new_session_pool(resource_pool.clone() as Arc<dyn ResourcePool>);
    let ctx = ExecutionContext::default();

    let sess_ctx = pool.get().unwrap();
    let se = Session::new(Arc::clone(&sess_ctx));
    let sess_ctx2 = pool.get().unwrap();
    let se2 = Session::new(Arc::clone(&sess_ctx2));

    se.begin_pessimistic(&ctx).unwrap();
    se2.begin_pessimistic(&ctx).unwrap();
    // 第一事务先拿到行锁。
    se.execute(&ctx, "update test.t set b = b + 1 where a = 1", "ut", &[])
        .unwrap();

    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let done_flag = Arc::clone(&done);
    let se2_thread = se2;
    let ctx2 = ctx.clone();
    // 第二事务在另一线程尝试同键更新，应阻塞直到第一事务提交。
    let handle = thread::spawn(move || {
        se2_thread
            .execute(&ctx2, "update test.t set b = b + 1 where a = 1", "ut", &[])
            .unwrap();
        {
            let (lock, cv) = &*done_flag;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
        se2_thread.commit(&ctx2).unwrap();
    });

    thread::sleep(Duration::from_millis(100));
    {
        let (lock, _) = &*done;
        assert!(!*lock.lock().unwrap(), "second txn should still be blocked");
    }
    // 提交释放行锁后，等待第二事务完成。
    se.commit(&ctx).unwrap();
    {
        let (lock, cv) = &*done;
        let mut finished = lock.lock().unwrap();
        while !*finished {
            finished = cv.wait(finished).unwrap();
        }
    }
    handle.join().unwrap();
    pool.put(sess_ctx).unwrap();
    pool.put(sess_ctx2).unwrap();
}

/// 对应 Go 的 TestSessionPoolDestroyResourcePool：destroy 后底层池再 get 应得到新会话。
// test_session_pool_destroy_resource_pool 对应 Go 的 TestSessionPoolDestroyResourcePool。
#[test]
fn test_session_pool_destroy_resource_pool() {
    let resource_pool = CountingPool::new(1, ResourcePoolKind::SlotCounting);
    let pool = new_session_pool(resource_pool.clone() as Arc<dyn ResourcePool>);

    let sess_ctx = pool.get().unwrap();
    let old_id = sess_ctx.session_id();
    pool.destroy(sess_ctx).unwrap();

    let new_res = resource_pool.get().unwrap();
    let Resource::Session(new_sess) = new_res else {
        panic!("expected session resource");
    };
    assert_ne!(old_id, new_sess.session_id());
    new_sess.close();
    resource_pool.put(None);
}

/// 对应 Go 的 TestSessionPoolDestroyDestroyableSessionPool：Destroyable 池应走 destroy 而非 put。
// test_session_pool_destroy_destroyable_session_pool 对应 Go 的 TestSessionPoolDestroyDestroyableSessionPool。
#[test]
fn test_session_pool_destroy_destroyable_session_pool() {
    let resource_pool = CountingPool::new(1, ResourcePoolKind::Destroyable);
    let pool = new_session_pool(resource_pool.clone() as Arc<dyn ResourcePool>);

    let sess_ctx = pool.get().unwrap();
    pool.destroy(sess_ctx).unwrap();

    assert_eq!(0, resource_pool.put_cnt.load(Ordering::Acquire));
    assert_eq!(1, resource_pool.destroy_cnt.load(Ordering::Acquire));
}
