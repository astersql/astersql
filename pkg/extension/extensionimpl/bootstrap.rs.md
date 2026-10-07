# `pkg/extension/extensionimpl/bootstrap.rs`

## 文件定位

`pkg/extension/extensionimpl/bootstrap.rs` 是 `astersql-extension-extensionimpl` crate 当前唯一公开子模块 `bootstrap` 的实现文件；`pkg/extension/extensionimpl/lib.rs` 通过 `pub mod bootstrap` 导出它，`Cargo.toml` 将 crate 根指定为 `lib.rs`。它位于扩展注册表与系统会话能力之间：扩展侧只认识 `astersql-extension` 的 `BootstrapContext`/`SessionPool`，本文件则把 Domain 提供的会话池、SQL executor 和 etcd client 组装成该上下文。

该文件是一个 canonical 适配边界，而不是扩展注册表、系统表 bootstrap 或具体扩展逻辑本身。仓库内限定搜索只发现 `bootstrap_test.rs` 实现 `BootstrapDomain` 并调用公开函数 `Bootstrap`，没有发现非测试 Rust 的实现者或调用点；因此当前事实是“实现和回归测试已存在，但生产 Rust 主链尚未发现接线”，不能仅依据公开 API 推断服务器启动时已经执行它。

## 核心职责

1. 用 `BootstrapDomain` 隔离 Domain 与具体 session crate，避免注释所述的 `domain → session → domain` 依赖环，同时保留取得系统会话池、SQL executor、资源类型名和 etcd client 的必要能力。
2. 用私有 `bootstrapContext` 实现扩展框架的 `ExtensionContext` 与 `BootstrapContext`，向扩展钩子暴露取消状态、内部 SQL 执行、可选 etcd client 和系统会话池。
3. 在 `ExecuteSQL` 中为 SQL 上下文写入 `kv::InternalTxnBootstrap` 来源，排空可选结果集，并保证已产生的结果集总会执行一次 `Close`。
4. 在公开 `Bootstrap` 中惰性取得已注册扩展，只在确有扩展时借用系统会话，校验资源可转换为 SQL executor，顺序执行所有扩展 bootstrap 钩子，并统一归还已借资源。

本文件不负责注册扩展、创建会话池、实现 SQL executor、决定扩展顺序或重试失败钩子；这些分别由 `pkg/extension/registry.rs`、Domain/session 适配实现、`astersql-util-sqlexec` 和 `pkg/extension/extensions.rs` 承担。

## 主要符号

- `BootstrapDomain: Send + Sync`：公开的最小 Domain 契约。`SysSessionPool` 返回共享的 `Arc<dyn extension::SessionPool>`；`GetSQLExecutor` 从可变借用的 `SessionResource` 产生生命周期不超过该资源借用的 `Box<dyn SQLExecutor>`；`SessionResourceTypeName` 为类型断言错误提供兼容文本；`GetEtcdClient` 返回可选的共享客户端。
- `bootstrapContext<'a>`：私有上下文，持有可克隆的 `kv::Context`、受资源借用生命周期约束的 SQL executor、可选 etcd client，以及会话池 `Arc`。它没有公开构造器，只由 `Bootstrap` 或同模块测试构造。
- `ExtensionContext::is_cancelled`：直接读取所保存 KV 上下文的取消状态，不另建取消令牌。
- `BootstrapContext::ExecuteSQL`：执行无参数内部 SQL，返回排空后的 `Vec<chunk::Row>`；其 executor 是 `&mut self` 使用，因此同一上下文中的 SQL 调用串行进行。
- `BootstrapContext::EtcdClient`：以借用形式返回 `Option<&Client>`，不转移 `Arc` 所有权。
- `BootstrapContext::SessionPool`：返回 trait object 引用，让扩展可临时借还额外系统会话。
- `Bootstrap(ctx, domain)`：公开入口，返回 `Result<(), ExtensionError>`；它是资源获取、上下文装配、钩子调度与资源归还的总控函数。

