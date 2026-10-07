# `pkg/dxf/framework/metering/data.rs`

## 文件定位

[`data.rs`](data.rs) 是 `astersql-dxf-framework-metering` crate 的计量数据模型层。模块入口 [`lib.rs`](lib.rs) 以 `pub mod data` 装载并整体再导出它；[`Cargo.toml`](Cargo.toml) 表明该 crate 属于 Go 包 `pkg/dxf/framework/metering` 的 Rust 移植，并提供与 Go `nextgen` 构建标签对应的 feature。文件本身不采集资源、不调度 flush、也不执行 I/O，而是承接两个边界：把 [`recorder.rs`](recorder.rs) 生成的累计快照表示为 `Data`，以及把当前快照与上次 flush 快照转换为 writer 接受的 `MeterItem`。

在完整链路中，`Recorder::curr_data` 构造 `Data`，`Meter::scrape_current_data` 收集各任务快照，`Meter::calculate_data_items` 调用 `Data::cal_meter_data_item`，随后 `Meter::flush` 把生成的条目交给 `write_meter_data`。因此本文件位于“并发计数采集”和“带时间戳的持久化/重试”之间，是快照差分与载荷模式的边界。

## 核心职责

1. 定义六个累计资源计数及其载荷字段名：对象存储 GET/PUT 次数、对象存储读写字节、集群读写字节（`DataValues` 与 `*_FIELD` 常量）。
2. 用 `Data` 将累计计数和任务元信息 `task_id`、`keyspace`、`task_type` 绑定为一次 recorder 快照。
3. 由 `Data::equals` 和 `Data::cal_meter_data_item` 计算相邻快照的增量；完全无变化时不产生条目，有变化时只写入非零差值，同时始终使用当前快照的任务元信息生成基础字段。
4. 用 `MeterValue`/`MeterItem` 提供 Rust 侧对 Go `map[string]any` 的受限表示，使 writer 只接收字符串、`i64` 和 `u64` 三类值。
5. 用 `Display for Data` 生成日志摘要，并用私有 `byte_size` 对四个字节计数进行与 Go `docker/go-units.BytesSize` 对齐的二进制单位格式化。
6. 导出 `RowCountField` 等六个任务摘要字段名，供与 Go 公共契约对齐。当前 Rust 生产搜索中这些常量没有调用者；Go 的 [`handle.go`](../handle/handle.go) 会把它们加入一次性任务计量条目，而 Rust [`handle.rs`](../handle/handle.rs) 目前使用自己定义的 `MeterItem`/`MeterValue` 和字符串键，不能把两套类型视为已经接线。

## 主要符号

- `MeterValue::{String, I64, U64}`：载荷值的封闭枚举。四个 `From` 实现允许从 `&str`、`String`、`i64`、`u64` 构造值；没有浮点、布尔或嵌套对象变体。
- `MeterItem = HashMap<String, MeterValue>`：单条计量记录。它只描述字段集合，不携带 timestamp、writer UUID 或重试次数；这些属于 [`metering.rs`](metering.rs) 的 `MeteringData`/`WriteFailData`。
- `DataValues`：六个公开 `u64` 累计计数的值对象，派生 `Default`、`Clone`、`PartialEq`/`Eq`。公开字段使 recorder 和测试可一次性构造快照。
- `Data`：持有私有 `values` 和私有任务元信息。`new` 是完整构造入口；`values`、`task_id`、`keyspace`、`task_type` 提供只读访问，不提供就地修改接口。
- `Data::equals(&Data) -> bool`：只比较 `DataValues`，故任务 ID、keyspace 或任务类型不同但计数相同仍返回 `true`。这是 flush 后判断是否仍有未上报资源变化的语义，不是完整结构相等；派生的 `PartialEq`/`Eq` 才会比较全部字段。
- `Data::cal_meter_data_item(&Data) -> Option<MeterItem>`：以 `self` 为当前快照、参数为上一快照。计数完全相同返回 `None`；否则创建基础字段并逐项加入正的无符号差值。
- `insert_positive_delta`：统一执行 `current.wrapping_sub(previous)`，结果非零才写入。该函数刻意保留 Go `uint64` 下溢回绕语义。
- `byte_size` 与 `Display for Data`：前者选择 `B` 到 `YiB` 的 1024 进制单位并保留约四位有效数字，后者输出任务身份、请求数和格式化后的字节计数；这是日志展示，不参与上报数值。
- `GetBaseMeterItem`：构造固定字段 `version="1"`、`source_name="dxf"`、`task_id`、`cluster_id`、`task_type`。其中 `cluster_id` 按 Go nextgen 约定承载 keyspace。
- `RowCountField`、`DataKVBytesField`、`IndexKVBytesField`、`RequiredSlotsField`、`MaxNodeCountField`、`DurationSecondsField`：与 Go 导出名保持一致的任务完成摘要键；它们不参与 `DataValues` 的周期性差分。

