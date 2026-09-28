// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// 内嵌（standalone）存储服务生命周期抽象。
//
// 对应 Go 侧 InnerServer：封装 DatabaseBundle 的启动/停止，
// 以及 Raft/批量 Raft/快照等占位接口，供 mock 环境管理存储后端。

use std::sync::Arc;

/// 数据库资源包：关闭时释放底层存储。
pub trait DatabaseBundle: Send + Sync {
    fn close(&self) -> Result<(), String>;
}
/// 内嵌服务接口：配置、启停与 Raft/快照入口。
pub trait InnerServer: Send + Sync {
    /// 启动前配置钩子。
    fn setup(&self);
    /// 启动服务。
    fn start(&self) -> Result<(), String>;
    /// 停止服务并关闭资源包。
    fn stop(&self) -> Result<(), String>;
    /// 单条 Raft 消息处理占位。
    fn raft(&self) -> Result<(), String>;
    /// 批量 Raft 消息处理占位。
    fn batch_raft(&self) -> Result<(), String>;
    /// 快照处理占位。
    fn snapshot(&self) -> Result<(), String>;
}

/// 单机内嵌服务：持有需要在停止时关闭的资源包。
pub struct StandAloneInnerServer<B> {
    /// 底层数据库资源包。
    bundle: Arc<B>,
}
/// `StandAloneInnerServer` 构造与状态查询。
impl<B: DatabaseBundle> StandAloneInnerServer<B> {
    /// 用给定资源包创建尚未启动的服务实例。
    pub fn new(bundle: Arc<B>) -> Self {
        Self { bundle }
    }
    /// standalone 的 Go `Start` 不维护启动状态；保留查询接口并恒为 false。
    pub fn is_started(&self) -> bool {
        false
    }
}
/// 实现 InnerServer：仅 stop 关闭资源包，其余方法与 Go 一样为空实现。
impl<B: DatabaseBundle> InnerServer for StandAloneInnerServer<B> {
    fn setup(&self) {}
    fn start(&self) -> Result<(), String> {
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        self.bundle.close()
    }
    fn raft(&self) -> Result<(), String> {
        Ok(())
    }
    fn batch_raft(&self) -> Result<(), String> {
        Ok(())
    }
    fn snapshot(&self) -> Result<(), String> {
        Ok(())
    }
}