本文件没有模块级常量、枚举、条件 feature 或后台任务；唯一条件编译项是末尾 `#[cfg(test)]`，把独立文件 `bootstrap_test.rs` 作为测试模块加载，符合生产代码与测试代码分文件的仓库约定。

## 执行流程

`Bootstrap` 首先调用 `extension::GetExtensions()`。该函数由 `pkg/extension/registry.rs` 的进程级注册表惰性构建 `Extensions`；构建错误立即向上传播，返回 `None` 则在接触系统会话池之前成功短路。

存在扩展时，入口依次执行：

1. `domain.SysSessionPool()` 取得共享池，并调用 `pool.Get()` 借出一个 `SessionResource`；借用失败直接返回，此时没有资源可归还。
2. 在可变借用资源前先记录 `SessionResourceTypeName`，再调用 `GetSQLExecutor(resource.as_mut())`。返回 `None` 时构造与 Go 对照一致的类型转换错误。
3. 成功取得 executor 后，克隆输入 `ctx`，读取可选 etcd client，并克隆池的 `Arc`，构造 `bootstrapContext`。
4. 调用 `Extensions::Bootstrap`；`pkg/extension/extensions.rs` 表明它按 Manifest 顺序遍历，仅调用存在的 bootstrap 回调，首个错误即停止后续钩子。
5. 无论类型转换、扩展钩子成功或扩展钩子失败，match 结束后都执行 `pool.Put(resource)`，最后返回先前保存的 `result`。`Put` 无返回值，因此归还阶段没有可传播错误。

扩展钩子调用 `ExecuteSQL(sql)` 时，流程为：从保存上下文克隆一份并用 `kv::WithInternalSourceType(..., InternalTxnBootstrap)` 标记内部来源；以空参数列表调用 `ExecuteInternal`；`None` 结果集映射为空行集合；`Some(record_set)` 则用批容量 `8` 调用 `DrainRecordSet`，随后无条件调用一次 `Close`，最后按固定优先级合并两阶段结果。

## 数据与状态

`bootstrapContext` 保存的 `kv::Context` 是输入上下文的克隆；测试证明克隆后仍能观察原取消状态，而每次 SQL 调用只在新的上下文值上附加 bootstrap 请求来源。SQL 参数始终是空 `Vec`，本文件不提供参数化执行入口。结果行由 `DrainRecordSet` 收集为拥有所有权的向量；没有结果集时 Rust 返回空向量，避免向上层暴露 Go 的 `nil` 切片区别。

会话池与 etcd client 用 `Arc` 共享，不复制底层对象。executor 的生命周期参数 `'a` 绑定到借出的 session resource，防止它在资源归还后继续存活；`bootstrap_context` 和 executor 都在调用 `pool.Put(resource)` 之前离开 match 分支并被释放。扩展注册状态属于 `pkg/extension/registry.rs::globalRegistry`，本文件只读取，不缓存或修改注册表。

入口使用一个外层会话资源支撑 SQL executor；扩展也能通过 `SessionPool()` 额外借用资源，但额外资源的归还责任属于扩展自身。`bootstrap_test.rs` 的成功场景分别统计外层借还与钩子内借还，验证两条生命周期互不替代。

## 依赖与调用关系

crate 清单 `pkg/extension/extensionimpl/Cargo.toml` 声明了 domain、extension、kv、sessionctx、chunk 与 sqlexec 路径依赖；目标文件直接导入并使用的是 `astersql-extension`、`astersql-kv` 和 `astersql-util-sqlexec`。domain/sessionctx/chunk 类型被有意藏在 `BootstrapDomain` 和上游 crate 的公开契约之后，目标文件没有直接引用这些 crate 名称。

下游调用链为 `Bootstrap` → `extension::GetExtensions` → `registry::Extensions`，以及 `Bootstrap` → `Extensions::Bootstrap` → 每个 `Manifest.bootstrap`。SQL 子链为 `bootstrapContext::ExecuteSQL` → `kv::WithInternalSourceType` → `SQLExecutor::ExecuteInternal` → `sqlexec::DrainRecordSet` → `RecordSet::Close`。`EtcdClient`、`SessionPool` 和 `is_cancelled` 则是由扩展回调按需拉取的能力。

