# `dumpling/context/context.rs`

## 文件定位

本文件实现 crate `astersql-dumpling-context` 的核心包装类型。crate 入口 `dumpling/context/lib.rs` 通过 `mod context` 加载本文件并以 `pub use context::*` 导出全部公开项；`dumpling/context/Cargo.toml` 声明它是对应 Go 包 `dumpling/context` 的 library crate，直接依赖只有相邻的 `astersql-dumpling-log`。工作区中 `dumpling/export/Cargo.toml` 是该 crate 的直接业务消费者，`dumpling/export/lib.rs` 将其别名为 `tcontext`，供导出主流程共享取消状态和 logger。

本文件不是通用异步运行时，也不直接执行导出任务。它把 `dumpling/context/stubs.rs` 提供的最小 Go 风格 context 与 dumpling logger 组合成一个轻量值对象，作为 `dumpling/export` 各阶段传递运行期上下文的统一接口。RustCodeGraph 对该文件的文件节点报告 10 个符号，并显示它被 32 个文件引用；实际业务入口可见于 `dumpling/export/dump.rs::NewDumper`。

## 核心职责

- `Context` 同时持有取消上下文与 logger，使调用方能够从同一个参数读取停止状态并记录带统一配置的日志。
- `Background` 和 `NewContext` 提供根对象与显式组合对象的构造入口。
- `WithContext`、`WithCancel`、`WithLogger` 采用“返回新值”的派生方式，保留未修改字段且不原地改变旧对象，对齐 Go `*Context` helper 的使用语义。
- `L`、`Done`、`Err` 提供 logger 与取消状态的窄接口；其中 `Done`/`Err` 是对内嵌 context 的显式转发，以补足 Rust 不具备 Go 匿名嵌入方法提升这一差异。

这里的职责边界很窄：真正的取消传播由 `dumpling/context/stubs.rs::Context` 完成，日志行为由 `astersql-dumpling-log::Logger` 完成；本文件只负责组合、派生和转发。

## 主要符号

- `pub struct Context { pub Context: gcontext::Context, logger: log::Logger }`：公开底层取消上下文，私有保存 logger。类型实现 `Clone` 和 `Debug`，没有自定义 `Drop`。
- `pub fn Background() -> Context`：以 `gcontext::Background()` 和 `log::Zap()` 构造未取消、使用 nop logger 的根包装。
- `pub fn NewContext(ctx, logger) -> Context`：原样组合调用方提供的 `gcontext::Context` 与 `log::Logger`，不创建新的取消节点。
- `Context::WithContext(&self, ctx) -> Context`：替换底层 context，克隆并保留现有 logger。
- `Context::WithCancel(&self) -> (Context, gcontext::CancelFunc)`：调用 `gcontext::Context::WithCancel(&self.Context)` 创建子取消节点；返回的新包装保留 logger，并把显式取消句柄交给调用方。
- `Context::WithLogger(&self, logger) -> Context`：替换 logger，克隆并保留现有取消上下文。
- `Context::L(&self) -> log::Logger`：返回 logger clone，而非借用或移动内部字段。
- `Context::Done(&self) -> bool`：转发 `self.Context.Done()`；当前 stub 以布尔值表示 Go `Done` channel 是否已关闭。
- `Context::Err(&self) -> Option<gcontext::Canceled>`：转发 `self.Context.Err()`；未取消时为 `None`，自身或祖先取消时为 `Some(Canceled)`。

文件没有模块级常量、trait、条件编译项或内部私有函数。所有构造器和方法均为公开 API，只有 `logger` 字段保持私有。

## 执行流程

典型导出会话的路径如下：

1. `dumpling/export/dump.rs::NewDumper` 调用 `tcontext::Background().WithCancel()`，得到整个 `Dumper` 会话共享的 `tctx` 和保存于 `Dumper.cancel` 的取消句柄。
2. 初始化步骤 `dumpling/export/dump.rs::initLogger` 调用 `WithLogger` 派生带应用 logger 的新包装；底层取消链保持不变。之后 `Dumper::L` 统一转发到 `tctx.L()`。
3. 查询、重试、锁表、HTTP 状态和 writer 等路径读取 `L()` 记录日志，并读取 `Done()` 决定是否停止。例如 `dumpling/export/conn.rs::queryRows` 把取消状态交给重试逻辑，`dumpling/export/consistency.rs::consistency_lock_setup` 在循环中取消即返回 `context canceled`。
4. 后台进度线程由 `dumpling/export/status.rs::Dumper::startLogProgress` 对传入上下文再次调用 `WithCancel`，线程持有派生 context，`LogProgressGuard` 持有 cancel handle；`stop` 先显式取消再 join 线程。
5. `dumpling/export/dump.rs::Dumper::Close` 调用会话 cancel handle；消费方的 `Done()` 随后观察到取消并退出。GC 保护更新循环 `runGCProtectionUpdater` 在退出前执行 cleanup。

