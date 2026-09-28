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

// 资源组控制器与 Runaway（失控查询）管理器的启动编排。
//
// Runaway 用于检测并限制长时间/高消耗的失控查询。本文件提供配置结构、
// 控制器与管理器 trait，以及启动 controller 后发布二者的初始化顺序。

// impl Domain {
// initResourceGroupsController 对应 Go 的方法：根据 PD client 初始化资源组控制器。
//     pub fn initResourceGroupsController(
//         &mut self,
//         ctx: context::Context,
//         pdClient: Option<pd::Client>,
//         uniqueID: u64,
//     ) -> Result<(), errors::Error> {
//         if pdClient.is_none() {
//             logutil::BgLogger().Warn("cannot setup up resource controller, not using tikv storage");
// Go 对 unistore 不支持 resource controller 的场景返回 nil，这里保持静默成功。
//             return Ok(());
//         }
//
//         let mut keyspaceID = constants::NullKeyspaceID;
//         if let Some(codec) = self.Store().GetCodec() {
//             keyspaceID = codec.GetKeyspaceID() as u32;
//         }
//
// NewResourceGroupController 会依赖 PD 与 keyspace；只保留外部依赖调用形状。
//         let control = rmclient::NewResourceGroupController(
//             ctx.clone(),
//             uniqueID,
//             pdClient.unwrap(),
//             None,
//             keyspaceID,
//             rmclient::WithMaxWaitDuration(runaway::MaxWaitDuration),
//         )?;
//         control.Start(ctx.clone());
//
//         let serverInfo = infosync::GetServerInfo()?;
//         let serverAddr = net::JoinHostPort(&serverInfo.IP, &serverInfo.Port.to_string());
//         self.runawayManager = runaway::NewRunawayManager(
//             control.clone(),
//             serverAddr,
//             self.sysSessionPool.clone(),
//             self.exit.clone(),
//             self.infoCache.clone(),
//             self.ddl.clone(),
//         );
//         self.SetResourceGroupsController(control.clone());
//         tikv::SetResourceControlInterceptor(control);
//         Ok(())
//     }
// }
// */
use std::net::IpAddr;
use std::sync::Arc;

/// 资源控制相关配置：实例标识、通告地址与是否启用 RU 模式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceControllerConfig {
    /// 实例唯一 ID。
    pub server_id: u64,
    /// 对外通告 IP。
    pub advertised_ip: IpAddr,
    /// 服务端口。
    pub port: u16,
    /// 是否按 Request Unit 计量模式运行。
    pub request_unit_mode: bool,
}

/// 资源组控制器：启动并暴露配置。
pub trait ResourceGroupController: Send + Sync {
    /// 启动控制器后台逻辑；对应 Go `control.Start(ctx)` 的无返回值契约。
    fn start(&self);
    /// 返回当前配置。
    fn config(&self) -> &ResourceControllerConfig;
}

/// Runaway 管理器：检测并处置失控查询。
///
/// Go 初始化函数只构造并保存 manager；其后台循环由 Domain 后续统一启动。
pub trait RunawayManager: Send + Sync {}

/// 控制器与 runaway 管理器的组合运行时。
pub struct ResourceGroupRuntime {
    /// 资源组控制器。
    pub controller: Arc<dyn ResourceGroupController>,
    /// Runaway 管理器。
    pub runaway_manager: Arc<dyn RunawayManager>,
}

impl ResourceGroupRuntime {
    /// 启动 controller 并返回已装配运行时。
    ///
    /// manager 的 flush/watch 循环不在这里启动，与 Go
    /// `initResourceGroupsController` 的职责边界一致。
    pub fn initialize(self) -> Self {
        self.controller.start();
        self
    }
}
