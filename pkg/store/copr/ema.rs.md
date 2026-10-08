# `pkg/store/copr/ema.rs`

## 文件定位

`ema.rs` 属于 `astersql-store-copr` crate；crate 根在 `pkg/store/copr/lib.rs` 中以 `pub mod ema` 声明模块并通过 `pub use ema::*` 再导出其公开项。文件实现 coprocessor 分页读取链路使用的、按观测时间衰减的读取字节数指数移动平均（EMA）。它不负责发送请求或解析响应，而是由 `pkg/store/copr/coprocessor.rs` 中的 `CopIterator` 和 `CopTaskWorker` 持有并调用：估计值在发请求前成为 `CopWireRequest::predicted_read_bytes`，实际读取量在具有后续范围的响应到达后反馈给 EMA。

`pkg/store/copr/Cargo.toml` 将该目录定义为库 crate（`[lib] path = "lib.rs"`）。本文件只依赖 Rust 标准库的 `Mutex`、`Duration` 和 `Instant`，不直接使用该 manifest 中的外部依赖，也没有 feature 或条件编译分支。

## 核心职责

- `RuEma` 保存一个逻辑扫描内共享的读取字节数估计，供多个 coprocessor worker 对后续请求做 RU 预扣估算。上游证据是 `CopIterator::new` 创建一个 `Arc<RuEma>`，worker 启动时通过 `CopTaskWorker::with_ema` 共享它。
- `RuEma::observe` 按相邻有效观测的时间间隔计算新样本权重：`alpha = 1 - exp(-Δt / tau)`，再以 `value += alpha * (sample - value)` 更新估计。固定时间常数 `DEFAULT_RU_EMA_TAU` 为一秒，因此间隔越大，新样本权重越高。
- `RuEma::predict` 返回当前估计的整数读取字节数。`CopTaskWorker::handle_task_once` 仅在任务启用行分页或请求指定字节分页预算时把它写入 wire request。
- `Mutex` 将值和最后观测时间作为一个原子状态保护，使并发 worker 的观测与预测串行化，避免读到不一致的 `(value, last_time)`。

## 主要符号

- `DEFAULT_RU_EMA_TAU: Duration`：私有模块常量，值为一秒，是指数衰减时间常数 `tau`。当前没有构造参数可以覆盖它。
- `pub struct RuEma`：公开结构体，但字段私有。`state: Mutex<(f64, Option<Instant>)>` 的二元组依次保存当前浮点估计和最后一个被接受为最新的单调时刻；`tau: Duration` 保存衰减常数。结构体派生 `Debug`，未实现 `Clone`，共享由调用方的 `Arc<RuEma>` 完成。
- `pub fn RuEma::new(seed_read_bytes: u64) -> Self`：以请求的分页字节预算作为初始预测，将其转换为 `f64`；最后观测时间初始化为 `None`，使首个真实样本获得权重 `1.0` 并完全替换种子。
- `pub fn RuEma::observe(&self, bytes: u64, now: Instant)`：在锁内更新估计。无历史时间时 `alpha = 1.0`；有历史时用 `Instant::saturating_duration_since` 将倒退或相同时间的间隔压为零。只有 `now > last` 才推进最后时刻，过期样本不会倒拨后续计算的时间基准。
- `pub fn RuEma::predict(&self) -> u64`：在锁内读取 `f64` 估计并用 Rust 浮点到整数的 `as u64` 转换返回；正常输入形成的估计非负。该接口不返回快照时间或置信度。

## 执行流程

1. `CopIterator::new` 用 `request.paging.size_bytes` 调用 `RuEma::new`。初值可为零，也可为调用方指定的字节预算。
2. `CopIterator::open` 把同一个 `Arc<RuEma>` 克隆给轻量 worker 或线程 worker；因此一次逻辑扫描的 worker 共享学习结果，而不同 iterator 各自维护状态。
3. `CopTaskWorker::handle_task_once` 构造 `CopWireRequest`。若 `task.paging` 为真，或请求级 `paging.size_bytes > 0`，就调用 `predict()` 填充 `wire.predicted_read_bytes`；否则维持初始值零。
4. 请求 admission/RU 计算使用该预测值预估读取成本；`DefaultCopBackend::on_request_wait` 可见其按每 64 KiB 一个读成本单位折算。响应阶段再依据实际读取量与预扣值结算差额。
5. `handle_task_once` 收到响应后，只有 `response.range.is_some()` 且 `response.read_bytes > 0` 时才以 `Instant::now()` 调用 `observe()`。这意味着只对仍有续页的非零读取反馈进行学习；最终页与零字节响应不改变 EMA。
6. `observe()` 取得互斥锁，计算时间权重并更新数值；随后仅在时间严格前进时记录新时刻。后续 worker 的 `predict()` 将看到完整更新后的状态。

