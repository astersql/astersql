// Copyright 2026 AsterSQL.
//! Local stand-ins for MySQL/SQL/storage/config/common/mydump/verification/model
//! boundaries (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! 中文总览：这个文件不是 `checkpoints` 包的真实业务实现，而是迁移过程中承接外部依赖边界的桩层。
//! 它的首要目标是把 Go 版 `checkpoints` 依赖到的周边能力压缩成可编译、可测试、可观察的最小接口。
//! 因为当前 crate 主要验证检查点协议本身，所以这里优先保证数据形状、错误文本和调用顺序可被测试使用。
//! 这意味着某些模块会故意只保留最薄语义，而不会实现完整生产能力。
//! 例如内存 SQL、对象存储和日志接口都只提供 checkpoint 测试真正依赖的那部分动作。
//! 注释必须明确这种边界，避免后续维护者把桩误认为真实接线已经完成。
//! 阅读时可以把本文件分成三层。
//! 第一层是错误、上下文、配置、公共工具等“基础形状”，负责复刻 Go 调用签名和常量。
//! 第二层是 mydump、checksum、JSON、SQL、存储等“测试支撑能力”，让检查点逻辑有可运行的依赖。
//! 第三层是路径、CSV 等“尾部适配器”，它们不是核心逻辑，但能帮助 parity 测试对齐 Go 辅助函数结果。
//! 这里的很多类型命名继续沿用 Go 风格大写字段，是为了降低移植期间的映射成本。
//! 因为只补注释，本次不会把任何桩偷偷升级成真实实现。
//! 若某段逻辑只是满足测试，不应被描述成“生产可用”。
//! 更准确的理解是：这些桩在 checkpoint 语义测试里充当受控替身，帮助我们隔离真正要验证的目标。
//! 当后续真实依赖被接入时，这些说明也能帮助判断哪些行为必须保留，哪些只是临时过渡。

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// errors (pingcap/errors shape)
// ---------------------------------------------------------------------------
// 这一段复刻 `pingcap/errors` 最常被 checkpoint 代码使用的外形。
// 目标不是完整移植错误库，而是保留“包装后仍可读出文本和分类标记”的最小语义。
// `not_found` 与 `no_rows` 两个布尔位承担了 Go 侧常见错误分流的职责。
// 测试会根据这些位或文本片段判断当前失败属于“缺资源”还是“查询为空”。
// 因此这里即使实现很薄，也不能把所有错误都折叠成同一种裸字符串。
// `errors_Trace` 维持透传语义，表达“这里保留包装层入口，但当前桩不额外加栈”。
// `errors_NotFoundf` 则显式打上 not_found，便于文件后端找不到表时保持与 Go 一致的分支。
// 这些 helper 名称继续沿用 Go 包函数命名，是为了让主实现迁移时不用立刻改写调用点。
// `Error::Error()` 也保留了 Go 风格读取文本的方法，而不是强迫上层完全切换到 Rust trait 风格。
// 从架构上看，这里服务的是“错误外观兼容”而不是“错误体系重建”。
// 后续如果真实错误库接入，最应该保护的是这些可观测字段和判断分支。
// 注释特别强调这一点，避免后续清理桩时误删测试仍依赖的行为。

#[derive(Clone, Debug)]
pub struct Error {
    pub msg: String,
    pub not_found: bool,
    pub no_rows: bool,
    pub class: Option<&'static str>,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            not_found: false,
            no_rows: false,
            class: None,
        }
    }
    pub fn Error(&self) -> &str {
        &self.msg
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}
impl std::error::Error for Error {}
impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        self.msg == other.msg
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn errors_New(msg: impl Into<String>) -> Error {
    Error::new(msg)
}
pub fn errors_Errorf(msg: impl fmt::Display) -> Error {
    Error::new(msg.to_string())
}
pub fn errors_Trace(err: Error) -> Error {
    err
}
pub fn errors_NotFoundf(msg: impl fmt::Display) -> Error {
    Error {
        msg: msg.to_string(),
        not_found: true,
        no_rows: false,
        class: None,
    }
}
pub fn errors_Cause(err: &Error) -> &Error {
    err
}
pub fn errors_IsNotFound(err: &Error) -> bool {
    err.not_found
}

pub mod errors {
    pub use super::{
        Error, Result, errors_Cause as Cause, errors_Errorf as Errorf,
        errors_IsNotFound as IsNotFound, errors_New as New, errors_NotFoundf as NotFoundf,
        errors_Trace as Trace,
    };
}

// ---------------------------------------------------------------------------
// context
// ---------------------------------------------------------------------------
// checkpoint 逻辑对 `context` 的使用非常轻量。
// 当前桩只需要提供可复制的 `Context` 值和 `Background()` 入口。
// 它不承载取消信号、deadline 或键值传递。
// 这样做是因为本包测试关注的是检查点协议，而不是异步取消传播。
// 保留独立模块仍然有意义：主实现调用签名可以继续与 Go 对齐。
// 当未来需要接真实上下文能力时，也能在这里集中替换，而不用碰业务代码。
// 因此这一段的价值在于“固定接口位置”，而不是“模拟完整上下文语义”。
// 读到 `Context` 时，应把它理解成测试用占位令牌。

pub mod context {
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Context;

    pub fn Background() -> Context {
        Context
    }
}

// ---------------------------------------------------------------------------
// build
// ---------------------------------------------------------------------------
// `build` 模块只暴露版本字符串。
// 文件检查点初始化会把版本号写进任务级元信息，用于回放和排障。
// 对 checkpoint 测试而言，值本身不重要，关键是字段存在且可稳定读取。
// 所以这里给出一个固定 stub 版本，既能参与序列化，也不会引入外部构建依赖。
// 这类常量桩属于“协议字段供给者”，不是功能逻辑本体。
// 后续若改为真实版本来源，也应保持字段可用和文本稳定性。

pub mod build {
    pub static ReleaseVersion: &str = "v0.0.0-astersql-stub";
}

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------
// 这一段复刻 checkpoint 真正会访问到的配置形状。
// 它把 Go `Config` 中和 checkpoint 驱动、数据源、TiDB、Importer 有关的字段抽出来。
// 原则是“只保留当前包测试确实读取的字段，不额外发明默认逻辑”。
// `Checkpoint` 子结构决定驱动类型、DSN、schema 和 MySQL 连接参数。
// `Mydumper`、`TikvImporter`、`TiDB` 则承接任务级元信息初始化所需的输入。
// 注意这里的大写字段名不是 Rust 风格问题，而是刻意维持与 Go 结构体的视觉映射。
// 这样在主实现和测试里对照 Go 代码时，可以更直观地确认字段来自哪里。
// `Default` 让测试夹具能快速构造“只覆盖少数字段”的配置。
// 这与 Go 测试中常见的字面量覆盖模式保持一致。
// 配置桩不负责校验字段组合是否合法。
// 真正的校验仍应由 checkpoint/open/init 等上层逻辑承担。
// 因此这里强调的是“承接输入形状”，而不是“保证配置正确”。
// 阅读本段时，可把它视为 checkpoint 协议与外部配置系统之间的最薄桥接层。
// 一旦真实配置 crate 替换它，需要优先保证这些字段名和默认可达路径不变。

pub mod config {
    use super::sql::MySQLConnectParam;

    pub const CheckpointDriverMySQL: &str = "mysql";
    pub const CheckpointDriverFile: &str = "file";

    #[derive(Clone, Debug, Default)]
    pub struct Checkpoint {
        pub Enable: bool,
        pub Driver: String,
        pub DSN: String,
        pub Schema: String,
        pub MySQLParam: Option<MySQLConnectParam>,
    }

    #[derive(Clone, Debug, Default)]
    pub struct Mydumper {
        pub SourceDir: String,
    }

    #[derive(Clone, Debug, Default)]
    pub struct TikvImporter {
        pub Backend: String,
        pub Addr: String,
        pub SortedKVDir: String,
        pub AddIndexBySQL: bool,
    }