`WithContext` 用于把包装切换到另一棵取消树，同时保留 logger；独立测试 `dumpling/context/parity_test.rs::contract_boundary_chaining` 验证该操作不修改原包装，也不会让无关 context 树之间错误传播取消。

## 数据与状态

`Context` 自身没有可变字段。每次 `With*` 都构造新值：

- `WithContext` 复制 logger 句柄、接收新的取消上下文；
- `WithLogger` 复制取消上下文、接收新的 logger；
- `WithCancel` 复制 logger，并让 stub 建立“新子节点指向当前父节点”的取消链。

底层 `dumpling/context/stubs.rs::Context` 使用 `Arc<AtomicBool>` 保存本节点取消位，并以 `Option<Arc<Context>>` 保存父节点；`CancelFunc` 共享子节点的原子标志。`Done` 先检查本节点，再递归检查父节点，因此父取消向子传播，子取消不会反向取消父节点。`Logger` 也通过 clone 共享其实现所管理的日志状态；`L()` 返回 clone 后，调用方仍写入同一 logger 所代表的目标，`parity_test.rs::contract_normal_paths` 和 `contract_boundary_chaining` 以 capture logger 验证了这一点。

重要不变量是：替换一个字段必须保留另一个字段；派生操作不能改变原包装；取消状态与 logger 生命周期彼此独立。

## 依赖与调用关系

向下依赖：

- `use crate::stubs as gcontext`：`Background` 调用 `gcontext::Background`；`WithCancel` 调用 `gcontext::Context::WithCancel`；`Done` 与 `Err` 分别转发同名方法，并公开 `CancelFunc`/`Canceled` 相关返回类型。
- `use astersql_dumpling_log as log`：`Background` 调用 `log::Zap`；结构体保存 `log::Logger`，`NewContext`/`WithLogger` 接收它，`L` 返回它。

向上调用者：

- `dumpling/export/dump.rs` 创建会话 context、安装 logger、在关闭时调用 cancel，并在长循环中检查 `Done`。
- `dumpling/export/status.rs` 为进度线程派生可独立停止的子 context，并同时使用 `Done` 和 `L`。
- `dumpling/export/conn.rs`、`consistency.rs` 将 `Done` 作为重试或循环的取消边界。
- `dumpling/export/http_handler.rs`、`retry.rs`、`sql.rs`、`writer_util.rs`、`metadata.rs`、`ir_impl.rs`、`block_allow_list.rs` 等生产文件通过 `L` 记录运行信息。

RustCodeGraph 的精确文件节点确认了符号与引用覆盖；其 `callers` 子命令对精确符号 ID 在本次环境中长时间无结果后被中止，因此调用边又以 crate 反向依赖搜索和上述实际调用点核验，没有把不完整的图输出当作结论。

## 错误处理与边界

本文件的构造和派生函数均不返回 `Result`，不执行 I/O，也不会主动制造业务错误。可观察的错误只有 `Err() -> Option<Canceled>`：未取消为 `None`，自身或父 context 取消为 `Some(Canceled)`；`Canceled` 的显示文本由 stub 定义为 `context canceled`。

当前实现的边界来自 `stubs.rs`：它只模拟 `Background`、`WithCancel`、`Done`、`Err`，不支持 Go context 的 deadline、timeout、value 或真正的 channel select。因此 `Done()` 是瞬时布尔读取，消费方若需等待必须自行轮询、sleep 或使用线程控制；本文件不能被描述为完整的 Go `context.Context` 实现。

`CancelFunc::call` 是显式且幂等的；丢弃句柄不会自动取消。`parity_test.rs::contract_resource_cleanup` 锁定这一行为，避免 Rust RAII 被误用成与 Go 不一致的隐式 cancel。调用方如果丢失句柄，context 仍保持未取消，除非其祖先后来取消。

## 并发与资源生命周期

包装本身没有锁和后台任务。并发安全依赖字段内部实现：取消位是 `Arc<AtomicBool>`，写入和读取使用 `Ordering::SeqCst`；父链通过 `Arc` 保活；logger clone 让不同线程共享日志实现。`Context` clone 会延长这些共享对象的生命周期，但不会复制出相互独立的取消位。

资源生命周期由上层显式管理。主会话 cancel handle 存在 `Dumper.cancel`，在 `Dumper::Close` 中通过 `take` 后调用；进度线程的句柄与 cancel handle 由 `LogProgressGuard` 同时管理，`stop`/`Drop` 都会先取消再 join。与之相对，本文件的 `Context` 没有 `Drop` 副作用；单纯丢弃包装或 `CancelFunc` 不应触发取消。取消后 logger 仍可使用，`parity_test.rs::contract_resource_cleanup` 在取消后继续调用 `L().Info` 验证了两者生命周期解耦。

