# `pkg/kv/txn_scope_var.rs`

## 文件定位

本文件属于 `astersql-kv` crate：`pkg/kv/Cargo.toml` 将 `lib.rs` 设为 crate 根，`pkg/kv/lib.rs:547-550` 再以 `txn_scope_var_impl` 私有模块装入本文件并公开重导出其符号。它把会话变量 `@@txn_scope` 的外部取值与向 PD/TSO 请求时使用的真实作用域封装在一起，是配置层与事务时间戳调用层之间的值对象。

当前 Rust 接线边界需要特别说明：全仓 Rust 引用搜索只发现 `pkg/kv/mpp_2_aster_unit_test.rs:174-187` 直接使用这些 API，尚未发现生产 Rust 调用者。完整应用中对应的生产接线仍可在 Go 的 `pkg/sessionctx/variable/session.go:2140-2148,2472,2593` 看到。因此，本文件已经提供并测试了 Rust API，但不能据此声称 Rust 会话主链已经使用它。

## 核心职责

- `TxnScopeVar` 同时保存两种语义不同的字符串：用户可见的 `global`/`local`，以及服务端真正传给时间戳服务的 scope（`global` 或具体 zone 标签）。这种分离避免把部署区域标签直接当作 `@@txn_scope` 的显示值（`TxnScopeVar`，`pkg/kv/txn_scope_var.rs:27-32`）。
- `NewDefaultTxnScopeVar` 从配置 crate 读取当前事务 scope，并把任何不等于 `global` 的配置值映射为“显示为 `local`、实际值保留原字符串”的对象（`pkg/kv/txn_scope_var.rs:37-43`）。
- 全局与本地构造器建立两种规范对象；只读 getter 分别暴露显示值和真实值（`pkg/kv/txn_scope_var.rs:47-69`）。
- `GlobalTxnScope` 与 `LocalTxnScope` 给调用者提供稳定的协议字符串（`pkg/kv/txn_scope_var.rs:80-84`）。

## 主要符号

- `pub struct TxnScopeVar { varValue: String, txnScope: String }`：字段私有，调用者只能通过构造函数创建、通过 getter 读取。`varValue` 表示 `@@txn_scope`，`txnScope` 表示真实 TSO scope。
- `pub fn NewDefaultTxnScopeVar() -> TxnScopeVar`：调用 `config::GetTxnScopeFromConfig()`；仅当返回值严格等于 `GlobalTxnScope` 时走全局分支，否则把返回值作为本地区域 scope。
- `pub fn NewGlobalTxnScopeVar() -> TxnScopeVar`：令两个字段都为 `"global"`。
- `pub fn NewLocalTxnScopeVar(txnScope: String) -> TxnScopeVar`：令显示字段为 `"local"`，并把入参所有权直接保存为真实 scope。
- `fn newTxnScopeVar(varValue: String, txnScope: String) -> TxnScopeVar`：文件内唯一私有构造器，集中完成字段赋值。
- `TxnScopeVar::GetVarValue(&self) -> &str` 与 `TxnScopeVar::GetTxnScope(&self) -> &str`：借用内部字符串，不复制、不转移所有权。
- `pub const GlobalTxnScope: &str = "global"` 与 `pub const LocalTxnScope: &str = "local"`：公开的比较与构造常量。前者在语义上要求与 PD/oracle 的全局 scope 定义保持一致。

## 执行流程

默认构造流程如下：

1. `NewDefaultTxnScopeVar` 调用 `config::GetTxnScopeFromConfig()`。
2. `pkg/config/config_util.rs:224-232` 将该调用转发给 `tikvcfg::GetTxnScopeFromConfig()`；当前 Rust 实现从全局配置的 `labels["zone"]` 取值，没有 zone 时返回 `"global"`（`pkg/config/lib.rs:23-35`）。
3. 若结果是 `"global"`，`NewGlobalTxnScopeVar` 经 `newTxnScopeVar` 生成 `(varValue="global", txnScope="global")`。
4. 若结果不是 `"global"`，`NewLocalTxnScopeVar` 生成 `(varValue="local", txnScope=<配置返回值>)`。因此例如 zone 为 `zone-a` 时，展示值仍是 `local`，而实际请求 scope 是 `zone-a`。
5. 上层分别调用 `GetVarValue` 判断会话选择，调用 `GetTxnScope` 取得实际 TSO scope。Go 主链中的 `SessionVars.CheckAndGetTxnScope` 展示了预期用法：受限 SQL 或禁用本地事务时强制返回全局 scope；仅在显示值为 `local` 时采用真实 scope（`pkg/sessionctx/variable/session.go:2140-2148`）。Rust 侧尚未找到该生产调用链的对应接线。