## 数据与状态

状态的数值部分以 `f64` 保存，样本和对外预测以 `u64` 表示。核心不变量是估计采用凸组合：对于有限的非负输入和正的一秒 `tau`，`alpha` 位于 `[0, 1)`（首样本特判为 `1`），所以新估计位于旧估计和新样本之间，不应超调。`predict()` 的整数转换会舍弃小数部分，因此测试对浮点计算结果采用最多一个字节的容差。

时间状态使用 `Option<Instant>` 而不是墙上时钟：`None` 表示尚无真实观测；`Some(last)` 表示最近被接受的严格较新时刻。`saturating_duration_since` 保证乱序时间不产生负间隔；相同或更早的时间得到 `alpha = 0`，既不改变估计，也不倒拨 `last`。大于 `tau` 很多的间隔会令 `alpha` 接近一，使陈旧样本影响迅速衰减，但在已有历史时数学上不会精确等于一。

EMA 的生命周期由 `Arc` 所有权管理，文件本身没有全局变量、缓存持久化或清理动作。新的 `CopIterator` 会创建新 EMA，不跨逻辑扫描复用历史。

## 依赖与调用关系

上游调用链以 `pkg/store/copr/coprocessor.rs` 为准：

- `CopIterator::new -> RuEma::new(request.paging.size_bytes)` 创建扫描级实例。
- `CopIterator::open -> CopTaskWorker::with_ema(Arc::clone(&self.ema))` 把同一实例交给并发 worker。
- `CopTaskWorker::handle_task_once -> RuEma::predict` 在分页相关请求发送前设置 `CopWireRequest::predicted_read_bytes`。
- `CopTaskWorker::handle_task_once -> RuEma::observe` 在带剩余范围且实际读取字节非零的响应后更新预测。

下游依赖全部来自标准库：`Mutex::lock` 负责状态同步，`Instant::saturating_duration_since` 计算非负单调间隔，`Duration::as_secs_f64` 提供公式单位，`f64::exp` 计算指数衰减。RustCodeGraph 的文件查询确认 `ema.rs` 被 `coprocessor.rs` 和 `ema_test.rs` 使用；精确探索结果还列出生产调用者 `handle_task_once` 以及五个独立 Rust 测试调用者。

## 错误处理与边界

该 API 没有 `Result` 返回值。两处锁获取都使用 `expect("RU EMA lock poisoned")`：若某线程持锁期间 panic 导致 mutex 中毒，后续 `observe()` 或 `predict()` 会继续 panic，而不是静默使用可能不一致的状态。这是当前明确的失败边界。

首个观测无论构造时是否有种子都以 `alpha = 1` 完全替换初值。乱序或同一时刻的观测权重为零；它们不会更新值或最后时刻。`bytes = 0` 本身是合法样本，但生产调用点显式跳过 `response.read_bytes == 0`，只有直接调用 API 才会用零样本衰减估计。极大 `u64` 转为 `f64` 可能损失整数精度，返回 `u64` 时也会截断小数；当前测试和调用场景没有要求逐字节精确保存超大值。

文件不校验 `tau`，但当前唯一赋值是非零的一秒常量，因此不存在除零路径。若未来允许配置 `tau`，必须拒绝零时长并为极小/极大时间常数补边界测试。

## 并发与资源生命周期

`observe(&self, ...)` 与 `predict(&self)` 都通过同一 `Mutex` 访问整个状态，允许 `RuEma` 经 `Arc` 在多个线程之间共享。锁只包围常数时间的浮点计算或读取，不跨网络请求、通道发送或休眠持有。`ema_concurrent_observe_and_predict` 用八个写线程和一个持续读线程验证不会 panic、死锁且最终预测为正；它验证线程安全的基本行为，但没有证明 worker 数量很大时的锁竞争上界。

