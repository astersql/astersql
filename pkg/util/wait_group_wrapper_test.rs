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

// WaitGroup 包装器单元测试。
//
// 覆盖基础 `WaitGroupWrapper`、增强版 `WaitGroupEnhancedWrapper` 的并发计数、
// panic 恢复回调，以及 `ErrorGroupWithRecover` 将 panic 转为错误的路径。

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::anyhow;

use crate::wait_group_wrapper::{
    NewErrorGroupWithRecover, NewWaitGroupEnhancedWrapper, WaitGroupWrapper,
};

/// 验证 `Run` / 增强版 `Run` 能正确等待所有任务完成并累计计数。
#[test]
fn TestWaitGroupWrapperRun() {
    let expect: i32 = 4;
    let val = Arc::new(AtomicI32::new(0));
    let wg = WaitGroupWrapper::default();
    // 并发提交 expect 个任务，各自对共享计数器 +1。
    for _ in 0..expect {
        let val = Arc::clone(&val);
        wg.Run(move || {
            val.fetch_add(1, Ordering::SeqCst);
        });
    }
    wg.Wait();
    assert_eq!(expect, val.load(Ordering::SeqCst));

    // 增强版 WaitGroup：携带任务名字符串，行为应与基础版一致。
    val.store(0, Ordering::SeqCst);
    let wg2 = NewWaitGroupEnhancedWrapper(String::new(), None, false);
    for i in 0..expect {
        let val = Arc::clone(&val);
        wg2.Run(
            move || {
                val.fetch_add(1, Ordering::SeqCst);
            },
            format!("test_{i}"),
        );
    }
    wg2.Wait();
    assert_eq!(expect, val.load(Ordering::SeqCst));
}

/// 验证 `RunWithRecover`：任务 panic 后仍调用恢复回调，Wait 能正常返回。
#[test]
fn TestWaitGroupWrapperRunWithRecover() {
    let expect: i32 = 2;
    let val = Arc::new(AtomicI32::new(0));
    let wg = WaitGroupWrapper::default();
    // 故意 panic，恢复回调中对计数器 +1，确认 recover 路径被触发。
    for _ in 0..expect {
        let val = Arc::clone(&val);
        wg.RunWithRecover(
            || panic!("test1"),
            Some(move |_: Option<_>| {
                val.fetch_add(1, Ordering::SeqCst);
            }),
        );
    }
    wg.Wait();
    assert_eq!(expect, val.load(Ordering::SeqCst));

    // 增强版同样验证 recover + 任务名。
    val.store(0, Ordering::SeqCst);
    let wg2 = NewWaitGroupEnhancedWrapper(String::new(), None, false);
    for i in 0..expect {
        let val = Arc::clone(&val);
        wg2.RunWithRecover(
            || panic!("test1"),
            Some(move |_| {
                val.fetch_add(1, Ordering::SeqCst);
            }),
            format!("test_{i}"),
        );
    }
    wg2.Wait();
    assert_eq!(expect, val.load(Ordering::SeqCst));
}

/// 验证增强版 `check`：有未完成任务时为 true，任务退出后为 false。
#[test]
fn TestWaitGroupWrapperCheck() {
    let wg = NewWaitGroupEnhancedWrapper(String::new(), None, false);
    let (quit_tx, quit_rx) = crossbeam_channel::bounded::<()>(0);
    // 阻塞任务：收到 quit 信号前一直挂起，使 check() 返回 true。
    wg.Run(
        move || {
            let _ = quit_rx.recv();
        },
        "test".to_owned(),
    );

    assert!(wg.check());

    // 放行任务并短暂等待，确认 check 变为 false。
    quit_tx.send(()).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert!(!wg.check());
}

/// 触发整数除零 panic，供 `RunWithLog` / ErrorGroup 恢复测试使用。
fn middleF() {
    let a = std::hint::black_box(0);
    let _ = 10 / a;
}

/// 验证 `RunWithLog` 内部捕获 panic 后 `Wait` 仍能完成。
#[test]
fn TestWaitGroupWrapperGo() {
    let wg = WaitGroupWrapper::default();
    wg.RunWithLog(|| {
        middleF();
    });
    // Panic is recovered inside RunWithLog; Wait must still complete.
    wg.Wait();
}

/// 验证 `ErrorGroupWithRecover`：panic 被转为 Wait 返回的错误。
#[test]
fn TestNewErrorGroupWithRecover() {
    let eg = NewErrorGroupWithRecover();
    eg.Go(|| {
        middleF();
        Ok(())
    });
    let err = eg
        .Wait()
        .expect_err("division by zero should be recovered as error");
    let message = err.to_string();
    // 不同平台 / 捕获层可能给出不同措辞，任一匹配即可。
    assert!(
        message.contains("attempt to divide by zero")
            || message.contains("integer divide by zero")
            || message.contains("divide by zero")
            || message.contains("pkg/util.middleF")
            || message.contains("panic"),
        "unexpected recover error: {message}"
    );
}

/// Go `errgroup.Group` records the first error by completion time, not submission order.
#[test]
fn TestErrorGroupReturnsFirstCompletedError() {
    let eg = NewErrorGroupWithRecover();
    eg.Go(|| {
        thread::sleep(Duration::from_millis(200));
        Err(anyhow!("submitted first, completed second"))
    });
    eg.Go(|| Err(anyhow!("completed first")));

    let err = eg.Wait().expect_err("the group should return an error");
    assert_eq!("completed first", err.to_string());
}
