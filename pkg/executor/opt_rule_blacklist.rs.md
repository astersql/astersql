# `pkg/executor/opt_rule_blacklist.rs`

## 文件定位

本文件属于 `astersql-executor` crate；crate 根由 [`pkg/executor/Cargo.toml`](Cargo.toml) 的 `lib.path = "lib.rs"` 指向 [`pkg/executor/lib.rs`](lib.rs)，后者通过 `pub mod opt_rule_blacklist` 公开本模块，并仅在 `cfg(test)` 下装配独立测试文件 [`pkg/executor/opt_rule_blacklist_test.rs`](opt_rule_blacklist_test.rs)。目标源码是 `ADMIN RELOAD OPT_RULE_BLACKLIST` 管理动作的 Rust 侧加载逻辑：它从系统表 `mysql.opt_rule_blacklist` 取得规则名，将结果转换为禁用规则集合，再通过抽象上下文发布给会话/规划器侧。

当前迁移状态必须与设计意图区分：[`pkg/executor/builder.rs`](builder.rs) 已定义 `ExecutorKind::ReloadOptRuleBlacklist`、`Plan::ReloadOptRuleBlacklist` 和叶子执行器构建分支，但仓库搜索只发现测试代码实现 `OptRuleBlacklistContext`，也只发现本文件与测试调用 Rust `LoadOptRuleBlacklist`。因此，本文件的通用加载算法已有测试，生产上下文适配和从通用 `ExecutorBox` 到这里的具体执行器接线则未在现有 Rust 代码中得到验证。

## 核心职责

- `ReloadOptRuleBlacklistExec::Next` 为一次管理命令执行创建新的内部权限上下文，并触发加载；它不产生结果行。
- `LoadOptRuleBlacklist` 使用固定的高优先级受限 SQL 查询规则名，以 `HashSet<String>` 消除完全相同的重复名称，然后一次性调用 `replace_disabled_logical_rules`。
- `OptRuleBlacklistContext` 隔离执行器与具体会话/规划器实现，使查询错误类型和状态发布方式由接入层决定。
- 本模块只负责“读取、去重、整集合替换”的边界，不解析或规范化规则名，也不判断某个名称是否对应有效逻辑优化规则。

## 主要符号

- `LOAD_OPT_RULE_BLACKLIST_SQL: &str`：模块私有常量，值为 `select HIGH_PRIORITY name from mysql.opt_rule_blacklist`；固定只读取 `name` 列，并要求数据库侧以高优先级执行。
- `pub trait OptRuleBlacklistContext`：生产接线必须实现的桥接 trait。
  - `type Error` 保留底层受限 SQL 的错误类型。
  - `exec_restricted_sql(&mut self, &Context, &str) -> Result<Vec<String>, Error>` 执行查询并把首列映射为字符串列表。
  - `replace_disabled_logical_rules(&mut self, HashSet<String>)` 发布完整的新集合；接口没有增量更新或失败返回值。
- `pub struct ReloadOptRuleBlacklistExec<C>`：泛型执行器壳，仅持有公开字段 `context: C`。
- `ReloadOptRuleBlacklistExec::Next<T, U>`：公开入口；泛型参数只用于兼容调用形态，传入的 `_ctx` 与 `_request` 均不参与逻辑。
- `pub fn LoadOptRuleBlacklist<C>(...)`：可被执行器或启动过程复用的加载函数；查询成功后去重并替换，成功返回 `Ok(())`。
- 文件级 `#![allow(non_snake_case)]`：允许 `Next`、`LoadOptRuleBlacklist` 保持与 Go 导出 API 一致的命名。

## 执行流程

1. 管理入口调用 `ReloadOptRuleBlacklistExec::Next`。
2. `Next` 丢弃调用方传入的上下文，执行 `WithInternalSourceType(Context::new(), InternalTxnPrivilege)`，创建带内部事务权限来源标记的新上下文。
3. `Next` 将内部上下文和可变桥接对象交给 `LoadOptRuleBlacklist`。
4. `LoadOptRuleBlacklist` 调用 `context.exec_restricted_sql` 执行固定 SQL。若查询失败，`?` 立即返回错误，后续状态不变。
5. 查询成功后，`rows.into_iter().collect::<HashSet<_>>()` 消耗字符串列表并去除完全相同的重复值。比较遵循 Rust `String` 的精确、区分大小写语义，因此 `rule-a` 与 `RULE-A` 是两个名称。
6. 函数调用一次 `replace_disabled_logical_rules(disabled_rules)`，即使查询返回空列表也会用空集合替换旧集合，最后返回 `Ok(())`。

