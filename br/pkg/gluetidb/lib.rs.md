# [`br/pkg/gluetidb/lib.rs`](lib.rs)

## 文件定位

`br/pkg/gluetidb/lib.rs` 是 Cargo 包 `astersql-br-pkg-gluetidb` 的 crate 根。`br/pkg/gluetidb/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定它，并用 `package.metadata.porting.go-package = "br/pkg/gluetidb"` 标明对应的 Go 包。文件本身只有模块接线和再导出，没有业务函数、类型、常量或运行时入口。

生产模块是 `glue.rs` 与 `infoschema_filter.rs`，分别由 `#[path] pub mod` 挂载；`parity_test.rs`、`glue_test.rs`、`test_support_test.rs` 只在 `cfg(test)` 下作为私有模块编译（`lib.rs:18-34`）。这使生产逻辑与独立 Rust 测试分文件保存，符合仓库“不把 Rust 测试放进源文件”的约束。

## 核心职责

本文件承担三个编译期职责：

1. 确立 TiDB Glue 的 crate 边界，把 TiDB 会话/Domain 适配和 BR InfoSchema 过滤器组成一个库。
2. 通过 `pub use glue::*` 与 `pub use infoschema_filter::*` 建立扁平根 API，使 crate 内测试和潜在下游可以直接写 `crate::New`、`crate::Glue`、`crate::NewInfoSchemaFilter`，无需经过子模块前缀。
3. 把公开契约测试挂到同一 crate 内。测试可以访问 `pub(crate)` 或模块私有接线所需的状态复位工具，同时生产构建不会包含这些测试模块。

当前仓库事实必须与设计意图区分：引用搜索没有发现其他 Rust 生产 crate 在 Cargo manifest 中依赖 `astersql-br-pkg-gluetidb`，也没有发现生产 `.rs` 通过 `astersql_br_pkg_gluetidb` 导入根 API。Rust BR CLI 当前在 `br/cmd/br/stubs.rs` 使用另一套本地 gluetidb 桩；因此本 crate 已形成可测试的迁移库，但尚不能据 Go 主链推断它已接入 Rust BR 生产入口。

## 主要符号

`lib.rs` 自身的符号都是模块或再导出：

- `pub mod glue`：公开 `glue.rs`。经通配再导出后，根级 API 包含 `Glue`、`New`、`brComment`、`GlobalConfigBits`、`GetGlobalConfigBits`、`FilterLoadSysDBs`、`FilterLoadSpecifiedDBAndSysDBs`、`DomainHooks`、`set_domain_hooks_for_test` 等。`Glue` 还提供 `GetDomain`、`CreateSession`、`Open`、`UseOneShotSession`、进度和版本委托，并实现上游 `astersql_br_pkg_glue::Glue` trait。
- `pub mod infoschema_filter`：公开 `infoschema_filter.rs`。根级 API 包含 `ActionType`、`SchemaDiff`、`DBInfo`、`InfoSchema`、`Filter` 与 `NewInfoSchemaFilter`；它们是当前迁移边界使用的最小类型/trait，不是完整 TiDB 类型的别名。
- `parity_test`：根公共契约对等测试，覆盖构造副作用、数据库过滤、InfoSchema 过滤、Domain/Session 生命周期、TiKV 委托及 `Glue` trait 实现。
- `glue_test`：对照 Go `TestTheSessionIsoation` 的独立测试；通过 `DomainHooks` 和本地描述符模拟 Domain 启动、schema 版本递增、建库/策略/批量建表及清理。
- `test_support`：测试共享状态的 RAII 护栏。`domain_state_guard` 串行化会改动全局 Domain hooks/config bits 的用例，并在 `Drop` 时复位。

两个通配再导出意味着子模块新增的任何 `pub` 项都会自动进入 crate 根公共命名空间；这既是便利，也是名字冲突和无意扩大 API 面的风险。

## 执行流程

`lib.rs` 没有可执行语句；它只在编译时决定模块图和名称解析。使用根 API 时，实际流程落在子模块：

