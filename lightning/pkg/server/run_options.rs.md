# `lightning/pkg/server/run_options.rs`

## 文件定位

本文件属于 `astersql-lightning-pkg-server` crate。crate 入口 `lightning/pkg/server/lib.rs` 以 `mod run_options` 装入本模块并通过 `pub use run_options::*` 重新导出其公开项；`lightning/pkg/server/Cargo.toml` 则把该 crate 标为 Go 包 `lightning/pkg/server` 的 Rust 移植库。它位于 Lightning 单次导入入口与具体任务初始化之间：`Lightning::RunOnceWithOptions` 接收一组 `RunOption`，组装 `options`，随后把它传给 `Lightning::run`。

该文件只定义单次任务的可选依赖和覆盖方式，不启动任务、不打开存储、不连接数据库，也不负责释放资源。真实消费逻辑位于 `lightning/pkg/server/lightning.rs` 的 `RunOnceWithOptions`、`run`、`initDataSource` 和 `initDBAndKeyspace`。

## 核心职责

- 用 `options` 集中保存一次 Lightning 任务可注入的外部存储、checkpoint 存储及名称、Prometheus 工厂与注册器、logger、重复数据指示器和测试数据库。
- 用 `RunOption = Box<dyn FnOnce(&mut options) + Send>` 复刻 Go `Option func(*options)` 的函数式选项模式，使调用方按需覆盖默认项，而不扩张 `RunOnceWithOptions` 的固定参数表。
- 为每个槽位提供单一用途的 `With*` 构造函数。除 `WithCheckpointStorage` 同时设置存储与名称外，每个闭包只改一个字段。
- 保持“配置阶段无 I/O、无验证”的边界：helper 只移动值并写入参数包，配置调整、存储探测、数据库连接、指标注册等副作用都延后到 `lightning.rs`。

## 主要符号

- `pub struct options`：内部参数包。类型本身公开是当前 crate 移植方式的结果，但名称为小写且主要由同 crate 的 `lightning.rs` 使用。
- `impl Default for options`：建立空注入状态；两个存储、两个指标对象、重复指示器和数据库均为 `None`，checkpoint 名为空字符串，logger 来自 `log::L()`。
- `pub type RunOption = Box<dyn FnOnce(&mut options) + Send>`：一次性、可在线程间转移的配置闭包。`FnOnce` 允许闭包取得所注入值的所有权；它不要求 `Sync`，也没有返回错误的通道。
- `WithDumpFileStorage(StorageRef)`：设置预先打开的数据源存储。
- `WithCheckpointStorage(StorageRef, String)`：成对设置 checkpoint 存储和 checkpoint 对象名。
- `WithPromFactory(Factory)`、`WithPromRegistry(Registry)`：分别覆盖指标构造工厂和注册表。
- `WithLogger(zap::Logger)`：把底层 logger 包装为 `log::Logger`；新包装的 `fields` 为空，`entries` 使用默认值。
- `WithDupIndicator(atomic::Bool)`：注入由下游 controller 更新/观察的重复数据指示器。
- `WithDB(sql::DB)`：注入测试数据库，避免 `initDBAndKeyspace` 走生产配置建连路径。Go 文件没有对应导出 helper，Go 包内测试直接写私有 `db` 字段。

## 执行流程

1. `Lightning::RunOnceWithOptions` 在 `lightning.rs:621` 创建 `options`：指标工厂和注册器先取自 `Lightning` 实例，logger 取 `log::L()`，其余字段采用 `Default`。
2. 它按 `Vec<RunOption>` 的顺序逐个调用 `opt(&mut o)`。同一字段被多次配置时，后执行的闭包覆盖先前值；本文件没有冲突检测。
3. 若 `dumpFileStorage` 已注入，入口把 `taskCfg.Mydumper.SourceDir` 改为 `noop://`，使配置校验不再要求由配置指定真实数据源；随后执行 `Config::Adjust` 并分配任务 ID。
4. `Lightning::run` 从参数包选择指标工厂/注册器，构建并注册任务指标，把 logger 放入任务 context，并建立任务取消句柄。
5. 非 Import-Into 后端由 `initDataSource` 优先使用 `dumpFileStorage`；没有注入时才根据 `SourceDir` 打开存储。controller 参数同时接收 checkpoint 存储/名称、重复指示器，并用 `dumpFileStorage.is_none()` 决定 `OwnExtStorage`。
6. `initDBAndKeyspace` 优先克隆 `o.db`；只有未注入数据库时才调用 `DBFromConfigLocal`。任务结束后，`run` 取消 context、清空 `cancel` 和 `importer`、广播结束状态并从注册器注销指标。

