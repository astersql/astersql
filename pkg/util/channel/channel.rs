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

// channel 辅助工具：排空可接收 channel 中的残留消息。
//
// 对应 Go `util/channel.Clear`。在并发关闭路径上，需先排空再退出，
// 避免发送端阻塞或消息泄漏。

// DrainableChannel 对应 Go 泛型约束 `chan T | <-chan T` 的可接收能力。
// `None` 表示所有发送端均已关闭。
/// 可被排空的 channel 抽象；`recv` 返回 `None` 表示发送端均已关闭。
pub trait DrainableChannel<T> {
    /// 接收一个值；channel 已关闭且无剩余值时返回 `None`。
    fn recv(&mut self) -> Option<T>;
}

impl<T> DrainableChannel<T> for std::sync::mpsc::Receiver<T> {
    fn recv(&mut self) -> Option<T> {
        std::sync::mpsc::Receiver::recv(self).ok()
    }
}

// Clear is to clear the channel
// Clear 对应 Go 的 `for range ch {}`：持续丢弃收到的值，直到 channel 被关闭。
/// 持续接收并丢弃，直到 channel 关闭；语义对齐 Go `for range ch {}`。
#[allow(non_snake_case)]
pub fn Clear<T, V>(mut ch: V)
where
    V: DrainableChannel<T>,
{
    while let Some(_value) = ch.recv() {
        // Go 循环体为空；显式丢弃元素，保留“清空 channel”的语义。
    }
}
