# `pkg/extension/registry.rs`

## 文件定位

`registry.rs` 是 `astersql-extension` crate 的进程级扩展注册入口。crate 由 `pkg/extension/Cargo.toml` 定义，`pkg/extension/lib.rs` 通过 `pub mod registry` 和 `pub use registry::*` 暴露本文件 API。它位于“扩展声明”与“扩展消费”之间：注册方提交 `Option` 或延迟工厂，本文件把它们转换成有序的 `Manifest` 集合；启动流程和 bootstrap、会话、鉴权等消费者再从 `Extensions` 读取结果。

本文件本身不定义扩展选项如何改变系统变量、动态权限、函数或认证插件；这些副作用由 `pkg/extension/manifest.rs` 的 `newManifestWithSetup` 执行。本文件负责安排构建顺序、缓存结果、聚合清理函数，并给这些操作提供进程内同步边界。

## 核心职责

1. 用 `globalRegistry: LazyLock<registry>` 保存全进程唯一注册表，并通过顶层 `RegisterFactory`、`Register`、`Setup`、`GetExtensions`、`Reset` 提供门面。
2. 在 `registry::RegisterFactory` 中拒绝 Setup 后注册、空名称和重复名称；同时维护排序后的 `extensionNames`，使 Manifest 构建顺序不受注册先后影响。
3. 在 `registry::doSetup` 中按名称调用扩展工厂及 `newManifestWithSetup`，把成功结果组装为 `Extensions`，并缓存到后续调用可共享的 `Arc` 中。
4. 当任一扩展初始化失败时，执行此前成功扩展的清理链；成功时把同一清理链留给 `Reset`。
5. 通过 `RwLock<registryState>` 串行化注册、首次构建与复位，并允许 Setup 完成后的读取走读锁快速路径。

## 主要符号

- `ExtensionFactory = Arc<dyn Fn() -> Result<Vec<Option>, ExtensionError> + Send + Sync>`：可共享、可跨线程调用的扩展工厂。工厂返回应用于单个 Manifest 的选项列表，或返回统一的 `ExtensionError`。
- `registryState`：私有状态，包含名称到工厂的 `factories`、已排序的 `extensionNames`、是否已完成构建的 `setup`、缓存的 `Option<Arc<Extensions>>`，以及成功构建后用于复位的 `Option<ClearFunc>`。
- `registry`：仅含一个 `RwLock<registryState>`。其 `new`、`doSetup` 为内部实现；`Setup`、`Extensions`、`RegisterFactory`、`Reset` 虽声明为 `pub`，但 `registry` 类型本身未从本模块公开，因此正常外部入口仍是同名顶层函数。
- `registry::RegisterFactory`：注册的唯一校验与写入点。成功后同时更新哈希表和排序名称向量。
- `registry::doSetup`：状态机核心。它处理幂等、空集合、逐项初始化、失败回滚、结果缓存和清理链保存。
- `globalRegistry`：`LazyLock` 延迟创建的进程级单例，首次访问时调用 `registry::new`。
- 顶层 `RegisterFactory` / `Register`：前者接收延迟工厂；后者把固定 `Vec<Option>` 包装成克隆该向量的工厂后复用前者。
- 顶层 `Setup` / `GetExtensions` / `Reset`：分别委托给全局注册表的显式构建、惰性获取和测试复位操作。

## 执行流程

注册阶段从 `Register` 或 `RegisterFactory` 开始。`Register` 捕获固定选项列表，每次工厂调用都返回其克隆；`RegisterFactory` 获取写锁，先检查 `setup`，再检查名称非空且未出现在 `factories` 中。通过校验后，它写入工厂，将名称加入 `extensionNames`，并立即按字符串自然顺序排序。

显式构建由 `Setup` 获取写锁后调用 `doSetup`。惰性读取由 `Extensions` 先获取读锁：若 `setup` 已为真，直接克隆缓存的 `Option<Arc<Extensions>>`；否则释放读锁、获取写锁，再调用 `doSetup`。`doSetup` 必须再次检查 `setup`，因为读锁释放至写锁获得之间，另一线程可能已经完成初始化。

