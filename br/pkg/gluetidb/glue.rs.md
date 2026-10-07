# `br/pkg/gluetidb/glue.rs`

## 文件定位

本文件属于 Cargo crate `astersql-br-pkg-gluetidb`；crate 入口 [`lib.rs`](lib.rs) 以 `pub mod glue` 挂载本模块并用 `pub use glue::*` 扁平导出公开符号。其直接依赖由 [`Cargo.toml`](Cargo.toml) 限定为 `astersql-br-pkg-glue`（公共 Glue/Session/Storage 契约）、`astersql-br-pkg-gluetikv`（存储、进度和版本实现）与 `astersql-errors`（共享错误）。

它是 Go [`glue.go`](glue.go) 的迁移对应物，处在 BR 通用接口与 TiDB Domain/Session 生命周期之间：对外实现 `astersql_br_pkg_glue::Glue`，对下把存储及 CLI 辅助能力委托给 `gluetikv::Glue`，把尚未直接链接到完整 TiDB 运行时的 Domain/Session 操作抽象为 `DomainHooks`。因此它既是适配层，也是当前迁移边界；默认 hooks 不是完整生产 Domain 实现。

## 核心职责

- `New` 建立 TiDB Glue，并记录 Go 构造函数的四项进程级副作用：schema lease 已设置、跳过 Dashboard 注册、关闭慢日志、Coprocessor 请求超时为 1800 秒。
- `FilterLoadSysDBs` 与 `FilterLoadSpecifiedDBAndSysDBs` 决定部分加载 InfoSchema 时保留哪些数据库：`mysql`、大小写敏感匹配原名的 `__TiDB_BR_Temporary_` 临时库，以及调用方显式指定的库。
- `getDomainInner`、`startDomainAsNeeded`、`GetDomain` 和 `CreateSession` 编排 Domain 查询、单次启动、首次初始化与会话创建。
- `UseOneShotSession` 把会话和可选 Domain 的清理绑定到作用域，保证正常返回和回调错误都执行清理。
- `Open`、`StartProgress`、`Record`、`GetVersion` 委托 `TikvGlue`；`OwnsStorage` 与 `GetClient` 固定声明 BR CLI 对资源和客户端类型的契约。
- `impl GlueTrait for Glue` 只做公开 trait 到固有方法的转发，确保调用方可通过通用 `Glue` trait 使用本实现。

## 主要符号

- `brComment: &str`：BR 发出 SQL 时使用的审计标记。当前 Rust 文件仅导出常量，未在本文件的会话转发逻辑中消费。
- `GlobalConfigBits`：可克隆的进程配置快照，字段对应 Go `New` 的关键副作用；`global_bits` 用 `OnceLock<Mutex<_>>` 延迟初始化唯一实例，`GetGlobalConfigBits` 读取快照，`reset_global_config_bits_for_test` 供独立测试复位。
- `FilterLoadSysDBs(&CIStr) -> bool`：比较 `CIStr.L` 与 `mysql`，并比较 `CIStr.O` 的 BR 临时库前缀。
- `FilterLoadSpecifiedDBAndSysDBs(&[String]) -> impl Fn(&CIStr) -> bool`：先把指定名称小写化并收集进 `HashSet`，再返回拥有该集合的线程安全闭包。
- `DomainHooks`：Domain/Session 生命周期的注入接口，包含查询或创建 Domain、启动 owner、启动 Domain、初始化 MDL、启动统计更新循环、创建 Session 与关闭 Domain 七类操作。
- `NopDomainHooks`：默认实现。除 `CreateSession` 外均为空操作或返回 `None`；`CreateSession` 明确返回 `domain hooks: CreateSession not configured`，所以它是防止静默误用的迁移桩，不是真实 TiDB 接线。
- `Glue`：持有 `StdIOGlue`、私有 `TikvGlue`、Domain 启动互斥量和可选 `InfoSchemaFilter` 的适配器主体。
- `OneShotSession`：实现完整 `Session` 转发接口的共享句柄；每次调用都通过 `with_inner` 锁定并访问底层会话。
- `OneShotSessionGuard` / `OneShotDomainGuard`：RAII 清理器，分别在 `Drop` 中调用 `Session::Close` 和 `DomainHooks::CloseDomain`。
- `New() -> Glue`：构造入口；`GetDomain`、`CreateSession`、`UseOneShotSession` 是 Domain/Session 主入口，其他公开方法完成 Glue trait 的外围契约。

## 执行流程

