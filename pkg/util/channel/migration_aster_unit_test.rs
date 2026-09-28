// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `Clear` 迁移回归测试：验证排空缓冲值与等待关闭语义。
//
// 用带 Drop 探针的消息确认：清空会消费全部已发送值，且在 channel
// 暂时为空时不会提前返回，直到发送端关闭。

use super::Clear;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

/// Drop 时递增计数，用于断言消息是否被真正接收并释放。
struct DropProbe(Arc<AtomicUsize>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// 缓冲若干值后关闭发送端，`Clear` 应全部排空。
#[test]
fn clear_drains_buffered_values_until_the_channel_is_closed() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();

    for _ in 0..3 {
        sender.send(DropProbe(Arc::clone(&dropped))).unwrap();
    }
    drop(sender);

    Clear(receiver);

    assert_eq!(dropped.load(Ordering::SeqCst), 3);
}

/// 另一线程上 `Clear` 在暂时为空时阻塞，直到发送端关闭后才结束。
#[test]
fn clear_waits_for_close_instead_of_stopping_when_temporarily_empty() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();
    let handle = std::thread::spawn(move || Clear(receiver));

    sender.send(DropProbe(Arc::clone(&dropped))).unwrap();
    sender.send(DropProbe(Arc::clone(&dropped))).unwrap();
    drop(sender);
    handle.join().unwrap();

    assert_eq!(dropped.load(Ordering::SeqCst), 2);
}
