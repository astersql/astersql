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

//! Local stand-ins for PD/TiKV/storage/meta/checkpoint/stream boundaries
//! (darwin-safe; no kvproto/grpcio/kv/domain).

//!
//! 本文件是 `log_client` 的本地桩/适配边界：用内存结构与最小行为替代
//! PD、TiKV、对象存储、meta、checkpoint、stream、domain 等真实依赖，
//! 以便在 darwin 上无 kvproto/grpcio 也能编译与单测。
//!
//! 重要约束（阅读/使用时务必遵守）：
//! - 这里的实现是**占位能力**，不是生产可用客户端；勿把成功返回当成集群已联通。
//! - protobuf 形状字段对齐 Go/kvproto 命名，但编解码多为手工/简化路径。
//! - Storage/PD/Split/Importer 以 Mem* 为主，覆盖测试需要的读写与错误注入点。
//! - checkpoint / stream / glue 仅模拟日志恢复管线会碰到的状态机片段。
//! - failpoint、metrics、summary 多为空操作或计数器，避免测试依赖真实观测后端。
//! - 错误类型 `Error` 提供 Trace/Annotate 以贴近 Go `errors` 包装习惯。
//! - 若某方法直接 `Ok(())` / 返回空，通常表示「边界已接线，语义未完整移植」。
//!
//! 模块地图（按文件内顺序）：
//! - `berrors`：BR 业务错误码字符串常量；
//! - `Context`：可取消/超时上下文桩；
//! - `log`/`logutil`/`summary`/`metrics`：日志与观测桩；
//! - `metapb`/`errorpb`/`import_sstpb`/`encryptionpb`/`kvrpcpb`/`backuppb`：消息形状；
//! - `storeapi`：对象存储读写遍历；
//! - `tablecodec`/`utils_retry`/`consts`/`grpc_status`/`multierr`：编解码与重试辅助；
//! - `checkpoint`/`stream`/`metautil`：日志恢复元数据与流式替换；
//! - `glue`/`kv`/`domain`：会话、KV、infoschema 最小面；
//! - `pd`/`pdhttp`/`tidbutil`/`importclient`/`rawkv`/`conn`：集群侧客户端桩；
//! - `encryption`/`failpoint`/`split_client`/`kv_entry`：加密、故障注入、拆分与条目。
//!
//! 与 Go 对照时：优先核对字段名与方法签名；行为完整度以调用方测试期望为准。

//! log_client 桩：为日志恢复客户端测试提供本地 PD/KV/元数据替身。
//! 不宣称完整 TiKV/PD 能力，只保证控制流与错误分类可测。
//! 桩返回值需稳定可断言，避免随机性掩盖回归。
//! 与 Go 侧 mock/stub 语义对齐，字段命名保持对照可读。
//! 未实现路径应显式失败，防止测试误把空实现当成功。
//! 符号索引补充 1：公开 API 的约束优先于内部实现细节。
//! 数据流补充 2：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 3：空输入、取消上下文、未知枚举值都应按 Go 方式处理。

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_br_pkg_restore_utils::AppliedFile;

/// 本包统一 Result；错误为本地 `Error` 而非 anyhow。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 可携带可选业务 code 的错误；Trace/Annotate 模拟 Go 包装链。
pub struct Error {
    pub msg: String,
    pub code: Option<&'static str>,
}

/// `Error` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `Error` 方法边界：非法参数应返回可分类错误而非 panic。
impl Error {
    /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `new` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: None,
        }
    }

    /// `with_code`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub fn with_code(code: &'static str, msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: Some(code),
        }
    }

    /// `Trace`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Trace` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Trace(err: Self) -> Self {
        err
    }

    /// `Annotate`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Annotate` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Annotate(err: Self, ctx: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", ctx.into(), err.msg),
            code: err.code,
        }
    }

    /// `Annotatef`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Annotatef` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Annotatef(err: Self, ctx: impl Into<String>) -> Self {
        Self::Annotate(err, ctx)
    }

    /// `Errorf`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Errorf` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Errorf(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }

    /// `Wrap`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Wrap` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Wrap(err: Self, msg: impl Into<String>) -> Self {
        Self::Annotate(err, msg)
    }

    /// `Cause`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Cause` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Cause(&self) -> &Self {
        self
    }
}

/// `Error` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `Error` 方法边界：非法参数应返回可分类错误而非 panic。
impl fmt::Display for Error {
    /// `fmt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `fmt` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

/// `Error` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `Error` 方法边界：非法参数应返回可分类错误而非 panic。
impl std::error::Error for Error {}

/// `Error` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `Error` 方法边界：非法参数应返回可分类错误而非 panic。
impl From<astersql_errors::SharedError> for Error {
    /// `from`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `from` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn from(e: astersql_errors::SharedError) -> Self {
        Self::new(e.to_string())
    }
}

// BR 错误码命名空间：字符串码与 Go berrors 对齐，供 Annotate/匹配使用。
pub mod berrors {
    use super::Error;

    /// `ErrInvalidArgument`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ErrInvalidArgument` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ErrInvalidArgument(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Common:ErrInvalidArgument", msg)
    }

    /// `ErrUnknown`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ErrUnknown` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ErrUnknown(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Common:ErrUnknown", msg)
    }

    /// `ErrPDLeaderNotFound`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ErrPDLeaderNotFound` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ErrPDLeaderNotFound(msg: impl Into<String>) -> Error {
        Error::with_code("BR:PD:ErrPDLeaderNotFound", msg)
    }

    /// `ErrKVRewriteRuleNotFound`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ErrKVRewriteRuleNotFound` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ErrKVRewriteRuleNotFound(msg: impl Into<String>) -> Error {
        Error::with_code("BR:KV:ErrKVRewriteRuleNotFound", msg)
    }

    /// `ErrKVRangeIsEmpty`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ErrKVRangeIsEmpty` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ErrKVRangeIsEmpty(msg: impl Into<String>) -> Error {
        Error::with_code("BR:KV:ErrKVRangeIsEmpty", msg)
    }

    /// `ErrKVEpochNotMatch`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ErrKVEpochNotMatch` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ErrKVEpochNotMatch(msg: impl Into<String>) -> Error {
        Error::with_code("BR:KV:ErrKVEpochNotMatch", msg)
    }
}

#[derive(Clone, Default)]
/// 上下文桩：支持 WithCancel/WithTimeout 的最小可取消语义。
pub struct Context {
    cancelled: Arc<Mutex<Option<Error>>>,
    source: Option<Arc<dyn Fn() -> Option<Error> + Send + Sync>>,
}

/// `Context` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `Context` 方法边界：非法参数应返回可分类错误而非 panic。
impl Context {
    /// `Background`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Background` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Background() -> Self {
        Self::default()
    }

    pub fn WithCancellationSource(
        source: impl Fn() -> Option<Error> + Send + Sync + 'static,
    ) -> Self {
        Self {
            source: Some(Arc::new(source)),
            ..Self::default()
        }
    }

    /// `cancel`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `cancel` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn cancel(&self, err: Error) {
        *self.cancelled.lock().unwrap() = Some(err);
    }

    /// `Err`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Err` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Err(&self) -> Option<Error> {
        self.cancelled
            .lock()
            .unwrap()
            .clone()
            .or_else(|| self.source.as_ref().and_then(|source| source()))
    }

    /// `Done`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Done` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }
}

// 日志桩：Info/Warn 等打印到 stdout，不接真实日志后端。
pub mod log {
    /// `Info`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Info` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Info(_msg: &str) {}
    /// `Warn`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Warn` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Warn(_msg: &str) {}
    /// `Error`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Error` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Error(_msg: &str) {}
    /// `Debug`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Debug` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Debug(_msg: &str) {}
    /// `Panic`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Panic` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Panic(msg: &str) -> ! {
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        panic!("{msg}")
    }
}

// 日志桩：Info/Warn 等打印到 stdout，不接真实日志后端。
pub mod logutil {
    use super::metapb;
    use std::fmt;

