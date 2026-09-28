// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 预取 Reader：在后台线程提前读出底层流数据，降低同步 IO 等待。
//
// 对应 Go 的 `util/prefetch`：用双缓冲与 channel 把已读块交给前台 `Read`；
// `rangeSize` 表示调用方预期总字节数，用于把预取缓冲偏大导致的 UnexpectedEOF
// 收敛为正常 EOF。`Read` 与 `Close` 不可并发调用。

#![allow(dead_code)]
#![allow(non_snake_case)]

use crossbeam_channel::{Receiver, Sender, bounded, select};
use std::io::{self, Cursor, ErrorKind, Read};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// 可读且可关闭的流，对应 Go 的 `io.ReadCloser`。
pub trait ReadCloser: Read + Send {
    /// 关闭底层资源；关闭后不应再继续读。
    fn close(&mut self) -> io::Result<()>;
}

// Reader is a reader that prefetches data from the underlying reader.
/// 带后台预取的 Reader：`bufCh` 传递已填满的块，`err` 槽记录后台错误。
pub struct Reader {
    /// 与后台 worker 共享的底层 ReadCloser。
    r: Arc<Mutex<Box<dyn ReadCloser + Send>>>,
    /// 当前正在消费的预取缓冲（Cursor 模拟 bytes.Reader）。
    curBufReader: Option<Cursor<Vec<u8>>>,
    /// 接收后台预取块的 channel。
    bufCh: Receiver<Vec<u8>>,
    /// 后台读错误槽；channel 断开后前台从此处取错误。
    err: Arc<Mutex<Option<(ErrorKind, String)>>>,
    /// 后台预取线程句柄，Close 时 join。
    wg: Option<JoinHandle<()>>,
    /// 是否已关闭，防止重复 Close。
    closed: bool,
    /// 关闭通知发送端；drop 后 worker 从 closedCh 退出。
    closedCh: Option<Sender<()>>,
}

// NewReader creates a new Reader.
// NewReader 创建带后台预取任务的 Reader，并返回 Go 语义中的 io.ReadCloser。
/// 创建预取 Reader：`prefetchSize` 对半拆成两块缓冲，`rangeSize` 为预期总长度。
pub fn NewReader(
    r: Box<dyn ReadCloser + Send>,
    rangeSize: i64,
    prefetchSize: usize,
) -> Box<dyn ReadCloser + Send> {
    let (buf_sender, buf_receiver) = bounded::<Vec<u8>>(0);
    let (closed_sender, closed_receiver) = bounded::<()>(0);
    let shared_reader = Arc::new(Mutex::new(r));
    let shared_err = Arc::new(Mutex::new(None));
    let worker_reader = Arc::clone(&shared_reader);
    let worker_err = Arc::clone(&shared_err);
    let half_prefetch_size = prefetchSize / 2;
    let worker = thread::spawn(move || {
        Reader::run(
            worker_reader,
            [vec![0; half_prefetch_size], vec![0; half_prefetch_size]],
            buf_sender,
            worker_err,
            closed_receiver,
            rangeSize,
        );
    });

    Box::new(Reader {
        r: Arc::clone(&shared_reader),
        curBufReader: None,
        bufCh: buf_receiver,
        err: Arc::clone(&shared_err),
        wg: Some(worker),
        closed: false,
        closedCh: Some(closed_sender),
    })
}

impl Reader {
    // run 对应 Go 的 (*Reader).run 后台预取循环。
    // Go 版本直接修改 Receiver 上的字段；把共享 reader、错误槽、channel 和双缓冲显式传入线程。
    fn run(
        r: Arc<Mutex<Box<dyn ReadCloser + Send>>>,
        mut buf: [Vec<u8>; 2],
        bufCh: Sender<Vec<u8>>,
        err: Arc<Mutex<Option<(ErrorKind, String)>>>,
        closedCh: Receiver<()>,
        rangeSize: i64,
    ) {
        // Go 的 defer r.wg.Done() 在 Rust 中由线程函数返回体现；JoinHandle 结束后 Close 才继续。
        let mut bufIdx = 0usize;
        let mut readSize = 0i64;
        loop {
            bufIdx = (bufIdx + 1) % 2;
            let current_buf = &mut buf[bufIdx];
            // io.ReadFull 会尽力填满当前预取缓冲；这里用 helper 展开，保留 n 和 err 同时返回的 Go 形状。
            let (n, read_err) = {
                let mut locked_reader = r.lock().expect("prefetch reader mutex poisoned");
                Self::readFullGo(&mut **locked_reader, current_buf)
            };
            // Go 中 buf = buf[:n] 会让发送方只交付已经读到的前 n 字节；Vec 克隆只是这里的所有权表达。
            let filled = current_buf[..n].to_vec();
            readSize += n as i64;

            select! {
                recv(closedCh) -> _ => return,
                send(bufCh, filled) -> result => if result.is_err() { return },
            }

            if let Some(err_value) = read_err {
                if err_value.kind() == ErrorKind::UnexpectedEof && readSize == rangeSize {
                    // this is caused by io.ReadFull. Because we are prefetching, the
                    // buffer size may be larger that caller's need. So convert to io.EOF.
                    // Note: the reader r.r.Read may also return io.ErrUnexpectedEOF,
                    // we need skip this case.
                    // Rust's Read trait represents EOF as Ok(0), so leave the
                    // error slot empty and let channel disconnection report it.
                } else {
                    *err.lock().expect("prefetch reader error slot poisoned") =
                        Some((err_value.kind(), err_value.to_string()));
                }
                return;
            }
        }
    }

