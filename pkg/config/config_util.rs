// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 配置工具模块：提供配置对象的深拷贝、动态配置项合并与配置扁平化等辅助函数。
//
// 背景说明：数据库服务端的配置分为两类——
// - 静态配置：仅能在启动时指定，运行期间不可变更；
// - 动态配置：允许在运行期间在线修改（无需重启进程），由白名单
//   `dynamicConfigItems` 控制哪些字段允许动态更新。
//
// 本模块借助 serde_json 的序列化/反序列化能力模拟 Go 版本中基于反射
// 的通用配置遍历逻辑：JSON Object 对应结构体、数组与标量对应叶子字段。

use serde::{Serialize, de::DeserializeOwned};
use std::collections::{HashMap, HashSet};

use super::{Config, tikvcfg};

// CloneConf 对应 Go 的同名函数：先 JSON 序列化再反序列化，得到深拷贝配置。
// JSON 往返会复用 Config 的 serde/TOML 标签语义；这里保留错误向上传递形状。
/// 深拷贝一个配置对象。
///
/// 通过“序列化为 JSON 字节 → 再反序列化”的往返实现深拷贝，
/// 避免为每个配置结构体手写 Clone；任一阶段失败都会把
/// `serde_json::Error` 原样返回给调用方。
pub fn CloneConf<T>(conf: &T) -> Result<T, serde_json::Error>
where
    T: Serialize + DeserializeOwned,
{
    let content = serde_json::to_vec(conf)?;
    let cloned_conf = serde_json::from_slice(&content)?;
    Ok(cloned_conf)
}

// dynamicConfigItems 对应 Go 的运行期可变配置白名单。
// Go 用 map[string]struct{} 做集合；用 HashSet 表达同样的成员测试语义。
/// 返回允许在运行期动态修改的配置项白名单。
///
/// 键为以 `.` 分隔的字段路径（Go 风格驼峰命名），例如
/// `"Performance.MaxMemory"`。只有出现在该集合中的叶子字段
/// 才会被 [`MergeConfigItems`] 接受并覆盖，其余字段一律拒绝。
/// 其中 `TiKVClient.StoreLimit` 涉及 TiKV（分布式 KV 存储引擎）
/// 客户端对单个 Store（存储节点）的请求限流。
pub fn dynamicConfigItems() -> HashSet<&'static str> {
    HashSet::from([
        "Performance.MaxProcs",
        "Performance.MaxMemory",
        "Performance.CrossJoin",
        "Performance.PseudoEstimateRatio",
        "Performance.StmtCountLimit",
        "Performance.TCPKeepAlive",
        "TiKVClient.StoreLimit",
        "Log.Level",
        "Log.ExpensiveThreshold",
        "Instance.SlowThreshold",
        "Instance.CheckMb4ValueInUTF8",
        "TxnLocalLatches.Capacity",
        "CompatibleKillQuery",
        "TreatOldVersionUTF8AsUTF8MB4",
        "OpenTracing.Enable",
    ])
}

// MergeConfigItems 对应 Go 的入口：只覆盖白名单中的动态配置项。
/// 将 `new_conf` 中的动态配置项合并进 `dst_conf`。
///
/// 返回二元组 `(accepted, rejected)`：
/// - `accepted`：成功覆盖的字段路径（在白名单内且值发生了变化）；
/// - `rejected`：因不在白名单而被拒绝修改的字段路径。
///
/// 实现上先把两份配置都转成 `serde_json::Value` 树，在树上做递归
/// 合并，最后再反序列化写回 `dst_conf`；两处 `expect` 的前提是入参
/// 本身就是合法配置，序列化/回写理应不会失败。
pub fn MergeConfigItems<T>(dst_conf: &mut T, new_conf: &T) -> (Vec<String>, Vec<String>)
where
    T: Serialize + DeserializeOwned,
{
    let mut dst_value =
        serde_json::to_value(&*dst_conf).expect("serializing an already valid config must succeed");
    let new_value =
        serde_json::to_value(new_conf).expect("serializing an already valid config must succeed");
    let (accepted, rejected) = mergeConfigItems(&mut dst_value, &new_value, "");
    *dst_conf = serde_json::from_value(dst_value)
        .expect("merging values from the same config type must remain deserializable");
    (accepted, rejected)
}