    /// `ShortError`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ShortError` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ShortError(err: &dyn fmt::Display) -> String {
        err.to_string()
    }

    /// `Key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Key` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Key(_name: &str, _key: &[u8]) -> String {
        "key".into()
    }

    /// `Region`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Region` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Region(_r: &metapb::Region) -> String {
        "region".into()
    }

    /// `Leader`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Leader` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Leader(_p: &metapb::Peer) -> String {
        "leader".into()
    }

    /// `CL`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `CL` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn CL(_ctx: &super::Context) -> CLHandle {
        CLHandle
    }

    /// `CLHandle`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `CLHandle` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct CLHandle;

    /// `CLHandle` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `CLHandle` 方法边界：非法参数应返回可分类错误而非 panic。
    impl CLHandle {
        /// `Debug`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Debug` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Debug(&self, _msg: &str) {}
        /// `Warn`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Warn` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Warn(&self, _msg: &str) {}
        /// `Info`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Info` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Info(&self, _msg: &str) {}
    }

    /// `ContextWithField`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ContextWithField` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ContextWithField(ctx: &super::Context, _fields: &[String]) -> super::Context {
        ctx.clone()
    }
}

// 汇总信息桩：收集关键计数，供测试断言而非真实报表。
pub mod summary {
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// `START`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    static START: Mutex<Option<u64>> = Mutex::new(None);

    /// `CollectInt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `CollectInt` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn CollectInt(_name: &str, _v: i32) {}
    /// `AdjustStartTimeToEarlierTime`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `AdjustStartTimeToEarlierTime` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn AdjustStartTimeToEarlierTime(t: SystemTime) {
        let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        *START.lock().unwrap() = Some(secs);
    }
}

// 指标桩：多为空实现或原子计数，避免依赖 Prometheus。
pub mod metrics {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// `Counter`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Counter` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Counter {
        v: AtomicU64,
    }

    /// `Counter` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Counter` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Counter {
        /// `fn`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
        /// 调整阈值前确认是否影响重试次数或批大小语义。
        pub const fn new() -> Self {
            Self {
                v: AtomicU64::new(0),
            }
        }
        /// `Inc`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Inc` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Inc(&self) {
            self.v.fetch_add(1, Ordering::Relaxed);
        }
        /// `get`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `get` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn get(&self) -> u64 {
            self.v.load(Ordering::Relaxed)
        }
    }

    /// `Histogram`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Histogram` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Histogram;
    /// `Histogram` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Histogram` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Histogram {
        /// `Observe`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Observe` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Observe(&self, _v: f64) {}
    }

    /// `Gauge`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Gauge` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Gauge {
        v: AtomicU64,
    }
    /// `Gauge` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Gauge` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Gauge {
        /// `fn`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
        /// 调整阈值前确认是否影响重试次数或批大小语义。
        pub const fn new() -> Self {
            Self {
                v: AtomicU64::new(0),
            }
        }
        /// `Set`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Set` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Set(&self, v: f64) {
            self.v.store(v as u64, Ordering::Relaxed);
        }
    }

    /// `KV_APPLY_REGION_FILES`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub static KV_APPLY_REGION_FILES: Histogram = Histogram;
    /// `KV_APPLY_BATCH_REGIONS`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub static KV_APPLY_BATCH_REGIONS: Histogram = Histogram;
    /// `KV_SPLIT_HELPER_MEM_USAGE`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub static KV_SPLIT_HELPER_MEM_USAGE: Gauge = Gauge::new();

    /// `KVApplyRunOverRegionsEvents`：子模块，聚合相关桩类型与辅助函数。
    pub mod KVApplyRunOverRegionsEvents {
        use super::Counter;
        /// `WithLabelValues`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
        /// 调整阈值前确认是否影响重试次数或批大小语义。
        pub fn WithLabelValues(_l: &str) -> &'static Counter {
            /// `C`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
            /// 调整阈值前确认是否影响重试次数或批大小语义。
            static C: Counter = Counter::new();
            &C
        }
    }
}

// PD metapb 消息形状：Region/Store/Peer 等，字段子集够用即可。
pub mod metapb {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `Peer`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Peer` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Peer {
        pub Id: u64,
        pub StoreId: u64,
    }
    /// `Peer` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Peer` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Peer {
        /// `GetStoreId`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetStoreId` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetStoreId(&self) -> u64 {
            self.StoreId
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    /// `StoreState`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `StoreState` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub enum StoreState {
        #[default]
        Up = 0,
        Offline = 1,
        Tombstone = 2,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `Store`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Store` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Store {
        pub Id: u64,
        pub State: StoreState,
        pub Labels: Vec<StoreLabel>,
    }
    /// `Store` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Store` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Store {
        /// `GetId`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetId` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetId(&self) -> u64 {
            self.Id
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `StoreLabel`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `StoreLabel` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct StoreLabel {
        pub Key: String,
        pub Value: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `RegionEpoch`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `RegionEpoch` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct RegionEpoch {
        pub ConfVer: u64,
        pub Version: u64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `Region`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Region` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Region {
        pub Id: u64,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub RegionEpoch: Option<RegionEpoch>,
        pub Peers: Vec<Peer>,
    }
    /// `Region` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Region` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Region {
        /// `GetId`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetId` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetId(&self) -> u64 {
            self.Id
        }
        /// `GetStartKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetStartKey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetStartKey(&self) -> &[u8] {
            &self.StartKey
        }
        /// `GetEndKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetEndKey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetEndKey(&self) -> &[u8] {
            &self.EndKey
        }
        /// `GetRegionEpoch`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetRegionEpoch` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetRegionEpoch(&self) -> Option<&RegionEpoch> {
            self.RegionEpoch.as_ref()
        }
    }
}

// TiKV errorpb 形状：EpochNotMatch 等，用于重试策略分类。
pub mod errorpb {
    use super::metapb;

    #[derive(Clone, Debug, Default)]
    /// `ServerIsBusy`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `ServerIsBusy` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct ServerIsBusy {
        pub Message: String,
    }

    #[derive(Clone, Debug, Default)]
    /// `NotLeader`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `NotLeader` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct NotLeader {
        pub Leader: Option<metapb::Peer>,
    }

    #[derive(Clone, Debug, Default)]
    /// `RegionNotInitialized`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `RegionNotInitialized` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct RegionNotInitialized;

    #[derive(Clone, Debug, Default)]
    /// 可携带可选业务 code 的错误；Trace/Annotate 模拟 Go 包装链。
    pub struct Error {
        pub Message: String,
        pub NotLeader: Option<NotLeader>,
        pub ServerIsBusy: Option<ServerIsBusy>,
        pub RegionNotInitialized: Option<RegionNotInitialized>,
    }

    /// `Error` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Error` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Error {
        /// `GetMessage`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetMessage` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetMessage(&self) -> &str {
            &self.Message
        }
        /// `GetNotLeader`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetNotLeader` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetNotLeader(&self) -> Option<&NotLeader> {
            self.NotLeader.as_ref()
        }
        /// `GetServerIsBusy`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetServerIsBusy` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetServerIsBusy(&self) -> Option<&ServerIsBusy> {
            self.ServerIsBusy.as_ref()
        }
        /// `GetRegionNotInitialized`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetRegionNotInitialized` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetRegionNotInitialized(&self) -> Option<&RegionNotInitialized> {
            self.RegionNotInitialized.as_ref()
        }
    }
}

// ImportSST 相关枚举/消息桩，覆盖 SwitchMode 等测试路径。
pub mod import_sstpb {
    use super::backuppb;
    use super::encryptionpb;
    use super::errorpb;
    use super::kvrpcpb;