`doSetup` 的分支如下：

1. 已 setup：原样克隆并返回缓存值，不重复调用工厂。
2. 没有工厂：写入 `extensions = None`、`setup = true` 并返回 `Ok(None)`。
3. 有工厂：按 `extensionNames` 顺序取出工厂并调用 `newManifestWithSetup(name, factory)`。每个成功结果将 `Arc<Manifest>` 加入列表，并将该 Manifest 的聚合清理函数收入外层 `clearFuncBuilder`。
4. 某项失败：立即执行外层构建器中此前收集的清理函数并返回原错误；失败路径不会把 `setup` 置为真，也不会保存 `close`，因此修正外部条件后可再次调用 Setup。
5. 全部成功：用 `Extensions::from_manifests` 创建集合，缓存其 `Arc`，设置 `setup = true`，并把最终清理链保存到 `close`。

`Reset` 持有写锁，先 `take` 并执行 `close`，再清空工厂、名称和缓存，最后恢复 `setup = false`。由于 `take` 移除了回调，连续 Reset 不会重复执行同一条清理链。

## 数据与状态

`factories` 是权威名称集合和工厂存储；`extensionNames` 是同一组键的有序视图。`RegisterFactory` 在同一写锁临界区同时维护两者，`doSetup` 因而可以把名称存在于哈希表视为不变量，并在违背时以 `expect("registered extension name must have a factory")` 终止。

状态可概括为：初始/Reset 后为 `setup = false`、无缓存；成功的空构建为 `setup = true`、`extensions = None`；成功的非空构建为 `setup = true`、`extensions = Some(Arc<_>)`、`close = Some(_)`；构建失败则保留未 setup 状态和工厂集合。`Arc<Extensions>` 允许调用方持有已构建快照；Reset 清除注册表里的 Arc 不会强制销毁调用方仍持有的集合，但相关全局注册副作用已由清理链撤销。

排序发生在每次注册后，构建复杂度包含重复排序成本；当前设计优先保证小规模扩展集合的确定性。Manifest 的消费顺序会进一步影响 bootstrap 钩子执行顺序，以及 `Extensions::GetAuthPlugins` 中同名插件“后者覆盖前者”的结果。

## 依赖与调用关系

直接下游依赖有三组：`manifest::{Option, newManifestWithSetup}` 负责把工厂结果应用到 Manifest 并注册资源；`util::{ClearFunc, ExtensionError, clearFuncBuilder}` 提供错误和清理聚合；`extensions::Extensions` 保存最终 Manifest 列表。标准库的 `HashMap`、`Arc`、`LazyLock`、`RwLock` 分别承担查重存储、共享所有权、单例延迟初始化和并发保护。

RustCodeGraph 确认的本文件内部调用边包括 `GetExtensions → registry::Extensions`、`Setup → registry::doSetup`、`Register → RegisterFactory` 和 `doSetup → Extensions::from_manifests`。图索引没有识别闭包内的 `doSetup → newManifestWithSetup` 与 `clearFuncBuilder` 调用；这些边由 `registry.rs` 第 126–151 行的直接源码核验。

应用侧直接证据包括：`cmd/tidb-server/main.rs::setupExtensions` 先调用 `extension::Setup` 再读取 `extension::GetExtensions`；`pkg/extension/extensionimpl/bootstrap.rs::Bootstrap` 通过 `GetExtensions` 取得集合，无扩展时短路，否则调用 `Extensions::Bootstrap`。RustCodeGraph 还识别到 `pkg/extension/auth_1_aster_unit_test.rs` 对这些全局入口的调用；普通文本检索补充了 `registry_test.rs`、`main_test.rs`、`bootstrap_test.rs` 等测试调用点。

