# `pkg/lightning/worker/worker.rs`

## 文件定位

本文件实现 Lightning 的固定容量 worker 令牌池，属于独立 crate `astersql-lightning-worker`。crate 入口 `pkg/lightning/worker/lib.rs` 公开 `worker` 模块并再导出本文件全部公开符号；根 crate 又在 `pkg/lib.rs` 的 `lightning::worker` 门面中再导出该 crate。因此外部 Rust 代码可经由该子 crate 或根门面访问 `Pool`、`Worker` 和 `NewPool`。

当前仓库的接线状态需要与设计目标区分：全仓 Rust 搜索只发现根门面和本 crate 的独立测试使用这里的 API，没有发现生产 Rust 调用者。`lightning/pkg/importer/stubs.rs::worker` 另有一个仅保存 `size`/`name`、立即执行闭包的迁移桩；`lightning/pkg/importer/*.rs` 当前引用的是那个桩，而不是本文件的令牌池。因此本文件已经提供可测试的真实有界池行为，但尚不能据此断言 Rust Lightning importer 主链已经使用它。

## 核心职责

- `NewPool` 创建容量为 `limit` 的有界通道，并预置恰好 `limit` 个 `Arc<Worker>`，把每个令牌的 `ID` 固定为 `1..=limit`。
- `Pool::Apply` 从接收端取出一个令牌；池空时同步阻塞，令牌被归还后才继续。
- `Pool::Recycle` 将令牌放回发送端；它拒绝 `None`，但不校验令牌是否来自本池。
- `Pool::HasWorker` 提供当前是否存在空闲令牌的瞬时观察，不承担预留或同步保证。
- 若 `MetricContext` 携带 `metric::Metrics`，构造、申请和归还会更新按池名区分的空闲 worker 仪表，申请还会记录包含等待时间的直方图样本。

该抽象限制的是“同时借出的令牌数”，并不创建线程、执行任务或自动归还令牌；调用方必须显式配对 `Apply` 与 `Recycle`。

## 主要符号

- `pub struct Pool`：池状态。`limit: usize` 保存构造容量；`workers: Receiver<Arc<Worker>>` 是申请端；`worker_sender: Sender<Arc<Worker>>` 是归还端；`name: String` 是指标标签；`metrics: Option<Arc<metric::Metrics>>` 保存可选观测句柄。所有字段均为私有。
- `pub struct Worker { pub ID: i64 }`：可复用的并发许可。`ID` 是公开字段，初始化后本文件不再修改。令牌用 `Arc` 传递，因此可用 `Arc::ptr_eq` 验证是否复用了同一个对象。
- `pub fn NewPool(ctx: &metric::MetricContext, limit: usize, name: String) -> Pool`：唯一公开构造函数。RustCodeGraph 将它解析到 `worker.rs:49`，并记录其直接调用 `pkg/lightning/metric/metric.rs::from_context`。
- `pub fn Pool::Apply(&self) -> Arc<Worker>`：阻塞式获取接口；RustCodeGraph 定位到 `worker.rs:78`。
- `pub fn Pool::Recycle(&self, worker: Option<Arc<Worker>>)`：阻塞式归还接口；RustCodeGraph 定位到 `worker.rs:99`。
- `pub fn Pool::HasWorker(&self) -> bool`：基于接收端当前长度的只读快照；RustCodeGraph 定位到 `worker.rs:114`。
- `pub fn Pool::limit(&self) -> usize`：公开容量访问器，当前没有仓库内调用者，主要保留 Go 结构中的容量信息供后续接线。

文件没有模块级常量、trait、泛型、异步函数或条件编译项；测试模块由相邻 `lib.rs` 通过 `#[cfg(test)]` 引入，而不是内嵌在本文件中。

## 执行流程

1. 调用方以 `MetricContext`、容量和名称调用 `NewPool`。
2. `crossbeam_channel::bounded(limit)` 建立同容量的发送端与接收端。构造循环依次发送 `Worker { ID: 1 }` 到 `Worker { ID: limit }`；由于通道容量正好等于发送次数，正容量构造不会等待消费者，初始读取顺序也是 ID 递增顺序。
3. `metric::from_context` 尝试取得指标对象。存在指标时，`idle_workers_gauge{name}` 立即设置为 `limit`；无指标时仍正常构造池。
4. `Apply` 先记录 `Instant`，再对接收端执行阻塞 `recv`。成功取得令牌后，先把空闲仪表设为接收端剩余长度，再把从进入 `Apply` 到取得令牌的全部时间写入 `apply_worker_seconds_histogram{name}`，最后返回 `Arc<Worker>`。
5. 调用方完成受限工作后，以 `Some(worker)` 调用 `Recycle`。方法先拆出令牌，再阻塞发送回有界通道；发送成功后才用当前接收端长度刷新空闲仪表。
6. 后续 `Apply` 按 crossbeam 有界通道的 FIFO 顺序取得已归还令牌。`worker_test.rs::test_apply_recycle` 和 `migration_aster_unit_test.rs::apply_recycle_matches_go_fifo_and_nil_guard` 以对象指针相等验证了复用行为。

