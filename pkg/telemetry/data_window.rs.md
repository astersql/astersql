# `pkg/telemetry/data_window.rs`

## 文件定位

本文件属于 `astersql-telemetry` crate 的时间窗口统计实现。模块由 [`pkg/telemetry/lib.rs`](lib.rs) 以 `mod data_window` 装配并整体再导出；crate 的入口由 [`pkg/telemetry/Cargo.toml`](Cargo.toml) 指向 `lib.rs`。它负责保存语句执行、TiFlash 下推与扫描、Coprocessor 缓存命中率分桶及内置函数使用次数，并把一分钟级快照合并成小时级上报数据。

在当前 Rust 调用链中，[`generateTelemetryData`](data.rs) 调用 `getWindowData`，结果写入 `telemetryData::WindowedStats`，随后由 [`ReportUsageData`](telemetry.rs) 序列化并缓存。仓库搜索没有发现 Rust 生产代码调用 `RotateSubWindow` 或递增本文件的全局原子计数器，因此窗口的“读取与上报”已经接线，而“定时轮转与生产端计数”尚未接入 Rust 运行链；不能仅依据本文件存在这些 API 就视为完整可用。

## 核心职责

- 用 12 个 `AtomicU64` 保存当前子窗口的执行、TiFlash 和 Coprocessor 缓存指标（`counters!`，`data_window.rs:15-30`）。
- 通过 `BuiltinFunctionsUsage = HashMap<String, u32>`、`BuiltinUsageExt::{Inc, Merge}` 和 `GlobalBuiltinFunctionsUsage` 聚合标量内置函数使用次数（`data_window.rs:40-76`）。
- `RotateSubWindow` 以原子 `swap(0)` 和一次 `Dump` 把当前累计转成 `windowData`，再将其放入最多 360 项的进程内缓冲（`data_window.rs:198-229`）。
- `getWindowData` 每 60 个子窗口聚合为一个结果；不足 60 个的尾组也会上报（`data_window.rs:263-278`）。
- `windowData::Marshal` 输出与 Go JSON 标签一致的字段，并将时间编码为 UTC RFC3339 风格文本（`data_window.rs:115-173`）。

## 主要符号

- `WindowSize = 3600s`、`SubWindowSize = 60s`：公开的逻辑窗口与采样周期。内部 `IN_WINDOW = 60` 与二者的比值一致；`MAX_SUB_WINDOWS = 360` 表示约六小时保留量。
- `CurrentExecuteCount`、`CurrentTiFlash*`、`CurrentCoprCacheHitRatio*`：公开的进程级原子计数器。写入者应使用适合自身同步需求的原子操作；轮转统一使用 `Ordering::AcqRel` 取值并清零。
- `BuiltinFunctionsUsage`：函数签名名到 `u32` 次数的映射。`BuiltinUsageExt::Inc` 增加单个名称，`Merge` 合并另一映射；两者均使用 `wrapping_add`，保持 Go 无符号整数溢出的回绕语义。
- `builtinFunctionsUsageCollector`：内部持有 `Mutex<BuiltinFunctionsUsage>`；公开方法 `Collect` 合并一批数据，`Dump` 用 `mem::take` 原子地取出当前映射并替换为空映射。其类型名本身不是公开 API，但 `GlobalBuiltinFunctionsUsage` 的公开静态类型使其方法可由 crate 使用者调用。
- `coprCacheUsageData`、`tiFlashUsageData`、`windowData`：分别描述缓存分桶、TiFlash 指标和完整窗口。字段公开，但类型保持模块私有；借助 `lib.rs` 的再导出，它们仍不会成为外部 crate 可命名的公开类型。
- `windowData::Marshal`：生成完整 JSON 字符串；内置函数 map 的键先排序，保证结果稳定。`json_quote` 负责字符串引号、反斜线、换行及控制字符转义。
- `RotateSubWindow`：公开轮转入口。它截断旧数据时保留最近的 `MAX_SUB_WINDOWS` 项。
- `merge`：crate 内可见的字段逐项合并函数，也是 Rust 溢出兼容测试的直接入口。
- `getWindowData`：公开函数，但返回私有 `windowData`；实际主要由同 crate 的 `generateTelemetryData` 使用。

## 执行流程

1. 预期的生产者对各 `Current*` 原子计数器执行增量，并将会话的 `BuiltinFunctionsUsage` 交给 `GlobalBuiltinFunctionsUsage.Collect`。当前 Rust 仓库只在独立测试中模拟了这一步，尚未找到生产接线。
2. 调度者应每 `SubWindowSize` 调用 `RotateSubWindow`。该函数先记录 `SystemTime::now()`，再依次对 12 个原子计数器执行 `swap(0, AcqRel)`，并调用 `Dump` 转移内置函数映射。
3. 完整快照构造后才获取 `windows()` 的互斥锁，追加到 `Vec<windowData>`；若长度超过 360，从头删除最旧项。
4. `generateTelemetryData` 调用 `getWindowData`。函数持有窗口锁，将切片按 60 项分组，以每组首项的克隆为基底，再用 `merge` 累加其余项。
5. 每组保留首个子窗口的 `BeginAt`，计数使用回绕加法；最后不足 60 项的组也产生一个窗口。
6. `telemetryData::Marshal` 对每个窗口调用 `windowData::Marshal`，形成上报载荷中的 `windowedStats` 数组。

