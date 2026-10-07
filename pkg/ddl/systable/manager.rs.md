# `pkg/ddl/systable/manager.rs`

## 文件定位

本文件是 `astersql-ddl-systable` crate 的 DDL 系统表只读访问层。模块入口 [`lib.rs`](./lib.rs) 公开重导出这里的类型和函数；crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义，运行时依赖只有 `astersql-meta-model`。它把上层的“按作业查询”请求转换为对 `mysql.tidb_ddl_job` 和 `mysql.tidb_mdl_info` 的内部 SQL，并把具体会话实现隔离在 `Session`/`SessionPool` trait 后面。

从 DDL 生命周期看，这不是创建作业、状态迁移或回填执行器，而是持久化作业元数据的查询门面。当前 Rust 生产接线中，`pkg/session/runtime/crossks_runtime.rs` 和 `pkg/session/runtime/normal_ddl_submit.rs` 构造 manager，主要用于最小 job ID 刷新和提交前的 FLASHBACK CLUSTER 冲突检查；Go 版本还在调度器、worker 和 reorg 路径使用同类接口。该文件本身不更新 schema version，不执行 job，也不管理 DDL owner。

## 核心职责

- `new_manager` 用一个共享会话池构造线程安全的 `Arc<dyn Manager>`，隐藏具体实现 `SystemTableManager`。
- `get_job_by_id`/`get_job_bytes_by_id_with_session` 从 `mysql.tidb_ddl_job.job_meta` 读取持久化字节，并用 `meta_model::group_3::Job::decode` 还原作业，同时保留原始字节到 `JobWrapper`。
- `get_mdl_version` 从 `mysql.tidb_mdl_info` 读取指定作业的 MDL（Metadata Lock）schema version。
- `get_min_job_id` 在给定下界之上查询仍存在作业的最小 ID，为 `MinJobIdRefresher` 的单调缓存提供数据。
- `has_flashback_cluster_job` 在同一下界窗口内判断是否存在类型值为 `ACTION_FLASHBACK_CLUSTER` 的作业，供新 DDL 提交前的互斥检查使用。
- `with_new_session` 统一负责池化会话的借出、调用和归还，确保业务闭包成功或返回 `Error` 时都会执行 `put`。

## 主要符号

- `Context { request_id: String }`：传给内部 SQL 执行器的请求上下文。目前 manager 不读取字段，由具体 `Session::execute` 决定是否使用。
- `Error::{NotFound, Pool, Execute, Decode}`：分别表示缺行、借池失败、SQL 执行失败和列/作业反序列化失败。`Display` 对后三类直接展示内部消息，`NotFound` 固定显示 `not found`。
- `Value::{Null, Int, Bytes}` 与 `Row(Vec<Value>)`：最小查询结果模型。`Row::bytes` 和 `Row::int64` 做按下标取值及类型检查。
- `Session: Send`：一条可变内部会话；核心方法为 `execute(&mut self, &Context, sql, label) -> Result<Vec<Row>, Error>`。
- `SessionPool: Send + Sync`：会话借还边界；`get` 返回独占的 `Box<dyn Session>`，`put` 接回所有权。
- `Manager: Send + Sync`：对外查询契约，包含五个方法；其中 `get_job_bytes_by_id_with_session` 允许调用者复用已有会话。
- `SystemTableManager { session_pool: Arc<dyn SessionPool> }`：唯一生产实现，公开类型但字段私有。
- `new_manager`：公共构造入口，返回 trait object，避免调用者依赖实现类型。
- `with_new_session<T>`：私有泛型辅助函数，是所有自行借会话查询的资源生命周期边界。
- `ACTION_FLASHBACK_CLUSTER`、`Job`、`JobWrapper`：从 `meta_model::group_3` 重导出；常量是系统表 `type` 列的持久化协议值，测试固定验证为 `62`。

## 执行流程

1. 调用者持有 `Arc<dyn Manager>`。除显式传入会话的字节查询外，入口先调用 `with_new_session`。
2. `with_new_session` 调用 `SessionPool::get`；借出失败立即返回池错误。借出成功后执行闭包，将闭包结果暂存，再无条件把会话交还 `SessionPool::put`，最后返回原结果。
3. `get_job_by_id` 复用同一借出会话调用 `get_job_bytes_by_id_with_session`。后者执行 `select job_meta ... where job_id = N`，空结果映射为 `NotFound`；首列必须是 bytes 或 null/缺失。随后 `Job::decode` 解析 JSON/协议字节，失败映射为 `Decode`，成功时 `new_job_w(job, bytes)` 同时保存结构化作业和原始载荷。
4. `get_mdl_version` 执行 `select version ... where job_id = N`；没有首行是 `NotFound`，有行则由 `Row::int64(0)` 读取版本。
5. `get_min_job_id` 执行带 `job_id >= previous_min_job_id` 的聚合查询。首行存在时读取第一列；没有行时返回 `0`。相邻的 `MinJobIdRefresher::refresh` 再以 `fetch_max` 使用结果，因此表清空返回 `0` 不会让缓存倒退。
6. `has_flashback_cluster_job` 执行带最小 ID 和 `type = ACTION_FLASHBACK_CLUSTER` 的 `count(1)` 查询；首行计数大于零才返回 `true`，无行返回 `false`。
7. Rust 主链中，`pkg/session/runtime/system_session.rs::SubmitGuard` 与 `pkg/session/runtime/crossks_session_pool.rs::CrossKSFlashbackGuard` 把本 trait 适配为 `astersql-ddl-jobsubmit::SystemTableManager`；`submit_batch` 在插入新作业前先调用它，发现 flashback 作业便拒绝提交。

