# [`pkg/executor/show_ddl.rs`](show_ddl.rs)

## 文件定位

本文件属于 `astersql-executor` crate：`pkg/executor/Cargo.toml` 的 `[lib]` 将 `lib.rs` 设为 crate 根，`pkg/executor/lib.rs` 通过 `pub mod show_ddl;` 公开该模块。它描述 `ADMIN SHOW DDL` 的结果执行阶段：把构建阶段已经取得的 schema 版本、DDL Owner、本节点 ID 和当前作业快照写成一行六列结果。

当前 Rust 接线是不完整的。仓库内除模块声明和本文件自身外，没有代码构造或调用 `show_ddl::ShowDDLExec`；`pkg/executor/builder.rs::buildShowDDL` 只通过抽象的 `BuilderDependencies::build_show_ddl_executor` 请求外部依赖构造执行器，未直接绑定本文件的类型。因此，本文件是可复用的结果组装核心和运行时适配边界，不能据此声称 Rust SQL 主链已经实际执行它。

## 核心职责

- `DdlJobInfo` 和 `DdlInfo` 保存展示所需的最小 DDL 快照，不在执行时访问 DDL 元数据。
- `ShowDdlRuntime` 隔离具体系统上下文、Owner 地址查询和 `Chunk` 写入 API。
- `ShowDDLExec::Next` 在首次调用时生成一行六列：schema 版本、Owner ID、Owner 地址、作业描述、本节点 ID、原始 SQL；后续调用只重置结果块并返回空结果。
- 多个作业的描述与查询分别按原顺序用换行符连接，以匹配 `pkg/executor/show_ddl.go::ShowDDLExec.Next`。

本文件不负责查找 Owner、开启元数据事务或获取 DDL 作业快照。这些动作在 Go 版本的 `pkg/executor/builder.go::buildShowDDL` 中发生；Rust 构建层的对应抽象位于 `pkg/executor/builder.rs::buildShowDDL`。

## 主要符号

- `pub struct DdlJobInfo { description: String, query: String }`：一项作业的展示文本和原始 SQL。两个字段都是拥有所有权的字符串，执行器可在不借用外部作业对象的情况下持有快照。
- `pub struct DdlInfo { schema_version: i64, jobs: Vec<DdlJobInfo> }`：执行器所需的 schema 版本与有序作业集合。这里的 `DdlInfo` 与 `pkg/executor/builder.rs` 中同名的 `trait DdlInfo` 是不同类型，目前没有可见转换实现。
- `pub trait ShowDdlRuntime`：运行时适配接口。关联类型 `Context` 和 `Error` 由实现者决定；`reset_chunk`、`append_int64`、`append_string` 封装结果块操作，`server_address` 根据实例 ID 返回 `(IP, port)` 或错误。
- `pub struct ShowDDLExec<R: ShowDdlRuntime>`：持有运行时适配器、Owner ID、本节点 ID、DDL 快照和一次性游标 `done`。所有字段当前均公开，构造者必须保证它们彼此属于同一逻辑快照。
- `pub fn ShowDDLExec::Next(&mut self, context, request) -> Result<(), R::Error>`：本文件唯一行为入口。名称保留 Go 风格，因此模块用 `#![allow(non_snake_case)]` 局部关闭命名告警。

## 执行流程

1. 无论是否已经完成，`Next` 首先调用 `runtime.reset_chunk(request)`，确保调用者看不到上一次批次的数据。
2. 若 `done == true`，立即返回 `Ok(())`；此时结果块保持为空。
3. 遍历 `ddl_info.jobs`，分别抽取 `description` 与 `query`，各自用 `"\n"` 连接。空列表得到两个空字符串；单项不带尾随换行。
4. 调用 `runtime.server_address(context, &ddl_owner_id)` 解析 Owner 的 IP 和端口，再用 `format!("{ip}:{port}")` 形成第三列。IPv6 是否需要方括号由适配器/上层契约决定，本文件不做规范化。
5. 按固定列号写入：`0=schema_version`、`1=ddl_owner_id`、`2=address`、`3=ddl_jobs`、`4=self_id`、`5=queries`。
6. 所有列写入后设置 `done = true` 并返回成功；下一次调用走第 2 步，形成一次性结果集语义。