## 执行流程

周期性 flush 的数据流如下：

1. [`recorder.rs`](recorder.rs) 的 `Recorder` 用原子计数累计对象存储访问与集群流量；`Recorder::curr_data` 以 relaxed load 读取六个计数并调用 `Data::new`。
2. [`metering.rs`](metering.rs) 的 `Meter::scrape_current_data` 以 `task_id` 为键收集所有 recorder 的当前 `Data`。
3. `Meter::calculate_data_items` 从 `last_flushed_data` 查找同一任务的上一快照；首次出现的任务使用 `Data::default()`，然后调用 `current.cal_meter_data_item(&previous)`。
4. `cal_meter_data_item` 先调用 `equals`。六个计数都相同就返回 `None`，避免仅因元信息差异产生空载荷。
5. 有任一计数不同时，函数用当前快照的元信息调用 `GetBaseMeterItem`，再对六个计数逐个调用 `insert_positive_delta`。差值为零的字段省略，非零字段以 `MeterValue::U64` 加入。
6. `Meter::flush` 收集所有 `Some(item)` 并写入。写入成功或失败后都会调用 `after_flush` 推进常规快照；失败时原始 `items` 另存到 `pending_retry_data`，以原 timestamp 重试，所以差分不会在下一轮重复生成。
7. 被注销的 recorder 只有在 `after_flush` 发现最新快照与 recorder 当前值按 `Data::equals` 相等时才移除；日志使用 `Display` 打印最终快照。

`GetBaseMeterItem` 也对应 Go 中构造一次性任务摘要条目的公共入口，但当前 Rust `handle.rs` 尚未直接复用本文件的类型与常量，属于另一条迁移接线边界。

## 数据与状态

`DataValues` 的六个 `u64` 被当作单调累计计数器快照；本文件不持有原子量，也不负责增加计数。`Data` 是拥有所有权的普通值，字符串元信息在 `new` 中通过 `Into<String>` 固化，适合克隆后放入 `MeterState::last_flushed_data`。默认值是全部计数为零、任务 ID 为 0、字符串为空，因此首次 flush 可自然地相对零快照计算总累计量。

相等性存在两个层次：派生的 `Data == Data` 比较所有字段，而业务方法 `Data::equals` 只比较累计计数。后者被 `cal_meter_data_item` 和 `Meter::after_flush` 使用，确保元信息变化本身既不产生计量条目，也不阻碍已注销 recorder 的清理。生成条目时则始终采用当前 `self` 的元信息；传入的 previous 元信息不会进入结果。

`MeterItem` 是无顺序保证的 `HashMap`。消费者应按键读取，不能依赖迭代或序列化顺序。六个周期计数值保留为 `U64`，任务 ID 为 `I64`，其余基础字段为 `String`；这种类型区别由测试明确断言。

## 依赖与调用关系

