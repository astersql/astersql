# `pkg/util/execdetails/util.rs` 逻辑说明

## 文件定位

`pkg/util/execdetails/util.rs` 是 `astersql-util-execdetails-util` 内部支撑 crate 的主体实现之一。它本身不是由顶层 `pkg/util/execdetails/lib.rs` 直接声明为模块，而是被 `pkg/util/execdetails/internal/util/lib.rs` 以 `include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../util.rs"))` 编译；随后顶层 `lib.rs` 通过 `pub mod util { pub use execdetails_util::*; }` 把其公共 API 暴露为 `astersql_util_execdetails::util::*`。对应 Cargo 边界见 `pkg/util/execdetails/internal/util/Cargo.toml`（直接依赖 `tdigest = "1.0.0"`）和聚合 crate 的 `pkg/util/execdetails/Cargo.toml`（依赖内部 `execdetails-util` crate，并声明 Go 包映射 `pkg/util/execdetails`）。

该文件对应 Go 的 `pkg/util/execdetails/util.go`，负责三组通用能力：执行明细在轻量 `Context` 中的初始化、继承、同步和读取；原子 TiKV 执行计数的值快照；泛型百分位统计及 Go 风格耗时格式化。它不负责定义完整 SQL 执行统计模型；`StmtExecDetails`、上下文 key、RU 桩等直接支撑类型由 `pkg/util/execdetails/internal/util/lib.rs` 提供，完整运行时统计另由 `execdetails.rs`、`runtime_stats.rs` 及 `internal/group1/lib.rs` 组装。

## 核心职责

1. **建立语句观测上下文**：`ContextWithInitializedExecDetails` 无条件安装新的 `ExecDetails`、`RUDetails`、`StmtExecDetails`；`ContextWithMissingExecDetailsInitialized` 只补缺项并保留已有 `Arc` 身份；`ContextWithInheritedRUV2Details` 只在目标缺失时复用来源对象。
2. **统一 RUv2 指标落点与同步**：`ContextWithRUV2Metrics`/`contextWithRUV2Metrics` 优先把指标放进已有 `StmtExecDetails`，没有语句对象时才使用独立 `RUV2MetricsCtxKey`；`SyncRUV2MetricsFromContext` 找到指标后，将 `RUDetails` 的待同步值排空并累计进去。
3. **生成稳定的观测快照**：`GetExecDetailsFromContext` 返回写 SQL 响应耗时、TiKV 明细值快照和共享 RU 对象；`LoadTiKVExecDetails` 对 4 个等待/退避字段及 8 个 KV/MPP 流量字段逐项做 `Relaxed` 原子读取，避免把活动原子对象本身交给下游。
4. **聚合延迟样本**：`Percentile<T>` 在样本少于阈值时保留原值并精确排序，达到 `MaxDetailsNumsForOneQuery`（内部 util crate 当前为 1000）后转换为 `tdigest::TDigest`，以受控内存继续近似聚合。
5. **生成可读耗时文本**：`FormatDuration` 按量级裁剪到一至两位小数，再由内部格式化函数产生与 Go `time.Duration.String` 对齐的 `ns/µs/ms/s/m/h` 组合。

## 主要符号

- `ContextWithInitializedExecDetails(Context) -> Context`（公开）：创建新的 `Arc<StmtExecDetails>`，先确保其 RUv2 指标存在，再安装 `ExecDetails`、`RUDetails` 和 statement details。它会替换同类型 key 的已有值，适合明确开启一次全新语句统计。
- `ContextWithMissingExecDetailsInitialized(Context) -> Context`（公开）：逐项检查三个明细对象；若 statement details 缺失，则优先继承独立 key 上的 `Arc<RUV2Metrics>`，否则新建指标。已有对象不会被替换。
- `ContextWithInheritedRUV2Details(Context, Option<Context>) -> Context`（公开）：来源为空时无操作；目标没有 RUDetails 或 RUv2 metrics 时，复用来源中的同一 `Arc`。
- `ContextWithRUV2Metrics`（公开）与 `contextWithRUV2Metrics`（私有）：拒绝 `None`；有 `StmtExecDetails` 时在其互斥保护字段中设置 metrics，否则写独立上下文 key。
- `SyncRUV2MetricsFromContext(&Context) -> Option<Arc<RUV2Metrics>>`（公开）：先通过 `RUV2MetricsFromContext` 按“statement 内部优先、独立 key 次之”查找指标，再调用 `SyncRUV2MetricsFromRUDetails`。
- `GetExecDetailsFromContext(&Context)`（公开）：返回 `(StdDuration, util::ExecDetails, Arc<util::RUDetails>)`；缺项分别回退为零耗时、全零快照和新建空 RUDetails。
- `LoadTiKVExecDetails(Option<&util::ExecDetails>)`（公开）：复制十二个原子计数的当前值；`None` 返回全零结构。
- `canGetFloat64`（公开 trait）：为百分位容器提供统一数值投影；`Int64 = i64`、`Duration = StdDuration` 和 `DurationWithAddr { D, Addr }` 均实现该 trait，其中时长按纳秒转换。
- `Percentile<T>`（公开类型，字段私有）：维护 `values`、样本数、排序标记、最小/最大值、总和及可选 t-digest；公开方法为 `Add`、`GetPercentile`、`GetMax`、`GetMin`、`MergePercentile`、`Size`、`Sum`。
- `FormatDuration`（公开）以及 `formatGoDuration`、`formatDecimalDuration`、`getUnit`（私有）：完成精度裁剪、单位选择和字符串拼装。文件没有条件编译项；专用测试的条件编译挂载位于 `internal/util/lib.rs`。