显式全局/本地构造流程跳过配置读取，直接进入相应构造器，适合会话重置、功能开关降级或测试。

## 数据与状态

`TxnScopeVar` 是拥有两个 `String` 的普通值对象，没有全局可变状态、内部缓存或延迟初始化。其关键不变量是：

- 由 `NewGlobalTxnScopeVar` 构造时，两个字段都等于 `GlobalTxnScope`。
- 由 `NewLocalTxnScopeVar` 构造时，`varValue` 固定为 `LocalTxnScope`，`txnScope` 原样保存调用者提供的字符串。
- 由 `NewDefaultTxnScopeVar` 构造时，配置值只按“是否严格等于 `global`”分类；非全局值不会被规范化或校验。

字段私有可阻止 crate 外部随意制造不一致组合，但公开的本地构造器仍允许空字符串、`"global"` 或任意文本作为真实 scope。当前文件把输入合法性视为配置层/调用层责任，而不是自身不变量。

## 依赖与调用关系

下游依赖只有 `use crate::config`。`pkg/kv/lib.rs` 中的 `config` 名称来自 Cargo 依赖 `config-dependency = { package = "astersql-config", path = "../config" }`；本文件实际调用其公开重导出的 `GetTxnScopeFromConfig`。本文件不直接依赖 PD 客户端或网络代码。

内部调用边为：`NewDefaultTxnScopeVar -> config::GetTxnScopeFromConfig`，随后二选一调用 `NewLocalTxnScopeVar` 或 `NewGlobalTxnScopeVar`；两个显式构造器都调用 `newTxnScopeVar`。getter 没有下游函数调用，只返回字段借用。

RustCodeGraph 将目标文件识别为 8 个符号，并显示它由 `pkg/kv/mpp_2_aster_unit_test.rs` 使用。精确 `callers/callees` 命令在本次会话中超时且没有产出，因此又用全仓精确 Rust 引用搜索复核：除目标文件自身外，仅上述独立测试调用三种构造器和两个 getter，没有发现生产 Rust 上游。

Go 对照中的直接上游是 `pkg/sessionctx/variable/session.go`：新建 `SessionVars` 时调用默认构造器，禁用本地事务时替换为全局对象，并由 `CheckAndGetTxnScope` 读取两个 getter。它是理解完整应用位置的直接证据，但不是 Rust 已接线的证据。

## 错误处理与边界

本文件没有 `Result`、`Option`、panic 或日志路径。内存分配失败之外，构造与读取均为不可失败 API。配置缺少 zone 时由 `pkg/config/lib.rs:28-34` 回退到 `"global"`。

需要由上层承担的边界包括：空 zone、大小写不同的 `GLOBAL`、意外的任意标签，以及把 `"global"` 传给 `NewLocalTxnScopeVar`。由于默认构造只进行字符串不等比较，这些值都会进入本地分支；本文件不会验证标签是否存在于 PD，也不会检查本地 TSO 是否可用。

两个 getter 返回的 `&str` 生命周期绑定到 `&self`，不能在对象被丢弃后继续使用。它们不会暴露可变引用，因此调用者无法绕过构造器修改字段。

## 并发与资源生命周期

本类型不含锁、原子量、通道、异步任务、文件句柄或网络资源，也没有自定义 `Drop`。构造时获得字符串所有权；对象销毁时由 Rust 自动释放。getter 只创建共享借用，读取本身不改变状态。

文件没有显式实现 `Clone`、`Send` 或 `Sync`。由于两个字段都是 `String`，类型可由编译器自动满足 `Send`/`Sync` 的结构性条件，但跨线程共享仍须由上层决定所有权或同步策略。本文件也不会动态跟踪全局配置变化：`NewDefaultTxnScopeVar` 只在调用瞬间读取配置并形成快照。

## 与 Go 版本的对应关系

`pkg/kv/txn_scope_var.go:22-76` 是逐符号对照来源。Rust 保留了 `TxnScopeVar`、三个公开构造器、两个 getter、私有构造器以及两个常量的名称和分支逻辑；字段含义与赋值顺序也一致。