上游方面，Go 生产入口存在于同路径 `bootstrap.go`，但 RustCodeGraph 文件查询和限定 Rust 搜索只找到 `bootstrap_test.rs` 对 Rust `Bootstrap` 的直接调用，只找到 `MockDomain` 实现 `BootstrapDomain`。这意味着本文可以确认适配层内部行为及测试调用边，不能确认其已进入 Rust 服务器启动链。RustCodeGraph 的精确 `callers/callees` 命令在本地查询超时且未输出边，因此该负面结论又用 `rg` 对 `pkg`、`cmd` 下非测试 Rust 调用形式与 trait 实现进行了交叉核验。

## 错误处理与边界

`GetExtensions`、会话池 `Get` 和扩展钩子的 `ExtensionError` 原样通过 `?` 或返回值传播。SQL executor、排空和关闭使用 `error.to_string()` 重新构造 `ExtensionError`，保留可读消息但丢失具体错误类型与错误链。类型转换失败由 `SessionResourceTypeName` 参与构造固定文案：`type '<实际类型>' cannot be casted to 'sessionctx.Context'`。

`ExecuteSQL` 的错误优先级与 Go 的 defer 语义对齐：执行失败时尚无结果集，无需关闭；排空失败后仍关闭结果集，但即使关闭也失败，最终仍返回排空错误；只有排空成功而关闭失败时返回关闭错误；两者成功才返回行。该函数不重试 SQL、不捕获 panic，也不对 SQL 文本做验证。

`Bootstrap` 在成功借出资源后覆盖类型失败、钩子失败和成功三条归还路径，但 `pool.Put` 不返回 `Result`，无法表达归还失败。若某个扩展 bootstrap 回调失败，`Extensions::Bootstrap` 会停止后续 Manifest；本文件仍先归还外层资源再传播错误。没有扩展或注册表构建失败都发生在借资源之前。

## 并发与资源生命周期

`BootstrapDomain` 要求 `Send + Sync`，会话池和 etcd client 通过 `Arc` 可跨线程共享；不过本文件自身是完全同步的，不创建线程、异步任务、通道或锁，也不并行执行扩展钩子。Manifest bootstrap 钩子由 `Extensions::Bootstrap` 按顺序串行调用，同一个 `&mut bootstrapContext` 被依次复用。

SQL executor 只存在于外层资源借用期间，并通过 `&mut self` 串行调用。结果集生命周期严格限于单次 `ExecuteSQL`：取得后先排空、再关闭、再返回；测试以原子计数验证各种排空/关闭组合下都只关闭一次。外层资源从 `pool.Get` 成功后一直持有到扩展调度结束，显式 `pool.Put` 负责归还；这里没有 RAII guard，所以若中间发生 panic，显式归还语句不会执行，这是当前实现边界。

输入 `kv::Context` 被克隆到 bootstrap 上下文，取消状态可被扩展和 SQL executor 观察；本文件不主动取消操作。扩展从 `SessionPool()` 借出的其他资源不受外层归还逻辑管理，扩展实现必须成对调用 `Get`/`Put`。

## 与 Go 版本的对应关系

直接基准是 `pkg/extension/extensionimpl/bootstrap.go`。两版都先取得全局扩展，空集合短路，再从 `Domain.SysSessionPool` 借资源、断言/适配为 session SQL executor、装配 context/etcd/pool、执行全部扩展 bootstrap，最后归还会话。两版 SQL 路径都标记 `InternalTxnBootstrap`，调用内部 SQL，以容量 `8` 排空结果集，并保证关闭；排空错误优先于关闭错误。

