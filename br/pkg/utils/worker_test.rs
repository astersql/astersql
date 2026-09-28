// Copyright 2026 AsterSQL.

use std::sync::mpsc;
use std::time::Duration;

use astersql_errors::SharedError;

use crate::worker::{
    AsyncStreamBy, BuildWorkerTokenChannel, CatchAndLogPanic, DefaultWorkerTokenChannelSize,
    MaxWorkerTokenChannelSize, PanicToErr,
};

#[test]
fn panic_helpers_preserve_success_and_recover_panics() {
    assert_eq!(PanicToErr(|| 42).unwrap(), 42);
    let err = PanicToErr(|| panic!("boom")).unwrap_err();
    assert!(
        err.to_string()
            .contains("panicked when executing, message: boom")
    );

    assert_eq!(CatchAndLogPanic(|| "ok"), Some("ok"));
    assert_eq!(CatchAndLogPanic(|| panic!("ignored")), None::<()>);
}

#[test]
fn async_stream_emits_items_then_one_terminal_error() {
    let mut next = 0;
    let stream = AsyncStreamBy(move || {
        next += 1;
        if next == 3 {
            Err(SharedError::new(std::io::Error::other("done")))
        } else {
            Ok(next)
        }
    });

    let frames: Vec<_> = stream.iter().collect();
    assert_eq!(frames.len(), 3);
    assert_eq!((frames[0].Item, frames[1].Item), (1, 2));
    assert!(frames[0].Err.is_none() && frames[1].Err.is_none());
    assert_eq!(frames[2].Item, 0);
    assert_eq!(frames[2].Err.as_ref().unwrap().to_string(), "done");
}

#[test]
fn async_stream_applies_unbuffered_backpressure() {
    let (called_tx, called_rx) = mpsc::channel();
    let stream = AsyncStreamBy(move || {
        called_tx.send(()).unwrap();
        Ok::<_, SharedError>(7)
    });

    called_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(called_rx.recv_timeout(Duration::from_millis(30)).is_err());
    assert_eq!(stream.recv_timeout(Duration::from_secs(1)).unwrap().Item, 7);
    called_rx.recv_timeout(Duration::from_secs(1)).unwrap();
}

#[test]
fn worker_token_channel_supports_go_style_take_and_return() {
    let channel = BuildWorkerTokenChannel(2);
    assert_eq!(channel.capacity(), 2);
    assert_eq!(channel.available(), 2);

    channel.acquire();
    channel.acquire();
    assert_eq!(channel.available(), 0);
    assert!(!channel.try_acquire());

    channel.release();
    assert_eq!(channel.available(), 1);
    assert!(channel.try_acquire());
}

#[test]
fn worker_token_channel_clamps_zero_and_excessive_sizes() {
    assert_eq!(
        BuildWorkerTokenChannel(0).capacity(),
        DefaultWorkerTokenChannelSize
    );
    assert_eq!(
        BuildWorkerTokenChannel(MaxWorkerTokenChannelSize + 1).capacity(),
        MaxWorkerTokenChannelSize
    );
}

#[test]
fn worker_token_channel_blocks_until_a_token_is_returned() {
    let channel = BuildWorkerTokenChannel(1);
    channel.acquire();
    let waiter = channel.clone();
    let (tx, rx) = mpsc::channel();
    let join = std::thread::spawn(move || {
        waiter.acquire();
        tx.send(()).unwrap();
    });

    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
    channel.release();
    rx.recv_timeout(Duration::from_secs(1)).unwrap();
    join.join().unwrap();
}
