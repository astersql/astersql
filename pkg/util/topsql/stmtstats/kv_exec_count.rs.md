# `pkg/util/topsql/stmtstats/kv_exec_count.rs`

## 文件定位

本文件属于 `astersql-util-topsql-stmtstats` crate，是语句级 TopSQL 统计中的 KV 目标去重计数器实现。crate 入口 `pkg/util/topsql/stmtstats/lib.rs` 以私有模块 `mod kv_exec_count` 装载本文件，并通过 `pub use kv_exec_count::*` 导出其公开类型和方法；`pkg/util/topsql/stmtstats/Cargo.toml` 表明它直接依赖同仓库的 TopSQL 全局状态、执行详情和 reporter metrics crate。

它位于“某次 SQL 执行已取得 SQL/Plan digest”与“把按存储目标统计的执行次数写入 `StatementStats`”之间。当前 Rust 仓库中，生产代码尚未检索到 `CreateKvExecCounter`、`record_target` 或本文件 `intercept` 的调用；现有直接使用者均为 Rust 测试。因此，本文件已经实现计数器语义，但不能据此断言它已像 Go 版本一样接入生产 RPC 主链。

## 核心职责

- `StatementStats::CreateKvExecCounter` 为一次语句执行创建计数器，绑定当前 `StatementStats`，保存 SQL/计划摘要，并初始化空的 target 集合。
- `KvExecCounter::record_target` 仅在 `topsql_state::TopSQLEnabled()` 为真时工作；同一次计数器生命周期内，每个 target 只向统计数据增加一次。
- `KvExecCounter::intercept` 提供 Rust 原生的通用拦截适配：先尝试记录 target，再无条件调用后继闭包，并原样返回其 `Result`。
- 文件只负责“是否应该对这个 target 加一”；实际统计项创建、哈希表累加和语句统计锁由 `StatementStats::add_kv_exec_count` 完成。

这里的计数含义不是 RPC 总数。同一语句对同一 TiKV target 发出多次请求仍只计一次；不同 target 各计一次，这对应 Go 注释中的 “SQL execution count of TiKV” 维度。

## 主要符号

- `StatementStats::CreateKvExecCounter(&self, sql_digest: &[u8], plan_digest: &[u8]) -> KvExecCounter<'_>`：公开构造入口。返回值通过生命周期参数借用 `StatementStats`，因此计数器不能比统计容器活得更久。方法名保留 Go 风格，文件级 `#![allow(non_snake_case)]` 为此关闭命名告警。
- `KvExecCounter<'a>`：公开结构体，但三个字段均为私有：`stats: &'a StatementStats` 是写入目标；`marked: Mutex<HashSet<String>>` 保存本次语句已经计数的 target；`digest: SQLPlanDigest` 拥有 SQL 与计划摘要。
- `KvExecCounter::intercept<T, R, E>`：接收 target、请求值和一次性后继闭包 `FnOnce(&str, T) -> Result<R, E>`。泛型不约束具体 RPC 类型，也不修改成功值或错误值。
- `KvExecCounter::record_target(&self, target: &str)`：公开的核心记账方法。TopSQL 关闭时立即返回；开启时在 `marked` 中插入 target，只有 `HashSet::insert` 返回 `true` 才调用 `add_kv_exec_count(..., 1)`。

本文件没有模块级常量、trait、枚举、条件编译项或自定义错误类型。

## 执行流程

1. 上游以当前语句的 SQL digest、plan digest 调用 `CreateKvExecCounter`。构造函数用 `SQLPlanDigest::new` 复制两段字节，创建空的 `HashSet<String>`，并借用原 `StatementStats`。
2. 每次请求前，调用者可直接调用 `record_target`，或通过 `intercept` 间接调用。
3. `record_target` 首先读取全局 TopSQL 开关。关闭时不加锁、不记 target，也不创建统计项。
4. 开启时获取 `marked` 的互斥锁，把 `target` 复制成 `String` 后插入集合。已存在的 target 令 `first_mark` 为 `false`，流程结束。
5. 首次 target 释放集合锁后，调用 `StatementStats::add_kv_exec_count`。后者获取 `StatementStats.inner` 锁，按 digest 获取或创建 `StatementStatsItem`，延迟创建 `KvExecCount` 映射，并把该 target 的值增加 `1`。
6. 若入口是 `intercept`，无论 TopSQL 是否开启、target 是否重复，都会继续调用 `next(target, request)`，并直接返回后继结果。

关键不变量是：一个 `KvExecCounter` 代表一次语句执行；在它的生命周期内，某个字符串 target 最多触发一次 `+1`。若调用者跨语句复用同一计数器，后续语句会被错误地视为重复 target，因此不应复用。

