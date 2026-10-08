# `pkg/resourcemanager/pool/spool/option.rs`

## 文件定位

本文件是 `astersql-resourcemanager-pool-spool` crate 的构造选项模块。模块在 [`lib.rs`](./lib.rs) 中以私有 `mod option` 装配，再通过 `pub use option::*` 将类型和函数暴露为 crate 根 API；crate 的边界、入口文件和依赖由 [`Cargo.toml`](./Cargo.toml) 声明。它只负责描述并装载线程池的行为开关，不创建线程、不注册资源管理器，也不直接执行容量判断。

选项的唯一生产消费点是 [`Pool::new`](./spool.rs)：构造 `PoolInner` 时调用 `load_options(options)`，把结果保存进 `PoolInner.options`。之后 [`Pool::check_and_add_running`](./spool.rs) 在池满时读取 `options.Blocking`，决定继续等待还是拒绝提交。因此，本文件位于“调用方组装构造参数”与“spool 执行准入策略”之间。

## 核心职责

- 用 `Options` 集中保存 spool 构造期配置；当前只有 `Blocking` 一个布尔开关。
- 用 `OptionFn`/`Option` 表达函数式选项，使调用方无需直接构造或修改 `Options`。
- 由 `default_option` 建立默认策略，再由 `load_options` 按传入顺序应用覆盖，保证“后写覆盖先写”。
- 同时提供 Rust 风格的 `default_option`、`with_blocking` 和 Go 迁移兼容名称 `DefaultOption`、`WithBlocking`。

本文件不负责验证池容量、报告过载、等待空闲槽位或处理关闭。那些行为分别由 [`Pool::new`](./spool.rs)、`Error` 和 `Pool::check_and_add_running` 实现。

## 主要符号

- `pub struct Options { pub Blocking: bool }`：可复制的配置快照。`Clone + Copy + Eq + PartialEq` 便于按值保存、比较和测试；字段保持 Go 风格名称以兼容迁移接口。`Blocking = true` 表示满载提交持续等待，`false` 表示立即走拒绝路径。
- `pub type OptionFn = Box<dyn Fn(&mut Options)>`：拥有闭包的 trait object。闭包通过可变借用原地更新一个配置对象，不返回错误。
- `pub type Option = OptionFn`：Go 名称 `Option` 的兼容别名，两者在类型层面完全相同。
- `pub fn load_options(options: &[OptionFn]) -> Options`：内部装载入口。先调用 `default_option`，再按切片顺序逐个调用闭包，最后按值返回配置。
- `pub fn default_option() -> Options`：Rust 风格默认值构造器，固定返回 `Options { Blocking: true }`。
- `pub fn with_blocking(blocking: bool) -> OptionFn`：把参数捕获进 `move` 闭包；闭包应用时仅覆写 `Options.Blocking`。
- `pub fn DefaultOption() -> Options` 与 `pub fn WithBlocking(bool) -> Option`：只转发到上述 Rust 风格实现的兼容门面，没有第二套状态或分支。

## 执行流程

1. 调用方准备零个或多个 `OptionFn`，通常由 `with_blocking` 或 `WithBlocking` 创建。
2. 调用方把选项切片传给 [`Pool::new`](./spool.rs)；构造器调用 `load_options`。
3. `load_options` 先用 `default_option` 得到 `Blocking = true`。
4. 函数依次调用切片中的每个闭包。多个选项修改同一字段时，后执行者覆盖先执行者；空切片则保留默认值。
5. 完成后的 `Options` 按值进入 `PoolInner.options`，创建后不再通过本模块修改。
6. 提交任务时，`Pool::run` 或 `Pool::run_with_concurrency` 进入 `Pool::check_and_add_running`。有空闲槽位时配置不影响准入；只有无空闲槽位时，`Blocking = false` 才立即返回 `None`，上层映射为 `Error::Overload`，而 `true` 会按 `WAIT_INTERVAL` 轮询。
7. 池被停止后，准入循环先检查 `is_stop` 并返回 `None`；因此即使是默认阻塞模式，关闭也会终止等待，而不是永久停留在选项控制的轮询分支。

## 数据与状态

`Options` 是小型值对象，没有内部可变性、引用或资源句柄。`load_options` 使用单个局部变量 `opts` 完成所有修改，闭包只在装载期间借用它。返回之后，`PoolInner` 按值持有最终快照；选项列表和闭包本身不会保存在池中。