## 数据与状态

`ShowDDLExec` 是一个持有快照的有状态执行器。`ddl_info.jobs` 的向量顺序同时决定描述列和查询列的行内顺序，所以两个换行分隔列表具有相同的索引对应关系。执行过程中只读取 `ddl_info`、`ddl_owner_id` 和 `self_id`，唯一内部状态变更是成功末尾将 `done` 从 `false` 置为 `true`。

构造阶段应传入 `done = false`；本文件没有构造函数或状态校验，若传入 `true`，首次调用也不会产生行。`Clone/Eq/PartialEq` 只派生在两个快照数据结构上，执行器和运行时适配器没有复制或比较契约。

## 依赖与调用关系

直接编译依赖只有 `astersql_util_chunk::Chunk`，对应 `pkg/executor/Cargo.toml` 中指向 `../util/chunk` 的 `astersql-util-chunk` 路径依赖；Owner 查询等系统依赖被 `ShowDdlRuntime` 隔离，没有在本文件中直接依赖 domain/infosync 或 DDL crate。

已核验的上游关系如下：

- `pkg/executor/lib.rs` 导出 `show_ddl` 模块。
- RustCodeGraph 对 `pkg/executor/show_ddl.rs` 报告 10 个符号，但文件级 `used by 0 files`，对 `ShowDDLExec`/`Next` 未给出静态调用边。
- `pkg/executor/builder.rs::buildShowDDL` 获取 Owner ID、系统会话与新事务中的 DDL 信息，然后调用抽象的 `build_show_ddl_executor`；仓库搜索未找到把该抽象实现为本文件 `ShowDDLExec<R>` 的代码。

Go 的完整主链是 `pkg/executor/builder.go` 中计划分派 `*plannercore.ShowDDL -> buildShowDDL`，构造 `ShowDDLExec` 后由统一 `exec.Executor` 协议调用 `Next`。Rust 当前只能把这条链视为对照设计，而非已完成的调用证据。

## 错误处理与边界

本文件唯一可传播错误来自 `ShowDdlRuntime::server_address`。它在任何列写入之前执行；若失败，`Next` 以 `?` 原样返回 `R::Error`，`done` 仍为 `false`，调用者可在后续调用重试。结果块已经在入口被重置，因此按该接口契约不会留下本轮部分行。

`reset_chunk` 和两个 append 方法没有返回 `Result`，本层无法处理它们的失败；实现者必须把这些操作设计为不会失败，或在其内部采用自身的错误策略。空作业列表是正常情况，会写入空的描述和查询列。端口类型为 `u16`，格式化为无符号十进制。代码不验证空 Owner ID、空 IP、字符串大小、作业数或 schema 版本范围，这些都属于快照提供者和运行时适配器的输入责任。

## 并发与资源生命周期

`Next` 要求 `&mut self` 和 `&mut Context`，并修改 `done`，表达的是单执行器串行推进；`ShowDdlRuntime` 没有 `Send`/`Sync` 约束，本文件不保证跨线程共享安全。它不创建线程、任务、锁、通道或事务，也不持有外部会话句柄。

Owner 地址只在第一次尚未完成的 `Next` 中查询一次；成功后不会再次查询。DDL 作业和 schema 版本在执行器中以拥有所有权的数据保存，因此它们的生命周期与执行器一致。Go 构建器在 `Next` 之前开启系统会话读取快照并立即释放会话；Rust `builder.rs` 的抽象流程也先释放系统会话再请求构造执行器，说明执行阶段不应依赖仍然存活的事务或会话借用。

## 与 Go 版本的对应关系

`pkg/executor/show_ddl.go` 是直接语义来源。字段一一对应：`ddlOwnerID/selfID/ddlInfo/done` 对应 Rust 的 `ddl_owner_id/self_id/ddl_info/done`；Go 的 `ddl.Info.SchemaVer` 与 `Jobs` 被压缩为 Rust 的 `DdlInfo`，而 `job.String()` 与 `job.Query` 被预先表示为 `DdlJobInfo.description/query`。

