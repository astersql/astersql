# `pkg/sessiontxn/staleread/errors.rs`

## 文件定位

本文件是 `astersql-sessiontxn-staleread` crate 的本地错误模型，源文件为 [`errors.rs`](errors.rs)。crate 根 `lib.rs` 通过 `pub mod errors` 声明模块，并用 `pub use errors::*` 将 `ErrorKind` 和 `Error` 重导出。`Cargo.toml` 将 crate 命名为 `astersql-sessiontxn-staleread`，且所有业务依赖都放在 `target.'cfg(any())'` 下；因此当前错误文件自身只依赖 Rust 标准库 `std::fmt`，不会直接拉入其他 crate。

它不负责判定 stale read 是否合法；真正的判定与构造点在同 crate 的 `processor.rs`、`util.rs` 和 `provider.rs`。本文件仅把这些路径的失败统一成可分类、可显示、可作为 `std::error::Error` 传播的值。

## 核心职责

- 用 `ErrorKind` 把 stale read 失败分成 `AsOf`、`AlreadyEvaluated`、`Unsupported`、`InvalidTimestamp` 和 `Backend` 五类，供调用方做稳定的结构化判断。
- 用 `Error { kind, message }` 同时保留程序分类与面向人的诊断文本。
- 提供通用构造器 `Error::new` 和两个高频快捷构造器 `Error::as_of`、`Error::backend`。
- 实现 `fmt::Display` 和 `std::error::Error`，使错误可进入标准 Rust 错误传播链。

该设计是一个轻量边界：它不存储 SQL 错误码、堆栈、下游错误源或重试建议，也不会在显示时自动加入分类前缀。

## 主要符号

- `pub enum ErrorKind`：可 `Clone + Copy + Debug + PartialEq + Eq` 的错误分类。
  - `AsOf`：`AS OF TIMESTAMP` 表达式求值、解析、快照校验或 AS OF 使用约束失败。
  - `AlreadyEvaluated`：同一 `Processor` 或 prepared-statement 路径重复固化求值结果。
  - `Unsupported`：过期读 Provider 不支持的 ForUpdateTS、ForUpdate 快照或进入事务方式。
  - `InvalidTimestamp`：本地时间戳表示无法转换；当前明确构造点是 `util.rs::millis_to_tso` 的负毫秒输入。
  - `Backend`：会话锁中毒以及后端事务、快照或会话资源操作失败的本地包装分类。
- `pub struct Error`：包含公开字段 `kind: ErrorKind` 和 `message: String`；派生 `Clone + Debug + PartialEq + Eq`，因而可在缓存、测试与状态转移中复制和精确比较。
- `Error::new(kind, message)`：接受任意 `Into<String>` 消息，是所有构造路径的底层入口。
- `Error::as_of(message)` / `Error::backend(message)`：分别固定 `ErrorKind::AsOf` 和 `ErrorKind::Backend`，避免高频映射点重复写分类。
- `impl fmt::Display for Error`：只写出 `message`，不显示 `kind`。
- `impl std::error::Error for Error`：使用默认实现，没有覆盖 `source()`，因此本类型不保留可遍历的底层错误链。

文件中没有模块常量、trait 定义、条件编译项或内部非公开 API。

## 执行流程

1. 下游逻辑检测失败条件。例如 `processor.rs::set_evaluated_values` 发现已经求值，`util.rs::calculate_as_of_ts_expr` 发现 NULL/无法解析/过早 TSO，或 `provider.rs::stmt_for_update_ts` 被调用。
2. 调用点使用 `Error::new`、`Error::as_of` 或 `Error::backend` 生成错误。`message.into()` 在此将字面量、`String` 或其他可转换文本归一为自有 `String`。
3. 函数通过 `Result<_, Error>` 和 `?` 向上传播。本文件不改写、记录或重试错误。
4. 需要程序分支时，调用方读取 `error.kind`；需要用户可读文本时，`Display`/`to_string()` 返回原始 `message`。