// mergeConfigItems 对应 Go 的递归反射实现。JSON object 对应结构体，数组和标量对应叶子。
/// 在 JSON 值树上递归合并配置项（[`MergeConfigItems`] 的内部实现）。
///
/// `field_path` 是当前节点在配置树中的累计路径（如 `"Log.Level"`），
/// 用于与白名单比对；返回值语义与 [`MergeConfigItems`] 相同。
pub fn mergeConfigItems(
    dst_conf: &mut serde_json::Value,
    new_conf: &serde_json::Value,
    field_path: &str,
) -> (Vec<String>, Vec<String>) {
    // 两侧子树完全相同则无需合并，直接短路返回。
    if dst_conf == new_conf {
        return (Vec::new(), Vec::new());
    }

    // 两侧均为 JSON Object（对应结构体）时逐字段递归下钻；
    // 新配置中缺失的字段跳过，路径按 Go 驼峰命名规则拼接。
    if let (serde_json::Value::Object(dst), serde_json::Value::Object(new)) =
        (&mut *dst_conf, new_conf)
    {
        let mut accepted = Vec::new();
        let mut rejected = Vec::new();
        for (field, dst_value) in dst {
            let Some(new_value) = new.get(field) else {
                continue;
            };
            let path = if field_path.is_empty() {
                goFieldName(field)
            } else {
                format!("{field_path}.{}", goFieldName(field))
            };
            let (mut child_accepted, mut child_rejected) =
                mergeConfigItems(dst_value, new_value, &path);
            accepted.append(&mut child_accepted);
            rejected.append(&mut child_rejected);
        }
        return (accepted, rejected);
    }

    // 走到叶子字段（标量或数组）：在白名单中则覆盖并记为 accepted，
    // 否则保持原值并记为 rejected。
    if dynamicConfigItems().contains(field_path) {
        *dst_conf = new_conf.clone();
        (vec![field_path.to_owned()], Vec::new())
    } else {
        (Vec::new(), vec![field_path.to_owned()])
    }
}

/// 把 serde 序列化产生的字段名（kebab-case / snake_case，如
/// `max-memory`）还原为 Go 结构体字段的驼峰命名（如 `MaxMemory`），
/// 以便与白名单中的路径进行匹配；`tikv` 特判为 `TiKV`。
fn goFieldName(field: &str) -> String {
    // `Instance.SlowThreshold` 的公开配置键带 `tidb_` 前缀，无法从
    // serde 字段名机械还原，因此显式映射回 Go 结构体字段名。
    if field == "tidb_slow_log_threshold" {
        return "SlowThreshold".to_owned();
    }
    field
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| match part {
            "tikv" => "TiKV".to_owned(),
            _ => {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect()
}

// ConfReloadFunc 对应 Go 的配置重载回调类型。
/// 配置热重载回调函数类型：在配置更新时被调用，
/// 入参分别为旧配置与新配置，供调用方感知差异并做出响应。
pub type ConfReloadFunc = fn(old_conf: &Config, new_conf: &Config);

// FlattenConfigItems 对应 Go 的配置 map 扁平化入口。
/// 将嵌套的配置 map 扁平化为单层 map。
///
/// 嵌套键会以 `.` 连接成路径作为新键，例如
/// `{"log": {"level": "info"}}` 会变为 `{"log.level": "info"}`；
/// 常用于把层级化配置转成便于查找/展示的键值列表。
pub fn FlattenConfigItems(
    nested_config: HashMap<String, serde_json::Value>,
) -> HashMap<String, serde_json::Value> {
    let mut flat_map = HashMap::new();
    flatten(
        &mut flat_map,
        serde_json::to_value(nested_config).unwrap_or_default(),
        "",
    );
    flat_map
}

// flatten 对应 Go 的递归：map 继续展开，数组和标量保持为叶子值。
/// 递归展开嵌套 JSON 值（[`FlattenConfigItems`] 的内部实现）。
///
/// `prefix` 为当前累计的键路径；遇到 Object 继续下钻拼接路径，
/// 遇到数组或标量则以当前路径为键写入 `flat_map`。
pub fn flatten(
    flat_map: &mut HashMap<String, serde_json::Value>,
    nested: serde_json::Value,
    prefix: &str,
) {
    match nested {
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                let path = if prefix.is_empty() {
                    key
                } else {
                    format!("{}.{}", prefix, key)
                };
                flatten(flat_map, value, &path);
            }
        }
        value => {
            flat_map.insert(prefix.to_string(), value);
        }
    }
}

// GetTxnScopeFromConfig 对应 Go 对 tikvcfg 的薄封装。
/// 从 TiKV 客户端配置读取事务作用域（txn scope）。
///
/// 事务作用域用于区分全局事务与仅限某地域（zone）的本地事务，
/// 是多地域部署下降低跨地域提交延迟的机制；此处直接转发给
/// `tikvcfg` 子模块的同名实现。
pub fn GetTxnScopeFromConfig() -> String {
    tikvcfg::GetTxnScopeFromConfig()
}
