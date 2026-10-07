# `pkg/ddl/systable/min_job_id.rs`

## 文件定位

本文件属于 Cargo crate `astersql-ddl-systable`；crate 入口 `pkg/ddl/systable/lib.rs` 将 `min_job_id` 声明为公开模块并重新导出其符号。它位于 DDL 系统表访问层，依赖同 crate 的 `Manager` 与 `Context`，维护 `mysql.tidb_ddl_job` 当前最小作业 ID 的进程内缓存。

这个水位不是 DDL job 分配器，也不执行 job、schema 状态迁移或 reorg/backfill。它以 `Manager::get_min_job_id` 为唯一数据源，为两类下游查询提供更接近现存记录的扫描起点：InfoSchema MDL 刷新，以及提交 DDL 前的 FLASHBACK CLUSTER 冲突检查。Go 文件注释把其直接动机归为避免从已删除记录形成的低位区间开始扫描（`pkg/ddl/systable/min_job_id.go:30-33`）。

## 核心职责

- `new_min_job_id_refresher` 组装一个持有 `Arc<dyn Manager>` 的刷新器，并把初始缓存设为 `0`。
- `MinJobIdRefresher::refresh` 用当前缓存作为 `previous_min_job_id` 查询系统表，然后通过 `AtomicI64::fetch_max` 只向前推进水位。
- `MinJobIdRefresher::start` 立即刷新一次，之后以 `REFRESH_INTERVAL`（10 秒）为周期等待；收到 `Cancellation` 后退出。
- `current_min_job_id` 为并发消费者提供无锁快照；`is_running` 仅报告循环状态。
- `Cancellation` 用共享的 `Mutex<bool> + Condvar` 提供可克隆、可唤醒的停止信号，使调用方无需等待完整的 10 秒间隔。

核心不变量是缓存不回退。即使系统表已无 job、`Manager` 返回 `0`，或者并发刷新观察到不同结果，`fetch_max` 也只保留当前值与新值中的较大者（`MinJobIdRefresher::refresh`）。

## 主要符号

- `pub const REFRESH_INTERVAL: Duration`：固定为 10 秒的周期等待时间。
- `pub struct Cancellation`：内部状态为 `Arc<(Mutex<bool>, Condvar)>`，克隆实例共享同一取消位。
  - `cancel(&self)`：将状态置为 `true` 并 `notify_all`。
  - `is_cancelled(&self) -> bool`：读取当前取消位。
  - `wait_timeout(&self, Duration) -> bool`：内部等待原语；已取消或等待中被取消时返回 `true`。
- `pub struct MinJobIdRefresher`：保存 `Arc<dyn Manager>`、`AtomicI64 current_min_job_id` 与 `AtomicBool running`。字段私有，外部只能通过方法观察或驱动状态。
- `pub fn new_min_job_id_refresher(Arc<dyn Manager>) -> MinJobIdRefresher`：公开构造入口，初始水位为 `0`，初始运行态为 `false`。
- `current_min_job_id(&self) -> i64`：以 `Acquire` 读取缓存。
- `start(&self, &Context, &Cancellation)`：同步、阻塞式刷新循环；函数自身不创建线程。
- `refresh(&self, &Context)`：执行单次查询与单调更新，公开是为了接线和独立测试可以主动预热。
- `is_running(&self) -> bool`：以 `Acquire` 读取运行标志。

文件没有 trait、枚举、条件编译项或异步任务定义。

## 执行流程

1. 运行时通过 `new_manager(session_pool)` 构造系统表 `Manager`，再调用 `new_min_job_id_refresher`；普通 Domain 的组装见 `pkg/session/runtime/session_factory.rs:550-579`，目标 keyspace 组装见同文件 `:403-417`，另一条 cross-keyspace 生产组装见 `pkg/session/runtime/crossks_runtime.rs:342-350`。
2. 调用方把 `start` 放入命名工作线程。`start` 先将 `running` 发布为 `true`，不先检查取消位。
3. 每轮先调用 `refresh`。它以 `Acquire` 读取缓存，把该值传给 `Manager::get_min_job_id(context, current)`。
4. `SystemTableManager::get_min_job_id` 实际执行 `select min(job_id) from mysql.tidb_ddl_job where job_id >= previous_min_job_id`，并保证借出的系统会话在查询成功或失败后均归还（`pkg/ddl/systable/manager.rs:125-137,169-180`）。
5. 查询成功时，`fetch_max(next, AcqRel)` 原子保留较大值；查询失败时本轮直接返回，缓存不变。
6. 循环通过 `Cancellation::wait_timeout(REFRESH_INTERVAL)` 等待。超时返回 `false`，开始下一轮；取消返回 `true`，跳出循环并将 `running` 发布为 `false`。

由于刷新位于取消检查之前，即使传入的 token 已取消，`start` 仍会执行恰好一次刷新；`test_start_refreshes_once_when_already_cancelled` 固定了这一语义。