1. 调用 `New()` 时进入 `glue.rs::New`，设置 schema lease、跳过 Dashboard 注册、关闭慢日志和 1800 秒 Coprocessor 请求超时等进程级配置位，再构造持有 `StdIOGlue`、`TikvGlue`、启动互斥量与可选过滤器的 `Glue`。
2. `Glue::GetDomain`/`CreateSession` 经 `DomainHooks` 查询或创建 Domain；`startDomainAsNeeded` 在互斥区内二次检查，然后依次启动 owner manager、创建 Domain、启动 Domain。首次创建的 `GetDomain` 还初始化 MDL 和统计循环。
3. `Glue::Open`、进度记录和版本查询委托 `astersql-br-pkg-gluetikv::Glue`；`UseOneShotSession` 用 RAII guard 保证会话关闭，并按 `closeDomain` 决定是否关闭 Domain。
4. 数据库范围过滤先由 `FilterLoadSysDBs` 或 `FilterLoadSpecifiedDBAndSysDBs` 形成允许规则；`NewInfoSchemaFilter` 再把可选 allow 闭包转换为 `Filter`。`SkipLoadDiff` 先放行建库、放置策略、资源组及 `SchemaID == 0` 的变更，再按最新 InfoSchema 反查库名决定是否跳过。
5. 运行 crate 测试时，三个 `cfg(test)` 模块同时挂载。`test_support::domain_state_guard` 为会改变全局 hooks/config 的测试提供串行化和退出复位，随后 `parity_test` 与 `glue_test` 从根级再导出访问被测 API。

由于没有 Rust 生产下游，以上是 crate 内部可验证流程，不等同于 Rust BR 命令当前真实调用链。

## 数据与状态

crate 根不持有数据。实际状态由 `glue.rs` 管理：

- `GlobalConfigBits` 位于进程级 `OnceLock<Mutex<_>>` 后，记录 `New()` 对全局配置的移植语义；`GetGlobalConfigBits` 返回快照，测试复位函数只为隔离用例服务。
- `Glue` 内含 `startDomainMu: Mutex<()>`，串行化 Domain 首次启动；`InfoSchemaFilter` 是可共享的可选过滤器；`tikvGlue` 和 `StdIOGlue` 分别承接存储/进度与控制台能力。
- 活跃 `DomainHooks` 是进程级可替换实现，默认 hooks 提供迁移期行为，测试可注入记录型实现。`OneShotSessionGuard` 与 `OneShotDomainGuard` 把资源释放绑定到作用域。
- `brInfoSchemaFilter` 只拥有一个 `Send + Sync` 的 allow 闭包，不缓存 InfoSchema；每次判断都使用调用方传入的 diff/schema 快照。

`lib.rs` 顶部的 crate 级 `allow` 接受 Go 风格命名和迁移期未使用项。它会作用于整个 crate，可能掩盖新代码的普通命名或未使用警告，因此不应把“编译无警告”视为所有公开项均有生产使用方。

## 依赖与调用关系

`Cargo.toml` 声明三个直接路径依赖：

- `astersql-br-pkg-glue`：提供 `Glue`/`Session`/`Storage`/`Domain`/`CIStr` 等公共抽象，`glue.rs::Glue` 实现其 `Glue` trait。
- `astersql-br-pkg-gluetikv`：由 `Glue` 委托存储打开、进度、版本和指标行为。
- `astersql-errors`：提供 `SharedError` 与错误构造。

模块内部关系是 `lib.rs -> {glue.rs, infoschema_filter.rs}`；`glue.rs` 使用 `infoschema_filter::Filter` 作为 `Glue::InfoSchemaFilter` 和 `DomainHooks::GetOrCreateDomainWithFilter` 的边界，因此扁平再导出不代表两个模块彼此独立。

RustCodeGraph 对 `lib.rs` 只识别一个文件级节点，没有运行时 callers/callees，符合纯门面的性质。精确节点查询确认 `glue.rs::New -> global_bits`；测试源码则从 `crate::{...}` 使用根级再导出。仓库级 Cargo/源码搜索没有找到本 crate 的 Rust 生产下游。`tests/realtikvtest/brietest/harness.rs` 中存在名为 `gluetidb` 的局部模块，不能与本 Cargo crate 混为一谈。

