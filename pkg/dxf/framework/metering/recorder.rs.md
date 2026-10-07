# `pkg/dxf/framework/metering/recorder.rs`

## 文件定位

本文件实现 DXF（分布式执行框架）计量子系统中的单任务累计器 `Recorder`。它位于 `astersql-dxf-framework-metering` crate，模块入口 `pkg/dxf/framework/metering/lib.rs` 将其声明为公开 `recorder` 模块并再导出其公共 API。该 crate 通过 `pkg/dxf/framework/metering/Cargo.toml` 直接依赖本地 `astersql-objstore-recording` crate，并由后者提供 `AccessStats` 与 `Traffic` 两类原子计数结构。

在完整链路中，`pkg/dxf/framework/metering/metering.rs` 的 `RegisterRecorder` 根据 `TaskBase` 创建或取得共享的 `Arc<Recorder>`；`Meter::scrape_current_data` 周期性调用 `Recorder::curr_data`，再由 `Data::cal_meter_data_item` 相对上次快照计算要写出的正增量。因而本文件只负责“累计并快照”，不负责调度 flush、重试、持久化或注销策略。

## 核心职责

- 用 `task_id`、`keyspace` 和 `task_type` 固定标识一项 DXF 任务；见 `Recorder` 与 `Recorder::new`。
- 累加对象存储 GET/PUT 请求数、对象存储读写字节以及集群读写字节；对象存储统计由 `Recorder::MergeObjStoreAccess` 批量合并，集群流量由 `IncClusterReadBytes`、`IncClusterWriteBytes` 逐次增加。
- 用 `Recorder::curr_data` 把六个独立原子计数读取为 `DataValues`，并连同任务元数据构造累计型 `Data` 快照。
- 向同 crate 的 `Meter` 暴露 `task_id_for_meter`，使 recorder 可按任务 ID 注册到 `MeterState::recorders`。

本文件不计算相邻快照差值。增量筛选和字段组装在 `pkg/dxf/framework/metering/data.rs` 的 `Data::cal_meter_data_item` 中完成。

## 主要符号

- `pub struct Recorder`：公开的单任务计量记录器。元数据字段为私有的 `task_id: i64`、`keyspace: String`、`task_type: String`；累计字段为私有的 `obj_store_access: AccessStats` 和 `cluster_traffic: Traffic`。`Default` 产生零 ID、空字符串和全零计数，供禁用计量的兼容路径使用。
- `pub fn Recorder::new(task_id, keyspace, task_type) -> Self`：保存任务元数据并保持所有计数为零。两个字符串参数接受 `Into<String>`。
- `pub fn Recorder::MergeObjStoreAccess(&self, other: &AccessStats)`：调用 `AccessStats::merge`，将传入统计当前值逐字段累加到 recorder，而不是持有 `other` 或替换已有值。
- `pub fn Recorder::IncClusterReadBytes(&self, n: u64)` / `IncClusterWriteBytes`：分别以 `Ordering::Relaxed` 对 `Traffic` 的读、写原子计数执行 `fetch_add`。
- `pub(crate) fn Recorder::curr_data(&self) -> Data`：读取六项累计值并构造快照；可见性限制为 crate 内。
- `pub(crate) fn Recorder::task_id_for_meter(&self) -> i64`：供 `Meter::get_or_register_recorder` 取得哈希表键。
- `record_obj_store_get`、`record_obj_store_put`、`record_obj_store_read`、`record_obj_store_write`：仅在 `cfg(test)` 下编译的 crate 内测试辅助方法，不属于生产 API。测试逻辑保存在独立的 `pkg/dxf/framework/metering/recorder_test.rs`，没有嵌入生产源文件。

文件没有模块级常量、trait、枚举、异步函数或 feature 条件；唯一条件编译项是上述四个测试辅助方法。

## 执行流程

1. `RegisterRecorder` 从 `TaskBase.ID`、`Keyspace` 和 `Type` 调用 `Recorder::new`。`Meter::get_or_register_recorder` 通过 `task_id_for_meter` 查表：已有同 ID recorder 时复用原 `Arc`，否则插入新对象。
2. 执行路径持有 `Arc<Recorder>` 并累计资源量。例如 `ingestCollector::Processed` 调用 `IncClusterWriteBytes`；冲突处理的 `LazyRefreshedSnapshot::BatchGet` 通过 `ResolutionMeterTraffic` 调用 `IncClusterReadBytes`；`Deleter::deleteBufferedKeys` 调用 `IncClusterWriteBytes`；`ConflictResolutionStepExecutor::RunSubtask` 结束对象存储操作后调用 `MergeObjStoreAccess`。
3. `Meter::scrape_current_data` 先在状态锁内克隆每个 recorder 的 `Arc`，释放该锁后逐个调用 `curr_data`。`curr_data` 分别读取请求数、对象存储流量和集群流量，形成累计快照。
4. `Meter::calculate_data_items` 把当前快照交给 `Data::cal_meter_data_item`，与 `last_flushed_data` 比较并只生成正增量。注销 recorder 时，`Meter::after_flush` 还会再次取快照；仅当最后快照已对齐才移除 recorder。