`pkg/extension/Cargo.toml` 没有为 registry 声明条件 feature；该模块总是由 crate 根编译。`serial_test` 仅为开发依赖，用于串行化会修改全局注册表的测试。

## 错误处理与边界

注册错误在持锁期间、修改状态之前返回，三个稳定消息分别覆盖已 Setup、空名称和重复名称。`pkg/extension/registry_test.rs` 明确断言空名与重复名消息与 Go 一致；Setup 后迟到注册只断言为错误，因此修改其文案的兼容风险低于前两者，但仍可能影响未列出的调用方。

扩展工厂、选项资源注册及认证校验错误由 `newManifestWithSetup` 以 `ExtensionError` 原样传播。该函数先回滚当前 Manifest 内已经注册的资源；外层 `doSetup` 再回滚之前 Manifest 的资源，因此错误边界覆盖“当前扩展内部部分成功”和“更早扩展全部成功”两层。`registry_setup_failure_rolls_back_initialized_extensions` 验证后一层清理恰好发生一次。

锁中毒不是对外错误：所有读写锁都用 `poisoned.into_inner()` 继续访问状态。这样避免因另一个线程持锁 panic 而永久拒绝服务，但也意味着代码承担继续使用可能处于中间状态数据的风险。真正违反 `extensionNames`/`factories` 一致性时，`expect` 会 panic，而不是返回 `ExtensionError`。

空注册表是一个需要保留的 Rust 边界：`GetExtensions` 返回 `Ok(None)`。Go 的 `*Extensions` 为 nil 时，`len(extensions.Manifests())` 仍可得到 0；Rust 无法对 `None` 直接调用方法，因此调用方必须像 `extensionimpl::Bootstrap` 那样显式短路。

## 并发与资源生命周期

所有可变注册状态都位于同一个 `RwLock` 下。注册、Setup、首次惰性 Setup 和 Reset 持有写锁，所以工厂执行及其资源注册期间不会与其他注册表操作交错。已经 Setup 的 `Extensions` 读取只持有短暂读锁并克隆 Arc。该实现没有在工厂执行期间释放写锁，因此工厂若同步回调同一全局 registry 的注册、Setup、GetExtensions 或 Reset，会造成不可重入等待；扩展工厂应只构造选项，不应重入注册表。

`ExtensionFactory` 要求 `Send + Sync`，`ClearFunc` 也要求 `Send + Sync + 'static`，从类型层面允许注册表安全地作为进程级共享对象。工厂被 `Arc::clone` 后移入一次性闭包；固定选项版本则依靠 `Vec<Option>: Clone` 支持可能的 Setup 重试。