    #[derive(Clone, Debug, Default)]
    pub struct TiDB {
        pub Host: String,
        pub Port: i32,
        pub PdAddr: String,
    }

    #[derive(Clone, Debug, Default)]
    pub struct Config {
        pub TaskID: i64,
        pub Checkpoint: Checkpoint,
        pub Mydumper: Mydumper,
        pub TikvImporter: TikvImporter,
        pub TiDB: TiDB,
    }
}

// ---------------------------------------------------------------------------
// common
// ---------------------------------------------------------------------------
// `common` 模块是这个桩文件里最像“公共工具箱”的部分。
// 它集中放置了检查点逻辑频繁复用的标识符处理、格式化和错误模板。
// `AllTables` 对应删除全部检查点时使用的保留常量，
// 也是 parity 测试里必须单独保护的协议值。
// `EscapeIdentifier` 与 `UniqueTable` 负责把 schema/table 名字转换成 Go 同款反引号形式。
// 这关系到 SQL 语句文本、checkpoint key 和测试断言是否一致。
// `SprintfWithIdentifiers` 则模仿 Go 的格式化路径，把 `%s` 和 `%[n]s` 映射到已转义标识符。
// 它看似只是字符串拼装，但会影响创建 schema/table/view 时生成的 SQL。
// 一旦这里行为漂移，很多后续断言会在错误地方失败，排查成本很高。
// `NormalizedError` 承担了预置错误模板的职责，
// 让上层能像 Go 那样先定义错误，再在需要时补参数生成文本。
// 这里不实现完整错误码系统，只保留检查点测试会观察到的 `GenWithStackByArgs` 路径。
// 另外，这个模块中保留了扫描结果结构和辅助数据形状，
// 是 SQL 桩与 checkpoint 主逻辑之间交换数据的共享契约。
// 从依赖方向看，`common` 让其他桩避免互相循环引用。
// 从维护视角看，最值得保护的是字符串转义和模板替换规则。
// 因为这些规则一旦改变，往往不是编译器报错，而是行为细节静默偏离 Go。
// 所以中文注释在这里特别强调“为什么这些小工具也是协议的一部分”。
// 它们不是杂项实现，而是驱动 checkpoint 行为稳定的辅助边界。

pub mod common {
    use super::{Error, Result};

    pub const AllTables: &str = "all";

    pub fn EscapeIdentifier(identifier: &str) -> String {
        let mut builder = String::with_capacity(identifier.len() + 2);
        builder.push('`');
        for b in identifier.bytes() {
            if b == b'`' {
                builder.push_str("``");
            } else {
                builder.push(b as char);
            }
        }
        builder.push('`');
        builder
    }

    pub fn UniqueTable(schema: &str, table: &str) -> String {
        format!("{}.{}", EscapeIdentifier(schema), EscapeIdentifier(table))
    }

    /// Go `fmt.Sprintf` with escaped identifiers; supports `%s`, `%%`, and `%[n]s`.
    pub fn SprintfWithIdentifiers(format: &str, identifiers: &[&str]) -> String {
        let escaped: Vec<String> = identifiers.iter().map(|s| EscapeIdentifier(s)).collect();
        sprintf_go(format, &escaped)
    }

    fn sprintf_go(format: &str, args: &[String]) -> String {
        let mut out = String::new();
        let bytes = format.as_bytes();
        let mut i = 0;
        let mut next = 0usize;
        while i < bytes.len() {
            if bytes[i] != b'%' {
                out.push(bytes[i] as char);
                i += 1;
                continue;
            }
            i += 1;
            if i >= bytes.len() {
                out.push('%');
                break;
            }
            if bytes[i] == b'%' {
                out.push('%');
                i += 1;
                continue;
            }
            let mut idx: Option<usize> = None;
            if bytes[i] == b'[' {
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b']' {
                    if let Ok(n) = std::str::from_utf8(&bytes[start..i])
                        .unwrap_or("")
                        .parse::<usize>()
                    {
                        idx = Some(n.saturating_sub(1));
                    }
                    i += 1;
                }
            }
            if i < bytes.len() && bytes[i] == b's' {
                let use_idx = idx.unwrap_or_else(|| {
                    let cur = next;
                    next += 1;
                    cur
                });
                if idx.is_some() {
                    // indexed verbs do not advance the implicit counter in Go for %s after %[n]s
                    // but sequential %s after still uses its own counter — match Go loosely:
                    // when idx is explicit, do not advance `next`.
                }
                if let Some(v) = args.get(use_idx) {
                    out.push_str(v);
                } else {
                    out.push_str("%s");
                }
                i += 1;
                continue;
            }
            out.push('%');
        }
        out
    }

    #[derive(Clone, Debug)]
    pub struct NormalizedError {
        pub template: &'static str,
    }

    impl NormalizedError {
        pub fn GenWithStackByArgs(&self, args: &[&str]) -> Error {
            let mut msg = self.template.to_string();
            for a in args {
                if let Some(pos) = msg.find("%s") {
                    msg.replace_range(pos..pos + 2, a);
                }
            }
            let mut error = Error::new(msg);
            if self.template == "checkpoint for table %s not found" {
                error.not_found = true;
                error.class = Some("ErrCheckpointTableNotFound");
            }
            error
        }
    }

    pub const ErrUnknownCheckpointDriver: NormalizedError = NormalizedError {
        template: "unknown checkpoint driver '%s'",
    };
    pub const ErrCheckpointTableNotFound: NormalizedError = NormalizedError {
        template: "checkpoint for table %s not found",
    };

    #[derive(Clone, Debug)]
    pub struct SQLWithRetry {
        pub DB: super::sql::DB,
        pub Logger: super::log::Logger,
        pub HideQueryLog: bool,
    }

    impl SQLWithRetry {
        pub fn Exec(
            &self,
            _ctx: super::context::Context,
            _name: &str,
            query: String,
        ) -> Result<()> {
            self.DB.Exec(query.as_str(), &[])
        }

        pub fn Transact<F>(&self, ctx: super::context::Context, _name: &str, f: F) -> Result<()>
        where
            F: FnOnce(super::context::Context, &super::sql::Tx) -> Result<()>,
        {
            let tx = self.DB.Begin()?;
            f(ctx, &tx)?;
            tx.Commit()
        }

        pub fn QueryRow(
            &self,
            _ctx: super::context::Context,
            _name: &str,
            query: String,
            dest: &mut TaskCheckpointScan,
        ) -> Result<()> {
            self.DB.QueryRowTask(query.as_str(), dest)
        }
    }

    /// Destination for TaskCheckpoint QueryRow.
    #[derive(Clone, Debug, Default)]
    pub struct TaskCheckpointScan {
        pub TaskID: i64,
        pub SourceDir: String,
        pub Backend: String,
        pub ImporterAddr: String,
        pub TiDBHost: String,
        pub TiDBPort: i32,
        pub PdAddr: String,
        pub SortedKVDir: String,
        pub LightningVer: String,
    }

    pub fn Retry<F>(_name: &str, _logger: super::log::Logger, mut f: F) -> Result<()>
    where
        F: FnMut() -> Result<()>,
    {
        f()
    }
}

// Fix ErrUnknownCheckpointDriver - can't have empty static with String. Use functions only.
// Re-export helpers used like common::ErrUnknownCheckpointDriver.GenWithStackByArgs

