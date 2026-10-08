# `pkg/util/stmtsummary/v2/column.rs`

## 文件定位

本文件是 `astersql-util-stmtsummary-v2` crate 的“语句摘要记录到虚拟表行”适配层。crate 根 `pkg/util/stmtsummary/v2/lib.rs` 以 `mod column; pub use column::*;` 暴露这里的列名、工厂与辅助函数；`pkg/util/stmtsummary/v2/Cargo.toml` 声明该 crate 依赖 `chrono`、`chrono-tz`、旧版 statement-summary 兼容类型以及 plan codec。直接生产调用者是同 crate 的 `reader.rs`：`NewMemReader` 和 `NewHistoryReader` 根据请求列构造工厂，`buildRow`/`buildValues` 再把 `StmtRecord` 投影为查询结果。

它不采集、聚合或持久化统计信息；这些职责分别属于 `record.rs`、`stmtsummary.rs` 与 `logger.rs`。它只定义 128 个表列名及一一对应的 128 个取值工厂，把已经聚合好的 `StmtRecord` 字段转换为 INFORMATION_SCHEMA 风格的 `Datum`。

## 核心职责

1. 用 `ClusterTableInstanceColumnNameStr` 至 `StorageMPPStr` 固化 statement-summary 表的列名协议，并由 `columnFactoryMap` 将每个列名映射到取值函数。
2. 用 `makeColumnFactories` 按调用方给出的 `model::ColumnInfo` 顺序选择工厂，保证输出行与表定义的列序一致。
3. 用 `ColumnValue` 延迟表示 NULL、有符号/无符号整数、浮点、字符串、时间和已构造 Datum，再由 `ColumnValue::into_datum` 统一落到 `types::Datum`。
4. 在工厂中实现派生列：执行次数或提交次数分母的平均值、纳秒时长、时区化时间戳、backoff 汇总文本、空字符串到 NULL、plan 解码等。
5. 保持 `pkg/util/stmtsummary/v2/column.go` 的列语义，包括零分母、未知列、非 UTF-8 plan 字节及 plan 解码失败等边界。

## 主要符号

- `ColumnValue`：工厂的中间值枚举。`Null` 经 `types::Datum::default()` 表示 SQL NULL；`Int`、`Uint`、`Float`、`String`、`Time` 分别调用对应 Datum 构造器；`Datum` 用于已经需要精确保留底层表示的值。
- `impl From<Vec<u8>> for ColumnValue`：UTF-8 成功时生成字符串；失败时调用 `SetBytesAsString`，保留 Go `string` 可以承载任意字节的语义，主要服务 `PLAN` 列。
- `ColumnInfoContext` / `ColumnContext`：向列工厂只暴露实例地址和会话时区。`ColumnContext::new` 在 reader 构造阶段保存这两个只读值。
- `ColumnFactory`：函数指针类型 `fn(&dyn ColumnInfoContext, &StmtRecord) -> ColumnValue`；没有捕获环境，因此可复制并跨 reader 工作阶段传递。
- `columnFactoryMap`：进程级 `LazyLock<HashMap<...>>`，第一次使用时注册全部列工厂。字段直取和派生逻辑都集中在此。
- `makeColumnFactories`：按 `ColumnInfo.Name.O` 查表并保持输入顺序；缺失注册时 panic，避免静默产出错位或空列。
- `duration_nanos`、`timestamp_value`、`ToSystemTimeSeconds`：完成 Duration 纳秒钳制、Unix 秒到会话时区 timestamp 的转换，以及 Unix epoch 前时间按 Go `Time.Unix()` 的向下取整语义。
- `formatBackoffTypes`：空 map 返回 `None`；非空项按计数降序形成 `type:count` 逗号串。同计数项未增加名称次序保证。
- `avgInt`、`avgFloat`、`avgFloat4Uint`、`avgSumFloat`：分别处理整数、整数转浮点、无符号累计值转浮点及浮点累计值；只有 `count > 0` 才除法，否则返回零。
- `convertEmptyToNil`：空字符串转 `None`，用于 schema、digest、表名、索引名等应显示 SQL NULL 的字段。

## 执行流程