## 执行流程

**上下文初始化与继承**：全新路径调用 `ContextWithInitializedExecDetails`，得到三类共享对象，其中 statement details 已含 RUv2 metrics。兼容已有上下文的路径调用 `ContextWithMissingExecDetailsInitialized`：先读取 statement details，再分别补齐 TiKV/RU 对象，最后处理 metrics；这样既保留已有 `Arc`，也能把早先暂存在独立 key 上的 metrics 搬入新建 statement details。派生上下文调用 `ContextWithInheritedRUV2Details` 时，只复制目标缺少的 RUDetails 和 metrics，不覆盖目标自己的计量状态。

**RU 同步与读取**：调用者累计数据到 `RUDetails` 后，`SyncRUV2MetricsFromContext` 定位当前 statement metrics，并把 RUDetails 中的待处理计数 drain 到 metrics；若找不到 metrics，直接返回 `None`。读取观测结果时，`GetExecDetailsFromContext` 从 statement details 取写响应耗时，用 `LoadTiKVExecDetails` 建立值快照，并原样返回上下文中的 RUDetails `Arc`。

**百分位追加与查询**：`Add` 每次更新排序标记、总和、样本数及最小/最大值。在 digest 尚未创建时，样本进入 `values`；长度达到 1000 后一次性 drain 为 `f64` 并创建压缩大小为 100 的 t-digest。后续样本以单点 merge 加入 digest。`GetPercentile` 在精确模式下按 `GetFloat64` 排序，并取下标 `len * f`；digest 模式调用 `estimate_quantile`。`MergePercentile` 对精确来源逐点 `Add`，对 digest 来源则先把目标残留值转成 digest，再合并两个摘要。

**耗时格式化**：`FormatDuration` 对不超过 1µs 的值直接输出；其他值按秒、毫秒、微秒选择单位。当前可读数小于 10 时保留两位小数，否则保留一位，使用整数纳秒算术四舍五入，再由 `formatGoDuration` 处理单位以及小时/分钟组合，`formatDecimalDuration` 去除尾随零。

## 数据与状态

- 上下文值由 `internal/util/lib.rs::context::Context` 按 key 类型的 `TypeId` 保存；克隆 `Context` 会克隆其中的共享值。该模型模拟 Go `context.WithValue`，但这里只承载类型化值，没有取消、截止时间或父链语义。
- `Arc<util::ExecDetails>`、`Arc<util::RUDetails>`、`Arc<StmtExecDetails>` 和 `Arc<RUV2Metrics>` 明确表达跨上下文共享身份。初始化函数决定是新建还是复用这些身份；专用测试用 `Arc::ptr_eq` 验证“不覆盖”和“继承同一对象”。
- `LoadTiKVExecDetails` 的返回值是新结构：十二个 `AtomicI64` 都由某一时刻读取出的普通数值重新构造。因此快照不随源对象后续写入变化，但逐字段读取不构成跨字段的事务一致快照。
- `Percentile` 的 `size` 和 `sumVal` 始终覆盖全部已接收样本；`values` 只在精确模式持有原对象，进入 digest 后被清空。`minVal`、`maxVal` 保留原泛型值，因此 `DurationWithAddr` 可同时保留极值对应地址。
- `isSorted` 只缓存精确模式的排序状态；任何 `Add` 或 merge 都将其复位。`GetPercentile` 需要 `&mut self`，因为它可能排序内部数组。

## 依赖与调用关系