## 数据与状态

池的权威空闲状态是通道中的 `Arc<Worker>` 集合。正常配对使用时，不变量是“通道内令牌数 + 已借出令牌数 = `limit`”，且每个初始令牌的 ID 唯一并稳定。`limit` 只记录初始容量，不参与后续检查；`HasWorker` 与指标读取的都是通道当下长度。

Rust 的所有权没有独占令牌本身：`Arc<Worker>` 可以被克隆，`Recycle` 也接受任意 `Arc<Worker>`。因此上述数量与唯一性不变量依赖调用契约，而不是类型系统强制保证。归还外部构造的 `Worker`、归还同一 `Arc` 的多个克隆或遗漏归还，都可能改变许可语义。当前测试覆盖正常借还、FIFO、阻塞唤醒和指标，不覆盖这些误用。

`metrics` 持有指标对象的 `Arc` 克隆，因此池存活期间指标对象不会被释放；标签字符串 `name` 由池拥有。指标是观测值而非同步依据：在并发申请/归还之间，读取长度与写入 gauge 之间可发生其他操作，故 gauge 可能短暂反映一次竞态快照。

## 依赖与调用关系

- crate 边界由 `pkg/lightning/worker/Cargo.toml` 定义：运行时仅直接依赖 `crossbeam-channel = "0.5"` 和路径依赖 `astersql-lightning-metric`；无 feature 声明。
- 本文件通过 `crate::metric` 使用 `MetricContext`、`Metrics` 与 `from_context`；`pkg/lightning/worker/lib.rs` 将 `lightning_metric` 再导出为该名称。
- 下游同步原语是 `crossbeam_channel::bounded`、`Sender::send`、`Receiver::recv`、`Receiver::len` 和 `Receiver::is_empty`；共享令牌与指标使用 `std::sync::Arc`，耗时使用 `std::time::Instant`。
- RustCodeGraph 的精确 `node` 证实 `NewPool` 到 `metric.rs::from_context` 的直接调用边；其对通用方法名的 `callers/callees` 查询存在名称消歧限制，因此仓库调用范围另由精确 `rg` 复核。
- 上游暴露链是 `worker.rs` → `pkg/lightning/worker/lib.rs` → 根 `pkg/lib.rs::lightning::worker`。当前直接行为调用只位于 `worker_test.rs` 与 `migration_aster_unit_test.rs`；没有检出生产 Rust 调用者。
- `lightning/pkg/importer/stubs.rs::worker::Pool` 与本类型同名但不是同一实现，不能视为调用边或兼容替代。

## 错误处理与边界

本 API 不返回 `Result`。它把违反内部通道假设或调用契约视为 panic/阻塞：

- `Recycle(None)` 在发送前以精确消息 `invalid restore worker` panic；Rust 和 Go 测试都固定了这条兼容行为。
- `recv` 或 `send` 的断开错误通过 `expect` 转为 panic，消息分别是 `worker pool channel unexpectedly closed` 与 `worker pool receiver unexpectedly closed`。在安全公开 API 的正常生命周期中，发送端和接收端共同由同一 `Pool` 持有，因此这些分支通常不可达。
- 池为空时 `Apply` 无超时、取消或上下文检查，会一直等待归还。
- 池已满时再次 `Recycle` 会一直等待空位；重复归还尤其可能造成死锁。方法不会检测重复令牌或跨池令牌。
- `limit == 0` 可创建零容量 rendezvous 通道且不预置令牌。单线程调用 `Apply` 或 `Recycle` 都会等待匹配方；当前既没有显式拒绝，也没有测试定义它的产品语义。
- `HasWorker == true` 之后，其他线程可先取走令牌；调用方不能据此假设紧随其后的 `Apply` 不阻塞。
- `Worker.ID` 的 `usize` 到 `i64` 转换使用 `as`，极端超出 `i64::MAX` 的容量会截断；实际在此之前通常已受内存/通道容量限制，但代码没有显式范围校验。

## 并发与资源生命周期

`Pool` 的方法都接收 `&self`，依赖 crossbeam 通道的线程安全能力允许多个线程并发申请和归还；测试 `apply_waits_until_a_worker_is_recycled` 将 `Pool` 放入 `Arc`，证明一个线程在空池上等待时，另一线程归还令牌会解除阻塞。这里没有 Tokio runtime、后台任务、锁或显式线程管理。

令牌生命周期从 `NewPool` 预置开始：空闲时由通道中的 `Arc` 持有，借出后由调用方持有，归还后重新由通道持有。池不提供 RAII guard，调用方 panic、提前返回或忘记 `Recycle` 都会永久减少可用许可，直到相关 `Arc` 被显式归还；单纯 drop 借出的 `Arc` 不会自动回池。