    #[derive(Clone, Debug, Default)]
    /// 可携带可选业务 code 的错误；Trace/Annotate 模拟 Go 包装链。
    pub struct Error {
        pub Message: String,
        pub StoreError: Option<errorpb::Error>,
    }
    /// `Error` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Error` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Error {
        /// `GetMessage`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetMessage` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetMessage(&self) -> &str {
            &self.Message
        }
        /// `GetStoreError`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetStoreError` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetStoreError(&self) -> Option<&errorpb::Error> {
            self.StoreError.as_ref()
        }
    }

    #[derive(Clone, Debug, Default)]
    /// `ClearRequest`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `ClearRequest` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct ClearRequest {
        pub Prefix: String,
    }

    #[derive(Clone, Debug, Default)]
    /// `ClearResponse`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `ClearResponse` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct ClearResponse {
        pub Error: Option<Error>,
    }

    #[derive(Clone, Debug, Default)]
    /// `KVMeta`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `KVMeta` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct KVMeta {
        pub Name: String,
        pub Cf: String,
        pub RangeOffset: u64,
        pub Length: u64,
        pub RangeLength: u64,
        pub IsDelete: bool,
        pub StartTs: u64,
        pub RestoreTs: u64,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub Sha256: Vec<u8>,
        pub CompressionType: i32,
        pub FileEncryptionInfo: Option<encryptionpb::FileEncryptionInfo>,
    }

    #[derive(Clone, Debug, Default)]
    /// `RewriteRule`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `RewriteRule` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct RewriteRule {
        pub OldKeyPrefix: Vec<u8>,
        pub NewKeyPrefix: Vec<u8>,
    }

    #[derive(Clone, Debug, Default)]
    /// `ApplyRequest`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `ApplyRequest` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct ApplyRequest {
        pub Meta: Option<KVMeta>,
        pub Metas: Vec<KVMeta>,
        pub StorageBackend: Option<backuppb::StorageBackend>,
        pub RewriteRule: RewriteRule,
        pub RewriteRules: Vec<RewriteRule>,
        pub Context: Option<kvrpcpb::Context>,
        pub StorageCacheId: String,
        pub CipherInfo: Option<backuppb::CipherInfo>,
        pub MasterKeys: Vec<encryptionpb::MasterKey>,
    }

    #[derive(Clone, Debug, Default)]
    /// `ApplyResponse`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `ApplyResponse` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct ApplyResponse {
        pub Error: Option<Error>,
    }
    /// `ApplyResponse` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `ApplyResponse` 方法边界：非法参数应返回可分类错误而非 panic。
    impl ApplyResponse {
        /// `GetError`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetError` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetError(&self) -> Option<&Error> {
            self.Error.as_ref()
        }
    }
}

// 加密相关 protobuf 形状占位。
pub mod encryptionpb {
    #[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    /// `FileEncryptionInfo`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `FileEncryptionInfo` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct FileEncryptionInfo;

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `MasterKey`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MasterKey` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MasterKey;
}

// KV RPC 消息子集占位。
pub mod kvrpcpb {
    use super::metapb;

    #[derive(Clone, Debug, Default)]
    /// 上下文桩：支持 WithCancel/WithTimeout 的最小可取消语义。
    pub struct Context {
        pub RegionId: u64,
        pub RegionEpoch: Option<metapb::RegionEpoch>,
        pub Peer: Option<metapb::Peer>,
    }
}

// 备份/日志恢复核心 protobuf：File/Migration/MetaEdit 等。
pub mod backuppb {
    use super::encryptionpb;
    use astersql_br_pkg_restore_utils::AppliedFile;
    use std::collections::HashMap;
    use std::fmt;

    /// `BackupSchemaVersion`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub const BackupSchemaVersion: i32 = 1;

    #[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    /// `File`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `File` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct File {
        pub Name: String,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub TotalBytes: u64,
        pub TotalKvs: u64,
        pub Size_: u64,
        pub Cf: String,
        pub Sha256: Vec<u8>,
        pub Path: String,
        pub RangeOffset: u64,
        pub Length: u64,
        pub RangeLength: u64,
        pub NumberOfEntries: i64,
        pub TableId: i64,
        pub IsMeta: bool,
        pub Type: FileType,
        pub CompressionType: i32,
        pub FileEncryptionInfo: Option<encryptionpb::FileEncryptionInfo>,
        pub MinTs: u64,
        pub MaxTs: u64,
        pub ResolvedTs: u64,
    }

    /// `File` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `File` 方法边界：非法参数应返回可分类错误而非 panic。
    impl File {
        /// `GetStartKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetStartKey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetStartKey(&self) -> Vec<u8> {
            self.StartKey.clone()
        }
        /// `GetEndKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetEndKey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetEndKey(&self) -> Vec<u8> {
            self.EndKey.clone()
        }
        /// `GetSha256`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetSha256` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetSha256(&self) -> Vec<u8> {
            self.Sha256.clone()
        }
        /// `GetName`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetName` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetName(&self) -> &str {
            &self.Name
        }
    }

    /// `File` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `File` 方法边界：非法参数应返回可分类错误而非 panic。
    impl AppliedFile for File {
        /// `GetStartKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetStartKey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetStartKey(&self) -> Vec<u8> {
            File::GetStartKey(self)
        }
        /// `GetEndKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetEndKey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetEndKey(&self) -> Vec<u8> {
            File::GetEndKey(self)
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    /// `FileType`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `FileType` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub enum FileType {
        #[default]
        Put = 0,
        Delete = 1,
    }

    /// `DataFileInfo`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `DataFileInfo` 契约：返回值与副作用应可被上层测试稳定观察。
    pub type DataFileInfo = File;

    #[derive(Clone, Debug, Default)]
    /// `DataFileGroup`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `DataFileGroup` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct DataFileGroup {
        pub Path: String,
        pub DataFilesInfo: Vec<DataFileInfo>,
        pub MinTs: u64,
        pub MaxTs: u64,
        pub MinBeginTsInDefaultCf: u64,
        pub Length: u64,
    }

    #[derive(Clone, Debug, Default)]
    /// `Metadata`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Metadata` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Metadata {
        pub FileGroups: Vec<DataFileGroup>,
        pub MetaVersion: i32,
        pub StoreId: u64,
        pub MinTs: u64,
        pub MaxTs: u64,
    }

    #[derive(Clone, Debug, Default)]
    /// `LogFileSubcompactionMeta`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `LogFileSubcompactionMeta` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct LogFileSubcompactionMeta {
        pub TableId: i64,
    }

    /// `LogFileSubcompactionMeta` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `LogFileSubcompactionMeta` 方法边界：非法参数应返回可分类错误而非 panic。
    impl fmt::Display for LogFileSubcompactionMeta {
        /// `fmt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `fmt` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "table:{}", self.TableId)
        }
    }

    #[derive(Clone, Debug, Default)]
    /// `LogFileSubcompaction`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `LogFileSubcompaction` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct LogFileSubcompaction {
        pub Meta: LogFileSubcompactionMeta,
        pub SstOutputs: Vec<File>,
    }

    #[derive(Clone, Debug, Default)]
    /// `RewrittenTableID`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `RewrittenTableID` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct RewrittenTableID {
        pub Upstream: i64,
        pub Downstream: i64,
    }

    #[derive(Clone, Debug, Default)]
    /// `StorageBackend`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `StorageBackend` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct StorageBackend {
        pub Local: Option<Local>,
    }

    #[derive(Clone, Debug, Default)]
    /// `Local`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Local` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Local {
        pub Path: String,
    }

    #[derive(Clone, Debug, Default)]
    /// `CipherInfo`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `CipherInfo` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct CipherInfo {
        pub CipherType: i32,
        pub CipherKey: Vec<u8>,
    }

    #[derive(Clone, Debug, Default)]
    /// `PitrDBMap`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `PitrDBMap` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct PitrDBMap {
        pub Name: String,
        pub IdMap: HashMap<i64, i64>,
        pub Tables: Vec<PitrTableMap>,
    }

    #[derive(Clone, Debug, Default)]
    /// `PitrTableMap`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `PitrTableMap` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct PitrTableMap {
        pub Name: String,
        pub IdMap: HashMap<i64, i64>,
    }

    #[derive(Clone, Debug, Default)]
    /// `BackupMeta`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `BackupMeta` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct BackupMeta {
        pub ClusterId: u64,
        pub DbMaps: Vec<PitrDBMap>,
        pub BackupSchemaVersion: i32,
        pub StartVersion: u64,
        pub EndVersion: u64,
    }

    /// `BackupMeta` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `BackupMeta` 方法边界：非法参数应返回可分类错误而非 panic。
    impl BackupMeta {
        /// `Unmarshal`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Unmarshal` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Unmarshal(&mut self, _data: &[u8]) -> Result<(), super::Error> {
            // Local stub: accept empty / opaque payloads without real protobuf.
            let _ = self;
            Ok(())
        }
        /// `Marshal`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Marshal` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Marshal(&self) -> Result<Vec<u8>, super::Error> {
            // Encode a stable JSON-like payload for segment round-trips in tests.
            let mut out = Vec::new();
            out.extend_from_slice(b"BM");
            out.extend_from_slice(&self.ClusterId.to_le_bytes());
            out.extend_from_slice(&(self.DbMaps.len() as u32).to_le_bytes());
            for db in &self.DbMaps {
                let name = db.Name.as_bytes();
                out.extend_from_slice(&(name.len() as u32).to_le_bytes());
                out.extend_from_slice(name);
            }
            Ok(out)
        }
        /// `GetDbMaps`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetDbMaps` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetDbMaps(&self) -> Vec<PitrDBMap> {
            self.DbMaps.clone()
        }
    }

    #[derive(Clone, Debug, Default)]
    /// `MetaEdit`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MetaEdit` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MetaEdit {
        pub Path: String,
        pub DestructSelf: bool,
        pub DeletePhysicalFiles: Vec<String>,
        pub DeleteLogicalFiles: Vec<DeleteFilesInPhysical>,
    }

    #[derive(Clone, Debug, Default)]
    /// `DeleteFilesInPhysical`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `DeleteFilesInPhysical` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct DeleteFilesInPhysical {
        pub Path: String,
        pub Spans: Vec<Span>,
    }

    #[derive(Clone, Debug, Default)]
    /// `Span`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Span` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Span {
        pub Offset: u64,
        pub Length: u64,
    }

    #[derive(Clone, Debug, Default)]
    /// `LogFileCompaction`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `LogFileCompaction` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct LogFileCompaction {
        pub InputMinTs: u64,
        pub InputMaxTs: u64,
        pub Artifacts: String,
        pub Comments: String,
        pub CompactionFromTs: u64,
        pub CompactionUntilTs: u64,
    }

    /// `LogFileCompaction` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `LogFileCompaction` 方法边界：非法参数应返回可分类错误而非 panic。
    impl LogFileCompaction {
        /// `GetComments`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetComments` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetComments(&self) -> &str {
            &self.Comments
        }
        /// `GetCompactionFromTs`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetCompactionFromTs` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetCompactionFromTs(&self) -> u64 {
            self.CompactionFromTs
        }
        /// `GetCompactionUntilTs`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetCompactionUntilTs` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetCompactionUntilTs(&self) -> u64 {
            self.CompactionUntilTs
        }
    }

    #[derive(Clone, Debug, Default)]
    /// `Migration`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Migration` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Migration {
        pub EditMeta: Vec<MetaEdit>,
        pub Compactions: Vec<LogFileCompaction>,
        pub IngestedSstPaths: Vec<String>,
    }

    #[derive(Clone, Debug, Default)]
    /// `IngestedSSTs`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `IngestedSSTs` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct IngestedSSTs {
        pub Files: Vec<File>,
        pub Rewritten: Option<RewrittenTableID>,
        pub Finished: bool,
        pub BackupTs: u64,
    }
}

