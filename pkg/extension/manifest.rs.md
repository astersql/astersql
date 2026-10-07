# `pkg/extension/manifest.rs`

## 文件定位

`manifest.rs` 是 `astersql-extension` crate 的声明与安装边界：扩展作者用一组 `With*` 工厂生成 `Option`，注册表在 Setup 阶段把这些选项应用到一个 `Manifest`，并把需要写入进程级注册中心的动态权限、系统变量和扩展函数真正安装进去。模块由 `pkg/extension/lib.rs` 公开声明并整体再导出；crate 边界与依赖见 `pkg/extension/Cargo.toml`，其中 `chunk_dependency`、`variable_dependency` 和 `etcd-client` 分别支撑 bootstrap 返回行、系统变量注册和 etcd 访问。

它处在“扩展注册”与“运行时消费”之间。上游 `pkg/extension/registry.rs::registry::doSetup` 按扩展名排序后调用 `newManifestWithSetup`；下游 `pkg/extension/extensions.rs::Extensions` 保存构造好的清单，并在 bootstrap、访问检查、认证插件收集和会话扩展创建时读取相应字段。这个文件不是扩展代码的动态加载器，也不直接执行所有运行期回调。

## 核心职责

- 定义扩展清单 `Manifest`，集中保存扩展名以及系统变量、动态权限、函数、访问检查、认证插件、会话处理器工厂、bootstrap 和关闭回调。
- 提供 `WithCustomSysVariables`、`WithCustomDynPrivs`、`WithCustomFunctions`、`WithCustomAccessCheck`、`WithCustomAuthPlugins`、`WithSessionHandlerFactory`、`WithClose`、`WithBootstrap` 与 `WithBootstrapSQL`，把用户配置封装成可重复应用的线程安全闭包 `Option`。
- 通过 `newManifestWithSetup` 串行执行工厂、应用选项、注册进程级资源、校验认证插件，并为成功注册的资源收集清理函数。
- 用 `InstallDynamicPrivilegeHooks` 安装权限子系统适配钩子，打破 extension 与权限实现之间的依赖环；扩展函数采用 `pkg/extension/function.rs` 中对应的钩子机制。
- 定义 bootstrap 和会话资源的抽象边界，使 extension crate 不依赖具体 session pool 或 SQL 执行器实现。

## 主要符号

- `SessionResource: Send` 及其 blanket impl：任何 `Send` 类型都可作为会话池资源。`SessionPool: Send + Sync` 只规定 `Get() -> Result<Box<dyn SessionResource>, ExtensionError>` 和 `Put(...)`，不规定容量、阻塞或回收策略。
- `Option = Arc<dyn Fn(&mut Manifest) + Send + Sync>`：配置闭包可克隆并跨线程共享；每个 `With*` 调用捕获输入值，在应用时克隆集合或 `Arc` 后写入清单字段。
- `AccessCheckFunc`：接收用户、主机、数据库、`mysql::PrivilegeType` 和 SEM 标志，返回额外所需的动态权限名。字段的运行期收集点是 `Extensions::GetAccessCheckFuncs`。
- `SessionHandlerFactory`：无参返回 `Option<SessionHandler>`；运行期由会话扩展组装逻辑消费，返回 `None` 表示该次不创建处理器。
- `CloseFunc`：可共享的无参关闭回调。它会在安装早期加入清理链，因而 Setup 后 `Reset` 或后续安装失败都会调用。
- `BootstrapContext: ExtensionContext`：提供可失败的 `ExecuteSQL`、可选 `EtcdClient` 和 `SessionPool`。`BootstrapFunc` 接收可变 trait object，允许按顺序更新上下文状态。
- `WithBootstrapSQL(Vec<String>)`：用 `WithBootstrap` 包装 SQL 列表，逐条调用 `ExecuteSQL`，第一条错误立即返回，后续 SQL 不再执行；返回行会被丢弃。
- `Manifest`：`name` 私有且只通过 `Name()` 读取，其余字段为 `pub(crate)`，只向 crate 内聚合层开放。`Manifest::empty` 为所有可选项提供空值，名称之外没有隐式默认能力。
- `RegisterDynamicPrivilege` / `RemoveDynamicPrivilege`、`DynamicPrivilegeHooks` 和全局 `DYNAMIC_PRIVILEGE_HOOKS`：保存权限注册与移除适配器。`InstallDynamicPrivilegeHooks` 是公开安装入口，读写通过 `OnceLock<RwLock<Option<_>>>` 完成。
- `newManifestWithSetup`：crate 内核心安装函数，返回拥有所有权的 `Manifest` 与一次性 `ClearFunc`；任何步骤失败时在返回错误前执行已收集清理。

