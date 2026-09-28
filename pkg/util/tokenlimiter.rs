// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 令牌限流器（Token Limiter）：有界通道实现的并发许可池。
//
// 对应 Go `util.TokenLimiter`：预填固定数量 Token，Get 阻塞取许可、Put 归还，
// 用于限制并发 worker / 资源占用上限。

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, bounded};

#[derive(Debug, Default)]
/// 空许可令牌；仅作通道中的占位资源。
pub struct Token;

/// 基于有界 channel 的令牌池；count 为容量。
pub struct TokenLimiter {
    count: usize,
    sender: Sender<Box<Token>>,
    receiver: Receiver<Box<Token>>,
}

impl TokenLimiter {
    /// 归还令牌到池中。
    pub fn Put(&self, token: Box<Token>) {
        self.sender
            .send(token)
            .expect("token limiter channel disconnected");
    }

    /// 阻塞获取一个令牌。
    pub fn Get(&self) -> Box<Token> {
        self.receiver
            .recv()
            .expect("token limiter channel disconnected")
    }

    /// 返回令牌池容量。
    pub fn Count(&self) -> usize {
        self.count
    }
}

/// 创建容量为 count 的令牌限流器并预填令牌。
pub fn NewTokenLimiter(count: usize) -> Arc<TokenLimiter> {
    // 预放入 count 个 Token，供后续 Get 消费。
    let (sender, receiver) = bounded(count);
    for _ in 0..count {
        sender
            .send(Box::new(Token))
            .expect("token limiter initialization failed");
    }
    Arc::new(TokenLimiter {
        count,
        sender,
        receiver,
    })
}
