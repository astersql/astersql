# `br/pkg/summary/collector.rs`

## 文件定位

本文件是 Cargo 包 `astersql-br-pkg-summary` 的核心聚合实现，crate 根 `br/pkg/summary/lib.rs` 通过 `#[path = "collector.rs"] pub mod collector` 挂载它，并重新导出常量、字段类型、`LogCollector`、构造/初始化函数和格式化辅助函数。相邻的 `br/pkg/summary/summary.rs` 提供包级薄包装，业务调用通常经这些包装访问本文件维护的全局收集器，而不是直接操作具体类型 `logCollector`。

`br/pkg/summary/Cargo.toml` 将该 crate 声明为 library，并用 `package.metadata.porting.go-package = "br/pkg/summary"` 标明 Go 来源。目标文件直接对应 `br/pkg/summary/collector.go`，职责是为 BR 备份、恢复及相关工具聚合 range 成败、阶段耗时、整数计数和数据量，最终生成一条 success/failed summary 结构化日志。它不是备份或恢复执行器，也不拥有任务取消、重试或存储资源。

## 核心职责

- 定义摘要字段协议：`BackupUnit`、`RestoreUnit`、`TotalKV`、`TotalBytes`、`BackupDataSize`、`RestoreDataSize` 以及 checkpoint 跳过量等常量。
- 用 `LogCollector` trait 固化采集接口，用私有 `logCollector` 保存累计状态；`NewLogCollector` 允许注入 `LogFunc`，从而使摘要输出不绑定具体日志后端。
- 通过 `COLLECTOR: LazyLock<Mutex<Box<dyn LogCollector>>>` 提供进程级默认实例，`with_collector` / `with_collector_result` 为 `summary.rs` 的包级 API 串行访问该实例。
- 在 `Summary` 中选择成功或失败模板：统一输出 range 计数，失败时区分取消与真实错误，成功时格式化总耗时、数据量及平均速度。
- 提供轻量的 `Field` / `FieldValue` / `zap` 构造器和十进制 `units::HumanSize`，复现本模块实际用到的 Go zap 与 `docker/go-units` 行为子集。

本文件是完整可调用的移植实现，不是只声明接口的门面；但内置 `log` 模块是隔离的日志替身，默认全局日志回调为 no-op，并不等同于完整的 `pingcap/log` 后端。

## 主要符号

- `SummaryValue::{Duration, UInt64}`：对应 Go `CollectSuccessUnit` 的 `any` 参数仅有的两个有效分支。`Duration` 同时增加成功单元数与按名耗时，`UInt64` 只累计数据量，传入的 `unit_count` 在后者中不生效。
- `SummaryError = Arc<dyn StdError + Send + Sync>`：可在线程间共享的错误对象；`ContextCanceled` 是对照 `context.Canceled` 的专用哨兵类型。
- `Field { key, value }` 与 `FieldValue`：承载 `Int(i64)`、`Duration`、`String`、`Uint64`、`Error(String)` 五类摘要字段；`zap::{Int, Duration, String, Uint64, Error}` 是构造函数，而不是外部 zap crate。
- `LogFunc = Arc<dyn Fn(&str, &[Field]) + Send + Sync>`：可克隆、可跨线程共享的日志回调。回调收到的字段切片只在调用期间有效，如需异步保存必须自行复制。
- `LogCollector: Send`：公开采集接口，包含设置单位、五类累计操作、状态/计时操作、`Summary` 与直接 `Log`。方法保留 Go 风格命名以维持移植契约。
- `logCollector`：私有具体实现。其 `success_costs`、`success_data`、`failure_reasons`、`durations`、`ints`、`uints` 分别保存不同语义的数据，不能因键和值类型相似而合并。
- `COLLECTOR`、`with_collector`、`with_collector_result`：全局实例及其唯一正常访问门；前者惰性创建，后二者在闭包完整执行期间持有全局互斥锁。
- `InitCollector`：重建全局实例。`has_log_file == true` 且 `log::InitLogger` 成功时，注入同时调用 logger 回调和全局 `log::Info` 的回调；初始化失败则回退到默认日志函数。
- `NewLogCollector`：创建所有 map 为空、计数为零、`success_status` 为 false、`start_time` 为当前时刻的 boxed trait object。
- `log_key_for`：将字段名中的所有空格替换为 `-`；不做大小写、标点或重复连字符规范化。
- `is_context_canceled`：沿 `StdError::source` 链按具体类型识别 `ContextCanceled`，不会仅凭显示文本为 `"context canceled"` 判定取消。

## 执行流程

