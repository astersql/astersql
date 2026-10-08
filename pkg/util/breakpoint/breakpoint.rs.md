# `pkg/util/breakpoint/breakpoint.rs`

## 文件定位

本文件是 `astersql-util-breakpoint` crate 的核心实现，提供“在命名 failpoint 命中时，从会话上下文取出通知回调并同步调用”的小型适配层。crate 入口 `pkg/util/breakpoint/lib.rs` 将本模块设为私有模块后，通过 `pub use breakpoint::*` 重新导出其公开符号；根门面 `pkg/lib.rs` 又在 `util::breakpoint` 下重导出该 crate。

`pkg/util/breakpoint/Cargo.toml` 表明它属于 Go 包 `pkg/util/breakpoint` 的 Rust 移植，直接依赖 `astersql-util-context`、`astersql-util-stringutil` 和启用 `failpoints` feature 的 `fail 0.5`。`pkg/executor/Cargo.toml` 与 `pkg/testkit/Cargo.toml` 已声明此 crate，但截至本次核验，全仓 Rust 源码中除同目录测试外没有调用本文件公开 API；因此它目前是可复用的迁移基础设施，而不是已经接入 Rust 执行主链的断点。

## 核心职责

- 用稳定键值 `"breakPointNotifyFunc"` 约定断点回调在 `ValueStoreContext` 中的存放位置（`NotifyBreakPointFuncKey`）。
- 用 `BreakPointNotifyFunc` 表达接收断点名的 Rust 回调，并使其能够作为 `Any` 值存入会话上下文。
- 在 `Inject` 中把动态命名的 `fail::eval` 与会话值查询连接起来：仅当 failpoint 命中且值的动态类型正确时才通知调用方。
- 保持 Go `pkg/util/breakpoint/breakpoint.go` 的静默容错语义：failpoint 未命中、键不存在或值类型不匹配都不执行通知，也不由本函数返回错误。

该文件只负责触发和分派通知，不负责启用/禁用 failpoint，不负责注册/清除回调，也不实现“等待继续”的通道协议；是否阻塞完全由调用方存入的回调决定。

## 主要符号

- `pub static NotifyBreakPointFuncKey: LazyLock<StringerStr>`：惰性构造的全局键，底层文本为 `breakPointNotifyFunc`。使用 `StringerStr` 是为了对应 Go 的 `stringutil.StringerStr`/`fmt.Stringer` 键约定。调用 `String()` 会返回一个新的 `String`。
- `pub type BreakPointNotifyFunc = Box<dyn Fn(String) + 'static>`：拥有所有权、生命周期为 `'static` 的动态回调。参数是本次命中的断点名。类型没有要求 `Send` 或 `Sync`，因此接口本身只承诺在当前调用路径同步调用，不保证可在线程间传递。
- `pub fn Inject<C>(sctx: &C, name: &str) where C: ValueStoreContext + ?Sized`：公开注入入口。泛型允许具体上下文和 trait object 引用；只借用上下文与名称，不修改上下文本身。
- `#![allow(non_snake_case, non_upper_case_globals)]`：保留 `Inject`、`NotifyBreakPointFuncKey` 等 Go 风格名称，以便迁移代码和 Go/Rust 对照保持一致。

文件没有结构体、枚举、trait、`impl` 或条件编译分支；核心行为全部集中在上述静态键、回调别名和函数中。

## 执行流程

1. 调用方以会话上下文和断点名调用 `Inject(sctx, name)`。
2. `Inject` 调用 `fail::eval(name, closure)`。若该动态名称对应的 failpoint 未启用或未命中，闭包不执行；同目录测试 `disabled_failpoint_does_not_read_the_session` 进一步验证此时连上下文都不会读取。
3. failpoint 命中后，闭包调用 `NotifyBreakPointFuncKey.String()` 得到键字符串，并通过 `sctx.Value(&key)` 查询动态值。
4. 查询结果存在时，使用 `Any::downcast_ref::<BreakPointNotifyFunc>()` 检查值是否恰为本文件定义的 boxed 回调类型。
5. 类型匹配时，把借入的 `&str` 复制为拥有所有权的 `String`，随后同步调用回调；类型不匹配或键不存在时直接结束闭包。
6. `fail::eval` 的返回值被 `let _ = ...` 明确丢弃，`Inject` 自身返回 `()`。

Go 生产链的对应位置可在 `pkg/executor/adapter.go` 看到：第一次打开 executor 前使用 `BreakPointBeforeExecutorFirstRun`，锁错误触发语句重试后使用 `BreakPointOnStmtRetryAfterLockError`。这说明该抽象用于把异步/重试流程停在可观察位置；但这些是 Go 调用边，不能视为 Rust executor 已接线的证据。

## 数据与状态

