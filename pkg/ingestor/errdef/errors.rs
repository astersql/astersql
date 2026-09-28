// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Ingest 错误定义：规范化错误（NormalizedError）、TiKV 导入相关 RFC code，以及 HTTP 状态错误。
//
// 对应 Go `pkg/ingestor/errdef`：为 ingest/write 路径提供稳定错误码与消息模板，
// 供包装层按 RFC code 分类重试或失败处理。Region 指 TiKV 数据分片单位。

use std::borrow::Cow;

/// NormalizedError 保留 pingcap/errors.Normalize 生成错误的消息模板与稳定 RFC code。
#[derive(Clone, Debug)]
pub struct NormalizedError {
    /// 错误消息模板；生成实例的展开结果单独保留。
    pub message: Cow<'static, str>,
    /// 跨包装层识别错误类别的稳定键。
    pub rfc_code: &'static str,
    /// `GenWithStackByArgs` 展开后的消息；原模板仍保留在 `message` 中。
    rendered_message: Option<String>,
}

#[allow(non_snake_case)]
impl NormalizedError {
    /// 构造规范化错误。
    pub fn new(message: impl Into<Cow<'static, str>>, rfc_code: &'static str) -> Self {
        Self {
            message: message.into(),
            rfc_code,
            rendered_message: None,
        }
    }

    /// Code 对应未配置 MySQL 数值码时的 Go 零值。
    pub fn Code(&self) -> i32 {
        0
    }

    /// RFCCode 对应 pingcap/errors.Error.RFCCode，供包装层稳定识别错误类别。
    pub fn RFCCode(&self) -> &'static str {
        self.rfc_code
    }

    /// ID 对应 pingcap/errors.Error.ID；本包所有错误都使用 RFC 文本码。
    pub fn ID(&self) -> &'static str {
        self.rfc_code
    }

    /// MessageTemplate 返回生成实例前的消息模板。
    pub fn MessageTemplate(&self) -> &str {
        &self.message
    }

    /// GetMsg 返回模板按当前参数展开后的消息。
    pub fn GetMsg(&self) -> &str {
        self.rendered_message
            .as_deref()
            .unwrap_or(self.message.as_ref())
    }

    /// GetSelfMsg 与 Go 实现一样复用 GetMsg。
    pub fn GetSelfMsg(&self) -> &str {
        self.GetMsg()
    }

    /// Error 保留 pingcap/errors.Error 的 `[RFC code]message` 文本契约。
    pub fn Error(&self) -> String {
        format!("[{}]{}", self.rfc_code, self.GetMsg())
    }

    /// GenWithStack 使用调用方提供的消息替换原型模板，并保留错误类别。
    ///
    /// Rust 没有 Go 的可变参数；需要格式化时由调用方先构造完整消息。
    pub fn GenWithStack(&self, message: impl Into<String>) -> Self {
        Self::new(message.into(), self.rfc_code)
    }

    /// GenWithStackByArgs 对应 Normalize 后按 Go `%d` 模板生成具体错误。
    pub fn GenWithStackByArgs(&self, argument: impl std::fmt::Display) -> Self {
        let mut generated = self.clone();
        generated.rendered_message = Some(self.message.replacen("%d", &argument.to_string(), 1));
        generated
    }

    /// Is 按 RFC ID 判断两个规范化错误是否属于同一类别。
    pub fn Is(&self, other: &Self) -> bool {
        self.rfc_code == other.rfc_code
    }
}

impl PartialEq for NormalizedError {
    fn eq(&self, other: &Self) -> bool {
        self.Is(other)
    }
}

impl Eq for NormalizedError {}

impl std::fmt::Display for NormalizedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.GetMsg())
    }
}

impl std::error::Error for NormalizedError {}