## 数据与状态

`digest: SQLPlanDigest` 是计数写入的一级键。其两个 `BinaryDigest(Vec<u8>)` 字段在构造时拥有输入摘要的副本，不依赖调用者切片的后续生命周期。空 plan digest 是合法键；Rust 独立测试以 `b""` 覆盖该情况。

`marked` 只保存 target 是否出现，不保存次数；字符串比较区分大小写，空字符串也没有特殊分支。集合在计数器销毁时整体释放，没有显式清理 API。

真正的累计状态位于 `StatementStats`：`add_kv_exec_count` 将数据写入对应 `StatementStatsItem.KvStatsItem.KvExecCount: Option<HashMap<String, u64>>`。首次写入会创建语句项和 KV map；`StatementStats::Take` 可取走这些累计结果。计数器内部没有溢出策略，单个计数器对每个 target 固定只传入 `1`，跨计数器的 `u64` 累加行为由 `add_kv_exec_count` 决定。

## 依赖与调用关系

直接标准库依赖是 `HashSet` 和 `Mutex`。crate 内部依赖为：

- `crate::topsql_state::TopSQLEnabled`：读取全局原子开关；该函数在 `pkg/util/topsql/state/state.rs` 中以 `Ordering::SeqCst` 加载状态。
- `crate::SQLPlanDigest::new`：把输入摘要转换为拥有所有权的 digest 键。
- `crate::StatementStats::add_kv_exec_count`：在 `pkg/util/topsql/stmtstats/stmtstats.rs` 中负责加锁、创建统计项并累加 target。

RustCodeGraph 能确认 `record_target -> TopSQLEnabled`、`record_target -> add_kv_exec_count` 和 `add_kv_exec_count -> get_or_create_statement_stats_item` 的源码关系，但没有为本文件公开入口给出生产调用者。补充的仓库文本检索只找到 `pkg/util/topsql/stmtstats/kv_exec_count_test.rs`、`aggregator_1_aster_unit_test.rs` 和 `pkg/server/tests/commontest/tidb_part2_aster_unit_test.rs` 中的 Rust 调用。

Go 主链则已接线：`pkg/executor/adapter.go` 创建计数器；`pkg/executor/select.go`、`update.go` 与 `pkg/sessiontxn/isolation/base.go` 把 `RPCInterceptor()` 安装到 snapshot/transaction；`pkg/distsql/distsql.go` 也可把它放入请求 context。对应 Rust 生产文件尚未发现等价调用，所以这些 Go 边只能作为待移植参照，不能算作当前 Rust 调用关系。

## 错误处理与边界

本文件没有可恢复的内部错误返回。两把相关锁（本文件的 `marked` 锁，以及下游 `StatementStats.inner` 锁）一旦 poisoned，分别通过 `expect("KvExecCounter mutex poisoned")` 或下游的 `expect("StatementStats mutex poisoned")` 触发 panic。

`intercept` 不捕获、不包装后继错误：`next` 的 `Result<R, E>` 原样返回。记账发生在调用 `next` 之前，因此即使后继返回错误，首次 target 仍已计数；这与 Go RPC interceptor 在调用 `next` 前执行 `mark` 的顺序一致。

TopSQL 关闭时，target 不会进入 `marked`。若同一计数器随后重新开启 TopSQL，该 target 第一次在开启状态下出现时仍会被计数；`aggregator_1_aster_unit_test.rs` 覆盖了这一先关闭、后开启的边界。当前测试还覆盖重复 target、多个 target、空 plan digest 与后继返回值透传；未见 poisoned mutex、并发压力、空 target、后继错误及整数溢出的专项测试。

## 并发与资源生命周期

`record_target(&self)` 使用 `Mutex<HashSet<String>>`，因此多个线程共享同一计数器时，首次插入判定是互斥的，不会因竞争让同一 target 重复加一。`first_mark` 在持有 `marked` 锁时计算，随后锁守卫在调用 `add_kv_exec_count` 前释放，避免同时持有计数器锁与 `StatementStats` 内部锁，降低锁顺序死锁风险。

`StatementStats` 本身以内部 mutex 串行化统计写入，因此不同计数器可以并发写同一语句摘要。计数器只借用 `StatementStats`，没有 `Arc`、后台任务、通道、异步 future 或显式关闭动作；Rust 生命周期在编译期保证统计容器先于计数器存活。是否能跨线程传递由字段的自动 `Send`/`Sync` 推导决定，本文件没有手写 unsafe 并发实现。

每个新 target 会分配一个拥有所有权的 `String`；空间复杂度随一次语句执行触达的唯一 target 数增长。摘要只在构造时复制一次。按设计应在语句结束后丢弃计数器，以同时释放 target 集合并保持“每次执行去重”的语义。