## 数据与状态

`options` 是一次任务独占的可变参数包，没有全局单例。`StorageRef`、指标对象、`atomic::Bool` 和 `sql::DB` 在下游按各自的可克隆句柄语义传递；本文件仅保存 `Option<T>`，不声明资源所有权关闭策略。

几个字段存在成对不变量或覆盖规则：`checkpointStorage` 与 `checkpointName` 应由 `WithCheckpointStorage` 一起设置；单独的空名称在默认状态是合法的，由下游 checkpoint 逻辑解释。`dumpFileStorage = None` 表示由任务配置打开并拥有外部存储，`Some` 表示调用方提供，`run` 据此设置 `OwnExtStorage`。`promFactory`/`promRegistry` 在纯 `Default` 中为空，但标准入口会先用 Lightning 实例值填充；`run` 仍为二者保留默认构造回退。`db` 明确是测试注入槽位。

## 依赖与调用关系

上游主链是 `lightning/cmd/tidb-lightning/main.rs` 的应用适配器调用 `Lightning::RunOnceWithOptions`；当前二进制路径传入空选项列表。RustCodeGraph 确认 `RunOnceWithOptions` 实例化 `run_options.rs::options` 并调用 `Lightning::run`。各 `With*` helper 当前在 Rust 生产代码中没有直接调用点，直接覆盖证据来自 `lightning/pkg/server/parity_test.rs`；这说明 API 已提供并通过契约测试，但仓库内当前生产适配器尚未利用这些覆盖项。

下游依赖均通过 crate 根的移植/替身类型进入：`storeapi::StorageRef` 表示外部对象存储，`promutil::{Factory, Registry}` 支撑任务指标，`log::Logger`/`zap::Logger` 支撑上下文日志，`atomic::Bool` 传递重复检测状态，`sql::DB` 支撑数据库测试注入。`lightning.rs` 再把这些值交给数据源初始化、metrics、context、`ControllerParamLocal` 和 `initDBAndKeyspace`。

## 错误处理与边界

所有 `With*` 闭包的返回类型都是 `()`，因此本文件既不产生也不传播业务错误。无效组合不会在配置阶段被拒绝：例如 checkpoint 名是否有效、存储能否遍历、注册器是否适用、数据库是否可查询，都由后续消费者处理。

错误边界位于 `lightning.rs`：`Config::Adjust` 可拒绝配置，`initDataSource` 可返回空数据源或存储错误，默认数据库创建会包装为 `ErrDBConnect`，TLS 配置和 importer 创建也会失败。注入 `dumpFileStorage` 只绕过按配置打开数据源，并不绕过后续 `WalkDir` 可用性/非空检查。`WithLogger` 会创建没有附加字段和历史 entries 的新 `log::Logger`，扩展时不能假定旧包装状态被保留。

## 并发与资源生命周期

`RunOption` 要求 `Send`，可随任务启动参数跨线程转移；它是 `FnOnce`，应用一次后即消费，避免重复使用已经移动的资源。选项应用本身在 `RunOnceWithOptions` 中串行发生，对同一 `options` 的覆盖顺序确定，不涉及锁。

本文件不关闭存储、数据库或注册器。`run` 为任务创建可取消 context，并在结束时清除受 `cancelLock` 保护的 `cancel`/`importer` 状态；指标在任务开始注册、结束注销。外部数据源的所有权由 `OwnExtStorage = o.dumpFileStorage.is_none()` 表达：调用方注入的存储不能被当成 Lightning 自行创建的资源。`dupIndicator` 使用原子布尔句柄以支持下游并发更新；本文件只传递它，不读写其值。

