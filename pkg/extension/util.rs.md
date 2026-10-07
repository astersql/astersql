# `pkg/extension/util.rs`

## 文件定位

`pkg/extension/util.rs` 是 `astersql-extension` crate 的通用边界层。`pkg/extension/lib.rs` 以 `pub mod util` 声明它，再通过 `pub use util::*` 将其符号从 crate 根导出。文件本身只直接使用标准库 `std::fmt`；`pkg/extension/Cargo.toml` 没有为它引入专用外部依赖，但其公开类型被同 crate 的认证、函数、Manifest、注册表和会话接口共享。

该文件不负责扩展的发现或调用，而是提供三项跨模块协议：统一字符串错误 `ExtensionError`、可查询取消状态的 `ExtensionContext`、可组合的一次性清理回调 `ClearFunc`。`clearFuncBuilder` 则是 Manifest 资源注册和全局 registry 组装两层共用的回滚/关闭顺序器。

## 核心职责

1. `ExtensionError` 把扩展边界的错误文案封装为实现 `Display` 和 `std::error::Error` 的 Rust 错误类型，使 trait 对象回调可以使用统一的 `Result` 签名。
2. `ExtensionContext` 定义扩展执行环境的最小取消查询协议；默认实现返回 `false`，容许不关心取消的上下文仅声明实现 trait。
3. `ClearFunc` 用所有权约束表示只能执行一次的清理动作，并要求闭包可跨线程移动和共享。
4. `clearFuncBuilder` 只收集成功产生的清理回调，最后将它们组合成一个按收集先后执行的 `ClearFunc`。这个顺序是 Go 实现的 FIFO 语义，不是常见的 LIFO 栈式回滚。

## 主要符号

- `pub struct ExtensionError(String)`：内部字符串字段为私有；派生 `Clone + Debug + Eq + PartialEq`。`ExtensionError::new(message: impl Into<String>)` 是唯一构造入口，可接受 `&str`、`String` 等。`Display::fmt` 原样写出消息，`Error` 使用默认行为，没有自定义 `source`。
- `pub trait ExtensionContext`：只有 `fn is_cancelled(&self) -> bool`，默认为 `false`。`pkg/extension/extensionimpl/bootstrap.rs::bootstrapContext` 覆盖它并委托给底层 `kv::Context`；`pkg/expression/extension.rs::extensionFnContext` 未覆盖，因而使用默认值。
- `pub type ClearFunc = Box<dyn FnOnce() + Send + Sync + 'static>`：拥有闭包的 trait object。`FnOnce` 表明清理可以消费捕获状态，同时在类型层面防止同一回调被重复调用。
- `pub struct clearFuncBuilder { clears: Vec<ClearFunc> }`：类型可公开命名，内部队列为私有，并由 `Default` 生成空构建器。它保留 Go 的命名以便对照，因 crate 根的 lint 设置而允许非 Rust 驼峰风格。
- `DoWithCollectClear<F>(&mut self, function: F) -> Result<(), ExtensionError>`：同步且恰好执行一次 `function`。`Err` 立即向上传播；`Ok(None)` 不改变队列；`Ok(Some(clear))` 才把回调追加到尾部。
- `Build(self) -> ClearFunc`：消费构建器及整个 `Vec`，返回遍历该向量并依次调用每个元素的单一闭包。空构建器产生的回调是有效的 no-op。

## 执行流程

Manifest 层的主链是 `pkg/extension/manifest.rs::newManifestWithSetup`。它先创建空 `clearFuncBuilder`，再按固定顺序完成资源设置：将 Manifest 自身的 close 回调放入队列，逐项注册动态权限并收集移除回调，逐项注册系统变量并收集注销回调，最后注册扩展函数并收集移除回调。所有步骤成功时，`Build` 产生返回给上层的 Manifest 清理函数；任一后续步骤失败时，该函数被立即执行，只清理错误前已成功登记的资源。

Registry 层的主链是 `pkg/extension/registry.rs::registry::doSetup`。它按排序后的扩展名构建 Manifest，每个成功的 `newManifestWithSetup` 返回一个已组合的清理回调，再被外层 `clearFuncBuilder` 收集。后续工厂失败时，外层 `Build()()` 按已成功 Manifest 的顺序回滚；全部成功时，组合回调保存到 `registryState.close`，由 `registry::Reset` 使用 `take()` 取出并执行一次。

`DoWithCollectClear` 自身不会在收集函数返错时自动执行旧回调；它只保留已有队列并返回错误。真正的失败回滚是上述两个调用方在错误分支显式执行 `Build()()` 实现的。

