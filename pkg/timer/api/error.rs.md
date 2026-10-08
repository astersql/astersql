# `pkg/timer/api/error.rs`

## 文件定位

本文件是 `astersql-timer-api` crate 的统一错误边界。crate 根模块 `pkg/timer/api/lib.rs` 通过 `pub mod error` 声明该模块，并用 `pub use error::*` 将错误类型、结果别名和兼容常量暴露到 crate 根；因此 API trait、内存存储、表存储、运行时 worker 以及 TTL timer 调用方都可以使用同一套错误协议。

crate 边界由 `pkg/timer/api/Cargo.toml` 确认：库入口为 `lib.rs`，Go 对照包为 `pkg/timer/api`，本文件直接依赖 `thiserror = "2"`。它不实现定时器业务操作，而是把业务失败分类成可比较、可显示、可传播的值。

## 核心职责

- `TimerError` 为 Timer API 定义四种可由程序分支判断的领域错误，以及一种保留具体诊断文本的通用错误。
- `TimerResult<T>` 统一所有 Timer API 的成功值和失败类型，供 `TimerStoreCore`、`TimerClient`、`Hook`、调度策略、SQL 编解码等接口复用。
- `ErrTimerNotExist`、`ErrTimerExists`、`ErrVersionNotMatch`、`ErrEventIDNotMatch` 保留 Go API 的公开名称，方便移植代码表达与 Go 版本对应。
- `TimerError::message` 把字符串或可转换为字符串的值收敛为 `Message(String)`，用于输入校验、解析、会话适配及底层错误加上下文。

## 主要符号

- `pub enum TimerError`：派生 `Clone`、`Debug`、`Eq`、`PartialEq` 和 `thiserror::Error`。`PartialEq/Eq` 使调用方能精确匹配分类错误；`thiserror::Error` 为每个变体生成 `Display` 与标准错误 trait 实现。
  - `TimerNotExist`：目标记录不存在，显示为 `timer not exist`。
  - `TimerExists`：同一命名空间下相同 key 已存在，显示为 `timer already exists`。
  - `VersionNotMatch`：乐观并发检查的期望版本与当前版本不一致，显示为 `timer version not match`。
  - `EventIdNotMatch`：关闭或更新事件时的事件 ID 与记录不一致，显示为 `timer event id not match`。
  - `Message(String)`：保存任意诊断文本，`Display` 原样输出内部字符串。
- `pub fn TimerError::message(message: impl Into<String>) -> Self`：接受 `&str`、`String` 等输入并构造 `Message`；除 `Into<String>` 转换外不分类、不包装来源错误。
- `pub type TimerResult<T> = Result<T, TimerError>`：crate 内公开接口的标准结果类型。
- 四个 `pub const Err...: TimerError`：分别映射到四个无载荷枚举变体。名称刻意沿用 Go 风格；`lib.rs` 的 `#![allow(non_upper_case_globals)]` 允许这些公开常量保持兼容命名。

## 执行流程

1. 业务入口以 `TimerResult<T>` 声明失败边界，例如 `TimerStoreCore` 的增删改查、`TimerClient` 操作以及 `Hook` 回调。
2. 存储或模型代码在已知领域条件下返回分类常量：`TimerStore::getOneRecord` 的空结果变成 `ErrTimerNotExist`；`MemoryStoreCore::Create` 的唯一键冲突变成 `ErrTimerExists`；`TimerUpdate::apply` 在 `CheckVersion` 或 `CheckEventID` 不匹配时分别返回版本或事件 ID 错误。
3. 无专用分类的失败通过 `TimerError::message` 生成 `Message(String)`。例如 `TimerRecord::Validate`、调度表达式解析、表存储 JSON 解码和 TTL timer 数据解析都沿此路径保留诊断文本。
4. `?` 将同一种 `TimerError` 沿 `TimerResult` 逐层传播；上层对分类错误执行控制流，对 `Message` 或其他未专门处理的错误通常继续返回或进入通用重试路径。
5. 运行时 worker 在 `pkg/timer/runtime/worker.rs` 中比较 `ErrVersionNotMatch` 与 `ErrTimerNotExist`：版本冲突触发重新读取最新记录，不存在则把缓存/元数据视为已删除；这说明分类值是运行协议的一部分，而非仅用于日志显示。

## 数据与状态

四个分类变体和四个对应常量都没有载荷，也不持有外部状态。`Message(String)` 独占一段字符串；克隆该错误会克隆字符串，比较则同时比较变体和完整消息内容。

错误值本身不记录 backtrace、错误源链、错误码或重试次数。版本、事件 ID、timer ID 等上下文也不存入分类变体；需要这些信息时，调用点必须在日志、上层状态或 `Message` 文本中保存。`TimerResult<T>` 只是类型别名，不改变 Rust `Result` 的内存或传播语义。

## 依赖与调用关系

- 下游直接依赖只有 `thiserror::Error` 派生宏和 Rust 标准库的 `Result`、`String`、`Into`。
- crate 内上游包括 `pkg/timer/api/store.rs`、`mem_store.rs`、`client.rs`、`timer.rs` 和 `hook.rs`；crate 根 `pkg/timer/api/lib.rs` 再导出本文件全部公开符号。
- 跨 crate 上游包括 `pkg/timer/tablestore/store.rs`、`sql.rs`、`notifier.rs`，以及 `pkg/timer/runtime/worker.rs`。`pkg/session/runtime/ttl_timer.rs` 也使用 `TimerError::message` 与 `TimerResult` 把 TTL hook 的解析和执行错误接入 Timer API。
- 关键调用边为：`TimerStore::{GetByID, GetByKey} -> getOneRecord -> ErrTimerNotExist`；`MemoryStoreCore::{Create, Update} -> ErrTimerExists/ErrTimerNotExist`；`TimerUpdate::apply -> ErrVersionNotMatch/ErrEventIDNotMatch`；`defaultTimerClient::ManualTriggerEvent -> TimerStore::Update`，并在版本错误上重试；runtime worker 根据不存在或版本冲突选择刷新、删除或重试响应。
- RustCodeGraph 对文件建立了索引并识别 `TimerError`，但 `message` 和通用 `Error` 名称在全仓库存在同名歧义，`callers/callees` 未返回可归属本文件的完整边；上述调用关系因此以索引的目标源码节点配合精确 `rg` 引用结果核实，而非据名称猜测。

