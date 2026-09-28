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

// 算子（Operator）之间的通道组合与数据通道抽象。
//
// DXF 数据流管道中，相邻算子通过无缓冲 channel 连接：上游写 Sink、下游读 Source。
// `Compose` 负责创建共享通道并把两端分别挂到两个算子上，语义对齐 Go 实现。

#![allow(dead_code, non_snake_case)]

use crate::workerpool::Channel;

/// An operator whose input channel can be assigned by `Compose`.
///
/// 可被 `Compose` 设置输入通道（Source）的算子。
pub trait WithSource<T> {
    /// 绑定上游输出到本算子的输入通道。
    fn SetSource(&mut self, channel: SimpleDataChannel<T>);
}

/// An operator whose output channel can be assigned by `Compose`.
///
/// 可被 `Compose` 设置输出通道（Sink）的算子。
pub trait WithSink<T> {
    /// Implementations must call `Finish` after their final send.
    ///
    /// 绑定本算子输出到下游输入；发送完毕后实现方须调用 `Finish` 关闭通道。
    fn SetSink(&mut self, channel: SimpleDataChannel<T>);
}

/// Connects the output of `op1` to the input of `op2` with the same
/// unbuffered channel used by the Go implementation.
///
/// 用容量为 0 的无缓冲通道连接 `op1` 的 Sink 与 `op2` 的 Source，
/// 使上游每发一条数据都与下游接收形成背压握手。
pub fn Compose<T, S, D>(op1: &mut S, op2: &mut D)
where
    S: WithSink<T>,
    D: WithSource<T>,
{
    // bounded(0) 对应 Go 的无缓冲 channel，发送方在接收方就绪前阻塞。
    let channel = NewSimpleDataChannel(Channel::bounded(0));
    op1.SetSink(channel.clone());
    op2.SetSource(channel);
}

/// 算子间数据通道接口：可取得底层 channel，并显式结束（关闭）。
pub trait DataChannel<T> {
    /// 返回可发送/接收的底层 workerpool Channel 句柄。
    fn Channel(&self) -> Channel<T>;
    /// 关闭通道，通知对端数据已结束。
    fn Finish(&self);
}

/// The shared, explicitly closable channel used between two operators.
///
/// 两个算子之间共享、可显式关闭的简单数据通道包装。
pub struct SimpleDataChannel<T> {
    channel: Channel<T>,
}

impl<T> Clone for SimpleDataChannel<T> {
    fn clone(&self) -> Self {
        Self {
            channel: self.channel.clone(),
        }
    }
}

/// 用已有 `Channel` 构造 `SimpleDataChannel`。
pub fn NewSimpleDataChannel<T>(channel: Channel<T>) -> SimpleDataChannel<T> {
    SimpleDataChannel { channel }
}

impl<T> DataChannel<T> for SimpleDataChannel<T> {
    fn Channel(&self) -> Channel<T> {
        self.channel.clone()
    }

    fn Finish(&self) {
        // 关闭底层 channel，使对端 recv 得到结束信号。
        self.channel.close();
    }
}
