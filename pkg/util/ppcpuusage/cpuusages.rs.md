# `pkg/util/ppcpuusage/cpuusages.rs`

## 文件定位

[`cpuusages.rs`](./cpuusages.rs) 是 `astersql-util-ppcpuusage` crate 的唯一业务实现文件，负责保存“当前 SQL”在 TiDB 与 TiKV 两侧消耗的 CPU 时间。crate 入口 [`lib.rs`](./lib.rs) 将 `cpuusages` 声明为公开模块，并通过 `pub use cpuusages::*` 再导出本文件的公开类型；[`Cargo.toml`](./Cargo.toml) 则把该 crate 映射到 Go 包 `pkg/util/ppcpuusage`，且本 crate 自身没有第三方依赖或 feature。

在已接线的 Rust 路径中，`pkg/distsql/context/context.rs::DistSQLContext::Detach` 用本文件的读写 API 为分离上下文创建 CPU 快照，`pkg/session/sessmgr/processinfo.rs::ProcessInfo::ToRow` 读取快照并生成 `INFORMATION_SCHEMA.PROCESSLIST` 的 TiDB/TiKV CPU 列。Go 对照版本还把同一容器接入 server profiler、executor、DistSQL result 与慢日志/语句摘要；这些 Go 调用点是迁移目标证据，不代表对应 Rust 入口都已接线。

## 核心职责

- `CPUUsages` 是公开值对象，分别保存 `TidbCPUTime` 和 `TikvCPUTime`，支持复制、比较、默认零值和整体清零。
- `SQLCPUUsages` 把当前 `sqlID` 与 `CPUUsages` 放在同一个互斥锁临界区内，保证并发读、替换、累加、分配 ID 和清零不会发生数据竞争或丢失更新。
- `MergeTidbCPUTime` 只接受当前 `sqlID` 的 profiler 结果，从而丢弃上一条 SQL 延迟到达的 TiDB CPU 样本；`MergeTikvCPUTime` 按 Go 约定不校验 ID，因为该值由正在执行语句的 executor 更新。
- `GetCPUUsages` 返回值拷贝而不是内部引用，使调用方不能绕过锁修改状态，也允许 `DistSQLContext::Detach` 建立后续互不影响的快照。

## 主要符号

- `pub struct CPUUsages { pub TidbCPUTime: Duration, pub TikvCPUTime: Duration }`：公开快照类型。`Clone + Copy + Default + Eq + PartialEq` 使读取结果可以低成本按值传递，默认值为两个 `Duration::ZERO`。
- `pub struct SQLCPUUsages { inner: Mutex<SQLCPUUsagesInner> }`：公开并发容器；受保护状态不对 crate 外暴露。
- `struct SQLCPUUsagesInner { sqlID: u64, cpuUsages: CPUUsages }`：Rust 为表达 Go“一个 mutex 同时保护两个字段”而新增的私有辅助结构。
- `CPUUsages::Reset(&mut self)`：将 TiKV、TiDB 两个累计值都写为零；只要求独占的可变引用，本身不加锁。
- `SQLCPUUsages::default()`：建立 `sqlID == 0`、两端用量均为零的可直接使用容器，对齐 Go 结构体零值。
- `SetCPUUsages(&self, usage)`：持锁整体替换快照。
- `MergeTidbCPUTime(&self, sqlID, d)`：持锁比较 ID，仅匹配时累加 TiDB 时间。
- `MergeTikvCPUTime(&self, d)`：持锁无条件累加 TiKV 时间。
- `GetCPUUsages(&self) -> CPUUsages`：持锁复制当前快照。
- `AllocNewSQLID(&self) -> u64`：持锁执行 `wrapping_add(1)` 并返回新 ID；默认容器首次分配得到 `1`，超过 `u64::MAX` 后回到 `0`。
- `ResetCPUTimes(&self)`：持锁调用 `CPUUsages::Reset`，保留当前 `sqlID`，只清除两端用量。