`Pool` 被 drop 时，发送端、接收端、通道内令牌、名称和指标引用随之释放；没有显式关闭或 drain。由于 `Pool` 自身未实现 `Clone`，共享池通常需由上层包裹在 `Arc<Pool>` 中。借出的 `Arc<Worker>` 可以比池活得更久，但池销毁后已无公开接收端可供归还。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/worker/worker.go`，基础回归是 `worker_test.go::TestApplyRecycle`。两版共同保留：固定容量通道、从 1 开始的稳定 ID、池空时阻塞申请、显式归还、FIFO 复用、`nil`/`None` panic 文案、`HasWorker` 快照，以及同名的 idle gauge 与 apply-wait histogram 更新时机。

Rust 适配差异如下：Go 的 `context.Context` 换成专用 `metric::MetricContext`；`*Pool` 返回值换成按值 `Pool`；`*Worker` 换成 `Arc<Worker>`；一个 Go channel 被表达为 crossbeam 的独立 `Sender`/`Receiver`；Go 的可空指针参数换成 `Option<Arc<Worker>>`。这些差异没有改变正常借还语义。

Rust 额外提供私有字段的 `limit()` 访问器；Go 同包代码可直接访问 `pool.limit`，而 Rust 跨模块必须通过方法。`migration_aster_unit_test.rs` 还补充验证了 Go 基础测试没有直接断言的“空池申请确实等待”和指标采样行为。另一方面，Rust importer 当前仍接到 `lightning/pkg/importer/stubs.rs` 的简化池，故完整 Go Lightning 调用链的迁移接线仍未验证。

## 扩展指南

- 若把真实池接入 Rust importer，应把调用点从 `lightning/pkg/importer/stubs.rs::worker::Pool` 迁到本 crate/根门面，并逐个核对现有桩的 `Pool::New(size, name)` 与闭包式 `Apply` 调用协议；不能只替换类型名。生产修改应同步相邻独立 Rust 测试，不把测试写回 `worker.rs`。
- 若增加自动归还，优先新增持有 `Arc<Worker>` 与池引用的 RAII guard，并明确 guard 的 drop 在池满、池已销毁或 panic 展开时是否允许阻塞；这是兼容性和死锁风险最高的改动点。
- 若支持取消或超时，应修改 `Apply` 的签名和接收策略，并同步定义等待耗时指标是否记录超时/取消样本；Go 对应行为也需一并核对，避免单边简化。
- 若要强化不变量，可为令牌增加池身份、禁止任意构造/跨池归还，或记录在借状态；需要评估公开 `Worker { ID }` 数据形状以及现有 `Arc` 克隆用法的兼容性。
- 修改容量、FIFO、panic 文案、指标标签或观测时机时，至少同步 `pkg/lightning/worker/worker_test.rs`、`migration_aster_unit_test.rs` 和 Go 对照测试 `worker_test.go`。涉及生产接线时还需为 importer 增加独立集成/并发回归。
- 性能上，`Apply`/`Recycle` 每次启用指标都会进行带标签查找并读取通道长度；新增热路径观测前应确认 Prometheus label 访问与原子操作成本，且不要把 gauge 当作严格一致的许可计数器。

## 验证依据

- RustCodeGraph 索引状态：目标仓库已索引 11,467 个文件，`files --filter pkg/lightning/worker` 覆盖 `worker.rs`、crate 入口、两个 Rust 测试及 Go 对照文件；目标 `worker.rs` 识别出 8 个符号。
- RustCodeGraph 精确节点：`pkg/lightning/worker/worker.rs::NewPool`、`::Apply`、`::Recycle`、`::HasWorker` 分别定位到第 49、78、99、114 行；`NewPool` 的图内直接下游为 `pkg/lightning/metric/metric.rs::from_context`。通用 Go 风格方法名的图调用查询无法可靠消歧，故未据其推断生产调用者。
- 已阅读生产与装配文件：`pkg/lightning/worker/worker.rs`、`pkg/lightning/worker/lib.rs`、`pkg/lightning/worker/Cargo.toml`、根 `Cargo.toml` 的 `facade_lightning_worker` 依赖、`pkg/lib.rs::lightning::worker` 门面、`pkg/lightning/metric/metric.rs` 的指标和上下文定义，以及 `lightning/pkg/importer/stubs.rs::worker`。
- 已阅读对照与测试：`pkg/lightning/worker/worker.go`、`worker_test.go`、`worker_test.rs`、`migration_aster_unit_test.rs`。测试证据覆盖 ID 顺序、借还 FIFO、同一对象复用、空池阻塞、`None` panic 和指标更新。
- 全仓精确文本搜索仅发现根门面、crate 自身测试和 importer 的独立桩，没有发现本 crate 的生产行为调用；因此文档将“生产主链接入”明确标为尚未验证，而非按预期架构推断。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证目标文件存在且固定二级标题恰好为 11 个，并人工复核文档能回答文件存在原因、运行流程与安全扩展边界。
