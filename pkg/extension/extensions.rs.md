# [`pkg/extension/extensions.rs`](extensions.rs)

## 文件定位

本文件属于 `astersql-extension` crate（见 `pkg/extension/Cargo.toml` 的 `[lib] path = "lib.rs"`），是扩展注册完成后的只读聚合层。`pkg/extension/lib.rs` 通过 `pub mod extensions` 和 `pub use extensions::*` 将这里的 API 暴露到 crate 根。

上游构造入口位于 `pkg/extension/registry.rs`：`registry::doSetup` 按扩展名排序并构造 `Manifest`，随后调用 `Extensions::from_manifests`，再把结果以 `Arc<Extensions>` 缓存在全局注册表中。下游主要有两类：`pkg/extension/extensionimpl/bootstrap.rs::Bootstrap` 调用 `Extensions::Bootstrap`，`pkg/extension/session.rs::newSessionExtensions` 调用 `Manifests` 生成每个会话自己的扩展句柄。当前 Rust 仓库中，`GetAccessCheckFuncs` 和 `GetAuthPlugins` 的直接使用主要见扩展测试；Go 版本还由权限、执行器和服务器包消费，不能据此宣称相应 Rust 主链已经全部接线。

## 核心职责

- 用 `Extensions { manifests }` 保存一次 registry setup 得到的、顺序稳定的 `Arc<Manifest>` 集合。
- 以克隆 `Arc` 的方式提供 Manifest 快照，避免把内部 `Vec` 的可变权交给调用方。
- 按 Manifest 顺序执行 bootstrap 回调，并在首个错误处短路。
- 汇总访问检查函数和全局认证插件表。
- 把进程级扩展描述转换成会话级 `SessionExtensions`。
- 通过 `manifests_or_empty` 为可选扩展值提供“无扩展即空列表”的局部适配。

该文件不负责注册、排序、Manifest 合法性校验或资源回滚；这些职责分别在 `registry.rs`、`manifest.rs` 和相关 option/setup 逻辑中完成。

## 主要符号

- `pub struct Extensions`：唯一数据字段是 `pub(crate) manifests: Vec<Arc<Manifest>>`。类型本身公开，字段只对 crate 内可见。
- `Extensions::from_manifests(Vec<Arc<Manifest>>) -> Self`：内部组装入口；不排序、不去重、不校验，保留传入顺序。生产构造者 `registry::doSetup` 会先按名称排序；测试也可直接传入特定顺序。
- `Extensions::Manifests(&self) -> Vec<Arc<Manifest>>`：克隆向量及其中的 `Arc`，不深拷贝 `Manifest`。
- `Extensions::Bootstrap(&self, &mut dyn BootstrapContext) -> Result<(), ExtensionError>`：跳过未配置 bootstrap 的 Manifest，依次执行已配置回调，以 `?` 原样传播第一个错误。
- `Extensions::GetAccessCheckFuncs(&self) -> Vec<AccessCheckFunc>`：按 Manifest 顺序收集非空 `accessCheckFunc`；回调类型自身是 `Arc<dyn Fn(...) + Send + Sync>`。
- `Extensions::NewSessionExtensions(&self) -> SessionExtensions`：委托 `session.rs::newSessionExtensions`，为一次会话实例化事件回调和认证插件映射。
- `Extensions::GetAuthPlugins(&self) -> HashMap<String, Arc<AuthPlugin>>`：遍历所有 Manifest 的插件列表，以插件 `Name` 为键；同名时后遍历者覆盖先遍历者。
- `manifests_or_empty(Option<&Extensions>) -> Vec<Arc<Manifest>>`：`None` 返回空向量，`Some` 调用 `Manifests`。精确搜索未发现当前仓库中的调用者，因此它目前是公开的兼容/便利 API，而不是已验证的主链入口。

本文件没有常量、trait、条件编译项或内部测试模块。

## 执行流程

1. 扩展通过 `registry.rs::Register` / `RegisterFactory` 注册；`registry::doSetup` 按 `extensionNames` 的字典序依次生成 Manifest。
2. `registry::doSetup` 调用 `Extensions::from_manifests(manifests)`，将结果放入 `Arc` 并缓存。由此产生的生产实例继承稳定排序；`from_manifests` 自身并不建立该不变量。
3. 集群扩展引导时，`extensionimpl/bootstrap.rs::Bootstrap` 从全局 registry 获取集合，借出会话资源并建立 `BootstrapContext`，然后调用本文件的 `Bootstrap`。每个已配置钩子按向量顺序运行；一旦返回 `ExtensionError`，后续钩子不再执行。外层适配器随后仍把借出的资源归还池。
4. 创建连接/会话扩展时，`NewSessionExtensions` 调用 `session.rs::newSessionExtensions`。后者通过 `Manifests` 遍历全部清单，调用会话处理器工厂并收集连接/语句事件回调。
5. 权限或认证消费者可通过 `GetAccessCheckFuncs` / `GetAuthPlugins` 取得聚合快照。Rust 当前代码搜索未显示生产调用者；Go 对照中的生产调用点包括 `pkg/privilege/privileges/privileges.go`、`pkg/executor/*.go` 和 `pkg/server/*.go`，它们只能作为迁移目标证据，不能代替 Rust 接线证据。