## 执行流程

1. `registry::doSetup` 取出按名称排序的扩展工厂，调用 `newManifestWithSetup(name, factory)`（`pkg/extension/registry.rs`）。
2. `newManifestWithSetup` 先创建 `Manifest::empty(name)`，执行工厂取得 `Vec<Option>`；工厂错误会直接进入统一回滚路径。
3. 选项按向量顺序逐个写入同一个清单。相同类别出现多次时，当前实现是后一次赋值覆盖前一次，而不是合并或拒绝。
4. 若存在 `close`，先把调用它的闭包加入 `clearFuncBuilder`；此时不会立即执行关闭回调。
5. 按 `dynPrivs` 顺序调用 `register_dynamic_privilege`。每次成功后收集对应的移除闭包；失败项本身没有清理闭包，已成功项会在统一回滚时移除。
6. 按 `sysVariables` 顺序检查元素非 `None`、名称非空、全局注册表不存在同名项；通过后调用 `variable::RegisterSysVar` 并收集 `UnregisterSysVar`。
7. 按 `funcs` 顺序调用 `register_extension_function`，成功后按函数名收集 `remove_extension_function`。
8. 调用 `validateAuthPlugin(&manifest)`；它在有插件列表时转交 `validate_auth_plugins`，检查名称、重复、保留名和三个必填认证回调（`pkg/extension/auth.rs`）。
9. 全部成功时返回清单和 `clear_builder.Build()`。失败时立即构建并执行清理，然后原样返回 `ExtensionError`。
10. 注册表把每个成功扩展的清理函数继续汇总；后续扩展失败时回滚先前扩展，正常情况下由 `registry::Reset` 调用。bootstrap 本身不在 Setup 中执行，而由 `Extensions::Bootstrap` 以后按 Manifest 顺序调用。

## 数据与状态

`Manifest` 是安装完成后的能力描述。集合字段保留调用者给出的顺序；这影响动态权限、系统变量、函数的注册顺序，也影响 `WithBootstrapSQL` 内 SQL 的执行顺序。选项闭包捕获并克隆 `Vec`/`Arc`，所以应用选项不会把捕获值移出，`Option` 本身可被注册表固定选项工厂重复克隆。

进程级可变状态包括动态权限钩子的 `OnceLock<RwLock<Option<DynamicPrivilegeHooks>>>`，以及被调用的系统变量、函数注册中心。`OnceLock` 只负责惰性创建锁；`InstallDynamicPrivilegeHooks` 可多次写入并替换当前钩子对。读取钩子时先在读锁内克隆 `Arc`，随后释放锁再调用外部回调，避免在未知实现执行期间持锁。

清理状态由 `clearFuncBuilder` 独占保存。其 `Build` 生成 `FnOnce`，并严格按收集顺序执行：关闭回调最先，其后是已注册的动态权限、系统变量和函数，而不是逆序栈式回滚（`pkg/extension/util.rs::clearFuncBuilder::Build`）。`manifest_test.rs` 把这一 Go 对齐顺序锁定为 `close -> remove privileges -> unregister sysvar（通过状态断言）-> remove function`。

## 依赖与调用关系

上游主链为 `Register`/`RegisterFactory` → `registry::Setup` 或惰性的 `registry::Extensions` → `registry::doSetup` → `newManifestWithSetup`。RustCodeGraph 对 `newManifestWithSetup` 的查询还显示直接测试调用者位于 `manifest_test.rs` 和 `event_listener_test.rs`；后者用它构造带会话处理器的清单。

