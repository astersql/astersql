# `pkg/server/handler/tikvhandler/global_variables.rs`

## 文件定位

本文件属于 `astersql-server-handler-tikvhandler` crate（见同目录 `Cargo.toml`），为 NextGen status API 提供“全局系统变量诊断快照”的组装与脱敏逻辑。`lib.rs` 通过 `pub mod global_variables` 声明模块并以 `pub use global_variables::*` 导出公开函数；直接生产调用者是 `pkg/server/http_status.rs` 中的 `global_variables_response`。后者只在 `astersql_config_kerneltype::IsNextGen()` 为真时把 `/variables/global` 注册到路由，先从 `Domain::global_system_variables` 取得内存缓存快照，再调用本文件的 `global_variables`，最后序列化为 JSON 并设置 `Cache-Control: no-store`。

这里不是完整 HTTP handler，也不负责实时查询 `mysql.global_variables`。HTTP 方法检查、状态码、JSON 序列化和响应头均由 `pkg/server/http_status.rs` 处理；本文件只把调用者给出的覆盖值与系统变量注册表组合成适合诊断输出的 `BTreeMap<String, String>`。

## 核心职责

- `global_variables` 遍历 `astersql_sessionctx_variable::GetSysVars()` 返回的系统变量注册表快照，排除仅 session 作用域的变量，以及在 `vardef::EnableNoopVariables` 关闭时的 noop 变量。
- 它借助临时 `SessionVars` 和只读的 `SnapshotAccessor` 调用每个变量的 `SysVar::GetGlobalFromHook`，从而保留变量自定义 getter、宽松校验和普通全局值回退的统一语义。
- 它在最终输出边界执行防泄漏处理：`tidb_cloud_storage_uri` 对调用者提供的原始覆盖值调用 `astersql_parser_ast::misc::redact_url`，其他标为 `IsSensitive` 的非空值统一替换为 `vardef::MaskPwd`（当前为 `******`），空敏感值仍保持为空。
- 它使用 `BTreeMap` 作为结果，使最终键顺序稳定；注册表本身由 `GetSysVars` 以 `HashMap` 快照返回，不能依赖遍历顺序。

## 主要符号

- `struct SnapshotAccessor(BTreeMap<String, String>)`：文件私有的只读 `GlobalVarAccessor`。构造时持有调用者覆盖表的深拷贝，避免生成快照期间依赖外部映射的借用或后续变化。
- `SnapshotAccessor::unsupported() -> VariableError`：统一构造 `VariableErrorKind::InvalidValue`，消息为 `global-variable diagnostic accessor is read-only`，供所有写操作和 `mysql.tidb` 表读取入口拒绝非诊断用途。
- `impl GlobalVarAccessor for SnapshotAccessor`：`get_global_sys_var` 先用 ASCII 小写名称查询覆盖表，未命中时按同名键从 `GetSysVars()` 再取注册默认值 `SysVar::Value`，两者都不存在则返回 `VariableError::unknown(name)`；`set_global_sys_var_only`、`get_tidb_table_value` 和 `set_tidb_table_value` 均返回只读错误。
- `pub fn global_variables(overrides: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>, String>`：唯一公开业务入口。输入约定是以小写系统变量名为键的运行时缓存覆盖表；成功返回完成过滤和脱敏的快照，任一 getter 失败则返回带变量名上下文的字符串错误。

本文件没有模块级常量、trait 定义、条件编译项或异步函数。公开面只有 `global_variables`；`SnapshotAccessor` 及其实现均为模块内部细节。

## 执行流程

1. `global_variables` 创建空结果表，并以 `overrides.clone()` 构造 `SnapshotAccessor`，再传给 `SessionVars::new`。该构造函数会确保内置系统变量已经注册。
2. 对 `GetSysVars()` 的注册表快照逐项处理。若 `variable.Scope == vardef::ScopeSession`，该变量没有全局诊断意义，直接跳过；若变量是 noop 且全局原子开关 `EnableNoopVariables` 为假，也跳过。
3. 调用 `variable.GetGlobalFromHook(&Context, &mut session_vars)`。按 `pkg/sessionctx/variable/variable.rs` 的实现，有自定义 `GetGlobal` 时先运行 hook 并做全局作用域的宽松校验；`ScopeNone` 直接使用注册值；其余情况调用 `SnapshotAccessor::get_global_sys_var`。
4. 任何 getter 错误立即终止整个快照，通过 `map_err` 转成 `read <变量名>: <原错误>`，不会返回部分结果。
5. 若当前名称不区分大小写等于 `vardef::TiDBCloudStorageURI`，并且覆盖表中确有该名称的小写键，则刻意从覆盖表重新取得原值并交给 `redact_url`。这保留 URI 的非秘密结构，同时遮蔽 access key、token、endpoint 内凭据等敏感参数。
6. 否则，只要 `variable.IsSensitive` 且 getter 结果非空，就把整个值替换为 `vardef::MaskPwd`；空值保持为空，以表达“未配置”。
7. 使用注册定义中的 `variable.Name` 作为输出键插入 `BTreeMap`，全部变量处理成功后返回快照。