## 错误处理与边界

分类错误必须保持稳定语义，因为调用方使用值相等判断决定行为。尤其不能把 `VersionNotMatch` 或 `TimerNotExist` 随意改成 `Message`：这会令客户端重试和 worker 元数据刷新分支失效。反过来，`Message` 没有机器可读类别，调用方不应依赖其文本做控制流。

`TimerError::message` 不自动调用来源错误的 `source()`，也不建立错误链；`map_err(TimerError::message)` 只在来源类型可直接 `Into<String>` 时适用，其他错误应先 `format!` 或 `to_string()`。分类常量是枚举值而非全局对象身份，判断依据是派生的值相等语义。

相关测试证明错误发生时写入保持原子语义：`pkg/timer/api/store_test.rs` 检查版本或事件 ID 不匹配后原记录不变；`pkg/timer/store_intergartion_test.rs` 检查失败更新后重新读取仍得到旧记录。错误文件本身没有独立的 `error_test.rs`，覆盖来自使用这些错误契约的独立测试文件。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、会话或事务，也没有析构逻辑；错误值是拥有所有数据的普通 Rust 值。并发含义来自使用方：`VersionNotMatch` 是乐观并发冲突信号，`EventIdNotMatch` 是防止旧事件关闭新事件的条件写护栏。

`MemoryStoreCore::Update` 在持锁期间读取记录、执行 `TimerUpdate::apply` 并写回；错误会在通知前返回。表存储在事务/会话范围内产生同样的分类错误。错误值离开这些资源作用域后仍可安全传播，因为不借用记录、锁或会话；本文件未显式承诺 `Send`/`Sync` trait，但所有变体只含 `String`，会获得相应自动 trait。

## 与 Go 版本的对应关系

Go 原型 `pkg/timer/api/error.go` 仅定义四个 `errors.New` 包级变量，错误文本与 Rust 四个无载荷变体逐一一致。Rust 的 `Err...` 常量保留 Go 名称，便于把 `errors.ErrorEqual`/哨兵错误判断迁移为 `PartialEq` 值比较。

Rust 额外引入了 `TimerError` 枚举、`TimerResult<T>` 和 `Message(String)`，把 Go 中可返回任意 `error` 的接口收窄成统一静态类型。Go 侧可能用 `errors.Trace` 保留包装关系；当前 Rust 类型没有等价的 source/backtrace 包装，因此只保证分类值和最终文本语义，不保证 Go 错误链结构。`pkg/timer/api/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/timer/api"` 明确记录了这一移植对应关系。

## 扩展指南

- 新增会驱动控制流的领域失败时，应添加新的无歧义枚举变体、稳定 `Display` 文本及 Go 兼容常量（若 Go API 有对应哨兵错误），并同步所有匹配分支。
- 新增纯诊断失败时优先在调用点用 `TimerError::message` 添加足够上下文；不要把可重试/不存在等机器可判定条件降级为文本。
- 若需要保留底层错误链，应显式设计带 `#[source]` 的变体并评估 `Clone/Eq/PartialEq` 契约是否仍可维持，不能只把任意动态错误塞入现有枚举。
- 最可能同步的测试包括 `pkg/timer/api/store_test.rs`、`client_test.rs`、`client_1_aster_unit_test.rs`、`pkg/timer/store_intergartion_test.rs` 与 `pkg/timer/runtime/worker_test.rs`；测试逻辑继续放在独立测试文件，不内嵌到 `error.rs`。
- 兼容性风险主要是错误文本、公开名称和值比较行为；性能风险很低，但高频路径构造 `Message(format!(...))` 会分配字符串，新增上下文时应避免无意义的重复格式化。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/timer/api` 确认目标、Go 对照和独立测试均在索引内。
- RustCodeGraph `node --file pkg/timer/api/error.rs` 与 `node TimerError`：核实 61 行源码、枚举变体、派生 trait、构造器、结果别名和四个常量；`node --file pkg/timer/api/error.go` 核实四个 Go 哨兵错误及文本。
- RustCodeGraph `query TimerError/getOneRecord/ManualTriggerEvent` 以及对 `store.rs`、`mem_store.rs`、`runtime/worker.rs`、`store_test.rs`、`client_test.rs`、`client_1_aster_unit_test.rs`、`store_intergartion_test.rs` 的文件节点读取：核实生产调用边、重试/刷新分支及错误后不修改记录的测试证据。
- 精确引用搜索覆盖 `pkg/timer` 与 `pkg/session/runtime/ttl_timer.rs`，用于补齐 RustCodeGraph 对同名 `message`、`Error` 的调用图歧义；未发现同名独立 `error_test.rs`。
- 配置与模块证据来自 `pkg/timer/api/Cargo.toml` 和 `pkg/timer/api/lib.rs`；Go 行为对照还参考 `pkg/timer/api/store.go`、`mem_store.go`、`client.go` 及对应 Go 测试引用。
- 本任务只新增文档，按计划不运行 Cargo；交付验证以固定章节结构、链接/路径真实性、直接证据复核和差异自检为准。