1. 表扫描创建 `MemReader` 或 `HistoryReader` 时，把所需 `model::ColumnInfo`、实例地址和会话时区传入（`reader.rs::NewMemReader`、`reader.rs::NewHistoryReader`）。
2. `makeColumnFactories` 依次读取每个 `ColumnInfo.Name.O`，在 `columnFactoryMap` 中解析为 `ColumnFactory`，所得 Vec 与请求列严格同序。
3. 内存路径经权限、digest、时间范围和淘汰行规则过滤后调用 `buildRow`；历史路径解析持久化 JSON 后调用 `buildValues`。两者都对一条 `StmtRecord` 逐工厂求值。
4. 工厂读取记录字段，必要时做派生计算：一般执行指标以 `ExecCount` 为分母；2PC 提交指标以 `CommitCount` 为分母；Duration 输出纳秒；时间列按 `ColumnInfoContext::getTimeLocation` 转换。
5. 内存路径立即调用 `into_datum`；历史 worker 先发送 `ColumnValue` 行，`HistoryReader::Rows` 收到后再转换为 `Datum`。最终每行的值数与所请求列数相同。
6. `PLAN` 工厂调用 `plancodec::DecodePlan`；成功结果按 Go 字符串字节语义转换，失败则记录到标准错误并返回空字节串对应的字符串值，不中止整行。

## 数据与状态

- 全局可变形态仅有 `columnFactoryMap` 的一次性延迟初始化；初始化完成后 map 和函数指针只读。
- `ColumnContext` 拥有 `instance_addr: String` 和可复制的 `chrono_tz::Tz`。它不持有 session、文件、连接、锁或记录所有权。
- 每次投影只借用 `StmtRecord`；字符串列按当前实现 clone/分配，`IndexNames` 还会 join。`formatBackoffTypes` 会复制 map 的键、排序并分配结果串。
- NULL 语义不是空字符串：`convertEmptyToNil` 和空 backoff map 生成 `ColumnValue::Null`；普通文本列（如 normalized SQL）即使为空也按各自工厂直接生成字符串。
- 数值类型需保持列协议：无符号最大值使用 `ColumnValue::Uint`，RocksDB/IA/affected-row 等无符号累计平均使用 `avgFloat4Uint`，避免先转 `i64` 溢出。
- 平均值分母是业务不变量：执行级指标使用 `ExecCount`，prewrite/commit/lock/latch/write/txn-retry 使用 `CommitCount`。`count <= 0` 均输出零。

## 依赖与调用关系

上游链路为 `reader.rs::{NewMemReader, NewHistoryReader}` → `makeColumnFactories`，以及 `reader.rs::{buildRow, buildValues}` → `ColumnFactory` → `ColumnValue::into_datum`。RustCodeGraph 的文件节点确认 `column.rs` 被 `record.rs`、`record_test.rs`、`reader_test.rs` 引用；原始调用搜索进一步确认生产投影入口在 `reader.rs`。

下游依赖包括：`StmtRecord`（`record.rs`）提供聚合字段；`model::ColumnInfo` 来自 `task-stmtsummary` 兼容层；`types::{Datum, Time}` 和 `mysql::TypeTimestamp` 由 crate 根转导旧版 statement-summary 类型；`plancodec::DecodePlan` 来自 `plancodec-dependency`；`chrono`/`chrono-tz` 处理时间范围和时区；标准库 `HashMap`、`LazyLock`、`Duration`、`SystemTime` 提供注册表与转换基础。

该文件不会回写 `StmtRecord`。reader 在调用前决定窗口时间、过滤权限/digest、是否包含淘汰汇总行以及历史解析并发；列层只投影传入记录，因此不能在这里添加影响筛选或聚合状态的逻辑。

## 错误处理与边界

- 未注册列：`makeColumnFactories` 通过 `unwrap_or_else` panic，消息点名遗漏列。这与 Go 的强失败一致，表定义新增列却未注册工厂属于编程错误。
- 除零：四个平均辅助函数均要求 `count > 0`，零或负计数返回数值零。
- Duration：`duration_nanos` 将超过 `i64::MAX` 的纳秒值钳制到 `i64::MAX`；这避免 Rust 转型环绕，但扩展时应继续确认与 Go `time.Duration` 可表示范围相容。
- 时间：`timestamp_value` 对 chrono 无法表示的 Unix 秒使用 `expect`；`SystemTime` 在 epoch 前有亚秒时向下取整到前一个整秒，测试覆盖 `-0.5s -> -1s`。
- plan：解码失败不向 reader 返回错误，而是写 stderr 并返回空字符串值；非 UTF-8 解码结果通过 bytes-as-string Datum 保留原字节、kind 和 collation。
- backoff：空集合为 NULL；只按次数降序，同次数时结果顺序受 HashMap 遍历/不稳定排序输入影响，不应把并列项的字典序当成契约。
- sample user：只取 `AuthUsers` 迭代遇到的第一个用户，集合多元素时具体用户不保证稳定；空集合转 NULL。

## 并发与资源生命周期

本文件自身不启动线程、不进行异步 I/O，也没有显式锁。`LazyLock` 保证 `columnFactoryMap` 在多线程首次访问时只初始化一次；之后 `ColumnFactory` 是无捕获函数指针，读取共享的 `StmtRecord` 和上下文，不修改它们。