1. crate 首次访问 `COLLECTOR` 时，`LazyLock` 调用 `NewLogCollector(default_log_func())`。业务入口通常来自 `summary.rs`，例如 `CollectDuration` 进入 `with_collector` 后再调用 trait 方法。
2. 采集阶段按键累计：`CollectDuration`、`CollectInt`、`CollectUInt` 使用 map entry 加法；`CollectSuccessUnit(Duration)` 增加 `success_unit_count` 并写 `success_costs`，`CollectSuccessUnit(UInt64)` 写 `success_data`；`CollectFailureUnit` 仅在 unit 名首次出现时保存错误并增加失败数。
3. `SetSuccessStatus` 设置最终模板选择标志。包级同名函数还会在 `summary.rs` 中同步更新 `LAST_STATUS`，因此直接调用 trait 方法与调用包级函数在 `Succeed()` 可见性上并不完全等价。
4. `Summary(name)` 先生成固定的 `total-ranges`、`ranges-succeed`、`ranges-failed`，再追加 `durations`、`ints`、`uints`。动态键都经 `log_key_for` 处理。
5. 若存在失败原因或 `success_status == false`，进入失败分支。普通错误追加 `unit-name` 与 `error`；`ContextCanceled` 只增加 `cancel-unit`，由全局 `log::Info("units canceled", ...)` 单独记录。随后调用注入回调输出 `"{name} failed summary"`。
6. 否则进入成功分支，追加 `total-take`。`TotalBytes` 转成 `total-kv-size` 和 `average-speed`，`SkippedBytesByCheckpoint` 转成人类可读字符串，备份/恢复压缩量使用各自规范化键，其余 `success_data` 原样输出为 `Uint64`，最后输出 `"{name} success summary"`。
7. 两个分支结束时都清空 `durations`、`ints`、`success_costs`、`failure_reasons`。其他字段保持不变，因此同一 collector 的后续 `Summary` 是“部分延续”，不是全量新会话。

直接 `Log(msg, fields)` 不经过累计或模板选择，原样调用注入的 `LogFunc`。`NowDureTime` 返回从构造时 `Instant` 起的 elapsed；`AdjustStartTimeToEarlierTime` 把起点向前移动，使前置阶段计入后续总耗时。

## 数据与状态

`logCollector` 的状态可分为四组：

- 身份/控制：`unit`、`success_status`。当前实现保存 `unit` 但 `Summary` 不读取它，摘要名称来自 `Summary(name)` 参数。
- 单元结果：`success_unit_count`、`failure_unit_count`、`success_costs`、`failure_reasons`。失败数按唯一 unit 名计数；成功数仅由 `SummaryValue::Duration` 分支增加。`success_costs` 会累计和清空，但当前 `Summary` 不把它写入字段。
- 通用指标：`success_data`、`durations`、`ints`、`uints`。同名值使用 Rust 原生加法；代码没有饱和、checked 或显式 wrapping 处理，极端溢出行为取决于构建时整数溢出检查配置。
- 生命周期：`start_time` 与 `log`。`start_time` 在构造时设置，此后只可向前调整；`Summary` 不重置它。`log` 是共享回调，直到 collector 被替换或释放。

值得特别注意的是清理集合不对称：`Summary` 不清空 `success_data`、`uints`，不归零两个 unit count，不恢复 `success_status`，也不重置 `unit`/`start_time`。这与 `collector.go` 的 defer 清理集合一致，扩展时不能假设一次 `Summary` 等价于 `NewLogCollector`。

字段输出来自 `HashMap` 遍历，除三个固定 range 字段的前置顺序外，动态字段顺序不稳定。测试和下游日志解析应按 key/值判断，不应依赖完整字段序列。

## 依赖与调用关系

crate 边界由 `br/pkg/summary/Cargo.toml` 定义。目标文件使用 Rust 标准库的集合、错误、同步与计时设施；manifest 中的 `bytesize`、`tracing`、`astersql-errors`、`astersql-br-pkg-logutil` 并未在本文件的当前实现中直接引用，它们属于 crate 或迁移期依赖集合。

上游主链是：

`BR 业务模块 -> br/pkg/summary/summary.rs 包级函数 -> with_collector[_result] -> COLLECTOR -> LogCollector 实现`。

RustCodeGraph 将 `collector.rs` 标为被 13 个文件使用，并识别到直接业务证据，例如：

- `br/pkg/utils/misc.rs::SummaryFiles` 通过公开 API累计 `TotalKV`、`TotalBytes` 和 CF 文件计数。
- `br/pkg/restore/restorer.rs::WaitUntilFinish`/相关恢复流程采集失败、耗时与成功文件数。
- `br/pkg/restore/snap_client/import.rs::Import` 和 `tikv_sender.rs` 累计总 KV、总字节及 checkpoint 跳过量。
- `br/pkg/restore/log_client/import.rs::importKVFileForRegionOwned` 累计 `RegionInvolved`。
- `br/pkg/task/*.rs` 的任务收尾路径调用包级 `Summary`，将此前累计状态刷为最终日志。