## 数据与状态

`SystemTableManager` 自身只有一个不可变的 `Arc<dyn SessionPool>`，不缓存作业、MDL version 或查询结果。持久状态全部在系统表：`mysql.tidb_ddl_job` 提供 `job_id`、`job_meta`、`type`，`mysql.tidb_mdl_info` 提供 `job_id`、`version`。

`job_meta` 被完整复制为 `Vec<u8>`，解码后的 `Job` 与原字节一起放入 `JobWrapper`，因此后续代码可以同时使用语义字段和持久化快照。`previous_min_job_id`/`min_job_id` 是包含下界（SQL 使用 `>=`），是避免从头扫描以及缩小 flashback 检查窗口的重要契约。

`Row` 对 null 或缺失列采用 Go 行读取器风格的零值语义：bytes 返回空向量，整数返回 `0`；但实际类型不匹配会返回 `Decode`。这意味着“查询没有行”与“存在一行但列为空”在不同入口可能分别表现为 `NotFound` 和零值，扩展时不能混用。

## 依赖与调用关系

下游依赖只有两类。第一类是 `meta_model::group_3`：`Job::decode`、`new_job_w` 和 `ACTION_FLASHBACK_CLUSTER` 定义持久化 job 的兼容协议。第二类是由调用方注入的 `SessionPool`/`Session`，负责真实连接、内部 SQL 执行和错误映射；本 crate 不依赖具体数据库客户端。

已由 RustCodeGraph 确认的本文件定义包括 `SystemTableManager`（`manager.rs:116`）和 `new_manager`（`manager.rs:121`），目标文件共索引 27 个符号。图索引没有解析出 `impl Manager` 内各 trait 方法的完整调用边，因此生产调用关系进一步以源码搜索核验：

- `pkg/ddl/systable/min_job_id.rs::MinJobIdRefresher::refresh` 调用 `get_min_job_id`。
- `pkg/session/runtime/system_session.rs::SubmitGuard` 和 `pkg/session/runtime/crossks_session_pool.rs::CrossKSFlashbackGuard` 调用 `has_flashback_cluster_job`。
- `pkg/session/runtime/crossks_runtime.rs`、`normal_ddl_submit.rs`、`session_factory.rs` 构造 manager，并将其接入刷新器/提交选项。
- `pkg/ddl/jobsubmit/submit.rs::submit_batch` 经适配 trait 间接触发 flashback 查询。
- 在当前 Rust 生产源中未检索到 `get_job_by_id`、`get_job_bytes_by_id_with_session`、`get_mdl_version` 的非适配/测试调用；它们已有实现与测试，但不能据此宣称已经覆盖 Go 调度器和 worker 的全部接线。

## 错误处理与边界

会话池 `get` 的错误原样向上传播；SQL 错误由具体 `Session` 返回，manager 不重试、不包装。`with_new_session` 在闭包返回任何普通 `Result` 后都会归还会话，但 Rust panic 会跳过显式 `put`；trait 没有 RAII lease 契约，因此具体池若要求 panic 安全，应在自己的会话包装器中以 `Drop` 保证释放。

按 ID 查询 job/MDL 时，空结果明确为 `Error::NotFound`，符合多 owner 场景下作业可能已被另一 owner 取走/删除的 Go 语义。聚合/计数查询则将无行视为 `0`/`false`。错误列类型是 `Decode`；格式错误的 `job_meta` 也为 `Decode`，`manager_test.rs::job_decode_rejects_malformed_json_like_go` 证明非法 JSON 不会被宽松接受。

SQL 中只插入强类型 `i64` 和编译期常量，不接收自由文本，因此当前格式化位置不存在字符串转义入口。查询只取首行，不检查意外的重复行；系统表键约束和聚合语义是这里依赖的外部不变量。

## 并发与资源生命周期

`Manager` 和 `SessionPool` 都要求 `Send + Sync`，并通过 `Arc` 在执行线程间共享；`Session` 只要求 `Send`，每次 `get` 后以独占 `Box` 和 `&mut dyn Session` 使用，不在本文件内并发共享同一会话。manager 没有锁、后台任务或事务状态。