## 与 Go 版本的对应关系

直接对照文件是 `lightning/pkg/server/run_options.go`。字段集合和六个 Go helper（dump storage、checkpoint storage、prom factory、prom registry、logger、dup indicator）在 Rust 中逐项保留；Go 的 nil 指针/接口对应 Rust 的 `Option<T>`，Go `Option func(*options)` 对应带 `Send` 约束的 boxed `FnOnce`。

Rust 默认实现显式把可选字段设为 `None`，并初始化空 checkpoint 名和全局 logger；标准入口再覆盖实例级指标对象，这与 Go `RunOnceWithOptions` 的对象字面量初始化顺序一致。Rust `WithLogger` 接收拥有所有权的 `zap::Logger`，Go 接收 `*zap.Logger`。Rust 额外公开 `WithDB` 作为测试 helper，而 Go 测试因处在同一 package 直接设置 `options.db`。

当前移植仍有可见差异：Go failpoint `setExtStorage` 会实际解析并创建存储，Rust 对应注入点目前为空闭包；这属于 `lightning.rs` 而非本文件的职责，但意味着不能仅凭本文件 helper 推断两端 failpoint 行为完全等价。Rust 的二进制适配器当前传空 `Vec<RunOption>`，而 API 仍保留库集成所需注入面。

## 扩展指南

新增任务级可注入依赖时，应同时完成以下接线：在 `options` 增加明确的 `Option<T>` 或安全默认值；更新 `Default`；新增只修改对应字段的 `With*`；在 `RunOnceWithOptions`/`run` 的恰当阶段消费；对照更新 Go 语义或明确记录 Rust 特有差异。若字段代表成对配置，优先用一个 helper 原子设置，避免出现半配置状态。

测试逻辑必须继续放在独立文件。应扩展 `lightning/pkg/server/parity_test.rs` 验证默认值、helper 应用和多次覆盖顺序；涉及运行期消费、错误清理或数据库注入时，应扩展 `lightning/pkg/server/lightning_serial_test.rs`。同步关注 Go 的 `run_options.go` 与 `lightning_serial_test.go`。兼容风险主要是改变默认值/覆盖优先级；资源风险主要是误判外部存储所有权或遗漏指标注销；性能风险主要来自把 I/O、建连或重型克隆错误地提前到 option 闭包中。

## 验证依据

- 源码与装配：`lightning/pkg/server/run_options.rs`、`lightning/pkg/server/lib.rs`、`lightning/pkg/server/Cargo.toml`。
- Rust 消费链：`lightning/pkg/server/lightning.rs` 中的 `RunOnceWithOptions`（应用 options）、`run`（注册指标和组装 controller）、`initDataSource`（选择数据源/所有权）、`initDBAndKeyspace`（测试 DB 或默认建连）。
- Rust 测试：`lightning/pkg/server/parity_test.rs:105-118` 直接应用六个 helper 并检查关键字段；`lightning/pkg/server/lightning_serial_test.rs` 覆盖无 options 的入口错误清理以及直接注入 logger/DB 后的运行错误路径。当前未发现直接调用 `WithDB` 的测试。
- Go 对照：`lightning/pkg/server/run_options.go`、`lightning/pkg/server/lightning.go`、`lightning/pkg/server/lightning_serial_test.go`。
- RustCodeGraph：`status` 显示目标 Rust 文件已索引；`node --file lightning/pkg/server/run_options.rs` 核对全部 124 行和 10 个符号；`callees RunOnceWithOptions` 确认 Rust 入口实例化 `options` 并调用 `run`；逐项 `callees With*` 显示 helper 无业务调用（`WithLogger` 仅有包装初始化边）。图查询未解析这些 boxed closure 的 callers，因此调用点以 `rg` 对索引内 Rust/Go/测试文件补证。