// ---------------------------------------------------------------------------
// importdef / model / mydump / verification
// ---------------------------------------------------------------------------
// 这一组模块共同描述 checkpoint 读取和持久化时会碰到的数据形状。
// `model` 给出目标表的最小模型，用于表示“期望导入到哪张表”。
// `importdef` 再把数据库和表的导入视图组合起来，服务初始化阶段的 DB/table 枚举。
// 这里故意只保留 checkpoint 真正会写入或比较的字段，
// 避免把整个 importer 数据模型都搬进来。
// `mydump` 模块提供源文件类型、压缩方式、文件元信息和 chunk 偏移。
// 这些字段直接决定 chunk checkpoint 的序列化内容与恢复坐标。
// `verify` 中的 `KVChecksum` 则承接 Go `verification` 的最小统计语义。
// 它保存字节数、KV 数和校验和值，用来判断某个 chunk 或表的进度是否一致。
// 这里的 `MakeKVChecksum` 命名和访问器都与 Go 保持接近，
// 方便在断言中直接照抄 Go 夹具值。
// 这一整组类型的职责不是“让 importer 运行起来”，
// 而是“让 checkpoint 数据能长得像 Go 期待的样子”。
// 所以即使字段不多，也都属于协议关键路径。
// 后续若真实模型接入，最应优先保护的是字段名、默认值和比较方式。
// 尤其 `Chunk` 偏移和 `KVChecksum` 三元组，一旦含义漂移，会直接影响恢复与进度展示。
// 注释把这些关系写清楚，是为了提醒维护者：这是数据契约层，不是普通 DTO 堆砌。

pub mod model {
    // `model` 保留的是目标表最薄的元信息镜像。
    // checkpoint 只需要知道表 ID 和名字，就能把“当前恢复目标是谁”串起来。
    // 这里不引入更完整的列、索引、schema 状态，是因为本包测试不会消费那些字段。
    // 这让序列化和比较逻辑保持聚焦，不被无关元数据噪声干扰。
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
    pub struct TableInfo {
        pub id: i64,
        pub name: String,
    }
}

pub mod importdef {
    // `importdef` 是比 `model` 更贴近导入流程的一层包装。
    // 它把“导入时看到的表”与“期望写入的目标表”并置，便于初始化 checkpoint。
    // DBInfo/TableInfo 组合后，主实现就能像 Go 那样逐库逐表创建初始状态。
    // 因为这里只服务 checkpoint，字段仍然严格限制在当前需要的最小集合。
    use super::model;

    #[derive(Clone, Debug, Default)]
    pub struct TableInfo {
        pub Name: String,
        pub ID: i64,
        pub Desired: Option<model::TableInfo>,
    }

    #[derive(Clone, Debug, Default)]
    pub struct DBInfo {
        pub Name: String,
        pub Tables: Vec<TableInfo>,
    }
}

pub mod mydump {
    // `mydump` 子模块定义源文件与 chunk 的输入坐标。
    // 对 checkpoint 来说，真正重要的是文件身份、压缩方式和偏移边界。
    // 这些值决定了断点恢复时“从哪继续读、已经读到哪、对应哪个逻辑片段”。
    // 因此即便类型很小，也属于恢复协议的核心组成部分。
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct SourceType(pub i32);

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Compression(pub i32);

    pub const CompressionNone: Compression = Compression(0);

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct ExtendColumnData {
        pub Columns: Vec<String>,
        pub Values: Vec<String>,
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct SourceFileMeta {
        pub Path: String,
        pub Type: SourceType,
        pub Compression: Compression,
        pub SortKey: String,
        pub FileSize: i64,
        pub ExtendData: ExtendColumnData,
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Chunk {
        pub Offset: i64,
        pub RealOffset: i64,
        pub EndOffset: i64,
        pub PrevRowIDMax: i64,
        pub RowIDMax: i64,
    }
}

pub mod verify {
    // `verify` 把 Go 中用于累积校验和的统计对象缩成最小三元组。
    // bytes、kvs、checksum 三个数字一起构成“已处理数据量与内容指纹”的摘要。
    // checkpoint 更新和测试断言都依赖这组三元组稳定存在。
    // 所以这里虽然不实现复杂 checksum 算法，但必须保留读取接口和构造入口。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct KVChecksum {
        bytes: u64,
        kvs: u64,
        checksum: u64,
    }

    pub fn MakeKVChecksum(bytes: u64, kvs: u64, checksum: u64) -> KVChecksum {
        KVChecksum {
            bytes,
            kvs,
            checksum,
        }
    }

    impl KVChecksum {
        pub fn SumSize(&self) -> u64 {
            self.bytes
        }
        pub fn SumKVS(&self) -> u64 {
            self.kvs
        }
        pub fn Sum(&self) -> u64 {
            self.checksum
        }
    }
}

// ---------------------------------------------------------------------------
// log / logutil / zap (no-ops)
// ---------------------------------------------------------------------------
// checkpoint 测试并不依赖真实日志系统，但主实现又会频繁调用 logger 链式接口。
// 因此这里提供 no-op 版本的 `log`、`logutil` 和 `zap`。
// 它们的重点在于“调用不会炸、签名能对上”，而不是“真的输出什么”。
// `Logger::With` 返回自身，表达链式附加字段在桩环境下被安全忽略。
// `logutil::Logger` 仍保留按 context 取 logger 的入口，维持调用点结构稳定。
// `zap::Field` 只是占位类型，用来承接 `String`、`Int64`、`Error` 等构造函数。
// 这让主实现保留与 Go 近似的日志书写方式，同时又不引入真实日志依赖。
// 中文注释强调它们是“无副作用兼容层”，避免被误解成生产日志接线已经完成。
// 如果未来接入真实日志库，也应继续允许这些调用路径无条件存在。
// 因为对 checkpoint 逻辑来说，日志是观测辅助手段，不应反向改变核心语义。

pub mod log {
    #[derive(Clone, Debug, Default)]
    pub struct Logger;

    impl Logger {
        pub fn With(self, _field: super::zap::Field) -> Self {
            self
        }
    }

    pub fn Wrap(_l: Logger) -> Logger {
        Logger
    }
}

pub mod logutil {
    pub fn Logger(_ctx: super::context::Context) -> super::log::Logger {
        super::log::Logger
    }
}

pub mod zap {
    #[derive(Clone, Debug)]
    pub struct Field;
    pub fn String(_k: &str, _v: &str) -> Field {
        Field
    }
    pub fn Int64(_k: &str, _v: i64) -> Field {
        Field
    }
    pub fn Error(_e: &super::Error) -> Field {
        Field
    }
}

// ---------------------------------------------------------------------------
// json
// ---------------------------------------------------------------------------
// JSON 桩只服务于 checkpoint 需要的序列化与反序列化。
// `Marshal` 直接委托给 `serde_json`，保持“结构能转成字节”的基本语义。
// `Unmarshal` 额外保留了空输入时报错的行为，
// 目的是贴近 Go `unexpected end of JSON input` 这类常见失败信号。
// 这使文件后端和 SQL 桩在读取空字段时，能给出更接近 Go 的反馈。
// 这里不追求覆盖全部 JSON 边界条件。
// 对 checkpoint 测试而言，最重要的是空输入和正常 round-trip 两个场景。
// 因此它属于典型的“足够表达契约的最小实现”。
// 注释在这里帮助读者区分：为什么选择简化，但又没有把错误语义完全省掉。

pub mod json {
    use super::{Error, Result};
    use serde::Serialize;

    pub fn Marshal<T: Serialize>(v: &T) -> Result<Vec<u8>> {
        serde_json::to_vec(v).map_err(|e| Error::new(e.to_string()))
    }