下游依赖主要是注入的 `LogFunc`、标准库 `HashMap`/`Mutex`/`RwLock`/`Instant`，以及本文件自己的 `zap` 与 `units` 模块。不存在网络、磁盘、数据库事务或异步 runtime 调用。

## 错误处理与边界

- `CollectFailureUnit` 对同名失败采用 first-error-wins：后续错误既不覆盖原因，也不增加 `failure_unit_count`。不同 unit 可各保留一个错误。
- 取消判断是类型/错误链判断。与 `ContextCanceled` 文本相同但类型不同的错误仍作为普通失败记录；`br/pkg/summary/parity_test.rs` 明确覆盖这一边界。
- `InitCollector` 吞掉 `InitLogger` 的字符串错误并回退默认回调，保持初始化不中断，但调用方也无法获知日志后端降级。
- 全局 `Mutex` 或日志 `RwLock` 一旦 poison，代码通过 `expect(...)` panic；trait 方法本身没有 `Result`，累计与输出失败不能正常向调用者传播。
- 日志回调若 panic，会在全局调用路径持锁期间展开，并可能 poison collector mutex。实现没有 `catch_unwind`。
- 成功分支的平均速度用 `total_dure_time.as_secs_f64()` 作除数；极短时间可能产生无穷大，`HumanSize` 会按 `+Inf` 格式化。当前代码没有最小时间钳制。
- `BackupDataSize` / `RestoreDataSize` 内部保留了 Go 的 “Nothing to bakcup/restore” 条件与拼写，但该检查位于已经确认 `success_status == true` 的成功分支，现有控制流下其中的 `!success_status` 条件不可满足；文档不将它描述为当前可达行为。
- `AdjustStartTimeToEarlierTime` 使用 `Instant -= Duration`，对超出平台可表示范围的时长没有显式错误返回。

## 并发与资源生命周期

全局路径由 `COLLECTOR` 的 `Mutex` 串行化；`with_collector` 在 trait 调用（包括用户注入的日志回调）返回前一直持锁。因此同一进程的包级采集不会并发修改状态，但慢回调会阻塞所有摘要操作，回调若重入包级 summary API 会造成不可重入锁等待风险。

`NewLogCollector` 返回 `Box<dyn LogCollector>`，trait 只要求 `Send`，修改方法使用 `&mut self`。独立实例没有 Go `logCollector.mu` 那样的内部 mutex；要跨线程共享，调用方必须在外层增加同步。相比之下，Go `collector.go` 的每个具体实例都以 `sync.Mutex` 保护方法。这是实现机制差异，不影响通过本 crate 全局 API 的串行语义。

`SummaryError`、`LogFunc` 使用 `Arc` 管理共享所有权；map 清空或 collector 被替换时相应引用计数下降。`SetLogCollector` 在全局锁内替换 boxed 实例，旧实例在锁内离开作用域时释放。没有后台线程、channel、文件句柄或显式 shutdown；`LazyLock` 全局值随进程存活。

内置日志模块另有 `GLOBAL_LOG: LazyLock<RwLock<LogFunc>>`。`InitCollector(true)` 构造的回调可能连续走 logger 包装和全局 `Info`，应按源码视为双通道调用，注入后端时需验证是否会落到同一目标并产生重复记录。

## 与 Go 版本的对应关系

`br/pkg/summary/collector.go` 是最直接的语义基线：常量、`LogCollector` 方法集、状态字段、按键累加、失败去重、摘要分支、字段名规范化和部分清理集合均逐项对应。`br/pkg/summary/summary.go` 与 Rust `summary.rs` 都把包级函数转发到 collector，并另存最近成功状态。

主要实现差异如下：

- Go 的 `CollectSuccessUnit` 接受 `any` 并忽略未知动态类型；Rust 用 `SummaryValue` 封闭枚举，使调用期只能提供 `Duration` 或 `UInt64`。
- Go 使用 `error` 与 `errors.Cause(reason) == context.Canceled`；Rust 使用线程安全 trait object，并沿 source 链查找 `ContextCanceled` 具体类型。
- Go 使用 `zap.Field`、`pingcap/log` 和 `docker/go-units`；Rust 在文件内实现所需字段与格式化子集。`collector_test.rs` 对小数和大数补充了 `HumanSize` 精度回归。
- Go 每个 `logCollector` 内部有 `sync.Mutex`；Rust 把全局同步移到 `COLLECTOR` 外层，独立实例由 `&mut self` 保证单线程可变访问。
- Go 全局变量赋值本身未在 `SetLogCollector` 中加锁；Rust 替换全局 collector 时也走同一个 mutex。
- Go `time.Time.Add(-t)` 可表达向前调整；Rust 用 `Instant -= t`，类型与异常边界不同，但正常时长下语义一致。

