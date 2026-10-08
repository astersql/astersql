# `pkg/util/dbterror/terror.rs`

## 文件定位

本文件是 `astersql-util-dbterror` crate 的基础错误分类适配层。crate 入口 `pkg/util/dbterror/lib.rs` 通过 `#[path = "terror.rs"] mod terror_impl` 装入它并以 `pub use terror_impl::*` 公开全部 API；同一入口随后装入的 `ddl_terror.rs` 以及仓库中的 KV、Types、InfoSchema、Server、Store Driver 等模块都以这些 API 创建可复用的错误原型。

它位于业务子系统与 `astersql-parser-terror` 通用错误框架之间：不重新实现错误格式化、RFC code 或 MySQL 协议转换，只包装底层 `terror::ErrClass`，补齐 Go `pkg/util/dbterror/terror.go` 的错误类门面，并从 `astersql-errno` 的标准消息表选择模板。`pkg/util/dbterror/Cargo.toml` 表明该 crate 只有 `astersql-errno` 和 `astersql-parser-terror` 两个运行时依赖，且没有 feature 或条件依赖。

## 核心职责

1. `ErrClass` 把 parser terror 的类别编号包装为 dbterror 层类型，使调用方以 `dbterror::ClassKV`、`dbterror::ClassTypes` 等稳定名称表达错误所属子系统。
2. 22 个公开静态错误类逐一转发到底层同名 `terror::Class*`，保持 Go 包中类别集合及顺序；本文件不新增类别编号。
3. `IntoErrCode` 同时接受 errno 常量常用的 `u16` 和已经构造好的 `terror::ErrCode`，让迁移后的调用方无需重复写数值包装。
4. `ErrClass::NewStd` 从 `errname::MySQLErrName` 按错误码取得标准 `ErrMessage`，再经 `NewStdErr` 创建带错误类别、MySQL code、RFC code 和脱敏参数位置的 `terror::Error` 原型。
5. `ErrClass::NewStdErr` 为需要自定义消息或“错误码与消息模板不是同一个 errno”的调用方保留直接入口；实际注册和规范化仍由底层 parser terror 完成。

## 主要符号

- `pub struct ErrClass { pub inner: terror::ErrClass }`：Go `type ErrClass struct{ terror.ErrClass }` 的显式字段版本。`inner` 公开，使迁移代码和测试能直接构造包装值；方法调用则委托给它。
- `pub trait IntoErrCode`：仅定义 `fn into_err_code(self) -> terror::ErrCode`。`u16` 实现用 `terror::ErrCode(self as isize)` 扩宽，`terror::ErrCode` 实现保持原值，因而 `NewStdErr` 不会先把已有的宽错误码压窄。
- `ClassAutoid`、`ClassDDL`、`ClassDomain`、`ClassExecutor`、`ClassExpression`、`ClassAdmin`、`ClassKV`、`ClassMeta`、`ClassOptimizer`、`ClassPrivilege`、`ClassSchema`、`ClassServer`、`ClassStructure`、`ClassVariable`、`ClassXEval`、`ClassTable`、`ClassTypes`、`ClassJSON`、`ClassTiKV`、`ClassSession`、`ClassPlugin`、`ClassUtil`：不可变的公开 `static ErrClass`，各自包装 `astersql_parser_terror` 中相应的类别常量。
- `ErrClass::NewStd(&self, code: impl IntoErrCode) -> Box<terror::Error>`：标准模板入口。它保留完整 `ErrCode` 用于后续注册，但使用 `code.0 as u16` 查询 `astersql_errno::errname::MySQLErrName`。
- `ErrClass::NewStdErr(&self, code: impl IntoErrCode, message: &terror::parser::mysql::errname::ErrMessage) -> Box<terror::Error>`：显式模板入口，调用 `self.inner.NewStdErr(...)`。底层实现位于 `pkg/parser/terror/terror.rs`。
- 本文件没有宏、枚举、异步函数或条件编译项。

