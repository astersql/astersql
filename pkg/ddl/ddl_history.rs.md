# `pkg/ddl/ddl_history.rs`

## 文件定位

本文件属于 `astersql-ddl` crate：`pkg/ddl/Cargo.toml` 将 crate 根设为 `lib.rs`，而 `pkg/ddl/lib.rs` 通过 `pub mod ddl_history;` 暴露本模块。它提供一个进程内的 DDL 历史任务容器 `HistoryStore`，承接 Go `pkg/ddl/ddl_history.go` 中“按任务 ID 查询、读取最近任务、遍历全部任务、分页扫描”等接口的部分语义。

这里不是当前 Rust DDL 完成归档主链的持久化实现。仓库搜索只发现独立测试 `pkg/ddl/ddl_history_test.rs` 直接构造和调用 `HistoryStore`，没有发现生产 Rust 调用者；模块也不访问事务、KV、系统表或 `mysql.tidb_ddl_history`。因此它目前更准确的定位是“已公开、已做局部语义移植并由单元测试约束的内存实现”，不能据此推断完整应用已经通过它保存历史任务。

## 核心职责

- 以 `Vec<Job>` 保存任务快照，并维持 `Job.id` 严格按降序排列的内部约定；最新任务位于下标 0（`HistoryStore::add_history_job`）。
- 用任务 ID 作为逻辑唯一键：重复写入同一 ID 时替换旧值，而不是追加重复项（`add_history_job`）。
- 提供三种读取顺序：`get_by_id` 定点读取，`last_n`/`iter_batches`/`scan` 按新到旧读取，`all` 按旧到新返回完整集合。
- 为无界扫描提供 `DEFAULT_SCAN_LIMIT = 2048`，并拒绝“指定起始 ID 但未指定条数”的调用（`scan`）。
- 对外返回克隆后的 `Job` 或借用期内的只读切片，避免调用方直接破坏内部排序和唯一性约定。

本文件不负责任务状态迁移、任务完成判定、持久化、事务边界、序列化或系统表写入；这些能力不能由 `HistoryStore` 的名字推断出来。

## 主要符号

- `pub const DEFAULT_SCAN_LIMIT: usize = 2048`：对应 Go 的 `DefNumGetDDLHistoryJobs`，仅在 `scan(0, 0)` 时生效。
- `pub struct HistoryStore { jobs: Vec<Job> }`：唯一公开类型。类型派生 `Clone`、`Debug`、`Default`；字段私有，外部只能通过方法维护和观察内容。
- `add_history_job(&mut self, job: Job, _update_raw_args: bool)`：按 `job.id` 查重，覆盖或追加后使用 `Reverse(job.id)` 重排。`_update_raw_args` 是 Go API 兼容参数，当前无行为。
- `get_by_id(&self, id: i64) -> Option<Job>`：线性查找并克隆命中的任务；未命中返回 `None`。
- `last_n(&self, maximum: usize) -> Vec<Job>`：从降序数组头部克隆至多 `maximum` 项。
- `iter_batches(&self, batch_size: usize, finish: impl FnMut(&[Job]) -> bool)`：以只读切片按新到旧分批回调；回调返回 `true` 时停止。`batch_size` 为 0 时按 1 处理。
- `all(&self) -> Vec<Job>`：反向遍历内部降序数组，返回按 ID 升序排列的全部克隆。
- `scan(&self, start_job_id: i64, limit: usize) -> Result<Vec<Job>, String>`：按新到旧过滤 `id <= start_job_id` 的任务并截断；起点为 0 表示不做 ID 过滤，条数为 0 则改用默认上限。

这些符号都没有条件编译属性；测试模块本身由 `pkg/ddl/lib.rs` 中的 `#[cfg(test)] mod ddl_history_test;` 独立挂载，测试逻辑没有内嵌在生产源文件中。

## 执行流程

写入流程从 `add_history_job` 开始：先在线性数组中寻找相同 ID；命中则原位替换完整 `Job`，否则推入尾部；随后无论覆盖还是新增都执行全量降序排序。由此建立所有读取方法依赖的核心不变量。

读取流程按接口分为四类：

1. `get_by_id` 从头到尾寻找第一个相同 ID，并把任务克隆给调用方。
2. `last_n` 直接截取内部数组前缀；`maximum == 0` 时自然得到空数组。
3. `iter_batches` 用 `chunks(batch_size.max(1))` 从数组头部开始分批；每批调用一次闭包，闭包决定是否提前结束。空存储不会调用闭包。
4. `all` 反转遍历方向得到旧到新顺序；`scan` 则保留新到旧顺序，先校验参数，再按可选上界过滤和限制条数。

