# `pkg/executor/admin_plugins.rs`

## 文件定位

[`pkg/executor/admin_plugins.rs`](admin_plugins.rs) 属于 `astersql-executor` crate，并由 [`pkg/executor/lib.rs`](lib.rs) 以公开模块 `admin_plugins` 导出。它描述 `ADMIN PLUGINS ENABLE/DISABLE` 的 Rust 执行器核心：把规划阶段给出的动作和插件名列表转换为逐插件的“修改禁用标志并刷盘”调用。

当前文件是可复用的执行逻辑边界，而不是已经完整接入 SQL 主链的生产执行器。RustCodeGraph 显示该文件当前只被 `pkg/executor/admin_plugins_test.rs` 和一个与本功能无生产接线关系的测试编译单元引用；仓库搜索也未发现 Rust builder 构造 `AdminPluginsExec`，或任何生产类型实现 `PluginFlagFlusher`。因此，“Rust 已具备动作分派和遍历语义”是已验证事实，“Rust SQL 请求会执行到这里”则尚未成立。Go 生产链路由 `pkg/executor/builder.go::buildAdminPlugins` 构造同名 Go 执行器。

## 核心职责

- `AdminPluginsAction` 表示启用、禁用和兼容未知整数动作。
- `PluginFlagFlusher` 把领域/插件层副作用抽象为一个可失败操作，使执行器不需要持有具体 domain 或 etcd 客户端。
- `AdminPluginsExec` 保存基类状态、动作、目标插件列表和刷盘实现。
- `AdminPluginsExec::Next` 将 `Enable` 映射为 `disabled = false`，将 `Disable` 映射为 `disabled = true`；未知动作成功返回但不产生副作用。
- `changeDisableFlagAndFlush` 按 `Plugins` 原顺序调用 flusher，并在第一个错误处立即停止。

该语句不产生结果行：`Next` 接受上下文和输出 chunk 的泛型占位参数，但两者均未使用，成功结果为 `Ok(())`。

## 主要符号

- `pub enum AdminPluginsAction`
  - `Enable = 1`：清除插件禁用标志。
  - `Disable = 2`：设置插件禁用标志。
  - `Unknown(i32)`：保留 Go 的整数动作可能承载的非预期值；执行时 no-op。枚举派生 `Clone`、`Copy`、`Debug`、`Eq`、`PartialEq`，并声明 `#[repr(i32)]`。
- `pub trait PluginFlagFlusher`
  - 关联类型 `Error` 由具体实现决定，执行器不包装或转换错误。
  - `change_disable_flag_and_flush(&mut self, plugin_name: &str, disabled: bool)` 是唯一领域副作用接口。
- `pub struct AdminPluginsExec<B, F>`
  - `BaseExecutor: B`：保留与 Go 嵌入 `exec.BaseExecutor` 对应的状态；本文件不读取它。
  - `Action: AdminPluginsAction`：本次语句动作。
  - `Plugins: Vec<String>`：按执行顺序保存目标插件名。
  - `Flusher: F`：拥有的领域操作实现，要求 `F: PluginFlagFlusher`。
- `pub fn Next<C, Q>(&mut self, _context: C, _chunk: &mut Q) -> Result<(), F::Error>`：公开迭代入口；泛型 `C`、`Q` 没有 trait 约束，也不参与行为。
- `fn changeDisableFlagAndFlush(&mut self, disabled: bool) -> Result<(), F::Error>`：私有顺序遍历辅助函数。

源文件没有模块级常量、条件编译项或独立自由函数。

## 执行流程

1. 上游构造 `AdminPluginsExec`，填入动作、插件名列表、基类状态和 flusher。当前仓库只验证了测试构造；Rust 生产 builder 尚未找到。
2. 迭代框架调用 `Next`。`_context` 和 `_chunk` 仅用于保持执行器形状，本实现不读写它们。
3. `Next` 匹配 `Action`：
   - `Enable` 调用 `changeDisableFlagAndFlush(false)`；
   - `Disable` 调用 `changeDisableFlagAndFlush(true)`；
   - `Unknown(_)` 直接返回 `Ok(())`。
4. 辅助函数按 `Plugins` 的向量顺序借用每个名字，并调用 `Flusher::change_disable_flag_and_flush`。
5. 每次成功后继续下一个名字；`?` 在首个错误处把原错误直接返回，后续插件不再处理；全部成功或列表为空时返回 `Ok(())`。

`pkg/executor/admin_plugins_test.rs` 分别覆盖启用时传 `false` 且保持顺序、禁用时传 `true` 且首错即停、未知动作不调用 flusher。

## 数据与状态