## 数据与状态

输入 `overrides` 是调用时刻的缓存快照。在当前生产链路中，`pkg/server/runtime.rs` 将 `Domain::global_system_variables()` 转发给 status server；`pkg/domain/domain.rs` 在读锁下克隆其 `global_system_variables`。本文件随后再次克隆该映射到 `SnapshotAccessor`，因此一次调用使用固定的覆盖值集合。

`SessionVars` 在函数栈内临时创建，仅用于满足系统变量 getter 的统一上下文；本文件不把它暴露或持久化。`GetSysVars()` 会在系统变量全局注册表读锁下深拷贝所有 `SysVar`，之后本文件遍历的是独立快照。唯一直接读取的可变全局状态是 `vardef::EnableNoopVariables` 原子值；如果该值在遍历期间并发变化，不同变量的可见性判断理论上可能观察到不同瞬间，本文件没有额外冻结该开关。

结果表保存拥有所有权的名称和值，且按键排序。覆盖表查询统一将请求名称转为 ASCII 小写，生产调用方及测试也使用规范化小写键；如果外部调用者传入非小写键，该条覆盖不会命中，这是当前接口的隐含输入约束。

## 依赖与调用关系

上游主链为：NextGen `build_status_router` 注册 `/variables/global` → `global_variables_response` 检查 GET → `Server::domain()` / `Domain::global_system_variables()` 取得缓存 → `astersql_server_handler_tikvhandler::global_variables` 生成快照 → `serde_json::to_string` 生成响应。RustCodeGraph 的文件节点同时报告本文件被 `pkg/session/runtime/control.rs` 使用；源码级生产入口搜索确认 HTTP 输出链路位于 `pkg/server/http_status.rs`，而本文件的函数级直接调用还包括同目录独立单元测试。

主要下游依赖如下：

- `astersql-sessionctx-variable`：提供 `Context`、`SessionVars`、`SysVar::GetGlobalFromHook`、系统变量注册表、访问器 trait 和错误类型；同目录 `Cargo.toml` 以本地路径 `../../../sessionctx/variable` 声明该依赖。
- `astersql-sessionctx-vardef`（由 variable crate 再导出的 `vardef`）：提供作用域、noop 开关、云存储变量名和密码掩码。
- `astersql-parser-ast`：仅在云存储 URI 特例中提供 `misc::redact_url`；`Cargo.toml` 指向 `../../../parser/ast`。
- 标准库 `BTreeMap`：承载输入快照和有序输出。

`SnapshotAccessor` 实现完整 `GlobalVarAccessor` 是因为变量 hook 共享统一的 `SessionVars` 接口，但诊断路径只支持全局变量读取。若某个 getter 尝试读取 `mysql.tidb` 表或执行写入，将由该访问器明确拒绝，而不是悄悄访问持久化层。

## 错误处理与边界

`get_global_sys_var` 的正常回退顺序是“运行时覆盖 → 注册默认值 → unknown variable 错误”。写操作、实例/global setter、`mysql.tidb` 表读写都不属于此诊断适配器能力；trait 的默认 setter 最终也会落到 `set_global_sys_var_only` 并得到只读错误。

`global_variables` 采用 fail-fast：任一 hook、宽松校验或访问器读取失败，整个函数返回 `Err(String)`，错误文本加入 `read <变量名>` 上下文。上层 `global_variables_response` 不把内部错误暴露给客户端，而统一返回 HTTP 500 `unable to read global variables`。本文件不处理序列化失败、非 GET 请求或非 NextGen 路由缺失，这些分别由 `pkg/server/http_status.rs` 处理。

安全边界上，普通敏感变量采用全值掩码，云存储 URI 则只在覆盖表存在原始配置时做结构化 URL 脱敏。如果该 URI 没有覆盖值，本文件沿用 getter/默认值结果，并不会进入显式 `redact_url` 分支；新增 URI 来源或改变键规范化规则时必须重新评估这一边界。当前 Rust 测试覆盖敏感值、空敏感值与云 URI 脱敏，但没有直接构造 getter 失败、未知变量访问、纯 session/noop 过滤或非小写覆盖键的单元测试。

## 并发与资源生命周期

本文件没有线程、任务、通道、事务、文件句柄、网络连接或数据库 session 生命周期。所有对象都在一次同步函数调用内创建并在返回时释放：覆盖表克隆、临时 `SessionVars`、注册表快照和结果表均为拥有所有权的数据。

共享状态访问由下游组件提供同步保证：`GetSysVars()` 在注册表 `RwLock` 下克隆，`EnableNoopVariables.Load()` 是原子读取，生产调用方的 `Domain::global_system_variables()` 在 `RwLock` 下克隆。因为 hook 是任意注册回调，它仍可能读取其他共享状态或失败；本文件不持有 Domain 锁执行 hook，也没有超时和取消机制。与 Go handler 的真实 session 生命周期不同，此 Rust 函数不会因客户端断开主动取消计算。

