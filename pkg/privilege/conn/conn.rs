// Copyright 2026 AsterSQL.

// Copyright 2023-2023 PingCAP, Inc.
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

// 认证插件与客户端交换协议包的连接抽象。
//
// `AuthConn` 对应 Go 侧认证连接接口：写入 AuthMoreData、读取普通包、按上下文 Flush。

/// An authentication connection used by authentication plugins to exchange
/// packets with a client.
/// 认证插件用来与客户端交换协议包的连接抽象。
#[allow(non_snake_case)]
pub trait AuthConn {
    /// The context type accepted by [`AuthConn::Flush`].
    /// Flush 接受的调用方上下文类型。
    type Context: ?Sized;
    /// The implementation's I/O error type.
    /// 具体实现的 I/O 错误类型。
    type Error: std::error::Error;

    /// Writes authentication-more-data to the client.
    ///
    /// As in Go, framing the payload with the protocol's `0x01` marker is the
    /// responsibility of the concrete connection implementation.
    /// 向客户端写入 AuthMoreData；协议 `0x01` 帧头由具体连接实现负责。
    fn WriteAuthMoreData(&mut self, data: &[u8]) -> Result<(), Self::Error>;

    /// Reads the next ordinary protocol packet from the client.
    /// 从客户端读取下一普通协议包。
    fn ReadPacket(&mut self) -> Result<Vec<u8>, Self::Error>;

    /// Flushes all pending packets to the client using the caller's context.
    /// 使用调用方上下文将挂起的包刷出到客户端。
    fn Flush(&mut self, ctx: &Self::Context) -> Result<(), Self::Error>;
}