## 数据与状态

`ExtensionError` 拥有一个不可直接读取的 `String`，没有错误码、分类、backtrace 或嵌套原因。克隆错误会复制消息字符串，相等性也只由该字符串决定。

`clearFuncBuilder` 的唯一可变状态是 `Vec<ClearFunc>`。收集为尾部追加，不去重，不重排，也不保留资源名或错误上下文。`Build(self)` 转移所有权后，原构建器不能再收集。组合回调执行时，每个子回调也被从向量中移出并消费，这与 `FnOnce` 的单次语义一致。

`ExtensionContext` 不保存任何状态；取消来源由实现者决定。默认 `false` 不会记住或传播取消，因此需要真实取消语义的上下文必须像 `bootstrapContext` 一样覆盖该方法。

## 依赖与调用关系

crate 边界由 `pkg/extension/Cargo.toml` 和 `pkg/extension/lib.rs` 确认：`util` 是 `astersql-extension` 的公开模块，四个主要符号通过 crate 根再导出。本文件的实现仅依赖 `std::fmt`、`Vec`、`Box` 和标准错误 trait，不直接使用 Cargo 中的其他 crate。

上游生产调用边主要有：

- `manifest.rs::newManifestWithSetup` 创建 `clearFuncBuilder`，四处收集 Manifest close、动态权限、系统变量和扩展函数的反向操作，并在成功返回或失败回滚时调用 `Build`。
- `registry.rs::registry::doSetup` 再聚合各 Manifest 的 `ClearFunc`，用于多扩展失败回滚和 `Reset` 关闭。
- `auth.rs`、`function.rs`、`manifest.rs`、`registry.rs`、`session.rs` 把 `ExtensionError` 用于边界签名；`pkg/expression/extension.rs` 也用它表达扩展函数注册与求值失败。
- `function.rs::FunctionContext` 和 `manifest.rs::BootstrapContext` 继承 `ExtensionContext`；具体实现分别出现在 `pkg/expression/extension.rs` 与 `pkg/extension/extensionimpl/bootstrap.rs`。

RustCodeGraph 已索引目标文件的 9 个符号，并把其标记为被 8 个文件使用。由于常见方法名的精确 `callers/callees` 查询未返回可用边，上述调用关系又用限定在 `pkg/extension` 和 `pkg/expression` 的源码搜索逐项核对。

## 错误处理与边界

`ExtensionError::new` 不修改传入文案，`Display` 也不添加前缀。这保留了 Go 测试依赖的具体错误字符串，但将外部错误以 `to_string()` 映射为它时会丢失原错误类型和错误链。本类型也不提供错误码；调用者不应在需要结构化分支时仅依赖文案猜测。

`DoWithCollectClear` 使用 `?` 传播收集闭包的错误，所以失败的这一步不会入队，但之前的回调仍留在 builder 中。调用方如果直接丢弃 builder，这些闭包只会被 drop，不会自动执行；因此错误分支显式 `Build()()` 是必须维持的上层不变量。

`ClearFunc` 无返回值，也无法报告可恢复错误。`Build` 没有 `catch_unwind`；任一子回调 panic 都会中断遍历，后续清理不会被执行。它也不保证函数式幂等；单次执行保障来自 `FnOnce` 和 registry `take()`，而不是对外部资源状态的检查。

## 并发与资源生命周期

`ClearFunc` 要求 `Send + Sync + 'static`，因此其捕获数据不能借用短生命期引用，并必须满足跨线程约束。然而 `clearFuncBuilder` 不使用锁或原子操作，收集期依赖独占的 `&mut self`；它不支持多个线程同时追加。`registry::doSetup` 在 registry 写锁保护的状态上调用它，并发串行化是上层提供的。

子回调的资源生命周期从 `DoWithCollectClear` 将其放入向量开始，转移到 `Build` 返回的组合闭包中，最终在该闭包被调用时消费。Manifest 层的组合闭包又可作为 registry 层的一个子回调，形成两级所有权链。如果对应 builder 或组合闭包未被调用就被 drop，Rust 会释放闭包及其捕获值，但不会执行闭包体。

本文件不创建任务、线程、通道或事务，也没有 `Drop` 实现。`ExtensionContext::is_cancelled` 只是同步查询；它不阻塞、不订阅取消通知，也不主动终止操作。

## 与 Go 版本的对应关系