一条具体主链是：`StaleReadProcessor::on_select_table` → `parse_and_validate_as_of` → `calculate_as_of_ts_expr` → 构造 `AsOf`/`Backend` 错误并逐层返回。Provider 链则由 `stmt_read_ts`、`on_initialize`、`snapshot_with_stmt_read_ts` 等入口产生 `Unsupported` 或 `Backend` 类别。

## 数据与状态

`ErrorKind` 是无负载的枚举，`Error` 是两字段自有值。构造后它不引用会话、事务、表达式或后端对象，因此错误的生命周期与原始资源解耦。

关键不变量是：`kind` 由构造时确定且不会在 `Display` 或传播中变化；`message` 是完整显示文本；两者均为公开字段，现有 API 未防止调用者在构造后直接改写。`Clone` 会复制 `String`；`Copy` 只适用于 `ErrorKind`，不适用于整个 `Error`。

## 依赖与调用关系

- 上游模块导出：`lib.rs` 公开声明并重导出本文件符号。
- 直接生产调用者：
  - `processor.rs`：构造 `AlreadyEvaluated`、`AsOf` 和会话锁相关 `Backend`，使用 `Error` 作为 `Processor` 路径的统一返回类型。
  - `util.rs`：在 AS OF 表达式、外部时间戳、会话锁和毫秒到 TSO 转换路径构造 `AsOf`、`Backend` 和 `InvalidTimestamp`。
  - `provider.rs`：在事务 Provider 不支持的操作中构造 `Unsupported`，在锁、激活与快照路径传播或构造 `Backend`。
- 下游依赖：只有 `std::fmt::Formatter`、`std::fmt::Result` 和 `std::error::Error` trait。`new` 依赖标准 `Into<String>` 转换。
- 测试调用者：`processor_test.rs`、`provider_test.rs`、`util_test.rs`、`externalts_test.rs` 直接检查 `error.kind`；`main_test.rs` 的 mock backend 使用 `as_of`/`backend` 产生故障。

RustCodeGraph 已索引该文件的 7 个符号；由于 `Error` 是仓库内高频同名符号，本文档的具体调用边以带路径的 RustCodeGraph 源码节点和限定在 `pkg/sessiontxn/staleread` 的引用检索交叉确认，不把其他 crate 的 `Error` 计入。

## 错误处理与边界

- `Display` 只返回消息；如果上层把错误转为字符串，`ErrorKind` 不会被编码进文本，结构化分类会丢失。
- `std::error::Error` 使用空的默认实现，没有 `source`；即使 `message` 来自后端错误，也只保留文本。
- `Error::as_of` 是语义分类器，不自行校验时间戳。例如“2013 年之前的 TSO”在 `calculate_as_of_ts_expr` 中被归类为 `AsOf`，而负毫秒在 `millis_to_tso` 中被归类为 `InvalidTimestamp`；不能仅按消息推断 kind。
- `Backend` 同时包含锁中毒和多种后端失败，当前粒度不能区分可重试性。`provider.rs::on_stmt_error_for_next_action` 对任意 `Error` 都返回 `(NoIdea, None)`，也没有由本类型自动产生重试策略。
- 公开字段和公开 `new` 允许任意 kind/message 组合，一致性依赖调用点约定，并非类型系统强制。

## 并发与资源生命周期

本文件不创建锁、任务、通道、线程、事务或 I/O 资源。`Error` 仅持有 `ErrorKind` 和自有 `String`，克隆后不共享可变状态，析构也不需要清理。

并发边界发生在调用者：`processor.rs`、`util.rs` 和 `provider.rs` 在 `SessionRef` 锁中毒时构造 `Error::backend("session lock poisoned")`，然后在锁守卫已经因错误返回而释放后传播该值。因此错误值不延长锁或事务借用的生命周期。

## 与 Go 版本的对应关系

Go 同路径 `errors.go` 只有版权头和 `package staleread`，没有对应的 `ErrorKind`/`Error` 声明。Rust 文件因此是为 Rust 静态错误边界新增的本地建模，不是 `errors.go` 的逐行翻译。

行为对应需要看 Go 的真实构造点：

