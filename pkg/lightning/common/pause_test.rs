// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `Pauser` 单元测试：验证未暂停放行、Pause/Resume 阻塞唤醒、以及取消上下文立即解除等待。

use crate::{Context, NewPauser};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

#[test]
fn resume_releases_waiters_from_the_current_pause_generation() {
    const WAITER_COUNT: usize = 128;

    let pauser = Arc::new(NewPauser());
    pauser.Pause();

    let ready = Arc::new(Barrier::new(WAITER_COUNT + 1));
    let (completed_tx, completed_rx) = std::sync::mpsc::channel();
    for _ in 0..WAITER_COUNT {
        let pauser = Arc::clone(&pauser);
        let ready = Arc::clone(&ready);
        let completed_tx = completed_tx.clone();
        std::thread::spawn(move || {
            ready.wait();
            pauser.Wait(&Context::Background()).unwrap();
            completed_tx.send(()).unwrap();
        });
    }
    drop(completed_tx);

    ready.wait();
    std::thread::sleep(Duration::from_millis(50));
    pauser.Resume();
    pauser.Pause();

    for _ in 0..WAITER_COUNT {
        if completed_rx
            .recv_timeout(Duration::from_millis(200))
            .is_err()
        {
            pauser.Resume();
            panic!("a waiter remained blocked after its pause generation was resumed");
        }
    }
}

/// 断言屏障在 `[minv, maxv]` 时间窗口内解除；超时则 panic。
///
/// 用于验证多线程 `Wait`/`Resume` 的阻塞与唤醒时序是否符合预期。
fn assert_unblocks_between(barrier: Arc<Barrier>, minv: Duration, maxv: Duration) {
    let (tx, rx) = std::sync::mpsc::sync_channel::<Duration>(1);
    let start = Instant::now();
    std::thread::spawn(move || {
        barrier.wait();
        tx.send(start.elapsed())
            .expect("send waitgroup elapsed duration");
    });

    match rx.recv_timeout(maxv) {
        Ok(dur) => {
            if dur < minv {
                panic!("WaitGroup unblocked before minimum duration, it was {dur:?}");
            }
        }
        Err(_) => match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(dur) => panic!("WaitGroup did not unblock after maximum duration, it was {dur:?}"),
            Err(_) => panic!("WaitGroup did not unblock after maximum duration"),
        },
    }
}

/// 覆盖 Pauser 主要场景：初始不阻塞、Pause 后阻塞直至 Resume、Cancel 立即解除、取消不改变暂停状态。
#[test]
fn test_pause() {
    let p = Arc::new(NewPauser());

    // initially these calls should not be blocking.
    // 初始未 Pause：10 个 Wait 应很快全部通过
    let barrier = Arc::new(Barrier::new(11));
    for _ in 0..10 {
        let p = Arc::clone(&p);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            p.Wait(&Context::Background()).unwrap();
            barrier.wait();
        });
    }
    assert_unblocks_between(
        barrier,
        Duration::from_millis(0),
        Duration::from_millis(100),
    );

    // after calling Pause(), these should be blocking...
    // Pause 后 Wait 应阻塞，直到下方延迟 Resume
    p.Pause();
    let barrier = Arc::new(Barrier::new(11));
    for _ in 0..10 {
        let p = Arc::clone(&p);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            p.Wait(&Context::Background()).unwrap();
            barrier.wait();
        });
    }

    // ... until we call Resume()
    // 约 500ms 后 Resume，解除阻塞应落在 500–800ms 窗口
    let p_resume = Arc::clone(&p);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        p_resume.Resume();
    });
    assert_unblocks_between(
        barrier,
        Duration::from_millis(500),
        Duration::from_millis(800),
    );

    // if the context is canceled, Wait() should immediately unblock...
    // 取消上下文后 Wait 应立即返回含 "canceled" 的错误
    let ctx = Context::Background();
    p.Pause();
    let barrier = Arc::new(Barrier::new(11));
    for _ in 0..10 {
        let p = Arc::clone(&p);
        let barrier = Arc::clone(&barrier);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let err = p.Wait(&ctx).expect_err("expected cancel");
            assert!(err.to_string().contains("canceled"));
            barrier.wait();
        });
    }
    ctx.Cancel();
    assert_unblocks_between(
        barrier,
        Duration::from_millis(0),
        Duration::from_millis(200),
    );

    // canceling the context does not affect the state of the pauser
    // 取消上下文不清除 Pause 状态；仍需 Resume 才能放行新的 Wait
    let barrier = Arc::new(Barrier::new(2));
    let p_wait = Arc::clone(&p);
    let barrier_wait = Arc::clone(&barrier);
    std::thread::spawn(move || {
        p_wait.Wait(&Context::Background()).unwrap();
        barrier_wait.wait();
    });
    let p_resume = Arc::clone(&p);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        p_resume.Resume();
    });
    assert_unblocks_between(
        barrier,
        Duration::from_millis(500),
        Duration::from_millis(800),
    );
}