## 数据与状态

`current_min_job_id` 是进程内、非持久化的 i64 水位，初值为 `0`。持久事实仍在 `mysql.tidb_ddl_job`；进程重启会从 `0` 重新建立水位。`Manager` 查询使用 `job_id >= previous_min_job_id`，因此缓存越接近现存最小 ID，需要跳过的已删除低位记录越少。

水位允许滞后但不允许回退：错误时保持旧值，表清空返回 `0` 时也保持旧值。新 job ID 按全局 ID 单调增长这一外部约束，使旧水位仍可作为后续查询的安全下界；本文件本身不生成或验证 ID。

`running` 是可观察状态而不是互斥门闩：`start` 不用 `compare_exchange` 拒绝第二个调用者，因此调用方必须保证每个刷新器只启动一个生命周期循环。`Cancellation` 一旦置位便不可复位，克隆只共享同一状态，不建立新的生命周期。

## 依赖与调用关系

下游依赖只有标准库同步原语，以及 crate 内的 `Context`、`Manager`。关键调用边为：

- `MinJobIdRefresher::refresh` → `Manager::get_min_job_id` → `SystemTableManager::get_min_job_id` → `mysql.tidb_ddl_job` 聚合查询。
- `pkg/infoschema/issyncer/syncer.rs:180-209` 通过 `SetMinJobIDRefresher` 保存共享刷新器；`RefreshMDLFromSQL` 读取 `current_min_job_id()`，再以该值调用 `ReadMDLRows(min, version)`。
- `pkg/ddl/jobsubmit/submit.rs:30-46` 从 `MinJobIdProvider` 读取该水位，并传给 `has_flashback_cluster_job(min_job_id)`，在插入新 DDL job 前检查 FLASHBACK CLUSTER 冲突。
- `pkg/session/runtime/crossks_session_pool.rs:470-485` 用 `CrossKSMinJobId` 把刷新器适配为 `jobsubmit::MinJobIdProvider`。
- 普通 schema runtime 在 `pkg/session/runtime/normal_ddl_service.rs:713-766` 启动 `normal-min-job-id` 线程；目标 keyspace 在 `pkg/session/runtime/session_factory.rs:465-474` 启动 `keyspace-min-job-id` 线程；`CrossKSMinIdLoop::start` 在 `pkg/session/runtime/crossks_runtime.rs:139-173` 启动并管理 `crossks-min-ddl-job-id` 线程。

RustCodeGraph 的文件索引确认 `min_job_id.rs` 含 16 个符号；精确 `query` 将构造函数标识为 `min_job_id.rs::new_min_job_id_refresher`。`explore` 找到构造、`start`、`refresh`、`get_min_job_id` 之间的调用流；由于同名 `start/refresh` 很多，生产调用点又用限定名称搜索复核。

## 错误处理与边界

`refresh` 把 `Manager::get_min_job_id` 的任意错误视为可恢复的单轮失败：不修改缓存、不停止循环，也不向调用方返回错误。与 Go 版本不同，Rust 当前不记录该错误，因此持续失败只能从系统表访问层的外部可观测性诊断，不能从本 API 获得失败计数或原因。

`Cancellation` 对中毒的 mutex 使用 `expect("cancellation mutex poisoned")`，因此持锁线程 panic 后的后续取消、检查或等待也会 panic。`Condvar::wait_timeout` 被虚假唤醒时可能让循环提前再刷新一次，但不会破坏单调水位。

`start` 是阻塞调用，必须由外层线程管理；直接在请求线程调用会一直运行到取消。已经取消的 token 仍允许首轮刷新。查询返回负数时 `fetch_max` 相对初值/当前非负水位会忽略它，但本文件没有单独校验数据库值的合法性。

## 并发与资源生命周期

读水位使用 `Acquire`，更新使用 `AcqRel`，因此多个读取者和偶发的并发 `refresh` 不产生数据竞争，且最大值合并不会因后写覆盖而回退。`running` 的 `Release/Acquire` 只发布循环是否位于 `start` 内部，不负责所有权或防重复启动。

本文件不拥有数据库 session：`Manager` 的具体实现负责借还。它也不拥有线程句柄：普通 runtime、keyspace runtime 与 `CrossKSMinIdLoop` 分别创建线程、保存 `JoinHandle`，关闭时先调用共享 `Cancellation::cancel` 唤醒 condvar，再 `join` 工作线程（例如 `pkg/session/runtime/session_factory.rs:260-276`、`pkg/session/runtime/normal_ddl_service.rs:778-793` 和 `pkg/session/runtime/crossks_runtime.rs:161-173`）。这一顺序避免关闭过程最多额外等待一个刷新周期，并保证依赖的 pool/domain 在刷新线程退出后才释放。

同一个 `Arc<MinJobIdRefresher>` 可安全分享给 InfoSchema、job submitter 和刷新线程，因为 `Manager: Send + Sync`，可变状态均通过原子或锁保护。安全扩展时仍应保持“每实例单刷新循环”的外层约束。