    pub fn Unmarshal<T: serde::de::DeserializeOwned>(data: &[u8]) -> Result<T> {
        if data.is_empty() {
            return Err(Error::new("unexpected end of JSON input"));
        }
        serde_json::from_slice(data).map_err(|e| Error::new(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// sql stub (in-memory)
// ---------------------------------------------------------------------------
// `sql` 是整个桩文件里最关键的一层，因为 MySQL checkpoint 后端的大部分行为都要靠它承接。
// 这里没有接真实数据库，而是用内存结构保存 schema、任务记录、表记录、engine 和 chunk 行。
// 设计目标不是复刻 SQL 解析器，而是为 checkpoint 主实现提供“看起来像数据库”的最小载体。
// `DB` 保存共享状态和执行日志，便于测试同时观察副作用与结果。
// `exec_log` 特别重要，因为很多 parity/单元测试会直接断言创建表或视图的 SQL 是否被调用。
// `Exec` 只识别少数 DDL 前缀，并按语义更新内存中的 schema 状态。
// 这说明它更像“命令记录器 + 最小状态机”，而不是真 SQL 执行器。
// `QueryRowTask` 负责返回任务级 checkpoint，区分“有记录”与“无记录”两种结果。
// 这里保留 `no_rows` 语义，是为了让调用方像 Go `database/sql` 一样判断空结果。
// `QueryContext` 当前返回空行集，体现的是边界占位，而不是完整查询支持。
// `Tx`、`Stmt` 和 `ExecContext` 则提供了与 Go 版事务写入形状相近的调用接口。
// 尤其 `Stmt::ExecContext` 会按参数数量和语句关键词，把任务、表、engine、chunk 数据灌进内存表。
// 这部分逻辑虽然是桩，但它直接决定 `Get`、`TaskCheckpoint` 等恢复读取是否有正确输入。
// 因此它不是完全随意的假实现，而是围绕 checkpoint 读写路径定制的窄实现。
// 这里保留许多 Go 风格名词，例如 `Rows`、`Row`、`Scan`、`ExecResult`，
// 是为了让迁移中的主实现尽量少改接口层代码。
// 也正因为如此，字段和方法会显得不像典型 Rust API。
// 注释要提醒读者：这是为了减小迁移摩擦，不是长期 API 设计定稿。
// 另一个重要点是，SQL 桩把“状态持久化”和“执行日志记录”同时保留下来。
// 前者服务读取断言，后者服务调用路径断言，两者缺一不可。
// 比如只记录 SQL 不保存表状态，就无法验证 round-trip 恢复。
// 只保存状态不记录 SQL，又无法确认 schema/view 是否按 Go 顺序创建。
// 所以这段实现的价值在于：让 checkpoint 测试可以同时观察输入、输出和副作用。
// `MySQLConnectParam::Connect` 直接返回内存 DB，也表达了当前阶段没有真实网络连接。
// 这让配置层和驱动层仍能走通，但不会把环境依赖引入测试。
// `Open` 同样总是返回内存 DB，强调 driver 名称在桩环境里只是形式参数。
// 当读到这些接口时，最重要的理解是“它们模拟的是交互面，而不是底层存储引擎”。
// 后续若替换成真实数据库接线，应优先回归执行日志、no rows、字段落点和事务入口这些契约。
// 中文注释之所以写得更长，是因为 SQL 桩最容易被误判为“已经够像生产”。
// 实际上它只覆盖 checkpoint 包当前测试真正使用到的那条窄路径。
// 超出这条路径的行为，都不应在没有额外验证前被假定成立。
// 因此维护者在扩展它时，需要先明确新增需求来自哪条真实调用路径。
// 只有这样，桩的复杂度才不会失控，同时还能保持与 Go 行为的必要对齐。

pub mod sql {
    // 进入 SQL 子模块后，可以把每个对象分别理解为数据库交互面的一个切片。
    // `DB` 是共享状态持有者，`Tx` 是事务语义包装，`Stmt` 是预编译语句外观。
    // `Rows` 和 `Row` 则负责把读路径保持成 Go 调用方熟悉的形状。
    // 这些对象组合起来，形成 checkpoint MySQL 后端测试所需的最小运行舞台。
    use super::common::TaskCheckpointScan;
    use super::{Error, Result};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug, Default)]
    pub struct MySQLConnectParam {
        pub host: String,
    }

    impl MySQLConnectParam {
        pub fn Connect(&self) -> Result<DB> {
            Ok(DB::new_memory())
        }
    }

    #[derive(Clone, Debug)]
    pub struct DB {
        inner: Arc<Mutex<DbInner>>,
    }

    // `DbInner` 里的每个字段都对应一类可观测副作用。
    // `schemas` 让建库/删库语句能留下痕迹。
    // `task`、`tables`、`engines`、`chunks` 则分别模拟四层 checkpoint 数据。
    // `exec_log` 单独保留，是为了支持“有没有发出预期 SQL”这类测试。

    #[derive(Debug, Default)]
    struct DbInner {
        closed: bool,
        /// schemas that exist
        schemas: HashMap<String, bool>,
        task: Option<TaskCheckpointScan>,
        /// table_name -> remain fields
        tables: HashMap<String, TableRow>,
        engines: HashMap<(String, i32), u8>,
        chunks: Vec<ChunkRow>,
        checkpoints: HashMap<String, crate::TableCheckpoint>,
        exec_log: Vec<String>,
    }

    #[derive(Clone, Debug, Default)]
    struct TableRow {
        status: u8,
        table_id: i64,
        table_info: Vec<u8>,
        kv_bytes: u64,
        kv_kvs: u64,
        kv_checksum: u64,
        auto_rand_base: i64,
        auto_incr_base: i64,
        auto_row_id_base: i64,
    }

    #[derive(Clone, Debug, Default)]
    struct ChunkRow {
        table_name: String,
        engine_id: i32,
        path: String,
        offset: i64,
        typ: i32,
        compression: i32,
        sort_key: String,
        file_size: i64,
        columns: Vec<u8>,
        pos: i64,
        real_pos: i64,
        end_offset: i64,
        prev_rowid_max: i64,
        rowid_max: i64,
        kvc_bytes: u64,
        kvc_kvs: u64,
        kvc_checksum: u64,
        timestamp: i64,
    }

    impl DB {
        // `new_memory` 表明这个 DB 从一开始就不是外部资源，而是测试内共享状态容器。
        // 这样 `Clone` 后的多个句柄仍能观察到同一份 checkpoint 数据。
        pub fn new_memory() -> Self {
            Self {
                inner: Arc::new(Mutex::new(DbInner::default())),
            }
        }

        pub fn Close(&self) -> Result<()> {
            let mut g = self.inner.lock().unwrap();
            g.closed = true;
            Ok(())
        }

        pub fn Exec(&self, query: &str, _args: &[SqlValue]) -> Result<()> {
            // `Exec` 只实现 checkpoint 初始化与清理真正会触达的少量 DDL 语义。
            // 这让测试能确认状态变化和 SQL 发出顺序，但不需要引入完整 SQL 引擎。
            let mut g = self.inner.lock().unwrap();
            g.exec_log.push(query.to_string());
            let q = query.trim().to_ascii_uppercase();
            if q.starts_with("CREATE DATABASE") || q.starts_with("CREATE SCHEMA") {
                // CREATE DATABASE IF NOT EXISTS `name`
                if let Some(name) = extract_backtick_ident(query) {
                    g.schemas.insert(name, true);
                }
                return Ok(());
            }
            if q.starts_with("CREATE TABLE") {
                return Ok(());
            }
            if q.starts_with("DROP SCHEMA") || q.starts_with("DROP DATABASE") {
                if let Some(name) = extract_backtick_ident(query) {
                    g.schemas.remove(&name);
                }
                g.task = None;
                g.checkpoints.clear();
                return Ok(());
            }
            if q.starts_with("RENAME TABLE") {
                return Ok(());
            }
            Ok(())
        }

        pub fn Begin(&self) -> Result<Tx> {
            Ok(Tx { db: self.clone() })
        }

        pub fn QueryRowTask(&self, _query: &str, dest: &mut TaskCheckpointScan) -> Result<()> {
            let g = self.inner.lock().unwrap();
            match &g.task {
                Some(t) => {
                    *dest = t.clone();
                    Ok(())
                }
                None => Err(Error {
                    msg: "sql: no rows in result set".into(),
                    not_found: false,
                    no_rows: true,
                    class: None,
                }),
            }
        }

        pub fn QueryContext(&self, _ctx: super::context::Context, query: &str) -> Result<Rows> {
            let _ = query;
            Ok(Rows { rows: vec![] })
        }

        pub fn set_task(&self, task: TaskCheckpointScan) {
            self.inner.lock().unwrap().task = Some(task);
        }

        pub fn schema_exists(&self, name: &str) -> bool {
            self.inner.lock().unwrap().schemas.contains_key(name)
        }

        pub fn put_checkpoint(&self, table_name: String, checkpoint: crate::TableCheckpoint) {
            self.inner
                .lock()
                .unwrap()
                .checkpoints
                .entry(table_name)
                .or_insert(checkpoint);
        }

        pub fn replace_checkpoint(&self, table_name: String, checkpoint: crate::TableCheckpoint) {
            self.inner
                .lock()
                .unwrap()
                .checkpoints
                .insert(table_name, checkpoint);
        }

        pub fn checkpoint(&self, table_name: &str) -> Option<crate::TableCheckpoint> {
            self.inner
                .lock()
                .unwrap()
                .checkpoints
                .get(table_name)
                .cloned()
        }

        pub fn remove_checkpoint(&self, table_name: &str) {
            self.inner.lock().unwrap().checkpoints.remove(table_name);
        }

        pub fn clear_checkpoint_data(&self) {
            let mut g = self.inner.lock().unwrap();
            g.task = None;
            g.checkpoints.clear();
        }

        pub fn local_storing_tables(&self) -> HashMap<String, Vec<i32>> {
            let g = self.inner.lock().unwrap();
            let mut result = HashMap::new();
            for (table_name, table) in &g.checkpoints {
                if table.Status <= crate::CheckpointStatusMaxInvalid
                    || table.Status >= crate::CheckpointStatusIndexImported
                {
                    continue;
                }
                for (engine_id, engine) in &table.Engines {
                    if engine.Status <= crate::CheckpointStatusMaxInvalid
                        || engine.Status >= crate::CheckpointStatusImported
                    {
                        continue;
                    }
                    if engine
                        .Chunks
                        .iter()
                        .any(|chunk| chunk.Chunk.Offset > chunk.Key.Offset)
                    {
                        result
                            .entry(table_name.clone())
                            .or_insert_with(Vec::new)
                            .push(*engine_id);
                    }
                }
            }
            result
        }

        pub fn ignore_error_checkpoint(&self, table_name: &str) -> bool {
            let mut g = self.inner.lock().unwrap();
            if table_name == "all" {
                for table in g.checkpoints.values_mut() {
                    reset_invalid_statuses(table);
                }
                return true;
            }
            let Some(table) = g.checkpoints.get_mut(table_name) else {
                return false;
            };
            reset_invalid_statuses(table);
            true
        }

        pub fn destroy_error_checkpoints(
            &self,
            table_name: &str,
        ) -> Option<Vec<crate::DestroyedTableCheckpoint>> {
            let mut g = self.inner.lock().unwrap();
            if table_name != "all" && !g.checkpoints.contains_key(table_name) {
                return None;
            }
            let names: Vec<String> = g
                .checkpoints
                .iter()
                .filter(|(name, table)| {
                    table.Status <= crate::CheckpointStatusMaxInvalid
                        && (table_name == "all" || name.as_str() == table_name)
                })
                .map(|(name, _)| name.clone())
                .collect();
            let mut destroyed = Vec::with_capacity(names.len());
            for name in names {
                if let Some(table) = g.checkpoints.remove(&name) {
                    let min_engine_id = table.Engines.keys().copied().min().unwrap_or(0);
                    let max_engine_id = table.Engines.keys().copied().max().unwrap_or(-1);
                    destroyed.push(crate::DestroyedTableCheckpoint {
                        TableName: name,
                        MinEngineID: min_engine_id,
                        MaxEngineID: max_engine_id,
                    });
                }
            }
            Some(destroyed)
        }

        pub fn dump_tables_csv(&self) -> String {
            let g = self.inner.lock().unwrap();
            let task_id = g.task.as_ref().map(|task| task.TaskID).unwrap_or_default();
            let mut names: Vec<_> = g.checkpoints.keys().cloned().collect();
            names.sort();
            let mut output = String::from(
                "task_id,table_name,hash,status,create_time,update_time,auto_rand_base,auto_incr_base,auto_row_id_base\n",
            );
            for name in names {
                let table = &g.checkpoints[&name];
                output.push_str(&format!(
                    "{},{},0,{},{},{},{},{},{}\n",
                    task_id,
                    csv_field(&name),
                    table.Status,
                    "",
                    "",
                    table.AutoRandBase,
                    table.AutoIncrBase,
                    table.AutoRowIDBase,
                ));
            }
            output
        }

        pub fn dump_engines_csv(&self) -> String {
            let g = self.inner.lock().unwrap();
            let mut rows = Vec::new();
            for (table_name, table) in &g.checkpoints {
                for (engine_id, engine) in &table.Engines {
                    rows.push((table_name, *engine_id, engine.Status));
                }
            }
            rows.sort_by(|left, right| left.0.cmp(right.0).then(left.1.cmp(&right.1)));
            let mut output = String::from("table_name,engine_id,status,create_time,update_time\n");
            for (table_name, engine_id, status) in rows {
                output.push_str(&format!(
                    "{},{},{},,\n",
                    csv_field(table_name),
                    engine_id,
                    status
                ));
            }
            output
        }

        pub fn dump_chunks_csv(&self) -> String {
            let g = self.inner.lock().unwrap();
            let mut rows = Vec::new();
            for (table_name, table) in &g.checkpoints {
                for (engine_id, engine) in &table.Engines {
                    for chunk in &engine.Chunks {
                        rows.push((table_name, *engine_id, chunk));
                    }
                }
            }
            rows.sort_by(|left, right| {
                left.0
                    .cmp(right.0)
                    .then(left.1.cmp(&right.1))
                    .then(left.2.Key.Path.cmp(&right.2.Key.Path))
                    .then(left.2.Key.Offset.cmp(&right.2.Key.Offset))
            });
            let mut output = String::from(
                "table_name,path,offset,type,compression,sort_key,file_size,columns,pos,real_pos,end_offset,prev_rowid_max,rowid_max,kvc_bytes,kvc_kvs,kvc_checksum,create_time,update_time\n",
            );
            for (table_name, _, chunk) in rows {
                let columns = serde_json::to_string(&chunk.ColumnPermutation)
                    .unwrap_or_else(|_| "[]".to_string());
                output.push_str(&format!(
                    "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},,\n",
                    csv_field(table_name),
                    csv_field(&chunk.Key.Path),
                    chunk.Key.Offset,
                    chunk.FileMeta.Type.0,
                    chunk.FileMeta.Compression.0,
                    csv_field(&chunk.FileMeta.SortKey),
                    chunk.FileMeta.FileSize,
                    csv_field(&columns),
                    chunk.Chunk.Offset,
                    chunk.Chunk.RealOffset,
                    chunk.Chunk.EndOffset,
                    chunk.Chunk.PrevRowIDMax,
                    chunk.Chunk.RowIDMax,
                    chunk.Checksum.SumSize(),
                    chunk.Checksum.SumKVS(),
                    chunk.Checksum.Sum(),
                ));
            }
            output
        }
    }

    fn reset_invalid_statuses(table: &mut crate::TableCheckpoint) {
        if table.Status <= crate::CheckpointStatusMaxInvalid {
            table.Status = crate::CheckpointStatusLoaded;
        }
        for engine in table.Engines.values_mut() {
            if engine.Status <= crate::CheckpointStatusMaxInvalid {
                engine.Status = crate::CheckpointStatusLoaded;
            }
        }
    }

    fn csv_field(value: &str) -> String {
        if value.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", value.replace('"', "\"\""))
        } else {
            value.to_string()
        }
    }

    pub fn Open(_driver: &str, _dsn: &str) -> Result<DB> {
        Ok(DB::new_memory())
    }

    #[derive(Clone, Debug)]
    pub struct Tx {
        db: DB,
    }

    // 事务包装的作用是维持 Go 代码中的调用层级。
    // 即使当前提交没有复杂隔离语义，`Begin`/`Commit`/`PrepareContext` 这些入口仍需存在。

    impl Tx {
        pub fn Commit(&self) -> Result<()> {
            Ok(())
        }
        pub fn PrepareContext(&self, _ctx: super::context::Context, query: String) -> Result<Stmt> {
            Ok(Stmt {
                db: self.db.clone(),
                query,
            })
        }
        pub fn ExecContext(
            &self,
            _ctx: super::context::Context,
            query: &str,
            args: &[SqlValue],
        ) -> Result<ExecResult> {
            self.db.Exec(query, args)?;
            Ok(ExecResult { affected: 1 })
        }
        pub fn QueryContext(
            &self,
            _ctx: super::context::Context,
            _query: &str,
            _args: &[SqlValue],
        ) -> Result<Rows> {
            Ok(Rows { rows: vec![] })
        }
        pub fn QueryRowContext(
            &self,
            _ctx: super::context::Context,
            _query: &str,
            _args: &[SqlValue],
        ) -> Row {
            Row
        }
    }

    #[derive(Clone, Debug)]
    pub struct Stmt {
        db: DB,
        query: String,
    }

    // `Stmt` 把“先准备、再多次执行”的形状保留下来。
    // 这对批量写入 task/table/chunk 数据尤其重要，因为 Go 版正是沿这条路径落盘。

    impl Stmt {
        pub fn ExecContext(&self, _ctx: super::context::Context, args: &[SqlValue]) -> Result<()> {
            // REPLACE INTO task
            if self.query.to_ascii_lowercase().contains("task_v2")
                || self.query.to_ascii_lowercase().contains("(id, task_id")
            {
                if args.len() >= 9 {
                    let task = TaskCheckpointScan {
                        TaskID: args[0].as_i64(),
                        SourceDir: args[1].as_string(),
                        Backend: args[2].as_string(),
                        ImporterAddr: args[3].as_string(),
                        TiDBHost: args[4].as_string(),
                        TiDBPort: args[5].as_i64() as i32,
                        PdAddr: args[6].as_string(),
                        SortedKVDir: args[7].as_string(),
                        LightningVer: args[8].as_string(),
                    };
                    self.db.set_task(task);
                }
            }
            let _ = &self.query;
            Ok(())
        }
        pub fn Close(&self) {}
    }

    #[derive(Clone, Debug)]
    pub struct ExecResult {
        pub affected: i64,
    }

    impl ExecResult {
        pub fn RowsAffected(&self) -> Result<i64> {
            Ok(self.affected)
        }
    }

    #[derive(Clone, Debug)]
    pub struct Rows {
        rows: Vec<Vec<SqlValue>>,
    }

    impl Rows {
        pub fn Next(&mut self) -> bool {
            false
        }
        pub fn Scan(&mut self, _dest: &mut [SqlValue]) -> Result<()> {
            Ok(())
        }
        pub fn Err(&self) -> Result<()> {
            Ok(())
        }
        pub fn Close(&mut self) {}
    }

    #[derive(Clone, Debug)]
    pub struct Row;

    impl Row {
        pub fn Scan(&self, _dest: &mut [SqlValue]) -> Result<()> {
            Err(Error {
                msg: "sql: no rows in result set".into(),
                not_found: false,
                no_rows: true,
                class: None,
            })
        }
    }

    #[derive(Clone, Debug)]
    pub enum SqlValue {
        Null,
        I64(i64),
        U64(u64),
        Str(String),
        Bytes(Vec<u8>),
        Bool(bool),
    }

    impl SqlValue {
        pub fn as_i64(&self) -> i64 {
            match self {
                SqlValue::I64(v) => *v,
                SqlValue::U64(v) => *v as i64,
                _ => 0,
            }
        }
        pub fn as_string(&self) -> String {
            match self {
                SqlValue::Str(s) => s.clone(),
                SqlValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                _ => String::new(),
            }
        }
    }

    impl From<i64> for SqlValue {
        fn from(v: i64) -> Self {
            SqlValue::I64(v)
        }
    }
    impl From<i32> for SqlValue {
        fn from(v: i32) -> Self {
            SqlValue::I64(v as i64)
        }
    }
    impl From<u8> for SqlValue {
        fn from(v: u8) -> Self {
            SqlValue::I64(v as i64)
        }
    }
    impl From<String> for SqlValue {
        fn from(v: String) -> Self {
            SqlValue::Str(v)
        }
    }
    impl From<&str> for SqlValue {
        fn from(v: &str) -> Self {
            SqlValue::Str(v.to_string())
        }
    }
    impl From<&String> for SqlValue {
        fn from(v: &String) -> Self {
            SqlValue::Str(v.clone())
        }
    }
    impl From<Vec<u8>> for SqlValue {
        fn from(v: Vec<u8>) -> Self {
            SqlValue::Bytes(v)
        }
    }

    fn extract_backtick_ident(query: &str) -> Option<String> {
        let start = query.find('`')?;
        let rest = &query[start + 1..];
        let end = rest.find('`')?;
        Some(rest[..end].to_string())
    }

    pub fn is_no_rows(err: &Error) -> bool {
        err.no_rows || err.msg.contains("no rows")
    }
}

// ---------------------------------------------------------------------------
// storeapi / objstore
// ---------------------------------------------------------------------------
// 文件 checkpoint 后端除了 SQL，还依赖对象存储与本地文件抽象。
// `storeapi` 在这里定义统一 `Storage` trait，并提供内存版与本地磁盘版两种实现。
// `MemoryStorage` 主要服务测试：零环境依赖、可直接观察写入结果、便于断言文件是否存在。
// `LocalStorage` 则让少数需要真实临时目录行为的场景也能跑通。
// 两者都只实现 checkpoint 真正会调用的那组方法：存在性检查、读写、删除、重命名。
// 这保证 `FileCheckpointsDB` 的持久化语义能被覆盖，而不引入完整对象存储 SDK。
// `StorageHandle` 是类型擦除包装，模仿 Go 中接口值的使用体验。
// 它让上层无需关心当前拿到的是内存存储还是本地存储。
// 这对 parity 测试很重要，因为同一组逻辑会在不同底座上被驱动。
// `objstore` 则负责更上层的 URL 和外部存储创建语义。
// `RawURL` 保留 scheme、host、path、query 等组成部分，
// 让 checkpoint 文件路径能继续沿用 Go 中对象存储 URL 的拆装方式。
// `String` 和 `with_path` 体现的是“路径重组规则”而非“网络访问能力”。
// 路径编码里刻意保留 `/` 等字符，也是为了贴近 Go `url.URL` 在这些用例中的输出。
// 对 checkpoint 文件来说，URL 文本稳定与否直接影响路径拆分、重命名和最终持久化位置。
// 因此虽然这里看上去像工具函数，实则服务文件后端协议。
// 这一段也是典型的“形状兼容优先于能力完备”模块。
// 它不尝试提供真正的 S3/GCS/HDFS 客户端。
// 但对测试而言，只要能把路径和文件副作用表达出来，就足以支撑 checkpoint 行为验证。
// 注释特别说明这一点，避免未来有人依据这些桩误以为对象存储已经全量可用。
// 如果后续需要接真实对象存储，最先要回归的是 URL 拼装、文件存在性与 rename/delete 语义。
// 因为这些是当前文件后端最直接依赖的行为。
// 另外，内存与本地两种实现并存，也在提醒读者：
// checkpoint 对“存储介质”本身其实是抽象的，真正需要稳定的是文件协议与副作用。

pub mod storeapi {
    // `storeapi` 子模块可以看成文件 checkpoint 的持久化底板。
    // 它既要支持纯内存测试，也要支持真实本地文件副作用测试。
    // 统一 trait 的好处是：文件后端逻辑完全不用关心底层介质差异。
    // 只要这些方法语义稳定，checkpoint 文件协议就能在不同环境复用。
    use super::{Error, Result, context};
    use std::collections::HashMap;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug, Default)]
    pub struct Options {}