## 执行流程

1. 调用方以 `SQLCPUUsages::default()` 创建容器，初始 ID 和累计时间均为零。共享场景通常再用 `Arc<SQLCPUUsages>` 持有它；容器方法只需 `&self`，内部可变性由 `Mutex` 提供。
2. 新 SQL 开始时，预期调用 `AllocNewSQLID` 取得代际 ID，并用 `ResetCPUTimes` 清除上一条 SQL 的累计值。当前仓库的完整生产顺序可在 Go 的 `pkg/server/conn.go` 中核对；Rust 生产调用搜索尚未发现这两个入口。
3. profiler 回传 TiDB CPU 时间时调用 `MergeTidbCPUTime(sqlID, d)`。方法在锁内比较样本 ID 与当前 ID；不相等时安静丢弃，相等时才累加，避免跨 SQL 记账。
4. executor/DistSQL 获得 TiKV coprocessor `ProcessTime` 时调用 `MergeTikvCPUTime(d)`，直接在锁内累加。Go 的直接调用点位于 `pkg/distsql/select_result.go`、`pkg/executor/point_get.go` 和 `pkg/executor/batch_point_get.go`；Rust 当前仅在本 crate 独立测试中调用该方法。
5. 展示、慢日志或摘要消费者调用 `GetCPUUsages` 取得一致快照。Rust 的 `ProcessInfo::ToRow` 已将两个 `Duration` 以纳秒数写入进程列表扩展列。
6. `DistSQLContext::Detach` 先读取原容器，再用新容器的 `SetCPUUsages` 写入同值；因此分离前数值相同，分离后两边的更新互不影响。

## 数据与状态

唯一共享可变状态是 `SQLCPUUsagesInner`。`sqlID` 是 SQL 代际标识而非业务 SQL 文本 ID；它只参与 TiDB profiler 样本过滤。`cpuUsages` 是当前代际的累计快照，两个字段均为 `std::time::Duration`，因此 Rust 表达的是非负时间。

锁不变量是：对 `sqlID` 和 `cpuUsages` 的每一次观察或修改都在同一个 `MutexGuard` 生命周期内完成。`ResetCPUTimes` 不改变 ID；`AllocNewSQLID` 不隐式清零；两步若需组成“切换 SQL”的更大原子操作，当前 API 不提供单次临界区保证，调用方必须遵循既定调用顺序并接受两次方法调用之间可被其他线程观察。

`CPUUsages` 是 `Copy` 类型，所以 `GetCPUUsages` 离开锁后返回独立值。`SetCPUUsages` 也按值接收快照。该设计是 `DistSQLContext::Detach` 能复制初值而不共享后续修改的基础。

## 依赖与调用关系

本文件只依赖标准库的 `std::sync::Mutex` 和 `std::time::Duration`。crate 根通过 `pub use cpuusages::*` 暴露 API；workspace 根将其登记为成员和 `facade_util_ppcpuusage`，直接 Rust 依赖可在以下 manifest 中确认：

- `pkg/distsql/context/Cargo.toml`：`ppcpuusage-dependency`，由其 `lib.rs` 再导出为 `ppcpuusage`；`DistSQLContext::Detach` 调用 `GetCPUUsages -> SetCPUUsages`。
- `pkg/session/sessmgr/Cargo.toml`：同样通过别名依赖；`ProcessInfo::ToRow` 调用 `GetCPUUsages`。
- `pkg/util/stmtsummary/Cargo.toml`：直接以 `ppcpuusage` 名称依赖；`statement_summary.rs` 和 `v2/record.rs` 消费 `CPUUsages` 值字段进行摘要累计。
- `pkg/ddl/Cargo.toml` 与 workspace facade 也声明依赖，但本次针对精确符号的 Rust 调用搜索没有据此推断额外运行时调用边。

