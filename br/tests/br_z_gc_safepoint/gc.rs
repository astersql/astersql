// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! Test backup with exceeding GC safe point.
//! Port of `br/tests/br_z_gc_safepoint/gc.go`.
//!
//! 集成测试负载：把 PD GC safepoint 推到“当前 TS 减去 offset”，制造超界场景。
//! 可选 UpdateServiceGCSafePoint（服务级）或 UpdateGCSafePoint（集群级）。
//! PD 客户端可注入；二进制路径经 stubs 拨号。非生产路径。
//! 与备份集成：推高 safepoint 后观察备份是否正确失败/拒绝。
//! 错误一律 log_panic，便于 shell 用例捕获非零退出。

use std::time::Duration;

use crate::stubs::{
    self, Context, GoDuration, SecurityOption, caller, log_info, log_panic, oracle,
    parse_go_duration,
};

/// CLI flags matching Go `flag` package defaults.
/// 字段对齐 Go flag：TLS 三件套、pd、gc-offset（默认 10s）、update-service。
#[derive(Clone, Debug)]
pub struct Flags {
    /// CA 路径。
    pub ca: String,
    /// 证书路径。
    pub cert: String,
    /// 私钥路径。
    pub key: String,
    /// PD 地址，必填。
    pub pd: String,
    /// 从当前 TS 回退的时长，用于计算新 safepoint。
    pub gc_offset: GoDuration,
    /// true 时走服务级 safepoint API。
    pub update_service: bool,
}

impl Default for Flags {
    /// 默认 gc_offset=10s，update_service=false，对齐 Go。
    fn default() -> Self {
        Self {
            ca: String::new(),
            cert: String::new(),
            key: String::new(),
            pd: String::new(),
            gc_offset: GoDuration::from_secs(10),
            update_service: false,
        }
    }
}

/// Parse argv like Go `flag` (`--name value` / `--name=value` / `-name`).
/// 未知/非法 flag 失败；布尔 flag 无 `=` 时立即置 true；位置参数终止解析。
pub fn parse_flags(args: &[String]) -> Flags {
    let mut f = Flags::default();
    let mut i = 0usize;
    while i < args.len() {
        let a = &args[i];
        if a == "--" || !a.starts_with('-') || a == "-" {
            break;
        }
        let (name, inline) = if let Some(rest) = a.strip_prefix("--") {
            if let Some((n, v)) = rest.split_once('=') {
                (n.to_string(), Some(v.to_string()))
            } else {
                (rest.to_string(), None)
            }
        } else if let Some(rest) = a.strip_prefix('-') {
            if let Some((n, v)) = rest.split_once('=') {
                (n.to_string(), Some(v.to_string()))
            } else {
                (rest.to_string(), None)
            }
        } else {
            i += 1;
            continue;
        };

        if !matches!(
            name.as_str(),
            "ca" | "cert" | "key" | "pd" | "gc-offset" | "update-service"
        ) {
            log_panic("flag provided but not defined", &[("flag", name)]);
        }

        let is_bool = name == "update-service";
        let val = if let Some(v) = inline {
            v
        } else if is_bool {
            "true".to_string()
        } else {
            i += 1;
            if i >= args.len() {
                log_panic("flag needs an argument", &[("flag", name)]);
            }
            args[i].clone()
        };

        match name.as_str() {
            "ca" => f.ca = val,
            "cert" => f.cert = val,
            "key" => f.key = val,
            "pd" => f.pd = val,
            "gc-offset" => {
                f.gc_offset = parse_go_duration(&val).unwrap_or_else(|err| {
                    log_panic("invalid value for gc-offset", &[("error", err.msg)])
                });
            }
            "update-service" => {
                f.update_service = match val.as_str() {
                    "1" | "t" | "T" | "TRUE" | "true" | "True" => true,
                    "0" | "f" | "F" | "FALSE" | "false" | "False" => false,
                    _ => log_panic("invalid value for update-service", &[("value", val)]),
                };
            }
            _ => unreachable!("known flag checked above"),
        }
        i += 1;
    }
    f
}

/// Compute the new GC safe point from a TSO and offset.
/// Mirrors Go:
///   now := oracle.ComposeTS(p, l)
///   nowMinusOffset := oracle.GetTimeFromTS(now).Add(-gcOffset)
///   newSP := oracle.ComposeTS(oracle.GetPhysical(nowMinusOffset), 0)
/// 返回 (now_ts, new_safe_point)；newSP 逻辑部分置 0。
pub fn compute_new_safe_point(physical: i64, logical: i64, gc_offset: GoDuration) -> (u64, u64) {
    let now = oracle::ComposeTS(physical, logical);
    // Go: GetTimeFromTS(now).Add(-gcOffset), then UnixNano()/1e6 (toward zero).
    // Negate before widening: Go's int64 unary minus wraps for time.Duration's minimum value.
    let negated_offset = gc_offset.as_nanos().wrapping_neg() as i128;
    let composed_physical = oracle::ExtractPhysical(now);
    let physical_nanos = composed_physical as i128 * 1_000_000 + negated_offset;
    let new_physical = (physical_nanos / 1_000_000) as i64;
    let new_sp = oracle::ComposeTS(new_physical, 0);
    (now, new_sp)
}