本文件持有的唯一全局状态是 `NotifyBreakPointFuncKey`。`LazyLock` 保证键最多初始化一次；初始化完成后只读。每次命中都会由 `StringerStr::String` 克隆键字符串，并由 `name.to_owned()` 为回调克隆断点名。

回调及其捕获状态不由本文件保存，而由 `ValueStoreContext` 的实现拥有。`pkg/util/context/context.rs` 定义的接口以 `Box<dyn Any>` 保存值、以 `Option<&dyn Any>` 借出值；本文件仅在回调执行期间持有借用，不移动或删除该值。注册者必须用完全相同的动态类型 `BreakPointNotifyFunc` 装箱，普通函数项、另一种闭包包装或错误类型不会通过 `downcast_ref`。

failpoint 的启停状态由 `fail` crate 的全局场景管理，不属于此文件。同目录测试使用 `fail::FailScenario::setup()` 管理场景，并用全局互斥锁串行化配置，证明测试必须防止不同用例之间的 failpoint 状态相互干扰。

## 依赖与调用关系

下游依赖如下：

- `std::sync::LazyLock`：一次性、安全地初始化全局键。
- `crate::contextutil::context::ValueStoreContext`：提供 `Value` 动态查询；实际 crate 由 `contextutil-crate` 重导出。
- `crate::stringutil::string_util::StringerStr`：承载与 Go 一致的可字符串化键；实际 crate 由 `stringutil-crate` 重导出。
- `fail::eval`：按运行时名称判断 failpoint，并只在命中时执行闭包。
- `std::any::Any::downcast_ref`（经上下文返回值使用）：执行回调类型检查。

已验证的直接 Rust 上游只有 `pkg/util/breakpoint/migration_aster_unit_test.rs` 的四个用例。crate 级潜在上游包括 `pkg/executor`、`pkg/testkit` 和根 facade 的 Cargo/重导出接线，但 `rg` 未发现这些 Rust 模块调用 `Inject` 或读取通知键。

Go 对照链有明确生产调用：`pkg/executor/adapter.go` 调用 `breakpoint.Inject` 两次；`pkg/testkit/stepped.go` 注册一个会发送“已停在断点”消息并等待 continue 消息的回调，同时启用带完整 Go 包路径的 failpoint，结束时清理键和 failpoint。`pkg/session/tidb_test.go`、`pkg/server/conn_test.go` 也直接注册通知回调以观察执行时点。

## 错误处理与边界

- failpoint 未启用/未命中：不读取上下文、不调用回调，正常返回。
- 上下文中没有该键：`Value` 返回 `None`，正常返回。
- 键存在但动态类型不是 `BreakPointNotifyFunc`：`downcast_ref` 返回 `None`，正常返回；迁移测试以 `i32` 验证不会 panic。
- `fail::eval` 的结果：本实现主动忽略，因而不向调用方暴露配置或求值错误。
- 回调自身 panic：本文件没有 `catch_unwind`，因此 panic 会沿当前调用栈传播。
- 回调重入、耗时或永久等待：本文件没有超时、取消或重入保护；这些都属于回调实现的责任。
- 键类型边界：查询时先生成 `String` 并以其 `Display` 表示访问上下文。上下文实现必须以相同文本语义匹配键，不能依赖 `StringerStr` 的对象身份。

`Inject` 没有结果值，因此调用方不能区分“未命中”“未注册”“类型错误”三种静默路径；若扩展可观测性，必须先评估是否会破坏 Go 兼容和大量测试钩子的非侵入特性。

## 并发与资源生命周期

`Inject` 不创建线程、任务、通道、锁或事务；`fail::eval` 闭包和通知回调都在调用 `Inject` 的线程上同步执行。回调可以像 Go 的 `SteppedTestKit` 那样阻塞当前执行路径，直到测试线程发送继续信号，因此调用方必须避免在持有不允许长时间占用的锁时注入断点。

全局键由 `LazyLock` 管理进程期生命周期，无需手动释放。上下文拥有 boxed 回调；本文件仅借用它并在调用结束后释放借用。注册/清除时机由外层会话或测试工具负责，Go `stepped.go` 的做法是在命令开始前注册、命令结束的 defer 中清空。