## 与 Go 版本的对应关系

Rust `CreateKvExecCounter`、`KvExecCounter` 的三个状态字段以及“每 target 首次出现才调用 `addKvExecCount(..., 1)`”均直接对应 `pkg/util/topsql/stmtstats/kv_exec_count.go`。`Mutex<HashSet<String>>` 合并表达了 Go 的 `sync.Mutex + map[string]struct{}`；Rust 借用引用替代 Go 的 `*StatementStats`，`SQLPlanDigest` 语义保持一致。

执行顺序也一致：先检查 TopSQL 开关、再去重计数、最后调用后继；后继结果不经转换。Rust 将开关检查放在 `record_target` 内，而 Go 在 `RPCInterceptor` 包装闭包中检查后再调用私有 `mark`，可观察行为相同。

主要差异是接口和接线成熟度。Go 提供绑定 client-go `tikvrpc.Request/Response` 的 `RPCInterceptor() interceptor.RPCInterceptor`，且已有生产调用者。Rust 提供与 RPC 类型无关的泛型 `intercept` 以及公开 `record_target`，没有在本文件中实现 client-go 等价适配；当前仓库证据只证明测试调用，不证明生产请求会经过它。Go 测试直接检查内部 `marked` 和 `stats.data`；Rust 测试通过公开 `Take()` 检查最终统计，并额外验证 `next` 返回值透传。

## 扩展指南

- 若要完成生产接线，应先确定 Rust RPC/事务/snapshot 的真实拦截接口，再在相应所属 crate 中做薄适配并调用本文件 `record_target` 或 `intercept`；不要让 stmtstats crate 反向依赖高层 executor。同步补充独立测试，验证每次语句都创建新计数器、失败请求是否仍计数、事务与 snapshot 路径均安装适配器。
- 若改变去重键（例如 target 加 store type、region 或请求类别），修改点是 `marked` 的键类型和 `record_target` 入参；必须评估字符串分配、集合基数、指标兼容性，并同步 Rust `kv_exec_count_test.rs` 与 Go 对照测试意图。
- 若要消除锁 poison panic，应统一审视 `StatementStats` 的锁错误策略，不能只改本文件而留下下游 panic。任何锁结构调整都应增加多线程竞争测试，并继续保证调用下游统计方法前释放 `marked` 锁。
- 若改变 TopSQL 动态开关语义，需明确关闭期间出现的 target 在重新开启后是否应计数；现状是不在关闭期间标记，因此重开后会计数。
- 测试逻辑应继续放在独立的 `pkg/util/topsql/stmtstats/kv_exec_count_test.rs`，不要内嵌回生产源文件；跨模块接线可在相应调用方的独立测试中覆盖。

## 验证依据

- 目标源码：`pkg/util/topsql/stmtstats/kv_exec_count.rs`，共 84 行；RustCodeGraph `node --file` 核对了 `CreateKvExecCounter`、`KvExecCounter`、`intercept`、`record_target` 的完整实现。
- crate 边界：`pkg/util/topsql/stmtstats/Cargo.toml` 与 `pkg/util/topsql/stmtstats/lib.rs`，核对 crate 名、三个路径依赖、模块装载及公开重导出。
- 下游状态：RustCodeGraph `node stmtstats.rs::add_kv_exec_count`、`node stmtstats.rs::SQLPlanDigest`、`node stmtstats.rs::BinaryDigest`，核对 digest 所有权、统计锁、条目创建和 `u64` 累加。
- 全局开关：RustCodeGraph `node pkg/util/topsql/state/state.rs::TopSQLEnabled`，核对顺序一致原子读取。
- Rust 测试：`pkg/util/topsql/stmtstats/kv_exec_count_test.rs`、`aggregator_1_aster_unit_test.rs`、`pkg/server/tests/commontest/tidb_part2_aster_unit_test.rs`，核对单 target 去重、多 target、开关切换、摘要键与后继结果透传。
- Go 对照：`pkg/util/topsql/stmtstats/kv_exec_count.go`、`kv_exec_count_test.go`；并以仓库检索确认 `pkg/executor/adapter.go`、`select.go`、`update.go`、`pkg/sessiontxn/isolation/base.go` 与 `pkg/distsql/distsql.go` 的生产接线。
- 调用边限制：RustCodeGraph 对精确 Rust 符号的 callers 查询未返回生产调用者；`rg` 的 Rust 调用检索也只命中上述测试。故文档将 Rust 生产接线明确标为未发现，而未从 Go 调用关系推断 Rust 已接线。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前仅执行固定 11 章节结构检查，并人工复核源码链接、事实边界和扩展建议。
