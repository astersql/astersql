# `pkg/resourcegroup/runaway/flusher.rs`

## 文件定位

本文件属于 `astersql-resourcegroup-runaway` crate；crate 根由 `pkg/resourcegroup/runaway/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/resourcegroup/runaway/lib.rs` 通过 `pub mod flusher` 公开本模块。它实现一个与业务记录类型解耦的泛型内存批量器 `BatchFlusher<K, V>`，负责合并条目并在数量阈值、显式请求或时间条件满足时调用写出回调。

当前接线状态必须与 Go 版本区分：仓库内 Rust 生产代码尚未实例化 `BatchFlusher`，可确认的 Rust 直接使用者只有独立测试 `pkg/resourcegroup/runaway/flusher_test.rs`。完整的生产接线目前仍见于 `pkg/resourcegroup/runaway/manager.go::RunawayRecordFlushLoop`，它创建三种 Go `batchFlusher`，分别处理 runaway 记录、quarantine 记录和过期 quarantine 记录。Rust `pkg/resourcegroup/runaway/manager.rs` 当前使用三个 `Mutex<Vec<_>>` 队列，并未调用本文件。

## 核心职责

- `BatchFlusher::new` 校验条数阈值并初始化指定容量的 `HashMap`、合并/写出回调和本地计数器。
- `BatchFlusher::add` 拒绝停止后的写入，累计入队次数，通过调用方提供的 `merge_fn` 插入或聚合同键值，并在不同键的数量达到 `threshold` 时同步写出。
- `BatchFlusher::flushIfDue` 提供由外部调度器传入 `Instant` 的时间触发入口；本类型自身不创建 ticker 或后台任务。
- `BatchFlusher::flush` 调用 `flush_fn`，按结果更新成功/失败次数，并在每次非空尝试后丢弃整个批次。
- `BatchFlusher::stop` 先尝试最后一次写出，再永久拒绝后续 `add`；`len`、`is_empty`、`buffer` 提供观察接口。

该类型只提供批处理机制，不生成 SQL、不持有会话池，也不负责重试、持久化事务、日志或 Prometheus 指标；这些能力需要由 `flush_fn` 或未来的上层接线提供。

## 主要符号

- `pub struct BatchFlusher<K, V>`（`flusher.rs:29`）：要求实现块中的 `K: Eq + Hash`，以 `HashMap<K, V>` 保存一个批次。公开字段为 `name`、`successful_flushes`、`failed_flushes`、`added`，其余状态封装在类型内部。
- `new(name, interval, threshold, merge_fn, flush_fn) -> Result<Self>`（`:56`）：`merge_fn` 类型为 `Fn(&mut HashMap<K, V>, K, V) + Send + Sync + 'static`，`flush_fn` 类型为 `Fn(&HashMap<K, V>) -> Result<()> + Send + Sync + 'static`。`threshold == 0` 返回 `Error::InvalidArgument`。
- `add(&mut self, key, value) -> Result<()>`（`:84`）：串行修改缓冲；先增加 `added`，再合并，最后按 `buffer.len()` 判断阈值。阈值衡量的是合并后的唯一键数，而不是 `add` 调用数。
- `flushIfDue(&mut self, now) -> Result<bool>`（`:97`）：在没有历史写出时间时，只要缓冲非空就到期；已有历史时间时按 `now.duration_since(last) >= interval` 判断，并返回本次是否判定到期。
- `flush(&mut self) -> Result<()>`（`:113`）：空批次不调用回调，并把 `last_flush_time` 重置为 `None`；非空批次记录回调结果、重建空 `HashMap`，把时间更新为函数内部的 `Instant::now()`，最终始终返回 `Ok(())`。
- `stop(&mut self) -> Result<()>`（`:133`）：调用 `flush` 后设置 `stopped = true`。由于 `flush` 当前吞掉写出回调错误，正常状态下该方法也返回 `Ok(())`。
- `len`、`is_empty`、`buffer`（`:140`、`:144`、`:148`）：分别返回唯一键数、空状态和当前映射的只读引用。

本文件没有模块级常量、trait、枚举、条件编译项或后台执行入口。

## 执行流程

1. 上层调用 `new`，传入合并策略和实际写出策略；构造函数拒绝零阈值，并按阈值预分配缓冲。
2. 每次 `add` 先检查 `stopped`。未停止时增加 `added`，再让 `merge_fn` 决定覆盖、去重或聚合方式。
3. 合并后的唯一键数达到阈值时，`add` 同步调用 `flush`；未达到时数据留在当前缓冲。
4. 外部调度代码也可调用 `flushIfDue(now)`。第一次有数据时立即判定到期；非第一次则比较传入时间与最近一次非空写出尝试的时间。
5. `flush` 对空缓冲直接返回。非空时把当前映射以共享引用传给 `flush_fn`，按 `Result` 增加一个成功或失败计数，然后无条件换成新映射，避免失败数据混入下一批。
6. `stop` 复用同一写出路径清理剩余数据，并设置停止标志；停止只限制 `add`，代码并未禁止之后显式调用 `flush`、`flushIfDue` 或观察方法。