## 与 Go 版本的对应关系

Rust 对照 `pkg/ddl/systable/min_job_id.go` 保留了关键算法：初值 0、每轮先刷新后等待、查询失败后下轮重试，以及用 `max(current, next)` 防止表清空时从 100 回退到 0。`pkg/ddl/systable/min_job_id_test.rs::test_refresh_min_job_id` 与 Go 的 `TestRefreshMinJobID` 使用相同的 0→1→100→0 序列，并额外核对每次传入 Manager 的 previous 值为 0、1、100。

当前可见差异如下：

- Go 以 `context.Context` 同时传递查询上下文和取消；Rust 将轻量 `Context` 与 `Cancellation` 分离。
- Go 使用 `atomic.Int64.Load` 后 `Store(max(...))`；Rust 使用 `fetch_max`，在并发刷新时仍保证原子最大值合并。
- Go 的 `refresh` 为私有方法，Rust 为公开方法；Rust 生产接线会在部分路径主动预热一次。
- Go 查询失败会写 DDL info 日志；Rust 当前静默忽略。
- Go 的刷新间隔是包变量，测试可临时缩短；Rust 是公开常量，独立测试通过直接调用 `refresh` 和预取消避免等待。
- Rust 增加 `running` 状态和 `is_running`，并增加已取消仍先刷新一次的回归测试；Go 结构体没有对应字段。

这些差异没有改变已测试的单调水位语义，但错误可观测性和并发启动约束是后续演进时需要明确处理的兼容点。

## 扩展指南

- 修改刷新算法时优先改 `MinJobIdRefresher::refresh`，并同步扩展独立文件 `pkg/ddl/systable/min_job_id_test.rs`；不要把测试嵌入生产 `.rs`。
- 改动 SQL 下界或空表语义时，应同时审查 `Manager::get_min_job_id`（`pkg/ddl/systable/manager.rs`）及其测试，并保持与 `pkg/ddl/systable/min_job_id.go`、`min_job_id_test.go` 的意图一致。
- 若要传播/记录查询错误，需要决定是维持后台循环“失败不退出”，还是把错误暴露给生命周期拥有者；同时评估日志频率，避免系统表故障期间每 10 秒持续刷屏。
- 若允许动态间隔，应提供实例级配置或可注入等待机制，避免恢复 Go 的全局可变间隔造成并行测试相互影响。
- 若要把 `running` 用作重复启动保护，需以 `compare_exchange` 明确定义第二次 `start` 是报错、立即返回还是共享既有循环，并增加并发回归测试；当前布尔值不能提供该保证。
- 改动取消协议时，必须保持普通 Domain、目标 keyspace、cross-keyspace 三条生命周期路径都能先取消、再 join、最后释放系统 session pool。
- 性能风险集中在刷新频率和查询下界：过短间隔增加内部 SQL 压力，水位不前进则扩大扫描范围；正确性风险集中在错误地允许水位回退或越过仍需检查的 job。

## 验证依据

- 源文件与主要符号：RustCodeGraph `node --file pkg/ddl/systable/min_job_id.rs --offset 1 --limit 260`；`query MinJobIdRefresher`；`query new_min_job_id_refresher --kind function`。
- 调用流：RustCodeGraph `explore "pkg/ddl/systable/min_job_id.rs MinJobIDRefresher refresh get_min_job_id get_global_min_job_id"`；并以限定符号搜索复核三个生产 runtime、InfoSchema 与 jobsubmit 调用点。精确 `callers` 查询在本地索引上未在时限内返回，因此未把它当作唯一证据。
- crate 边界：`pkg/ddl/systable/Cargo.toml` 声明 crate 名、`lib.rs` 入口、Go package 映射，唯一显式依赖为路径 crate `astersql-meta-model`；`pkg/ddl/systable/lib.rs` 负责模块声明、重导出和独立测试挂载。
- 直接实现证据：`pkg/ddl/systable/manager.rs`、`pkg/infoschema/issyncer/syncer.rs`、`pkg/ddl/jobsubmit/submit.rs`、`pkg/ddl/jobsubmit/types.rs`、`pkg/session/runtime/session_factory.rs`、`pkg/session/runtime/normal_ddl_service.rs`、`pkg/session/runtime/crossks_runtime.rs`、`pkg/session/runtime/crossks_session_pool.rs`、`pkg/session/runtime/normal_ddl_submit.rs`。
- Go 对照与测试：`pkg/ddl/systable/min_job_id.go`、`pkg/ddl/systable/min_job_id_test.go`；Rust 独立测试为 `pkg/ddl/systable/min_job_id_test.rs`。
- 人工复核结论：文档区分了水位缓存、系统表查询、MDL 消费与 flashback guard，未把该文件描述成 job 执行器或 schema 状态机；测试建议保持在独立测试文件。