回调别名缺少 `Send + Sync` 约束。这与当前同步借用调用相容，但若未来上下文实现要求保存值可跨线程共享，不能仅在调用点加线程操作；需要同时调整回调别名、上下文值约束和独立测试。测试中的 `Arc<Mutex<Vec<String>>>` 只是被回调捕获的数据同步手段，不代表回调类型本身承诺线程安全。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/util/breakpoint/breakpoint.go`：

- Go 常量 `stringutil.StringerStr("breakPointNotifyFunc")` 对应 Rust 的 `LazyLock<StringerStr>`，文本值相同；Rust 因 `String` 不是 `const` 可构造值而使用惰性静态量。
- Go `func(string)` 的动态类型断言对应 Rust `BreakPointNotifyFunc = Box<dyn Fn(String)>` 与 `downcast_ref`。
- Go `sessionctx.Context` 参数对应更窄的 Rust `ValueStoreContext` trait bound，只要求此函数真实使用的值查询能力。
- Go `failpoint.Inject(name, ...)` 对应 Rust `fail::eval(name, ...)`；两者均只在命名点命中时访问会话和通知回调。
- Go 将原字符串参数直接传给回调；Rust 为满足 owned 回调签名调用 `name.to_owned()`。
- 两版都对缺失值和错误动态类型静默跳过，且都让实际等待行为由注册的回调实现。

迁移差异是 Rust 回调必须以 `BreakPointNotifyFunc` 的精确 boxed 类型存储，并且当前不带 `Send + Sync`；Go 的 `func(string)` 可直接作为接口值保存。另一个重要状态差异是 Go 已在 executor 与 stepped testkit 中生产接线，Rust 侧目前只有 Cargo 依赖、facade 重导出和本 crate 的独立迁移测试，尚未找到对应生产调用。

## 扩展指南

- 新增断点不需要修改本文件；应在拥有业务时点的模块定义稳定名称并调用 `Inject`，同时在独立 `*_test.rs` 文件中覆盖“未启用不访问上下文”和“启用后传递准确名称”。不要把测试模块写进 `breakpoint.rs`。
- 若改变键名或键类型，必须同步 `NotifyBreakPointFuncKey`、注册回调的所有调用方、Go 兼容预期和 `migration_aster_unit_test.rs::notify_key_matches_the_go_stringer_value`。键名属于跨模块协议，修改会让已有注册静默失效。
- 若支持不同回调签名或返回错误，应优先修改 `BreakPointNotifyFunc` 和 `Inject` 的 downcast/传播策略，并新增错误类型、panic、重复命中等边界测试；避免接受多个隐式动态类型而掩盖配置错误。
- 若将 Rust executor/testkit 真正接入，应参考 Go `pkg/executor/adapter.go` 的两个精确时点和 `pkg/testkit/stepped.go` 的注册—启用—等待—继续—清理生命周期，而不是仅凭 Cargo 依赖声称功能完整。
- 若要求跨线程保存或调用回调，需要评估为 trait object 增加 `Send + Sync`、上下文 `Any` 容器的线程安全约束以及阻塞回调可能造成的死锁；这是兼容性变化，不能只做局部类型修改。
- 性能敏感路径扩展时需注意每次命中会克隆键和断点名。未命中路径由测试证明不访问上下文，但仍调用 `fail::eval`；新增观测或日志不应改变其低干扰特性。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引包含 11,467 个文件；`files --filter pkg/util/breakpoint` 返回 `breakpoint.rs`、`lib.rs`、`migration_aster_unit_test.rs` 与 Go 对照文件；`node --file pkg/util/breakpoint/breakpoint.rs --offset 1 --limit 240` 核对了目标文件全部 53 行。索引将该 Rust 文件报告为 2 个符号且未给出使用文件，精确 callers/callees 查询未返回可用结果，因此调用关系又由下列源码搜索核验。
- 目标实现：`pkg/util/breakpoint/breakpoint.rs`，核对 `NotifyBreakPointFuncKey`、`BreakPointNotifyFunc`、`Inject` 和 `fail::eval` 闭包。
- crate 边界：`pkg/util/breakpoint/Cargo.toml`、`pkg/util/breakpoint/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`；反向依赖声明见 `pkg/executor/Cargo.toml` 与 `pkg/testkit/Cargo.toml`。
- 直接依赖定义：`pkg/util/context/context.rs::ValueStoreContext` 与 `pkg/util/stringutil/string_util.rs::StringerStr`。
- Rust 独立测试：`pkg/util/breakpoint/migration_aster_unit_test.rs` 的 `notify_key_matches_the_go_stringer_value`、`disabled_failpoint_does_not_read_the_session`、`enabled_failpoint_notifies_with_the_injected_name`、`enabled_failpoint_ignores_a_non_callback_value`。
- Go 对照与真实调用：`pkg/util/breakpoint/breakpoint.go`、`pkg/executor/adapter.go`、`pkg/testkit/stepped.go`、`pkg/session/tidb_test.go`；断点名的 Rust 移植定义见 `pkg/sessiontxn/failpoint.rs`。
- 全仓 `rg` 结果：Rust 侧对三个公开符号的引用仅出现在目标文件及其同目录测试；`astersql-util-breakpoint` 则存在 Cargo 依赖和 facade 重导出。这支持“crate 已装配、API 尚无 Rust 生产调用”的结论。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前另以任务规定的命令验证恰好存在 11 个固定二级标题。