`scan(0, 0)` 的具体流程是把 0 条替换为 2048 条，返回最新至多 2048 个任务；`scan(nonzero, 0)` 在接触数据前直接返回错误；`scan(nonzero, positive)` 从所有 `id <= start_job_id` 的记录中取最新的指定条数。实现对负数起点没有特殊禁止，仍按相同比较规则处理，这一点由 Rust 测试显式覆盖错误参数组合。

## 数据与状态

唯一持久状态是 `HistoryStore.jobs: Vec<crate::ddl::Job>`。`Job` 定义在 `pkg/ddl/ddl.rs`，包含 ID、SQL 文本、状态、版本、时间戳、动作类型及库表 ID 等字段；本模块只读取 `id` 做唯一性、排序和过滤，替换与克隆时保留整个任务对象。

内部不变量是“每个 ID 最多一项，且 ID 降序”。字段私有可防止普通外部代码绕过写入方法，但 `HistoryStore` 的 `Clone` 会复制完整容器，因此两个克隆此后独立演化。`Default` 产生空数组。

内存与时间特征如下：写入查重为 O(n)，随后排序为 O(n log n)；`get_by_id` 为 O(n)；`last_n` 和受限 `scan` 的结果克隆成本与返回量相关，但 `scan` 的过滤最坏仍遍历 O(n)；`all` 克隆全部任务，内存为 O(n)。容器没有容量上限、淘汰策略或磁盘后端，长期接入生产归档前必须评估内存增长。

## 依赖与调用关系

直接代码依赖只有 `crate::ddl::Job` 和 Rust 标准库的 `Vec`、迭代器、切片及 `std::cmp::Reverse`。`pkg/ddl/Cargo.toml` 证明本模块位于 `astersql-ddl` crate，`pkg/ddl/lib.rs` 公开模块并在测试构建中挂载 `ddl_history_test.rs`。

RustCodeGraph 将目标识别为 10 个符号，并确认 `ddl_history_test.rs` 对 `HistoryStore` 的导入边。精确 `callers` 查询在本次分析中超时，因此又以仓库文本搜索核对：除同名但无关的统计模块 trait 外，生产 `.rs` 中没有 `crate::ddl_history`、`ddl_history::HistoryStore`、本模块 `HistoryStore` 或 `DEFAULT_SCAN_LIMIT` 的直接使用；已验证的直接上游只有 `pkg/ddl/ddl_history_test.rs`。

下游关系均发生在容器内：`add_history_job` 操作 `Job.id` 和数组排序；其余方法读取数组并返回克隆或临时切片。它没有调用 `pkg/ddl/table_mode.rs` 的历史系统表写入逻辑，也没有接入 `job_worker`、`meta`、KV 事务或 session。因此“DDL owner 完成任务后将其移入历史”的完整链路是 DDL 子系统背景，不是本文件当前已验证的调用链。

## 错误处理与边界

唯一显式错误来自 `scan`：只要 `start_job_id != 0 && limit == 0`，就返回固定字符串错误 `when 'start_job_id' is specified, it must work with a 'limit'`。该条件也覆盖负数起点；`pkg/ddl/ddl_history_test.rs::test_scan_history_ddl_jobs_with_error_limit` 对 10 和 -1 都做了断言。

其他边界依赖迭代器的自然行为：空存储查询返回 `None` 或空数组；`last_n`/`scan` 的限制大于记录数时只返回现有记录；0 批大小被提升为 1；回调无法返回错误，只能以布尔值停止。相比 Go `IterHistoryDDLJobs` 的 `(bool, error)` 回调，Rust 接口丢失了回调错误传播能力。

任务 ID 没有正数校验；排序、过滤和重复判断都接受任意 `i64`。重复 ID 会无条件以新对象替换旧对象，`_update_raw_args` 不参与决策。方法通过克隆隔离返回值修改，但克隆大型 `Job.query` 等字段会带来分配成本。

## 并发与资源生命周期

`HistoryStore` 自身没有锁、原子变量、异步任务、通道、事务或 I/O。修改方法要求 `&mut self`，读取方法要求 `&self`，并发访问策略由 Rust 借用规则和外部所有者决定；本类型没有自行提供跨线程共享包装。需要多线程共享时，调用方必须选择合适的同步容器，并保持所有写入仍经过 `add_history_job`。

