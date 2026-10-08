# `pkg/sessionctx/variable/nextgen.rs`

## 文件定位

本文件属于 `astersql-sessionctx-variable` crate；crate 根 `pkg/sessionctx/variable/lib.rs` 以 `pub mod nextgen` 将它公开。它抽取了 next-gen 内核模式下三个受限系统变量的校验、回落值和精简会话写入行为：`tidb_pessimistic_transaction_fair_locking`、`tidb_dml_type` 与 `tidb_replica_read`。

当前接线需要特别区分：`nextgen` 模块是可被外部 crate 调用的公开 API，但全仓库 Rust 搜索到的直接使用者只有 `pkg/sessionctx/variable/nextgen_test.rs`。正常系统变量注册和会话路径由同 crate 的 `sysvar.rs`、`sysvar_builtins.rs`、`variable.rs` 等实现承担；因此不能把本文件描述成已经接管全部生产 `SET` 流程。

## 核心职责

1. `GetSysVar` 只为上述三个变量构造轻量 `SysVar` 句柄，形成明确白名单。
2. `SysVar::Validate` 模拟 Go 系统变量框架“先做类型规范化、再执行 next-gen 专用限制”的结果，并保留 Go `(val, err)` 的关键契约：拒绝时仍返回安全的规范化回落值。
3. `SysVar::SetSessionFromHook` 把已经规范化的值写入本文件定义的精简 `SessionVars`，用于验证 Go `SetSession` 钩子的状态变化。
4. `GlobalSystemVariableInitialValue` 以固定 next-gen 测试环境调用通用的 `GlobalSystemVariableInitialValueWithRuntime`，验证新安装时公平锁默认值被强制为 `OFF`。

这里不是完整系统变量框架：没有作用域、全局变量存储、权限检查、持久化、警告收集或真实生产 `SessionVars` 的全部字段。

## 主要符号

- `ReplicaRead`：副本读取模式枚举，覆盖 `Leader`、`PreferLeader`、`Follower`、混合读取、就近读取、自适应就近读取和 learner。它对应 Go `kv.ReplicaReadType` 在本文件所需的取值集合。
- `SessionVars`：仅保存 `PessimisticTransactionFairLocking`、`BulkDMLEnabled` 和私有 `replica_read` 的精简状态；`Default` 保证公平锁与 bulk DML 关闭、副本读为 leader。
- `SessionVars::GetReplicaRead`：只读暴露私有副本读状态。
- `NextGenError`：保存静态 `ErrorDescriptor` 和已格式化消息。`unsupported` 使用 `ErrNotSupportedInNextGen`（MySQL 错误码 1235），`wrong_value` 使用 `ErrWrongValueForVar`（错误码 1231）；公开访问器为 `descriptor` 和 `message`。
- `SysVar`：只保存静态变量名的轻量句柄；字段私有，合法实例只能通过 `GetSysVar` 获得。
- `GetSysVar(name: &str) -> Option<SysVar>`：大小写敏感地匹配 `vardef` 中的三个规范变量名，其他名称返回 `None`。
- `SysVar::Validate(&self, value: &str) -> (String, Option<NextGenError>)`：执行变量特定的规范化与 next-gen 限制。
- `SysVar::SetSessionFromHook(&self, vars: &mut SessionVars, value: &str) -> Result<(), String>`：模拟 Go `SetSession` 钩子。
- `GlobalSystemVariableInitialValue(var_name, var_value)`：构造 `store_is_tikv=false`、`in_test=true`、`next_gen=true` 的 `RuntimeEnvironment`，再委托 `sysvar.rs` 的通用实现。

## 执行流程

典型调用顺序由 `pkg/sessionctx/variable/nextgen_test.rs` 体现：调用 `GetSysVar` 获取句柄，调用 `Validate` 得到规范值和可选错误；即使存在 next-gen 拒绝错误，调用者仍可把返回的安全值交给 `SetSessionFromHook`，最终状态保持在支持范围内。

三个校验分支如下：

