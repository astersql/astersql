# `pkg/resourcemanager/rm.rs`

## 文件定位

本文件是实例级资源管理器的状态与生命周期核心，源码见 [`rm.rs`](rm.rs)。它负责构造并持有进程内的池注册表、调度器、CPU 观察器、退出状态和后台任务等待器；具体的单轮调度决策与调容执行位于相邻的 [`schedule.rs`](schedule.rs)，二者通过多个 `impl ResourceManager` 共同组成完整实现。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：包名为 `astersql-resourcemanager`，库入口是 [`lib.rs`](lib.rs)，后者公开 `rm` 模块并再导出 `InstanceResourceManager`、`NewResourceManger`、`RandomName` 和 `ResourceManager`。直接依赖中，`uuid` 为测试辅助名生成 v4 UUID；CPU 观察能力来自路径依赖 `astersql-util-cpu`；调度器与池公共类型来自路径依赖 `astersql-resourcemanager-scheduler`。

在应用主链上，[`cmd/tidb-server/main.rs`](../../cmd/tidb-server/main.rs) 在创建存储、Domain 和 Server 前调用全局单例 `Start`，在信号退出与 keyspace 激活退出路径调用 `Stop`。池侧的直接接线见 [`pool/spool/spool.rs`](pool/spool/spool.rs)：`Pool::new` 注册自身，`release_and_wait` 在回收 worker 后注销。因此本文件连接了服务进程生命周期、可调度池生命周期和 [`schedule.rs`](schedule.rs) 的周期调容流程。

## 核心职责

1. `InstanceResourceManager` 以 `LazyLock<ResourceManager>` 提供按需初始化、进程内唯一的资源管理器（`rm.rs:34-36`）。
2. `NewResourceManger` 与 `ResourceManager::NewResourceManger` 构造默认实例，安装一个 CPU 调度器、一个 CPU 观察器、空的分片池映射、退出状态和等待器（`rm.rs:73-100`）。
3. `Start` 启动 CPU 观测，并通过 `WaitGroupWrapper` 启动每 100 ms 触发一次 `schedule` 的后台循环（`rm.rs:102-136`）。
4. `Stop` 停止 CPU 观察器、永久关闭调度生命周期并等待调度线程结束（`rm.rs:138-161`）。
5. `Register` / `Unregister` 管理带名称和 `Component` 的 `GoroutinePool`；`Reset` 原子替换整个池映射，供测试隔离状态（`rm.rs:163-210`）。
6. `RandomName` 生成可解析且高概率唯一的 UUID 字符串，主要服务注册相关测试（`rm.rs:38-42`）。

## 主要符号

- `InstanceResourceManager: LazyLock<ResourceManager>`：进程级单例；首次解引用时调用包级 `NewResourceManger`（`rm.rs:34-36`）。
- `RandomName() -> String`：返回 `uuid::Uuid::new_v4().to_string()`（`rm.rs:38-42`）。
- `ResourceManager`：可克隆公开句柄，只含 `Arc<ResourceManagerInner>`；克隆不会复制状态或新建调度线程（`rm.rs:44-51`）。
- `ResourceManagerInner`：共享状态容器。`poolMap` 是可整体替换的 `Arc<ShardPoolMap>`；`scheduler` 是构造后固定的 trait-object 列表；`cpuObserver`、`exitCh` 分别以 `Mutex` 串行化启停；`wg` 跟踪本文件创建的调度线程（`rm.rs:53-65`）。
- `ExitState`：`sender` 保存当前调度循环的退出发送端，`stopped` 记录 Go `exitCh` 一旦关闭便不可重开的语义（`rm.rs:67-71`）。
- 包级 `NewResourceManger() -> ResourceManager`：保留 Go 源码中的拼写错误并转发到关联构造函数（`rm.rs:73-77`）。
- `ResourceManager::NewResourceManger() -> Self`：以 `NewCPUScheduler()` 建立默认调度器列表（`rm.rs:79-83`）。
- `ResourceManager::new_with_schedulers(...)`：隐藏但公开的依赖注入入口，独立测试用它安装固定命令调度器（`rm.rs:85-100`）。
- `Start` / `Stop`：资源管理器的单向运行生命周期 API（`rm.rs:102-161`）。
- `Register` / 私有 `registerPool`：把 `Arc<dyn GoroutinePool>` 与组件类型包装为 `PoolContainer`，再交给分片映射 `Add`（`rm.rs:163-187`）。
- `Unregister`：调用映射 `Del`，缺失名称不报错（`rm.rs:189-200`）。
- `Reset`：在外层写锁内用新的空 `ShardPoolMap` 替换旧 `Arc`（`rm.rs:202-210`）。

