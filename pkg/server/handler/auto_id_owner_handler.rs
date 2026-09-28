// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Auto ID Owner HTTP 处理器。
//
// Auto ID（自增列 ID 分配）在集群中通常由单一 owner 节点负责；本模块暴露
// 健康检查与 owner 身份查询接口，供运维或调度侧判断当前实例是否可分配自增 ID。

#![allow(dead_code, non_snake_case, non_camel_case_types)]

/// 检查实例健康状态及是否为 Auto ID owner。
pub trait AutoIDOwnerChecker {
    /// 返回实例是否健康（不可用时应拒绝业务查询）。
    fn Health(&self) -> bool;
    /// 返回当前实例是否持有 Auto ID owner 身份。
    fn IsAutoIDOwner(&self) -> bool;
}

/// HTTP 响应写出抽象，解耦具体 HTTP 框架。
pub trait AutoIDOwnerResponse {
    type Error;

    /// 仅写出 HTTP 状态码（例如健康检查失败时的 500）。
    fn write_status(&mut self, status: u16) -> Result<(), Self::Error>;
    /// 写出 owner 状态 JSON 载荷。
    fn write_owner_status(&mut self, status: autoIDOwnerStatus) -> Result<(), Self::Error>;
}

/// Auto ID owner 查询 handler，持有检查器实现。
pub struct AutoIDOwnerHandler<C> {
    checker: C,
}

/// 序列化给客户端的 owner 状态：`IsOwner` 表示本实例是否为 Auto ID 分配者。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct autoIDOwnerStatus {
    pub IsOwner: bool,
}

/// 构造 `AutoIDOwnerHandler`。
pub fn NewAutoIDOwnerHandler<C: AutoIDOwnerChecker>(checker: C) -> AutoIDOwnerHandler<C> {
    AutoIDOwnerHandler { checker }
}

impl<C: AutoIDOwnerChecker> AutoIDOwnerHandler<C> {
    /// 处理 HTTP 请求：不健康返回 500；否则返回当前 owner 状态。
    pub fn ServeHTTP<W: AutoIDOwnerResponse>(&self, writer: &mut W) -> Result<(), W::Error> {
        // 健康检查失败时不暴露 owner 信息，直接 500。
        if !self.checker.Health() {
            return writer.write_status(500);
        }
        writer.write_owner_status(autoIDOwnerStatus {
            IsOwner: self.checker.IsAutoIDOwner(),
        })
    }
}

/// 包级入口，转发到 handler 的 `ServeHTTP`。
pub fn ServeHTTP<C: AutoIDOwnerChecker, W: AutoIDOwnerResponse>(
    handler: &AutoIDOwnerHandler<C>,
    writer: &mut W,
) -> Result<(), W::Error> {
    handler.ServeHTTP(writer)
}