成功构建后，清理链一直由 `registryState.close` 持有，只有 `Reset` 执行。构建失败时，清理链立即执行且不保存。`clearFuncBuilder::Build` 按收集顺序而非逆序执行回调，这是与 Go 一致的明确语义。生产入口中未看到自动进程退出清理；当前 `Reset` 的注释和测试均将其定位为测试复位 API。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/extension/registry.go`：Go 的 `sync.RWMutex` 对应 `RwLock<registryState>`，两个集合与 `setup/extensions/close` 字段保持同一职责；注册校验、名称排序、双重检查式惰性 Setup、逐 Manifest 构建、失败回滚和 Reset 顺序均被保留。

所有权表达存在必要差异：Go 用 `*Extensions` 和 `func()`，Rust 用 `Option<Arc<Extensions>>` 与一次性的 `ClearFunc`；Go 的可变参数 `Register(name, options ...Option)` 映射为 Rust 的 `Register(name, Vec<Option>)`；Go 工厂是普通函数值，Rust 工厂增加 `Send + Sync` 并包在 Arc 中。Go 的 nil map 通过首次注册时分配，Rust 的 HashMap 由 `Default` 初始化。

空集合的可观察表达不同但意图一致：Go `doSetup` 返回 nil `*Extensions`，其测试利用 nil 接收者上的 `Manifests` 获得空切片；Rust 返回 `None`，Rust 测试与 bootstrap 消费者显式检查该值。错误消息、按名称排序和 Reset 清理一次的行为由两侧 `registry_test` 直接对照。Rust 测试还单独固定了失败时回滚先前扩展的语义。

## 扩展指南

新增一种注册入口时，应最终复用 `registry::RegisterFactory`，不要绕过名称校验或同时维护 `factories`/`extensionNames` 的临界区。若需要改变命名规则、允许 Setup 后动态注册或改变排序规则，首先评估 Manifest 顺序对 bootstrap、会话扩展与同名认证插件覆盖规则的影响，并同步修改 `pkg/extension/registry_test.rs`，必要时也更新 `auth_1_aster_unit_test.rs` 的跨模块断言。

新增扩展资源类型通常应接入 `pkg/extension/manifest.rs::newManifestWithSetup`：在资源注册成功后立即把对称撤销操作加入其内部 `clearFuncBuilder`。registry 层只应聚合每个 Manifest 返回的完整清理函数。必须补充独立测试覆盖当前 Manifest 内失败回滚、后续 Manifest 失败触发跨 Manifest 回滚，以及 Reset 不重复清理；Rust 单元测试应继续放在独立 `*_test.rs` 文件而非本源文件。

调整并发策略时要保持 `Extensions` 的二次 setup 检查，否则两个首次读取者可能重复运行有全局副作用的工厂。若计划在执行工厂时释放锁，则需要另行设计“初始化中”状态、等待/失败传播与 Reset 竞态，不能仅缩小现有锁范围。若改变失败后是否允许重试，也要明确工厂是否可重复调用和固定 `Option` 中闭包的克隆语义。

兼容性风险主要是公开错误文本、空集合的 `None` 语义、Manifest 排序与清理顺序；性能风险主要是注册时重复排序和 Setup 在写锁内执行所有工厂。当前测试以进程级单例为对象，新增用例必须使用 `serial_test::serial` 或等价隔离，并在前后调用 Reset。

## 验证依据

- 目标源码：`pkg/extension/registry.rs`（197 行），核对了全部类型别名、结构体、静态项、方法和顶层函数；文件无条件编译项。
- crate 与模块边界：`pkg/extension/Cargo.toml`、`pkg/extension/lib.rs`；确认 crate 名称、入口、公开再导出、无 registry feature，以及 `serial_test` 开发依赖。
- 直接实现依赖：`pkg/extension/manifest.rs::newManifestWithSetup`、`pkg/extension/util.rs::{ExtensionError, ClearFunc, clearFuncBuilder}`、`pkg/extension/extensions.rs::Extensions`。
- 上游消费证据：`cmd/tidb-server/main.rs::setupExtensions`、`pkg/extension/extensionimpl/bootstrap.rs::Bootstrap`。
- Rust 独立测试：`pkg/extension/registry_test.rs` 覆盖排序、迟到注册、名称错误、Reset 单次清理和失败回滚；`pkg/extension/main_test.rs` 覆盖空注册表；`pkg/extension/extensionimpl/bootstrap_test.rs` 覆盖无扩展短路与注册表错误传播；`pkg/extension/auth_1_aster_unit_test.rs` 提供跨扩展集合行为证据。
- Go 对照：`pkg/extension/registry.go`、`pkg/extension/registry_test.go`，核对字段、控制流、错误文本、清理和空结果行为。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件且覆盖 `pkg/extension/registry.rs`；`files --filter pkg/extension` 确认相邻源与测试；`query` 定位 Rust/Go 的 `GetExtensions`、`RegisterFactory`、`doSetup`；`callers`/`callees` 确认 `GetExtensions → registry::Extensions`、`Setup → doSetup`、`Register → RegisterFactory`、`doSetup → Extensions::from_manifests` 及相关测试调用。闭包内调用边由源码补验，未把索引缺失当作不存在调用。
- 按任务约束未运行 Cargo；本次为纯文档分析，最终以固定章节结构检查和人工事实复核作为验证。