// 对象存储抽象 + MemStorage：测试用内存文件树。
pub mod storeapi {
    use super::{Context, Error, Result};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// `Storage`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `Storage` 契约：返回值与副作用应可被上层测试稳定观察。
    pub trait Storage: Send + Sync {
        /// `FileExists`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `FileExists` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool>;
        /// `ReadFile`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ReadFile` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>>;
        /// `WriteFile`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `WriteFile` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn WriteFile(&self, _ctx: &Context, name: &str, data: &[u8]) -> Result<()>;
        /// Return all object names below `prefix`, in deterministic lexical order.
        fn WalkDir(&self, _ctx: &Context, prefix: &str) -> Result<Vec<String>>;
    }

    #[derive(Clone, Default)]
    /// `MemStorage`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MemStorage` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MemStorage {
        files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    }

    /// `MemStorage` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `MemStorage` 方法边界：非法参数应返回可分类错误而非 panic。
    impl MemStorage {
        /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `new` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn new() -> Self {
            Self::default()
        }
    }

    /// `MemStorage` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `MemStorage` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Storage for MemStorage {
        /// `FileExists`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `FileExists` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool> {
            Ok(self.files.lock().unwrap().contains_key(name))
        }
        /// `ReadFile`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ReadFile` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>> {
            self.files
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .ok_or_else(|| Error::new(format!("file not found: {name}")))
        }
        /// `WriteFile`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `WriteFile` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn WriteFile(&self, _ctx: &Context, name: &str, data: &[u8]) -> Result<()> {
            self.files
                .lock()
                .unwrap()
                .insert(name.to_string(), data.to_vec());
            Ok(())
        }
        fn WalkDir(&self, _ctx: &Context, prefix: &str) -> Result<Vec<String>> {
            let prefix = prefix.trim_end_matches('/');
            let mut names: Vec<_> = self
                .files
                .lock()
                .unwrap()
                .keys()
                .filter(|name| *name == prefix || name.starts_with(&format!("{prefix}/")))
                .cloned()
                .collect();
            names.sort();
            Ok(names)
        }
    }

    #[derive(Clone, Debug, Default)]
    /// `Options`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Options` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Options;
}

// 表前缀编解码薄封装，委托真实/简化 tablecodec。
pub mod tablecodec {
    pub use astersql_br_pkg_restore_utils::stubs::tablecodec::*;
}

// 重试状态机：InitialRetryState 等，对齐 Go utils/retry。
pub mod utils_retry {
    use std::time::Duration;

    #[derive(Clone, Debug)]
    /// `RetryState`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `RetryState` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct RetryState {
        max_retry: i32,
        attempt: i32,
        current: Duration,
        max_backoff: Duration,
    }

    /// `RetryState` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `RetryState` 方法边界：非法参数应返回可分类错误而非 panic。
    impl RetryState {
        /// `InitialRetryState`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `InitialRetryState` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn InitialRetryState(max_retry: i32, initial: Duration, max_backoff: Duration) -> Self {
            Self {
                max_retry,
                attempt: 0,
                current: initial,
                max_backoff,
            }
        }

        /// `ShouldRetry`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ShouldRetry` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn ShouldRetry(&self) -> bool {
            self.attempt < self.max_retry
        }

        /// `ExponentialBackoff`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ExponentialBackoff` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn ExponentialBackoff(&mut self) -> Duration {
            self.attempt += 1;
            let d = self.current;
            self.current = std::cmp::min(self.current.saturating_mul(2), self.max_backoff);
            d
        }
    }

    /// `InitialRetryState`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `InitialRetryState` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn InitialRetryState(
        max_retry: i32,
        initial: Duration,
        max_backoff: Duration,
    ) -> RetryState {
        RetryState::InitialRetryState(max_retry, initial, max_backoff)
    }

    /// `PrefixNextKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `PrefixNextKey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn PrefixNextKey(key: &[u8]) -> Vec<u8> {
        let mut buf = key.to_vec();
        let mut i = key.len() as isize - 1;
        while i >= 0 {
            let idx = i as usize;
            buf[idx] = buf[idx].wrapping_add(1);
            if buf[idx] != 0 {
                return buf;
            }
            i -= 1;
        }
        buf.push(0);
        buf
    }

    /// `WithRetryV2`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `WithRetryV2` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn WithRetryV2<T, F>(
        _ctx: &super::Context,
        _strategy: BackoffRetryAllErrorStrategy,
        mut f: F,
    ) -> super::Result<T>
    where
        F: FnMut(&super::Context) -> super::Result<T>,
    {
        // Simplified: try a few times immediately (no sleep) for unit tests.
        let mut last = None;
        for _ in 0..4 {
            match f(&super::Context::Background()) {
                Ok(v) => return Ok(v),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| super::Error::new("retry exhausted")))
    }

    #[derive(Clone, Debug)]
    /// `BackoffRetryAllErrorStrategy`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `BackoffRetryAllErrorStrategy` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct BackoffRetryAllErrorStrategy {
        pub max_retry: i32,
    }

    /// `NewBackoffRetryAllErrorStrategy`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `NewBackoffRetryAllErrorStrategy` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn NewBackoffRetryAllErrorStrategy(
        max_retry: i32,
        _initial: Duration,
        _max: Duration,
    ) -> BackoffRetryAllErrorStrategy {
        BackoffRetryAllErrorStrategy { max_retry }
    }
}

// 跨模块常量桩。
pub mod consts {
    /// `DefaultCF`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub const DefaultCF: &str = "default";
    /// `WriteCF`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub const WriteCF: &str = "write";
}