`br/pkg/summary/collector_test.go::TestSumDurationInt` 与 Rust `collector_test.rs::test_sum_duration_int` 都验证同名 duration/int 的累加和七字段成功摘要。Rust `parity_test.rs::go_rust_public_contract_matches` 进一步验证失败去重、取消分流、同文案不同类型不视作取消、部分 map 重置、键格式化和 human-size 行为。

## 扩展指南

- 新增一种累计数据语义时，先判断它应进入现有 `durations`/`ints`/`uints`/`success_data`，还是必须扩展 `SummaryValue` 与 `LogCollector`。若改公开 trait，必须同步 `logCollector`、所有自定义实现、`lib.rs` 导出、`summary.rs` 包级包装和 Go 对照契约。
- 新增特殊字段展示规则应放在 `Summary` 的 success-data 分派中，并明确是否使用 `log_key_for`、是否 human-size、是否跨多次 `Summary` 保留。不要无意改变当前部分清理语义。
- 新增错误分类应扩展 `is_context_canceled` 附近的明确类型判定，避免用错误字符串分类；应在独立的 `parity_test.rs` 或 `collector_test.rs` 增加普通错误、包装错误与相似文本用例。
- 更换日志后端时优先保持 `LogFunc` 边界，并审查回调在持有 `COLLECTOR` 锁期间执行的阻塞、panic 和重入风险。若要释放锁后再输出，需要先设计状态快照，不能简单缩短锁作用域。
- 如需让每次 `Summary` 成为完全独立周期，应同时审查并有意决定是否重置 counts、`success_data`、`uints`、status、unit 和 start time；这会偏离当前 Go 行为，不能作为顺手清理。
- 测试必须继续放在独立文件：Go 对照核心场景在 `br/pkg/summary/collector_test.rs`，更全面的跨语言契约在 `br/pkg/summary/parity_test.rs`，crate 测试挂载位于 `br/pkg/summary/lib.rs`。不要把测试模块内嵌回 `collector.rs`。

兼容风险集中在日志 key/值类型、失败去重、清理集合和公开 Go 风格符号；性能风险集中在全局锁、锁内日志回调、map 增长和错误 `Arc` 的保留时间。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；目标区域被索引。
- RustCodeGraph `files --filter br/pkg/summary`：确认 `collector.rs`、`summary.rs`、crate 根及三个独立 Rust 测试文件的模块范围。
- RustCodeGraph `explore "br/pkg/summary/collector.rs LogCollector Field InitCollector NewLogCollector SetLogCollector CollectSuccessUnit Summary"`：确认目标文件 81 个符号、包级包装调用和 BR 恢复/工具调用边；图中目标文件被 13 个文件使用。
- RustCodeGraph `query LogCollector --kind struct`、`query NewLogCollector --kind function`、`query SetLogCollector --kind function`：消除 Go/Rust 同名符号歧义并定位 Rust 定义。
- RustCodeGraph `node --file br/pkg/summary/collector.rs`：逐段核验全部 581 行实现，重点复核 `LogCollector`、`logCollector`、`COLLECTOR`、`InitCollector`、`NewLogCollector`、`Summary`、`SetLogCollector` 和 `is_context_canceled`。
- RustCodeGraph `node` 读取 `br/pkg/summary/summary.rs`、`collector_test.rs`、`parity_test.rs`：核验包级转发、原子成功状态与测试覆盖。
- 直接读取 `br/pkg/summary/Cargo.toml`、`lib.rs`：核验 crate 边界、模块挂载、公开 re-export 和依赖声明。
- 直接读取 Go `br/pkg/summary/collector.go`、`summary.go`、`collector_test.go`、`main_test.go`：核验移植字段、方法、摘要分支、清理语义及原始测试意图。
- `rg` 搜索 `br/cmd/br`、`br/pkg/backup`、`br/pkg/restore`、`br/pkg/utils` 的 Rust 调用：抽样核验 `SummaryFiles`、恢复收尾、snap import/tikv sender 与 log client 的真实业务入口。
- 结构验证要求：目标文件必须存在，且固定的十一个二级标题各出现一次；本任务是纯文档分析，按任务约束不运行 Cargo。