执行器自身只有四类状态：不透明的 `BaseExecutor`、值语义的动作枚举、拥有字符串的插件列表、拥有的 flusher。执行期间不修改 `Action`、`Plugins` 或 `BaseExecutor`；可变状态集中在 `Flusher`，因此测试可通过 `RecordingFlusher.calls` 观察调用顺序。

本文件不保存完成游标，也没有“已经执行”标志。若上游对同一个实例多次调用 `Next`，整个插件列表会被重复处理；是否只调用一次属于外部执行器生命周期约束，本文件没有防重保证。

多插件操作不是事务：在第一个失败之前已成功处理的插件不会回滚。具体生产副作用可参考 `pkg/plugin/plugin.rs::change_disable_flag_and_flush`：它先验证插件存在、处于 Ready 且支持 watcher，再以 Release 顺序写入内存原子禁用标志，最后向 etcd 写入 `0` 或 `1`。如果 etcd 写入失败，内存标志已经改变；执行器层不补偿这一状态。

## 依赖与调用关系

直接编译依赖仅为 Rust 标准库类型和泛型约束；源文件没有 `use` 外部 crate。其 crate 边界由 `pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 确定，`pkg/executor/lib.rs` 公开声明 `pub mod admin_plugins`，测试模块则在 `#[cfg(test)]` 下声明。

已验证的 Rust 关系如下：

- 上游：`pkg/executor/admin_plugins_test.rs` 直接构造执行器并调用 `Next`；未找到 Rust 生产调用者或 builder 接线。
- 内部：`Next` 调用私有 `changeDisableFlagAndFlush`；后者调用 trait 方法 `PluginFlagFlusher::change_disable_flag_and_flush`。
- 潜在下游而非当前接线：`pkg/plugin/plugin.rs::change_disable_flag_and_flush` 已实现真实插件状态变更与 etcd 通知，但仓库中没有把它包装成 `PluginFlagFlusher` 的实现。
- crate 能看到 `astersql-plugin`、`astersql-domain` 等路径依赖（见 `pkg/executor/Cargo.toml`），但本文件尚未直接使用它们。

Go 的完整生产调用链为规划器生成 `pkg/planner/core/common_plans.go::AdminPlugins`，`pkg/executor/builder.go::buildAdminPlugins` 构造 `AdminPluginsExec`，执行器 `Next` 获取 domain 后调用 `pkg/plugin/plugin.go::ChangeDisableFlagAndFlush`。

## 错误处理与边界

- flusher 的错误类型完全透传为 `F::Error`，没有日志、重试、错误上下文或错误归一化。
- 处理策略是 fail-fast：失败插件已经被调用，后续插件不会被调用；此前成功项不会回滚。
- 空 `Plugins` 列表自然成功，且不调用 flusher。
- 重复插件名不会去重，会按输入次数重复调用。
- 插件名不在执行器层校验；空字符串、未知插件、未 Ready 或不支持 flush 等条件由具体 flusher 负责。
- `Unknown(i32)` 是成功 no-op，不会把未知动作报告为错误。这与 Go `switch` 的 default 成功路径一致，但可能掩盖上游非法动作，扩展时必须保留或明确改变这一兼容契约。
- `_context`、`_chunk` 和 `BaseExecutor` 不参与当前实现，取消、超时和结果集容量不会中断循环。
- 真实 Rust 插件函数在 etcd 失败前已更新内存原子标志；调用方不能把错误理解为“完全没有状态变化”。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或网络连接。`Next` 需要 `&mut self`，因此安全 Rust 中同一执行器实例不能被两个调用者同时可变执行；循环本身严格串行。

插件名字符串和 flusher 由执行器拥有，插件名在调用期间以 `&str` 临时借用。flusher 可在每次调用中修改自身状态，借用在该次调用结束后释放。错误返回不会销毁执行器，但会终止本次循环；再次调用 `Next` 会从列表首项重新开始。

跨节点同步、原子内存顺序和 etcd 资源由具体插件层负责，不由本 trait 规定。当前 `pkg/plugin/plugin.rs` 的候选真实实现使用原子变量和 watcher 的 etcd 客户端；由于尚无 `PluginFlagFlusher` 生产适配器，本文件本身并未获得这些并发或资源保证。

## 与 Go 版本的对应关系

Rust 基本保留了 `pkg/executor/admin_plugins.go` 的控制流：

- Go `core.Enable`/`core.Disable` 分别对应 Rust `Enable = 1`/`Disable = 2`。
- 两侧都把启用映射为 `disabled=false`、禁用映射为 `disabled=true`。
- 两侧都按输入顺序逐项执行、首错返回、空列表成功、未知动作成功 no-op。
- Go 的 `AdminPluginsExec` 嵌入 `exec.BaseExecutor` 并保存 `Action`、`Plugins`；Rust 以泛型字段显式保存相同概念，并新增 `Flusher` 字段。