`with_blocking` 捕获的 `bool` 也是按值数据。当前 `OptionFn` 没有 `Send`、`Sync` 或显式生命周期约束，这与它仅在 `Pool::new` 当前线程中同步执行的事实一致。由于类型是 `Box<dyn Fn>` 而不是 `FnOnce`，每个选项可以被调用多次；不过 `load_options` 对给定切片中的每项只调用一次。

关键不变量是：默认配置始终为阻塞；选项严格按切片顺序应用；每个 `with_blocking` 只修改 `Blocking`；构造后的池读取稳定快照，不存在运行期选项热更新。

## 依赖与调用关系

本文件只使用 Rust 标准库中的 `Box`、闭包 trait 和派生宏，没有直接使用 [`Cargo.toml`](./Cargo.toml) 中的 `crossbeam-channel`、`prometheus` 或 `resourcemanager-dependency`。这些 crate 依赖服务于同一 crate 的 spool、任务通道和资源管理器接线。

静态关系如下：

- [`lib.rs`](./lib.rs) 声明并重新导出本模块，形成公开 API。
- [`Pool::new`](./spool.rs) 接收 `&[OptionFn]` 并调用 `load_options`，是唯一生产调用者。
- `load_options` 调用 `default_option`，并动态调用每个 `OptionFn` trait object。
- `DefaultOption` 调用 `default_option`；`WithBlocking` 调用 `with_blocking`。
- [`Pool::check_and_add_running`](./spool.rs) 是最终配置消费者；`Pool::run` 和 `Pool::run_with_concurrency` 是其上游提交路径。
- [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 和 [`spool_test.rs`](./spool_test.rs) 是主要 Rust 验证面；Go 对照测试位于 [`spool_test.go`](./spool_test.go)。

RustCodeGraph 将 `option.rs` 标记为被 `spool.rs`、`migration_aster_unit_test.rs`、`spool_test.rs` 使用；它还报告一个 `pkg/objstore/local.rs` 的同名/解析关联，但目标 crate 的直接生产接线以 `spool.rs` 中显式导入和调用为准，不据此推断跨 crate 选项共享。

## 错误处理与边界

本文件没有 `Result` 返回值，也没有主动错误分支。选项闭包的签名同样不能返回可恢复错误；自定义闭包若 panic，`load_options` 不捕获 panic，构造过程会随栈展开终止。标准构造器 `with_blocking` 仅赋值，不会自行失败。

需要区分以下边界：

- 空选项切片合法，结果为默认阻塞模式。
- 重复设置合法且不去重，最终值由最后一个相关选项决定。
- `Blocking = false` 并不保证所有提交都失败；它只在没有可用容量时避免等待。
- `Pool::check_and_add_running` 对“已停止”和“非阻塞满载”都返回 `None`，当前上层在准入检查后统一映射为 `Error::Overload`；提交前显式观察到已停止时则返回 `Error::Closed`。这是消费端边界，不是选项模块生成的错误。
- 本模块不校验池大小、并发请求数或资源管理器注册状态。

## 并发与资源生命周期

选项装载本身是同步、单线程、短生命周期操作：`OptionFn` 切片只在 `Pool::new` 中借用，配置闭包逐个执行，随后仅保留可复制的 `Options` 值。这里没有锁、原子变量、线程、通道或析构逻辑。

配置对并发的影响发生在下游：`PoolInner.options` 与容量、运行计数等共享状态一起被 `Arc<PoolInner>` 持有；`check_and_add_running` 在 `admission` 互斥锁保护的容量检查中读取 `Blocking`。阻塞模式在释放锁后休眠再重试，不会持锁等待；非阻塞模式在锁内判定后立即退出。`release_and_wait` 设置停止标志，使正在轮询的提交者退出。由于配置在构造后不可变，读取 `Blocking` 不需要额外同步。

资源所有权方面，调用方拥有 `Box<dyn Fn>`；选项切片销毁时闭包随之释放。`Options` 不拥有线程池资源，线程和 `JoinHandle` 的生命周期由 [`spool.rs`](./spool.rs) 管理。

## 与 Go 版本的对应关系

直接对照文件是 [`option.go`](./option.go)：

- Go `type Option func(opts *Options)` 对应 Rust `OptionFn = Box<dyn Fn(&mut Options)>`，并由 `Option` 保留原名。
- Go `loadOptions(options ...Option) *Options` 对应 Rust `load_options(&[OptionFn]) -> Options`。二者都先取默认值再顺序执行选项；Rust 用借用切片代替可变参数，并按值返回而非返回指针。
- Go `Options.Blocking` 与 Rust `Options.Blocking` 名称和含义一致。
- Go `DefaultOption` 默认 `Blocking: true`；Rust `default_option` 和兼容门面 `DefaultOption` 保持相同语义。
- Go `WithBlocking` 返回修改字段的闭包；Rust 的 `with_blocking`/`WithBlocking` 同样只改该字段。

消费侧也保持同一主干语义：Go [`NewPool`](./spool.go) 调用 `loadOptions` 并保存 `options`，Go `checkAndAddRunning` 在满载时读取 `p.options.Blocking`；Rust [`Pool::new`](./spool.rs) 与 `check_and_add_running` 对应这两个位置。实现机制不同之处包括 Go 使用 goroutine、指针和 `sync.RWMutex`，Rust 使用线程、值快照与 `Mutex`/原子量，但这些差异不改变本选项的默认值和准入含义。

## 扩展指南

新增构造选项时，应在本文件给 `Options` 增加有明确默认值的字段，在 `default_option` 中初始化，并增加单一职责的 `with_*` 构造器；若该 API 需要与 Go 迁移接口对齐，再增加薄的 Go 风格兼容门面。随后在真正消费行为的模块中读取新字段，避免把线程池执行逻辑塞入选项闭包。

需要同步更新独立测试，而不是把测试写进 `option.rs`：

- 在 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 扩展默认值、应用顺序、字段互不干扰和兼容名称测试。
- 若选项改变准入、调容或释放行为，在 [`spool_test.rs`](./spool_test.rs) 增加相应行为覆盖，并与 [`spool_test.go`](./spool_test.go) 的边界条件核对。
- 若 Go 侧同时演进，先核对 [`option.go`](./option.go) 的默认值、覆盖次序和公开名称，避免 Rust 独自引入不同语义。

兼容性风险主要来自改变默认 `Blocking`、调整重复选项的覆盖顺序或移除 `Option`/`DefaultOption`/`WithBlocking` 名称。性能风险主要在消费端：新增会导致循环等待、锁竞争或运行期分配的选项必须在 `spool.rs` 评估；本模块当前每个选项只有构造期一次装箱和一次动态调用。若未来需要跨线程存储选项闭包，必须重新评估并显式增加 `Send + Sync` 约束，不能假定当前类型已经线程安全。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库 Rust/Go 文件；目标 `option.rs` 已被索引。
- RustCodeGraph `files --filter pkg/resourcemanager/pool/spool`：确认目标源、模块入口、Go 对照与独立 Rust/Go 测试集合。
- RustCodeGraph `node --file pkg/resourcemanager/pool/spool/option.rs`：核对 `Options`、`OptionFn`、`Option`、`load_options`、`default_option`、`with_blocking`、`DefaultOption`、`WithBlocking` 的源码和签名。
- RustCodeGraph `query` 与 `explore`：核对目标符号及 `Pool::new -> load_options`、`Pool::run`/`run_with_concurrency -> check_and_add_running` 的调用关系；精确 `callers/callees` 子命令对这些符号未返回结果，故调用边以已索引源码中的显式调用复核。
- [`Cargo.toml`](./Cargo.toml) 与 [`lib.rs`](./lib.rs)：核对 crate 名、入口、依赖、模块私有性和公开再导出。
- [`spool.rs`](./spool.rs)：核对配置保存位置、满载分支、轮询、停止、锁与线程资源生命周期。
- [`option.go`](./option.go) 与 [`spool.go`](./spool.go)：核对函数式选项、默认值、按序覆盖及消费端的 Go 语义。
- [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)：`options_apply_in_order_and_default_to_blocking` 验证默认值和后写覆盖，`go_option_api_names_remain_available` 验证兼容 API，`nonblocking_run_enforces_capacity_and_recovers_panics` 与 `release_stops_blocked_submitters_then_waits_for_running_tasks` 验证选项在过载和释放边界中的效果。
- [`spool_test.rs`](./spool_test.rs)：`TestRunOverload`、`TestRunWithNotEnough`、`TestRunWithNotEnough2` 验证非阻塞满载拒绝，`TestPoolTuneScaleUpAndDown` 验证阻塞提交随扩容继续执行。

本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求文档存在且恰好包含上述 11 个固定二级标题。