- 上游数据源：`Recorder::curr_data` 是生产路径中 `Data::new`/`DataValues` 的直接构造者；它从 `recording::AccessStats` 和 `recording::Traffic` 的原子计数读取当前值。
- 核心调用者：`Meter::calculate_data_items` 调用 `Data::cal_meter_data_item`；`Meter::after_flush` 调用 `Data::equals`；注销完成日志通过 `Display` 使用 `Data`。
- 下游消费者：`Meter::write_meter_data` 把 `Vec<MeterItem>` 包装进 `MeteringData` 并交给 `MeteringWriter`。本文件不知道 writer 类型、超时、UUID、timestamp 或重试策略。
- crate 边界：[`Cargo.toml`](Cargo.toml) 将库入口设为 `lib.rs`、关闭 doctest，并声明 `nextgen = ["kerneltype/nextgen"]`。本文件自身只直接依赖 Rust 标准库；同 crate 的 recorder、writer、指标和协议依赖由其他模块使用。
- Go 对照：[`data.go`](data.go) 是逐符号语义来源；[`metering.go`](metering.go) 提供同样的 scrape、差分、flush 和失败重试位置；[`handle.go`](../handle/handle.go) 展示六个任务摘要常量在 Go 侧的实际调用点。
- RustCodeGraph 的精确文件查询识别出 `data.rs` 的 19 个符号；对通用名称的全库调用图存在大量同名噪声，因此调用关系又用 `metering.rs`、`recorder.rs` 的文件限定查询和引用搜索收窄确认。

## 错误处理与边界

本文件的公共计算 API 不返回 `Result`：构造和差分均为内存操作。`cal_meter_data_item` 的 `None` 只表示六个累计值没有变化，不表示错误。writer 错误由 `Meter::flush`/`retry_write` 处理，不会反向改变本文件的快照差分规则。

重要边界包括：

- 计数倒退时不饱和到零，也不报错。`wrapping_sub` 会产生与 Go 无符号减法相同的大正数，例如 `4 - 5 == u64::MAX`；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 专门锁定这一兼容行为。调用者应维护累计计数单调性，不能把 recorder 重置后的值与旧快照直接配对，除非接受该回绕载荷。
- 某些字段不变时被省略；若至少一个字段变化，返回的条目仍包含全部五个基础字段。完全相同才返回 `None`。
- `equals` 忽略所有元信息。若同一 task ID 被错误地复用且只改变 keyspace/type、计数不变，本层不会发出更正条目。
- `byte_size` 内部对 Rust 科学计数格式的 `split_once` 和指数解析使用 `expect`。这些断言依赖标准库格式契约，输入范围仅为 `u64` 转换后的有限非负 `f64`；业务输入不会形成 NaN 或无穷大。
- `MeterValue` 不接受任意 JSON 值。新增载荷字段前必须确认其类型可由现有三种变体表达，或同步扩展 writer 的序列化逻辑和测试。

## 并发与资源生命周期

`Data`、`DataValues`、`MeterItem` 本身没有锁、原子、线程或 I/O 资源；它们是抓取时创建、在内存中克隆和移动的快照值。并发保证来自上下游：`Recorder` 以 relaxed 原子操作累计/读取计数，`Meter` 用互斥状态保存 recorder 表、`last_flushed_data` 和失败载荷。

一次 `curr_data` 会分别读取六个原子，因此不是跨字段的事务性瞬时快照；各字段可能来自略有差异的时点。设计依赖累计计数和周期差分，允许这种最终一致视图。`calculate_data_items` 在持有 Meter 状态锁时读取上一快照并计算条目；`after_flush` 再替换整张快照表。writer 调用不属于 `Data` 生命周期，失败条目会被 `Meter` 克隆后独立保留重试。

注销也不是立即释放：`unregister_recorder` 只做标记，最终 flush 后 `after_flush` 再用 `Data::equals` 确认计数对齐并移除 recorder 与快照。扩展 `DataValues` 时必须同时更新该相等性所依赖的派生比较、快照构造和差分，否则可能过早清理或永远无法清理。

## 与 Go 版本的对应关系

Rust 的 `Data`/`DataValues` 对应 [`data.go`](data.go) 的 `Data`/`dataValues`；私有元信息、六个累计 `uint64` 字段、只比较计数的 `equals`、按字段省略零差值的 `calMeterDataItem`、字符串展示和 `GetBaseMeterItem` 均保持 Go 行为。Rust 用显式的 `MeterValue` 替代 Go 的 `any`，用 `Option<MeterItem>` 表示 Go 的 `nil map`，并用访问器替代同包内直接访问私有字段。

`insert_positive_delta` 显式使用 `wrapping_sub`，是对 Go `uint64` 下溢语义的必要移植；不能以“计数应单调”为由换成普通 debug 下溢、`checked_sub` 或 `saturating_sub`。`byte_size` 是本地实现，但测试覆盖 `1KiB`、`1.5KiB`、`976.6KiB` 和 `u64::MAX -> 16EiB`，用于对齐 Go `go-units.BytesSize` 的可见日志格式。