    pub trait Storage: Send + Sync {
        fn FileExists(&self, ctx: context::Context, name: &str) -> Result<bool>;
        fn ReadFile(&self, ctx: context::Context, name: &str) -> Result<Vec<u8>>;
        fn WriteFile(&self, ctx: context::Context, name: &str, data: Vec<u8>) -> Result<()>;
        fn DeleteFile(&self, ctx: context::Context, name: &str) -> Result<()>;
        fn Rename(&self, ctx: context::Context, old: &str, new: &str) -> Result<()>;
    }

    #[derive(Clone, Debug, Default)]
    pub struct MemoryStorage {
        files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    }

    impl MemoryStorage {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl Storage for MemoryStorage {
        fn FileExists(&self, _ctx: context::Context, name: &str) -> Result<bool> {
            Ok(self.files.lock().unwrap().contains_key(name))
        }
        fn ReadFile(&self, _ctx: context::Context, name: &str) -> Result<Vec<u8>> {
            self.files
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .ok_or_else(|| Error::new(format!("file not found: {name}")))
        }
        fn WriteFile(&self, _ctx: context::Context, name: &str, data: Vec<u8>) -> Result<()> {
            self.files.lock().unwrap().insert(name.to_string(), data);
            Ok(())
        }
        fn DeleteFile(&self, _ctx: context::Context, name: &str) -> Result<()> {
            self.files.lock().unwrap().remove(name);
            Ok(())
        }
        fn Rename(&self, _ctx: context::Context, old: &str, new: &str) -> Result<()> {
            let mut g = self.files.lock().unwrap();
            if let Some(data) = g.remove(old) {
                g.insert(new.to_string(), data);
            }
            Ok(())
        }
    }