/// Core logic matching Go `main` after flag parse (injectable PD client).
/// 取 TS → 算新 SP → 按 flag 更新服务/集群 safepoint → cancel 清理。
/// 失败路径 `log_panic`，对齐 Go Panic。
pub fn run_with_client(flags: &Flags, pdclient: &stubs::PdClient) {
    let timeout = Duration::from_secs(10);
    let (ctx, cancel) = Context::WithTimeout(&Context::Background(), timeout);

    run_with_client_context(flags, pdclient, &ctx);

    // Go: defer cancel() — explicitly finish the timeout context on return.
    cancel.cancel();
}

/// Execute GetTS and safepoint update with the caller-owned context.
fn run_with_client_context(flags: &Flags, pdclient: &stubs::PdClient, ctx: &Context) {
    // 先取 TSO；失败直接 panic 对齐 Go。
    let (p, l) = match pdclient.GetTS(ctx) {
        Ok(ts) => ts,
        Err(err) => log_panic("get ts failed", &[("error", err.msg)]),
    };
    // 用 offset 回推物理时间得到新 safepoint。
    let (now, new_sp) = compute_new_safe_point(p, l, flags.gc_offset);

    // 服务级：TTL=300，服务名 "br"，对齐 Go。
    if flags.update_service {
        if let Err(err) = pdclient.UpdateServiceGCSafePoint(ctx, "br", 300, new_sp) {
            log_panic("update service safe point failed", &[("error", err.msg)]);
        }
        // 记录服务级更新成功日志字段 SP/now。
        log_info(
            "update service GC safe point",
            &[("SP", new_sp.to_string()), ("now", now.to_string())],
        );
    } else {
        // 集群级 UpdateGCSafePoint。
        if let Err(err) = pdclient.UpdateGCSafePoint(ctx, new_sp) {
            log_panic("update safe point failed", &[("error", err.msg)]);
        }
        // 记录集群级更新成功日志。
        log_info(
            "update GC safe point",
            &[("SP", new_sp.to_string()), ("now", now.to_string())],
        );
    }
}

/// Create PD client matching Go `pd.NewClientWithContext`.
/// 10s 超时上下文 + SecurityOption；真实拨号由 stubs 接管。
pub fn create_pd_client(flags: &Flags) -> stubs::Result<stubs::PdClient> {
    let timeout = Duration::from_secs(10);
    let (ctx, _cancel) = Context::WithTimeout(&Context::Background(), timeout);
    stubs::NewClientWithContext(
        &ctx,
        caller::TestComponent,
        vec![flags.pd.clone()],
        SecurityOption {
            CAPath: flags.ca.clone(),
            CertPath: flags.cert.clone(),
            KeyPath: flags.key.clone(),
        },
    )
}

/// Binary / library entry matching Go `main` (returns for testability; panics on Go Panic paths).
/// 校验 pd 非空且 gc_offset 非零后创建客户端并运行。
pub fn run_with_flags(flags: &Flags) {
    // pd 必填。
    if flags.pd.is_empty() {
        log_panic("pd address is empty", &[]);
    }
    // 零 offset 无意义且危险，禁止。
    if flags.gc_offset.is_zero() {
        log_panic("zero gc-offset is not allowed", &[]);
    }

    // Go main 的一个 WithTimeout Context 同时覆盖拨号、GetTS 与更新。
    let timeout = Duration::from_secs(10);
    let (ctx, cancel) = Context::WithTimeout(&Context::Background(), timeout);
    let pdclient = match stubs::NewClientWithContext(
        &ctx,
        caller::TestComponent,
        vec![flags.pd.clone()],
        SecurityOption {
            CAPath: flags.ca.clone(),
            CertPath: flags.cert.clone(),
            KeyPath: flags.key.clone(),
        },
    ) {
        Ok(c) => c,
        Err(err) => log_panic("create pd client failed", &[("error", err.msg)]),
    };
    run_with_client_context(flags, &pdclient, &ctx);
    cancel.cancel();
}

/// Binary entrypoint corresponding to Go `main`.
/// 跳过 argv[0]，解析 flag 后进入 run_with_flags。
pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags = parse_flags(&args);
    run_with_flags(&flags);
}
