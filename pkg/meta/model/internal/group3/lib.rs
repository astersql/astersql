// Copyright 2026 AsterSQL.

// group3 为完整 DDL Job / Placement / Reorg / ResourceGroup 模型提供编译边界。
//
// 通过 `include!` 引入 `job.rs`、`placement.rs`、`masking_policy.rs`、`reorg.rs`、
// `resource_group.rs`、`table_mode.rs`；DB/Table/SchemaState/AST 复用 group1 的
// 正式生产身份，其余基础适配保持独立 crate 可编译。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// AST 类型统一来自 group1 的正式 parser AST 身份。
pub mod ast {
    pub use group_1::ast::*;
}

/// 错误桩类型。
pub mod errors {
    pub type ErrorID = u64;
    pub type Error = String;

    pub fn Trace<E: std::fmt::Display>(error: E) -> Error {
        error.to_string()
    }
}

/// MySQL SQL Mode 桩。
pub mod mysql {
    pub type SQLMode = u64;
}

/// terror 错误桩，与同包 Job 字段类型对齐。
pub mod terror {
    pub type Error = String;
}

/// 会话追踪信息桩。
pub mod tracing {
    use serde::{Deserialize, Serialize};

    mod trace_id {
        use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

        pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let Some(value) = value else {
                return serializer.serialize_none();
            };
            let mut encoded = String::with_capacity(value.len().div_ceil(3) * 4);
            for chunk in value.chunks(3) {
                let bits = (u32::from(chunk[0]) << 16)
                    | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                    | u32::from(*chunk.get(2).unwrap_or(&0));
                encoded.push(ALPHABET[((bits >> 18) & 0x3f) as usize] as char);
                encoded.push(ALPHABET[((bits >> 12) & 0x3f) as usize] as char);
                encoded.push(if chunk.len() > 1 {
                    ALPHABET[((bits >> 6) & 0x3f) as usize] as char
                } else {
                    '='
                });
                encoded.push(if chunk.len() > 2 {
                    ALPHABET[(bits & 0x3f) as usize] as char
                } else {
                    '='
                });
            }
            serializer.serialize_str(&encoded)
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
        where
            D: Deserializer<'de>,
        {
            let Some(encoded) = Option::<String>::deserialize(deserializer)? else {
                return Ok(None);
            };
            if encoded.len() % 4 != 0 {
                return Err(D::Error::custom("invalid base64 trace_id length"));
            }
            let mut decoded = Vec::with_capacity(encoded.len() / 4 * 3);
            for chunk in encoded.as_bytes().chunks_exact(4) {
                let mut values = [0_u8; 4];
                for (index, byte) in chunk.iter().copied().enumerate() {
                    values[index] = match byte {
                        b'A'..=b'Z' => byte - b'A',
                        b'a'..=b'z' => byte - b'a' + 26,
                        b'0'..=b'9' => byte - b'0' + 52,
                        b'+' => 62,
                        b'/' => 63,
                        b'=' if index >= 2 => 0,
                        _ => return Err(D::Error::custom("invalid base64 trace_id")),
                    };
                }
                if chunk[2] == b'=' && chunk[3] != b'=' {
                    return Err(D::Error::custom("invalid base64 trace_id padding"));
                }
                let bits = (u32::from(values[0]) << 18)
                    | (u32::from(values[1]) << 12)
                    | (u32::from(values[2]) << 6)
                    | u32::from(values[3]);
                decoded.push((bits >> 16) as u8);
                if chunk[2] != b'=' {
                    decoded.push((bits >> 8) as u8);
                }
                if chunk[3] != b'=' {
                    decoded.push(bits as u8);
                }
            }
            Ok(Some(decoded))
        }
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 连接 ID 与会话别名，供 Job 记录诊断上下文。
    pub struct TraceInfo {
        #[serde(rename = "connection_id")]
        pub ConnectionID: u64,
        #[serde(rename = "session_alias")]
        pub SessionAlias: String,
        #[serde(default, rename = "trace_id", with = "trace_id")]
        pub TraceID: Option<Vec<u8>>,
    }
}

/// DDL 重组相关系统变量默认值桩。
pub mod vardef {
    /// 默认重组并行 worker 数。
    pub fn GetDDLReorgWorkerCounter() -> i64 {
        4
    }

