// Copyright 2026 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 外部工作负载（external-workload）配置模块。
//
// 对应 TiDB 配置文件中的 `[external-workload]` 段（仅 Starter 版本使用），
// 用于把本实例声明为承担某种外部后台任务的节点，例如：
// - GC（垃圾回收，清理 MVCC 多版本数据中已过期的历史版本）工作节点；
// - TTL（Time To Live，按行过期时间自动删除数据）任务工作节点；
// - 自动统计信息收集（auto-analyze，为优化器生成执行计划提供统计数据）工作节点。
//
// 模块提供角色常量、配置结构体 [`ExternalWorkload`] 及其规范化/校验逻辑。

// ExternalWorkloadRole is the role name accepted by [external-workload].
/// 外部工作负载角色名类型别名，对应 Go 中基于 string 的类型别名。
/// 取值见下方 `Role*` 常量。
pub type ExternalWorkloadRole = String;

// External workload roles accepted by the [external-workload] config.
// 这些常量对应 Go 的字符串类型别名常量，保持原始配置文本不变。
/// 主控角色：负责协调其余外部工作负载节点。
pub const RoleMaster: &str = "master";
/// GC v2 工作节点角色：执行第二代垃圾回收任务，清理过期的 MVCC 历史版本。
pub const RoleGCV2Worker: &str = "gcv2";
/// TTL 任务工作节点角色：扫描并删除超过存活时间（Time To Live）的行。
pub const RoleTTLTaskWorker: &str = "ttl";
/// 自动统计信息收集工作节点角色：后台执行 auto-analyze，更新优化器统计信息。
pub const RoleAutoAnalyzeWorker: &str = "auto-analyze";

// ExternalWorkload is the Starter-only [external-workload] section.
/// `[external-workload]` 配置段（仅 Starter 版本），描述本实例的外部工作负载设置。
///
/// 通过 serde 的 `kebab-case` 重命名与 `default` 属性，与 Go 侧 toml/json
/// 标签（如 `tidb-pool`、`controller-addr`）保持一致的序列化行为。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ExternalWorkload {
    // Go tags: toml:"enable" json:"enable,omitempty"。
    /// 是否启用外部工作负载模式。
    pub Enable: bool,
    // Go tags: toml:"role" json:"role,omitempty"。
    /// 本实例承担的角色，取值为 `Role*` 常量之一；空值在校验时默认为 master。
    pub Role: ExternalWorkloadRole,
    // TidbPool names the serving pool, for example vip-tidb-pool or super-vip-tidb-pool.
    // Go tags: toml:"tidb-pool" json:"tidb-pool,omitempty"。
    /// 所属服务池名称，例如 vip-tidb-pool、super-vip-tidb-pool。
    pub TidbPool: String,
    // ControllerAddr is the external workload controller address.
    // Go tags: toml:"controller-addr" json:"controller-addr,omitempty"。
    /// 外部工作负载控制器（controller）的地址，启用时不可为空。
    pub ControllerAddr: String,
}

// defaultExternalWorkload 对应 Go 的同名函数，返回结构体零值。
/// 返回默认（零值）的外部工作负载配置：未启用、各字段为空字符串。
pub fn defaultExternalWorkload() -> ExternalWorkload {
    ExternalWorkload::default()
}

impl ExternalWorkload {
    // Valid normalizes and validates an enabled [external-workload] section.
    /// 规范化并校验已启用的 `[external-workload]` 配置。
    ///
    /// 未启用时直接返回 Ok；启用时会就地修改字段（规范化 role、去除
    /// 地址与池名的首尾空白），随后逐项校验，任一不合法则返回错误信息。
    pub fn Valid(&mut self) -> Result<(), String> {
        if !self.Enable {
            return Ok(());
        }

        // Go 会在启用后修改接收者字段：role 先规范化，空 role 默认 master。
        self.Role = normalized(&self.Role);
        if self.Role.is_empty() {
            self.Role = RoleMaster.to_string();
        }

        // Go 使用 strings.TrimSpace 就地清理 controller-addr 和 tidb-pool。
        self.ControllerAddr = self.ControllerAddr.trim().to_string();
        self.TidbPool = self.TidbPool.trim().to_string();

        if self.ControllerAddr.is_empty() {
            return Err(
                "external-workload controller-addr must not be empty when enabled".to_string(),
            );
        }
        if !valid(&self.Role) {
            return Err(format!("invalid external-workload role {:?}", self.Role));
        }
        if self.TidbPool.is_empty() {
            return Err("external-workload tidb-pool must not be empty when enabled".to_string());
        }

        Ok(())
    }

    // isConfigured 对应 Go 的非导出方法：只检查是否写过任一配置项，不要求配置有效。
    /// 判断用户是否在配置文件中填写过任一 `[external-workload]` 字段。
    pub fn isConfigured(&self) -> bool {
        self.Enable
            || !normalized(&self.Role).is_empty()
            || !self.ControllerAddr.trim().is_empty()
            || !self.TidbPool.trim().is_empty()
    }
}

// normalized 对应 Go 的 ExternalWorkloadRole.normalized 方法。
// Go 先 TrimSpace 再 ToLower；这里保持同样的参数解析顺序。
/// 规范化角色名：去除首尾空白并转为小写。
pub fn normalized(r: &ExternalWorkloadRole) -> ExternalWorkloadRole {
    r.trim().to_lowercase()
}

// valid 对应 Go 的 ExternalWorkloadRole.valid 方法。
/// 校验角色名是否为受支持的取值（master/gcv2/ttl/auto-analyze）。
pub fn valid(r: &ExternalWorkloadRole) -> bool {
    matches!(
        r.as_str(),
        RoleMaster | RoleGCV2Worker | RoleTTLTaskWorker | RoleAutoAnalyzeWorker
    )
}