两版 `Next` 都先清空 chunk、对完成状态返回空批次、以换行连接多个作业、查询 Owner 地址、按相同六列顺序写入并在末尾置 `done`。主要差异是 Go 直接调用 `infosync.GetServerInfoByID` 并实现 `exec.Executor`，Rust 通过泛型 trait 注入运行时能力，且目前没有实现统一 Rust executor trait 的证据。Go 的构建器拥有三秒 Owner 查询超时和事务快照获取逻辑；这些策略不在本文件内。

`pkg/executor/test/executor/executor_test.go::TestAdmin` 验证六列、schema 版本、Owner 地址、空作业描述以及第二次 `Next` 返回零行。`pkg/executor/test/executor/executor_test.rs` 保留了相同场景和断言形态，但内容仍使用 Go 风格测试 API，并且 `pkg/executor/lib.rs` 没有将它注册为本模块单测；因此它是迁移语义证据，不是本文件已执行的 Rust 回归测试证据。

## 扩展指南

- 增加或调整结果列时，应同时修改 `ShowDDLExec::Next` 的固定列映射、上游计划 schema/构造适配，以及 Go 对照实现；先确认列序和客户端兼容性。
- 改变作业展示格式时，应修改 `DdlJobInfo` 的生产端和连接逻辑，并覆盖零项、单项、多项及包含换行的输入。不要让描述和查询使用不同排序。
- 接通真实 Rust 主链时，需要为 `ShowDdlRuntime` 提供 Owner 地址与 `Chunk` 适配实现，并在 `BuilderDependencies::build_show_ddl_executor` 的实现中把 builder 的 `Box<dyn DdlInfo>` 转成此处的 `DdlInfo`；还应接入统一 executor trait，而不是绕过 builder 的三秒超时、系统会话释放和错误传递流程。
- 测试应放在独立文件（建议 `pkg/executor/show_ddl_test.rs`）并由 `pkg/executor/lib.rs` 的 `#[cfg(test)]` 模块声明接入，至少覆盖六列顺序、多作业换行、空作业、第二次调用为空、地址查询失败后 `done` 不变且可重试。不要把测试写回生产源文件。
- 性能风险集中在两个 `collect::<Vec<_>>().join("\n")` 的临时向量与结果字符串分配；若作业量可能很大，优化时仍须保持顺序、分隔符和错误前不写部分行的行为。

## 验证依据

- Rust 源码：`pkg/executor/show_ddl.rs`（`DdlJobInfo`、`DdlInfo`、`ShowDdlRuntime`、`ShowDDLExec::Next`）。
- crate 与模块：`pkg/executor/Cargo.toml` 的 `[lib]`、`astersql-util-chunk` 依赖；`pkg/executor/lib.rs` 的 `pub mod show_ddl` 及相邻测试模块声明。
- Rust 构建链：`pkg/executor/builder.rs::BuilderDependencies::{ddl_owner_id, ddl_info_with_new_transaction, build_show_ddl_executor}` 与 `ExecutorBuilder::buildShowDDL`。
- Go 对照：`pkg/executor/show_ddl.go::ShowDDLExec.Next`；`pkg/executor/builder.go::buildShowDDL` 及计划分派分支。
- 测试证据：`pkg/executor/test/executor/executor_test.go::TestAdmin` 的 show DDL 段；迁移参考 `pkg/executor/test/executor/executor_test.rs` 对应段。仓库内未发现 `show_ddl_test.rs` 或直接构造 Rust `ShowDDLExec` 的测试。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；目标文件被索引为 10 个符号，文件查询显示 `used by 0 files`，精确查询确认 Rust/Go 两个同名 `ShowDDLExec` 定义；对本文件未返回静态 caller/callee。
- 辅助搜索：`rg` 用于 Cargo、模块声明、未形成图边的 Rust 引用及测试路径核对。未运行 Cargo，符合本纯文档任务约束。