    #[derive(Clone, Debug)]
    pub struct LocalStorage {
        root: PathBuf,
    }

    impl LocalStorage {
        pub fn new(root: impl Into<PathBuf>) -> Self {
            Self { root: root.into() }
        }
        fn full(&self, name: &str) -> PathBuf {
            if name.is_empty() {
                return self.root.clone();
            }
            // If root is a directory, join; if name is absolute-ish under root.
            let p = Path::new(name);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                self.root.join(name)
            }
        }
    }

    impl Storage for LocalStorage {
        fn FileExists(&self, _ctx: context::Context, name: &str) -> Result<bool> {
            Ok(self.full(name).exists())
        }
        fn ReadFile(&self, _ctx: context::Context, name: &str) -> Result<Vec<u8>> {
            fs::read(self.full(name)).map_err(|e| Error::new(e.to_string()))
        }
        fn WriteFile(&self, _ctx: context::Context, name: &str, data: Vec<u8>) -> Result<()> {
            let path = self.full(name);
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let mut f = fs::File::create(&path).map_err(|e| Error::new(e.to_string()))?;
            f.write_all(&data).map_err(|e| Error::new(e.to_string()))
        }
        fn DeleteFile(&self, _ctx: context::Context, name: &str) -> Result<()> {
            let path = self.full(name);
            if path.exists() {
                fs::remove_file(path).map_err(|e| Error::new(e.to_string()))?;
            }
            Ok(())
        }
        fn Rename(&self, _ctx: context::Context, old: &str, new: &str) -> Result<()> {
            let from = self.full(old);
            let to = self.full(new);
            if let Some(parent) = to.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::rename(from, to).map_err(|e| Error::new(e.to_string()))
        }
    }

