// Copyright 2026 AsterSQL.

// DDL Label（标签）规则 crate 的入口模块。
//
// 本 crate 负责将表/分区的 attributes（属性，如 `merge_option=allow`）
// 转换为 PD（Placement Driver）可消费的 Label Rule，并把表 ID 映射为
// Region（TiKV 数据分片）的 key range。主要职责：
// - 重新导出依赖类型，模拟原 Go 包的引用路径；
// - 提供 PD `RegionLabel` / `LabelRule` / `LabelRulePatch` 数据结构；
// - 提供 TiKV Keyspace（键空间，NextGen 多租户隔离单元）感知的 Codec；
// - 组织 `attributes`、`errors`、`rule` 等核心子模块。

#![allow(non_snake_case, non_upper_case_globals)]

/// AST（抽象语法树）相关类型的兼容命名空间，导出 `AttributesSpec`。
pub mod ast {
    pub use parser_ast_dependency::AttributesSpec;
}

/// 内核类型判断（Classic / NextGen）的兼容命名空间。
pub mod kerneltype {
    pub use kerneltype_dependency::*;
}

/// 字节编解码工具的兼容命名空间（如 `EncodeBytes`）。
pub mod codec {
    pub use codec_dependency::*;
}

/// 表键前缀生成工具：按 table ID 生成 Region 扫描起止前缀。
pub mod tablecodec {
    pub use tablecodec_dependency::GenTablePrefix;
}

/// PD Label Rule 相关数据结构，字段命名与 JSON 序列化形状对齐 Go 侧。
pub mod pd {
    use serde::{Deserialize, Serialize};

    /// 单条 Region Label：`key`/`value` 以及可选的 TTL、生效起点。
    #[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
    #[serde(default)]
    pub struct RegionLabel {
        #[serde(rename = "key")]
        pub Key: String,
        #[serde(rename = "value")]
        pub Value: String,
        #[serde(rename = "ttl", skip_serializing_if = "String::is_empty")]
        pub TTL: String,
        #[serde(rename = "start_at", skip_serializing_if = "String::is_empty")]
        pub StartAt: String,
    }

    /// 完整的 Label Rule：绑定一组 labels 到若干 key range（`data` 字段）。
    #[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
    #[serde(default)]
    pub struct LabelRule {
        #[serde(rename = "id")]
        pub ID: String,
        #[serde(rename = "index")]
        pub Index: isize,
        #[serde(rename = "labels")]
        pub Labels: Vec<RegionLabel>,
        #[serde(rename = "rule_type")]
        pub RuleType: String,
        #[serde(rename = "data")]
        pub Data: serde_json::Value,
    }

    /// 批量更新 Label Rule 的补丁：设置规则列表与待删除规则 ID。
    #[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
    pub struct LabelRulePatch {
        #[serde(default, rename = "sets")]
        pub SetRules: Vec<Box<LabelRule>>,
        #[serde(default, rename = "deletes")]
        pub DeleteRules: Vec<String>,
    }
}

/// TiKV Keyspace 与 Region key 编码相关的轻量 Codec 实现。
pub mod tikv {
    /// Keyspace 元信息，目前仅保留 24-bit 的 Keyspace ID。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct KeyspaceMeta {
        pub Id: u32,
    }

    /// Keyspace ID 超出 client-go 支持的 uint24 范围。
    #[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
    #[error("keyspaceID {keyspace_id} is out of range, maximum is 16777215")]
    pub struct CodecError {
        keyspace_id: u32,
    }

    /// API 版本：V1 无 keyspace 前缀；V2 携带 KeyspaceMeta。
    #[derive(Clone, Debug, Eq, PartialEq)]
    enum ApiVersion {
        V1,
        V2(KeyspaceMeta),
    }

    /// Key 编码器；`None` 表示空 Codec（NilCodec）。
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Codec(Option<ApiVersion>);

    /// 创建 Classic/V1 Codec（不对 key 加 keyspace 前缀）。
    pub fn NewCodecV1() -> Codec {
        Codec(Some(ApiVersion::V1))
    }

    /// 创建 NextGen/V2 Codec，keyspace ID 必须能放入 24 位无符号整数。
    pub fn NewCodecV2(keyspace_id: u32) -> Result<Codec, CodecError> {
        if keyspace_id > 0x00ff_ffff {
            return Err(CodecError { keyspace_id });
        }
        Ok(Codec(Some(ApiVersion::V2(KeyspaceMeta {
            Id: keyspace_id,
        }))))
    }

    /// 创建空 Codec，表示未绑定任何 API 版本。
    pub fn NilCodec() -> Codec {
        Codec(None)
    }

    impl Codec {
        /// 是否为空 Codec。
        pub fn is_nil(&self) -> bool {
            self.0.is_none()
        }

        /// 仅 V2 返回 Keyspace 元信息。
        pub fn GetKeyspaceMeta(&self) -> Option<&KeyspaceMeta> {
            match &self.0 {
                Some(ApiVersion::V2(meta)) => Some(meta),
                _ => None,
            }
        }

        /// 返回 Keyspace ID；非 V2 时返回 0。
        pub fn GetKeyspaceID(&self) -> u32 {
            self.GetKeyspaceMeta().map_or(0, |meta| meta.Id)
        }

        /// 将表前缀起止键编码为 Region 边界键。
        ///
        /// V2 会先拼 `x` + 3 字节 keyspace ID 前缀，再做 mem-comparable 编码。
        pub fn EncodeRegionRange(&self, start: Vec<u8>, end: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
            match &self.0 {
                Some(ApiVersion::V2(meta)) => {
                    // 构造 4 字节 keyspace 前缀：'x' 后跟 24-bit big-endian ID。
                    let prefix = [
                        b'x',
                        ((meta.Id >> 16) & 0xff) as u8,
                        ((meta.Id >> 8) & 0xff) as u8,
                        (meta.Id & 0xff) as u8,
                    ];
                    let mut start_with_prefix = prefix.to_vec();
                    start_with_prefix.extend(start);
                    let end_with_prefix = if end.is_empty() {
                        (u32::from_be_bytes(prefix) + 1).to_be_bytes().to_vec()
                    } else {
                        let mut encoded_end = prefix.to_vec();
                        encoded_end.extend(end);
                        encoded_end
                    };
                    (
                        codec_dependency::EncodeBytes(Vec::new(), &start_with_prefix),
                        codec_dependency::EncodeBytes(Vec::new(), &end_with_prefix),
                    )
                }
                _ => {
                    let encoded_start = codec_dependency::EncodeBytes(Vec::new(), &start);
                    let encoded_end = if end.is_empty() {
                        Vec::new()
                    } else {
                        codec_dependency::EncodeBytes(Vec::new(), &end)
                    };
                    (encoded_start, encoded_end)
                }
            }
        }
    }
}

/// 属性字符串解析与 Labels 集合操作。
pub mod attributes;
/// Label 相关错误类型与文案常量。
pub mod errors;
pub use attributes::*;
/// Label Rule 构建、重置与补丁生成逻辑。
pub mod rule;
pub use rule::*;

#[cfg(test)]
#[path = "attributes_test.rs"]
mod attributes_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "rule_test.rs"]
mod rule_test;
