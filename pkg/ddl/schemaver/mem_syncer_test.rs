// Copyright 2026 AsterSQL.

use crate::{Context, NewMemSyncer};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

/// Go replaces `globalVerCh` during Init without closing the previous channel.
#[test]
fn init_keeps_previous_global_version_channel_open_like_go() {
    let syncer = NewMemSyncer();
    let context = Context::Background();
    syncer.Init(context.clone()).unwrap();
    let previous = syncer.GlobalVersionCh();

    syncer.Init(context).unwrap();

    assert_eq!(
        previous.RecvTimeout(Duration::from_millis(10)),
        Err(RecvTimeoutError::Timeout)
    );
}