`pkg/extension/util.go` 的 `clearFuncBuilder` 是 Rust 构建器的直接语义基准。Go `DoWithCollectClear` 也是先调用函数，错误则直接返回，非 `nil` 清理函数才 append；Go `Build` 也是从切片首到尾执行。Rust 用 `Result<Option<ClearFunc>, ExtensionError>` 表达 Go 的 `(func(), error)`，用 `Option` 取代 `nil`，并用 `FnOnce` 和所有权强化单次执行。

Go `pkg/extension/manifest.go::newManifestWithSetup` 和 `registry.go::doSetup` 也在错误分支执行 `clearBuilder.Build()()`，在成功分支保存组合回调。`pkg/extension/registry_test.go::TestRegisterExtensionWithClose` 验证 Reset 调用 close 一次以及后续工厂错误会自动清理早先扩展；Rust `registry_test.rs` 保留了同样的回归意图。

`ExtensionError`、`ExtensionContext` 和显式的 `ClearFunc` 别名不存在于 Go `util.go` 中，是 Rust 为静态 trait/trait-object 边界增加的表示。它们不应被描述成 Go 同文件中有完全同型实现；可对照的是其使用处的普通 `error`、`context.Context` 和 `func()` 契约。

## 扩展指南

新增需要失败回滚或 Reset 释放的注册资源时，应在实际注册成功之后立即用 `DoWithCollectClear` 登记精确的反向操作，并确保所有提前返回错误的路径最终经过 `Build()()`。不要默认该构建器是 defer 栈；若不能改变 Go 协议，就必须保留 FIFO 清理顺序。

如需让清理返回错误、继续执行 panic 后的其他清理，或改为 LIFO，都会同时改变 `ClearFunc`、`Build`、Manifest 和 registry 的协议，应先评估 Go 兼容性和已注册资源之间的顺序依赖。长清理链为线性执行，新增高延迟回调时还应评估 Setup 失败和 Reset 的同步阻塞时间。

修改 `ExtensionError` 时应保持现有 `Display` 文案，并检查 `auth.rs`、`function.rs`、`manifest.rs`、`registry.rs`、`session.rs` 以及 `pkg/expression/extension.rs` 的所有 `Result` 边界。修改 `ExtensionContext` 时要同步其子 trait 和两个已知具体实现，并明确默认取消语义是否仍安全。

回归测试应继续放在独立文件，不嵌入 `util.rs`：直接收集顺序可扩展 `pkg/extension/auth_1_aster_unit_test.rs::clear_builder_runs_in_go_collection_order`；Manifest 内部资源顺序与局部回滚应扩展 `manifest_test.rs`；多扩展失败与 Reset 应扩展 `registry_test.rs`。若语义源自 Go，还应对照 `util.go`、`manifest.go`、`registry.go` 及 `registry_test.go`。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/extension` 确认 `util.rs`、Go 对照和独立测试均在索引中。
- RustCodeGraph 源码证据：`node --file pkg/extension/util.rs` 读取完整 81 行和 9 个符号；另读取 `manifest.rs::newManifestWithSetup`、`registry.rs::registry::doSetup` 及 `auth_1_aster_unit_test.rs::clear_builder_runs_in_go_collection_order`。`query` 确认 Rust/Go 两个 `DoWithCollectClear` 定义以及 `ExtensionError` 在 extension/expression 边界的使用者。
- RustCodeGraph 对常见方法名的 `callers/callees` 未返回可用结果；按技能的回退规则，用 `rg` 限定搜索 `pkg/extension/**/*.rs`、`pkg/expression/extension.rs` 和 Go 对照，核实了构建器、错误和上下文的直接定义/调用边。
- crate 与 Go 对照：`pkg/extension/Cargo.toml`、`pkg/extension/lib.rs`、`pkg/extension/util.go`、`pkg/extension/manifest.go`、`pkg/extension/registry.go`。
- Rust 测试证据：`pkg/extension/auth_1_aster_unit_test.rs::clear_builder_runs_in_go_collection_order`；`manifest_test.rs::manifest_options_register_resources_and_clear_in_go_order`、`setup_error_rolls_back_only_resources_registered_before_the_error`、`invalid_system_variables_match_go_errors_and_rollback_prior_setup`；`registry_test.rs::registry_reset_runs_close_once` 和 `registry_setup_failure_rolls_back_initialized_extensions`。Go 回归基准是 `pkg/extension/registry_test.go::TestRegisterExtensionWithClose`。
- 人工复核结论：文档已说明该文件的存在原因、FIFO 收集/组合流程、错误与 panic 边界、两级资源生命周期以及安全扩展所需同步的独立测试。本任务为纯文档分析，按计划不运行 Cargo；最终验收使用固定十一章结构检查。
