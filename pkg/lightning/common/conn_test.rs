// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

use crate::{ClientConn, ConnFactory, Context, GRPCConns, NewConnPool, NewGRPCConns};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::Duration;

#[test]
fn zero_capacity_is_accepted_when_constructing_pool() {
    let factory: ConnFactory = Arc::new(|_| Ok(Arc::new(ClientConn::new("unused"))));
    let _pool = NewConnPool(0, factory);
}

#[test]
fn close_waits_for_in_flight_get_and_closes_its_connection() {
    let conns = Arc::new(NewGRPCConns());
    let factory_entered = Arc::new((Mutex::new(false), Condvar::new()));
    let allow_factory_to_finish = Arc::new((Mutex::new(false), Condvar::new()));

    let factory: ConnFactory = {
        let factory_entered = Arc::clone(&factory_entered);
        let allow_factory_to_finish = Arc::clone(&allow_factory_to_finish);
        Arc::new(move |_| {
            let (entered, entered_cv) = &*factory_entered;
            *entered.lock().expect("entered mutex poisoned") = true;
            entered_cv.notify_one();

            let (allowed, allowed_cv) = &*allow_factory_to_finish;
            let _allowed = allowed_cv
                .wait_while(allowed.lock().expect("allowed mutex poisoned"), |allowed| {
                    !*allowed
                })
                .expect("allowed mutex poisoned while waiting");
            Ok(Arc::new(ClientConn::new("store-1")))
        })
    };

    let get_conns = Arc::clone(&conns);
    let get_thread =
        thread::spawn(move || get_conns.GetGrpcConn(&Context::Background(), 1, 1, factory));

    let (entered, entered_cv) = &*factory_entered;
    let _entered = entered_cv
        .wait_while(entered.lock().expect("entered mutex poisoned"), |entered| {
            !*entered
        })
        .expect("entered mutex poisoned while waiting");

    let (close_done_tx, close_done_rx) = mpsc::channel();
    let close_conns = Arc::clone(&conns);
    let close_thread = thread::spawn(move || {
        close_conns.Close();
        close_done_tx.send(()).expect("close receiver dropped");
    });

    assert!(
        close_done_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "Close must be serialized behind an in-flight GetGrpcConn"
    );

    let (allowed, allowed_cv) = &*allow_factory_to_finish;
    *allowed.lock().expect("allowed mutex poisoned") = true;
    allowed_cv.notify_one();

    let conn = get_thread
        .join()
        .expect("get thread panicked")
        .expect("connection creation failed");
    close_done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("Close did not finish after GetGrpcConn");
    close_thread.join().expect("close thread panicked");
    assert!(conn.IsClosed(), "Close must close the in-flight connection");
}