一个细节是：非空 `flush` 后 `last_flush_time` 为 `Some`。若缓冲一直为空而外部在间隔到期后调用 `flushIfDue`，它仍会返回 `true` 并进入空 `flush`，后者再把时间重置为 `None`；这是当前代码的精确行为，不应解释成内部周期任务。

## 数据与状态

- `buffer: HashMap<K, V>` 是唯一待写批次；同键如何处理完全由 `merge_fn` 决定。每次非空写出尝试后替换整个映射，因此先前从 `buffer()` 取得的借用不能跨越可变操作。
- `interval` 仅由 `flushIfDue` 使用；类型不会主动等待或唤醒。
- `threshold` 同时控制自动写出的唯一键数与新映射的初始容量。
- `last_flush_time` 表示最近一次非空写出尝试完成后记录的本地单调时间，而不是“最近一次成功写出”。空 `flush` 会清除它。字段注释中的“成功刷盘”应以函数实现为准：失败尝试也会更新时间。
- `stopped` 初始为 `false`，`stop` 后保持为 `true`，没有重启接口。
- `successful_flushes` / `failed_flushes` 按非空 `flush_fn` 调用次数分类；`added` 统计被接受并执行合并的 `add` 次数。停止后被拒绝的调用不会增加 `added`。
- `name` 当前仅保存标签字符串；本文件没有读取它来输出日志或指标。

## 依赖与调用关系

直接标准库依赖为 `std::collections::HashMap`、`std::hash::Hash` 和 `std::time::{Duration, Instant}`；错误边界复用 crate 根的 `crate::{Error, Result}`。`Cargo.toml` 没有为本文件引入专属第三方依赖；该 crate 的大量本地依赖均位于 `cfg(windows)` 目标节，不能据此推断本文件直接使用它们。

RustCodeGraph 对路径明确的内部调用边为：

- `add -> flush`；
- `flushIfDue -> flush`；
- `stop -> flush`；
- `new -> Error::InvalidArgument`（源码可确认；图对通用枚举构造器存在跨文件歧义）。

RustCodeGraph 为 `add` 和 `flush` 找到的明确外部调用者均在 `pkg/resourcegroup/runaway/flusher_test.rs`；仓库级 `rg` 也未发现其他 Rust 实例化点。因此，当前 Rust 应用主链只能描述为“crate 已导出、测试已覆盖、生产管理器尚未接线”。作为设计对照，Go 主链是 `Manager.RunawayRecordFlushLoop -> newBatchFlusher -> add/flush/stop`，但它不是 Rust 的已存在调用边。

## 错误处理与边界

- `threshold == 0` 在构造期返回 `Error::InvalidArgument("flush threshold must be positive")`，避免每次添加都满足无意义阈值。
- `stop` 后调用 `add` 返回 `Error::Closed`，且不会修改计数或缓冲。
- `flush_fn` 的错误不会向 `flush`、`add`、`flushIfDue` 或 `stop` 的调用者传播；它只增加 `failed_flushes`。失败批次仍被清空，当前实现没有重试或死信队列，这是一项显式的数据丢弃语义。
- `merge_fn` 和 `flush_fn` 是普通 `Fn` 回调；若回调 panic，本类型没有恢复逻辑，清空缓冲和计数更新也可能不会执行。
- `flushIfDue` 使用 `Instant::duration_since`。调用方应传入来自同一单调时钟、语义上不早于已记录时间的值；当前标准库对更早的值按零时长饱和，因此会表现为“尚未到期”，但上层不应依赖时钟倒退来调度。
- 计数器是普通 `u64`，没有溢出处理；release 构建下的溢出行为不能当作业务保证。
- 空 `flush` 不调用 `flush_fn`、不增加成功/失败计数，并清除时间基准。

## 并发与资源生命周期

`BatchFlusher` 的修改方法都要求 `&mut self`，所以该类型不提供内部并发调度或锁。两个回调被约束为 `Send + Sync`，但这不等于 `BatchFlusher` 可在没有外部同步的情况下并发修改；若跨线程共享，应由上层使用 `Mutex`、单线程事件循环或等价串行化机制。

生命周期顺序为“构造 -> 多次添加/显式或到期写出 -> 停止”。缓冲和值由 `BatchFlusher` 拥有，`flush_fn` 只在调用期间借用映射，不能安全保留引用；测试通过克隆映射保存快照。非空写出后旧映射及其中未被回调另行拥有的数据被释放，新映射按 `threshold` 容量重新分配。