// gRPC status 码/错误分类占位。
pub mod grpc_status {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    /// `Code`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Code` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub enum Code {
        Unavailable,
        Aborted,
        ResourceExhausted,
        DeadlineExceeded,
        Unknown,
    }

    #[derive(Clone, Debug)]
    /// `Status`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Status` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Status {
        code: Code,
    }

    /// `Status` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Status` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Status {
        /// `Code`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Code` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Code(&self) -> Code {
            self.code
        }
    }

    /// `FromError`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `FromError` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn FromError(err: &super::Error) -> Option<Status> {
        let msg = err.msg.to_lowercase();
        if msg.contains("unavailable") {
            Some(Status {
                code: Code::Unavailable,
            })
        } else if msg.contains("aborted") {
            Some(Status {
                code: Code::Aborted,
            })
        } else if msg.contains("resource_exhausted") || msg.contains("resource exhausted") {
            Some(Status {
                code: Code::ResourceExhausted,
            })
        } else if msg.contains("deadline") {
            Some(Status {
                code: Code::DeadlineExceeded,
            })
        } else {
            None
        }
    }
}

// 多错误聚合占位。
pub mod multierr {
    use super::Error;

    /// `Append`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Append` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Append(base: Option<Error>, err: Error) -> Option<Error> {
        match base {
            None => Some(err),
            Some(e) => Some(Error::new(format!("{}; {}", e.msg, err.msg))),
        }
    }

    /// `Errors`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Errors` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Errors(err: &Error) -> Vec<Error> {
        err.msg.split("; ").map(|s| Error::new(s)).collect()
    }
}

// 日志恢复 checkpoint 内存管理器与进度枚举。
pub mod checkpoint {
    use super::storeapi::Storage;
    use super::{Context, Result};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    /// `LogRestoreKeyType`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `LogRestoreKeyType` 契约：返回值与副作用应可被上层测试稳定观察。
    pub type LogRestoreKeyType = String;

    #[derive(Clone, Debug, Default)]
    /// `LogRestoreValueMarshaled`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `LogRestoreValueMarshaled` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct LogRestoreValueMarshaled {
        pub Goff: i32,
        pub Foffs: HashMap<i64, Vec<i32>>,
    }

    #[derive(Clone, Debug, Default)]
    /// `LogRestoreValueType`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `LogRestoreValueType` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct LogRestoreValueType;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    /// `RestoreProgress`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `RestoreProgress` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub enum RestoreProgress {
        InLogRestoreAndIdMapPersisted,
        Other,
    }

    #[derive(Clone, Debug)]
    /// `CheckpointProgress`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `CheckpointProgress` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct CheckpointProgress {
        pub Progress: RestoreProgress,
    }

    /// `InLogRestoreAndIdMapPersisted`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub const InLogRestoreAndIdMapPersisted: RestoreProgress =
        RestoreProgress::InLogRestoreAndIdMapPersisted;

    /// `LogMetaManager`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `LogMetaManager` 契约：返回值与副作用应可被上层测试稳定观察。
    pub trait LogMetaManager: Send + Sync {
        /// `LoadCheckpointData`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `LoadCheckpointData` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn LoadCheckpointData(
            &self,
            _ctx: &Context,
            fn_: &mut dyn FnMut(LogRestoreKeyType, LogRestoreValueMarshaled) -> Result<()>,
        ) -> Result<SystemTime>;

        /// `SaveCheckpointProgress`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `SaveCheckpointProgress` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn SaveCheckpointProgress(&self, _ctx: &Context, _meta: &CheckpointProgress) -> Result<()>;

        /// `TryGetStorage`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `TryGetStorage` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn TryGetStorage(&self) -> Option<Arc<dyn Storage>>;
    }

    /// `LogMetaManagerT`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `LogMetaManagerT` 契约：返回值与副作用应可被上层测试稳定观察。
    pub type LogMetaManagerT = dyn LogMetaManager;

    #[derive(Default)]
    /// `MemLogMetaManager`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MemLogMetaManager` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MemLogMetaManager {
        pub storage: Option<Arc<dyn Storage>>,
        pub data: Mutex<Vec<(LogRestoreKeyType, LogRestoreValueMarshaled)>>,
        pub progress: Mutex<Option<RestoreProgress>>,
    }

    /// `MemLogMetaManager` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `MemLogMetaManager` 方法边界：非法参数应返回可分类错误而非 panic。
    impl LogMetaManager for MemLogMetaManager {
        /// `LoadCheckpointData`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `LoadCheckpointData` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn LoadCheckpointData(
            &self,
            _ctx: &Context,
            fn_: &mut dyn FnMut(LogRestoreKeyType, LogRestoreValueMarshaled) -> Result<()>,
        ) -> Result<SystemTime> {
            for (k, v) in self.data.lock().unwrap().iter() {
                fn_(k.clone(), v.clone())?;
            }
            Ok(SystemTime::now())
        }

        /// `SaveCheckpointProgress`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `SaveCheckpointProgress` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn SaveCheckpointProgress(&self, _ctx: &Context, meta: &CheckpointProgress) -> Result<()> {
            *self.progress.lock().unwrap() = Some(meta.Progress);
            Ok(())
        }

        /// `TryGetStorage`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `TryGetStorage` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn TryGetStorage(&self) -> Option<Arc<dyn Storage>> {
            self.storage.clone()
        }
    }

    /// `CheckpointRunner`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `CheckpointRunner` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct CheckpointRunner<K, V> {
        _p: std::marker::PhantomData<(K, V)>,
    }

    /// `CheckpointRunner` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `CheckpointRunner` 方法边界：非法参数应返回可分类错误而非 panic。
    impl<K, V> CheckpointRunner<K, V> {
        /// `WaitForFinish`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `WaitForFinish` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn WaitForFinish(&mut self, _ctx: &Context, _flush: bool) {}
    }
}

// stream 元数据：ingested SST 加载、SchemasReplace、表映射。
pub mod stream {
    use super::backuppb::{DataFileInfo, PitrDBMap};
    use super::{Context, Result};
    use std::collections::HashMap;