1. 公平锁先按布尔类型规则接受大小写不敏感的 `ON`/`OFF` 及数字 `1`/`0`。非法文本原样返回并附带 `ErrWrongValueForVar`；`ON`/`1` 在 next-gen 下返回 `OFF` 加 `ErrNotSupportedInNextGen`；`OFF`/`0` 成功归一为 `OFF`。
2. DML 类型是字符串类型，`Validate` 只禁止大小写不敏感的 `bulk`，并回落到 `vardef::DefTiDBDMLType`；其余字符串原样通过。随后 `SetSessionFromHook` 只接受 `standard` 或 `bulk`，所以未知字符串会在写入阶段失败。
3. 副本读按枚举类型接受七个大小写不敏感的文本值，也接受从 `0` 开始的枚举序号。非法值原样返回并报告 `ErrWrongValueForVar`；只有 `leader`（或序号 `0`）成功，其余已知模式均返回 `leader` 和 next-gen 不支持错误。

`SetSessionFromHook` 对公平锁和 DML 类型写入布尔字段；对副本读执行文本到 `ReplicaRead` 的映射。副本读空串被当作 leader，未知串遵循 Go 钩子行为：不修改已有状态并返回成功，因为正常框架应已先完成类型校验。

## 数据与状态

模块没有全局可变状态。`SysVar` 只含 `&'static str`，`NextGenError` 只含静态描述符引用和拥有所有权的消息字符串；二者不借用会话对象。

会话状态全部集中在调用者传入的 `&mut SessionVars`。`SetSessionFromHook` 的每个成功分支至多修改一个字段；DML 未知值在返回 `Err` 前不修改 `BulkDMLEnabled`，副本读未知值显式保持原状态。`Default` 给出 next-gen 安全起点，与三个限制的回落方向一致。

`GlobalSystemVariableInitialValue` 不读取进程全局配置，而是构造确定性的运行时快照；其中 `default_txn_assertion_level` 来自 `vardef::GetDefaultTxnAssertionLevel()`，实际变量覆盖规则则位于 `sysvar.rs::GlobalSystemVariableInitialValueWithRuntime`。

## 依赖与调用关系

上游关系：`pkg/sessionctx/variable/lib.rs` 公开该模块；`pkg/sessionctx/variable/nextgen_test.rs` 是目前唯一直接 Rust 调用者，覆盖 `GetSysVar`、`Validate`、`SetSessionFromHook`、`GetReplicaRead` 与 `GlobalSystemVariableInitialValue`。RustCodeGraph 符号查询确认了 `nextgen.rs::GetSysVar`（第 92 行）和 `nextgen.rs::GlobalSystemVariableInitialValue`（第 223 行）；全仓库 `rg` 未发现测试外的 `nextgen::...` 调用边。

下游关系：错误构造依赖 `crate::error::{ErrNotSupportedInNextGen, ErrWrongValueForVar, ErrorDescriptor}`；变量名称、默认值及 `ON`/`OFF` 常量依赖 `crate::vardef`；初始值入口调用 `crate::sysvar::GlobalSystemVariableInitialValueWithRuntime` 并传入 `RuntimeEnvironment`。`Cargo.toml` 声明本 crate 名为 `astersql-sessionctx-variable`，`vardef` 是路径依赖 `../vardef`；本文件其余依赖均为 crate 内模块，没有新增外部依赖或 feature 条件。

## 错误处理与边界

`Validate` 把“类型值非法”和“类型合法但 next-gen 不支持”分开：前者使用错误码 1231，后者使用错误码 1235。两者都返回一个值，调用者不能仅凭返回字符串判断成功，必须同时检查 `Option<NextGenError>`。

公平锁的非法字符串不会被静默视作开启或关闭。副本读只接受固定七项及序号 `0..=6`，超范围数字也属于错误值。DML 类型由于对齐 Go `TypeStr`，校验阶段允许未知文本，写入阶段才返回字符串错误 `unsupport DML type: <value>`（保留了 Go 当前拼写）。

`GetSysVar` 对未知名称返回 `None`。`SysVar` 字段私有，所以 `Validate` 与 `SetSessionFromHook` 中的兜底 `unreachable!` 依赖一个模块不变量：外部安全 Rust 代码无法构造含其他名称的 `SysVar`。副本读未知值写入时返回 `Ok(())` 且不改状态，这只有在调用者绕过或忽略校验时才会出现。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络资源。所有操作均为同步、局部且有界的字符串比较或字段赋值。

并发安全主要来自所有权边界：会话写入要求独占 `&mut SessionVars`；错误消息由 `String` 独立持有；错误描述符和变量名为只读静态引用。模块自身没有跨会话共享的可变状态，也没有清理阶段。调用者若跨线程共享会话，必须在模块外提供同步机制。

## 与 Go 版本的对应关系

