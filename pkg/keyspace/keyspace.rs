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

// Keyspace（键空间）核心工具：隔离多租户数据与操作的命名空间抽象。
//
// 在 nextgen TiDB 中，每个 keyspace 对应物理集群上的一个逻辑集群。
// 本模块提供：
// - API 版本（V1/V2）与编解码器约定，供 etcd 路径与 TiKV codec 使用；
// - 从全局配置读取 keyspace 名称及其字节缓存；
// - 为日志核心注入 `keyspaceName` 结构化字段；
// - 根据 keyspace 名称构建客户端 API 上下文。

use std::sync::OnceLock;

use crate::{config, kerneltype};

/// The reserved keyspace used by system-level services.
///
/// 系统级服务保留的 keyspace 名称（常量 `"SYSTEM"`）。
pub const System: &str = "SYSTEM";
/// etcd 上 TiDB keyspace 路径前缀，V2 命名空间会拼上 keyspace ID。
const tidbKeyspaceEtcdPathPrefix: &str = "/keyspaces/tidb/";

/// API versions needed by TiKV's keyspace codec.
///
/// TiKV keyspace 编解码所需的 API 版本：
/// - `V1`：无 keyspace 前缀的经典路径；
/// - `V2`：带 keyspace ID 的多租户路径。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiVersion {
    /// 经典 API，不使用 keyspace 命名空间前缀。
    V1,
    /// 多租户 API，路径与编码携带 keyspace ID。
    V2,
}

/// Minimal codec contract used by the namespace helpers.
///
/// 命名空间辅助函数所需的最小编解码契约：查询 API 版本与 keyspace ID。
pub trait Codec {
    /// 返回当前编解码器使用的 API 版本。
    fn api_version(&self) -> ApiVersion;
    /// 返回数值型 keyspace ID（V2 路径拼接用）。
    fn keyspace_id(&self) -> u32;
}

/// 基础编解码器：直接持有 `api_version` 与 `keyspace_id` 字段。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BasicCodec {
    /// API 版本（V1 或 V2）。
    pub api_version: ApiVersion,
    /// Keyspace 数值 ID。
    pub keyspace_id: u32,
}

impl Codec for BasicCodec {
    fn api_version(&self) -> ApiVersion {
        self.api_version
    }

    fn keyspace_id(&self) -> u32 {
        self.keyspace_id
    }
}

/// The transaction API-v1 codec corresponding to Go's package-level CodecV1.
///
/// 对应 Go 包级 `CodecV1`：事务 API-v1 编解码器，keyspace_id 固定为 0。
pub static CodecV1: BasicCodec = BasicCodec {
    api_version: ApiVersion::V1,
    keyspace_id: 0,
};

/// 构造 etcd 上的 keyspace 命名空间路径（无尾部斜杠）。
///
/// V1 返回空串；V2 返回 `/keyspaces/tidb/{id}`。
pub fn MakeKeyspaceEtcdNamespace(c: &dyn Codec) -> String {
    // V1 无 keyspace 隔离前缀，保持与 Go 空字符串语义一致。
    if c.api_version() == ApiVersion::V1 {
        String::new()
    } else {
        format!("{tidbKeyspaceEtcdPathPrefix}{}", c.keyspace_id())
    }
}

/// 构造带尾部斜杠的 etcd keyspace 命名空间路径。
///
/// V1 返回空串；V2 返回 `/keyspaces/tidb/{id}/`，便于继续拼接子路径。
pub fn MakeKeyspaceEtcdNamespaceSlash(c: &dyn Codec) -> String {
    if c.api_version() == ApiVersion::V1 {
        String::new()
    } else {
        format!("{tidbKeyspaceEtcdPathPrefix}{}/", c.keyspace_id())
    }
}

/// 从全局配置读取当前进程绑定的 keyspace 名称。
pub fn GetKeyspaceNameBySettings() -> String {
    config::get_global_keyspace_name()
}

/// 惰性缓存的 keyspace 名称字节切片（对应 Go 的 sync.Once）。
static keyspaceNameBytes: OnceLock<Vec<u8>> = OnceLock::new();

/// Caches the same configuration snapshot as Go's sync.Once implementation.
/// Classic builds expose an empty slice, the Rust equivalent used by callers
/// for Go's nil byte slice.
///
/// 与 Go `sync.Once` 相同：只初始化一次配置快照。
/// Classic 构建返回空切片（对应 Go 的 nil `[]byte`）；
/// NextGen 则缓存全局 keyspace 名称的 UTF-8 字节。
pub fn GetKeyspaceNameBytesBySettings() -> &'static [u8] {
    keyspaceNameBytes
        .get_or_init(|| {
            // NextGen 才真正暴露 keyspace 名称字节；Classic 保持空切片。
            if kerneltype::IsNextGen() {
                config::get_global_keyspace_name().into_bytes()
            } else {
                Vec::new()
            }
        })
        .as_slice()
}

/// 判断 keyspace 名称是否为空（未配置）。
pub fn IsKeyspaceNameEmpty(keyspaceName: &str) -> bool {
    keyspaceName.is_empty()
}

/// Adapter implemented by logging cores that can carry a structured field.
///
/// 可由日志核心实现的适配器：追加结构化键值字段。
pub trait LogCore: Sized {
    /// 附加一个结构化字段并返回新的日志核心。
    fn with_field(self, key: &str, value: &str) -> Self;
}

/// Adds the Go-compatible `keyspaceName` field only for a configured keyspace.
///
/// 仅在已配置 keyspace 时，为日志核心追加与 Go 兼容的 `keyspaceName` 字段。
pub fn WrapZapcoreWithKeyspace<C: LogCore>(core: C) -> C {
    let keyspace_name = GetKeyspaceNameBySettings();
    if keyspace_name.is_empty() {
        core
    } else {
        core.with_field("keyspaceName", &keyspace_name)
    }
}

/// 客户端 API 上下文：V1 无租户，V2 携带 keyspace 名称字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApiContext {
    /// 无 keyspace 的经典上下文。
    V1,
    /// 带 keyspace 名称的多租户上下文。
    V2(String),
}

/// 根据 keyspace 名称构建 API 上下文：空名称 → V1，否则 → V2。
pub fn BuildAPIContext(keyspaceName: &str) -> ApiContext {
    if keyspaceName.is_empty() {
        ApiContext::V1
    } else {
        ApiContext::V2(keyspaceName.to_owned())
    }
}