    #[derive(Clone, Debug, Default)]
    /// `PreDelRangeQuery`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `PreDelRangeQuery` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct PreDelRangeQuery {
        pub Sql: String,
    }

    #[derive(Clone, Debug, Default)]
    /// `SchemasReplace`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `SchemasReplace` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct SchemasReplace {
        pub DbMap: HashMap<i64, DBReplace>,
    }

    #[derive(Clone, Debug, Default)]
    /// `DBReplace`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `DBReplace` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct DBReplace {
        pub Name: String,
        pub TableMap: HashMap<i64, TableReplace>,
    }

    #[derive(Clone, Debug, Default)]
    /// `TableReplace`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `TableReplace` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct TableReplace {
        pub Name: String,
        pub NewTableID: i64,
    }

    #[derive(Clone, Debug, Default)]
    /// `TableMappingManager`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `TableMappingManager` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct TableMappingManager {
        db_maps: Vec<PitrDBMap>,
    }

    /// `NewTableMappingManager`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `NewTableMappingManager` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn NewTableMappingManager() -> TableMappingManager {
        TableMappingManager {
            db_maps: Vec::new(),
        }
    }

    /// `TableMappingManager` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `TableMappingManager` 方法边界：非法参数应返回可分类错误而非 panic。
    impl TableMappingManager {
        /// `ToProto`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ToProto` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn ToProto(&self) -> Vec<PitrDBMap> {
            self.db_maps.clone()
        }
        /// `FromProto`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `FromProto` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn FromProto(&mut self, maps: Vec<PitrDBMap>) {
            self.db_maps = maps;
        }
        /// `CleanTempKV`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `CleanTempKV` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn CleanTempKV(&mut self) {}
        /// `ParseMetaKvAndUpdateIdMapping`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ParseMetaKvAndUpdateIdMapping` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn ParseMetaKvAndUpdateIdMapping(
            &mut self,
            _entry: &super::kv_entry::Entry,
            _cf: &str,
            _ts: u64,
            _collector: &mut LogBackupTableHistoryManager,
        ) -> Result<()> {
            Ok(())
        }
        /// `MergeBaseDBReplace`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `MergeBaseDBReplace` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn MergeBaseDBReplace(&mut self, _db_maps: &[PitrDBMap]) {}
    }

    #[derive(Clone, Debug, Default)]
    /// `LogBackupTableHistoryManager`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `LogBackupTableHistoryManager` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct LogBackupTableHistoryManager {
        pub Renames: Vec<(String, String)>,
    }

    /// `NewTableHistoryManager`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `NewTableHistoryManager` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn NewTableHistoryManager() -> LogBackupTableHistoryManager {
        LogBackupTableHistoryManager::default()
    }

    #[derive(Clone, Debug, Default)]
    /// `KvEntry`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `KvEntry` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct KvEntry {
        pub Key: Vec<u8>,
        pub Value: Vec<u8>,
    }

    #[derive(Clone, Debug, Default)]
    /// `MetadataHelper`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MetadataHelper` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MetadataHelper;

    /// `MetadataHelper` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `MetadataHelper` 方法边界：非法参数应返回可分类错误而非 panic。
    impl MetadataHelper {
        /// `InitCacheEntry`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `InitCacheEntry` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn InitCacheEntry(&self, _path: &str, _ref: i32) {}
        /// `ReadFile`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ReadFile` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn ReadFile(
            &self,
            _ctx: &Context,
            _path: &str,
            _offset: u64,
            _length: u64,
            _raw_length: u64,
            _compression: i32,
            _storage: &dyn super::storeapi::Storage,
            _enc: Option<&super::encryptionpb::FileEncryptionInfo>,
        ) -> Result<Vec<u8>> {
            Ok(Vec::new())
        }
        /// `ParseToMetadata`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ParseToMetadata` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn ParseToMetadata(&self, _raw: &[u8]) -> Result<super::backuppb::Metadata> {
            Ok(super::backuppb::Metadata::default())
        }
        /// `Close`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Close` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Close(&self) {}
    }

    /// `NewMetadataHelper`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `NewMetadataHelper` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn NewMetadataHelper() -> MetadataHelper {
        MetadataHelper
    }

    #[derive(Clone, Debug, Default)]
    /// `PathedIngestedSSTs`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `PathedIngestedSSTs` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct PathedIngestedSSTs {
        pub IngestedSSTs: super::backuppb::IngestedSSTs,
        pub Path: String,
    }

    /// `IngestedSSTsGroup`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `IngestedSSTsGroup` 契约：返回值与副作用应可被上层测试稳定观察。
    pub type IngestedSSTsGroup = Vec<PathedIngestedSSTs>;

    /// `IngestedSSTsGroupExt`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `IngestedSSTsGroupExt` 契约：返回值与副作用应可被上层测试稳定观察。
    pub trait IngestedSSTsGroupExt {
        /// `GroupTS`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GroupTS` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GroupTS(&self) -> u64;
        /// `GroupFinished`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GroupFinished` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GroupFinished(&self) -> bool;
    }

    /// `IngestedSSTsGroup` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `IngestedSSTsGroup` 方法边界：非法参数应返回可分类错误而非 panic。
    impl IngestedSSTsGroupExt for IngestedSSTsGroup {
        /// `GroupTS`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GroupTS` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GroupTS(&self) -> u64 {
            self.first().map(|p| p.IngestedSSTs.BackupTs).unwrap_or(0)
        }
        /// `GroupFinished`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GroupFinished` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GroupFinished(&self) -> bool {
            self.iter().all(|p| p.IngestedSSTs.Finished)
        }
    }

    /// `LoadIngestedSSTs`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `LoadIngestedSSTs` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn LoadIngestedSSTs(
        _ctx: &Context,
        _s: &dyn super::storeapi::Storage,
        paths: &[String],
    ) -> Box<dyn astersql_br_pkg_utils_iter::TryNextor<IngestedSSTsGroup>> {
        let groups: Vec<IngestedSSTsGroup> = paths
            .iter()
            .map(|p| {
                vec![PathedIngestedSSTs {
                    Path: p.clone(),
                    IngestedSSTs: super::backuppb::IngestedSSTs {
                        Finished: true,
                        BackupTs: 1,
                        ..Default::default()
                    },
                }]
            })
            .collect();
        astersql_br_pkg_utils_iter::FromSlice(groups)
    }
}

// meta 工具桩：解析/遍历辅助。
pub mod metautil {
    use super::backuppb::BackupMeta;
    use super::storeapi::Storage;
    use super::{Context, Error, Result};

    /// `MetaFileSize`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub const MetaFileSize: usize = 128 * 1024;

    /// `MetaWriter`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MetaWriter` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MetaWriter {
        storage: std::sync::Arc<dyn Storage>,
        name: String,
        meta: BackupMeta,
    }

    /// `NewMetaWriter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `NewMetaWriter` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn NewMetaWriter(
        storage: std::sync::Arc<dyn Storage>,
        _size: usize,
        _append: bool,
        name: String,
        _cipher: Option<()>,
    ) -> MetaWriter {
        MetaWriter {
            storage,
            name,
            meta: BackupMeta::default(),
        }
    }

    /// `MetaWriter` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `MetaWriter` 方法边界：非法参数应返回可分类错误而非 panic。
    impl MetaWriter {
        /// `Update`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Update` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Update<F: FnOnce(&mut BackupMeta)>(&mut self, f: F) {
            f(&mut self.meta);
        }
        /// `FlushBackupMeta`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `FlushBackupMeta` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn FlushBackupMeta(&self, ctx: &Context) -> Result<()> {
            let data = self.meta.Marshal()?;
            self.storage.WriteFile(ctx, &self.name, &data)
        }
    }

    /// `CheckBackupMetaCompatibilityFromBytes`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `CheckBackupMetaCompatibilityFromBytes` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn CheckBackupMetaCompatibilityFromBytes(_data: &[u8], _meta: &BackupMeta) -> Result<()> {
        Ok(())
    }
}

// SQL 会话/执行器内存桩，支撑 id-map 表读取等路径。
pub mod glue {
    use super::{Context, Result};

    /// `Session`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `Session` 契约：返回值与副作用应可被上层测试稳定观察。
    pub trait Session: Send + Sync {
        /// `Close`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Close` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn Close(&mut self);
        /// `ExecuteInternal`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ExecuteInternal` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ExecuteInternal(&self, _ctx: &Context, sql: &str, args: &[SqlArg]) -> Result<()>;
        /// `GetSessionCtx`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetSessionCtx` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetSessionCtx(&self) -> &SessionCtx;
        /// SQL transport boundary used by flow-control configuration queries and writes.
        fn ExecRestrictedSQL(&self, ctx: &Context, sql: &str, args: &[SqlArg]) -> Result<Vec<Row>> {
            self.GetSessionCtx()
                .GetRestrictedSQLExecutor()
                .ExecRestrictedSQL(ctx, None, sql, args)
        }
    }