基础字段中 Go 使用包常量 `category`，其值为 `dxf`；Rust 当前在 `GetBaseMeterItem` 中直接写入 `"dxf"`，而 `metering.rs` 的外层 `MeteringData.category` 也使用 `CATEGORY`。两者在当前测试中一致，但未来修改类别名时需同步，避免内外两层字段漂移。

六个 CamelCase 摘要常量保持 Go 导出 API 名称，并由 `lib.rs` 的 lint allow 接受。Go `handle.go` 已用这些常量构造任务完成条目；Rust `handle.rs` 当前有独立的 `BTreeMap<String, handle::MeterValue>` 实现，类型和字段接线尚未统一。这是当前迁移状态，而不是本文件已覆盖的周期差分流程。

## 扩展指南

新增一个周期累计指标时，应最小且成组地修改：

1. 在 `data.rs` 增加字段名常量和 `DataValues` 字段，并在 `cal_meter_data_item` 中调用 `insert_positive_delta`；确认 `Display` 是否需要展示。
2. 在 [`recorder.rs`](recorder.rs) 增加对应累计来源，并在 `Recorder::curr_data` 填入快照。若来源并非无符号单调计数，不应直接套用现有回绕差分规则。
3. 扩展独立测试 [`data_test.rs`](data_test.rs) 的相等/全字段差分/单字段省略用例，并扩展 [`recorder_test.rs`](recorder_test.rs)；Go 移植契约变化还应更新 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 和同路径 Go 测试。
4. 检查 [`metering_test.rs`](metering_test.rs) 的 flush、失败保留和重试断言，确保新字段经过完整 writer 链路且不会重复上报。

新增基础元信息字段时，应修改 `GetBaseMeterItem` 和 writer 序列化测试，并明确它是否参与“有变化”的判断；当前约定是元信息不参与 `equals`。新增 `MeterValue` 变体时，必须同步所有将条目转成 JSON/SDK 载荷的 match 分支，否则会造成编译失败或序列化不完整。

若要让 Rust 一次性任务摘要真正复用本文件的 `RowCountField` 等常量，需要先解决 [`handle.rs`](../handle/handle.rs) 与本 crate 两套 `MeterItem`/`MeterValue` 的边界，而不是只替换字符串字面量。应评估数值符号差异（handle 当前使用 `i64`，周期计数使用 `u64`）、模块依赖方向和序列化兼容性，并在 handle 的独立测试中验证。性能上，新增字段会增加每个任务每轮差分和 HashMap 分配；通常是常数开销，但高任务数场景仍应避免不必要的字符串复制或无变化条目。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、`data.rs` 有 19 个符号；通过 `node --file` 读取了 [`data.rs`](data.rs)、[`lib.rs`](lib.rs)、[`recorder.rs`](recorder.rs)、[`metering.rs`](metering.rs)、[`data_test.rs`](data_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的相关区段，并用 `query cal_meter_data_item` 确认实现与独立测试入口。
- crate 与模块：[`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs) 证明 crate 名称、入口、feature、Go 包映射、模块再导出及测试分离方式。
- Go 语义：[`data.go`](data.go) 与 [`data_test.go`](data_test.go) 证明六个计数、忽略元信息的相等性、nil/增量字段、基础字段和展示来源；[`handle.go`](../handle/handle.go) 证明任务摘要常量的 Go 调用点。
- Rust 测试：[`data_test.rs`](data_test.rs) 覆盖每个计数的相等性和差异、任务 ID 被忽略、全字段差分、单字段差分及无变化 `None`；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 额外覆盖 Go 无符号回绕、基础字段值/类型、recorder 快照以及字节展示边界。
- 应用调用边：[`recorder.rs`](recorder.rs) 的 `curr_data` → `Data::new`；[`metering.rs`](metering.rs) 的 `scrape_current_data` → `calculate_data_items` → `Data::cal_meter_data_item` → `flush`/`write_meter_data`，以及 `after_flush` → `Data::equals`。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前仅执行任务规定的 11 章节结构检查，并人工复核没有把 Rust handle 的独立实现误写成已接线能力。