RustCodeGraph 对目标文件识别出 12 个符号，并确认 `CPUUsages`/`SQLCPUUsages` 及各方法定义。精确 `rg` 调用核验表明 Rust 生产代码直接使用的是 `SetCPUUsages` 与 `GetCPUUsages`；`MergeTidbCPUTime`、`MergeTikvCPUTime`、`AllocNewSQLID`、`ResetCPUTimes` 当前只在 `migration_aster_unit_test.rs` 中出现为调用。完整上游语义来自同路径 Go 实现及其 server/executor/DistSQL 调用点，文档没有把这些 Go 边冒充为 Rust 已接线边。

## 错误处理与边界

该 API 不返回 `Result`。每个 `SQLCPUUsages` 方法都用带上下文文本的 `expect` 获取锁；如果任一持锁线程 panic 导致 mutex poisoned，之后的访问会继续 panic，而不是恢复内部值。这与 Go `sync.Mutex` 没有 poisoning 的行为存在差异，调用方不应在持锁路径中引入可 panic 的扩展逻辑。

ID 不匹配不是错误：`MergeTidbCPUTime` 直接忽略该样本。ID 溢出也不是错误：`wrapping_add` 明确使 `u64::MAX` 的下一代为 `0`，对齐 Go 注释描述的回绕。回绕理论上可能让极旧样本重新匹配，但需要完整耗尽 64 位空间，当前实现没有额外 epoch。

Rust `Duration` 不能表示负值，而 Go `time.Duration` 是有符号纳秒数；正常 CPU 时间不应为负，因此常规语义一致。累加没有饱和处理；极端总量溢出 `Duration` 可表示范围时会 panic。`ProcessInfo::ToRow` 将 `as_nanos()` 的 `u128` 以 `as i64` 转换，超大值会截断；该消费侧转换不在本文件中，但扩展计量范围时必须一并评估。

## 并发与资源生命周期

`Mutex<SQLCPUUsagesInner>` 串行化全部状态操作；单次累加是读改写原子临界区，多线程合并不会丢更新。锁只覆盖一次比较、赋值、累加、复制或清零，不跨外部调用、I/O 或任务等待。`MutexGuard` 离开方法作用域即自动释放，对应 Go 的 `defer Unlock()`。

本文件不创建线程、异步任务、通道、事务或外部资源。共享所有权由调用方用 `Arc` 管理；例如 `ProcessInfo` 与 `DistSQLContext` 保存 `Option<Arc<SQLCPUUsages>>`。容器最后一个 `Arc` 释放时，mutex 与其中的纯值状态一起销毁，无显式关闭流程。

独立测试 `concurrent_merges_are_not_lost` 以 8 个线程分别执行 2,000 次 TiDB/TiKV 合并，并在 join 后验证精确累计值，直接覆盖锁的并发不丢更新性质。`DistSQLContext` 的两组独立测试还验证 detach 前数值相同、容器指针不同、修改原容器不污染副本。

## 与 Go 版本的对应关系

[`cpuusages.go`](./cpuusages.go) 与本文件具有一一对应的两个类型和七个方法：字段顺序、方法名、TiDB 的 ID 过滤、TiKV 的无条件合并、读取/整体替换、ID 回绕及只清时间不清 ID 的行为均被保留。Rust 使用 `#![allow(non_snake_case)]` 保持 Go 风格 API 名称，便于逐行迁移和调用方对照。

主要表示差异如下：

- Go 在 `SQLCPUUsages` 中嵌入 `sync.Mutex` 并并列保存字段；Rust 用私有 `SQLCPUUsagesInner` 将两个字段整体放入 `Mutex`。
- Go 的 `time.Duration` 是有符号 64 位纳秒数；Rust 的 `Duration` 是非负、范围更大的秒/纳秒组合。
- Go 的结构体值返回天然是副本；Rust 显式令 `CPUUsages: Copy`，实现相同的快照语义。
- Go mutex 不 poisoning；Rust 对 poisoned mutex 使用 `expect` panic。
- Go 生产接线已覆盖 `pkg/server`、`pkg/executor`、`pkg/distsql`、慢日志和进程信息；当前 Rust 搜索只确认 detach 复制、processlist 读取及 statement summary 对值类型的消费，其余方法有迁移单元测试但尚不能据此宣称完整生产接线。