[`pkg/executor/opt_rule_blacklist_test.rs`](opt_rule_blacklist_test.rs) 的 `next_uses_fresh_privilege_context_and_replaces_with_unique_names` 验证步骤 2、固定 SQL、去重、大小写保留及单次替换；`load_propagates_query_error_without_replacing_global_state` 验证步骤 4 的错误短路。

## 数据与状态

模块本身没有静态可变状态。短生命周期数据包括一个新的 `astersql_kv::Context`、查询返回的 `Vec<String>`，以及由它消费得到的 `HashSet<String>`。集合所有权整体移交给 `replace_disabled_logical_rules`，避免逐项修改期间暴露半更新状态；但“原子替换”的实际同步保证属于 trait 实现者，本文件没有锁或原子变量可独立保证这一点。

空查询结果具有明确语义：发布空集合，从而清除所有已禁用名称。重复项只保留一份；名称不做 trim、大小写折叠、别名转换或有效性校验。Rust 规划器侧另有 [`pkg/planner/core/optimizer_runtime.rs`](../planner/core/optimizer_runtime.rs) 的 `DefaultDisabledLogicalRulesList: RwLock<Vec<String>>`，但当前仓库未发现本 trait 到该静态量的实现或本静态量的 Rust 消费点，不能据此声称加载结果已进入生产优化流程。

## 依赖与调用关系

直接标准库依赖只有 `std::collections::HashSet`。外部依赖来自 `astersql-kv`，具体为 `Context`、`InternalTxnPrivilege` 和 `WithInternalSourceType`；[`pkg/executor/Cargo.toml`](Cargo.toml) 以工作区路径 `../kv` 声明 `astersql-kv`，本逻辑不受 `nextgen` feature 条件控制。

RustCodeGraph 对目标文件的索引显示：`ReloadOptRuleBlacklistExec::Next -> LoadOptRuleBlacklist -> OptRuleBlacklistContext::{exec_restricted_sql, replace_disabled_logical_rules}`。已索引调用者包括本文件的 `Next` 和独立测试；代码搜索未发现生产 `OptRuleBlacklistContext` 实现。`pkg/executor/builder.rs` 能把对应计划分类为 `ExecutorKind::ReloadOptRuleBlacklist`，但其 `buildReloadOptRuleBlacklist` 当前仅调用通用 `build_leaf`，没有直接构造本文件的泛型结构体。

Go 主链证据更完整：[`pkg/executor/opt_rule_blacklist.go`](opt_rule_blacklist.go) 的 `ReloadOptRuleBlacklistExec.Next` 调用 `LoadOptRuleBlacklist`；[`pkg/session/session.go`](../session/session.go) 的 `bootstrapSessionImpl` 也在启动时调用它。Rust 仓库中未找到对应启动调用，故启动自动加载仅能确认为 Go 行为。

## 错误处理与边界

唯一可返回的业务错误来自 `exec_restricted_sql`，并以关联类型 `C::Error` 原样向上传播。查询失败发生在任何集合构造和发布之前，因此旧状态不会被本次调用覆盖。相反，`replace_disabled_logical_rules` 的签名没有返回 `Result`，发布失败无法通过该接口表达；实现若会 panic、锁中毒或产生部分更新，本模块也没有恢复逻辑。

本文件假定桥接层已经把数据库结果转换为 `Vec<String>`，因此 SQL `NULL`、列类型错误、行解码失败等细节必须由 `exec_restricted_sql` 实现处理。它也不验证规则名合法性；未知名称如何处理取决于下游规划器。`Next` 无视调用方取消上下文，使用全新 `Context::new()`，所以调用方取消/截止时间不会由此参数自动继承；这与 Go 对照实现使用 `context.Background()` 的意图一致。

## 并发与资源生命周期

每次 `Next` 都创建独立内部上下文；函数内没有线程、异步任务、通道、事务句柄或显式资源清理。`Vec<String>` 在收集时被消费，临时 `HashSet` 随后转移给状态发布方法。