## 数据与状态

`task_id` 是 `MeterState::recorders` 和 `last_flushed_data` 的键；同一个 `Meter` 中相同任务 ID 会共享既有 recorder。因此调用方不能指望用相同 ID 再次注册来更新 `keyspace` 或 `task_type`：复用路径保留第一次创建对象中的元数据。

六项计数均是从零开始、仅做累加的 `u64`：对象存储 GET、PUT、读字节、写字节，集群读字节、写字节。`curr_data` 返回值拥有元数据字符串副本和计数值副本，后续计数变化不会修改已经生成的 `Data`。`Data` 再负责与上一快照比较；`Recorder` 自身不保存“已 flush”位置，也没有重置操作。

`MergeObjStoreAccess` 合并的是调用时对 `other` 各原子字段读取到的值。这是累计快照相加语义；若同一份累计型 `AccessStats` 被重复完整合并，其已有计数也会被重复计入，调用方应只在每段统计生命周期结束时合并一次。

## 依赖与调用关系

上游主要关系如下：

- `pkg/dxf/framework/metering/metering.rs::RegisterRecorder` → `Recorder::new` → `Meter::get_or_register_recorder`。
- `Meter::get_or_register_recorder` → `Recorder::task_id_for_meter`，并返回共享的 `Arc<Recorder>`。
- `Meter::scrape_current_data`、`Meter::after_flush` → `Recorder::curr_data`。
- `pkg/dxf/importinto/task_executor.rs::ingestCollector::Processed` → `IncClusterWriteBytes`，以已处理的非负字节数记录 ingest 集群写流量。
- `pkg/dxf/importinto/conflict_resolution.rs::ResolutionMeterTraffic` 把冲突处理的 `TrafficRecorder` 接口调用转发给读写累加方法；下层 `conflictedkv/handler.rs::BatchGet` 统计键和值长度，`conflictedkv/deleter.rs::deleteBufferedKeys` 统计被删键长度。
- `pkg/dxf/importinto/conflict_resolution.rs::ConflictResolutionStepExecutor::RunSubtask` → `MergeObjStoreAccess`，合并对象存储包装层收集的 `AccessStats`。

下游依赖为 `crate::data::{Data, DataValues}`、`recording::{AccessStats, Traffic}` 与标准库 `std::sync::atomic::Ordering`。`Cargo.toml` 中的 `recording` 指向 `../../../objstore/recording`；`nextgen` feature 只转发给 `kerneltype/nextgen`，本文件没有自己的 feature 分支。

## 错误处理与边界

本文件的所有方法都不返回 `Result`，也不执行 I/O；因此没有本地错误传播。对象存储、集群操作或 writer 失败由调用层处理，recorder 只接收调用方决定计入的数值。

- `MergeObjStoreAccess` 要求有效的 `&AccessStats`，Rust 类型系统排除了 Go 中的空指针参数；它不做去重，也不验证该统计是否已合并过。
- 增量方法接受 `u64`，不表达负数。已知调用点 `ingestCollector::Processed` 明确使用 `bytes.max(0) as u64`，避免把负的 `i64` 转成巨大计数。
- `Default` recorder 的任务 ID 为 `0` 且字符串为空。`RegisterRecorder` 仅在 Classic 模式或未安装全局 Meter 时返回这种空 recorder；它仍会累计，但不进入实际 Meter 的 flush 表。
- `curr_data` 不保证六项字段来自同一个逻辑时刻；并发更新可能出现在相邻字段读取之间。这对累计计量的周期快照是既定边界，不应将其当作事务一致的审计记录。
- 原子计数没有饱和、回退或重置逻辑。扩展调用方应避免重复累计和超出 `u64` 表达范围。

## 并发与资源生命周期

`Recorder` 的可变统计全部封装在 `AtomicU64` 中，公开累计方法只借用 `&self`，因此同一实例可由多个持有 `Arc<Recorder>` 的执行路径并发更新。所有加法和读取都采用 `Ordering::Relaxed`：这保证单个计数器操作的原子性，但不建立与业务数据之间的 happens-before 关系，也不提供多字段一致快照。`pkg/objstore/recording/recording.rs` 对 `Requests::snapshot` 和 `AccessStats::merge` 同样明确采用逐字段原子读取/累加。

