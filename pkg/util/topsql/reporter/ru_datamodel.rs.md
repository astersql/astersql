# `pkg/util/topsql/reporter/ru_datamodel.rs`

## 文件定位

本文件是 `astersql-util-topsql-reporter` crate 的 TopRU 内存数据模型。`pkg/util/topsql/reporter/lib.rs` 以私有模块 `ru_datamodel` 装入它，再用 `pub use ru_datamodel::*` 对 crate 使用者重新导出公开符号。它位于语句 RU 增量与线上 protobuf 之间：输入是 `stmtstats::RUKey`、`RUIncrement`、`RUIncrementMap`，输出是 `tipb::TopRuRecord`。

应用内的直接上游是 `pkg/util/topsql/reporter/ru_window_aggregator.rs`：`RUWindowAggregator::addBatch` 把批次写入 `ruCollecting::addBatch`；桶轮转调用 `compactWithLimits`；构造上报窗口时通过 `mergeFrom` 合并桶并由 `toTopRURecords` 编码。更上游的 `pkg/util/topsql/reporter/reporter.rs` 在收集 worker 中调用 `RUWindowAggregator::addBatch`。本文件不负责定时、网络发送、版本切换或迟到数据策略。

## 核心职责

- 用 `ruItem` 表示一个时间戳上的 RU、执行次数与执行时长，用 `ruRecord` 保存同一 SQL digest/Plan digest 的时间序列和用于 Top-N 排序的 `totalRU`。
- 用 `userRUCollecting` 管理单用户的 SQL/Plan 记录；达到预收集容量后，把新键聚合到 `othersRec`，防止键空间无界增长。
- 用 `ruCollecting` 管理用户集合；达到用户预收集容量后，把新用户聚合到独立字段 `othersUser`。
- 在上报前按 RU 总量做两级 Top-N：先筛用户，再筛每个保留用户的 SQL；所有淘汰量仍进入 others 桶而不丢失。
- 合并不同基础桶时可保留时间戳，也可把所有样本重写到目标窗口时间戳；最后把内存结构转换成按时间戳排序的 tipb 记录。

## 主要符号

- 容量常量：`maxTopUsers = 200`、`maxTopSQLsPerUser = 200` 是默认上报上限；`maxPreTopNUsers`、`maxPreTopNSQLsPerUser` 均为对应上限的两倍，是收集阶段的有界容量。`0` 作为构造或压缩参数时表示采用默认值。
- `othersUserWireLabel`：全局 others 用户在线路上的固定用户名 `_TIDB_TOPRU_OTHERS_USER`。真实空用户名仍保留为空，不能用 `userRUCollecting.user` 是否为空判断合成用户。
- `ruItem` / `ruItem::toProto`：将四个标量字段逐一映射到 `tipb::TopRuRecordItem`。
- `sqlPlanKey`、`makeKey`、`othersKey`、`isOthersKey`：以两个 `BinaryDigest` 组成哈希键；两个 digest 都为空的静态键是“其他 SQL”哨兵。只有一个 digest 为空不是哨兵。
- `ruRecord`：保存 digest、`items` 与 `totalRU`。`add` 合并相同时间戳；`addIncr` 忽略 `None`；`merge` 保留源时间戳；`mergeWithTimestamp` 统一改写时间戳。
- `userRUCollecting`：`records` 保存正常 SQL 键，`othersRec` 单独保存淘汰/溢出 SQL，`totalRU` 用于用户级排序。`add`、`addOthers`、`mergeRecord` 是写入口；`getReportRecordsWithLimit` 返回 Top SQL 加可选 others。
- `ruCollecting`：`users` 只保存真实用户，`othersUser` 单独保存全局溢出。`add`/`addBatch` 收集数据，`take` 转移快照并重置，`compactWithLimits` 构造裁剪结果，`mergeFrom` 合并窗口，`toTopRURecords` 编码线路记录。
- 内部辅助：`splitTopRecords`、`splitTopUsers` 用 `f64::total_cmp` 降序排序后切分；`topRURecord` 负责 protobuf 字段装配；`mergeUserIntoOthers` 和 `mergeUserIntoOthersWithTimestamp` 把一个用户的全部 SQL 折叠成目标的 others SQL。

## 执行流程