    // readFullGo 对应 Go 标准库 io.ReadFull 的局部展开。
    // 它不是新增业务逻辑，只是为了在这里保留“读满缓冲或返回已读字节数加错误”的 IO 语义。
    fn readFullGo(reader: &mut dyn Read, buf: &mut [u8]) -> (usize, Option<io::Error>) {
        let mut total = 0usize;
        while total < buf.len() {
            match reader.read(&mut buf[total..]) {
                Ok(0) => {
                    // Go 的 io.ReadFull 在未读满时遇到 EOF 会返回 ErrUnexpectedEOF。
                    return (
                        total,
                        Some(io::Error::new(ErrorKind::UnexpectedEof, "unexpected EOF")),
                    );
                }
                Ok(n) => {
                    total += n;
                }
                Err(err) => {
                    // 保留 Go 中 n, err 同时返回的形状，调用方会先发送已经读到的部分再处理错误。
                    return (total, Some(err));
                }
            }
        }
        (total, None)
    }

    // Read implements io.Reader. Read should not be called concurrently with Close.
    // Read 从当前缓冲 reader 读取数据；当前缓冲耗尽后再从 bufCh 接收后台预取的新缓冲。
    pub fn Read(&mut self, mut data: &mut [u8]) -> (usize, Option<io::Error>) {
        let mut total = 0usize;
        loop {
            if self.curBufReader.is_none() {
                match self.bufCh.recv() {
                    Ok(b) => {
                        // 对应 bytes.NewReader(b)，后续 Read 会从这段已预取数据中顺序消费。
                        self.curBufReader = Some(Cursor::new(b));
                    }
                    Err(_) => {
                        if total > 0 {
                            // Go 语义：本次已经读到数据时先返回数据和 nil，错误留到下一次 Read。
                            return (total, None);
                        }
                        let err = self
                            .err
                            .lock()
                            .expect("prefetch reader error slot poisoned")
                            .as_ref()
                            .map(|(kind, message)| io::Error::new(*kind, message.clone()));
                        return (0, err);
                    }
                }
            }

            let expected = data.len();
            let n = match self.curBufReader.as_mut().unwrap().read(data) {
                Ok(n) => n,
                Err(err) => {
                    // bytes.Reader 在 Go 中通常只返回 EOF；Rust Cursor 一般也不会产生外部 IO 错误。
                    // 这里仍保留错误分支，方便对照 io.Reader 的通用返回形状。
                    return (total, Some(err));
                }
            };
            total += n;
            if n == expected {
                return (total, None);
            }

            // Go 代码 data = data[n:]，继续填充调用方剩余的目标切片。
            data = &mut data[n..];
            let cur_exhausted = self
                .curBufReader
                .as_ref()
                .map(|reader| reader.position() as usize == reader.get_ref().len())
                .unwrap_or(true);
            if n == 0 || cur_exhausted {
                // 对应 err == io.EOF || r.curBufReader.Len() == 0 时丢弃当前 reader 并继续接收下一块。
                self.curBufReader = None;
                continue;
            }
        }
    }

    // Close implements io.Closer. Close should not be called concurrently with Read.
    // Close 先关闭底层 reader，再通知后台预取任务退出，最后等待后台任务收尾。
    pub fn Close(&mut self) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        // Go 先调用 r.r.Close()，这样后台 ReadFull 如果阻塞在底层 reader 上，有机会被关闭动作打断。
        let ret = self
            .r
            .lock()
            .expect("prefetch reader mutex poisoned")
            .close();
        // Dropping the sole sender disconnects the close channel. This wakes
        // the worker even when it is blocked handing off a prefetched buffer.
        self.closedCh.take();
        if let Some(handle) = self.wg.take() {
            // 对应 sync.WaitGroup.Wait，确保后台 run 不再访问共享资源后再标记 closed。
            let _ = handle.join();
        }
        self.closed = true;
        ret
    }
}

impl Read for Reader {
    /// 将 Go 风格 `(n, Option<Error>)` 适配为 Rust `io::Result`。
    fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
        let (n, err) = self.Read(data);
        match err {
            Some(err) => Err(err),
            None => Ok(n),
        }
    }
}

impl ReadCloser for Reader {
    /// 委托到 `Close`，满足 ReadCloser 契约。
    fn close(&mut self) -> io::Result<()> {
        self.Close()
    }
}