生命周期由 reader 管理：`MemReader` 持有 `ColumnContext` 与工厂 Vec，并同步构建行；`HistoryReader` 把 context/factories 移入调度线程，parse worker 产生 `ColumnValue`，通道接收端再转 Datum。这里产生的 String、Vec 和 Datum 均按值随行转移和释放，不持有文件句柄、channel 或取消标志。新增工厂应继续保持纯读取，避免把锁、阻塞 I/O 或跨行缓存引入每列热路径。

## 与 Go 版本的对应关系

权威对照是 `pkg/util/stmtsummary/v2/column.go`，Rust 保留了同名列常量、`columnFactoryMap`/`makeColumnFactories`、`formatBackoffTypes`、四类平均函数及 `convertEmptyToNil`。`column_test.go::TestColumn`、`TestExecutionAverageColumnsUseExecCount`、`TestIAAvgColumns` 和 `TestIAAvgColumnsChunkRoundTrip` 给出 Go 的列值与类型契约；Rust 的 `column_test.rs` 对应覆盖代表列、无符号大数、IA 指标、时区和 chunk 往返。

语言适配差异主要有三点：Go 工厂返回 `any`，Rust 以 `ColumnValue` 明确区分 Datum 类型；Go 的 `columnInfo` 由 reader 类型实现，Rust 使用独立 `ColumnContext`；Go 可直接把任意字节放进 string，Rust 对 `Vec<u8>` 增加 bytes-as-string Datum 路径。Go plan 解码失败写结构化日志，Rust 当前写标准错误，外部可观察日志设施不同，但两者都返回空 plan 而不让查询失败。Rust 还对纳秒转换做 `i64::MAX` 钳制，并显式实现 epoch 前亚秒向下取整。

迁移状态不是桩：内存与历史 reader 均已接线到 Rust 工厂。不过 Go 文件仍是逐项行为基准；新增或改名列时必须同步检查 Go 实现、Rust 常量/注册表及两侧测试，不能只让 Rust 表面编译通过。

## 扩展指南

新增 statement-summary 列时，先确认其 Go commit 增量与表字段类型，然后同步：列名常量、`columnFactoryMap` 条目、`StmtRecord` 已有字段或必要的局部接线，以及独立的 `column_test.rs`/适用的 formal 测试。不要把 Rust 测试嵌入本源文件；本 crate 已通过 `lib.rs` 挂载独立测试。

选择返回类型时按 SQL 列类型使用 `Int`、`Uint`、`Float`、`String`、`Time` 或专用 `Datum`；尤其不可将 `u64` 最大值经 `i64` 中转。派生平均值必须先判定其分母属于 `ExecCount` 还是 `CommitCount`，并补零分母测试。时间列需复用 `timestamp_value` 并覆盖目标时区/epoch 边界；可空字符串应明确是否复用 `convertEmptyToNil`。

修改热路径时关注每行每列的 clone、join、排序和 plan 解码成本。若需要改变错误策略、并列 backoff 次序或 sample-user 确定性，这些都属于可观察兼容行为，应先更新 Go 对照与独立回归测试，而不是在工厂内静默改变。新增表列但遗漏注册会在 reader 构造阶段 panic，因此应增加“请求新列并执行工厂”的测试来证明接线完整。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/util/stmtsummary/v2` 列出目标、Go 对照、reader/record 与独立测试；目标文件节点报告 176 个符号及直接引用文件。精确 `query` 确认 Rust `makeColumnFactories` 位于第 790 行、`formatBackoffTypes` 位于第 810 行、`ColumnValue` 与 `ColumnInfoContext` 均属于本文件。通用 `explore` 因常见列名产生大量跨仓库同名噪声，精确 callers/callees 查询在 30 秒内无结果，故生产调用边另由下述源码搜索核实。
- 已读生产文件：`pkg/util/stmtsummary/v2/column.rs`（全文件）、`lib.rs`、`Cargo.toml`，以及 `reader.rs` 中 `NewMemReader`、`NewHistoryReader`、`parseWorker`、`buildRow`、`buildValues`；目标目录没有 `doc.go`。
- 已读 Go 对照：`pkg/util/stmtsummary/v2/column.go`、`column_test.go`，并以 `reader.go` 中两处 `makeColumnFactories` 调用确认 Go 主链一致。
- 已读 Rust 测试：`pkg/util/stmtsummary/v2/column_test.rs` 与 `column_1_aster_unit_test.rs`。证据覆盖列顺序与代表字段、零分母、无符号大数、IA 指标、chunk 往返、epoch 前时间、plan 非 UTF-8 字节、backoff 格式及 reader 集成。
- 静态计数：目标文件包含 128 个 `pub const` 列名和 128 个 `factories.insert` 注册项。本文是纯文档分析，按计划未运行 Cargo 或代码测试；最终只执行任务指定的 11 章节结构验证并人工复核关键行为与直接调用关系。