// 以下错误按 Go var 块原顺序声明；RFC code 是跨包装层识别错误类别的稳定键。
/// Region 无 Leader（Leader：Raft 主副本）。
#[allow(non_upper_case_globals)]
pub static ErrNoLeader: NormalizedError = NormalizedError {
    message: Cow::Borrowed("region has no leader, region '%d'"),
    rfc_code: "KV:ErrNoLeader",
    rendered_message: None,
};
/// Epoch 不匹配（Epoch：Region 版本元数据，分裂/合并后会递增）。
#[allow(non_upper_case_globals)]
pub static ErrKVEpochNotMatch: NormalizedError = NormalizedError {
    message: Cow::Borrowed("epoch not match"),
    rfc_code: "Ingest:EpochNotMatch",
    rendered_message: None,
};
/// 当前节点不是 Region Leader。
#[allow(non_upper_case_globals)]
pub static ErrKVNotLeader: NormalizedError = NormalizedError {
    message: Cow::Borrowed("not leader"),
    rfc_code: "Ingest:NotLeader",
    rendered_message: None,
};
/// TiKV 服务繁忙，通常可重试。
#[allow(non_upper_case_globals)]
pub static ErrKVServerIsBusy: NormalizedError = NormalizedError {
    message: Cow::Borrowed("server is busy"),
    rfc_code: "Ingest:ServerIsBusy",
    rendered_message: None,
};
/// 目标 Region 不存在或已迁移。
#[allow(non_upper_case_globals)]
pub static ErrKVRegionNotFound: NormalizedError = NormalizedError {
    message: Cow::Borrowed("region not found"),
    rfc_code: "Ingest:RegionNotFound",
    rendered_message: None,
};
/// ReadIndex 未就绪（ReadIndex：Raft 读一致性确认步骤）。
#[allow(non_upper_case_globals)]
pub static ErrKVReadIndexNotReady: NormalizedError = NormalizedError {
    message: Cow::Borrowed("read index not ready"),
    rfc_code: "Ingest:ReadIndexNotReady",
    rendered_message: None,
};
/// Store 磁盘已满。
#[allow(non_upper_case_globals)]
pub static ErrKVDiskFull: NormalizedError = NormalizedError {
    message: Cow::Borrowed("store disk full"),
    rfc_code: "Ingest:StoreDiskFull",
    rendered_message: None,
};
/// 向 TiKV 导入 SST 失败。
#[allow(non_upper_case_globals)]
pub static ErrKVIngestFailed: NormalizedError = NormalizedError {
    message: Cow::Borrowed("ingest tikv failed"),
    rfc_code: "Ingest:ErrKVIngestFailed",
    rendered_message: None,
};
/// Raft proposal 被丢弃。
#[allow(non_upper_case_globals)]
pub static ErrKVRaftProposalDropped: NormalizedError = NormalizedError {
    message: Cow::Borrowed("raft proposal dropped"),
    rfc_code: "Ingest:ErrKVRaftProposalDropped",
    rendered_message: None,
};

/// IsKVDiskFullError 判断错误本身或任一包装原因是否带有 TiKV 磁盘满 RFC code。
/// 遍历 `source()` 同时覆盖 Go `errors.Is`、`errors.As` 与 `errors.Cause` 所处理的包装层。
#[allow(non_snake_case)]
pub fn IsKVDiskFullError(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(err);
    while let Some(candidate) = current {
        if let Some(normalized) = candidate.downcast_ref::<NormalizedError>() {
            if normalized.rfc_code == ErrKVDiskFull.rfc_code {
                return true;
            }
        }
        // source 为 None 时说明已到根因；函数与 Go 实现一样返回 false，而不是制造新错误。
        current = candidate.source();
    }
    false
}

/// HTTPStatusError 用于 nextgen write/ingest API 收到非 200 且响应体没有错误详情的场景。
/// 调用方可根据 StatusCode 与 Message 判断请求是否适合重试。
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(non_snake_case)]
pub struct HTTPStatusError {
    pub StatusCode: i32,
    pub Message: String,
}

impl std::fmt::Display for HTTPStatusError {
    /// fmt 保留 Go Error 方法的精确输出格式，避免重试分类或日志文本发生漂移。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "request failed with status code {}: {}",
            self.StatusCode, self.Message
        )
    }
}

// 对应 Go 的 `var _ error = (*HTTPStatusError)(nil)` 编译期接口断言。
impl std::error::Error for HTTPStatusError {}

#[allow(non_snake_case)]
impl HTTPStatusError {
    /// Error 保留 Go error 接口的显式方法形状，并复用 Display 的稳定格式。
    pub fn Error(&self) -> String {
        self.to_string()
    }
}
