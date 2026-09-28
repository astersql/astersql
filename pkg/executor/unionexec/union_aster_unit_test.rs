// Copyright 2026 AsterSQL.

// UnionExec 生命周期与异常路径的单元测试。
//
// 通过可控的子执行器制造打开失败、拉取 panic 和关闭失败，并记录打开/关闭事件，
// 验证并发 worker 的错误传播、资源回收及关闭后重新打开语义。

use std::sync::{Arc, Mutex};

use super::union::{Chunk, Executor, UnionExec};

#[derive(Clone, Default)]
/// 跨 worker 共享的生命周期事件记录器。
struct Events(Arc<Mutex<Vec<&'static str>>>);

/// 在拉取数据时 panic，用于验证 worker panic 会转换为普通错误。
struct PanicOnNext;

impl Executor for PanicOnNext {
    fn open(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn next(&mut self) -> Result<Option<Chunk>, String> {
        panic!("child next panic");
    }

    fn close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// 打开必定失败、但仍记录关闭动作的子执行器。
struct OpenFails {
    events: Events,
}

impl Executor for OpenFails {
    fn open(&mut self) -> Result<(), String> {
        Err("open failed".to_string())
    }

    fn next(&mut self) -> Result<Option<Chunk>, String> {
        Ok(None)
    }

    fn close(&mut self) -> Result<(), String> {
        self.events.0.lock().unwrap().push("close");
        Ok(())
    }
}

/// 每次打开后只产出一个 Chunk，并可按需注入关闭错误。
struct OneChunk {
    events: Events,
    close_error: Option<&'static str>,
    yielded: bool,
}

impl Executor for OneChunk {
    fn open(&mut self) -> Result<(), String> {
        self.events.0.lock().unwrap().push("open");
        self.yielded = false;
        Ok(())
    }

    fn next(&mut self) -> Result<Option<Chunk>, String> {
        if self.yielded {
            Ok(None)
        } else {
            self.yielded = true;
            Ok(Some(vec![vec!["value".to_string()]]))
        }
    }

    fn close(&mut self) -> Result<(), String> {
        self.events.0.lock().unwrap().push("close");
        self.close_error
            .map_or(Ok(()), |error| Err(error.to_string()))
    }
}

#[test]
fn worker_panic_is_reported_by_next() {
    let mut union = UnionExec::new(vec![Box::new(PanicOnNext)]);
    union.open().unwrap();

    assert_eq!(union.next(), Err("union worker panicked".to_string()));
    union.close().unwrap();
}

#[test]
fn failed_open_child_is_closed() {
    let events = Events::default();
    let mut union = UnionExec::new(vec![Box::new(OpenFails {
        events: events.clone(),
    })]);
    union.open().unwrap();

    // 子执行器由首次 next 延迟打开，因此打开错误也从 next 返回。
    assert_eq!(union.next(), Err("open failed".to_string()));
    union.close().unwrap();

    // 即使 open 失败，该子执行器已经被 worker 触达，仍必须执行 close。
    assert_eq!(*events.0.lock().unwrap(), vec!["close"]);
}

#[test]
fn union_can_be_reopened_after_close() {
    let events = Events::default();
    let mut union = UnionExec::new(vec![Box::new(OneChunk {
        events: events.clone(),
        close_error: None,
        yielded: false,
    })]);

    // 连续执行两个完整生命周期，确认 close 会恢复子执行器并允许再次启动 worker。
    for _ in 0..2 {
        union.open().unwrap();
        assert_eq!(union.next().unwrap(), Some(vec![vec!["value".to_string()]]));
        assert_eq!(union.next().unwrap(), None);
        union.close().unwrap();
    }

    assert_eq!(
        *events.0.lock().unwrap(),
        vec!["open", "close", "open", "close"]
    );
}

#[test]
fn close_returns_first_error_and_closes_all_reached_children() {
    let events = Events::default();
    let mut union = UnionExec::with_concurrency(
        vec![
            Box::new(OneChunk {
                events: events.clone(),
                close_error: Some("first close error"),
                yielded: false,
            }),
            Box::new(OneChunk {
                events: events.clone(),
                close_error: Some("second close error"),
                yielded: false,
            }),
        ],
        1,
    );
    union.open().unwrap();
    assert!(union.next().unwrap().is_some());
    assert!(union.next().unwrap().is_some());
    assert_eq!(union.next().unwrap(), None);

    // 关闭流程保留首个错误，但不能因此跳过后续已触达子执行器。
    assert_eq!(union.close(), Err("first close error".to_string()));
    let events = events.0.lock().unwrap();
    assert_eq!(events.iter().filter(|event| **event == "close").count(), 2);
}