- Rust `AsOf` 对应 Go `processor.go`/`util.go` 中的 `plannererrors.ErrAsOf.FastGenWithCause` 或 `GenWithStack`；Rust 保留了测试关心的消息和类别，但当前没有保留 TiDB SQL 错误码或堆栈。
- Rust `AlreadyEvaluated` 对应 Go `processor.go` 的 `errors.New("already evaluated")`。
- Rust `Unsupported` 对应 Go `processor.go`/`provider.go` 的通用 `errors.New("not supported"...)` 和 `errors.Errorf("Unsupported type: %v", tp)`。
- Rust `InvalidTimestamp` 是 Rust 毫秒转换边界的细化分类；Go 的过早 TSO 路径仍使用 `plannererrors.ErrAsOf`，不存在同名 kind。
- Rust `Backend` 是对 Go 中直接返回/`errors.Trace` 后端错误以及 Rust 特有锁中毒失败的统一包装。

因此扩展时应优先保持 Go 分支条件和消息语义，不应把 Rust 的五个 kind 误认为 Go 对外错误类型的一一复刻。

## 扩展指南

- 新增错误分类时，先确认现有 `AsOf`/`Unsupported`/`Backend` 是否已能表达分支；若确需新 variant，修改 `ErrorKind`，再更新所有匹配、测试断言和对外转换层。未备全的枚举会引入源码兼容性风险。
- 新增高频构造器时，放在 `impl Error` 中并始终调用 `Error::new`，避免 kind/message 映射在各调用点漂移。
- 若需保留底层错误链，需重新设计 `Error` 负载并实现 `source()`；这会影响当前 `Clone`/`Eq` 契约，不应只在单个调用点临时加字符串替代。
- 若需映射到 Go/TiDB 的 SQL 错误码，应在会话或协议边界明确增加转换，并与 `plannererrors.ErrAsOf` 校对；直接修改 `Display` 会改变所有现有消息，有兼容风险。
- 生产构造点的回归测试应继续放在独立文件：Processor 分支同步 `processor_test.rs`，Provider 不支持分支同步 `provider_test.rs`，时间戳转换同步 `util_test.rs`，外部时间戳同步 `externalts_test.rs`；不要把测试内嵌到 `errors.rs`。
- 性能方面，每次构造都可能分配 `String`；不要在成功热路径预构造错误。当前错误只在失败分支中构造，不影响正常 stale read 执行。

## 验证依据

- RustCodeGraph 索引状态：项目已索引，目标 `errors.rs` 显示 7 个符号；`node --file pkg/sessiontxn/staleread/errors.rs` 核对了 73 行完整定义。
- RustCodeGraph 源码节点：`processor.rs` 100–344 行附近证明 `AlreadyEvaluated`/`AsOf`/`Backend` 构造和传播链；`util.rs` 210–346 行附近证明 `AsOf`/`Backend`/`InvalidTimestamp` 构造点；`provider.rs` 90–324 行附近证明 `Unsupported`/`Backend` 和无重试建议的边界。
- crate 边界：`pkg/sessiontxn/staleread/Cargo.toml` 和 `lib.rs` 证明 crate 名称、Go 包对应、模块公开导出与条件化依赖。
- Go 对照：`errors.go` 为空声明；`processor.go`、`util.go`、`provider.go` 的 `ErrAsOf`、`errors.New/Errorf/Trace` 构造点证明 Rust 分类的行为来源和差异。
- 独立 Rust 测试：`processor_test.rs` 断言 `AsOf` 和 `AlreadyEvaluated`；`provider_test.rs` 断言 `Unsupported` 并覆盖语句错误动作；`util_test.rs` 和 `externalts_test.rs` 覆盖 AS OF/外部时间戳错误分类。`main_test.rs` 提供 mock 错误注入。
- Go 测试：`processor_test.go` 明确覆盖过小/为零 TSO 的“2013-01-01 之前”消息，用于校对 Rust AS OF 路径语义。
- 结构验证应使用任务指定的 `test -f` 与固定 11 标题计数命令。本任务是纯文档分析，不运行 Cargo。