每个 `get_job_by_id`、`get_mdl_version`、`get_min_job_id`、`has_flashback_cluster_job` 调用各借一次会话并归还一次。`get_job_bytes_by_id_with_session` 是例外：会话生命周期由调用者管理，便于 worker 在已有会话/事务上下文中查询。该函数绝不能自行归还传入会话。

后台刷新和取消生命周期属于相邻的 `min_job_id.rs`，不在本文件实现；它周期调用这里的查询接口，并以原子变量保证缓存单调。系统表并发变化可能让一次查询立即过时，本层只提供单次读取，不承诺快照跨调用一致性。

## 与 Go 版本的对应关系

直接对照文件是 [`manager.go`](./manager.go)，Rust 的 `Manager`、`manager`/`SystemTableManager`、`NewManager`/`new_manager`、`withNewSession`/`with_new_session` 和五个查询方法逐项对应，SQL 文本和 label（`get-job-by-id`、`check-mdl-info`、`get-min-job-id`、`has-flashback-cluster-job`）保持一致。

语义一致点包括：缺失 job/MDL 返回 not found；job 解码后保留原字节；最小 ID 和 flashback 查询使用包含下界；无聚合结果返回零值；借出的会话在普通成功/错误路径归还。Rust 用 trait 注入替代 Go 的具体 `session.Pool`，用 `Arc<dyn Manager>` 替代接口值，并把 Go `errors.Trace` 的错误链简化为本地 `Error` 枚举。

需要注意的差异是 Go `withNewSession` 会用 `defer` 归还资源，panic 时仍执行，而 Rust 当前是显式 `put`；此外 Rust 的 `Context` 只是轻量 `request_id`，没有完整等价于 Go `context.Context` 的取消、截止期和值传播。Go 生产代码在 `ddl.go`、`job_scheduler.go`、`job_worker.go`、`reorg.go`、`index.go` 有更广泛调用；当前 Rust 搜索证据只支持前述最小 ID/flashback 主链以及若干会话运行时构造，不能把 Go 全部调用点视为已经移植。

## 扩展指南

新增系统表查询时，应先在 `Manager` 增加最小接口，再在 `SystemTableManager` 实现：需要独立查询就通过 `with_new_session`，需要复用事务/会话则仿照 `get_job_bytes_by_id_with_session` 接受 `&mut dyn Session`。同时扩展 `manager_test.rs` 中独立的 `MockSession` SQL 分支和测试，不要把测试放回生产源文件。

新增列类型时需同时更新 `Value`、`Row` 取值器和类型错误测试；不要静默把类型错误转为零值。修改 job 类型或序列化必须先核对 `meta-model` 和 Go `pkg/meta/model` 的持久化协议，并保留 `ACTION_FLASHBACK_CLUSTER == 62` 等跨语言兼容约束。

若要补齐 Go 调度器/worker 接线，应在各自 crate 增加适配与调用，而不是把调度、重试、事务或 owner 逻辑塞进 manager。性能方面应保留 `job_id >= previous_min_job_id` 下界，避免退回全表扫描；新增高频查询需检查系统表索引和会话池压力。若改变借还机制，必须验证成功、SQL 错误、解码错误和（如要求）panic 四种路径的资源释放。

最直接的同步测试是 [`manager_test.rs`](./manager_test.rs)；跨模块行为还应覆盖 `min_job_id_test.rs`、`pkg/session/runtime/*ddl*test.rs` 或 jobsubmit 测试。Go 语义变化则应同步核对 [`manager_test.go`](./manager_test.go)。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/ddl/systable` 找到本 crate 的 Rust/Go 源与测试；`node --file pkg/ddl/systable/manager.rs` 读取完整 196 行；`query`/`node` 确认 `SystemTableManager` 和 `new_manager` 定义。trait impl 方法调用边未由图返回，已明确使用源码搜索补证。
- 生产源与边界：`pkg/ddl/systable/manager.rs`、`lib.rs`、`Cargo.toml`、`min_job_id.rs`，以及 `pkg/ddl/jobsubmit/submit.rs`、`types.rs`、`pkg/session/runtime/system_session.rs`、`crossks_session_pool.rs`、`crossks_runtime.rs`、`normal_ddl_submit.rs`。
- Go 对照：`pkg/ddl/systable/manager.go`；调用面以 `pkg/ddl/ddl.go`、`job_scheduler.go`、`job_worker.go`、`reorg.go` 和 `index.go` 的搜索结果交叉核验。
- 测试证据：`pkg/ddl/systable/manager_test.rs` 覆盖缺失/存在 job、完整 job 字段和原字节、缺失/存在 MDL version、最小 ID 的包含下界与空表、flashback 类型和下界、协议常量以及非法 JSON；`manager_test.go` 提供真实 Go 行为基线。
- 人工复核结论：该文件存在是为了把 DDL 系统表读取与具体会话实现解耦；运行时以“一次借会话—执行单条查询—归还”的方式工作；安全扩展的关键是保持持久化协议、包含下界、错误分类和池资源生命周期，并在独立测试文件同步覆盖。