recorder 不拥有线程、任务、通道、锁、文件句柄或 writer，也无需显式关闭。生命周期由 `MeterState` 中的 `Arc<Recorder>` 管理：注册后可被执行器克隆；`UnregisterRecorder` 只设置待注销标记；最后一次快照与当前累计值对齐后，`Meter::after_flush` 才从表中移除它。若外部仍持有 `Arc`，对象会继续存在并可累计，但已不再被该 `Meter` 抓取。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/dxf/framework/metering/recorder.go`，字段和生产方法保持一一对应：Go `Recorder` 的 `taskID/keyspace/taskType/objStoreAccess/clusterTraffic` 对应 Rust 同名语义字段；`MergeObjStoreAccess`、`IncClusterReadBytes`、`IncClusterWriteBytes` 和私有 `currData` 分别对应 Rust 方法。

关键实现语义也一致：两侧均以原子加法累计，以逐字段原子读取构建包含六项值的累计快照。`pkg/dxf/framework/metering/recorder_test.go::TestRecorder` 与 Rust 的 `recorder_test.rs::test_recorder` 使用相同的 `1/"ks"/"tt"` 元数据以及 `100、200、11、22、300、400` 六项输入，断言相同快照。

Rust 为适配语言和模块边界增加了三点结构差异：显式 `Recorder::new` 构造拥有的字符串；`task_id_for_meter` 代替 Go 同包代码直接读取私有字段；四个 `cfg(test)` 辅助方法代替 Rust 测试对私有 `AccessStats` 字段的直接访问。它们不改变生产计量模型。Rust 引用还排除了 Go 指针参数为 `nil` 的情况。

## 扩展指南

新增计量维度时，不应只修改本文件。至少需要：

1. 在 `Recorder` 中增加适合并发累计的状态，并提供语义明确的累计入口；在 `curr_data` 中把它映射到 `DataValues`。
2. 同步修改 `pkg/dxf/framework/metering/data.rs` 的 `DataValues`、字段名和增量组装，否则新计数不会进入 `MeterItem`。
3. 若 Go 版本已有对应能力，保持字段、累计时机和边界一致，并同步核对 `recorder.go`、`data.go`；不要仅为 Rust 引入不同的计量口径。
4. 在独立的 `pkg/dxf/framework/metering/recorder_test.rs` 扩展快照断言，并视端到端行为补充 `metering_test.rs` 或 `migration_aster_unit_test.rs`。不要把测试模块内嵌到 `recorder.rs`。
5. 若新增生产调用点，确认“成功前计数还是成功后计数”、重试是否会重复计数，以及输入的有符号值如何处理。对象存储累计尤其要确认传入的是本段增量还是全生命周期快照。

兼容性风险主要是改变现有字段名、任务元数据或注册复用语义；正确性风险主要是重复合并、遗漏某个执行分支或误把负数转换为 `u64`；性能风险主要是热路径增加原子操作。若要求跨字段一致性，不能简单加强单个原子的内存序，需重新设计快照同步方案并评估 flush 热点。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；目标目录中的 `recorder.rs`、`metering.rs`、`data.rs`、对应 Go 文件和测试均在索引内。
- RustCodeGraph 源码/符号查询：`recorder.rs` 的 `Recorder`、`new`、`MergeObjStoreAccess`、`IncClusterReadBytes`、`IncClusterWriteBytes`、`curr_data`、`task_id_for_meter`；`metering.rs` 的 `RegisterRecorder`、`Meter::get_or_register_recorder`、`scrape_current_data`、`after_flush`；`data.rs` 的 `Data`、`DataValues`、`cal_meter_data_item`。
- RustCodeGraph 调用证据：`task_executor.rs::ingestCollector::Processed`、`conflict_resolution.rs::ResolutionMeterTraffic` 与 `ConflictResolutionStepExecutor::RunSubtask`、`conflictedkv/handler.rs::BatchGet`、`conflictedkv/deleter.rs::deleteBufferedKeys`。
- crate 与依赖证据：`pkg/dxf/framework/metering/Cargo.toml`、`pkg/dxf/framework/metering/lib.rs`、`pkg/objstore/recording/recording.rs`。
- Go 对照与测试证据：`pkg/dxf/framework/metering/recorder.go`、`recorder_test.go`、`recorder_test.rs`、`migration_aster_unit_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务规定的 `test` 加 `rg -c` 命令确认目标文件存在且恰有 11 个固定二级章节。
