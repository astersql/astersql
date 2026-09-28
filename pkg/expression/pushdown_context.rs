// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Owned planner-to-expression context used while constructing push-down PB.
//
// 下推（push-down）构建上下文：规划器把可下推表达式序列化为 TiPB
//（TiKV/TiFlash 侧协议缓冲）时，需要求值上下文、KV 客户端与告警路由。
// 对应 Go `PushDownContext` 的 owned trait-object 形态。

use std::sync::Arc;

use crate::expr_to_pb_kernel::{NewPBConverter, PbConverter};
use crate::{contextutil, exprctx, kv};

/// 表达式构建上下文的共享引用。
pub type PushDownBuildContextRef = Arc<dyn exprctx::BuildContext>;
/// 下推序列化所需的 KV 客户端共享引用。
pub type PushDownClientRef = Arc<dyn kv::Client + Send + Sync>;
/// 告警追加器共享引用。
pub type PushDownWarnAppenderRef = Arc<dyn contextutil::WarnAppender + Send + Sync>;

/// Go's interface-valued `PushDownContext`, represented by owned Rust trait objects.
///
/// 持有求值上下文、可选 KV 客户端、告警处理器，以及 `GROUP_CONCAT` 长度上限。
#[derive(Clone)]
pub struct PushDownContext {
    eval_ctx: PushDownBuildContextRef,
    client: Option<PushDownClientRef>,
    warn_handler: Option<PushDownWarnAppenderRef>,
    group_concat_max_len: u64,
}

/// Preserves Go's warning routing: explain uses the normal handler, while
/// ordinary planning uses the extra-warning handler. If either handler is
/// absent, warnings are intentionally discarded.
///
/// 构造下推上下文。EXPLAIN 走普通告警处理器，其它规划路径走额外告警处理器；
/// 任一缺失则丢弃告警（与 Go 一致）。
pub fn NewPushDownContext(
    eval_ctx: PushDownBuildContextRef,
    client: Option<PushDownClientRef>,
    in_explain_stmt: bool,
    warn_handler: Option<PushDownWarnAppenderRef>,
    extra_warn_handler: Option<PushDownWarnAppenderRef>,
    group_concat_max_len: u64,
) -> PushDownContext {
    // 仅当两个处理器都存在时才启用告警路由。
    let warn_handler = match (warn_handler, extra_warn_handler) {
        (Some(normal), Some(extra)) => Some(if in_explain_stmt { normal } else { extra }),
        _ => None,
    };
    PushDownContext {
        eval_ctx,
        client,
        warn_handler,
        group_concat_max_len,
    }
}

impl PushDownContext {
    /// 取底层求值上下文。
    pub fn EvalCtx(&self) -> &dyn exprctx::EvalContext {
        self.eval_ctx.GetEvalCtx()
    }

    /// 基于当前客户端与求值上下文构造 PB 转换器。
    pub fn PbConverter(&self) -> PbConverter<'_> {
        NewPBConverter(self.Client(), self.EvalCtx())
    }

    /// 取 KV 客户端；下推转 PB 时必须已注入，否则 panic。
    pub fn Client(&self) -> &dyn kv::Client {
        self.client
            .as_deref()
            .expect("PushDownContext requires a KV client for PB conversion")
    }

    /// 返回 `GROUP_CONCAT` 结果长度上限。
    pub fn GetGroupConcatMaxLen(&self) -> u64 {
        self.group_concat_max_len
    }

    /// 若已配置告警处理器则追加一条警告。
    pub fn AppendWarning(&self, error: contextutil::errors::SharedError) {
        if let Some(handler) = &self.warn_handler {
            handler.AppendWarning(error);
        }
    }
}