## 执行流程

默认构造从包级 `NewResourceManger` 进入关联函数，再调用 `new_with_schedulers`。内部创建八分片空映射、默认 CPU 调度器、尚未启动的 CPU 观察器、`sender = None / stopped = false` 的退出状态，以及计数为零的 `WaitGroupWrapper`。构造本身不创建线程；全局单例也只有在首次使用时才初始化。

`Start` 每次先创建一对新的 MPSC 通道，然后持有 `exitCh` 锁检查状态。若已经 `Stop`，立即返回且不会重启 CPU 观察器；否则保存新的发送端。随后持有 `cpuObserver` 锁调用 `Observer::Start`，该下游方法自行创建约每 100 ms 采样一次 CPU 的线程。最后克隆管理器句柄，通过 `wg.Run` 创建调度线程：`recv_timeout(100ms)` 超时就执行 [`ResourceManager::schedule`](schedule.rs)，收到退出消息或发现通道断开则退出。

`Stop` 先调用 `Observer::Stop`，下游会关闭其采样通道并 join CPU 线程。然后在 `exitCh` 锁内断言尚未停止，将 `stopped` 永久设为 `true` 并取走发送端；如果此前执行过 `Start`，发送一个退出消息唤醒调度线程。无论是否存在发送端，最后都调用 `wg.Wait` 等待已登记的调度线程归零。

池注册从 `Register` 进入：调用者提供 trait-object 池、名称与组件，方法构造 `PoolContainer` 并由 `registerPool` 交给当前映射的 `Add`。`Unregister` 用同一映射按名称删除。`Reset` 不逐项清空旧映射，而是替换外层 `Arc`；已在其他线程克隆的旧映射仍可存活到其引用释放，后续注册与查询使用新映射。

## 数据与状态

- `ResourceManager` 的复制语义是共享：所有 clone 访问同一个 `ResourceManagerInner`，因此注册表、停止标志、观察器和等待器都是实例级状态。
- `poolMap: RwLock<Arc<ShardPoolMap>>` 的外层锁只保护“当前映射指针”的读取与替换；映射内部再由八个分片各自的 `RwLock<HashMap<...>>` 保护增删与遍历。`Register` / `Unregister` 持外层读锁，`Reset` 持外层写锁。
- `scheduler: Vec<Box<dyn Scheduler + Send + Sync>>` 只在构造时写入，本文件没有运行期增删接口。`schedulePool` 按向量顺序询问调度器，故注入顺序具有行为意义。
- `cpuObserver` 需要可变启停，故放在 `Arc<Mutex<Observer>>` 中；观察结果本身由 CPU crate 的全局原子状态发布。
- `ExitState::stopped` 是不可逆状态。`sender` 只代表当前是否存在可通知的调度循环，不能单独表达永久关闭，因此两者不能合并。
- `WaitGroupWrapper` 只跟踪 `Start` 中的调度线程；CPU 观察线程由 `Observer::Stop` 自己 join，不计入这里的 `wg`。

## 依赖与调用关系

上游调用关系：

