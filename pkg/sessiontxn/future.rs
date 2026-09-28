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

#![allow(non_snake_case)]

// 会话事务模块中的“常量 Future”实现。
//
// TSO（Timestamp Oracle，时间戳预言机）通常通过异步 Future 获取全局时间戳；
// 本文件提供立即可用的常量实现，便于测试或无需真正向 PD 请求 TSO 的场景。

use astersql_sessionctx as sessionctx;

/// 包装固定 `u64` 值的 Future：调用 `Wait` 时直接返回该常量，不发起网络请求。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConstantFuture(pub u64);

impl ConstantFuture {
    /// 同步等待并返回内嵌的常量时间戳。
    pub fn Wait(&self) -> Result<u64, sessionctx::GoError> {
        Ok(self.0)
    }
}

impl sessionctx::OracleFuture for ConstantFuture {
    /// 实现会话上下文要求的 OracleFuture：同样直接返回常量。
    fn Wait(self: Box<Self>) -> Result<u64, sessionctx::GoError> {
        Ok(self.0)
    }
}