`newManifestWithSetup` 的直接下游包括本文件的 `Manifest::empty`、`register_dynamic_privilege`、`remove_dynamic_privilege`，`pkg/extension/function.rs` 的 `register_extension_function`/`remove_extension_function`，`pkg/extension/auth.rs::validateAuthPlugin`，以及 `pkg/extension/lib.rs` 再导出的 `variable::{GetSysVar, RegisterSysVar, UnregisterSysVar}`。RustCodeGraph 明确识别了前述本地与函数注册调用边；认证和变量调用同时由源码逐句核验。

安装后的字段由 `pkg/extension/extensions.rs` 消费：`Bootstrap` 调用 bootstrap，`GetAccessCheckFuncs` 收集访问检查，`GetAuthPlugins` 聚合认证插件，`NewSessionExtensions` 间接消费会话处理器工厂。`pkg/extension/Cargo.toml` 没有 feature 条件；本文件也没有条件编译项。

## 错误处理与边界

- 工厂错误、动态权限注册错误、系统变量校验错误、函数注册错误和认证插件校验错误统一使用 `ExtensionError` 向上传播；已收集资源在返回前清理。
- 未安装动态权限注册钩子时，注册返回 `RegisterDynamicPrivilege is not installed`；未安装移除钩子时清理静默跳过。函数钩子具有对应行为，错误文本为 `RegisterExtensionFunc is not installed`（`pkg/extension/function.rs`）。
- 系统变量元素允许在类型层面为 `None`，但 Setup 会报 `system var should not be nil`；空名称和已注册名称分别报固定错误。`manifest_test.rs` 验证这些错误及先前资源的回滚。
- `WithBootstrapSQL` 不做事务包裹、重试或补偿；部分 SQL 已成功而后续 SQL 失败时，数据库副作用由调用方/SQL 自身负责。`EtcdClient` 也明确允许缺失。
- `WithBootstrap` 与 `WithBootstrapSQL` 在 `README.md` 中被描述为互斥，但当前 Rust 与 Go 源码都只是给同一字段赋值：若同时作为选项传入，后应用者覆盖前者，代码不会拒绝。这是文档约束而非运行时不变量。
- `Option` 只修改清单，不返回错误；因此自定义选项自身无法直接报告失败。可失败的动态决策应放在扩展工厂，资源安装错误则由 Setup 阶段产生。
- 动态权限的合法性（例如空名、内建重名、跨扩展重名）不由本文件判断，而由已安装的注册钩子决定；Go 的 `registry_test.go` 覆盖了这些下游规则。

## 并发与资源生命周期

公开回调和 trait 的约束使选项、bootstrap、访问检查、关闭回调及钩子可跨线程传递/共享；`SessionPool` 同时要求 `Send + Sync`。不过这些约束不表示单个 `Manifest` 会并行安装：`registry::doSetup` 持注册表写锁并按名称串行构建扩展。

全局动态权限钩子由 `RwLock` 保护，锁中毒时通过 `into_inner()` 继续使用已有状态。外部钩子在锁外调用，减少死锁和长时间占锁风险。清理函数是 `FnOnce`，从类型上防止同一返回值被重复执行；注册表还用 `state.close.take()` 保证一次 `Reset` 只取走一次聚合清理。`registry_test.rs` 与 Go `TestRegisterExtensionWithClose` 验证重复 Reset 不重复关闭。

