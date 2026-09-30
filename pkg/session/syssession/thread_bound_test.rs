// Copyright 2026 AsterSQL.

use crate::{SessionError, ThreadBoundSession};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

struct LocalSession {
    value: Rc<u64>,
    drop_notice: mpsc::Sender<std::thread::ThreadId>,
}
impl Drop for LocalSession {
    fn drop(&mut self) {
        self.drop_notice.send(std::thread::current().id()).unwrap();
    }
}
fn cleanup(_: &mut LocalSession) {}

#[test]
fn crossks_align_system_session_non_send_thread_lifetime() {
    let (notice, dropped) = mpsc::channel();
    let handle = ThreadBoundSession::new(
        move || {
            Ok(LocalSession {
                value: Rc::new(42),
                drop_notice: notice,
            })
        },
        cleanup,
    )
    .unwrap();
    let worker = handle
        .call(|session| {
            assert_eq!(*session.value, 42);
            Ok(std::thread::current().id())
        })
        .unwrap();
    assert_ne!(worker, std::thread::current().id());
    assert_eq!(
        handle.call(|_| Ok(std::thread::current().id())).unwrap(),
        worker
    );
    handle.close();
    assert_eq!(
        dropped.recv_timeout(Duration::from_secs(1)).unwrap(),
        worker
    );
    assert!(handle.call(|_| Ok(())).is_err());
    handle.close();
}

#[test]
fn crossks_align_system_session_start_failure_and_panic_cleanup() {
    let failed =
        ThreadBoundSession::<Rc<u64>>::new(|| Err(SessionError::new("factory failure")), |_| {});
    assert!(failed.is_err());
    let (notice, dropped) = mpsc::channel();
    let handle = ThreadBoundSession::new(
        move || {
            Ok(LocalSession {
                value: Rc::new(42),
                drop_notice: notice,
            })
        },
        cleanup,
    )
    .unwrap();
    assert!(handle.call::<()>(|_| panic!("request panic")).is_err());
    dropped.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(handle.call(|_| Ok(())).is_err());
    handle.close();
}

#[test]
fn crossks_align_system_session_close_waits_for_accepted_request() {
    use std::sync::Arc;
    let (notice, dropped) = mpsc::channel();
    let handle = Arc::new(
        ThreadBoundSession::new(
            move || {
                Ok(LocalSession {
                    value: Rc::new(42),
                    drop_notice: notice,
                })
            },
            cleanup,
        )
        .unwrap(),
    );
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let caller = Arc::clone(&handle);
    let operation = std::thread::spawn(move || {
        caller.call(move |session| {
            entered.send(()).unwrap();
            wait.recv().unwrap();
            Ok(*session.value)
        })
    });
    started.recv_timeout(Duration::from_secs(1)).unwrap();
    let closer = Arc::clone(&handle);
    let close = std::thread::spawn(move || closer.close());
    // The accepted operation is still using its non-Send value.
    assert!(matches!(dropped.try_recv(), Err(mpsc::TryRecvError::Empty)));
    release.send(()).unwrap();
    assert_eq!(operation.join().unwrap().unwrap(), 42);
    close.join().unwrap();
    dropped.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(handle.call(|_| Ok(())).is_err());
}