`iter_batches` 借用内部数组并把短生命周期的 `&[Job]` 交给同步闭包，切片不能安全逃逸出回调；闭包执行期间也不能通过同一 `HistoryStore` 做可变写入。其他读取方法克隆结果，返回后不再借用存储。容器析构时由 `Vec` 正常释放所有 `Job`；没有需要显式关闭或回滚的资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/ddl_history.go`，Cargo 元数据也声明 `go-package = "pkg/ddl"`。语义对应关系如下：

- `DEFAULT_SCAN_LIMIT` 对应 `DefNumGetDDLHistoryJobs = 2048`。
- `add_history_job` 近似 `AddHistoryDDLJob` 的“归档并按 ID 可查询”目的，但 Rust 只写内存；Go 会先尝试向 `mysql.tidb_ddl_history` 插入编码后的任务，再写入 `meta.Mutator` 的历史 job list，并且表写失败只记录日志、后者错误才返回。Rust 不编码、不双写、不记录日志，且忽略 `updateRawArgs`。
- `get_by_id` 对应 `GetHistoryJobByID` 的结果语义，但 Go 会创建并提交只读事务并从 meta 读取；Rust 线性读取内存。
- `last_n` 对应 `GetLastNHistoryDDLJobs`；两者都从最新任务开始并限制条数。
- `iter_batches` 对应 `IterHistoryDDLJobs` 的分批与提前停止意图，但 Go 固定每批 10 条、可传播迭代器和回调错误；Rust 批大小由调用者给出且回调不能报错。
- `all` 对应 `GetAllHistoryDDLJobs` 的最终升序结果；Go 以 128 条为批从持久迭代器读取后显式排序，Rust 依赖内部已降序而直接反转克隆。
- `scan` 对应 `ScanHistoryDDLJobs` 的起点、限制、降序和默认 2048 条语义；Go 可通过 failpoint 改默认限制并传播 meta 迭代器错误，Rust 没有 failpoint 或存储错误。

`pkg/ddl/ddl_history_test.go` 验证 Go 的事务、meta、系统表相关路径及默认限制 failpoint；`pkg/ddl/ddl_history_test.rs` 只验证内存排序、扫描、默认上限、错误组合、重复替换和批次提前停止。两套测试的覆盖层级不同，不能用 Rust 测试通过来证明 Go 持久化链路已移植。

## 扩展指南

若只扩展内存查询语义，应优先修改 `HistoryStore` 的相应方法，并同步更新独立测试 `pkg/ddl/ddl_history_test.rs`；不要把测试放回生产源文件。新增筛选或分页 API 时必须明确返回顺序、起点是否包含、0 值含义、负数 ID 行为以及克隆成本，并维护“ID 唯一、内部降序”不变量。

若要把该模块接入真实 DDL 完成归档链路，不能只在某个 worker 中塞入一次 `add_history_job` 调用。需要先对齐 Go `AddHistoryDDLJob` 的持久化/事务/失败语义，明确系统表与 meta 历史列表的兼容策略、owner failover 后可恢复性、何时从运行队列移除，以及进程重启后如何读取；这些是当前实现缺失的能力。接线时还应在生产调用处验证锁顺序和生命周期，避免仅靠无界 `Vec` 累积历史。

若改变 `iter_batches`，建议考虑让闭包返回 `Result<bool, E>` 以对齐 Go 的错误传播；若改变写入策略，需新增重复 ID、空存储、乱序/负数 ID、大量记录和早停测试。兼容风险集中在扫描顺序和边界值，性能风险集中在每次写入全量排序、全量克隆和无界内存增长。

## 验证依据

- 源码：`pkg/ddl/ddl_history.rs`（`DEFAULT_SCAN_LIMIT`、`HistoryStore` 及六个公开方法）；`pkg/ddl/ddl.rs`（`Job` 定义与 `Job::new`）。
- crate 与装配：`pkg/ddl/Cargo.toml`（`astersql-ddl`、`lib.rs`、Go 包映射）；`pkg/ddl/lib.rs`（公开 `ddl_history` 模块与独立测试模块）。
- Rust 测试：`pkg/ddl/ddl_history_test.rs`，覆盖乱序写入后的排序、ID 查询、最近 N 条、全量升序、带起点扫描、默认 2048 上限、非法参数、重复替换及批次早停。
- Go 对照：`pkg/ddl/ddl_history.go` 与 `pkg/ddl/ddl_history_test.go`，用于核对事务/持久化、批次、顺序、默认上限、failpoint 和错误行为差异。
- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/ddl/ddl_history.rs` 命中该文件；`node --file ...` 读取 112 行与 10 个符号；`query ddl_history --json` 核对 Rust/Go 源和测试符号；`node ddl_history.rs::HistoryStore` 给出测试导入边。`callers` 查询超时后，使用 `rg` 补查并确认生产 Rust 没有本模块的直接调用点。
- 结构验证应由任务指定命令确认文档存在且恰好包含本文的 11 个固定二级标题；本任务是纯文档分析，按计划不运行 Cargo。
