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

// 本文件由 pkg/config/deploymode/mode.go 迁移而来，保留 Go 实现结构。

// 部署模式（deploy mode）模块。
//
// 定义 TiDB 实例的部署模式枚举（Premium / PremiumReserved / Starter），
// 并提供全局的模式读写、字符串解析与 JSON/TOML 反序列化能力。
// 部署模式决定集群资源的分配策略：例如是否按需扩缩容各类 worker 组件。
// 该模式在 TiDB 启动阶段一次性设置，之后不可更改，且仅对
// nextgen（下一代内核形态，见 `kerneltype` 模块）生效。

use std::sync::atomic::{AtomicI32, Ordering};

use crate::kerneltype;

/// Premium 模式的字符串名称。
const premiumName: &str = "premium";
/// PremiumReserved 模式的字符串名称。
const premiumReservedName: &str = "premium_reserved";
/// Starter 模式的字符串名称。
const starterName: &str = "starter";

// Mode 对应 Go 的 int32 类型别名：表示 TiDB 实例的部署模式。
// 这里用单字段结构保留 Go 可以携带非法整数值的语义，便于 String/Valid 复刻默认分支。
/// 部署模式。内部包装一个 `i32`，合法取值见 `Premium`、`PremiumReserved`、`Starter` 常量。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mode(pub i32);

// Premium is the default deployment mode.
/// Premium：默认部署模式，资源可按需弹性伸缩。
pub const Premium: Mode = Mode(0);
// PremiumReserved is the reserved premium deployment mode. In Premium Reserved,
// resources are fixed when the cluster starts. TiDB-worker, TiKV-worker, and
// coprocessor-worker are not scaled on demand.
/// PremiumReserved：资源预留模式。集群启动时资源即固定，
/// TiDB-worker、TiKV-worker（分布式 KV 存储层工作节点）与
/// coprocessor-worker（协处理器，负责下推计算）均不按需扩缩容。
pub const PremiumReserved: Mode = Mode(1);
// Starter is for deployments that support a large number of small tenants.
/// Starter：面向大量小租户（multi-tenant）场景的部署模式。
pub const Starter: Mode = Mode(2);

// currentMode 对应 Go 的 atomic.Int32 全局变量。
// Rust 使用 AtomicI32 保留原子读写语义，默认值 0 与 Go 零值 Premium 对齐。
static currentMode: AtomicI32 = AtomicI32::new(Premium.0);

// Get returns the current deployment mode.
/// 返回当前进程的部署模式（原子读取全局状态）。
pub fn Get() -> Mode {
    Mode(currentMode.load(Ordering::SeqCst))
}

// IsPremiumReserved returns true if the current deployment mode is PremiumReserved.
/// 判断当前是否运行在 PremiumReserved 模式；仅当内核为 nextgen 时才可能为真。
pub fn IsPremiumReserved() -> bool {
    kerneltype::IsNextGen() && Get() == PremiumReserved
}

// IsStarter returns true if the current deployment mode is Starter.
/// 判断当前是否运行在 Starter 模式；仅当内核为 nextgen 时才可能为真。
pub fn IsStarter() -> bool {
    kerneltype::IsNextGen() && Get() == Starter
}

// Set sets the current deployment mode during TiDB startup.
// The deployment mode cannot be changed after it is set.
/// 在 TiDB 启动阶段设置部署模式，设置后不可再更改。
/// 约束：仅 nextgen 内核允许设置，且 mode 必须是合法取值。
pub fn Set(mode: Mode) -> Result<(), String> {
    if !kerneltype::IsNextGen() {
        return Err("deploy mode can only be set for nextgen TiDB".to_string());
    }
    if !mode.Valid() {
        return Err(format!("invalid deploy mode {}", mode.0));
    }

    // Go 使用 currentMode.Store(int32(mode))；这里保留顺序一致的原子写入。
    currentMode.store(mode.0, Ordering::SeqCst);
    Ok(())
}

// Parse returns the deployment mode for the given string.
/// 将字符串解析为部署模式，大小写不敏感；无法识别时返回错误。
pub fn Parse(s: &str) -> Result<Mode, String> {
    // Go 使用 strings.ToLower，不会 trim 空白；保持相同大小写归一化范围。
    match s.to_lowercase().as_str() {
        premiumName => Ok(Premium),
        premiumReservedName => Ok(PremiumReserved),
        starterName => Ok(Starter),
        _ => Err(format!("invalid deploy mode {:?}", s)),
    }
}

impl Mode {
    // String returns the string representation of the deployment mode.
    /// 返回部署模式的字符串表示；非法值返回 `unknown(<数值>)`，复刻 Go 的默认分支。
    pub fn String(self) -> String {
        match self {
            Premium => premiumName.to_string(),
            PremiumReserved => premiumReservedName.to_string(),
            Starter => starterName.to_string(),
            _ => format!("unknown({})", self.0),
        }
    }

    // Valid returns true if the deployment mode is valid.
    /// 判断当前值是否为合法的部署模式。
    pub fn Valid(self) -> bool {
        matches!(self, Premium | PremiumReserved | Starter)
    }

    // MarshalJSON implements json.Marshaler.
    /// 序列化为 JSON（对应 Go 的 json.Marshaler 接口）：
    /// 先校验合法性，再将字符串名称编码为 JSON 字节。
    pub fn MarshalJSON(self) -> Result<Vec<u8>, String> {
        if !self.Valid() {
            return Err(format!("invalid deploy mode {}", self.0));
        }

        // Go 调用 json.Marshal(m.String())；这里保留返回字节数组的形状。
        serde_json::to_vec(&self.String()).map_err(|err| err.to_string())
    }

    // UnmarshalJSON implements json.Unmarshaler.
    /// 从 JSON 字节反序列化（对应 Go 的 json.Unmarshaler 接口）：
    /// 先解出字符串，再经 `Parse` 转换为部署模式并就地赋值。
    pub fn UnmarshalJSON(&mut self, data: &[u8]) -> Result<(), String> {
        let s: String = serde_json::from_slice(data).map_err(|err| err.to_string())?;
        let mode = Parse(&s)?;
        *self = mode;
        Ok(())
    }

    // UnmarshalTOML implements toml.Unmarshaler.
    /// 从 TOML 值反序列化（对应 Go 的 toml.Unmarshaler 接口）：
    /// 仅接受字符串类型的 TOML 值，其余类型视为非法。
    pub fn UnmarshalTOML(&mut self, v: &toml::Value) -> Result<(), String> {
        let toml::Value::String(s) = v else {
            return Err(format!("invalid deploy mode {v}"));
        };
        let mode = Parse(s)?;
        *self = mode;
        Ok(())
    }
}

// ModeList returns all valid deployment modes.
/// 返回全部合法部署模式的列表。
pub fn ModeList() -> Vec<Mode> {
    vec![Premium, PremiumReserved, Starter]
}