本类型没有 `Drop` 实现，因此直接丢弃对象不会自动刷出剩余数据；上层必须显式调用 `stop` 或 `flush`。它也不持有 ticker、线程、异步任务、通道、数据库会话或事务。Go 版本持有 `time.Ticker` 并由管理器的 `select` 循环驱动，而 Rust 版本把时钟驱动责任移给调用者的 `flushIfDue`。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/resourcegroup/runaway/flusher.go`，测试为 `flusher_test.go`。两版共同语义包括：泛型键值缓冲、调用方定义合并策略、按唯一键数触发、空批次不执行回调、每次写出尝试后丢弃批次，以及写出失败不向添加方传播。Rust 测试 `threshold_flush_and_explicit_empty_flush_match_go`、`merge_and_interval_flush_preserve_one_batch`、`flush_error_is_counted_and_swallowed_before_next_batch` 分别验证阈值/空刷、同键聚合/时间入口、失败计数与失败批次隔离。

已确认的差异如下：

- Go `newBatchFlusher` 接收 SQL 生成函数和 session pool，在内部组成数据库写入回调；Rust 构造函数直接接收 `flush_fn`，更通用但尚未接入持久化层。
- Go 自带 `time.Ticker` 和 `tickerCh`，`stop` 会停止 ticker；Rust 无 ticker，以 `flushIfDue(now)` 暴露轮询式时间判断。
- Go 写入 Prometheus 的 batch size、duration、interval、成功/失败和 add 指标并记录日志；Rust 只有三个公开累计数，不记录批量大小、耗时或相邻间隔，`name` 也尚未消费。
- Go 的 threshold 没有构造期正数校验，Rust 明确拒绝零值；Rust 还用 `Error::Closed` 定义了停止后添加的错误，而 Go 的 `stop` 不设置同类状态。
- Go 使用尝试开始时的 `now` 更新 `lastFlushTime`；Rust 在回调之后用新的 `Instant::now()` 更新时间。
- Go 生产管理器建立三类 flusher 并从通道/ticker 驱动；Rust `manager.rs` 尚未使用本组件，这是迁移缺口而不是本文件已经实现的能力。

## 扩展指南

- 若接入 Rust `Manager`，应在管理器的单线程循环或明确的互斥边界内为三类记录分别构造 `BatchFlusher`，把 `record.rs` 的 SQL 生成与 `RestrictedSqlExecutor` 调用封装进 `flush_fn`；不要仅因 Go 存在对应接线就宣称 Rust 已持久化。
- 若增加自动定时驱动，最小接入点是 `flushIfDue`；应明确谁拥有时钟/任务、如何停止任务，以及空闲期是否继续返回到期。不要在本类型内部和上层同时创建计时器。
- 若修改失败策略，应集中修改 `flush`，并同步决定是否保留失败批次、是否传播错误及 `stop` 的语义；这会影响数据丢失、重复写入和重试幂等性。
- 若增加指标，`name` 可作为标签，计数更新点在 `add` 与 `flush`；还需考虑普通 `u64` 与并发采集的可见性。性能上应关注每批重新分配 `HashMap::with_capacity(threshold)` 以及 `threshold` 过大带来的常驻/峰值内存。
- 若改变合并规则，优先由构造方替换 `merge_fn`，保持本类型通用；阈值仍按唯一键数计算，测试必须覆盖重复键不会错误提前刷出。
- Rust 测试必须继续放在独立的 `pkg/resourcegroup/runaway/flusher_test.rs`，不要嵌入 `flusher.rs`。至少同步覆盖零阈值、停止后添加、时间未到/到期/空缓冲、回调失败、panic 或时钟前移策略中实际承诺的部分；Go 对齐变化还应核对 `flusher_test.go` 与 `manager.go::RunawayRecordFlushLoop`。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 `11467` 个文件、`307296` 个节点、`1848419` 条边；`files --filter pkg/resourcegroup/runaway` 确认目标 Rust/Go 源与独立测试均在索引中。
- RustCodeGraph `node --file pkg/resourcegroup/runaway/flusher.rs --offset 1 --limit 500`：核对完整 151 行源码及 14 个文件符号。
- RustCodeGraph `node flusher.rs::{new,add,flushIfDue,flush,stop,len,is_empty,buffer}`：核对函数签名、源码和路径明确的 `add/flushIfDue/stop -> flush` 调用边；图中通用名称产生的跨包候选未作为结论依据。
- 读取 `pkg/resourcegroup/runaway/Cargo.toml` 与 `lib.rs`：核对 crate 名称、crate 根、模块导出、公共 `Error`/`Result` 以及测试作为独立模块的装配方式。
- 读取 `pkg/resourcegroup/runaway/flusher.go`、`flusher_test.go` 和 `manager.go::RunawayRecordFlushLoop`：核对 Go 的 ticker、指标、SQL/session pool 写出、失败吞掉、三类生产 flusher 和停机写出流程。
- 读取 `pkg/resourcegroup/runaway/flusher_test.rs` 与 `manager.rs`，并用 `rg` 搜索 Rust `BatchFlusher` / `flushIfDue` 使用点：核对 Rust 测试边界以及当前没有生产实例化点。
- 本任务是纯文档分析，按计划不运行 Cargo；结构检查要求本文恰有十一个规定的二级标题。