同目录没有 Go 测试文件；Rust 的 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 是独立测试文件，覆盖 Go 方法契约和 Rust 并发实现，符合测试不内嵌生产源文件的仓库约束。

## 扩展指南

- 新增统计维度时，应同时修改 `CPUUsages`、`Reset`、必要的 merge/set/get 语义，并核对 Go 同路径结构；所有构造字面量和 `ProcessInfo::ToRow`、statement summary 消费点也需要同步。字段增加会使现有无 `..Default` 的字面量编译失败，这是定位遗漏消费者的有用信号。
- 改变 SQL 代际切换逻辑时，重点审查 `AllocNewSQLID` 与 `ResetCPUTimes` 分离调用产生的可观察窗口。若要求二者原子化，应新增一个持单次锁的明确 API，并同步 Go 行为与 server 调用顺序，不能只在调用方外层假设原子性。
- 接通 Rust profiler/executor 链路时，TiDB 样本必须携带由 `AllocNewSQLID` 返回的 ID，TiKV 路径则应只累计当前 executor 的 process time；应参照 Go 的 `pkg/server/server.go`、`pkg/distsql/select_result.go` 与 point-get 调用点，而不是绕过容器直接改快照。
- 改变溢出、负值或 poisoned-lock 策略会造成 Go/Rust 兼容差异，需明确决定是保持现状、返回错误还是饱和，并增加边界测试。
- 测试应继续放在独立的 `migration_aster_unit_test.rs`（或同目录新的独立 `*_test.rs`）中；至少同步覆盖零值、ID 匹配/不匹配、回绕边界、set/reset、并发累计，以及 detach 快照独立性。不要把 `#[cfg(test)]` 测试模块塞回 `cpuusages.rs`。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/util/ppcpuusage` 找到 Rust 实现、Go 对照、crate 入口和独立测试；`explore "pkg/util/ppcpuusage/cpuusages.rs CpuUsages ..."` 返回目标完整源码与 Go 对照；`query` 确认各公开方法在 Rust/Go 中的定义。单独的 `callers MergeTidbCPUTime --file ...` 在 60 秒内无输出后中止，因此实际调用边又用精确 `rg` 核验，未据缺失图结果作推断。
- 实现与装配：`pkg/util/ppcpuusage/cpuusages.rs`、`pkg/util/ppcpuusage/lib.rs`、`pkg/util/ppcpuusage/Cargo.toml`、workspace `Cargo.toml`，以及直接依赖该 crate 的各 Cargo manifest。
- Go 对照与生产链：`pkg/util/ppcpuusage/cpuusages.go`；调用搜索覆盖 `pkg/server/conn.go`、`pkg/server/server.go`、`pkg/executor/adapter.go`、`pkg/executor/adapter_slow_log.go`、`pkg/executor/{point_get,batch_point_get}.go`、`pkg/distsql/{context,select_result}.go` 和 `pkg/session/sessmgr/processinfo.go`。
- Rust 直接调用与消费：`pkg/distsql/context/context.rs`、`pkg/session/sessmgr/processinfo.rs`、`pkg/util/stmtsummary/statement_summary.rs`、`pkg/util/stmtsummary/v2/record.rs`。
- 测试证据：`pkg/util/ppcpuusage/migration_aster_unit_test.rs` 覆盖 reset、ID 过滤、set/get、ID 起点及多线程合并；`pkg/distsql/context/context_test.rs` 与 `migration_aster_unit_test.rs` 覆盖 detach 快照独立性；`pkg/session/sessmgr/migration_aster_unit_test.rs` 覆盖 processlist CPU 输出。
- 本任务是只新增说明文档的分析任务，按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构命令，并人工复核以上“已接线/Go 目标链路”的边界表述。