    /// 默认重组批大小。
    pub fn GetDDLReorgBatchSize() -> i64 {
        256
    }
}

/// 内核形态探测桩；NextGen 影响默认 Job 协议版本。
pub mod kerneltype {
    /// 是否为 NextGen 内核（桩恒为 false）。
    pub fn is_next_gen() -> bool {
        false
    }
}

/// 时区 Location 加载/固定偏移构造桩。
pub mod time {
    #[derive(Clone, Debug, Eq, PartialEq)]
    /// 时区名称与固定偏移（秒）。
    pub struct Location {
        pub name: String,
        pub offset: i32,
    }

    #[derive(Clone, Debug)]
    /// 时区加载错误。
    pub struct Error(pub String);

    /// 按名称加载时区；空名返回错误。
    pub fn load_location(name: &str) -> Result<Location, Error> {
        if name.is_empty() {
            Err(Error("empty time zone name".to_owned()))
        } else {
            Ok(Location {
                name: name.to_owned(),
                offset: 0,
            })
        }
    }

    /// 构造固定偏移时区。
    pub fn fixed_zone(name: &str, offset: i32) -> Location {
        Location {
            name: name.to_owned(),
            offset,
        }
    }
}

/// Job 历史元数据与表模型共享 group1 的正式类型身份。
pub use group_1::{DBInfo, TableInfo, TimeZoneLocation};

/// Job 普通参数接口：V1 数组布局与 JSON 表示。
pub trait JobArgs {
    fn get_args_v1(&self, job: &job::Job) -> Vec<serde_json::Value>;
    fn to_json(&self) -> serde_json::Value;
}

/// 完成态 Job 参数接口；结束后回写结果时使用。
pub trait FinishedJobArgs: JobArgs {
    fn get_finished_args_v1(&self, job: &job::Job) -> Vec<serde_json::Value>;
}

/// 时间戳转换桩（恒等）；Job Display 使用。
pub fn ts_convert_to_time(ts: u64) -> u64 {
    ts
}

// 引入正式 job.rs；依赖本文件提供的桩与同 crate 其它子模块。
mod job {
    use crate::reorg::{DDLReorgMeta, ReorgStage, ReorgType};
    use crate::terror;
    use crate::{
        DBInfo, FinishedJobArgs, JobArgs, TableInfo, ast, errors, kerneltype, mysql, time, tracing,
        ts_convert_to_time,
    };
    use group_1::SchemaState;
    include!("../../job.rs");
}
pub use group_1::SchemaState;
pub use job::*;

// Placement Policy 模型。
mod placement {
    use crate::{SchemaState, ast};
    include!("../../placement.rs");
}
pub use placement::*;

// 数据脱敏策略模型。
mod masking_policy {
    use crate::{SchemaState, ast};
    include!("../../masking_policy.rs");
}
pub use masking_policy::*;

// DDL 重组（reorg）元数据与 backfill 状态机。
mod reorg {
    use crate::{JobMeta, TimeZoneLocation, errors, mysql, terror, vardef};
    include!("../../reorg.rs");
}
pub use reorg::*;

// 资源组设置与 Runaway 管控。
mod resource_group {
    use crate::{SchemaState, ast, placement};
    include!("../../resource_group.rs");
}
pub use resource_group::*;

// 表模式及其合法迁移。
mod table_mode {
    use crate::ast;
    include!("../../table_mode.rs");
}
pub use table_mode::*;

/// 构造带指定 ID 的 `Arc<TableInfo>` 测试辅助。
pub fn arc_table(id: i64) -> Arc<TableInfo> {
    Arc::new(TableInfo {
        ID: id,
        ..Default::default()
    })
}