并发读写策略不由本模块决定。为了满足注释所述的整集合替换语义，生产 `OptRuleBlacklistContext` 实现应在单个同步临界区中发布完整集合，并保证并发规划请求只观察旧集合或新集合，而非中间状态。若接到规划器的 `RwLock`，还需定义锁中毒处理和持锁范围；这些行为目前没有生产实现证据。

## 与 Go 版本的对应关系

[`pkg/executor/opt_rule_blacklist.go`](opt_rule_blacklist.go) 是直接对照：两者均执行相同的 `HIGH_PRIORITY` SQL，均以内部权限来源执行，均对名称进行精确去重，并仅在查询成功后整体替换状态。Rust 的 `HashSet<String>` 对应 Go 的 `set.StringSet`；Rust 通过 trait 抽象受限 SQL 和发布操作，Go 则直接取得 `RestrictedSQLExecutor` 并调用 `plannercore.DefaultDisabledLogicalRulesList.Store`。

差异与迁移缺口如下：

- Go 执行器嵌入 `exec.BaseExecutor` 并由其 `Ctx()` 取得会话；Rust 结构体直接拥有泛型 `context`，尚未发现与通用执行器框架的生产适配。
- Go 发布到进程级 `atomic.Value`；Rust 文件把发布责任交给 trait。规划器 Rust 静态量当前是 `RwLock<Vec<String>>`，类型也不是本函数产生的 `HashSet<String>`，需由适配层明确转换及同步语义。
- Go 启动过程在 `pkg/session/session.go` 调用加载函数；未发现 Rust 启动链等价调用。
- 两个实现都不规范化名称。Rust 测试特别确认大小写不同的名称同时保留。

## 扩展指南

若要完成生产接线，应优先实现 `OptRuleBlacklistContext`：将 `exec_restricted_sql` 接到真实会话的受限 SQL 能力，并让 `replace_disabled_logical_rules` 一次性更新规划器实际消费的状态；随后把 `builder.rs` 的叶子执行器分派和 Rust bootstrap 路径接到 `ReloadOptRuleBlacklistExec`/`LoadOptRuleBlacklist`。接线前应先确认规划器期望 `Vec<String>` 还是集合，以及规则匹配是否区分大小写，不能只为类型兼容静默改变 Go 语义。

新增逻辑时应同步扩展独立文件 [`pkg/executor/opt_rule_blacklist_test.rs`](opt_rule_blacklist_test.rs)，不要把测试嵌入生产源码。建议覆盖：空结果清空旧状态、重复项与大小写、真实状态替换的并发可见性、无效/未知规则名、查询解码错误、启动加载以及管理命令端到端生效。若修改 SQL、权限来源或错误传播，还需与 Go 文件和其调用链保持一致。性能风险主要是一次加载同时持有 `Vec` 与 `HashSet`、以及发布时可能复制集合；正确性风险集中在非原子发布、接错会话上下文和规则名规范化差异。

## 验证依据

- RustCodeGraph：`status` 显示目标工作区索引包含 11,467 个文件；`node --file pkg/executor/opt_rule_blacklist.rs` 读取了完整 72 行，并标出测试与 `pkg/session/syssession/session.rs` 的索引关系；`query LoadOptRuleBlacklist`、`query OptRuleBlacklistExec` 和 `explore` 确认核心符号与 `Next -> LoadOptRuleBlacklist -> trait 方法` 调用链。
- 源码：[`pkg/executor/opt_rule_blacklist.rs`](opt_rule_blacklist.rs)（常量、trait、执行器与加载函数）。
- crate/装配：[`pkg/executor/Cargo.toml`](Cargo.toml)、[`pkg/executor/lib.rs`](lib.rs)、[`pkg/executor/builder.rs`](builder.rs)。
- Rust 规划器状态：[`pkg/planner/core/optimizer_runtime.rs`](../planner/core/optimizer_runtime.rs)。
- 独立 Rust 测试：[`pkg/executor/opt_rule_blacklist_test.rs`](opt_rule_blacklist_test.rs)。
- Go 对照与生产入口：[`pkg/executor/opt_rule_blacklist.go`](opt_rule_blacklist.go)、[`pkg/session/session.go`](../session/session.go)。
- 仓库搜索：`rg` 只找到测试中的 `OptRuleBlacklistContext` 实现，且 Rust 侧未找到 `LoadOptRuleBlacklist` 的生产调用；因此文中把生产适配、启动接线和实际规划器消费明确标为未验证/迁移缺口。