1. `ruCollecting::addBatch(timestamp, increments)` 遍历增量映射并调用 `add`。已存在用户直接下沉到 `userRUCollecting::add`；新用户在 `preTopNUsers` 未满时创建，已满时只把增量计入 `othersUser.othersRec`。
2. 单用户 `add` 先构造 `sqlPlanKey`。空/空 digest 走 `addOthers`；已有键累加原记录；新键在 SQL 容量未满时创建，容量已满则进入 `othersRec`。每条被接纳的增量恰好使对应记录和用户的 `totalRU` 各增加一次。
3. `RUWindowAggregator` 关闭基础桶时调用 `compactWithLimits`。该函数对真实用户按 `totalRU` 排序，保留前 `maxUsers`；每个保留用户再由 `getReportRecordsWithLimit` 保留前 `maxSQLsPerUser`。淘汰 SQL 合入该用户 `othersRec`，淘汰用户的全部记录合入合成的 `othersUser.othersRec`。
4. 聚合更大时间区间时，`mergeFrom` 逐用户合并。目标仍有容量时保留用户和 SQL 身份；用户或 SQL 超容量时降级到相应 others。`rewriteTimestamp = true` 时，源记录所有 item 被聚合到 `targetTimestamp`；为 `false` 时保留原时间戳。
5. `toTopRURecords` 对每条记录的 `items` 原地按 `timestamp` 排序，编码真实用户的普通 SQL 与 per-user others，最后只把 `othersUser.othersRec` 编码为固定线标签。该函数本身不再执行 Top-N，调用者应先压缩。

## 数据与状态

`ruRecord.totalRU` 是其所有已累加 item RU 的汇总；`userRUCollecting.totalRU` 是该用户正常记录与 others 记录的汇总。这两个冗余总量驱动 Top-N，修改合并路径时必须同步维护。`execCount` 与 `execDuration` 在同时间戳相加时使用 `wrapping_add`，因此溢出按无符号整数回绕而不是报错；`totalRU` 使用普通 `f64` 加法。

others 有两个独立层次：`userRUCollecting.othersRec` 表示某个真实用户内未保留身份的 SQL；`ruCollecting.othersUser` 表示未保留身份的用户，并最终只输出其中的 `othersRec`。`addOthers` 还会把遗留在 `records[othersKey]` 的兼容数据迁移进 `othersRec`。`HashMap` 的迭代顺序不稳定，因此 protobuf 记录整体顺序没有稳定保证；只有每条记录内部的 item 时间顺序由 `toTopRURecords` 保证。

`take` 使用 `std::mem::replace` 转移 `users`，使用 `Option::take` 转移 `othersUser`，并保留容量配置；返回快照与复位后的收集器不共享这些容器。`compactWithLimits` 从 `&self` 克隆候选记录并新建结果，不修改原收集器；`toTopRURecords` 需要 `&mut self`，因为它会原地排序 item。

## 依赖与调用关系

crate 边界由 `pkg/util/topsql/reporter/Cargo.toml` 定义，包名为 `astersql-util-topsql-reporter`，`lib.rs` 是库入口，未为本文件设置 feature 条件。直接类型依赖来自 `topsql_stmtstats`（在 `lib.rs` 重导出为 `stmtstats`）和 git 固定 revision 的 `tipb`（重导出为 `tipb_protobuf`）；本文件自身只使用标准库 `HashMap`、`LazyLock`。

关键调用边经 RustCodeGraph 与源码交叉确认：`RUWindowAggregator::addBatch → ruCollecting::addBatch → ruCollecting::add → userRUCollecting::add → ruRecord::addIncr/add`；`rotateBucketsBefore → compactWithLimits → splitTopUsers/getReportRecordsWithLimit/mergeUserIntoOthers`；`buildReportRecords → mergeFrom → userRUCollecting::mergeRecord → ruRecord::merge/mergeWithTimestamp`；最终 `buildReportRecords → toTopRURecords → topRURecord → ruItem::toProto`。

`pkg/util/topsql/reporter/lib.rs` 将本文件符号整体重导出，也把独立测试模块 `ru_datamodel_test.rs` 仅在 `cfg(test)` 下装入。Cargo 的 `failpoints` feature 为空，本文件没有条件编译项。

## 错误处理与边界

本模型没有可恢复错误返回值。可缺省输入统一使用 `Option`：`addIncr(None)`、`merge(None)`、`mergeRecord(None, ...)`、`mergeFrom(None, ...)` 都是无操作；`mergeRecord` 还拒绝 `items` 为空的源，即使源的 `totalRU` 非零也不创建目标记录。空收集器的 `compactWithLimits` 返回 `None`，而 `toTopRURecords` 返回空向量。

容量判断只在创建新用户/新 SQL 时触发，已有键即使容量已满仍继续累加。Top-N 使用 `f64::total_cmp`，因此对 NaN 等特殊浮点值也有全序，但文件没有拒绝负值、NaN 或无穷值；调用者负责提供合法 RU。相同 `totalRU` 的相对顺序没有对外承诺。

内部 `expect("user initialized")` 与 `expect("others record initialized")` 依赖同一可变借用内先创建再取回的不变量；正常路径不会失败。protobuf 转换不执行 I/O，也不返回编码错误。全局 others 只输出 `othersUser.othersRec`；若外部绕过 API 直接把正常记录放入公开的 `othersUser.records`，须先经压缩/合并路径折叠，否则不会被线路输出。

## 并发与资源生命周期

`ruCollecting`、`userRUCollecting` 和 `ruRecord` 本身没有锁或原子量，写操作要求独占 `&mut self`。生产主链由 `RUWindowAggregator.state: Mutex<AggregatorState>` 包围桶创建、写入、轮转与取出；`buildReportRecords` 在移出窗口桶并释放锁后，独占处理这些值。因此本文件依赖所有权和上游外部同步，而不是内部锁。