1. 构造时，`New` 锁定全局配置位、写入四项 BR 默认值，然后创建 `TikvGlue`、启动互斥量和空的 `InfoSchemaFilter`。
2. `GetDomain` 先以 `store=None` 查询启动前是否已有 Domain；随后调用 `startDomainAsNeeded`。后者持有 `startDomainMu` 后再次查询，已有 Domain 就直接返回，否则依次调用 `StartOwnerManager`、以 `store=Some` 创建/取得 Domain、再调用 `StartDomain`。
3. `GetDomain` 在启动后再次以真实 store 取得 Domain。如果启动前不存在 Domain，它继续按顺序执行 `InitMDLVariable` 和 `UpdateTableStatsLoop`；已有 Domain 时跳过这两项一次性初始化。
4. `CreateSession` 先复用 `startDomainAsNeeded`，再调用当前 hooks 的 `CreateSession`。默认 hooks 会在这里报错，只有注入了真实或测试 hooks 才能得到可用会话。
5. `UseOneShotSession` 创建会话后立即建立 `OneShotSessionGuard`，取得 Domain 并初始化 MDL，再根据 `closeDomain` 建立可能持有 Domain 的 `OneShotDomainGuard`。回调收到一个转发句柄；回调结果原样返回，离开函数时先释放 Domain guard、再释放 session guard，因而 Domain（若要求）先关闭，会话随后关闭。
6. `Open`、进度和版本相关调用不进入 Domain 链，直接落到 `tikvGlue`；公开 trait 实现再把动态分派入口转回上述固有方法。

## 数据与状态

- `GlobalConfigBits` 是进程级共享状态。`OnceLock` 保证容器只初始化一次，内部 `Mutex` 保护读写；`GetGlobalConfigBits` 返回克隆，调用方不能绕过锁修改全局值。
- `OVERRIDE_HOOKS` 是进程级可变测试接线。`active_hooks` 每次克隆当前 `Arc`；没有覆盖时回退到 `default_hooks` 中的单例 `NopDomainHooks`。
- `Glue::InfoSchemaFilter` 为每个 Glue 实例保存可选过滤器，并由 `getDomainInner` 传给 hooks。过滤器本身是 `Arc<dyn Filter>`，可安全共享。
- `startDomainMu` 属于单个 `Glue` 实例，只串行化同一实例上的 Domain 启动。若多个独立 `Glue` 实例共享同一底层 Domain，最终去重还依赖 hooks 的 `GetOrCreateDomainWithFilter(None)` 查询语义。
- 一次性会话存储为 `Arc<Mutex<Option<Box<dyn Session>>>>`。guard 通过 `Option::take` 获得唯一所有权并关闭底层会话；回调保留的句柄随后再调用会触发 `one-shot session already closed` panic，而不会访问已释放会话。
- 指定数据库过滤闭包拥有规范化后的 `HashSet<String>`，创建后不再修改；查询使用 `CIStr.L`，临时库判断则使用保留原始大小写的 `CIStr.O`。

## 依赖与调用关系

向上，本 crate 的 [`lib.rs`](lib.rs) 导出本文件的 API；`GlueTrait for Glue` 让 BR 中接受 `dyn Glue`/泛型 Glue 的流程间接调用这里。RustCodeGraph 显示公共 `Glue` trait 在备份、恢复、流式备份、连接管理等 BR 路径被广泛依赖；在本文件的直接静态边上，trait 方法分别转发到 `Glue::{GetDomain, CreateSession, Open, OwnsStorage, StartProgress, Record, GetVersion, UseOneShotSession, GetClient}`。由于动态 trait 分派和同名 Go/Rust 符号会使静态图产生歧义，不能仅凭图把每个 BR 任务函数认定为本具体 `Glue` 的直接调用者。

向下，`getDomainInner` 调 `active_hooks().GetOrCreateDomainWithFilter`；`startDomainAsNeeded` 调 `StartOwnerManager`、`getDomainInner`、`StartDomain`；`GetDomain` 再调 `InitMDLVariable` 和 `UpdateTableStatsLoop`；`CreateSession` 调 hooks 的同名方法；一次性 guard 调 `Session::Close` 和 `CloseDomain`。`OneShotSession` 的十个 Session 方法都只经 `with_inner` 转发到底层 `Session`，不在本层改写 SQL、参数或返回错误。

存储和 CLI 辅助链为 `Glue::{Open, StartProgress, Record, GetVersion} -> TikvGlue`。`StdIOGlue` 作为公开字段嵌入，但 Rust 不具备 Go 的匿名字段方法提升，本文件的 `GlueTrait` 也未实现 console 扩展接口；当前已验证的公开接线以 `astersql_br_pkg_glue::Glue` 为准。

## 错误处理与边界