会话资源的借还生命周期仅由 `SessionPool::Get/Put` 契约表示，本文件不提供 RAII guard，调用者必须显式归还。bootstrap 上下文是调用期间借用，`EtcdClient` 和 session pool 引用不能逃逸该借用生命周期。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/extension/manifest.go`：相同的 Option 字段、清单字段、安装顺序和失败回滚意图均被保留。`pkg/extension/manifest_test.rs::manifest_options_register_resources_and_clear_in_go_order` 专门验证资源注册与清理顺序；`setup_error_rolls_back_only_resources_registered_before_the_error` 和 `invalid_system_variables_match_go_errors_and_rollback_prior_setup` 验证部分安装失败及错误文本。

主要语言适配差异如下：Go `Option` 是普通函数，Rust 用 `Arc<dyn Fn + Send + Sync>`；Go 的 nil 指针在 Rust 中成为 `Option<Arc<_>>`；Go 全局函数变量 `RegisterDynamicPrivilege`/`RemoveDynamicPrivilege` 在 Rust 中成为带锁的可替换钩子对；Go `context.Context` 嵌入被缩减为本地 `ExtensionContext::is_cancelled` 抽象；Go `ExecuteSQL(ctx, sql)` 在 Rust 中改为 `&mut self, sql`，取消状态由上下文本身承担；Go `WithBootstrapSQL` 是可变参数，Rust 接收 `Vec<String>`。

Go `newManifestWithSetup` 用 `defer` 在命名错误返回时回滚，Rust 用闭包结果加 `match` 显式实现相同行为。两边的 `clearFuncBuilder` 都按收集先后执行。Go `bootstrap_test.go::TestBootstrap` 与 Rust `bootstrap_test.rs::canonical_bootstrap_sql_preserves_registration_order` 均证明 bootstrap SQL 和后续自定义 bootstrap 的顺序；Rust 测试用内存上下文替代完整存储。

## 扩展指南

- 新增纯描述型能力时，在 `Manifest` 增加字段和空默认值，再提供相应 `With*`；同时在真正消费该字段的 `Extensions`/session 层接线。若多个同类选项应合并而非覆盖，必须显式改变 Option 逻辑并补覆盖顺序测试。
- 新增需要进程级注册的资源时，应在 `newManifestWithSetup` 中选择明确的安装阶段，并在每次成功后立刻收集对称清理。要特别确认清理应保持现有 FIFO 语义，还是必须先修改 `clearFuncBuilder` 的全局契约。
- 新增可失败配置不要塞入当前无返回值的 `Option`；优先由 `ExtensionFactory` 返回错误，或在 Setup 的集中校验/注册阶段处理。改变 `Option` 签名会影响所有 `With*`、`Register` 与测试。
- 修改动态权限或函数钩子时，要保持“锁内克隆、锁外调用”，并同时验证钩子未安装、安装失败和清理钩子缺失路径。
- 修改认证字段时同步 `pkg/extension/auth.rs::validateAuthPlugin` 与独立认证测试；修改 bootstrap 时同步 `pkg/extension/bootstrap_test.rs`；修改安装/回滚顺序时同步 `pkg/extension/manifest_test.rs` 和 `registry_test.rs`。Rust 测试应继续独立放置，不嵌入生产源文件。
- 兼容性风险集中在公开回调签名、固定错误文本、安装/清理顺序和“后选项覆盖前选项”行为；性能风险主要来自 Setup 期间逐项全局注册和回调执行，当前没有批处理或并行安装。

## 验证依据

- 源码：`pkg/extension/manifest.rs`（27 个索引符号）、`pkg/extension/lib.rs`、`pkg/extension/registry.rs`、`pkg/extension/extensions.rs`、`pkg/extension/util.rs`、`pkg/extension/auth.rs`、`pkg/extension/function.rs`。
- crate 与使用说明：`pkg/extension/Cargo.toml`、`pkg/extension/README.md`。目标包不存在 `doc.go`，因此以模块入口和 README 作为最近的包级说明。
- Go 对照：`pkg/extension/manifest.go`、`pkg/extension/registry.go`、`pkg/extension/bootstrap_test.go`、`pkg/extension/registry_test.go`。
- Rust 独立测试：`pkg/extension/manifest_test.rs`、`pkg/extension/bootstrap_test.rs`、`pkg/extension/registry_test.rs`；调用搜索还定位到 `event_listener_test.rs` 与 `auth_1_aster_unit_test.rs` 的相关构造路径。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/extension` 确认目标与相邻 Rust/Go 文件均入图；对 `Manifest newManifestWithSetup WithBootstrapSQL InstallDynamicPrivilegeHooks` 的 `explore` 核对了主要调用者、被调用者及测试触达，精确 `query` 将 Rust `newManifestWithSetup` 定位到 `pkg/extension/manifest.rs:226`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另行执行固定 11 章节结构检查，并人工复核本页没有把 README 约束误写成代码强制行为。