Go 侧则已经接入真实主链：`br/cmd/br/cmd.go` 构造 `gluetidb.New()` 并安装 `NewInfoSchemaFilter`，`backup.go`/`restore.go` 使用数据库过滤器，多个 restore/checkpoint 测试也直接创建 Go Glue。那些路径是语义对照证据，不是 Rust 接线证据。

## 错误处理与边界

根文件不生成、包装或记录错误。子模块以 `SharedError` 传播 Domain、会话和存储失败；`?` 保持原错误链，显式 `New("failed to create domain")`/`New("domain missing after start")` 用于 hooks 未兑现“有 store 即可取得 Domain”的契约时。

`UseOneShotSession` 即使回调返回错误也会由 guard 关闭会话，并按选项关闭 Domain；它不吞掉回调错误。互斥锁使用 `unwrap()`，生产状态锁若中毒会 panic；测试专用 `domain_state_guard` 对锁中毒采用 `into_inner()` 以便仍能复位共享状态。

过滤器的边界是：没有 allow 时不安装过滤器；没有 DBInfo 时保守加载；特殊全局 DDL 和零 SchemaID 不跳过；非零 SchemaID 但缺少最新 InfoSchema 时跳过；反查不到 schema 也跳过。Rust 当前没有复制 Go 在跳过 diff 时的日志副作用，且使用最小 stand-in 类型，不能宣称与真实 `issyncer.Filter` 已完成接线。

通配再导出还形成公共 API 边界：两个子模块若出现同名公开符号会导致根级冲突；测试辅助公开函数也可能被自动提升。新增 `pub` 项前必须检查是否真应成为下游稳定契约。

## 并发与资源生命周期

`lib.rs` 不创建线程、任务、通道、事务或 I/O 资源。它公开的实现中，Domain 首次启动由 `Glue::startDomainMu` 串行化，并在持锁后再次检查现有 Domain，避免并发重复启动 owner manager。全局 hooks/config bits 也由互斥量保护。

一次性会话的生命周期由两个 Drop guard 管理：会话 guard 始终取出并关闭内部 session；Domain guard 只在 `closeDomain == true` 且取得 Domain 时关闭它。回调正常返回、错误返回或 unwind 都会触发析构，这是 Go `defer` 的 Rust 对应方式。

测试级 `DOMAIN_STATE_LOCK` 保证修改进程级 hooks/config 的测试不并行污染；guard 构造时清理旧状态，析构时再次复位。新增涉及全局 hooks/config 的测试必须继续放在独立测试文件并持有该 guard，不能依赖测试执行顺序。

`Filter`、`InfoSchema`、allow 闭包和 `DomainHooks` 都要求 `Send + Sync`；实际并发安全仍依赖注入实现兑现契约。过滤判断位于 schema 加载路径，若新增锁或昂贵闭包会直接增加该路径延迟。

## 与 Go 版本的对应关系

Go 没有与 Rust `lib.rs` 一一对应的源文件；Rust crate 根覆盖同目录 Go 包的 `glue.go` 与 `infoschema_filter.go`。

- `glue.rs` 对应 `glue.go` 的 `Glue`、`New`、数据库过滤、Domain/Session 生命周期、TiKV 委托和一次性会话。Rust 以 trait、`Result<_, SharedError>`、`Mutex` 和 Drop guard 对应 Go interface、error、`sync.Mutex` 与 `defer`。
- `infoschema_filter.rs` 对应 `infoschema_filter.go` 的 allow-to-skip 规则，但 Rust 使用本 crate 的最小 `ActionType`/`SchemaDiff`/`DBInfo`/`InfoSchema`/`Filter`，尚未直接实现完整 TiDB issyncer 接口；Go 在跳过 diff 时写日志，Rust 当前静默。
- Go `Glue` 包装真实 `sessionapi.Session` 并执行 SQL/DDL；Rust 当前通过 `DomainHooks` 和上游 glue crate 的轻量接口保留调用次序与公共行为。`glue_test.rs` 的注释明确真实 TiDB session/domain/testkit 在当前平台不可用，测试用 stand-in 验证形状和不变量，而不是完整集成。
- Go 包已由 `br/cmd/br` 和 restore/checkpoint 路径使用；Rust crate 尚无生产下游，CLI 仍有独立桩。这是当前最重要的迁移状态差异。

