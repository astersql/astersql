// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::{
    backpressure::*, consistenthash::*, executor::*, server_maintainer::*, time_proxy::*, utils::*,
};
use crossbeam_channel::{Receiver, bounded};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
fn wait(mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "condition timed out"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn recv<T>(rx: &Receiver<T>) -> T {
    rx.recv_timeout(Duration::from_secs(5)).unwrap()
}
fn value(a: &std::sync::atomic::AtomicI64) -> i64 {
    a.load(Ordering::SeqCst)
}
fn executor(timeout: Duration) -> (Arc<MockTimeModule>, Arc<TaskExecutor>) {
    let m = MockTimeModule::new(unix(1700000000, 0));
    let e = TaskExecutor::with_clock(1, timeout, Clock::Mock(m.clone()));
    (m, e)
}
fn hash() -> ConsistentHash {
    let mut h = ConsistentHash::new(1);
    h.hash_func = Arc::new(|b| match b {
        b"nodeA#0" => 10,
        b"nodeB#0" => 20,
        b"nodeC#0" => 15,
        b"low" => 5,
        b"mid" => 15,
        b"high" => 25,
        _ => panic!("unexpected hash input {b:?}"),
    });
    h
}
#[test]
fn port_7e84fb645a_hash_empty() {
    let h = ConsistentHash::new(1);
    assert_eq!(h.get_node(b"any"), "");
    assert_eq!(h.node_count(), 0);
    assert!(h.get_nodes().is_empty());
}
#[test]
fn port_7e84fb645a_hash_wraps() {
    let mut h = hash();
    assert!(h.add_node("nodeB"));
    assert!(h.add_node("nodeA"));
    assert!(!h.add_node("nodeA"));
    assert!(!h.add_node(""));
    for (k, n) in [("low", "nodeA"), ("mid", "nodeB"), ("high", "nodeA")] {
        assert_eq!(h.get_node(k.as_bytes()), n)
    }
    assert!(h.ring.windows(2).all(|w| w[0] <= w[1]));
}
#[test]
fn port_7e84fb645a_hash_remove() {
    let mut h = ConsistentHash::new(2);
    h.add_node("nodeA");
    h.add_node("nodeB");
    assert_eq!(h.node_count(), 2);
    assert_eq!(h.virtual_nodes("nodeB"), 2);
    h.remove_node("nodeB");
    h.remove_node("absent");
    assert_eq!(h.virtual_nodes("nodeB"), 0);
    assert_eq!(h.ring.len(), 2);
    assert_eq!(h.get_nodes(), ["nodeA"]);
    assert!(h.ring.windows(2).all(|w| w[0] <= w[1]));
}
#[test]
fn port_7e84fb645a_hash_rebuild() {
    let mut h = ConsistentHash::new(2);
    h.rebuild(["nodeA", "nodeB"]);
    assert_eq!(h.node_count(), 2);
    assert_eq!(h.ring.len(), 4);
    assert!(!h.get_node(b"key").is_empty());
    assert!(h.ring.windows(2).all(|w| w[0] <= w[1]));
}
#[test]
fn port_7e84fb645a_hash_rebuild_resets() {
    let mut h = ConsistentHash::new(2);
    h.add_node("nodeA");
    h.add_node("nodeB");
    h.rebuild(["nodeC"]);
    assert_eq!(h.node_count(), 1);
    assert_eq!(h.ring.len(), 2);
    assert_eq!(h.get_node(b"key"), "nodeC");
    assert!(h.ring.windows(2).all(|w| w[0] <= w[1]));
}
#[test]
fn port_7e84fb645a_hash_vectors_and_replica_bounds() {
    assert_eq!(murmur3(b""), 0);
    assert_eq!(murmur3(b"foo"), 0xf6a5c420);
    assert_eq!(murmur3(b"hello"), 0x248bfa47);
    let mut low = ConsistentHash::new(0);
    low.add_node("a");
    assert_eq!(low.virtual_nodes("a"), 1);
    let mut high = ConsistentHash::new(999);
    high.add_node("a");
    assert_eq!(high.virtual_nodes("a"), 200);
}
#[derive(Clone, Default, Debug, PartialEq)]
struct Node(&'static str, i64);
impl Lessable for Node {
    fn less(&self, o: &Self) -> bool {
        self.1 < o.1
    }
}
#[test]
fn port_7e84fb645a_priority_order_update() {
    let mut q = PriorityQueue::default();
    q.push(Node("b", 2000));
    q.push(Node("a", 1000));
    q.push(Node("c", 3000));
    assert_eq!(q.front().unwrap().borrow().value.0, "a");
    q.update(q.front().as_ref(), Node("d", 5000));
    q.push(Node("c", 500));
    let mut out = vec![];
    while let Some(i) = q.pop() {
        out.push(i.borrow().value.0)
    }
    assert_eq!(out, ["c", "b", "c", "d"]);
}
#[test]
fn port_7e84fb645a_priority_invalid() {
    let mut q = PriorityQueue::default();
    assert!(q.front().is_none());
    assert!(q.pop().is_none());
    assert!(q.remove(None).is_none());
    q.update(None, Node("ignored", 0));
    let ghost = ItemImpl::new(Node("ghost", 1));
    q.update(Some(&ghost), Node("still", 2));
    assert!(q.remove(Some(&ghost)).is_none());
    assert_eq!(ghost.borrow().value.0, "ghost");
}
#[test]
fn port_7e84fb645a_priority_removed() {
    let mut q = PriorityQueue::default();
    let a = q.push(Node("a", 2000));
    let b = q.push(Node("b", 3000));
    let c = q.push(Node("c", 1000));
    assert_eq!(q.remove(Some(&b)).unwrap().borrow().value.0, "b");
    q.update(Some(&b), Node("b2", 500));
    q.update(Some(&a), Node("a2", 4000));
    q.push(Node("d", 1500));
    let mut out = vec![];
    while let Some(i) = q.pop() {
        out.push(i.borrow().value.0)
    }
    assert_eq!(out, ["c", "d", "a2"]);
    assert_eq!(c.borrow().value.0, "c");
}
#[test]
fn port_7e84fb645a_notifier() {
    let n = Arc::new(Notifier::default());
    n.wake();
    n.weak_wake();
    assert!(n.is_awake());
    n.wait();
    assert!(!n.is_awake());
    let (tx, rx) = bounded(1);
    let c = n.clone();
    let t = std::thread::spawn(move || {
        c.wait();
        tx.send(()).unwrap();
    });
    n.wake();
    recv(&rx);
    t.join().unwrap();
    assert!(!n.clear());
}
#[test]
fn port_7e84fb645a_cpu_mem() {
    let mut c = CPUMemBackpressureController::new(0.7, 0.8, Duration::ZERO);
    c.get_cpu_usage = Arc::new(|| (0.9, false));
    c.get_mem_total = Arc::new(|| 100);
    c.get_mem_used = Arc::new(|| Ok(20));
    assert_eq!(c.should_backpressure(), (true, DEFAULT_DELAY));
    c.get_cpu_usage = Arc::new(|| (0.1, false));
    c.get_mem_used = Arc::new(|| Ok(90));
    c.delay = Duration::from_millis(200);
    assert_eq!(c.should_backpressure(), (true, c.delay));
    c.get_mem_used = Arc::new(|| Ok(10));
    assert_eq!(c.should_backpressure(), (false, Duration::ZERO));
    c.get_cpu_usage = Arc::new(|| (1.0, true));
    c.get_mem_used = Arc::new(|| Err("unavailable".into()));
    assert!(!c.should_backpressure().0);
    c.get_mem_total = Arc::new(|| 0);
    assert!(!c.should_backpressure().0);
    c.cpu_threshold = 0.0;
    c.mem_threshold = 0.0;
    assert!(!c.should_backpressure().0);
}
#[test]
fn port_7e84fb645a_queue_fifo() {
    let mut q = TaskQueue::default();
    for i in 1..=4 {
        q.push(TaskRequest {
            name: format!("t{i}"),
            task: Box::new(|| Ok(())),
        })
    }
    for i in 1..=2 {
        assert_eq!(q.pop().unwrap().name, format!("t{i}"));
    }
    for i in 5..=12 {
        q.push(TaskRequest {
            name: format!("t{i}"),
            task: Box::new(|| Ok(())),
        })
    }
    assert_eq!(q.len(), 10);
    for i in 3..=12 {
        assert_eq!(q.pop().unwrap().name, format!("t{i}"));
    }
    assert_eq!(q.len(), 0);
    assert!(q.pop().is_none());
}
#[test]
fn port_7e84fb645a_queue_releases() {
    let mut q = TaskQueue::default();
    let a = Arc::new(());
    let c = a.clone();
    q.push(TaskRequest {
        name: "a".into(),
        task: Box::new(move || {
            drop(c);
            Ok(())
        }),
    });
    assert_eq!(Arc::strong_count(&a), 2);
    drop(q.pop());
    assert_eq!(Arc::strong_count(&a), 1);
    for _ in 0..2 {
        let c = a.clone();
        q.push(TaskRequest {
            name: "b".into(),
            task: Box::new(move || {
                drop(c);
                Ok(())
            }),
        })
    }
    assert_eq!(q.clear(), 2);
    assert_eq!(q.len(), 0);
    assert_eq!(q.tasks.capacity(), 0);
    assert_eq!(Arc::strong_count(&a), 1);
}
fn task_block(
    e: &TaskExecutor,
    name: &str,
    started: crossbeam_channel::Sender<String>,
    block: Receiver<()>,
) {
    let name = name.to_owned();
    assert!(e.submit(
        name.clone(),
        Box::new(move || {
            started.send(name).unwrap();
            let _ = block.recv();
            Ok(())
        })
    ));
}
#[test]
fn port_7e84fb645a_executor_max_concurrency() {
    let (_, e) = executor(Duration::ZERO);
    assert!(e.run());
    assert!(!e.run());
    let (tx, rx) = bounded(2);
    let (unblock, block) = bounded(0);
    task_block(&e, "t1", tx.clone(), block.clone());
    task_block(&e, "t2", tx, block);
    assert_eq!(recv(&rx), "t1");
    assert!(rx.try_recv().is_err());
    assert_eq!(value(&e.metrics.submitted), 2);
    assert_eq!(value(&e.metrics.running), 1);
    assert_eq!(value(&e.metrics.waiting), 1);
    drop(unblock);
    assert_eq!(recv(&rx), "t2");
    wait(|| value(&e.metrics.finished) == 2);
    assert!(e.close());
    assert_eq!(value(&e.metrics.running), 0);
    assert_eq!(value(&e.metrics.waiting), 0);
}
#[test]
fn port_7e84fb645a_executor_update_concurrency() {
    let (_, e) = executor(Duration::ZERO);
    e.run();
    let (tx, rx) = bounded(3);
    let (unblock, block) = bounded(0);
    task_block(&e, "t1", tx.clone(), block.clone());
    task_block(&e, "t2", tx, block);
    recv(&rx);
    assert!(rx.try_recv().is_err());
    e.set_max_concurrency(2);
    recv(&rx);
    assert_eq!(value(&e.metrics.running), 2);
    e.set_max_concurrency(1);
    e.set_max_concurrency(0);
    drop(unblock);
    wait(|| value(&e.metrics.finished) == 2);
    assert!(e.close());
}
#[test]
fn port_7e84fb645a_executor_reject_close() {
    let (_, e) = executor(Duration::ZERO);
    assert!(e.close());
    assert!(!e.close());
    assert!(!e.run());
    assert!(!e.submit("rejected", Box::new(|| Ok(()))));
    assert_eq!(value(&e.metrics.submitted), 0);
    assert_eq!(value(&e.metrics.rejected), 1);
    assert_eq!(value(&e.metrics.finished), 0);
}
fn timeout_test(update: bool) {
    let (m, e) = executor(if update {
        Duration::ZERO
    } else {
        Duration::from_millis(50)
    });
    e.run();
    if update {
        e.set_timeout(Duration::from_millis(40))
    }
    let (tx, rx) = bounded(2);
    let (unblock, block) = bounded(0);
    task_block(&e, "long", tx.clone(), block);
    assert_eq!(recv(&rx), "long");
    assert_eq!(value(&e.metrics.running), 1);
    m.advance(Duration::from_millis(if update { 40 } else { 500 }));
    wait(|| value(&e.metrics.timeout) == 1 && value(&e.metrics.timed_out_running) == 1);
    assert_eq!(value(&e.metrics.running), 0);
    assert!(e.submit(
        "short",
        Box::new(move || {
            tx.send("short".into()).unwrap();
            Ok(())
        })
    ));
    assert_eq!(recv(&rx), "short");
    drop(unblock);
    wait(|| value(&e.metrics.finished) == 2);
    assert!(e.close());
    assert_eq!(value(&e.metrics.failed), 0);
    assert_eq!(value(&e.metrics.timeout), 1);
    assert_eq!(value(&e.metrics.timed_out_running), 0);
    assert_eq!(value(&e.metrics.waiting), 0);
    assert_eq!(value(&e.metrics.running), 0);
}
#[test]
fn port_7e84fb645a_executor_timeout_releases_slot() {
    timeout_test(false)
}
#[test]
fn port_7e84fb645a_executor_update_timeout() {
    timeout_test(true)
}
struct Toggle {
    blocked: AtomicBool,
    delay: Duration,
}
impl TaskBackpressureController for Toggle {
    fn should_backpressure(&self) -> (bool, Duration) {
        (self.blocked.load(Ordering::SeqCst), self.delay)
    }
}
#[test]
fn port_7e84fb645a_executor_backpressure() {
    let (m, e) = executor(Duration::ZERO);
    e.run();
    let c = Arc::new(Toggle {
        blocked: AtomicBool::new(true),
        delay: Duration::from_secs(1),
    });
    e.set_backpressure_controller(Some(c.clone()));
    let (tx, rx) = bounded(1);
    e.submit(
        "blocked",
        Box::new(move || {
            tx.send(()).unwrap();
            Ok(())
        }),
    );
    wait(|| m.pending() > 0);
    assert!(rx.try_recv().is_err());
    m.advance(Duration::from_secs(5));
    wait(|| m.pending() > 0);
    assert!(rx.try_recv().is_err());
    assert_eq!(value(&e.metrics.backpressure), 1);
    c.blocked.store(false, Ordering::SeqCst);
    m.advance(Duration::from_secs(1));
    recv(&rx);
    wait(|| value(&e.metrics.finished) == 1);
    e.close();
    assert_eq!(value(&e.metrics.backpressure_blocked), 0);
}
struct Blocking {
    entered: crossbeam_channel::Sender<()>,
    release: Receiver<()>,
}
impl TaskBackpressureController for Blocking {
    fn should_backpressure(&self) -> (bool, Duration) {
        let _ = self.entered.try_send(());
        let _ = self.release.recv();
        (false, Duration::ZERO)
    }
}
#[test]
fn port_7e84fb645a_executor_backpressure_outside_lock() {
    let (_, e) = executor(Duration::ZERO);
    e.run();
    let (ent, entered) = bounded(1);
    let (release, blocked) = bounded(0);
    e.set_backpressure_controller(Some(Arc::new(Blocking {
        entered: ent,
        release: blocked,
    })));
    let (tx, rx) = bounded(2);
    let c = tx.clone();
    e.submit(
        "t1",
        Box::new(move || {
            c.send(1).unwrap();
            Ok(())
        }),
    );
    recv(&entered);
    let (done, done_rx) = bounded(1);
    let other = e.clone();
    std::thread::spawn(move || {
        assert!(other.submit(
            "t2",
            Box::new(move || {
                tx.send(2).unwrap();
                Ok(())
            })
        ));
        done.send(()).unwrap();
    });
    recv(&done_rx);
    drop(release);
    assert_eq!(recv(&rx), 1);
    assert_eq!(recv(&rx), 2);
    wait(|| value(&e.metrics.finished) == 2);
    e.close();
}
#[test]
fn port_7e84fb645a_executor_close_interrupts_backpressure() {
    let (m, e) = executor(Duration::ZERO);
    e.run();
    e.set_backpressure_controller(Some(Arc::new(Toggle {
        blocked: AtomicBool::new(true),
        delay: Duration::from_secs(3600),
    })));
    e.submit("must not run", Box::new(|| panic!("task should not run")));
    wait(|| m.pending() > 0);
    let start = Instant::now();
    assert!(e.close());
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(value(&e.metrics.finished), 0);
    assert_eq!(value(&e.metrics.waiting), 0);
    assert_eq!(value(&e.metrics.backpressure_blocked), 0);
    assert!(value(&e.metrics.backpressure) > 0);
}
#[test]
fn port_7e84fb645a_executor_error_panic_and_close_wait() {
    let (_, e) = executor(Duration::ZERO);
    e.run();
    e.submit("error", Box::new(|| Err("boom".into())));
    e.submit("panic", Box::new(|| panic!("boom")));
    wait(|| value(&e.metrics.finished) == 2);
    assert_eq!(value(&e.metrics.failed), 2);
    let (tx, rx) = bounded(1);
    let (unblock, block) = bounded(0);
    task_block(&e, "wait", tx, block);
    recv(&rx);
    let (done, done_rx) = bounded(1);
    let c = e.clone();
    std::thread::spawn(move || {
        c.close();
        done.send(()).unwrap();
    });
    assert!(done_rx.recv_timeout(Duration::from_millis(20)).is_err());
    drop(unblock);
    recv(&done_rx);
}
#[test]
fn port_7e84fb645a_wait_count_keeps_time() {
    let m = MockTimeModule::new(123);
    let mut polls = 0;
    wait(|| {
        polls += 1;
        polls == 2
    });
    assert_eq!(m.now(), 123);
}
#[test]
fn port_7e84fb645a_clock_ops() {
    let start = unix(1770714487, 0);
    let m = MockTimeModule::new(start);
    let _restore = install_mock_time_module_for_test(Some(m.clone()));
    let c = current_clock();
    assert_eq!(c.now(), start);
    assert_eq!(c.since(start), 0);
    assert_eq!(c.until(start + 3_000_000_000), 3_000_000_000);
    m.advance(Duration::from_secs(5));
    assert_eq!(c.since(start), 5_000_000_000);
    assert_eq!(c.until(start + 7_000_000_000), 2_000_000_000);
    assert_eq!(unix(123, 456), 123000000456);
    assert_eq!(unix_milli(789), 789000000);
}
#[test]
fn port_7e84fb645a_clock_after_sleep() {
    let m = MockTimeModule::new(0);
    let c = Clock::Mock(m.clone());
    let rx = c.after(Duration::from_secs(2));
    assert!(rx.try_recv().is_err());
    m.advance(Duration::from_secs(1));
    assert!(rx.try_recv().is_err());
    m.advance(Duration::from_secs(1));
    assert_eq!(recv(&rx), 2_000_000_000);
    m.set(0);
    c.sleep(Duration::from_secs(3));
    assert_eq!(m.now(), 3_000_000_000);
    m.set(0);
    m.set_auto_advance_on_sleep(false);
    let (done, done_rx) = bounded(1);
    let c = c.clone();
    std::thread::spawn(move || {
        c.sleep(Duration::from_secs(4));
        done.send(()).unwrap();
    });
    wait(|| m.pending() == 1);
    m.advance(Duration::from_secs(3));
    assert!(done_rx.try_recv().is_err());
    m.advance(Duration::from_secs(1));
    recv(&done_rx);
    assert_eq!(recv(&m.after(Duration::ZERO)), m.now());
}
#[test]
fn port_7e84fb645a_clock_timer() {
    let m = MockTimeModule::new(0);
    let c = Clock::Mock(m.clone());
    let t = c.timer(Duration::from_secs(2));
    m.advance(Duration::from_secs(1));
    assert!(t.c.try_recv().is_err());
    m.advance(Duration::from_secs(1));
    recv(&t.c);
    assert!(!t.reset(Duration::from_secs(3)));
    m.advance(Duration::from_secs(2));
    assert!(t.c.try_recv().is_err());
    m.advance(Duration::from_secs(1));
    recv(&t.c);
    assert!(!t.stop());
    assert!(!t.reset(Duration::from_secs(5)));
    assert!(t.reset(Duration::from_secs(5)));
    assert!(t.stop());
    m.advance(Duration::from_secs(10));
    assert!(t.c.try_recv().is_err());
}
#[test]
fn port_7e84fb645a_clock_real_fallback() {
    let c = current_clock();
    let start = Clock::Real.now();
    assert!((c.now() - start).abs() < 100_000_000);
    recv(&c.after(Duration::from_millis(10)));
    let t = c.timer(Duration::from_millis(10));
    recv(&t.c);
}
#[test]
#[should_panic(expected = "mock time module is nil")]
fn port_7e84fb645a_clock_nil_install() {
    let _ = install_mock_time_module_for_test(None);
}
struct Helper {
    self_info: Mutex<Result<ServerInfo, String>>,
    infos: Mutex<BTreeMap<String, ServerInfo>>,
    err: AtomicBool,
    filter: AtomicBool,
    calls: std::sync::atomic::AtomicUsize,
    all_calls: std::sync::atomic::AtomicUsize,
}
impl Default for Helper {
    fn default() -> Self {
        Self {
            self_info: Mutex::new(Ok(ServerInfo {
                id: "nodeA".into(),
                ..Default::default()
            })),
            infos: Mutex::new(BTreeMap::new()),
            err: AtomicBool::new(false),
            filter: AtomicBool::new(false),
            calls: 0.into(),
            all_calls: 0.into(),
        }
    }
}
impl ServerDiscovery for Helper {
    fn server_filter(&self, s: &ServerInfo) -> bool {
        !self.filter.load(Ordering::SeqCst) || s.id == "nodeA"
    }
    fn get_server_info(&self) -> Result<ServerInfo, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.self_info.lock().unwrap().clone()
    }
    fn get_all_server_info(
        &self,
        _: &Cancellation,
    ) -> Result<BTreeMap<String, ServerInfo>, String> {
        self.all_calls.fetch_add(1, Ordering::SeqCst);
        if self.err.load(Ordering::SeqCst) {
            Err("boom".into())
        } else {
            Ok(self.infos.lock().unwrap().clone())
        }
    }
}
fn set_servers(h: &Helper, ids: &[&str]) {
    *h.infos.lock().unwrap() = ids
        .iter()
        .map(|id| {
            (
                id.to_string(),
                ServerInfo {
                    id: id.to_string(),
                    ..Default::default()
                },
            )
        })
        .collect();
}
#[test]
fn port_7e84fb645a_server_rebuild_sorted() {
    let h = Arc::new(Helper::default());
    let s = ServerConsistentHash::new(Cancellation::default(), 1, h.clone());
    s.membership.write().unwrap().hash = hash();
    set_servers(&h, &["nodeB", "nodeA"]);
    assert_eq!(s.refresh(), Ok(true));
    assert_eq!(s.server_ids(), ["nodeA", "nodeB"]);
    {
        let m = s.membership.read().unwrap();
        assert_eq!(m.servers.len(), 2);
        assert_eq!(m.hash.node_count(), 2);
        assert_eq!(m.hash.get_node(b"low"), "nodeA");
        assert_eq!(m.hash.get_node(b"mid"), "nodeB");
    }
    set_servers(&h, &["nodeA"]);
    assert_eq!(s.refresh(), Ok(true));
    assert_eq!(s.server_ids(), ["nodeA"]);
    let m = s.membership.read().unwrap();
    assert_eq!(m.hash.node_count(), 1);
    assert_eq!(m.hash.get_node(b"mid"), "nodeA");
}
#[test]
fn port_7e84fb645a_server_key_types() {
    let h = Arc::new(Helper::default());
    set_servers(&h, &["nodeA", "nodeB"]);
    let s = ServerConsistentHash::new(Cancellation::default(), 1, h);
    s.membership.write().unwrap().hash.hash_func = Arc::new(|b| match b {
        b"nodeA#0" => 10,
        b"nodeB#0" => 20,
        b"mid" => 15,
        _ => {
            assert_eq!(b, 123456789_i64.to_be_bytes());
            15
        }
    });
    s.refresh().unwrap();
    for id in ["nodeB", "nodeA"] {
        s.membership.write().unwrap().id = id.into();
        let expected = id == "nodeB";
        assert_eq!(s.available(&123456789_i64), expected);
        assert_eq!(s.available(&b"mid".to_vec()), expected);
        assert_eq!(s.available(&"mid".to_string()), expected);
        assert_eq!(s.available_int64(123456789), expected);
        assert_eq!(s.available_string("mid"), expected);
        assert_eq!(s.available(&"mid"), expected);
        assert_eq!(s.available(&b"mid".as_slice()), expected);
        assert!(!s.available(&42_u32));
    }
}
#[test]
fn port_7e84fb645a_server_filter() {
    let h = Arc::new(Helper::default());
    set_servers(&h, &["nodeA", "nodeB"]);
    h.filter.store(true, Ordering::SeqCst);
    let s = ServerConsistentHash::new(Cancellation::default(), 1, h);
    s.refresh().unwrap();
    assert_eq!(s.server_ids(), ["nodeA"]);
    assert_eq!(s.membership.read().unwrap().hash.get_node(b"mid"), "nodeA");
}
#[test]
fn port_7e84fb645a_server_no_change() {
    let h = Arc::new(Helper::default());
    set_servers(&h, &["nodeA"]);
    let s = ServerConsistentHash::new(Cancellation::default(), 1, h.clone());
    assert_eq!(s.refresh(), Ok(true));
    let ring = s.membership.read().unwrap().hash.ring.clone();
    h.infos.lock().unwrap().get_mut("nodeA").unwrap().ip = "changed".into();
    assert_eq!(s.refresh(), Ok(false));
    assert_eq!(h.all_calls.load(Ordering::SeqCst), 2);
    assert_eq!(s.membership.read().unwrap().hash.ring, ring);
    assert_eq!(s.membership.read().unwrap().servers["nodeA"].ip, "");
}
#[test]
fn port_7e84fb645a_server_init() {
    let h = Arc::new(Helper::default());
    let s = ServerConsistentHash::new(Cancellation::default(), 1, h.clone());
    assert!(s.init());
    assert_eq!(s.id(), "nodeA");
    assert_eq!(s.membership.read().unwrap().hash.node_count(), 0);
    assert_eq!(h.calls.load(Ordering::SeqCst), 1);
    assert_eq!(h.all_calls.load(Ordering::SeqCst), 0);
}
#[test]
fn port_7e84fb645a_server_init_canceled() {
    let h = Arc::new(Helper::default());
    *h.self_info.lock().unwrap() = Err("boom".into());
    let ctx = Cancellation::default();
    ctx.cancel();
    let s = ServerConsistentHash::new(ctx, 1, h.clone());
    let start = Instant::now();
    assert!(!s.init());
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(h.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn port_7e84fb645a_server_init_retry() {
    let h = Arc::new(Helper::default());
    *h.self_info.lock().unwrap() = Err("boom".into());
    let m = MockTimeModule::new(0);
    let s = Arc::new(ServerConsistentHash::with_clock(
        Cancellation::default(),
        1,
        h.clone(),
        Clock::Mock(m.clone()),
    ));
    let c = s.clone();
    let thread = std::thread::spawn(move || c.init());
    wait(|| m.pending() == 1);
    *h.self_info.lock().unwrap() = Ok(ServerInfo {
        id: "nodeB".into(),
        ..Default::default()
    });
    m.advance(Duration::from_secs(1));
    assert!(thread.join().unwrap());
    assert_eq!(s.id(), "nodeB");
    assert_eq!(h.calls.load(Ordering::SeqCst), 2);
}
#[test]
fn port_7e84fb645a_server_fetch_error() {
    let h = Arc::new(Helper::default());
    h.err.store(true, Ordering::SeqCst);
    let s = ServerConsistentHash::new(Cancellation::default(), 1, h);
    assert_eq!(s.refresh(), Err("boom".into()));
    assert!(s.server_ids().is_empty());
}

#[test]
fn port_7e84fb645a_panic_error_preserves_context() {
    let err = TaskExecutor::safe_execute(TaskRequest {
        name: "refresh-node".into(),
        task: Box::new(|| panic!("original task failure")),
    })
    .unwrap_err();
    assert!(err.contains("refresh-node"), "{err}");
    assert!(err.contains("original task failure"), "{err}");
}