    /// Type-erased storage handle used by FileCheckpointsDB.
    #[derive(Clone)]
    pub struct StorageHandle {
        inner: Arc<dyn Storage>,
    }

    // `StorageHandle` 模仿 Go 接口值的使用体验。
    // 上层拿到它之后只做转发，不再关心具体实现是内存还是本地磁盘。
    // 这让测试夹具可以按场景替换介质，但主实现代码保持不变。

    impl StorageHandle {
        pub fn from_storage(s: Arc<dyn Storage>) -> Self {
            Self { inner: s }
        }
        pub fn memory() -> Self {
            Self {
                inner: Arc::new(MemoryStorage::new()),
            }
        }
        pub fn local(root: impl Into<PathBuf>) -> Self {
            Self {
                inner: Arc::new(LocalStorage::new(root)),
            }
        }
        pub fn FileExists(&self, ctx: context::Context, name: &str) -> Result<bool> {
            self.inner.FileExists(ctx, name)
        }
        pub fn ReadFile(&self, ctx: context::Context, name: &str) -> Result<Vec<u8>> {
            self.inner.ReadFile(ctx, name)
        }
        pub fn WriteFile(&self, ctx: context::Context, name: &str, data: Vec<u8>) -> Result<()> {
            self.inner.WriteFile(ctx, name, data)
        }
        pub fn DeleteFile(&self, ctx: context::Context, name: &str) -> Result<()> {
            self.inner.DeleteFile(ctx, name)
        }
        pub fn Rename(&self, ctx: context::Context, old: &str, new: &str) -> Result<()> {
            self.inner.Rename(ctx, old, new)
        }
    }

    impl Default for StorageHandle {
        fn default() -> Self {
            Self::memory()
        }
    }

    impl fmt::Debug for StorageHandle {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "StorageHandle")
        }
    }

    use std::fmt;
}

pub mod objstore {
    // `objstore` 的重点不是联网，而是路径与 URL 语义。
    // checkpoint 文件名拼接、改名和还原路径时，都会经过这里的文本规则。
    // 因而这个模块更像“对象存储地址解释器”而非“对象存储客户端”。
    // 只有把这一点说清楚，后续维护者才不会误判能力边界。
    use super::storeapi::{Options, StorageHandle};
    use super::{Error, Result, context};
    use std::path::PathBuf;

    #[derive(Clone, Debug, Default)]
    pub struct RawURL {
        pub Scheme: String,
        pub Host: String,
        pub Path: String,
        pub RawQuery: String,
        pub Opaque: String,
        /// Original input after `+` → `%2B` substitution (for local abs path).
        pub raw: String,
    }

    impl RawURL {
        // `String` 负责把拆开的 URL 重新还原成 Go 侧熟悉的文本形式。
        // 这也是路径测试里为什么会直接比较字符串结果。
        pub fn String(&self) -> String {
            if self.Scheme.is_empty() {
                return self.Path.clone();
            }
            let mut out = String::new();
            out.push_str(&self.Scheme);
            out.push_str("://");
            if !self.Host.is_empty() {
                out.push_str(&self.Host);
            }
            // Encode path like Go url.URL: escape `?` etc. but keep `/`
            out.push_str(&encode_path(&self.Path));
            if !self.RawQuery.is_empty() {
                out.push('?');
                out.push_str(&self.RawQuery);
            }
            out
        }

        pub fn with_path(&self, path: String) -> Self {
            let mut u = self.clone();
            u.Path = path;
            u
        }
    }

    fn encode_path(path: &str) -> String {
        let mut out = String::new();
        for b in path.bytes() {
            match b {
                b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'-'
                | b'.'
                | b'_'
                | b'~'
                | b'/'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'@' => out.push(b as char),
                // Go PathEscape leaves some chars; `?` must be escaped
                _ => {
                    out.push('%');
                    out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
                    out.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
                }
            }
        }
        // uppercase hex like Go
        out
    }

    /// Match Go `objstore.ParseRawURL`: replace `+` with `%2B`, then parse like `net/url`.
    pub fn ParseRawURL(raw_url: &str) -> Result<RawURL> {
        let raw = raw_url.replace('+', "%2B");
        parse_go_url(&raw)
    }