## 执行流程

标准错误原型的主流程如下：

1. 调用方选择业务类别，例如 `pkg/kv/error.rs` 选择 `ClassKV`，并把 errno 常量传给 `NewStd`。
2. `IntoErrCode` 把 `u16` 常量扩宽为 `terror::ErrCode`；若调用方已经传入 `ErrCode`，数值原样保留。
3. `NewStd` 把该数值转换为 `u16`，在 `astersql_errno::errname::MySQLErrName` 中按键索引标准消息；索引成功后把原始宽 `ErrCode` 与 `ErrMessage` 传给 `NewStdErr`。
4. 包装层 `NewStdErr` 委托给 `inner.NewStdErr`。根据 `pkg/parser/terror/terror.rs` 的实现，底层先通过 `initError` 把类别/错误码写入全局注册表并生成 `<class>:<code>` RFC code，再用消息原文、脱敏参数位置、MySQL code 和 RFC code 调用 `errors::Normalize`。
5. 返回值是盒装错误原型。调用方通常把它放入 `LazyLock`，随后通过底层错误 API 的 `GenWithStackByArgs` 或 `FastGenByArgs` 绑定参数，生成可传播的共享错误。

自定义消息流程只跳过第 3 步的标准表查询。例如 `pkg/kv/error.rs` 先在标准消息后附加事务重试标记，再调用 `ClassKV.NewStdErr`；`pkg/types/errors.rs` 也用该入口组合指定错误码与另一条标准消息模板。

## 数据与状态

本文件自身的数据均为零堆分配的类别门面：22 个 `static ErrClass` 只持有一个底层类别值；`IntoErrCode` 的转换也不保存状态。每次构造成功后唯一直接拥有的新资源是返回的 `Box<terror::Error>`。

真正的可变全局状态在 `astersql-parser-terror`：`ErrClassToMySQLCodes` 记录类别到错误码集合，`rfcCode2errClass` 记录 RFC 前缀到类别，`registerFinish` 控制注册冻结。底层 `ErrClass::NewStdErr` 每次都经 `initError` 更新前两张表；因此这些构造 API 虽然在 Rust 签名上只借用 `&self`，却不是纯函数。

`NewStd` 有两个不同宽度语义：注册阶段沿用 `terror::ErrCode(isize)`，标准消息查表阶段把数值转换为 `u16`。这兼容 MySQL errno 表的键类型，但新增非 `u16` 范围错误码时不能假设标准查表仍有唯一、有效的结果；此类需求应先核对 errno 表或改用有明确模板的 `NewStdErr`。

## 依赖与调用关系

- 上游装配：`pkg/util/dbterror/lib.rs` 私有装入本文件并公开再导出；同文件的 `dbterror`、`errno`、`errors`、`terror` 子模块提供兼容命名空间。
- crate 边界：`pkg/util/dbterror/Cargo.toml` 声明库入口为 `lib.rs`，运行时仅依赖 `astersql-errno` 与 `astersql-parser-terror`，测试另依赖 `astersql-testkit-testsetup`。
- 直接下游：`NewStd` 读取 `astersql_errno::errname::MySQLErrName`；`NewStdErr` 调用 `astersql_parser_terror::ErrClass::NewStdErr`。后者负责注册、RFC code、MySQL code 与脱敏元数据，错误参数展开由返回原型上的 parser terror 方法完成。
- crate 内消费者：`pkg/util/dbterror/ddl_terror.rs` 大量用 `ClassDDL.NewStd(...)` 声明 DDL 错误原型。
- 跨 crate 消费者：`pkg/kv/error.rs` 使用 `ClassKV`/`ClassTiKV`，`pkg/types/errors.rs` 使用 `ClassTypes`，`pkg/tablecodec/tablecodec.rs` 使用 `ClassXEval`，`pkg/infoschema/error.rs` 使用 `ClassSchema`/`ClassExecutor`，`pkg/store/driver/error/error.rs` 使用 `ClassTiKV`。各调用方通常在自己的 `Cargo.toml` 中以路径依赖引入 `astersql-util-dbterror`。
- RustCodeGraph 对文件给出的装配边是 `pkg/util/dbterror/lib.rs -> pkg/util/dbterror/terror.rs`；其同名方法级 callers/callees 查询存在歧义，因此上述消费者又以精确的 `dbterror::Class*.NewStd*` 源码搜索核验，而不是把宽泛图结果当作完整调用清单。

