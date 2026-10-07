# `dumpling/context/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-dumpling-context` 的 crate 根。其 [`Cargo.toml`](Cargo.toml) 用 `[lib] path = "lib.rs"` 明确入口，并以 `package.metadata.porting.go-package = "dumpling/context"` 记录对应的 Go 包。本文件不实现取消状态或 logger 包装逻辑，而是显式装载 [`context.rs`](context.rs) 与 [`stubs.rs`](stubs.rs)，再把需要稳定暴露的符号提升到 crate 根。

该 crate 位于 Dumpling 导出编排的公共上下文层。直接生产使用者是 `dumpling/export`：例如 [`dump.rs`](../export/dump.rs) 的 `Dumper` 保存 `tcontext::Context` 和 `CancelFunc`，`NewDumper` 创建一次导出会话的可取消根上下文；[`status.rs`](../export/status.rs) 又从该上下文派生后台进度线程的子上下文。因而 `lib.rs` 是 API 门面，真实包装逻辑在 `context.rs`，Go 标准库 `context` 的最小 Rust 替身在 `stubs.rs`。

## 核心职责

1. 通过 `#[path = "stubs.rs"] mod stubs` 和 `#[path = "context.rs"] mod context` 固定两个私有子模块的来源。
2. 通过 `pub use context::*` 导出 Dumpling 自己的 `Context`、`Background` 和 `NewContext`，让调用方只依赖 crate 根路径。
3. 选择性导出标准库风格替身：`Context`、`Background`、`WithCancel` 分别改名为 `GoContext`、`GoBackground`、`GoWithCancel`，避免与 Dumpling 包装层同名；`CancelFunc` 与 `Canceled` 保持原名。
4. 仅在 `cfg(test)` 下装载独立的 [`parity_test.rs`](parity_test.rs)，使测试逻辑不进入生产构建，也不与生产源码混放。
5. 用 crate 级 `allow` 接纳迁移代码中的 Go 风格命名及暂未使用项；这些属性只影响编译诊断，不改变可见性、取消行为或线程安全语义。

本文件不负责创建线程、轮询取消、写日志或处理业务错误；它只定义模块边界与公共 API 形状。

## 主要符号

- `mod stubs`：私有模块，定义 Go `context` 最小子集。虽然模块不可从 crate 外命名，其选定公开项会被 `lib.rs` 再导出。
- `mod context`：私有模块，定义 Dumpling 包装类型 `Context` 以及 `Background()`、`NewContext(...)`。
- `pub use context::*`：把 `context.rs` 中所有公开项提升到 crate 根。目前关键 API 是 `Context`、`Background`、`NewContext`。
- `GoContext`：`stubs::Context` 的公开别名，内部包含本节点取消位和可选父节点。
- `GoBackground`：`stubs::Background` 的公开别名，建立无父节点、未取消的根上下文。
- `GoWithCancel`：`stubs::WithCancel` 的公开别名，从传入的 `GoContext` 派生子节点和 `CancelFunc`。
- `CancelFunc`：可克隆的显式取消句柄；`call()` 以原子写将关联节点置为已取消。
- `Canceled`：`Err()` 在取消后返回的错误类型，显示文本固定为 `context canceled`。
- `mod parity_test`：测试配置下的私有模块，验证上述 crate 根 API 与 Go 契约的对应关系。

这里有意不做 `pub mod context` 或 `pub mod stubs`：外部兼容边界是再导出后的 crate 根，而不是内部文件布局。

## 执行流程

一次 Dumpling 导出会话的主路径如下：

1. `dumpling/export/dump.rs::NewDumper` 调用 `tcontext::Background().WithCancel()`。
2. 门面导出的 `Background` 进入 `context.rs::Background`，组合 `stubs::Background()` 与 `dumpling/log::Zap()`，得到未取消且使用 nop logger 的 Dumpling `Context`。
3. `Context::WithCancel` 调用 `stubs::Context::WithCancel(&self.Context)`，为子节点创建独立 `AtomicBool`，保存父上下文，并克隆原 logger；调用方同时取得 `CancelFunc`。
4. `dump.rs::initLogger` 通过 `Context::WithLogger` 生成保留取消链、替换 logger 的新包装值，再赋回 `Dumper.tctx`。
5. 导出流程把 `&Context` 传入各子系统；它们通过 `L()` 获取 logger，通过 `Done()`/`Err()`观察取消。
6. `Dumper::Close` 对保存的 `CancelFunc` 调用 `call()`，使会话上下文可见取消状态，再关闭 HTTP、PD 与数据库资源。

后台进度路径由 `status.rs::startLogProgress` 展示：它从传入上下文再次 `WithCancel`，把子上下文移动到线程中；线程在 `runLogProgressWithTicks` 循环检查 `Done()`，guard 停止时先调用 cancel，再 `join`。`lib.rs` 不执行这些步骤，但保证所有调用都解析到同一组公开类型和函数。