`CopIterator` 拥有主 `Arc`，每个 worker 持有克隆；worker 退出和 iterator 被丢弃后引用计数归零，`Mutex` 状态自动释放。模块不创建线程、任务、通道或计时器，也没有显式关闭协议。`Instant` 由调用方注入，使单元测试可构造稳定时间序列；生产路径在响应处理时使用 `Instant::now()`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/copr/ema.go`。Rust `RuEma` 对应 Go 私有类型 `ruEMA`，`new/observe/predict` 分别对应 `newRUEMA/Observe/Predict`，两者均采用一秒 `tau`、同一指数公式、mutex 保护以及不倒拨最后观测时间的规则。Rust 将 Go 的 `value`、`lastObsAt` 合并到一个 mutex 元组中，并用 `Option<Instant>` 表示 Go 的零值 `time.Time`。

首样本语义等价但实现更明确：Go 从零时间计算一个极大间隔，使 `alpha` 数值上约等于一；Rust 在 `last == None` 时直接选择 `1.0`。负时间差方面，Go 显式把 `dt < 0` 设为零，Rust 使用 `saturating_duration_since` 得到同样结果。两边预测都在锁内把浮点值转换为 `uint64/u64`。

生产接线也保持同一意图：Go `copIterator.ema` 注释说明每个逻辑扫描一个 EMA、跨 worker 共享；`predictedReadBytesForTask` 在分页或字节预算场景读取预测，`handleCopPagingResult` 仅对存在后续分页范围的非零读取量调用 `Observe`。Rust 把等价逻辑并入 `CopTaskWorker::handle_task_once`。Rust 独立测试 `pkg/store/copr/ema_test.rs` 与 Go `pkg/store/copr/ema_test.go` 对齐五类行为：种子与首样本、工作负载迁移、大时间间隔、非单调时间、并发读写。

## 扩展指南

- 调整衰减策略时，优先修改 `DEFAULT_RU_EMA_TAU` 或把 `tau` 变为经过校验的构造参数；同步更新 `pkg/store/copr/ema_test.rs` 的迁移速度、大间隔和非单调时间断言，并核对 Go `defaultRUEMATau` 的兼容语义。时间常数改变会影响 RU 预扣响应速度和短期波动。
- 改变观测条件或样本定义时，需要同时检查 `CopTaskWorker::handle_task_once` 中 `response.range/read_bytes` 的门槛、wire request 的 `predicted_read_bytes` 赋值，以及请求/响应 admission 的差额结算。把最终页纳入观测可能降低保守性，但会改变下一任务的预算预测。
- 增加统计字段、置信度或快照时间时，应让相关字段仍在同一锁域内读取/更新，避免值与时间分离造成撕裂；不要把测试内嵌回 `ema.rs`，继续放在独立的 `pkg/store/copr/ema_test.rs`。
- 若要降低高并发锁竞争，需先保留“值与最后时间原子更新”和乱序时间不倒拨两个不变量，再评估原子浮点或分片方案。并发测试应扩展为可重复的顺序/边界断言，而不只验证无 panic。
- 任何 Go/Rust 对齐修改都应同时复查 `pkg/store/copr/ema.go`、`ema_test.go`、`coprocessor.go` 及 Rust 对应文件，特别关注首样本精确替换与整数截断差异。

## 验证依据

- 目标源码：`pkg/store/copr/ema.rs`，核对常量 `DEFAULT_RU_EMA_TAU`、类型 `RuEma` 及 `new/observe/predict` 的完整实现；文件无条件编译项。
- crate 边界：`pkg/store/copr/Cargo.toml` 与 `pkg/store/copr/lib.rs`，确认库名、模块声明、公开再导出和独立测试模块；目标包没有 `doc.go`。
- Rust 生产调用：`pkg/store/copr/coprocessor.rs` 中 `CopIterator::new/open`、`CopTaskWorker::new/with_ema/handle_task_once`、`CopWireRequest::predicted_read_bytes` 以及请求/响应 RU 结算代码。
- Go 对照：`pkg/store/copr/ema.go` 的 `ruEMA/newRUEMA/Observe/Predict`，以及 `pkg/store/copr/coprocessor.go` 的扫描级共享、预测与反馈调用点。
- 测试证据：`pkg/store/copr/ema_test.rs` 与 `pkg/store/copr/ema_test.go`，覆盖种子/无种子首样本、稳定值、负载迁移、大间隔、乱序及并发访问。
- RustCodeGraph：`status` 显示本仓库索引包含目标 Rust/Go 文件；`node --file pkg/store/copr/ema.rs` 返回完整 51 行源码并标出 `coprocessor.rs`、`ema_test.rs` 两个使用文件；`query RuEma` 定位 Rust/Go 对照符号；`node/callees with_ema` 与精确 `explore` 确认共享入口、生产 `handle_task_once` 调用以及五个测试调用者。通用名称的全库 `callers observe/predict` 查询因重名超时，相关边已用精确探索和直接调用点复核。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前运行任务指定的 11 章节结构命令，并人工复核关键结论均可回溯到上述符号或文件。
