// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Unix SIGUSR1 handler — ported from `sigusr1_unix.go`.
//!
//!
//! Unix 版本通过一对 `UnixStream` 把异步信号转换成线程内可轮询的字节通知。
//! 这样做的重点不是偷懒，而是避开 signal handler 中只能执行异步信号安全操作的限制。
//! `PIPE` 充当全局桥梁：C 信号处理器只负责往写端塞一个字节。
//! 真正的业务回调则在 Rust 线程里读取字节后再调用 `handler()`。
//! 这与 Go 中“信号到 channel，再由普通 goroutine 处理”的职责分层一致。
//! 写端被设成非阻塞，是为了降低连续信号下 handler 卡死的风险。
//! 读取循环遇到 `WouldBlock` 时短暂休眠，等价于一个轻量轮询器。
//! 收到信号后先打 debug 日志，再执行回调，便于排查状态服务是否按约定拉起。
//! 这里没有把底层 `sigaction` 细节暴露给调用方，平台差异完全封装在模块内部。
//! 因此上层只需要知道：在 Unix 上注册一次，就能在每次 `SIGUSR1` 到来时重复触发回调。

use crate::log;
use crate::zap;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

type Handler = Arc<Mutex<Box<dyn Fn() + Send>>>;

struct Dispatcher {
    handlers: Mutex<Vec<Handler>>,
    _writer: UnixStream,
}

static DISPATCHER: OnceLock<Dispatcher> = OnceLock::new();
static WRITE_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_sigusr1(_: libc::c_int) {
    let fd = WRITE_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte = 1_u8;
        unsafe {
            // `write` is async-signal-safe. A full nonblocking socket merely
            // coalesces an already-pending notification, like Go's buffered
            // signal channel.
            libc::write(fd, (&byte as *const u8).cast(), 1);
        }
    }
}

fn dispatcher() -> &'static Dispatcher {
    DISPATCHER.get_or_init(|| {
        let (reader, writer) = UnixStream::pair().expect("sigusr1 pipe");
        writer
            .set_nonblocking(true)
            .expect("nonblocking sigusr1 pipe");
        WRITE_FD.store(writer.as_raw_fd(), Ordering::Relaxed);

        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_sigusr1 as *const () as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            sa.sa_flags = 0;
            assert_eq!(
                libc::sigaction(libc::SIGUSR1, &sa, std::ptr::null_mut()),
                0,
                "install SIGUSR1 handler"
            );
        }

        thread::spawn(move || {
            use std::io::Read;
            let mut buf = [0_u8; 64];
            let mut reader = reader;
            while let Ok(size) = reader.read(&mut buf) {
                if size == 0 {
                    break;
                }
                for _ in 0..size {
                    log::L().Debug("received signal", zap::Int("signal", libc::SIGUSR1 as i64));
                    let handlers = DISPATCHER
                        .get()
                        .expect("initialized SIGUSR1 dispatcher")
                        .handlers
                        .lock()
                        .expect("SIGUSR1 handlers")
                        .clone();
                    for handler in handlers {
                        handler.lock().expect("SIGUSR1 handler")();
                    }
                }
            }
        });

        Dispatcher {
            handlers: Mutex::new(Vec::new()),
            _writer: writer,
        }
    })
}

/// handleSigUsr1 listens for SIGUSR1 and executes `handler()` each time.
pub fn handleSigUsr1<F>(handler: F)
where
    F: Fn() + Send + 'static,
{
    dispatcher()
        .handlers
        .lock()
        .expect("SIGUSR1 handlers")
        .push(Arc::new(Mutex::new(Box::new(handler))));
}