## 数据与状态

`lib.rs` 自身没有字段、全局变量或缓存。经它公开的数据状态分成两层：

- `context.rs::Context` 保存公开的 `Context: GoContext` 和私有 `logger: astersql_dumpling_log::Logger`。`WithContext` 只替换底层上下文，`WithLogger` 只替换 logger，`WithCancel` 派生取消子节点并保留 logger；三者都返回新值，不原地修改旧包装。
- `stubs.rs::Context` 保存 `Arc<AtomicBool>` 取消位和 `Option<Arc<Context>>` 父链。克隆上下文会共享相同取消位及父链；克隆 `CancelFunc` 也会共享同一取消位。

公开字段名 `Context` 和大写函数名保留 Go 移植接口的辨识度。`Go*` 别名则把标准库风格上下文与 Dumpling 包装类型明确区分，避免 `Context`/`Background` 名称冲突。

## 依赖与调用关系

- crate 边界：`dumpling/context/Cargo.toml` 只声明一个直接依赖 `astersql-dumpling-log = { path = "../log" }`，没有 feature；根 workspace 把 `dumpling/context` 列为成员。
- 内部下游：`lib.rs -> context.rs` 提供 Dumpling 包装 API；`context.rs -> stubs.rs` 提供取消状态；`context.rs -> dumpling/log` 提供 `Logger` 与默认 `Zap()`。
- 上游生产 crate：`dumpling/export/Cargo.toml` 以路径 `../context` 依赖本 crate。RustCodeGraph 将 `context.rs` 标为被 32 个文件使用，代表性入口包括 `export/dump.rs`、`export/status.rs` 及其独立测试。
- 主导出链：`export/dump.rs::NewDumper -> Background -> Context::WithCancel`；`initLogger -> Context::WithLogger`；`Dumper::Close -> CancelFunc::call`。
- 后台线程链：`export/status.rs::startLogProgress -> Context::WithCancel -> thread::spawn -> runLogProgressWithTicks -> Context::Done`，停止侧为 `LogProgressGuard::stop -> CancelFunc::call -> JoinHandle::join`。
- 测试链：`parity_test.rs::go_rust_public_contract_matches` 从 crate 根导入 `Background`、`GoBackground`、`GoWithCancel`、`NewContext`，直接验证门面可见性及包装、替换、取消和清理契约。

RustCodeGraph 对精确 `callers/callees` 请求未返回符号边，因此这里的跨 crate 调用关系以其文件级 `used by` 结果、Cargo 路径依赖和上述入口源码共同核对，没有据此推断未出现的调用者。

## 错误处理与边界

`lib.rs` 不产生 `Result` 或错误。经门面公开的可观察边界是：

- 未取消时 `Done()` 为 `false`、`Err()` 为 `None`；本节点或任一祖先取消后，`Done()` 为 `true`、`Err()` 为 `Some(Canceled)`。
- `Canceled` 的显示文本固定为 `context canceled`；当前没有 deadline exceeded、取消原因或其他错误类别。
- `CancelFunc::call()` 是幂等原子写，重复调用不会 panic；丢弃句柄而未调用不会自动取消。这由 `parity_test.rs::contract_error_cancel` 和 `contract_resource_cleanup` 锁定。
- `WithContext` 可切换到完全不同的取消树，并保留 logger；原包装仍绑定旧树。`WithLogger` 不改变取消树。
- `stubs.rs` 明确只模拟当前 Dumpling 所需的 `Background`、`TODO`、`WithCancel`、`Done`、`Err`，不提供 Go channel、deadline、value 或 cause 语义。把它当作完整 Go `context.Context` 会超出当前实现事实。
- 父链的 `Done()` 使用递归遍历；极深派生链会增加查询成本和调用栈深度。当前直接证据没有表明生产路径会构造异常深的链。

## 并发与资源生命周期

取消位使用 `Arc<AtomicBool>` 与 `Ordering::SeqCst`，所以克隆到其他线程的上下文能观察到 `CancelFunc::call()`。父子传播不是主动通知：子节点每次调用 `Done()` 都先读自身原子位，再沿父链递归查询。它也没有阻塞等待通道，因此消费者必须像 `status.rs::runLogProgressWithTicks` 一样在自己的循环边界轮询。

`CancelFunc` 的 `Drop` 没有副作用，取消必须显式发生；这对 Go 语义对齐很重要，也意味着拥有者应在所有退出路径执行 `call()`。`Dumper` 将根句柄保存到 `cancel: Option<CancelFunc>`，`Close` 用 `take()` 保证只消费一次；`LogProgressGuard` 则在 `stop`/`Drop` 中取消并等待线程退出。

父节点由子节点通过 `Arc` 持有，子节点存活期间父链不会被释放。logger 通过 clone 在派生包装之间共享其内部资源，但取消不会使 logger 失效；parity 测试验证取消后仍可调用 `L().Info(...)`。本 crate 自身不创建线程、不持有文件或网络句柄，也没有异步 runtime 依赖。

