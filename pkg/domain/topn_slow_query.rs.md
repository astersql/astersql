# `pkg/domain/topn_slow_query.rs`

## 文件定位

本文件属于 `astersql-domain` crate；crate 根 `pkg/domain/lib.rs` 以 `pub mod topn_slow_query` 公开该模块，测试则通过同一 crate 中的独立文件 `pkg/domain/topn_slow_query_test.rs` 引入。`pkg/domain/Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `[package.metadata.porting] go-package = "pkg/domain"` 共同确认了 crate 边界与 Go 移植来源。

它提供一个内存中的、线程安全的慢查询最近队列与 Top-N 容器。当前全仓 Rust 调用搜索只发现模块声明和 `topn_slow_query_test.rs` 对公开 API 的使用；`pkg/domain/domain.rs` 目前维护另一套 `SlowQueryInfo`/`SlowQueryState`，没有接入这里的 `TopNSlowQueries`。因此本文件目前是可复用的公开实现与 Go 语义移植件，不应描述成已经驱动 Rust `Domain` 的 `SHOW SLOW` 主链。

文件第 24—280 行是整段注释掉的机械翻译草稿，不参与编译；实际实现从 `use std::collections::VecDeque` 开始。

## 核心职责

- `TopNSlowQueries` 同时维护容量受限的 recent FIFO、用户慢查询 Top-N 和内部慢查询 Top-N（`SlowQueryState::{recent,user,internal}`）。
- `append` 让每条被接受的记录先进入 recent，再依据 `SlowQueryInfo::internal` 进入用户堆或内部堆；Top-N 仅保留耗时最长的记录。
- `query_all` 按 FIFO 的旧到新顺序返回 recent 全量；`query_recent` 按新到旧返回最多 `count` 条；`query_top` 按耗时从长到短返回所选类别。
- `remove_expired` 只清理两个 Top-N 集合，不清理 recent 队列，这与 Go `topNSlowQueries.RemoveExpired` 的范围一致。
- `close` 阻止后续追加并发出条件变量通知；`append` 以布尔值报告记录是否被接受。

## 主要符号

- `ExecDetails`：公开的执行明细值对象，记录 `process_time`、`wait_time`、`backoff_time` 和 `request_count`；它是本文件本地的简化 Rust 数据类型。
- `SlowQueryInfo`：公开慢查询记录，包含 SQL、起始时间、耗时、执行明细、连接/会话/事务、用户/数据库/表/索引、digest、内部语句标记与成功标记。记录被 `Arc<SlowQueryInfo>` 共享，查询无需深拷贝各字符串。
- `ShowSlowKind::{Default, Internal, All}`：公开查询分类；这是本模块自己的枚举，并非 `pkg/parser/ast` 中同名语义类型。
- `SlowQueryHeap`：私有、以 `Vec<Arc<SlowQueryInfo>>` 保存的 Top-N 集合。`rebuild` 将其按 `duration` 升序排列，因此索引 0 是当前最短项；实现使用完整排序而非标准库二叉堆。
- `SlowQueryHeap::push_top_n`：容量未满时插入；已满时仅当新记录严格慢于最短项才替换。相等耗时不会替换，`capacity == 0` 直接忽略。
- `SlowQueryHeap::remove_expired`：保留满足 `start + period > now` 的记录；等于边界的记录已经过期。
- `SlowQueryQueue`：私有定长 `VecDeque`。`enqueue` 在满时先移除队首，容量为零时直接丢弃；`query` 反向迭代形成新到旧结果。
- `take_last_n`：从升序切片尾部反向克隆最多 `count` 个 `Arc`，供堆查询和合并查询复用。
- `SlowQueryState`：由单个 `RwLock` 保护的全部可变集合及 `closed` 标记，保证追加、清理、查询和关闭看到一致状态。
- `TopNSlowQueries`：公开门面，保存不可变配置 `top_n`、`period`，以及 `state` 和 `(Mutex<bool>, Condvar)` 组成的 `close_event`。公开方法为 `new`、`append`、`query_all`、`query_recent`、`query_top`、`remove_expired`、`close`、`is_closed`。

## 执行流程

1. `TopNSlowQueries::new(top_n, period, queue_size)` 创建 recent 队列、两个空 Top-N 集合和未关闭状态；容量参数允许为零。
2. `append(info)` 获取 `state.write()`。若已经关闭，立即返回 `false`，且 recent/Top-N 均不改变。
3. 未关闭时将记录包装为 `Arc`，克隆一个引用进入 `SlowQueryQueue::enqueue`，原引用依据 `internal` 进入 `user` 或 `internal` 的 `push_top_n`，最后返回 `true`。
4. `push_top_n` 在未满时插入并升序重排；已满时比较最短项，只用严格更长的新记录替换它。由此每个分类最多保留 `top_n` 条最慢记录。
5. 查询方法取得读锁：`query_all` 直接按 recent 的队首到队尾收集；`query_recent` 反向读取；`query_top(Default/Internal)` 从对应升序集合尾端反取，`All` 则合并两组、按耗时升序排序后反取。
6. `remove_expired(now)` 取得写锁，对用户和内部集合分别执行过滤及重排。判断采用严格的 `expires > now`。
7. `close()` 先在写锁内设置 `closed = true`，再把 `close_event` 的布尔值设为真并 `notify_all`；之后 `is_closed` 返回真且 `append` 拒绝新记录。

## 数据与状态

`top_n` 和 `period` 构造后不变。recent 的长度不超过 `queue_size`；用户与内部集合各自不超过 `top_n`。一条已接受记录最多由 recent 与其中一个 Top-N 集合共同引用，二者通过 `Arc` 指向同一不可变 `SlowQueryInfo`。

Top-N 的核心不变量是 `SlowQueryHeap::data` 按 `duration` 升序排列：`first()` 是淘汰候选，`take_last_n` 的结果为降序。相同耗时的记录在集合已满时不会替换现存最短项。recent 与 Top-N 的保留策略彼此独立：记录可能已从 recent 被容量淘汰但仍在 Top-N，也可能被 `remove_expired` 从 Top-N 清除却仍留在 recent。

过期计算使用 `SystemTime::checked_add`。若 `start + period` 溢出，闭包返回 `false`，该记录会被移除；这是一项 Rust 特有边界行为。`count` 大于集合长度时迭代自然截断，`count == 0` 返回空向量。

## 依赖与调用关系

本文件只依赖 Rust 标准库：`VecDeque`、`Arc`、`Mutex`、`RwLock`、`Condvar`、`Duration` 与 `SystemTime`；没有直接使用 `pkg/domain/Cargo.toml` 中的外部 crate 依赖。

内部调用边为：`TopNSlowQueries::append` → `SlowQueryQueue::enqueue` 与 `SlowQueryHeap::push_top_n` → `SlowQueryHeap::rebuild`；`query_recent` → `SlowQueryQueue::query`；`query_top` → `SlowQueryHeap::query`/`take_last_n`；公开 `remove_expired` → 两个私有堆的 `remove_expired` → `rebuild`。

RustCodeGraph 已索引 `pkg/domain/topn_slow_query.rs` 并识别 `TopNSlowQueries`、`SlowQueryHeap`、`push_top_n`、`query_top` 和两层 `remove_expired`。图查询在此大型索引上未返回可用的 callers/callees 文本，因此又以全仓精确 Rust 搜索核对调用面：除 `pkg/domain/lib.rs` 和 `pkg/domain/topn_slow_query_test.rs` 外没有直接使用者。Go 侧的真实上游在 `pkg/domain/domain.go`：构造函数创建 `topNSlowQueries`，`topNSlowQueryLoop` 接收新增记录、每十分钟清理过期项并处理查询消息。

## 错误处理与边界

公开 API 不返回 `Result`。锁获取统一调用 `expect`；如果持锁线程 panic 导致锁中毒，后续调用会以 `slow query state poisoned` 或 `slow query close event poisoned` panic，而不是恢复或返回错误。

关闭是幂等的状态赋值和通知，但 `append` 与 `close` 的先后由同一写锁线性化：先拿到锁的追加可以完成，关闭生效后的追加返回 `false`。`remove_expired` 即使已经关闭仍会执行，因为方法没有检查 `closed`。`query_*` 与 `is_closed` 同样可在关闭后使用。

零容量得到明确定义：recent 丢弃全部记录，Top-N 不保存记录，追加在未关闭时仍返回 `true`。过期边界为 `start + period <= now` 即删除。由于本文件没有 SQL 解析、权限或阈值判断，它假定调用者只提交已经认定为慢查询的完整记录。

## 并发与资源生命周期

所有集合和关闭标记位于同一 `RwLock<SlowQueryState>` 下：多个只读查询可并行，追加、清理和关闭互斥；查询返回 `Arc` 克隆，释放读锁后记录仍有效。写锁覆盖 recent 与分类 Top-N 的联合更新，因此读者不会观察到“已进 recent 但尚未进 Top-N”的中间状态。

`close_event` 保存一个条件变量和布尔谓词，`close` 会设置谓词并唤醒所有等待者；但本文件没有公开或私有的等待方法，当前仓库也未发现读取该谓词或调用 `Condvar::wait` 的代码，所以通知目前没有本模块内的可观察消费者。该事实与注释中“对应 Go channel worker”应区分开：Rust 实现本身不创建线程、不启动定时清理，也没有输入/查询 channel，调用者必须自行安排追加、清理与生命周期。

资源全部由 RAII 管理；最后一个 `Arc` 被释放时记录销毁，`TopNSlowQueries` 被丢弃时锁、队列和条件变量随之销毁。没有自定义 `Drop`，丢弃容器不会隐式调用 `close`。

## 与 Go 版本的对应关系

直接对照为 `pkg/domain/topn_slow_query.go`，测试对照为 `pkg/domain/topn_slow_query_test.go`。Rust 保留了 Go 的三个主要数据结构语义：定长 recent FIFO、按用户/内部拆分的 Top-N、严格 `Start + period > now` 的保留边界；`pkg/domain/topn_slow_query_test.rs` 逐项复刻了 Go 的堆替换、过期和队列顺序用例。

实现机制存在明确差异：Go 用 `container/heap` 维护小根堆，Rust 每次变更后完整升序排序；Go 的 `topNSlowQueries` 由 `ch`/`msgCh` 和 `Domain.topNSlowQueryLoop` 单 owner 串行处理，Rust 用 `RwLock` 让调用者直接同步调用；Go `Close` 关闭输入 channel，Rust 设置标记并让 `append` 返回 `false`。Go 的查询种类来自 `ast.ShowSlowKind`，Rust 本文件定义独立 `ShowSlowKind`。

数据模型也不是逐字段类型复用：Go `Detail` 使用 `execdetails.ExecDetails`，Rust 定义本地 `ExecDetails`；字段改成 snake_case。Rust 额外处理零容量而不会索引空 Top-N，且用 `checked_add` 处理时间溢出。另一方面，Rust `close_event` 尚无等待端，不能等同于 Go 关闭 channel 对接收循环的唤醒效果。

此外，当前 `pkg/domain/domain.rs` 自己定义了更精简的慢查询记录和单一 Top 列表；它不区分用户/内部 Top-N，也未复用本模块。后续接线必须先决定数据模型、枚举和生命周期的统一方式，不能简单假设两套类型可互换。

## 扩展指南

- 若要把本实现接入 Rust `Domain`，最可能修改 `pkg/domain/domain.rs` 的慢查询字段、`log_slow_query`/`show_slow_queries` 与构造路径，并建立 `domain.rs::SlowQueryInfo`、本文件 `SlowQueryInfo` 及 parser AST 枚举之间的明确边界；不要同时保留两套互不一致的状态作为同一功能真相源。
- 若新增查询种类，应同步修改 `ShowSlowKind`、`TopNSlowQueries::query_top` 和独立测试 `pkg/domain/topn_slow_query_test.rs`；若面向 SQL `SHOW SLOW`，还要核对 `pkg/parser/ast` 的枚举与解析接线。
- 若改变 Top-N 相同耗时替换规则、排序方式或过期边界，必须同步 Go 对照意图及 Rust 的三个对齐测试。性能上，当前 `rebuild` 每次插入为排序成本；切换到 `BinaryHeap` 时仍须保持“最短项可淘汰、查询降序、等值不替换”的外部行为。
- 若真正需要等待关闭，应在 `close_event` 上增加带谓词循环的等待 API，处理虚假唤醒，并补并发测试；否则可评估移除当前未消费的条件变量。任何选择都应说明与 Go worker 退出语义的关系。
- 测试必须继续放在独立 `pkg/domain/topn_slow_query_test.rs`，不要嵌入生产文件。建议补充零容量、时间加法溢出、关闭与并发追加竞争、关闭后查询/清理以及锁中毒策略的用例。

## 验证依据

- 源码：`pkg/domain/topn_slow_query.rs`，重点符号 `ExecDetails`、`SlowQueryInfo`、`ShowSlowKind`、`SlowQueryHeap`、`SlowQueryQueue`、`SlowQueryState`、`TopNSlowQueries` 及其公开方法。
- crate 与模块入口：`pkg/domain/Cargo.toml`、`pkg/domain/lib.rs`；目标包没有 `pkg/domain/doc.go`。
- Rust 调用与迁移现状：`pkg/domain/domain.rs` 的独立 `SlowQueryInfo`、`SlowQueryState::add`、`Domain::log_slow_query`、`Domain::show_slow_queries`，以及全仓 `.rs` 精确符号搜索。
- Go 对照及真实主链：`pkg/domain/topn_slow_query.go`、`pkg/domain/domain.go` 的 `slowQuery` 字段、构造位置和 `topNSlowQueryLoop`。
- 测试：`pkg/domain/topn_slow_query_test.rs` 与 `pkg/domain/topn_slow_query_test.go`；覆盖 Top-N 替换、严格过期边界、recent 容量/顺序、用户与内部分类及关闭后拒绝追加。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/domain/topn_slow_query.rs` 返回完整 548 行及符号上下文；`query` 定位了 `TopNSlowQueries`、`SlowQueryHeap`、`push_top_n`、`query_top` 和 `remove_expired`。callers/callees 命令在超时窗口内未产生文本结果，因此调用面结论以精确仓库搜索交叉验证，并未把图的“used by”摘要误当成真实 Rust 调用者。
- 结构验收使用任务指定命令，要求目标文件存在且恰有十一个固定二级标题；本任务为纯文档分析，按计划不运行 Cargo。