存在三点实现层差异：

- Go getter 按值返回 `string`，而 Rust getter 返回 `&str`，避免只读访问时复制，同时引入与 `self` 绑定的借用生命周期。
- Go 的 `GlobalTxnScope` 直接别名到 `client-go/oracle.GlobalTxnScope`；Rust 本文件把它写成字面量 `"global"`。`pkg/kv/lib.rs:365-367` 的 Rust oracle 桩也定义了同值常量，但本文件并未引用它，因此两处值需要人工保持同步。
- Go 的配置薄封装最终委托 client-go 的 `tikvcfg`；当前 Rust `astersql-config` 在仓库内自行读取全局配置的 zone 标签。这能覆盖默认全局和指定 zone 的基本语义，但依赖实现并非同一个上游库。

Rust 独立测试 `pkg/kv/mpp_2_aster_unit_test.rs:174-187` 验证了默认配置下的全局对象、显式全局对象，以及 `zone-a` 本地对象的显示值/真实值分离。未发现 Go 中专门以 `TxnScopeVar` 命名的独立测试；Go 会话代码提供了生产使用证据。

## 扩展指南

- 新增事务 scope 类别时，应先决定 `@@txn_scope` 的用户可见枚举与 PD 实际 scope 是否仍需二层表示，再同步修改常量、默认分类逻辑和构造器；不能只增加字符串常量而遗漏 `SessionVars.CheckAndGetTxnScope` 一类上层策略。
- 若要加强输入校验，最集中入口是 `NewLocalTxnScopeVar` 或私有 `newTxnScopeVar`。这会把当前不可失败签名改为可失败 API，需评估 Go 兼容性和所有调用者迁移，尤其不能静默把非法 zone 降级为全局事务。
- 若要消除全局常量重复，应在合适的驱动/边界层统一 `txn_scope_var::GlobalTxnScope` 与 `oracle::GlobalTxnScope`，并避免引入 crate 依赖环。
- 若接入 Rust 会话主链，应对照 `pkg/sessionctx/variable/session.go:2140-2148,2472,2593` 完成默认初始化、功能开关强制全局以及读取策略，而不是只调用 `GetTxnScope` 绕过显示值和开关判断。
- 测试必须继续放在独立文件。优先扩展 `pkg/kv/mpp_2_aster_unit_test.rs`，补充配置 zone 驱动的默认本地分支、空值/异常标签策略和常量一致性；若生产会话层完成 Rust 接线，还应在对应 `pkg/sessionctx/variable/*_test.rs` 中覆盖受限 SQL及本地事务开关分支。
- 该对象只在会话初始化或显式重置时分配两个小字符串，当前没有明显性能热点。扩展时的主要风险是兼容性与正确性：显示值和真实 zone 混淆会导致错误的 TSO 路由。

## 验证依据

- 源文件与符号：`pkg/kv/txn_scope_var.rs:27-84`；RustCodeGraph `files --filter pkg/kv/txn_scope_var.rs` 报告该文件含 8 个符号，`node --file ...` 核对了完整 84 行源码。
- crate 边界：`pkg/kv/Cargo.toml` 的 `[lib] path = "lib.rs"`、`config-dependency`；`pkg/kv/lib.rs:547-550` 的模块装入与公开重导出。
- 配置下游：`pkg/config/config_util.rs:224-232`、`pkg/config/lib.rs:23-35`。
- Go 对照及完整应用接线：`pkg/kv/txn_scope_var.go:22-76`、`pkg/sessionctx/variable/session.go:2140-2148,2472,2593`。
- Rust 独立测试：`pkg/kv/mpp_2_aster_unit_test.rs:174-187`。该测试验证默认/显式全局构造以及本地显示值与真实 zone 的分离；未运行 Cargo，符合本纯文档任务约束。
- 调用关系核验：RustCodeGraph 的宽查询定位到目标符号与直接测试；精确图查询超时无输出，随后以 `rg` 对全仓 `.rs` 做精确引用搜索，确认当前没有生产 Rust 调用者。该限制已明确保留，未把 Go 主链误述为 Rust 主链。
- 交付结构以任务指定命令检查，要求目标文件存在且恰有 11 个固定二级标题；另人工复核本文明确回答文件存在目的、默认/显式执行流程、边界以及安全扩展入口。