Go 行为位于 `pkg/sessionctx/variable/sysvar.go` 的系统变量注册表，而不是单独的 `nextgen.go`；next-gen 专用回归测试位于带 `//go:build nextgen` 的 `pkg/sessionctx/variable/nextgen_test.go`。

- Go 公平锁定义先由 `TypeBool` 框架规范化，再在 `kerneltype.IsNextGen()` 且值为开启时返回 `OFF` 和 `ErrNotSupportedInNextGen`；Rust 显式实现布尔规范化，并增加非法值测试。
- Go `tidb_dml_type` 为 `TypeStr`，next-gen 只拒绝 `bulk`，`SetSession` 仅识别 `standard`/`bulk`；Rust保持相同的两阶段边界，并增加未知字符串写入失败测试。
- Go `tidb_replica_read` 为七项 `TypeEnum`，next-gen 只允许 leader；Go 类型框架负责文本/序号规范化，Rust 在本文件内显式实现这层规则，并增加序号 `0` 与非法值测试。
- Go `GlobalSystemVariableInitialValue` 根据真实全局配置和 `kerneltype.IsNextGen()` 分支；本文件固定构造 next-gen 测试环境后复用 Rust 通用函数，因此是确定性适配入口，不等同于生产环境自动探测入口 `sysvar.rs::GlobalSystemVariableInitialValue`。

Rust 的 `SessionVars` 和 `ReplicaRead` 是为这组语义提取的精简类型，不能与 Go 完整 `SessionVars`/`kv.ReplicaReadType` 或本 crate 的完整 `variable.rs::SessionVars` 视为类型级一一替代。

## 扩展指南

新增 next-gen 受限变量时，至少需要同步修改 `GetSysVar` 白名单、`SysVar::Validate` 和 `SysVar::SetSessionFromHook`；若引入新状态，再扩充精简 `SessionVars` 的字段、默认值与只读访问器。每个新增分支都应说明类型规范化发生在哪里、拒绝时返回什么安全值，以及写入失败是否保持原状态。

测试必须继续放在独立的 `pkg/sessionctx/variable/nextgen_test.rs`，不要内嵌进生产文件；同时核对 Go `sysvar.go` 对应注册项及 `nextgen_test.go` 的行为。需要覆盖合法值、大小写/数字别名、非法类型值、next-gen 拒绝描述符、回落值以及回落值写入后的状态。

若目标是接入真实 Rust 系统变量主链，还需在 `sysvar_builtins.rs`/`variable.rs` 的注册与真实 `SessionVars` 写入点做最小必要接线，并避免与本模块的轻量 `SysVar`、`SessionVars` 同名类型混淆。兼容风险集中在错误码、规范化次序和回落值；性能风险较低，但副本读枚举查找若扩展为大量值可改为无分配的直接匹配。任何行为改动都应保持 Rust 与 Go 测试意图一致。

## 验证依据

- 源码：`pkg/sessionctx/variable/nextgen.rs`，完整检查类型、函数、实现与所有分支。
- crate 边界：`pkg/sessionctx/variable/Cargo.toml` 与 `pkg/sessionctx/variable/lib.rs`；确认 crate 名、`vardef` 路径依赖、公开模块及独立测试挂载。
- Rust 测试：`pkg/sessionctx/variable/nextgen_test.rs`；覆盖三个变量、两类错误描述符、回落值、会话状态和 next-gen 初始值。
- Go 对照：`pkg/sessionctx/variable/sysvar.go` 中三个变量的 `Validation`/`SetSession` 及 `GlobalSystemVariableInitialValue`，以及 `pkg/sessionctx/variable/nextgen_test.go`。
- 错误定义：`pkg/sessionctx/variable/error.rs` 中 `ErrWrongValueForVar`（1231）和 `ErrNotSupportedInNextGen`（1235）。
- RustCodeGraph：`status` 显示目标仓库索引可用；`query GlobalSystemVariableInitialValue --kind function --json` 定位本文件第 223 行及其 `sysvar.rs`/Go 对照；`query GetSysVar --kind function --json` 定位本文件第 92 行。限定名 callers/callees 查询没有产出可用边，故直接调用关系以源码与全仓库 `rg` 交叉核验，并明确记录当前只有测试调用者。
- 未运行 Cargo：本任务仅新增说明文档，按任务约束以结构检查和人工事实复核替代代码构建/测试。