## 错误处理与边界

- `NewStd` 通过 `MySQLErrName[&mysql_code]` 索引消息表。错误码没有对应标准模板时会 panic，而不是返回 `Result`；调用方必须使用已登记的 errno，或明确传入模板调用 `NewStdErr`。
- 底层 `initError` 在 `RegisterFinish` 冻结注册后会打印 backtrace 并 panic。因此新原型应在 crate/进程初始化期或现有 `LazyLock` 初始化约束内建立，不能把 `NewStd*` 当成请求热路径上的任意错误构造器。
- 底层锁中毒同样通过 `expect` 变为 panic。此层不捕获、包装或降级这些初始化一致性错误。
- `NewStdErr` 接受任意实现 `IntoErrCode` 的值和借用消息；消息在底层规范化时被复制进错误对象，不在本层保存借用。
- 本层不检查消息占位符数量。参数不足、脱敏位置和格式化结果由 parser terror 的错误生成逻辑决定；相关行为由独立测试覆盖。
- 选择错误的 `Class*` 不会在本层报错，却会改变 RFC code 前缀、类别比较和协议侧诊断语义，属于兼容性问题。

## 并发与资源生命周期

`ErrClass` 静态值只读，可被所有线程共享；本文件没有锁、任务、通道、事务、I/O 或显式清理逻辑。`Box<terror::Error>` 的生命周期由拥有它的调用方管理，静态错误原型通常置于 `LazyLock`，进程结束时统一释放。

并发风险来自下游注册副作用。parser terror 使用 `RwLock<HashMap<...>>` 保护注册表并用 `AtomicU32` 的 Release/Acquire 标记冻结，因此内存访问有同步保护；但 Go 源文件对 `NewStd` 的契约明确写着“not goroutine-safe”并建议仅用于全局变量初始化，Rust 文件也保留该注意事项。安全扩展时应遵守更强的语义约束：在注册冻结前完成一次性原型创建，不在并发请求处理中动态注册。测试 `pkg/util/dbterror/terror_test.rs` 修改 parser terror 的全局脱敏模式时使用 `RedactModeGuard` 在 `Drop` 中恢复原值，说明测试也必须隔离全局状态。

## 与 Go 版本的对应关系

`pkg/util/dbterror/terror.go` 是直接对照：Go `ErrClass` 匿名嵌入 `terror.ErrClass`，Rust 用公开 `inner` 字段显式组合；22 个 Go 包变量与 Rust 静态值逐项对应；Go `NewStd(code terror.ErrCode)` 与 Rust `NewStd(code: impl IntoErrCode)` 都从 `errno.MySQLErrName[uint16(code)]` 选模板并委托 `NewStdErr`。

Rust 的有意适配有三点：

1. 通过 `IntoErrCode` 同时接收迁移后 errno crate 的 `u16` 常量和 parser terror 的 `ErrCode`，而 Go 依靠统一的命名类型。
2. `NewStdErr` 在 Rust 包装层显式声明，以替代 Go 匿名嵌入带来的方法提升；其底层语义仍由 parser terror 实现。
3. Go 包变量可直接调用构造器，Rust 消费者多用 `LazyLock` 延迟执行非 `const` 构造与注册。