- 所有可失败的 hooks 和 `TikvGlue::Open` 返回 `SharedError`，本层主要用 `?` 原样传播；回调错误也由 `UseOneShotSession` 原样返回。与 Go 的 `errors.Trace` 相比，Rust 本层没有额外包装上下文。
- `startDomainAsNeeded` 在带 store 的查询仍返回 `None` 时主动生成 `failed to create domain`；`GetDomain` 启动后仍缺 Domain 时生成 `domain missing after start`。
- 默认 `NopDomainHooks::CreateSession` 明确失败；默认 `GetOrCreateDomainWithFilter` 返回 `None`，所以默认配置下 `GetDomain` 会在创建检查处失败。真实生产可用性取决于仓库其他位置是否注入真实 hooks，本文件本身没有这样的安装函数。
- 所有普通 `Mutex::lock` 都使用 `unwrap`。若持锁线程 panic 导致锁中毒，后续配置、hooks、启动或会话访问会 panic；只有测试状态护栏对 poisoned lock 做了恢复。
- `FilterLoadSysDBs` 只把精确小写 `mysql` 视为系统库，这比 Go `metadef.IsSystemDB` 可识别的系统库集合更窄；这是当前 Rust 实现事实。BR 临时库前缀对 `CIStr.O` 做大小写敏感匹配。
- `UseOneShotSession` 若在创建会话后、建立 Domain guard 前发生 `getDomainInner` 或 `InitMDLVariable` 错误，session guard 仍关闭会话；尚未建立 Domain guard，因此即使 `closeDomain=true` 也不会关闭 Domain。这与 Go 中 Domain defer 只在 MDL 初始化成功之后注册的顺序一致。
- 回调若主动调用转发会话的 `Close`，离开作用域时 guard 还会再次对同一个底层对象调用 `Close`；本层未声明底层 `Close` 必须幂等。现有测试只覆盖 guard 触发一次关闭，未覆盖回调主动关闭。

## 并发与资源生命周期

Domain 启动采用“锁外预查（`GetDomain`）/锁内复查（`startDomainAsNeeded`）”模式。真正阻止同一 `Glue` 上双启的是 `startDomainMu` 内的第二次查询；owner 启动、Domain 创建和 Domain 启动都在锁的临界区内串行执行。`GetDomain` 的“是否首次”依据锁外快照：并发调用可能都观察到无 Domain，并在各自返回路径执行 MDL/统计初始化，因此启动被去重并不等于一次性初始化在所有并发条件下严格只执行一次，这是当前实现需要 hooks/上层容忍的边界。

一次性会话的所有操作由同一 `Mutex` 串行化。`Arc` 允许回调把句柄移出当前调用栈，但 owning guard 不依赖句柄引用计数来决定清理：函数退出即 `take` 并关闭底层 Session。局部变量逆序析构使 `_domain_guard` 先于 `_session_guard` 释放；`closeDomain=false` 时 Domain guard 不持有 Domain。`parity_test.rs` 验证成功回调会话关闭一次、错误回调仍关闭会话，以及仅 `closeDomain=true` 关闭 Domain。

进程级 hooks/config 会影响并行测试。[`test_support_test.rs`](test_support_test.rs) 用 `DOMAIN_STATE_LOCK` 串行化相关测试，并在 guard 的创建和析构时清除 hooks、复位配置位，防止跨测试泄漏。

## 与 Go 版本的对应关系

Rust `New` 对齐 Go 的 schema lease、Dashboard、慢日志和 1800 秒 Coprocessor 超时副作用，但只把结果记录在 `GlobalConfigBits`，没有直接调用 Go 中的 `vardef`/`config` 全局配置设施。`Glue` 的启动互斥、InfoSchema 过滤器、TiKV 委托、`OwnsStorage=true` 与 `ClientCLP` 均保持同一意图。

Domain 主流程与 Go 顺序相同：空 store 探测、互斥内复查、启动 DDL owner、创建并启动 Domain；`GetDomain` 在首次创建后初始化 MDL 与统计循环；`CreateSession` 先保证 Domain。Rust 通过 `DomainHooks` 替代 Go 对 `session`、`ddl`、`domain` 包的直接调用，这是可测试的迁移接缝，并非 Go 完整运行时能力本身。

会话层差异更明显。Go `tidbSession` 在本文件内实现内部 SQL 执行、结果集触发、事务 InfoSchema 清理、建库建表、放置策略、AlterTableMode 与 RefreshMeta；Rust `OneShotSession` 仅转发已有 `dyn Session`，真实语义属于注入的 Session 实现。Go 还提供 `WrapSession`、`tidbSession` 和批量建表实现，当前 Rust 文件没有对应生产实现。相反，Rust 增加 RAII guard，以适配可被回调持有的 boxed trait object，同时维持 Go `defer se.Close()`/可选 `dom.Close()` 的退出语义。