内存增长由两级 pre-TopN 容量约束：真实用户数不超过 `preTopNUsers`，每个真实用户的正常 SQL 键不超过 `preTopNSQLsPerUser`，溢出聚合到常数个 others 结构。每个 `ruRecord.items` 的长度仍随不同时间戳数量增长；窗口聚合器通过固定基础桶、轮转压缩和窗口取出来界定生产生命周期。大量压缩会克隆记录并完整排序，开销约受候选用户数、每用户候选 SQL 数及 item 数量影响。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/topsql/reporter/ru_datamodel.go`，同名常量、结构、构造器、收集、压缩、合并和 protobuf 转换构成一一对应的移植主干；`pkg/util/topsql/reporter/ru_datamodel_test.go` 与 Rust 独立测试覆盖相同的累加、Top-N、others、take、时间排序和空输入场景。

当前可见差异需要在后续同步时保留意识：Go 的 `ruCollecting` 内含 `sync.Mutex`，其 `take` 自行加锁；Rust 模型没有内部锁，由 `RUWindowAggregator` 的状态锁提供生产同步。Go Top-N 使用 quickselect，失败时记录警告并返回未裁剪集合；Rust 使用全量 `sort_by(...total_cmp...)`，没有错误分支。Go 的 `getReportRecordsWithLimit` 和 `compactWithLimits` 有少分配快路径，后者可能直接返回原对象；Rust 总是收集/克隆并构造新的压缩结果。Go 对非正整数容量回退默认值，Rust 参数是 `usize`，只需处理零值。两者在线路语义、others 身份以及时间戳合并规则上保持一致。

## 扩展指南

- 新增采样字段时，应同步修改 `ruItem`、`ruItem::toProto`、`ruRecord::add/mergeWithTimestamp`、tipb schema/依赖版本，以及独立的 Rust/Go 数据模型测试，确保同时间戳累加、跨窗口合并和线路字段都覆盖。
- 调整 Top-N 或容量策略时，入口是四个容量常量、`splitTopRecords`、`splitTopUsers`、`userRUCollecting::add/mergeRecord` 与 `ruCollecting::add/compactWithLimits/mergeFrom`。必须验证总 RU 守恒、已有键不因容量满而丢失、per-user others 与全局 others 不混淆，并同步检查 `ru_window_aggregator.rs` 中用于基础桶、区间和最终报告的容量选择。
- 修改 others 表示时，必须同时检查 `othersKey`、`addOthers` 的遗留键迁移、`othersUser` 独立字段和 `othersUserWireLabel`；空用户名是合法真实用户，不能重新引入基于空字符串推断合成用户的逻辑。
- 改动 `toTopRURecords` 时要保持 item 按时间戳排序、keyspace 逐记录复制、digest 字节不变以及全局 others 线标签兼容性。若需要稳定的记录间顺序，应显式定义并补测试，不能依赖 `HashMap`。
- Rust 测试逻辑应继续放在同目录独立文件 `pkg/util/topsql/reporter/ru_datamodel_test.rs`，并由 `lib.rs` 的 `#[cfg(test)] mod ru_datamodel_test` 接入；Go 对照测试是 `ru_datamodel_test.go`。不要把测试嵌回生产源文件。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件，目标 `ru_datamodel.rs` 有 50 个符号；通过 `files`、`node --file`、`query ruCollecting`、`query toTopRURecords` 以及关键方法的 `callers`/`callees` 查询核对。由于同名方法较多，callers 查询未能可靠消歧，上游边改由相邻源码的定向引用搜索确认；callees 明确显示了 `compactWithLimits`、`mergeFrom`、`toTopRURecords` 等到本文件辅助函数的边。
- 已读生产证据：`pkg/util/topsql/reporter/ru_datamodel.rs`（完整 625 行）、`pkg/util/topsql/reporter/ru_window_aggregator.rs`（收集、桶轮转、合并和输出主链）、`pkg/util/topsql/reporter/reporter.rs` 的 `ruAggregator.addBatch` 调用点、`pkg/util/topsql/reporter/lib.rs`（模块可见性和重导出）、`pkg/util/topsql/reporter/Cargo.toml`（crate、feature 与依赖边界）。目标包没有 `doc.go`，因此无额外包契约文件可读。
- 已读对照与测试：`pkg/util/topsql/reporter/ru_datamodel.go`、`pkg/util/topsql/reporter/ru_datamodel_test.go`、`pkg/util/topsql/reporter/ru_datamodel_test.rs`；Rust 测试覆盖字段映射、同/异时间戳累加、空源忽略、两级容量、Top-N/others、空用户名隔离、合并、take、排序和空收集器。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以计划给出的命令确认目标文件存在且恰好包含上述 11 个固定二级标题，并人工复核唯一生产物、源码链接、调用链和 Go 差异。