编译装配链为：`pkg/util/execdetails/internal/util/lib.rs` 定义轻量 context、执行/RU 类型和 key，然后 `include!` 本文件；`pkg/util/execdetails/lib.rs` 再将整个内部 crate 导出到 `execdetails::util`。`tdigest` 是本文件唯一直接使用的外部算法依赖；其声明位于内部 util crate 的 Cargo 清单，聚合 crate 同时也声明该依赖。

RustCodeGraph 对本文件给出的直接证据包括：`ContextWithRUV2Metrics -> contextWithRUV2Metrics`、`ContextWithInheritedRUV2Details -> contextWithRUV2Metrics`、`GetExecDetailsFromContext -> LoadTiKVExecDetails`；`LoadTiKVExecDetails` 的实际 Rust 调用者包括 `pkg/sessionctx/variable/slow_log.rs`、`pkg/util/stmtsummary/statement_summary.rs` 和 `pkg/util/stmtsummary/v2/record.rs`。`FormatDuration` 在本内部 util crate 的测试及 `pkg/util/execdetails/execdetails_test.rs` 中使用。

需要区分同名实现：顶层 `execdetails::execdetails::*` 来自 `internal/group1/lib.rs`，该文件自己定义了 `Percentile` 和 `FormatDuration` 并供 `runtime_stats.rs`、`execdetails.rs` 使用；本文件的版本通过 `execdetails::util::*` 暴露。因此 `pkg/distsql/select_result.rs` 等对 `execdetails::FormatDuration` 的调用通常解析到 group1 版本，并不能作为本文件格式化函数的直接调用证据。

## 错误处理与边界

- 本文件 API 不返回 `Result`；缺失上下文值通过 `Option` 或零值处理。来源 context/metrics 为 `None` 时，继承和绑定函数无副作用；同步函数缺 metrics 时返回 `None`。
- `GetExecDetailsFromContext` 在缺 RUDetails 时仅返回一个新建对象，并不把它写回原 context；调用者若要持续累计，必须显式初始化上下文。
- `GetPercentile` 没有在函数内部校验空样本或 `f` 的范围。精确模式在空数组或使下标等于/超过长度时会索引 panic；安全调用约束是先保证 `Size() > 0`，并传入能映射到有效下标的分位值（通常 `0.0 <= f < 1.0`）。digest 的非法分位行为由 `tdigest` crate 决定，不能从本文件推断额外保证。
- 浮点排序遇到不可比较值时以 `CmpOrdering::Equal` 处理；当前内建实现来自整数和非负 `StdDuration`，不会产生 NaN，但未来实现 `canGetFloat64` 时必须考虑这一边界。
- `StdDuration` 不能表示负时长；这与 Go `time.Duration` 可为负数的类型能力不同。当前测试和调用证据只覆盖非负执行耗时。
- `FormatDuration` 将中间 `u128` 纳秒结果限制到 `u64::MAX` 后构造 `StdDuration`；超大输入的输出上界因此由 `StdDuration`/该饱和转换决定。

## 并发与资源生命周期

`ExecDetails` 和 `TrafficDetails` 的计数器使用 `AtomicI64`；快照读取采用 `Ordering::Relaxed`，提供单字段原子性而不建立字段间 happens-before 关系。该选择与统计用途及 Go 的逐字段 `atomic.LoadInt64` 对应。RUDetails 测试模型也以 Relaxed 原子进行累加和 drain；并发同步时每次 drain 获取不同的待处理增量。

`StmtExecDetails.metrics` 由 `Mutex<Option<Arc<RUV2Metrics>>>` 保护，`ensureRUV2Metrics`、`getRUV2Metrics`、`setRUV2Metrics` 在短临界区内完成。锁中毒会通过 `expect("metrics lock poisoned")` panic，而不是恢复或返回错误。上下文和其中对象由 `Arc` 管理：继承函数延长同一对象生命周期，不复制累计状态；最后一个引用释放后资源自动销毁，没有显式 close、任务、通道或异步生命周期。

`Percentile` 的修改方法要求 `&mut self`，本身不提供内部同步；若跨线程共享，调用方必须在外层加锁。达到阈值后从 O(n) 原始样本存储切换为固定压缩参数的摘要，以降低长查询统计的内存增长，但后续每个样本当前通过 `take`/`merge_unsorted` 重建摘要，扩展高吞吐路径时需重新评估开销。

## 与 Go 版本的对应关系