数据库过滤的指定库小写匹配和临时库原名匹配与 Go 一致；系统库判断并非完整等价：Go 委托 `metadef.IsSystemDB`，Rust 当前只接受 `mysql`。[`glue_test.rs`](glue_test.rs) 说明真实 TiDB `session/domain/testkit/model` 在该平台不可用，测试用 hooks 和描述符模拟调用顺序及 schema 版本行为，因此这些测试证明适配契约，不证明与真实 TiDB 集成已经接通。

## 扩展指南

- 接入真实 TiDB 运行时时，应实现并安装生产 `DomainHooks`，重点保持 `store=None` 只查询、`Some(store)` 按需创建的约定，以及 owner → Domain → MDL → stats 的顺序。新增安装机制时要避免与仅供测试的全局覆盖互相踩踏。
- 扩充系统库识别前，先以 Go `metadef.IsSystemDB` 的当前集合为基准补齐独立 Rust 测试；不要只改常量而遗漏大小写字段选择和临时库规则。
- 修改 `UseOneShotSession` 时必须同时验证成功、回调错误、MDL/Domain 查询错误、`closeDomain` 两值、回调保留句柄以及回调主动 `Close` 的行为；清理顺序改变可能影响 Domain 依赖仍存活 Session 的实现。
- 若把 Go `tidbSession` 语义迁入 Rust，应放在独立生产源文件并在同目录独立测试文件中覆盖内部 SQL 结果集触发、Txn InfoSchema 清理、DDL query string 恢复和批量建表；不要把测试写入 `glue.rs`。
- 改动 Domain 并发逻辑时，应针对多个线程同时调用 `GetDomain` 添加回归测试，分别计数 `StartOwnerManager`、`StartDomain`、`InitMDLVariable` 与 `UpdateTableStatsLoop`，明确是只保证启动一次还是所有初始化都严格一次。
- 修改公共方法签名或委托关系时，同步检查 `astersql-br-pkg-glue::Glue` trait、[`lib.rs`](lib.rs) 的再导出、[`parity_test.rs`](parity_test.rs) 的 trait 接线断言，以及 [`glue_test.rs`](glue_test.rs) 的 Go 等价场景。

## 验证依据

- 源码全貌：[`glue.rs`](glue.rs) 的常量、`GlobalConfigBits`、过滤器、`DomainHooks`/`NopDomainHooks`、`Glue`、两个 RAII guard、固有方法与 `GlueTrait` 实现。
- crate 边界：[`Cargo.toml`](Cargo.toml) 的包名、`lib.rs` 入口及三项直接依赖；[`lib.rs`](lib.rs) 的模块挂载、测试隔离与扁平再导出。
- Go 对照：[`glue.go`](glue.go) 的 `New`、过滤器、Domain/Session 启动、`UseOneShotSession`、TiKV 委托和 `tidbSession` 具体行为；[`glue_test.go`](glue_test.go) 的会话隔离、策略创建和批量建表场景。
- Rust 测试：[`glue_test.rs`](glue_test.rs) 验证关闭 bootstrap Domain 后 `CreateSession` 触发 owner/Domain 启动，并以测试替身覆盖 schema 版本、DDL 和表/策略关系；[`parity_test.rs`](parity_test.rs) 验证配置位、过滤边界、公开 trait、Open 委托，以及一次性会话在成功/错误路径上的清理；[`test_support_test.rs`](test_support_test.rs) 验证全局测试状态的串行与复位策略。
- RustCodeGraph：索引状态为 7032 个 Rust 文件、目标 `glue.rs` 含 67 个符号；`node --file` 核对了全部 466 行；`callees`/`explore` 确认 `getDomainInner -> GetOrCreateDomainWithFilter`、`startDomainAsNeeded -> StartOwnerManager/GetOrCreateDomainWithFilter/StartDomain`、`GetDomain -> InitMDLVariable/UpdateTableStatsLoop`、`UseOneShotSession -> CreateSession/InitMDLVariable`，以及 guard 到 `Close`/`CloseDomain` 的边。
- 验证限制：本任务按计划不运行 Cargo。调用图对同名 Go/Rust 方法和 trait 动态分派存在歧义，所以上游应用位置只作为公共 Glue 契约的间接使用证据；真实生产 hooks 的安装在本文件及其直接 crate 入口中未找到，本文不宣称完整 TiDB Domain 已接通。