父链的 `Done` 是递归读取。当前调用方式通常只形成很浅的派生链；若未来大量嵌套派生，需要注意递归检查成本和父节点因 `Arc` 链持续存活的内存影响。

## 与 Go 版本的对应关系

直接对照文件为 `dumpling/context/context.go`。字段和 helper 基本逐项对应：Go 的匿名嵌入 `gcontext.Context` 对应 Rust 的公开 `Context: gcontext::Context`；私有 `logger` 保持相同可见性；`Background`、`NewContext`、`WithContext`、`WithCancel`、`WithLogger`、`L` 保持同名与同样的字段保留规则。

主要语言差异如下：

- Go 返回 `*Context`，Rust 返回拥有所有权的 `Context` 值，并靠内部 clone 共享底层状态；外部效果仍是“派生新包装，不修改旧包装”。
- Go 匿名嵌入会提升 `Done`/`Err`，Rust 额外声明同名转发方法。
- Go `Done` 返回 channel，Rust stub 返回 `bool`；因此取消检查语义对齐，但等待机制不是等价实现。
- Go 标准库 context 支持更完整的 deadline/value 能力；Rust 本地 stub 明确只覆盖 dumpling 当前使用的最小取消子集。
- Go `cancel()` 对应 Rust `CancelFunc::call()`，两者都要求显式调用；Rust handle 的 drop 不自动 cancel。

`dumpling/context/parity_test.rs` 是本 crate 的独立 Rust 测试文件；它以四个场景对照 Go 契约：正常构造与 logger、链式字段替换、取消错误及父子传播、资源释放与显式取消。Go 上层使用证据还可见于 `dumpling/export/dump_test.go`、`status_test.go`、`consistency_test.go` 等，它们展示 `WithContext`、`WithLogger`、`WithCancel` 在实际导出测试中的组合方式。

## 扩展指南

- 新增包装字段时，应同步修改 `Background`、`NewContext` 以及全部 `With*`，明确每个派生操作是替换还是继承该字段；遗漏会破坏不可变派生不变量。
- 扩展取消语义时，真实实现位置首先是 `dumpling/context/stubs.rs`。若增加 deadline、timeout 或 value，必须先定义与 Go 的差异，再在本文件增加窄转发 API；不能仅在 wrapper 中伪造“已支持”。
- 修改 logger 返回或共享方式时，应保持 `WithContext`/`WithCancel` 不丢 logger、`WithLogger` 不切断取消链，并评估跨线程 clone 的兼容性和性能。
- 新增或修改契约测试应放在独立的 `dumpling/context/parity_test.rs`，不要把测试内嵌进 `context.rs`。至少覆盖旧值不变、父子取消方向、重复取消、句柄 drop 和取消后 logger 可用性。
- 若改变公开签名，还需同步检查直接消费者 `dumpling/export`，尤其是 `dump.rs::NewDumper`/`Close`、`status.rs::startLogProgress`、`conn.rs` 与 `consistency.rs`。将布尔 `Done` 改为可等待对象会影响现有轮询与重试接口；改变父链或原子顺序则有并发正确性和性能风险。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter dumpling/context` 找到 `context.rs`、`lib.rs`、`stubs.rs`、`parity_test.rs` 和 Go 对照；`node --file dumpling/context/context.rs` 读取完整 95 行并识别 10 个符号，文件节点报告被 32 个文件引用。
- 目标与 crate 边界：`dumpling/context/context.rs`、`dumpling/context/lib.rs`、`dumpling/context/stubs.rs`、`dumpling/context/Cargo.toml`。
- Go 对照：`dumpling/context/context.go`。
- 独立 Rust 契约测试：`dumpling/context/parity_test.rs`，覆盖 `contract_normal_paths`、`contract_boundary_chaining`、`contract_error_cancel`、`contract_resource_cleanup`。
- 上层调用证据：`dumpling/export/Cargo.toml`、`dumpling/export/lib.rs`、`dumpling/export/dump.rs::NewDumper/initLogger/Dumper::Close/runGCProtectionUpdater`、`dumpling/export/status.rs::startLogProgress/LogProgressGuard`、`dumpling/export/conn.rs::queryRows/ExecSQL`、`dumpling/export/consistency.rs::consistency_lock_setup`。
- 调用点补充搜索：`rg` 确认 `astersql-dumpling-context` 的 workspace 反向依赖与 `Background`、`WithCancel`、`WithLogger`、`Done`、`L` 的生产调用位置；未把测试文件作为生产调用依据。
- 本任务是纯文档分析，按计划不运行 Cargo；最终结构验证要求本文恰好包含上述 11 个固定二级标题。