关键差异是生产接线。Go 的私有辅助函数通过 `domain.GetDomain(e.Ctx())` 取得 domain，再直接调用 `plugin.ChangeDisableFlagAndFlush(dom, ...)`，且 builder 已构造该执行器。Rust 用 trait 注入副作用，`Next` 的上下文不参与取 domain；虽然 `pkg/plugin/plugin.rs` 有对应真实函数，当前没有 trait 适配器和 builder 调用。因此 Rust 版本目前是经过独立测试的执行器核心，不等价于已连通的 SQL 端到端实现。

Go 仓库未找到针对 `AdminPluginsExec` 或 `ChangeDisableFlagAndFlush` 的直接单元测试；`pkg/parser/parser_test.go` 只覆盖相关 SQL 的解析/恢复，`pkg/plugin/integration_test.go` 中相关 ADMIN PLUGINS 用例仍被注释。Rust 独立测试提供了本文件控制流的直接证据，但不验证真实 domain、etcd 或多节点传播。

## 扩展指南

- 接入生产链路时，应新增位于合适生产模块的 `PluginFlagFlusher` 具体实现，将调用委托给 `pkg/plugin/plugin.rs::change_disable_flag_and_flush`，并在 Rust executor builder 中从规划节点构造 `AdminPluginsExec`；不要把测试 recorder 当作生产 fallback。
- 增加动作时，需要同步修改 `AdminPluginsAction`、`Next` 的匹配分支、规划器动作映射和独立测试。必须决定旧的 `Unknown(_)` no-op 兼容语义是否仍成立。
- 改变批量语义（去重、并行、重试、回滚）前，应先处理“部分成功”的兼容性。并行化会改变稳定顺序和首错停止行为，不能作为无行为变化的优化。
- 若加入取消或超时，应为上下文定义明确 trait/类型并在循环中检查；当前无约束泛型不能提供这些能力。
- 若要防止重复 `Next` 产生重复副作用，需要增加执行状态并同步迭代器生命周期测试，不能假设外部永远只调用一次。
- 测试继续放在独立的 `pkg/executor/admin_plugins_test.rs`，不要内嵌到生产源文件。至少同步覆盖空列表、重复名字、首项/中间项失败、未知动作以及生产适配器错误透传；真实插件状态和 etcd 行为应在插件层独立测试。
- 新接线可能带来兼容风险（未知动作、错误文本）、正确性风险（内存已变更而 etcd 失败）和性能风险（大量插件严格串行写 etcd），评审时需逐项确认。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `explore "pkg/executor/admin_plugins.rs AdminPluginsExec OpenPluginsExec ClosePluginsExec ReloadPluginsExec"`：返回目标 Rust/Go 文件及独立 Rust 测试的完整上下文，并给出 Go 辅助函数由 `Next` 调用的关系。
- RustCodeGraph `node --file pkg/executor/admin_plugins.rs --offset 1 --limit 400`：核对本文件 96 行全部源码，并显示其被两个测试编译单元使用。
- RustCodeGraph `query AdminPluginsExec --kind struct`、`query change_disable_flag_and_flush`：核对执行器、trait 方法、测试实现和 `pkg/plugin/plugin.rs` 中的候选真实函数。
- RustCodeGraph `node --file pkg/plugin/plugin.rs --offset 590 --limit 90` 与 `node disable_flag`：核对插件存在/Ready/watcher 校验、原子标志更新和 etcd 写入顺序。
- 已读源码与配置：`pkg/executor/admin_plugins.rs`、`pkg/executor/admin_plugins_test.rs`、`pkg/executor/lib.rs`、`pkg/executor/Cargo.toml`、`pkg/executor/admin_plugins.go`、`pkg/executor/builder.go`、`pkg/planner/core/common_plans.go`、`pkg/plugin/plugin.go`、`pkg/plugin/plugin.rs`、`pkg/parser/parser_test.go`、`pkg/plugin/integration_test.go`。
- 仓库搜索 `admin_plugins|AdminPluginsExec|AdminPluginsAction|PluginFlagFlusher|ChangeDisableFlagAndFlush`：确认 Rust 模块声明和测试引用，未发现 Rust 生产构造器或 `PluginFlagFlusher` 生产实现；确认 Go builder 和插件调用链。
- 结构校验按任务文件指定命令执行；本任务为纯文档分析，按计划不运行 Cargo。