Go `glue_test.go::TestTheSessionIsoation` 的意图由 `glue_test.rs::test_the_session_isoation` 对齐；Rust 额外用 `parity_test.rs::go_rust_public_contract_matches` 覆盖过滤分支和资源关闭，并用 `glue_implements_public_glue_trait` 检查根级 `Glue` 可作为公共 trait 使用。

## 扩展指南

- 新增生产模块时，在本文件增加明确的 `#[path] pub mod`，并决定是否需要根级再导出；新增测试继续放进独立 `*_test.rs`，以 `#[cfg(test)]` 私有挂载。
- 向 `glue.rs` 或 `infoschema_filter.rs` 增加 `pub` 项前，先检查通配再导出是否会无意扩大根 API、与另一模块重名或暴露测试辅助。必要时改为显式再导出，而不是继续扩大 `allow` 范围。
- 扩展 Domain/Session 流程时，最可能修改 `DomainHooks`、`Glue::startDomainAsNeeded`、`GetDomain`、`CreateSession` 或 `UseOneShotSession`；必须保持双重检查、首次初始化、错误传播和 RAII 清理，并同步 `parity_test.rs`、`glue_test.rs`，同时对照 `glue.go`。
- 扩展过滤动作时修改 `infoschema_filter.rs::skip_load_diff_inner`，保持“全局特殊动作 -> 零 SchemaID -> latest InfoSchema -> allow”优先级，核对 Go ActionType 数值和真实 SchemaID 语义，并在 `parity_test.rs` 增加独立边界测试。
- 若要把该 crate 接入 Rust BR 主链，应从 Cargo 依赖和 `br/cmd/br` 的真实构造点完成最小接线，消除与 `stubs.rs`/realtikv harness 局部模块的重复边界；仅让 crate 自测通过不能作为生产接线证据。
- 用真实 TiDB/PD 类型替换 stand-in 时，应验证完整 Domain、session、InfoSchema 与错误/取消语义，不能删减 Go 行为来换取编译通过。性能风险集中在 Domain 启动锁和过滤器热路径；兼容风险集中在根级公开名称与全局配置副作用。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/gluetidb` 列出 `lib.rs`、两个生产模块、三个独立 Rust 测试和 Go 对照文件。
- RustCodeGraph：`node --file br/pkg/gluetidb/lib.rs --offset 1 --limit 200` 核对 38 行 crate 根、两个公开生产模块、三个 `cfg(test)` 私有模块与两组通配再导出；该文件只有一个文件级节点，符合无运行时符号的事实。
- RustCodeGraph：精确 `node br/pkg/gluetidb/glue.rs::New` 确认 `New -> global_bits` 及配置位/字段初始化；读取索引中的 `parity_test.rs`、`glue_test.rs`、`test_support_test.rs` 核对测试入口、hooks 模拟和共享状态 guard。
- Cargo/引用：读取 `br/pkg/gluetidb/Cargo.toml`，确认包名、crate 入口、Go 包映射和三个路径依赖；用 `rg` 搜索 `astersql-br-pkg-gluetidb`/`astersql_br_pkg_gluetidb`，未发现 Rust 生产下游，并识别 `tests/realtikvtest/brietest/harness.rs` 的同名局部模块。
- 源码与 Go 对照：读取 `br/pkg/gluetidb/glue.rs`、`infoschema_filter.rs`、`glue.go`、`infoschema_filter.go`，核对根级导出覆盖的真实符号、执行顺序、错误/锁/RAII 和 Go 差异；读取 `br/cmd/br/cmd.go`、`backup.go`、`restore.go` 的引用结果确认 Go 生产接线。
- 测试：读取 Rust `parity_test.rs`、`glue_test.rs`、`test_support_test.rs` 和 Go `glue_test.go`；同目录没有 `infoschema_filter_test.rs`/`infoschema_filter_test.go`，过滤器直接契约由 Rust parity 测试承担。
- 本任务只新增说明文档，按计划不运行 Cargo。结构验证命令及退出码在交付报告中给出；人工复核结论是：本文件存在于编译期组织 TiDB Glue 迁移库，运行逻辑完全下沉到两个子模块，安全扩展必须同时控制根 API、独立测试、Go 对照和尚未完成的生产接线边界。