## 数据与状态

`Extensions` 的唯一持久状态是 `Vec<Arc<Manifest>>`。向量顺序具有行为意义：它决定 bootstrap 和访问检查函数的执行/返回顺序，也决定同名认证插件的覆盖顺序以及会话事件处理器的广播顺序。

`Manifests`、`GetAccessCheckFuncs` 和 `GetAuthPlugins` 都返回新容器；调用者修改返回的 `Vec`/`HashMap` 不会改变 `Extensions`。元素仍通过 `Arc` 共享，因此克隆成本主要是容器分配和引用计数递增，而不是深拷贝回调或 Manifest。

该类型没有内部锁、缓存或惰性字段。可变全局状态在 `registry.rs` 的 `RwLock<registryState>` 中；本文件只消费 setup 后的快照。`GetAuthPlugins` 每次调用都会重新分配映射，`NewSessionExtensions` 每次调用都会重新构造会话级集合。

## 依赖与调用关系

直接依赖均来自同一 crate：

- `manifest::{Manifest, BootstrapContext, AccessCheckFunc}` 定义聚合的数据与 bootstrap/访问检查协议。
- `session::{newSessionExtensions, SessionExtensions}` 实现进程级 Manifest 到会话级状态的转换。
- `auth::AuthPlugin` 是认证插件值类型。
- `util::ExtensionError` 是 bootstrap 错误边界。
- 标准库 `Arc` 提供跨集合/会话共享所有权，`HashMap` 构建插件索引。

已验证的关键边为：`registry.rs::doSetup -> Extensions::from_manifests`、`extensionimpl/bootstrap.rs::Bootstrap -> Extensions::Bootstrap`、`Extensions::NewSessionExtensions -> session.rs::newSessionExtensions -> Extensions::Manifests`。RustCodeGraph 将目标文件标为被 `pkg/extension/registry.rs` 和 `pkg/extension/event_listener_test.rs` 使用；其独立 callers/callees 查询未返回更细的 Rust 调用边，因此上述细粒度关系由精确源码搜索核验。

`Cargo.toml` 没有为本文件设置 feature gate；该模块在 `lib.rs` 中无条件编译。`Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/extension"` 明确记录其 Go 对照包。

## 错误处理与边界

只有 `Bootstrap` 直接返回错误。它不包装 `ExtensionError`，首个失败回调通过 `?` 原样返回，后续 Manifest 不再执行，也不在本层回滚已完成的 bootstrap 副作用。资源归还由 `extensionimpl/bootstrap.rs::Bootstrap` 的外层流程保证；扩展注册期间的失败清理由 `registry.rs::doSetup` 和 `manifest.rs::newManifestWithSetup` 负责。

空集合时，各方法自然返回成功或空容器。Rust 方法要求存在 `&self`，不能像 Go 方法那样在 nil receiver 上调用；只有 `manifests_or_empty` 明确处理 `Option<&Extensions>`，而 `NewSessionExtensions` 等其他方法没有通用的 `Option` 适配。另一个已验证差异是：Rust registry 在完全没有 factory 时返回 `None`，Go registry 测试则取得非 nil 的空 `Extensions`。调用方必须按各自语言当前契约处理，不能照搬 Go 的 nil 行为。

`GetAuthPlugins` 对不同 Manifest 间的同名插件执行后者覆盖；合法性检查发生在 `manifest.rs` 的 setup 路径，本方法本身不报告冲突。`from_manifests` 是公开函数且不校验输入，绕过 registry 的调用者需要自行保证 Manifest 已正确构造及排序。

## 并发与资源生命周期

`Extensions` 不包含可变同步原语。其 Manifest 和回调均经 `Arc` 共享，相关回调类型要求 `Send + Sync`，因此集合适合由 registry 的 `Arc<Extensions>` 分发给并发消费者。这里不会启动任务、创建通道、持锁或持有事务。

生产生命周期由 `registry.rs` 控制：setup 后缓存 `Arc<Extensions>`，`Reset` 移除缓存并运行已收集的 close 回调。调用者持有的 `Arc` 可以让集合及 Manifest 在 registry reset 后继续存活，但 close 回调是否已执行由 registry 状态决定；本文件不协调“仍持有快照”与“扩展已关闭”之间的语义。