## 数据与状态

状态均为进程内全局状态，不持久化：原子计数器保存尚未轮转的数据；`GlobalBuiltinFunctionsUsage` 保存尚未 Dump 的函数计数；`windows()` 内的 `Vec` 保存已经轮转的快照。进程重启会清空全部状态，上报本身也不会删除窗口缓冲。

`RotateSubWindow` 的快照不保证所有指标来自同一个严格瞬间：各原子计数器按字段顺序交换，内置函数映射随后独立加锁转移。并发写入恰好跨越这些操作时，可能进入当前或下一个子窗口，但每个原子交换本身不会丢失已经纳入交换的值。`BeginAt` 是轮转发生时刻，不是通过整分钟边界计算出的时间。

`windowData::Marshal` 直接构造 JSON。内置函数键经过排序；整数按十进制输出。`format_time` 对 Unix epoch 之后的 `SystemTime` 生成 UTC 时间，纳秒尾随零会被移除；若时间早于 epoch，`duration_since` 的错误被 `unwrap_or_default` 降级为 epoch，因此不会保留真实的负时间。

## 依赖与调用关系

- 上游：`pkg/telemetry/data.rs::generateTelemetryData -> getWindowData`；`pkg/telemetry/telemetry.rs::ReportUsageData -> generateTelemetryData -> telemetryData::Marshal -> windowData::Marshal`。
- 内部下游：`RotateSubWindow -> builtinFunctionsUsageCollector::Dump` 与 `windows`；`getWindowData -> windows -> merge -> BuiltinUsageExt::Merge`；`Marshal -> format_time/json_quote`。
- 模块边界：`pkg/telemetry/lib.rs` 声明并再导出本模块，并以 `#[path = "data_window_test.rs"]` 挂载独立测试文件。
- Cargo 边界：本文件只使用 `std`。`Cargo.toml` 中列出的包依赖全部位于 `target.'cfg(any())'`，该条件恒假，因而不是当前编译中本文件的有效运行依赖。
- Go 生产链证据：`pkg/session/session.go` 递增 `CurrentExecuteCount` 并收集内置函数用量；`pkg/domain/domain.go` 调用 `RotateSubWindow`。仓库中没有对应的 Rust 调用边，属于当前移植接线缺口，而非本文件内部逻辑。

RustCodeGraph 对目标文件识别出 31 个符号，并确认 `data.rs`、`telemetry.rs` 等相邻文件；但对精确 `callers/callees` 命令未返回边，因此上述跨文件接线由仓库级符号搜索补证。

## 错误处理与边界

- 本文件没有 `Result` 返回路径。`Mutex::lock().expect(...)` 遇到锁毒化会 panic，包括内置函数收集器和窗口缓冲。
- 所有用量加法都显式回绕：函数计数为 `u32`，窗口指标为 `u64`。这不是饱和计数，也不会报告溢出。
- `getWindowData` 对空缓冲返回空向量。`chunk[0]` 安全依赖标准库 `chunks(IN_WINDOW)` 不产生空块且 `IN_WINDOW` 固定为 60；若未来允许窗口大小为 0，必须先保护该不变量。
- 缓冲最多保留 360 个已轮转子窗口；继续轮转会静默丢弃最旧数据。当前常量是固定六小时，未动态引用 `telemetry.rs::ReportInterval`。
- `Collect` 接受拥有所有权的 map，合并后丢弃输入；`Dump` 会清空全局累计。额外或过早调用 `Dump` 会从后续窗口中移走数据。
- `json_quote` 覆盖 JSON 常见转义与 U+0000–U+001F 控制字符；它是手写序列化路径，新增字符串字段时应复用该函数，不能直接插值未转义文本。

## 并发与资源生命周期

数值指标使用 `AtomicU64`，轮转的 `swap(AcqRel)` 同时读取并归零。内置函数映射使用独立 `Mutex`；`Collect` 和 `Dump` 互斥，因此每一批 map 要么在本次 Dump 前合并，要么留给下一次 Dump。

已轮转窗口由 `OnceLock<Mutex<Vec<windowData>>>` 延迟初始化并存活至进程退出。`RotateSubWindow` 在完成所有原子交换和内置函数 Dump 后才锁住窗口向量，缩短窗口锁持有时间；`getWindowData` 则在克隆和合并全部窗口期间一直持锁，会阻塞同期轮转，成本随最多 360 个子窗口和其中函数 map 的大小增长。