## 与 Go 版本的对应关系

Go 对照文件是同目录 `global_variables.go`。两版共同语义包括：遍历 `variable.GetSysVars()`；排除纯 session 变量；按 `EnableNoopVariables` 控制 noop 可见性；通过 `GetGlobalFromHook` 获取全局值；对非空 `IsSensitive` 值统一使用 `vardef.MaskPwd`；任一读取失败时放弃响应。Go 集成测试 `pkg/server/handler/tests/global_variables_test.go::TestGlobalVariables` 还验证 GET-only、`no-store`、当前全局值、noop 可见性、多类敏感变量、空值和多种云 URI 脱敏行为。

架构差异是 Rust 已将 HTTP 与变量快照拆开：Go 的 `GlobalVariablesHandler::ServeHTTP` 在请求内创建真实 `session.Session`，设置基于请求上下文的超时并在结束时关闭 session；Rust 的 `global_variables` 使用只读 `SnapshotAccessor` 和内存中的 Domain 覆盖快照，不访问存储，也没有超时上下文。Go 注释说明云 URI 的 getter 已完成 `ast.RedactURL`；Rust 对调用者提供的 `tidb_cloud_storage_uri` 覆盖值显式再次调用 `redact_url`。因此 Rust 当前是面向 NextGen 诊断端点的缓存快照实现，不能被描述为 Go handler 的完整存储/session 生命周期复刻。

## 扩展指南

- 新增输出过滤规则时，优先修改 `global_variables` 的注册表循环，并在独立的 `global_variables_test.rs` 增加对应测试；不要把测试嵌入生产源文件。需要同步检查 `pkg/server/http_status_test.rs::nextgen_global_variables_route_masks_diagnostic_secrets` 的端到端契约。
- 新增敏感变量通常应正确设置注册定义的 `IsSensitive`，让统一末端掩码自动生效；若值需要像 URI 一样保留非敏感结构，应增加经过审计的专用脱敏分支和多协议、大小写、畸形输入测试，避免输出原始凭据。
- 若要支持需要 `mysql.tidb`、存储或写操作的 getter，不能简单把 `SnapshotAccessor::unsupported` 改成成功桩。应先明确新的资源、超时、取消和错误传播契约，并评估是否应由真实 session 路径承载；否则会破坏当前“无 I/O、只读快照”的不变量。
- 若改变覆盖表键格式，应在入口统一规范化整个映射，或明确扩大 `get_global_sys_var` 与云 URI 特例的查找规则；两处必须保持一致，并测试混合大小写及重复规范化键的冲突策略。
- 性能上，每次请求当前至少克隆 Domain 覆盖表、`SnapshotAccessor` 映射和整个 `SysVar` 注册表。变量量级扩大或请求频率升高时，可评估减少克隆，但必须保证 hook 执行期间不持有全局锁，并保持单次快照一致性与稳定输出顺序。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/server/handler/tikvhandler/global_variables.rs` 确认文件含 10 个符号；`node --file ... --offset 1 --limit 260` 展示完整 94 行源码并报告文件级使用者 `pkg/session/runtime/control.rs`；`query global_variables`、`query SnapshotAccessor`、`query GlobalVarAccessor` 和 `query global_variables_response` 用于消歧主要符号。精确 `callers/callees global_variables` 因重名未返回可用函数边，因此生产调用边另外由源码搜索核验。
- 目标与模块边界：`pkg/server/handler/tikvhandler/global_variables.rs`、`pkg/server/handler/tikvhandler/lib.rs`、`pkg/server/handler/tikvhandler/Cargo.toml`。
- 直接入口和状态来源：`pkg/server/http_status.rs::global_variables_response`、`pkg/server/http_status.rs::build_status_router`、`pkg/server/server.rs::Domain::global_system_variables`、`pkg/server/runtime.rs` 的 Domain 转发实现、`pkg/domain/domain.rs::global_system_variables`。
- 下游语义：`pkg/sessionctx/variable/variable.rs::GlobalVarAccessor`、`SessionVars::new`、`SysVar::GetGlobalFromHook`、`GetSysVars`，以及 `pkg/sessionctx/vardef/sysvar.rs::MaskPwd`、`pkg/sessionctx/vardef/tidb_vars.rs::{TiDBCloudStorageURI, EnableNoopVariables, ScopeSession}`。
- Rust 测试：`pkg/server/handler/tikvhandler/global_variables_test.rs::{masks_sensitive_values_and_redacts_cloud_storage_credentials, preserves_empty_sensitive_values}`；HTTP 集成层证据为 `pkg/server/http_status_test.rs::nextgen_global_variables_route_masks_diagnostic_secrets`。
- Go 对照：`pkg/server/handler/tikvhandler/global_variables.go::{GlobalVariablesHandler, NewGlobalVariablesHandler, ServeHTTP}` 和 `pkg/server/handler/tests/global_variables_test.go::TestGlobalVariables`。本任务按计划为纯文档分析，未运行 Cargo 或代码测试；验收采用固定章节结构检查和上述源码/图证据人工复核。