bootstrap 回调是同步串行调用，共享同一个可变 `BootstrapContext`，所以前一钩子的状态和副作用对后一钩子可见。`NewSessionExtensions` 创建独立的会话容器，但其中回调/插件仍以 `Arc` 引用进程级对象。测试中的全局 registry 用 `serial_test::serial` 隔离，说明并行测试不应无序修改该全局注册状态。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/extension/extensions.go`。字段和五个核心方法逐项对应：`manifests`、`Manifests`、`Bootstrap`、`GetAccessCheckFuncs`、`NewSessionExtensions`、`GetAuthPlugins`。Rust 保留 Go 风格的公开方法名以便迁移比对，并额外提供 `from_manifests` 与 `manifests_or_empty`。

主要一致点：Manifest 按既有顺序遍历；bootstrap 首错返回；访问检查仅收集非空回调；认证插件以名称建表并让后者覆盖；会话构造委托给 session 模块。`pkg/extension/bootstrap_test.rs` 对照 Go `bootstrap_test.go::TestBootstrap` 验证 bootstrap SQL 顺序；`registry_test.rs` 验证按名称排序；`event_listener_test.rs` 验证会话监听器按 Manifest 顺序广播；`auth_1_aster_unit_test.rs` 验证全局认证表累加而会话表采用最后一个非空 Manifest 的 Go 语义。

主要差异：Go 对 nil receiver 返回 nil/无操作，Rust 依赖引用和 `Option`；Go `NewSessionExtensions` 返回指针，Rust 返回值；Go 空 registry 当前产生可调用的空集合，Rust `GetExtensions` 返回 `None`；Rust 使用 `Arc` 表达共享所有权。Go 生产侧对访问检查、认证插件和会话扩展的接线比当前 Rust 搜索结果更完整，迁移状态应以实际 Rust 调用者为准。

## 扩展指南

- 新增“从全部 Manifest 聚合”的能力时，优先在 `Manifest` 增加由 `With*` option 填充的字段，再在 `Extensions` 增加只读聚合方法；不要在本文件重做注册、排序或合法性校验。
- 若行为依赖顺序，明确选择“累加”“首个生效”或“后者覆盖”，并同时覆盖 registry 排序与直接 `from_manifests` 的自定义顺序。认证插件应特别测试同名冲突/覆盖。
- 新增可能失败的批量调用时，明确首错短路、是否继续以及是否需要回滚。若涉及资源借还，应在拥有资源的外层适配器处理，不能仅依赖本层循环。
- 若要扩展 nil/无扩展兼容 API，应统一使用 `Option<&Extensions>` 边界并核对 Go nil receiver 语义；不要假设现有所有方法都支持空值。
- 应把 Rust 测试逻辑放在独立测试文件，而不是嵌入 `extensions.rs`。适合扩展的现有测试面包括 `bootstrap_test.rs`、`registry_test.rs`、`event_listener_test.rs` 和 `auth_1_aster_unit_test.rs`；新增访问检查聚合行为时宜在同目录新增或扩展独立测试文件，并同步核对 `extensions.go` 及相应 Go 测试意图。
- 性能敏感调用要注意 `Manifests`/`GetAccessCheckFuncs` 的向量克隆、`GetAuthPlugins` 的映射重建，以及 `Arc` 引用计数操作；若引入缓存，需要重新评估 registry reset 和插件生命周期。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；`files --filter pkg/extension` 找到目标文件；`node --file pkg/extension/extensions.rs` 读取完整 85 行并报告使用文件；`query Extensions` 核对 `Extensions`、`from_manifests`、五个公开方法和 `manifests_or_empty`。细粒度 `callers/callees` 未返回结果，因此未将其当作已验证调用边。
- Rust 源与 crate 边界：`pkg/extension/extensions.rs`、`pkg/extension/Cargo.toml`、`pkg/extension/lib.rs`、`pkg/extension/registry.rs`、`pkg/extension/manifest.rs`、`pkg/extension/session.rs`、`pkg/extension/extensionimpl/bootstrap.rs`、`cmd/tidb-server/main.rs`。
- Rust 独立测试：`pkg/extension/bootstrap_test.rs`、`pkg/extension/registry_test.rs`、`pkg/extension/event_listener_test.rs`、`pkg/extension/auth_1_aster_unit_test.rs`、`pkg/extension/extensionimpl/bootstrap_test.rs`。
- Go 对照与测试：`pkg/extension/extensions.go`、`pkg/extension/session.go`、`pkg/extension/bootstrap_test.go`、`pkg/extension/registry_test.go`、`pkg/extension/auth_test.go`；生产调用证据还包括 `pkg/extension/extensionimpl/bootstrap.go`、`pkg/privilege/privileges/privileges.go`、`pkg/executor/simple.go`、`pkg/executor/grant.go`、`pkg/server/server.go` 和 `pkg/server/rpc_server.go`。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付检查以固定章节结构、链接/路径存在性、精确引用搜索和人工事实复核为准。