`pkg/util/execdetails/util.go` 与本文件在函数分组、初始化顺序、缺失项补齐、RUDetails/metrics 继承、逐字段原子快照、百分位精确/摘要双路径及耗时裁剪规则上逐项对应。Rust 用 `Option<context::Context>` 表达 Go 的 nil source，用 `Option<Arc<_>>` 表达可空指针，用元组表达 Go 命名多返回值，并以 `Arc` 明确共享身份。

重要类型差异包括：Go 的 `Int64`、`Duration` 是新定义类型，Rust 目前是 `i64` 和 `StdDuration` 的类型别名；Go 的空百分位极值是类型零值，Rust 用 `Option<T>` 明确“尚无样本”；Go 的 influxdata t-digest 通过逐点 `Add`/centroid merge，Rust 使用 `tdigest 1.0.0` 的 `merge_unsorted`/`merge_digests`，因此大样本分位只要求近似语义而非逐位相同。Rust 的格式化辅助函数手工复现 Go `time.Duration.String`，因为标准库 `StdDuration` 没有同样的字符串协议。

测试对应关系：`pkg/util/execdetails/util_3_aster_unit_test.rs` 覆盖 context 指针保留、继承不覆盖、RU drain 幂等结果、原子快照隔离、精确与 digest 百分位、merge 计数以及完整格式表；它由 `internal/util/lib.rs` 在 `#[cfg(test)]` 下挂载。Go 侧 `pkg/util/execdetails/execdetails_test.go` 覆盖 `LoadTiKVExecDetails` 十二字段及 `TestFormatDurationForExplain` 表格；Rust 的 `pkg/util/execdetails/execdetails_test.rs` 还从聚合 crate 边界验证快照和格式化输出。

## 扩展指南

- 新增上下文明细时，应同时更新全新初始化、缺失补齐、继承/读取语义，并明确“覆盖、复用还是仅补缺”；同步更新 `internal/util/lib.rs` 的 key/类型以及 `util_3_aster_unit_test.rs` 的 `Arc::ptr_eq`、空上下文和已有对象用例。
- 给 `util::ExecDetails` 增加原子字段时，必须同步扩展 `LoadTiKVExecDetails`，否则慢日志和 statement summary 会静默遗漏；对应更新 Rust 快照测试与 Go `execdetails_test.go` 对照字段。
- 修改 `Percentile` 阈值、压缩参数或分位定义时，应同时验证精确路径下标规则、精确与 digest 互相 merge、最小/最大值携带的原对象、`Size`/`Sum` 守恒以及近似误差范围。不要假设本文件与 `internal/group1/lib.rs` 的同名 `Percentile` 自动同步；若公共行为需要一致，两个实现及各自测试都要审查。
- 修改耗时格式时，应维护 Go 表格全部单位边界（1µs、10 倍单位、进位到 ms/s/min/h）及 Rust 两处同名实现的一致性；兼容风险主要是 EXPLAIN、慢日志和运行时统计文本变化。
- 性能敏感点是每次 digest 追加的合并分配、精确路径首次查询的排序、逐字段原子读取以及 metrics mutex。改动前应确认真实调用的是 `execdetails::util` 还是 group1 导出的根 API，避免优化错误实现。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标区域列出 `util.rs`、`util_3_aster_unit_test.rs`、Go 对照和相关 runtime 文件。使用了 `files --filter pkg/util/execdetails`、目标文件 `node`、精确 `query` 以及含全部关键入口的 `explore`；`explore` 核对了上述内部调用边和外部调用者。
- 完整阅读：`pkg/util/execdetails/util.rs`（511 行）、`pkg/util/execdetails/util.go`、`pkg/util/execdetails/Cargo.toml`、`pkg/util/execdetails/internal/util/Cargo.toml`、`pkg/util/execdetails/lib.rs`、`pkg/util/execdetails/internal/util/lib.rs`、`pkg/util/execdetails/util_3_aster_unit_test.rs`。
- 直接辅助证据：`pkg/util/execdetails/ruv2_metrics.rs`（生产 RU 同步语义）、`pkg/util/execdetails/execdetails_test.go`、`pkg/util/execdetails/execdetails_test.rs`，以及 `rg` 得到的 `slow_log.rs`、两个 statement summary 文件调用位置。
- 人工复核结论：该文件存在是为了集中 Go `util.go` 对应的上下文/快照/聚合/格式化工具；它由内部 util crate 实际编译并经顶层 `util` 模块再导出，安全扩展必须保持共享对象身份、原子快照字段集合、百分位边界和 Go 文本兼容性。
- 本任务是纯文档分析，按计划不运行 Cargo；最终结构以任务指定命令验证，要求且只允许上述 11 个固定二级章节。