本文件不创建线程、定时器、异步任务或通道。`SubWindowSize` 只是周期契约，真正的调度必须由外部完成；当前 Rust 生产链尚无该调度者。两个全局状态使用不同的锁，因此不存在本文件内部的嵌套锁顺序，但快照也不是跨两类状态的单一事务。

## 与 Go 版本的对应关系

直接对照文件为 [`pkg/telemetry/data_window.go`](data_window.go)，测试对照为 [`pkg/telemetry/data_window_test.go`](data_window_test.go)。Rust 保留了 Go 的核心模型：相同的计数器集合、1 分钟子窗口、1 小时聚合、约 6 小时保留、函数用量 `uint32`、窗口指标 `uint64`、取出即清空的 `Dump`，以及以首项时间作为合并窗口起点。

主要实现差异如下：

- Go 的 `maxSubWindowLength` 从 `ReportInterval / SubWindowSize` 计算，Rust 固定为 360；按当前 Go `ReportInterval = 6h` 两者等价，但未来修改报告周期时 Rust 不会自动同步。
- Go 使用 `RWMutex`，读取聚合可持读锁；Rust 使用普通 `Mutex`，所有读写串行。
- Go `getWindowData` 返回指针切片，Rust 返回拥有所有权的克隆值；两者都不修改已保存的子窗口。
- Go 依赖 `encoding/json` 和 `time.Time` 序列化，Rust 手写 `Marshal`、时间格式化和字符串转义。Rust 测试只覆盖一个带纳秒的 epoch 后时间及关键字段，不代表所有 `time.Time.MarshalJSON` 边界均已等价。
- Go 的 `domain.go` 定时调用轮转，`session.go` 写入执行和函数使用计数；这些生产接线尚未在 Rust 侧出现。
- Rust 明确用 `wrapping_add` 复现 release 构建下 Go 无符号整数回绕，并由独立测试覆盖；这避免 Rust debug 构建普通 `+` 的溢出 panic。

## 扩展指南

新增指标时，应同时修改：对应 `Current*` 原子计数器、`windowData` 或其子结构、`RotateSubWindow` 的交换、`merge` 的累加、`Marshal` 的字段输出，以及独立的 [`data_window_test.rs`](data_window_test.rs)。若该字段需从 SQL 执行链产生，还必须补上真实 Rust 生产者；只添加计数器和测试模拟不足以形成运行时功能。

调整时间窗口时，应使 `WindowSize`、`SubWindowSize`、`IN_WINDOW`、`MAX_SUB_WINDOWS` 与 `ReportInterval` 保持可验证的一致关系，并检查非整除情况下的取整约定。若改为动态配置，需要避免 `chunks(0)`，并说明配置变更时现存缓冲如何处理。

修改序列化时，要逐字段核对 Go JSON tag、`time.Time` 行为、map 键排序和字符串转义；优先增加独立测试，而不要把测试嵌入生产源文件。并发方向的优化可考虑缩短 `getWindowData` 的锁持有期（例如锁内取得快照、锁外聚合），但必须验证克隆成本、窗口一致性和轮转期间的可见性。将轮转接入生产环境时，应在与 Go `domain.go` 等价的生命周期位置启动和停止调度，避免重复调度或无人调度。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标目录 18 个索引文件；`files --filter pkg/telemetry` 确认目标、Go 对照及测试均已索引；`node --file pkg/telemetry/data_window.rs --offset 1 --limit 400` 读取目标全部 278 行；`query WindowData`、`query RotateSubWindow`、`query getWindowData` 定位 Rust/Go 对应符号；精确 `callers/callees` 无输出，故未将其当作完整调用图。
- Rust 源与模块证据：`pkg/telemetry/data_window.rs`、`pkg/telemetry/data.rs`、`pkg/telemetry/telemetry.rs`、`pkg/telemetry/lib.rs`。
- crate 证据：`pkg/telemetry/Cargo.toml`。
- Go 对照与生产接线：`pkg/telemetry/data_window.go`、`pkg/telemetry/data.go`、`pkg/telemetry/data_window_test.go`、`pkg/session/session.go`、`pkg/domain/domain.go`。
- Rust 独立测试：`pkg/telemetry/data_window_test.rs` 覆盖 `Collect`/`Dump`、`u32` 和 `u64` 回绕、TiFlash 计数器模拟、JSON 字段与纳秒时间编码；它没有覆盖 `RotateSubWindow` 的缓冲上限、`getWindowData` 的 60 项分组、并发轮转或锁毒化。
- 人工复核结论：文件存在是为了把离散运行计数转换为有界的分钟快照和小时上报数据；内部聚合与序列化已实现，但 Rust 生产端计数和周期轮转尚未接线，安全扩展必须同时维护采集、轮转、合并、序列化与独立测试五个层面。
