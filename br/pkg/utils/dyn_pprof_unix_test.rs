// Copyright 2026 AsterSQL.

use std::sync::mpsc;
use std::time::Duration;

use signal_hook::consts::signal::SIGUSR1;
use signal_hook::iterator::Signals;

use crate::dyn_pprof_unix::listen_for_start_signal;

#[test]
fn test_sigusr1_dispatches_dynamic_pprof_start() {
    let signals = Signals::new([SIGUSR1]).expect("register SIGUSR1 for test");
    let (started_tx, started_rx) = mpsc::channel();

    let listener = std::thread::spawn(move || {
        listen_for_start_signal(signals, || {
            started_tx.send(()).expect("report callback");
            Err("stop test listener".to_owned())
        });
    });

    signal_hook::low_level::raise(SIGUSR1).expect("raise SIGUSR1");
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("SIGUSR1 should invoke the dynamic pprof callback");
    listener.join().expect("signal listener thread");
}