Rust 为打破 crate 依赖环增加了 `BootstrapDomain` trait，把 Go 对具体 `*domain.Domain`、`sessionctx.Context` 的直接类型断言改为上游提供的 `GetSQLExecutor` 与 `SessionResourceTypeName`。Go 依靠 `defer pool.Put(r)`，Rust 在 match 后显式 `Put`；正常错误返回上的结果一致，但 Rust 显式清理不具备 panic 展开保护。Go 的 `nil` record set 返回 `nil, nil`，Rust 用 `Ok(Vec::new())` 表示同一“无行”语义。

Go `ExecuteSQL` 接收调用者传入的 `context.Context`，而当前 Rust trait 签名只有 `sql: &str`，使用 `bootstrapContext` 保存的上下文克隆；因此 Rust 扩展不能为每条 bootstrap SQL 单独传入另一上下文。Go 直接返回底层错误，Rust 在 executor/record-set 边界将错误字符串化。Rust 独立测试 `bootstrap_test.rs` 明确覆盖这些有意的表示差异和共同不变量。

## 扩展指南

若要把本适配层接入生产 Rust 启动链，应在拥有 concrete Domain 与 session resource 知识的 session 层实现 `BootstrapDomain`，并在与 Go 启动时机一致的位置调用本文件 `Bootstrap`；必须新增该层的独立测试，证明类型校验、executor 借用生命周期和启动顺序，而不能把测试内嵌到 `bootstrap.rs`。接线前还应检查 `Cargo.toml` 中现有 domain/sessionctx 依赖是否仍必要，避免为接线重新引入注释明确规避的依赖环。

扩展 `BootstrapContext` 能力时，应同步修改 `pkg/extension/manifest.rs` 的 trait、本文件 `bootstrapContext` 实现、Go 对照接口/实现，以及 `pkg/extension/extensionimpl/bootstrap_test.rs`。新增 SQL 行为应继续保持内部来源标记、结果集必关和“排空错误优先”不变量；若增加参数化执行，需明确参数所有权及 Go API 兼容策略。

修改资源管理时，优先考虑可验证的作用域守卫以覆盖 panic，但必须评估 `SessionPool::Put` 的对象安全和借用顺序，不能让 executor 活过 resource。新增并行钩子会改变 Manifest 顺序、共享可变上下文和错误短路语义，属于兼容性与并发风险，不应只在本文件局部实现。性能上应关注批容量 `8`、全量收集结果行和扩展自行借用额外会话造成的资源占用；改变这些行为须同步 Go 语义与压力边界测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/extension/extensionimpl` 识别 `bootstrap.go`、`bootstrap.rs`、`bootstrap_test.rs`、`lib.rs`，目标 Rust 文件含 12 个符号。
- RustCodeGraph 源码证据：完整读取 `pkg/extension/extensionimpl/bootstrap.rs`，并读取 `pkg/extension/manifest.rs::BootstrapContext`、`WithBootstrap`、`WithBootstrapSQL`，`pkg/extension/extensions.rs::Extensions::Bootstrap` 与 `pkg/extension/registry.rs::GetExtensions`。精确 `callers/callees` 查询超时且无输出，未把其当作成功证据。
- crate 与模块证据：`pkg/extension/extensionimpl/Cargo.toml`、`pkg/extension/extensionimpl/lib.rs`。
- Go 对照证据：`pkg/extension/extensionimpl/bootstrap.go`，逐项核对扩展短路、系统会话借还、类型断言、内部 SQL、容量 `8` 的排空与关闭错误优先级。
- Rust 独立测试证据：`pkg/extension/extensionimpl/bootstrap_test.rs` 中 `kv_context_is_the_sql_execution_context`、`execute_sql_marks_internal_source_drains_rows_and_closes`、`execute_sql_preserves_go_error_and_close_precedence`、`bootstrap_matches_go_short_circuit_pool_error_and_resource_cleanup_paths`。
- 限定搜索证据：搜索 `BootstrapDomain` 实现与 extensionimpl `Bootstrap` 调用，只发现上述测试中的 `MockDomain` 和直接调用；因此明确记录生产 Rust 接线“未发现”，而非推测已支持。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；验证范围是索引/源码事实复核、Go/Rust 测试语义核对及固定十一章节结构检查。