## 与 Go 版本的对应关系

直接对照文件是 [`context.go`](context.go)。两版都将一个标准上下文和一个 Dumpling logger 组合为 `Context`，并提供 `Background`、`NewContext`、`WithContext`、`WithCancel`、`WithLogger`、`L`。字段保留规则相同：替换一侧时保留另一侧，派生取消上下文时保留 logger，旧包装不被原地修改。

主要实现差异如下：

- Go `Context` 嵌入标准库 `context.Context` 接口并通过方法提升获得 `Done`/`Err`；Rust 用具体 `stubs::Context` 作为公开字段，并在包装层显式转发为布尔 `Done()` 和 `Option<Canceled>`。
- Go `Background`/各 `With*` 返回指针；Rust 返回拥有型、可克隆值，以 clone 保留共享状态。
- Go 的 `Done()` 是可阻塞接收的 channel；Rust 的 `Done()` 只是即时布尔查询，需要调用方轮询。
- Go 标准库支持 deadline、value 和多种错误；Rust stub 只覆盖取消传播，`TODO()` 与 `Background()` 等价。
- Rust crate 根额外导出 `GoContext`、`GoBackground`、`GoWithCancel`，供移植接线与 parity 测试显式操作底层 Go 风格层；Go 包不需要这些别名。

Go 测试证据包括 `dumpling/export/dump_test.go` 与 `consistency_test.go` 对 `Background().WithLogger(...).WithCancel()`、`WithContext(...)` 的使用；Rust 对应的直接契约集中在 `dumpling/context/parity_test.rs`，上层回归则分布于 `dumpling/export/*_test.rs`。

## 扩展指南

- 新增 Dumpling 包装能力时，优先在 `context.rs::Context` 增加方法；若它构成公共 API，当前 `pub use context::*` 会自动导出。同步更新 `parity_test.rs`，不要把测试内嵌进生产文件。
- 新增底层标准 context 能力时，在 `stubs.rs` 实现，并在 `lib.rs` 选择性再导出；应继续使用 `Go*` 前缀处理会与包装层冲突的名称。若引入 deadline/value/cause，必须明确线程等待、错误枚举与 Go 兼容边界，不能仅增加空桩。
- 改动取消语义时至少扩展 `contract_error_cancel` 与 `contract_resource_cleanup`，覆盖父取消传播、重复取消、未调用句柄即 drop、取消后 logger 可用性；同时检查 `export/status_test.rs` 和 `export/dump_test.rs` 的后台停止及关闭路径。
- 改动 `WithContext`/`WithLogger` 时扩展 `contract_boundary_chaining`，确保替换字段不会污染另一字段，也不会原地改变旧值；上层 `export/consistency_test.rs` 是真实组合使用证据。
- 公共符号名、Go 风格大小写和 crate 根路径属于迁移兼容面。收紧 crate 级 `allow` 前需先处理所有再导出 API；把私有模块直接公开会扩大承诺范围，应单独评估。
- 性能风险主要是高频 `Done()` 沿深父链递归以及 `SeqCst` 原子开销；正确性风险主要是漏掉显式 cancel、破坏父子传播或让 logger 与取消生命周期错误绑定。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter dumpling/context` 确认区域内有 `lib.rs`、`context.rs`、`stubs.rs`、`parity_test.rs` 和 `context.go`。
- RustCodeGraph `node --file dumpling/context/lib.rs`：确认 31 行 crate 根只包含两个生产模块、两组再导出、crate 级 allow 与测试模块挂载。
- RustCodeGraph `node`：已读 `context.rs`、`stubs.rs`、`parity_test.rs`，核对包装字段、构造/派生方法、原子取消、父链传播和四组契约测试；`context.rs` 的文件级结果显示 32 个使用文件。
- RustCodeGraph 精确 `callers/callees` 查询已执行，但未产生可用输出；调用链以索引读取的 `dumpling/export/dump.rs`、`status.rs`、`consistency_test.rs`、`dump_test.rs` 直接核验并明确记录此限制。
- 已读配置与生产路径：`dumpling/context/Cargo.toml`、根 `Cargo.toml` 的 workspace 成员、`dumpling/export/Cargo.toml`、`dumpling/export/dump.rs`、`dumpling/export/status.rs`。
- 已读 Go 对照与测试引用：`dumpling/context/context.go`、`dumpling/export/dump_test.go`、`dumpling/export/consistency_test.go`；已读独立 Rust 测试：`dumpling/context/parity_test.rs`、`dumpling/export/consistency_test.rs`、`dumpling/export/dump_test.rs`。同目录没有独立 Go `context_test.go`。
- 本任务是纯文档分析，按计划未运行 Cargo。结构验证用于确认目标文件存在且恰有十一个规定章节，并人工复核文档能回答该门面为何存在、运行链如何经过它以及安全扩展时应修改和测试哪些位置。