    fn parse_go_url(raw: &str) -> Result<RawURL> {
        // Detect scheme: letters before ":"
        let mut scheme = String::new();
        let mut rest = raw;
        if let Some(idx) = raw.find(':') {
            let cand = &raw[..idx];
            if !cand.is_empty()
                && cand
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.')
                && cand.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
            {
                // For paths like `/a:b` we should NOT treat as scheme — Go requires
                // scheme to be followed by hierarchical or opaque. Bare "file:..." ok.
                // Local paths starting with `/` never have scheme.
                if !raw.starts_with('/') && !raw.starts_with('.') {
                    scheme = cand.to_ascii_lowercase();
                    rest = &raw[idx + 1..];
                }
            }
        }

        if scheme.is_empty() {
            return Ok(RawURL {
                Scheme: String::new(),
                Path: raw.to_string(),
                raw: raw.to_string(),
                ..Default::default()
            });
        }

        // strip //
        let mut host = String::new();
        let mut path = String::new();
        let mut raw_query = String::new();
        if let Some(stripped) = rest.strip_prefix("//") {
            // authority + path
            let (auth_path, query) = split_once_query(stripped);
            raw_query = query;
            if let Some(slash) = auth_path.find('/') {
                host = auth_path[..slash].to_string();
                path = auth_path[slash..].to_string();
            } else {
                host = auth_path.to_string();
                path = String::new();
            }
        } else {
            let (p, query) = split_once_query(rest);
            path = p.to_string();
            raw_query = query;
        }

        // Unescape path (%XX), matching Go
        path = percent_decode(&path);

        Ok(RawURL {
            Scheme: scheme,
            Host: host,
            Path: path,
            RawQuery: raw_query,
            Opaque: String::new(),
            raw: raw.to_string(),
        })
    }

    fn split_once_query(s: &str) -> (&str, String) {
        if let Some(i) = s.find('?') {
            (&s[..i], s[i + 1..].to_string())
        } else {
            (s, String::new())
        }
    }

    fn percent_decode(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let (Some(h), Some(l)) = (
                    (bytes[i + 1] as char).to_digit(16),
                    (bytes[i + 2] as char).to_digit(16),
                ) {
                    out.push(((h << 4) | l) as u8);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[derive(Clone, Debug)]
    pub enum Backend {
        Local { path: PathBuf },
        Remote { kind: String, url: String },
        Noop,
    }

    pub fn ParseBackend(raw_url: &str, _options: Option<&Options>) -> Result<Backend> {
        if raw_url.is_empty() {
            return Err(Error::new("empty store is not allowed"));
        }
        let u = ParseRawURL(raw_url)?;
        match u.Scheme.as_str() {
            "" => {
                let abs = std::fs::canonicalize(PathBuf::from(raw_url)).unwrap_or_else(|_| {
                    // if file doesn't exist yet, still build abs-like path
                    let p = PathBuf::from(raw_url);
                    if p.is_absolute() {
                        p
                    } else {
                        std::env::current_dir().unwrap_or_default().join(p)
                    }
                });
                // For separateCompletePath's newPath which may be "." or "tmp" dir:
                // use the path string as given when file may not exist.
                let path = if PathBuf::from(raw_url).exists() {
                    abs
                } else {
                    let p = PathBuf::from(raw_url);
                    if p.is_absolute() {
                        p
                    } else {
                        std::env::current_dir().unwrap_or_default().join(p)
                    }
                };
                Ok(Backend::Local { path })
            }
            "local" | "file" => Ok(Backend::Local {
                path: PathBuf::from(&u.Path),
            }),
            "noop" => Ok(Backend::Noop),
            "s3" | "ks3" | "oss" | "gcs" | "gs" | "azblob" | "hdfs" => Ok(Backend::Remote {
                kind: u.Scheme.clone(),
                url: raw_url.to_string(),
            }),
            other => Err(Error::new(format!("storage {other} not support yet"))),
        }
    }

    pub fn New(
        _ctx: context::Context,
        backend: Backend,
        _options: &Options,
    ) -> Result<StorageHandle> {
        match backend {
            Backend::Local { path } => {
                // If path is a file's directory, use that dir as root; file name handled separately.
                let root = if path.is_dir() || path.to_string_lossy().ends_with('/') {
                    path
                } else if let Some(parent) = path.parent() {
                    if parent.as_os_str().is_empty() {
                        PathBuf::from("/")
                    } else {
                        parent.to_path_buf()
                    }
                } else {
                    PathBuf::from(".")
                };
                let _ = std::fs::create_dir_all(&root);
                Ok(StorageHandle::local(root))
            }
            Backend::Noop => Ok(StorageHandle::memory()),
            Backend::Remote { .. } => {
                // Cloud mocked: in-memory store keyed per process.
                Ok(StorageHandle::memory())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// path helpers matching Go `path` package (slash-separated)
// ---------------------------------------------------------------------------
// `gopath` 模块复刻的是 Go `path` 包，而不是 OS 相关的 `filepath`。
// 这一区别对对象存储和 URL 路径尤为重要，因为它们统一使用 `/` 作为分隔符。
// `Base` 与 `Dir` 都需要保持 Go 的边界行为，
// 比如空串、根目录、尾部斜杠和 `..` 清理后的结果。
// checkpoint 路径拆分测试会直接依赖这些输出。
// 因此这里不能简单套用 Rust 本地路径 API，否则跨平台结果会发生偏移。
// 注释强调“slash-separated”就是为了提醒维护者这个模块面向逻辑路径而非文件系统语义。
// 后续若替换实现，也应继续用 Go `path` 规则校验而不是宿主机规则校验。
// 这正是 parity 测试里路径相关 case 存在的原因。
// 它们保护的不是性能，而是跨语言的一致性。

pub mod gopath {
    /// Go `path.Base`.
    pub fn Base(path: &str) -> String {
        if path.is_empty() {
            return ".".into();
        }
        let path = path.trim_end_matches('/');
        if path.is_empty() {
            return "/".into();
        }
        match path.rfind('/') {
            Some(i) => path[i + 1..].to_string(),
            None => path.to_string(),
        }
    }

    /// Go `path.Dir` (includes Clean).
    pub fn Dir(path: &str) -> String {
        if path.is_empty() {
            return ".".into();
        }
        let (dir, _) = split(path);
        clean(dir)
    }

    fn split(path: &str) -> (&str, &str) {
        match path.rfind('/') {
            Some(i) => (&path[..=i], &path[i + 1..]),
            None => ("", path),
        }
    }

    fn clean(path: &str) -> String {
        if path.is_empty() {
            return ".".into();
        }
        let rooted = path.starts_with('/');
        let mut out: Vec<&str> = Vec::new();
        for part in path.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." {
                if let Some(last) = out.last() {
                    if *last != ".." {
                        out.pop();
                        continue;
                    }
                }
                if !rooted {
                    out.push("..");
                }
                continue;
            }
            out.push(part);
        }
        let mut s = String::new();
        if rooted {
            s.push('/');
        }
        s.push_str(&out.join("/"));
        if s.is_empty() {
            ".".into()
        } else if s == "/" {
            s
        } else {
            s.trim_end_matches('/').to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// sqltocsv stub
// ---------------------------------------------------------------------------
// 最后的 `sqltocsv` 是一个非常薄的尾部适配桩。
// checkpoint 当前只需要它能把“有一组行结果”写出成某种可消费文本。
// 这里选择固定写入换行，表达“调用路径可达且 writer 会被触碰”。
// 这足以让上层在测试里验证导出调用发生，而不会把注意力拉到 CSV 格式细节。
// 换句话说，它保护的是接口连通性，不是格式正确性。
// 若未来某个测试开始真的关心 CSV 内容，再按那条真实需求补足即可。
// 在此之前，把它描述成占位器比假装完整实现更诚实也更安全。

pub mod sqltocsv {
    use super::sql::Rows;
    use super::{Error, Result};
    use std::io::Write;

    pub fn Write(mut writer: impl Write, mut _rows: Rows) -> Result<()> {
        writer
            .write_all(b"\n")
            .map_err(|e| Error::new(e.to_string()))?;
        Ok(())
    }
}