- [`lib.rs`](lib.rs) 将本文件的公开 API 再导出为 crate 根接口。
- [`cmd/tidb-server/main.rs`](../../cmd/tidb-server/main.rs) 的正常启动路径在 `executor::Start()` 后调用 `InstanceResourceManager.Start()`；信号处理与 `exitAfterKeyspaceActivate` 路径调用 `Stop()`。
- [`pool/spool/spool.rs`](pool/spool/spool.rs) 的 `Pool::new` 调用 `InstanceResourceManager.Register`，`release_and_wait` 调用 `Unregister`，把实际 worker 池接入/移出调度集合。
- [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 直接覆盖构造、注册、注销、重置与启停；[`schedule_test.rs`](schedule_test.rs) 经 `NewResourceManger` 验证调容上限。

下游依赖关系：

- [`util/shard_pool_map.rs`](util/shard_pool_map.rs) 提供 `ShardPoolMap::{Add,Del,Iter}` 和重名错误 `PoolMapError`；名称首字节决定分片。
- [`schedule.rs`](schedule.rs) 消费 `poolMap` 与 `scheduler`：遍历注册池，跳过 `DistTask`，调用调度器并执行 `Exec`。
- `astersql-resourcemanager-scheduler` 提供 `Scheduler`、`NewCPUScheduler`、`Component`、`GoroutinePool` 和 `PoolContainer`。
- [`pkg/util/cpu/cpu.rs`](../util/cpu/cpu.rs) 的 `Observer::{Start,Stop}` 管理 CPU 采样线程。
- [`pkg/util/wait_group_wrapper.rs`](../util/wait_group_wrapper.rs) 的 `WaitGroupWrapper::{Run,Wait}` 为调度线程提供启动计数和退出等待。
- 标准库 `mpsc` 承担调度退出通知；`Arc`、`Mutex`、`RwLock` 和 `LazyLock` 分别承担共享所有权、可变状态互斥、映射指针读写与单例惰性初始化。

## 错误处理与边界

- `Register` 将重名冲突原样返回为 `PoolMapError`；现有错误文本为 `pool is already exist`。空名称会在 `ShardPoolMap::hash` 访问首字节时 panic，与 Go 实现的索引失败语义一致。
- `Unregister` 对不存在的非空名称是幂等空操作，但空名称同样会在散列时 panic。
- `Start` 和 `Stop` 没有返回 `Result`。同步原语中毒通过带上下文的 `expect` 转为 panic；这不是可恢复业务错误。
- `Stop` 只能成功调用一次；第二次会因 `assert!(!stopped, "close of closed resource manager")` panic，对齐 Go 重复关闭 channel 的行为。
- 构造后未 `Start` 就调用 `Stop` 是受测试支持的：没有发送端可通知，`wg` 也为空，但停止状态会永久置位；后续 `Start` 静默返回。
- `Start` 在停止前并没有显式防止重复调用。第二次调用会覆盖 `ExitState.sender` 并启动另一组观察/调度任务，而旧调度接收端因原发送端被丢弃会观察到断开后退出；CPU `Observer::Start` 也会覆盖其内部 worker 句柄。现有独立测试未覆盖重复 `Start`，调用方应遵守服务主链中的单次启动约定。
- 退出通知 `send` 的失败被忽略，因为接收端已经退出时目标也已达成；`Observer::Stop` 和 `WaitGroupWrapper::Wait` 负责实际资源收敛。
- `Reset` 面向测试，不协调正在进行的一轮 `schedule`。该轮若已经克隆旧 `Arc<ShardPoolMap>`，仍可能遍历旧池；后续轮次才使用新映射。

## 并发与资源生命周期

全局 `LazyLock` 保证默认管理器只构造一次。`ResourceManager` 被调度闭包克隆后，通过 `Arc` 保证后台线程运行期间内部状态有效；`wg.Run` 在线程创建前先增加计数，并用 RAII 在所有正常返回和 unwind 路径减少计数，所以 `Stop` 可等待调度线程完成。

启动后的资源链有两条：CPU 观察器拥有自己的退出发送端与 `JoinHandle`，由 `Observer::Stop` 回收；资源管理器的调度循环只由 `ExitState.sender` 唤醒，并由 `WaitGroupWrapper` 等待。`Stop` 先回收观察线程，再通知和等待调度线程，结束后不允许重启。

注册表使用两级锁避免把所有池操作串行化。周期调度在 [`schedule.rs`](schedule.rs) 开始时只短暂读取外层 `poolMap` 并克隆 `Arc`，随后释放外层锁再遍历分片；因此 `Reset` 可以替换当前映射而无需等待整个调度周期，但这也形成“当前轮可能继续处理旧快照”的明确语义。每个分片的 `Iter` 在读锁持有期间调用回调，调度期间同分片的 `Add` / `Del` 会等待。

池的正常生命周期是“构造池并注册—周期调度—池释放并注销—进程停止资源管理器”。全局单例持有注册池的 `Arc<dyn GoroutinePool>`，仅丢弃池的外部句柄不会自动注销；具体池实现必须显式执行其释放路径。

## 与 Go 版本的对应关系

直接对照文件是 [`rm.go`](rm.go)，调度延伸实现是 [`schedule.go`](schedule.go)。Rust 保留了 `InstanceResourceManager`、拼写为 `Manger` 的构造函数、`RandomName`、`Start`、`Stop`、`Register`、`Unregister` 和 `Reset` 等 Go 风格名字。默认调度器、100 ms 调度周期、重复注册报错、缺失注销忽略、停止后不可重开以及测试用重置意图均与 Go 版本一致。

主要实现差异来自所有权与线程模型。Go 返回 `*ResourceManager`，Rust 返回含 `Arc` 的可克隆值；Go 直接持有池映射指针，Rust 用 `RwLock<Arc<_>>` 支持并发快照和安全替换；Go 关闭 `exitCh` 广播退出，Rust 以 `stopped` 加一次发送来模拟永久关闭状态；Go 将 `cpuObserver.Start` 本身交给 `WaitGroupWrapper.Run`，Rust 的 `Observer::Start` 同步创建并保存自己的采样线程，因此外层只用 `wg` 跟踪调度线程。

Go 的 `Start` 用 `time.Ticker`，Rust 用 `recv_timeout(100ms)` 同时承担时钟与退出选择。Rust 在超时后调用 `schedule`，在消息或发送端断开时返回；行为目的相同，但不是严格的固定相位 ticker。Rust 额外用显式锁中毒消息暴露同步失败，并用 `wrapping_*` 在 [`schedule.rs`](schedule.rs) 对齐 Go `int32` 边界算术。

独立 Rust 测试 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 覆盖了 Go 测试之外的重要迁移契约：UUID、重复注册、注销/重置后重注册、调度器顺序与降频守卫、整数回绕、`DistTask` 跳过、正常启停以及先停后启不恢复。Go/Rust 的 [`schedule_test.go`](schedule_test.go) 与 [`schedule_test.rs`](schedule_test.rs) 都覆盖超频不能超过原始并发加 `MaxOverclockCount`。

## 扩展指南

- 新增资源管理器状态时，应优先放入 `ResourceManagerInner` 并明确其共享、锁和停止顺序；不要把可变状态放到可克隆外壳中造成副本分叉。
- 新增或更换调度器，应修改 `ResourceManager::NewResourceManger` 的默认列表；若只需测试策略组合，继续使用 `new_with_schedulers`。同时在独立的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 增加顺序、短路和边界测试。
- 修改启停协议时，要联合检查 `Start`、`Stop`、`ExitState`、CPU `Observer` 与服务入口的所有退出路径，保持“停止观察器—通知调度线程—等待结束”的资源回收保证。若计划支持重启或重复 `Start`，必须先定义旧 worker、旧通道和观察器句柄如何收敛。
- 修改注册语义时，要同步检查 [`util/shard_pool_map.rs`](util/shard_pool_map.rs)、[`pool/spool/spool.rs`](pool/spool/spool.rs) 的构造/释放路径，以及空名、重名和并发 `Reset` 的兼容行为。
- 若增加对新池类型的接线，该类型必须实现 `GoroutinePool`，在成功构造后注册，并在全部 worker 回收后注销；组件分类会影响 [`schedule.rs`](schedule.rs) 是否跳过该池。
- 测试代码必须继续放在独立文件中。生命周期和注册契约扩展 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，调容行为扩展 [`schedule_test.rs`](schedule_test.rs)，并同步对照 Go 的 [`rm.go`](rm.go) / [`schedule_test.go`](schedule_test.go)。
- 兼容风险集中在公开 Go 风格命名、重复 `Stop` 的 panic、停止后不可重启和池映射快照语义；性能风险集中在 100 ms 轮询、分片遍历期间持读锁，以及调度回调执行时间。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/resourcemanager/rm.rs` 确认目标文件已索引，`node --file pkg/resourcemanager/rm.rs --offset 1 --limit 260` 读取了全部 211 行和 14 个符号。
- RustCodeGraph 查询：`explore "pkg/resourcemanager/rm.rs ResourceManager ..."`、目标文件 `node`、以及对 [`schedule.rs`](schedule.rs)、[`pool/spool/spool.rs`](pool/spool/spool.rs)、[`util/shard_pool_map.rs`](util/shard_pool_map.rs)、[`cmd/tidb-server/main.rs`](../../cmd/tidb-server/main.rs)、CPU 观察器和 `WaitGroupWrapper` 的文件节点查询。直接调用链由这些查询确认：服务入口 `Start/Stop`，池构造 `Register`，池释放 `Unregister`，调度线程 `schedule`。
- crate 与模块证据：[`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs)。
- Go 对照证据：[`rm.go`](rm.go) 与 [`schedule.go`](schedule.go)。
- 独立测试证据：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、[`schedule_test.rs`](schedule_test.rs) 和 [`schedule_test.go`](schedule_test.go)。其中已覆盖 UUID、注册表操作、调度守卫、调容边界、`DistTask` 跳过及启停；重复 `Start` 与并发 `Reset` 没有专项测试，本文只按源码陈述其当前行为。
- 本任务是纯文档分析，按计划未运行 Cargo。交付验证使用任务指定的结构命令确认本文存在且恰含 11 个固定二级章节，并人工检查重要结论均可回溯到上述符号、调用边、Cargo、Go 对照或独立测试。