    #[derive(Clone, Debug)]
    /// `SqlArg`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `SqlArg` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub enum SqlArg {
        U64(u64),
        I64(i64),
        Bytes(Vec<u8>),
        Str(String),
    }

    /// `SqlArg` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `SqlArg` 方法边界：非法参数应返回可分类错误而非 panic。
    impl From<u64> for SqlArg {
        /// `from`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `from` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn from(v: u64) -> Self {
            SqlArg::U64(v)
        }
    }
    /// `SqlArg` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `SqlArg` 方法边界：非法参数应返回可分类错误而非 panic。
    impl From<i64> for SqlArg {
        /// `from`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `from` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn from(v: i64) -> Self {
            SqlArg::I64(v)
        }
    }
    /// `SqlArg` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `SqlArg` 方法边界：非法参数应返回可分类错误而非 panic。
    impl From<Vec<u8>> for SqlArg {
        /// `from`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `from` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn from(v: Vec<u8>) -> Self {
            SqlArg::Bytes(v)
        }
    }
    /// `SqlArg` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `SqlArg` 方法边界：非法参数应返回可分类错误而非 panic。
    impl From<&[u8]> for SqlArg {
        /// `from`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `from` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn from(v: &[u8]) -> Self {
            SqlArg::Bytes(v.to_vec())
        }
    }

    #[derive(Default)]
    /// `SessionCtx`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `SessionCtx` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct SessionCtx {
        pub executor: MemRestrictedSQLExecutor,
    }

    /// `SessionCtx` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `SessionCtx` 方法边界：非法参数应返回可分类错误而非 panic。
    impl SessionCtx {
        /// `GetRestrictedSQLExecutor`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetRestrictedSQLExecutor` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetRestrictedSQLExecutor(&self) -> &MemRestrictedSQLExecutor {
            &self.executor
        }
    }

    #[derive(Clone, Debug, Default)]
    /// `Row`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Row` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Row {
        pub cols: Vec<SqlArg>,
    }

    /// `Row` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Row` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Row {
        /// `GetUint64`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetUint64` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetUint64(&self, i: usize) -> u64 {
            match self.cols.get(i) {
                Some(SqlArg::U64(v)) => *v,
                _ => 0,
            }
        }
        /// `GetBytes`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetBytes` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetBytes(&self, i: usize) -> Vec<u8> {
            match self.cols.get(i) {
                Some(SqlArg::Bytes(v)) => v.clone(),
                _ => Vec::new(),
            }
        }
    }

    #[derive(Default)]
    /// `MemRestrictedSQLExecutor`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MemRestrictedSQLExecutor` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MemRestrictedSQLExecutor {
        pub rows: std::sync::Mutex<Vec<Row>>,
    }

    /// `MemRestrictedSQLExecutor` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `MemRestrictedSQLExecutor` 方法边界：非法参数应返回可分类错误而非 panic。
    impl MemRestrictedSQLExecutor {
        /// `ExecRestrictedSQL`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ExecRestrictedSQL` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn ExecRestrictedSQL(
            &self,
            _ctx: &super::Context,
            _opts: Option<()>,
            _sql: &str,
            _args: &[SqlArg],
        ) -> Result<Vec<Row>> {
            Ok(self.rows.lock().unwrap().clone())
        }
    }

    #[derive(Default)]
    /// `MemSession`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MemSession` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MemSession {
        pub ctx: SessionCtx,
        pub executed: std::sync::Mutex<Vec<(String, Vec<SqlArg>)>>,
        pub closed: bool,
    }

    /// `MemSession` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `MemSession` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Session for MemSession {
        /// `Close`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Close` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn Close(&mut self) {
            self.closed = true;
        }
        /// `ExecuteInternal`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ExecuteInternal` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ExecuteInternal(&self, _ctx: &Context, sql: &str, args: &[SqlArg]) -> Result<()> {
            self.executed
                .lock()
                .unwrap()
                .push((sql.to_string(), args.to_vec()));
            Ok(())
        }
        /// `GetSessionCtx`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetSessionCtx` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetSessionCtx(&self) -> &SessionCtx {
            &self.ctx
        }
    }

    /// `Glue`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `Glue` 契约：返回值与副作用应可被上层测试稳定观察。
    pub trait Glue: Send + Sync {
        /// `CreateSession`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `CreateSession` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn CreateSession(&self, _store: &dyn super::kv::Storage) -> Result<Box<dyn Session>>;
    }

    #[derive(Default)]
    /// `MemGlue`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MemGlue` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MemGlue;

    /// `MemGlue` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `MemGlue` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Glue for MemGlue {
        /// `CreateSession`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `CreateSession` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn CreateSession(&self, _store: &dyn super::kv::Storage) -> Result<Box<dyn Session>> {
            Ok(Box::new(MemSession::default()))
        }
    }
}

// KV KeyRange 等基础类型。
pub mod kv {
    use super::{Context, Result};

    pub use super::kv_entry::Entry;

    /// `InternalTxnBR`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub const InternalTxnBR: &str = "br";

    /// `WithInternalSourceType`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `WithInternalSourceType` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn WithInternalSourceType(ctx: Context, _src: &str) -> Context {
        ctx
    }

    /// `Storage`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `Storage` 契约：返回值与副作用应可被上层测试稳定观察。
    pub trait Storage: Send + Sync {
        /// `Close`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Close` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn Close(&self) -> Result<()>;
    }

    #[derive(Default)]
    /// `MemKVStorage`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MemKVStorage` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MemKVStorage;

    /// `MemKVStorage` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `MemKVStorage` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Storage for MemKVStorage {
        /// `Close`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Close` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn Close(&self) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `KeyRange`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `KeyRange` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct KeyRange {
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
    }
}

// Domain/InfoSchema 最小内存实现。
pub mod domain {
    #[derive(Default)]
    /// `Domain`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Domain` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Domain {
        pub info_schema: InfoSchema,
        pub has_restore_id_column: bool,
    }

    /// `Domain` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Domain` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Domain {
        /// `InfoSchema`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `InfoSchema` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn InfoSchema(&self) -> &InfoSchema {
            &self.info_schema
        }
    }

    #[derive(Default)]
    /// `InfoSchema`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `InfoSchema` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct InfoSchema {
        pub tables: std::collections::HashSet<(String, String)>,
    }

    /// `InfoSchema` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `InfoSchema` 方法边界：非法参数应返回可分类错误而非 panic。
    impl InfoSchema {
        /// `TableExists`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `TableExists` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn TableExists(&self, db: &str, table: &str) -> bool {
            self.tables
                .contains(&(db.to_lowercase(), table.to_lowercase()))
        }
    }
}

// 对 restore/misc 的再导出或薄包装，避免循环依赖。
pub mod restore_misc {
    use super::domain::Domain;

    /// `HasRestoreIDColumn`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `HasRestoreIDColumn` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn HasRestoreIDColumn(dom: &Domain) -> bool {
        dom.has_restore_id_column
    }
}

// 操作上下文 hint 字段存取。
pub mod operation {
    use std::collections::HashMap;

    #[derive(Clone, Debug, Default)]
    /// 上下文桩：支持 WithCancel/WithTimeout 的最小可取消语义。
    pub struct Context {
        hints: HashMap<String, String>,
    }

    /// `Context` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Context` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Context {
        /// `SetHintField`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `SetHintField` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn SetHintField(&mut self, k: &str, v: &str) {
            if v.is_empty() {
                self.hints.remove(k);
            } else {
                self.hints.insert(k.to_string(), v.to_string());
            }
        }
        /// `GetHintField`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetHintField` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn GetHintField(&self, k: &str) -> Option<&str> {
            self.hints.get(k).map(|s| s.as_str())
        }
    }
}

// PD 客户端桩：TS/store 列表等。
pub mod pd {
    use super::metapb;
    use super::{Context, Result};

    /// `Client`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `Client` 契约：返回值与副作用应可被上层测试稳定观察。
    pub trait Client: Send + Sync {
        /// `GetClusterID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetClusterID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetClusterID(&self, _ctx: &Context) -> u64;
        /// `GetAllStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetAllStores` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>>;
    }

    #[derive(Clone, Default)]
    /// `MemPdClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MemPdClient` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MemPdClient {
        pub cluster_id: u64,
        pub stores: Vec<metapb::Store>,
    }

    /// `MemPdClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `MemPdClient` 方法边界：非法参数应返回可分类错误而非 panic。
    impl Client for MemPdClient {
        /// `GetClusterID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetClusterID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetClusterID(&self, _ctx: &Context) -> u64 {
            self.cluster_id
        }
        /// `GetAllStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetAllStores` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
            Ok(self.stores.clone())
        }
    }
}

// PD 客户端桩：TS/store 列表等。
pub mod pdhttp {
    use super::Context;
    use astersql_errors::SharedError;
    use std::collections::HashMap;
    use std::sync::Arc;
    pub trait ReplicateConfigClient: Send + Sync {
        fn GetReplicateConfig(
            &self,
            ctx: &Context,
        ) -> std::result::Result<HashMap<String, serde_json::Value>, SharedError>;
    }
    #[derive(Clone, Default)]
    pub struct Client {
        pub backend: Option<Arc<dyn ReplicateConfigClient>>,
    }
}

// TiDB 工具函数桩。
pub mod tidbutil {
    use std::sync::Arc;