测试意图也保持对应。Go `pkg/util/dbterror/terror_test.go::TestErrorRedact` 在 Enable/Marker 两种模式下覆盖标准模板、敏感参数和 `GenWithStackByArgs`/`FastGenByArgs`；Rust `pkg/util/dbterror/terror_test.rs::test_error_redact` 使用同一组 19 个 errno 场景，并进一步逐条断言完整消息字符串。Rust 测试位于独立文件并由 `lib.rs` 的 `#[cfg(test)] mod terror_test` 接入。

## 扩展指南

- 新增错误类别门面时，先在 `astersql-parser-terror` 定义并注册真实类别，再在本文件按 Go 对照位置增加对应 `static ErrClass`；不要在 dbterror 层发明与底层无关的编号。若 Go 同路径文件也有该增量，应保持名称、顺序与类别语义一致。
- 新增标准错误原型时，优先在所属业务模块用既有 `Class*.NewStd(errno)` 和 `LazyLock` 声明；只有需要改写文案或复用另一模板时才用 `NewStdErr`。不要把业务错误清单堆入本基础门面。
- 新 errno 必须确认 `astersql-errno::errname::MySQLErrName` 有键，并核对 `RedactArgPos`；否则 `NewStd` 会 panic，或敏感参数会错误暴露。
- 修改 `IntoErrCode` 或数值转换时，应保留 `ErrCode` 的完整宽度直到注册，只在标准 MySQL 消息查表边界转换为 `u16`，并明确评估越界/截断兼容风险。
- 同步测试放在独立的 `pkg/util/dbterror/terror_test.rs`，不要内嵌到生产文件。至少覆盖新类型转换、缺失模板/冻结注册等边界（若可在隔离进程安全测试），以及 Enable/Marker 脱敏模式；Go 行为发生变化时也要对照 `terror_test.go`。
- 兼容性风险主要是错误类/RFC 前缀、MySQL code、消息模板与脱敏位置变化；性能风险主要来自把注册构造移入热路径造成锁写入和堆分配。本文件没有异步或 I/O 性能面。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录的 Rust/Go 源与测试均已收录。
- RustCodeGraph `files --filter pkg/util/dbterror`：确认 `terror.rs`、`lib.rs`、`ddl_terror.rs`、Rust/Go 对照测试和相关子 crate 的文件边界。
- RustCodeGraph `node --file`：完整读取 `pkg/util/dbterror/terror.rs`（1–164）、`pkg/util/dbterror/lib.rs`（1–63）、`pkg/util/dbterror/terror_test.rs`（1–269）、`pkg/util/dbterror/terror.go`（1–57）、`pkg/util/dbterror/terror_test.go`（1–95），以及直接下游 `pkg/parser/terror/terror.rs` 的类别、注册和 `NewStdErr` 实现。
- RustCodeGraph/源码调用证据：`pkg/util/dbterror/ddl_terror.rs`、`pkg/kv/error.rs`、`pkg/types/errors.rs`，并以 `rg` 精确核对 `pkg/tablecodec/tablecodec.rs`、`pkg/infoschema/error.rs`、`pkg/store/driver/error/error.rs` 等实际消费者。图的同名方法查询结果过宽，未把它用于声称完整调用覆盖。
- Cargo 证据：`pkg/util/dbterror/Cargo.toml`；另以消费者 Cargo manifest 的 `astersql-util-dbterror` 路径依赖核对跨 crate 接线。
- 测试证据：`pkg/util/dbterror/terror_test.rs::test_error_redact` 与 Go `pkg/util/dbterror/terror_test.go::TestErrorRedact`。本任务是纯文档分析，按计划未运行 Cargo 或代码测试。
- 人工复核结论：该文件存在是为了保留 Go dbterror 的错误类门面并集中桥接标准 errno 消息；运行时沿“类别选择 → 错误码转换 → 标准/显式模板 → parser terror 注册与规范化 → 参数化错误生成”工作；扩展必须保持类别、码、模板、脱敏信息和初始化时序一致。