    /// `WorkerPool`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `WorkerPool` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct WorkerPool {
        size: u32,
    }

    /// `NewWorkerPool`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `NewWorkerPool` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn NewWorkerPool(size: u32, _name: &str) -> WorkerPool {
        WorkerPool { size }
    }

    /// `WorkerPool` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `WorkerPool` 方法边界：非法参数应返回可分类错误而非 panic。
    impl WorkerPool {
        /// `ApplyOnErrorGroup`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ApplyOnErrorGroup` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn ApplyOnErrorGroup<F>(&self, _eg: &ErrorGroup, f: F)
        where
            F: FnOnce() + Send + 'static,
        {
            f();
        }
        /// `Size`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Size` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Size(&self) -> u32 {
            self.size
        }
    }

    /// `ErrorGroup`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `ErrorGroup` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct ErrorGroup;
    /// `ErrorGroup` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `ErrorGroup` 方法边界：非法参数应返回可分类错误而非 panic。
    impl ErrorGroup {
        /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `new` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn new() -> Self {
            Self
        }
        /// `Wait`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Wait` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Wait(&self) -> super::Result<()> {
            Ok(())
        }
    }
}

// Importer gRPC 客户端内存实现。
pub mod importclient {
    use super::import_sstpb;
    use super::{Context, Result};
    use std::sync::{Arc, Mutex};

    /// `ImporterClient`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `ImporterClient` 契约：返回值与副作用应可被上层测试稳定观察。
    pub trait ImporterClient: Send + Sync {
        /// `CloseGrpcClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `CloseGrpcClient` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn CloseGrpcClient(&self) -> Result<()>;
        /// `ClearFiles`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ClearFiles` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ClearFiles(
            &self,
            _ctx: &Context,
            _store_id: u64,
            _req: &import_sstpb::ClearRequest,
        ) -> Result<import_sstpb::ClearResponse>;
        /// `ApplyKVFile`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ApplyKVFile` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ApplyKVFile(
            &self,
            _ctx: &Context,
            _store_id: u64,
            _req: &import_sstpb::ApplyRequest,
        ) -> Result<import_sstpb::ApplyResponse>;
    }

    #[derive(Default)]
    /// `MemImporterClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MemImporterClient` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MemImporterClient {
        pub closed: Mutex<bool>,
        pub applied: Mutex<usize>,
        pub cleared: Mutex<usize>,
        pub fail_apply: Mutex<Option<super::Error>>,
        pub apply_pb_error: Mutex<Option<import_sstpb::Error>>,
    }

    /// `MemImporterClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `MemImporterClient` 方法边界：非法参数应返回可分类错误而非 panic。
    impl ImporterClient for MemImporterClient {
        /// `CloseGrpcClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `CloseGrpcClient` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn CloseGrpcClient(&self) -> Result<()> {
            *self.closed.lock().unwrap() = true;
            Ok(())
        }
        /// `ClearFiles`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ClearFiles` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ClearFiles(
            &self,
            _ctx: &Context,
            _store_id: u64,
            _req: &import_sstpb::ClearRequest,
        ) -> Result<import_sstpb::ClearResponse> {
            *self.cleared.lock().unwrap() += 1;
            Ok(import_sstpb::ClearResponse::default())
        }
        /// `ApplyKVFile`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ApplyKVFile` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ApplyKVFile(
            &self,
            _ctx: &Context,
            _store_id: u64,
            _req: &import_sstpb::ApplyRequest,
        ) -> Result<import_sstpb::ApplyResponse> {
            if let Some(err) = self.fail_apply.lock().unwrap().clone() {
                // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
                return Err(err);
            }
            *self.applied.lock().unwrap() += 1;
            Ok(import_sstpb::ApplyResponse {
                Error: self.apply_pb_error.lock().unwrap().clone(),
            })
        }
    }

    /// `SharedImporter`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `SharedImporter` 契约：返回值与副作用应可被上层测试稳定观察。
    pub type SharedImporter = Arc<dyn ImporterClient>;
}

// RawKV 客户端占位。
pub mod rawkv {
    use super::{Context, Result};

    /// `RawKVBatchClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `RawKVBatchClient` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct RawKVBatchClient {
        pub closed: bool,
    }

    /// `RawKVBatchClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `RawKVBatchClient` 方法边界：非法参数应返回可分类错误而非 panic。
    impl RawKVBatchClient {
        /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `new` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn new() -> Self {
            Self { closed: false }
        }
        /// `Close`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Close` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Close(&mut self) {
            self.closed = true;
        }
        /// `Put`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `Put` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        pub fn Put(&self, _ctx: &Context, _key: &[u8], _value: &[u8], _ts: u64) -> Result<()> {
            Ok(())
        }
    }
}

// 连接管理占位。
pub mod conn {
    use super::metapb;
    use super::pd;
    use super::{Context, Result};

    /// `GetAllTiKVStoresWithRetry`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetAllTiKVStoresWithRetry` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn GetAllTiKVStoresWithRetry(
        ctx: &Context,
        pd_client: &dyn pd::Client,
        _skip_tiflash: bool,
    ) -> Result<Vec<metapb::Store>> {
        pd_client.GetAllStores(ctx)
    }

    /// `util`：子模块，聚合相关桩类型与辅助函数。
    pub mod util {
        /// `SkipTiFlash`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
        /// 调整阈值前确认是否影响重试次数或批大小语义。
        pub const SkipTiFlash: bool = true;
    }
}

// 加密管理占位。
pub mod encryption {
    #[derive(Default)]
    /// `Manager`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Manager` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Manager;
}

// failpoint 开关桩：默认关闭。
pub mod failpoint {
    /// `Inject`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Inject` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Inject(_name: &str, _f: impl FnOnce()) {}
}

// Region 拆分客户端内存实现。
pub mod split_client {
    use super::metapb;
    use super::{Context, Result};
    use std::sync::Mutex;

    #[derive(Clone, Debug, Default)]
    /// `RegionInfo`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `RegionInfo` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct RegionInfo {
        pub Region: Option<metapb::Region>,
        pub Leader: Option<metapb::Peer>,
    }

    /// `SplitClient`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
    /// 实现方需保持与 Go 接口相同的错误可重试语义。
    /// `SplitClient` 契约：返回值与副作用应可被上层测试稳定观察。
    pub trait SplitClient: Send + Sync {
        /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetRegionByID(&self, ctx: &Context, id: u64) -> Result<RegionInfo>;
        /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ScanRegions(
            &self,
            ctx: &Context,
            start: &[u8],
            end: &[u8],
            limit: i32,
        ) -> Result<Vec<RegionInfo>>;
    }

    /// `CheckRegionEpoch`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `CheckRegionEpoch` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn CheckRegionEpoch(a: &RegionInfo, b: &RegionInfo) -> bool {
        match (&a.Region, &b.Region) {
            (Some(ra), Some(rb)) => ra.RegionEpoch == rb.RegionEpoch && ra.Id == rb.Id,
            _ => false,
        }
    }

    /// `ScanRegionPaginationLimit`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub const ScanRegionPaginationLimit: i32 = 128;

    /// `PaginateScanRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `PaginateScanRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn PaginateScanRegion(
        ctx: &Context,
        client: &dyn SplitClient,
        start: &[u8],
        end: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionInfo>> {
        client.ScanRegions(ctx, start, end, limit)
    }

    #[derive(Default)]
    /// `MemSplitClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `MemSplitClient` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct MemSplitClient {
        pub regions: Mutex<Vec<RegionInfo>>,
        pub by_id: Mutex<std::collections::HashMap<u64, RegionInfo>>,
    }

    /// `MemSplitClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `MemSplitClient` 方法边界：非法参数应返回可分类错误而非 panic。
    impl SplitClient for MemSplitClient {
        /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetRegionByID(&self, _ctx: &Context, id: u64) -> Result<RegionInfo> {
            self.by_id
                .lock()
                .unwrap()
                .get(&id)
                .cloned()
                .ok_or_else(|| super::Error::new(format!("region {id} not found")))
        }
        /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ScanRegions(
            &self,
            _ctx: &Context,
            _start: &[u8],
            _end: &[u8],
            _limit: i32,
        ) -> Result<Vec<RegionInfo>> {
            Ok(self.regions.lock().unwrap().clone())
        }
    }
}

// KV KeyRange 等基础类型。
pub mod kv_entry {
    #[derive(Clone, Debug, Default)]
    /// `Entry`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Entry` 生命周期：构造后是否可变、是否跨线程共享需明确。
    pub struct Entry {
        pub Key: Vec<u8>,
        pub Value: Vec<u8>,
    }
}
