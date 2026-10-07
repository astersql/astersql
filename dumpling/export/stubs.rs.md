# `dumpling/export/stubs.rs`

## 文件定位

[`stubs.rs`](stubs.rs) 是 `astersql-dumpling-export` crate 的本地兼容层。crate 入口 [`lib.rs`](lib.rs) 以私有 `mod stubs` 装载它，再通过 `pub use stubs::*` 把其中的公开符号提升到 crate 根；其余 `include!` 进来的 export 实现因此可以像 Go 同包代码一样直接使用这些符号。

该文件不是独立业务模块，也没有同路径的 Go `stubs.go`。它把 Go 版 dumpling 依赖的多个真实组件——`database/sql`、MySQL driver、PingCAP errors/dbutil/utils、table-filter、外部存储、Prometheus、容量单位、`text/template`、HTTP 服务、etcd 与 PD 客户端——收敛成 arm64/无 CGO 环境可编译、可注入、可观察的轻量替身。它服务于当前 Rust export 实现和独立测试，但源码头部已明确标注“非生产实现”，不能视为真实数据库、对象存储或控制面的等价实现。

[`Cargo.toml`](Cargo.toml) 将本目录定义为 library crate，入口为 `lib.rs`；直接依赖中只有 `astersql-objstore-storeapi` 被本文件用于 `WriterOption`、`ErrExceedMaxUploadParts` 与错误转换，其余 SQL、HTTP、etcd、PD 和 Prometheus 能力都没有引入对应外部 Rust SDK，而是由本文件自行模拟。

## 核心职责

本文件承担八组兼容职责：

1. 统一错误与版本模型：`Error`/`Result`、errors 风格辅助函数、`MySQLError`、`ServerType`、`SemVer`、`ServerInfo` 和 `ParseServerInfo`。
2. 提供脚本化 SQL 层：`RawBytes`、`ColumnType`、`Rows`、`SqlResult`、`Conn`、`DB` 与 `openDB`，让调用方无需真实 MySQL 即可驱动查询、执行、Ping、失败和关闭路径。
3. 提供控制流辅助：`BackoffStrategy`/`WithRetry`、`Filter`/`PatternFilter`、容量解析、failpoint 判定与可重试错误分类。
4. 提供内存外部存储：`Storage`、`ObjectWriter`、`MemStorage`、`MemWriter`，覆盖整文件读写与“写入后 Close 才提交”的流式路径。
5. 提供简化指标系统：counter/gauge/histogram、向量、factory、registry 与进程级 `DefaultGatherer`。
6. 提供导出路径模板：`OutputTemplate` 和 `filename_escape`，支持 dumpling 当前使用的少量命名模板与对象名转义。
7. 提供服务/控制面句柄：`HttpServiceHandle`、`EtcdClient`、`PdClient`、`mockGCStatesClient`，记录状态、参数与注入错误，不执行真实网络操作。
8. 提供常量和空操作适配：压缩类型、Parquet 列信息与默认值、BR summary no-op、`multierr_combine`。

其设计重点是“测试可控性和移植接线”，不是完整复刻依赖包。诸如 SQL 解析、连接池调度、Prometheus 标签/分桶、Go template 语法、etcd 一致性和 PD RPC 都被有意缩减。

## 主要符号

- `Error` 与 `Result<T>`：crate 内统一错误面。`Error` 保存文本、可选 `MySQLError` 和“上传分片数超限”标志；`errors_annotate`/`errors_annotatef` 添加上下文时保留后两者。`PartialEq` 只比较 `msg`，而 `source()` 仅为 `exceed_upload_parts` 暴露 `ErrExceedMaxUploadParts`。
- `ServerType`、`SemVer`、`ServerInfo`、`ParseServerInfo`：将版本文本粗略分类为 TiDB、MariaDB、MySQL 或 Unknown，并提取首个至少含两个点的数字段。`SemVer::LessThan` 只比较三元组，不比较 prerelease。
- `Rows`：内存二维结果集和单向游标。`Next` 推进 `idx`，`Scan` 要求游标有效且目标数等于列数，`Close` 翻转状态并消费一次 `close_error`，`Err` 返回行级错误的克隆。
- `Conn`：共享脚本响应、列定义与失败队列的连接替身。`QueryContext` 优先消费错误，再消费精确的 `row_responses`，最后从 SQL 文本映射取数据；`ExecContext` 总是先记录 SQL；`PingContext` 检查注入错误和关闭状态。
- `DB`：连接工厂。每次 `Conn` 分配递增 ID，脚本和失败队列跨连接共享，但执行日志、Ping 错误和 `row_responses` 属于新连接。`DB::Close` 只阻止后续建连，不追踪或关闭已经发出的 `Conn`。
- `BackoffStrategy` 与 `WithRetry`：在回调成功、取消或重试次数耗尽前循环；它调用 `NextBackoff` 获取时长但不实际 sleep，因此是控制流替身。
- `Filter`、`CaseInsensitiveFilter`、`PatternFilter`、`filter_parse`：支持 `db.table`、`*` 和 schema deny 的小子集；`!/` 固定展开为系统库排除列表。
- `Storage`/`ObjectWriter`、`MemStorage`/`MemWriter`：以内存 `HashMap<String, Vec<u8>>` 模拟对象存储；`CreateWithOptions` 忽略 writer option，`MemWriter::Close` 才把缓冲复制进 map。
- `Counter`、`Gauge`、`Histogram` 及 `*Vec`：以 `AtomicU64` 保存 `f64` 位模式。Counter 和 Histogram 使用 CAS；Gauge 的读后写不是原子复合操作。向量的 `With` 忽略全部标签。
- `Factory`、`Registry`、`DefaultRegistry`、`DefaultGatherer`：创建指标并按名字注册可抓取回调。默认 gatherer 由 `OnceLock` 初始化，是进程级共享状态；抓取前按名字排序以保持输出稳定。
- `RAMInBytes`/`HumanSize`：在二进制和十进制容量后缀间转换。解析使用 `f64` 后截断为 `i64`，空串返回 0。
- `OutputTemplate`：内置 schema/table/view/sequence/data/placement-policy 模板；`Parse` 只识别 data define 或把无 define 的文本当 data 模式，`Execute` 只做固定占位符替换。
- `HttpServiceHandle`、`EtcdClient`、`PdClient`：分别记录 HTTP 生命周期、内存前缀 KV 读取、GC safe point/barrier 调用；它们都不建立真实网络连接。

文件中绝大多数符号是 `pub`，原因是 `lib.rs` 的再导出与 Go 单包迁移模型；明确的内部实现包括 `extract_version_nums`、`CaseInsensitiveFilter`、`fn_escape`，以及若干指标内部读写方法。

## 执行流程

典型导出接线如下：

1. [`config.rs`](config.rs) 的 `DefaultConfig` 创建 `OutputTemplate`、默认 filter、指标 factory/registry；`Config::createExternalStorage` 创建 `MemStorage`，容量选项由 `RAMInBytes` 解析。
2. [`dump.rs`](dump.rs) 的初始化路径通过 `openDB` 得到 `DB`，查询版本后调用 `ParseServerInfo`，再按配置保存 HTTP、storage、DB 和 PD 句柄。当前 `openDB` 忽略 DSN，只返回空脚本库。
3. [`conn.rs`](conn.rs) 和 [`sql.rs`](sql.rs) 通过 `DB::Conn`、`Conn::QueryContext`/`ExecContext`、`Rows::Next`/`Scan`/`Close` 执行脚本化 SQL；`WithRetry` 为查询和执行提供重试控制。
4. [`prepare.rs`](prepare.rs) 与 [`writer.rs`](writer.rs) 使用 `OutputTemplate` 生成相对路径；writer 经 `Storage::Create` 写入缓冲，在 `ObjectWriter::Close` 时提交到 `MemStorage`。
5. [`metrics.rs`](metrics.rs) 从 `Factory` 创建指标并注册到 `Registry`；[`http_handler.rs`](http_handler.rs) 优先抓取配置 registry，若 registry 只支持注册而不能 gather，则回退到 `DefaultGatherer`。
6. [`util.rs`](util.rs) 用 `EtcdClient::GetPrefixWithTimeout` 获取 DDL ID；[`dump.rs`](dump.rs) 用 `PdClient` 记录 GC safe point/barrier 设置和清理。所有交互仍是进程内状态变化。

脚本化查询的细化顺序是：`Conn::QueryContext` 先从 `fail_queue` 或 `fail_query` 取得错误；无错时优先按完整 SQL 消费 `row_responses`；否则 `lookup_scripted` 先精确匹配，再做包含匹配，均未命中则返回名为 `col` 的空结果集。每个 seed 队列按 FIFO 消费，适合表达“首次失败/错误行，重建后成功”等测试场景。

## 数据与状态

- 共享所有权：SQL 脚本、列、错误队列、内存文件、registry、etcd KV、PD 记录均以 `Arc` 共享；可变集合使用 `Mutex`，布尔/计数状态使用原子类型。
- 消费型状态：`VecDeque` 保存多次 SQL 响应和失败，查询时 `pop_front`；`Rows::close_error` 和 `Conn::fail_queue` 也只消费一次。
- 生命周期状态：`Rows.closed` 是普通布尔值，因为 `Rows` 按可变借用使用；`Conn.closed`、`DB.closed`、HTTP started/stopped、PD closed 是共享原子标志。
- 模板状态：`OutputTemplate` 维护原始 `text` 与实际执行使用的 `defines`；Clone 是深拷贝，默认值不依赖全局可变状态。
- 指标状态：克隆指标会共享底层原子值；`DefaultRegistry` 同时维护名称列表和采样回调 map。重复 `MustRegister` 可产生重复名称，而 map 中同名 sample 会被覆盖。
- 全局状态：`DefaultGatherer` 的 `OnceLock<Arc<DefaultRegistry>>` 持续整个进程；`failpoint_inject` 每次读取 `GO_FAILPOINTS` 环境变量，当前只识别 `EnableLogProgress` 的一个精确条目。

重要不变量包括：`Scan` 目的列数必须严格等于结果列数；`MemWriter` 在 `Close` 前不可被 storage 读到；关闭的 `Conn` 不能 Ping，关闭的 `DB` 不能再创建连接；PD/GC 调用先增加计数或记录参数，再返回注入错误，因此失败也可被测试观察。

## 依赖与调用关系

上游生产调用证据：

- [`dump.rs`](dump.rs) 调用 `openDB`、`ParseServerInfo`、`DB::Conn`、`PdClient::UpdateServiceGCSafePoint`，并持有 `DB`、`HttpServiceHandle`、`PdClient`。
- [`config.rs`](config.rs) 调用 `filter_parse`、`RAMInBytes`、`NewDefaultFactory`、`NewDefaultRegistry`，并创建 `MemStorage` 和 `OutputTemplate`。
- [`conn.rs`](conn.rs) 两条 SQL 路径调用 `WithRetry`；[`retry.rs`](retry.rs) 使用 `IsRetryableError` 决定 MySQL 错误是否继续重试。
- [`writer.rs`](writer.rs) 用 `Conn`、`Storage` 与 `OutputTemplate::Execute` 完成文件输出；[`ir.rs`](ir.rs)、[`ir_impl.rs`](ir_impl.rs)、[`sql.rs`](sql.rs) 直接消费 `Rows`。
- [`http_handler.rs`](http_handler.rs) 使用 `DefaultGatherer` 与 `HttpServiceHandle`；[`util.rs`](util.rs) 使用 `EtcdClient`。

下游依赖主要来自标准库：collections、格式化、`Arc`/`Mutex`、原子类型和 `Duration`。唯一直接的工作区依赖是 `astersql_objstore_storeapi`：`Storage::CreateWithOptions` 接受其 `WriterOption`，`Error` 可由 `ExceedMaxUploadParts` 转换，并通过 `source()` 暴露其哨兵错误。

RustCodeGraph 已索引目标文件并识别 203 个符号；`node --file` 显示该文件被 33 个索引文件使用。对 `ParseServerInfo`、`WithRetry`、`filter_parse`、`IsRetryableError`、`openDB`、`DefaultGatherer`、`filename_escape`、`RAMInBytes` 执行带 `--file` 的 callers 查询没有返回边，原因是调用者通过 `lib.rs` 再导出和 `include!` 形成 crate 根单包视图，图没有把这些非限定调用回连到私有模块定义。因此上述调用关系以精确文本引用和调用点源码为补充证据，不把空图结果解释为“无人调用”。

## 错误处理与边界

`Error` 是简化错误容器，不构建通用 cause 链：`errors_trace` 原样返回，`errors_cause` 返回自身，只有上传分片超限能通过标准 `source()` 下钻。annotate 会保留 MySQL 错误码和上传分片标志，这是 [`retry.rs`](retry.rs) 仍能正确分类带上下文错误的前提。

SQL 层的显式错误包括无效游标、Scan 参数数不匹配、关闭连接 Ping、关闭 DB 建连、注入的查询/执行/Ping/行/Close 错误。未 seed 的查询返回空集而不是错误；`ExecContext` 默认成功且 `rows_affected` 为 0。模糊 SQL 匹配可能把相似语句关联到同一脚本，测试应优先用完整 SQL 或 `seed_rows` 消除歧义。

其他关键边界：

- `ParseServerInfo` 只做文本包含和数字扫描，不验证完整厂商格式；畸形数字分量按 0 处理，prerelease 不参与排序。
- `WithRetry` 不等待 `NextBackoff` 返回的时长，也不重新读取取消状态；`done` 是调用时传入的静态布尔值。
- filter 只支持简单 `*`，不是完整 table-filter 语法；大小写包装只小写输入，不重写已有 pattern。
- `Gauge::Add/Sub` 不是 CAS，多个线程同时复合更新可能丢增量；Histogram 只有 sum，没有 bucket/count 语义；向量忽略标签。
- `RAMInBytes` 的浮点乘法可能截断并缺少溢出检查；`HumanSize` 只输出到 GB。
- `OutputTemplate::Parse` 不解析一般 Go template AST，未知 template 名会把名字本身当模板；`Execute` 不处理条件、循环或任意函数。
- `HttpServiceHandle::start` 仅以地址是否含 `invalid` 判断失败；etcd/PD 不提供超时、租约、网络重试、鉴权或一致性保证。
- 所有 `Mutex::lock()` 都直接 `unwrap()`；一旦锁中毒会 panic，而不是转换为 `Error`。

## 并发与资源生命周期

`Conn`/`DB`/storage/registry/etcd/PD 的克隆通过 `Arc` 观察同一底层状态。SQL 响应和文件 map 受 `Mutex` 保护；ID、调用次数和关闭标志使用 `SeqCst`，便于测试稳定观测。Counter 与 Histogram 的浮点累加使用 CAS 避免 lost update；Gauge 是例外，其 `load + store` 只保证单次原子读写，不保证并发加减的整体原子性。

资源关闭语义是显式而简化的：`Rows::Close`、`Conn::Close`、`DB::Close`、`MemWriter::Close`、`HttpServiceHandle::stop`、`PdClient::Close` 只改变进程内状态，没有 Drop 兜底，也没有网络 socket、线程或文件描述符。调用方必须显式 Close/stop；尤其 `MemWriter` 若未 Close，缓冲不会提交。`DefaultGatherer` 是进程全局共享资源，测试注册的 sample 必须 Unregister，避免跨用例泄漏。

该文件本身不创建线程、channel 或异步任务。真实 HTTP serving thread 位于 [`http_handler.rs`](http_handler.rs)，本文件只提供其共享停止标志；真实导出任务并发也在其他实现文件中，不由这些 stubs 调度。

## 与 Go 版本的对应关系

Go `dumpling/export` 没有与本文件一一对应的 `stubs.go`。对应关系是多源汇聚：

- [`conn.go`](conn.go) 使用真实 `database/sql` 和 `utils.WithRetry`；Rust 以 `DB`/`Conn`/`Rows` 与 `WithRetry` 替代。
- [`dump.go`](dump.go) 使用 `version.ParseServerInfo`、真实 SQL driver、PD client 与 GC safe point API；Rust 以简化版本解析和记录式 `PdClient` 替代。
- [`config.go`](config.go) 使用 `pkg/util/table-filter`、Prometheus、units 和外部存储配置；Rust 在本文件内提供其最小子集。
- [`prepare.go`](prepare.go) 使用 Go `text/template` 构造默认输出模板；Rust `OutputTemplate` 仅执行 dumpling 当前需要的 define 和占位符替换。
- [`http_handler.go`](http_handler.go) 使用 Prometheus gatherer/promhttp 和真实监听；Rust 的指标对象、registry 与 handle 是内存实现，实际轻量 HTTP 循环另在 `http_handler.rs`。
- [`util.go`](util.go) 使用 etcd v3 client；Rust `EtcdClient` 对共享 map 做前缀筛选并记录 timeout。

因此“对齐”主要体现在公开命名、调用形状、关键错误分支和测试可观察副作用，不意味着协议、性能、语法或并发行为完全等价。独立 Rust 测试继续与同名 Go 测试意图对照，但生产化时应优先替换为对应工作区 crate 或带发布 tag 的上游依赖，而不是继续扩张这个聚合桩。

## 扩展指南

扩展前先判断能力是否仍属于离线测试替身。若要支持真实 SQL、对象存储、Prometheus、etcd 或 PD，最安全的接入点是把调用方改接 canonical crate，并保留本文件实现为 trait/mock；不要在此逐步重造完整客户端。

- SQL 行为：修改 `Rows`、`Conn`、`DB` 时同步 [`conn_test.rs`](conn_test.rs)、[`sql_test.rs`](sql_test.rs)、[`ir_test.rs`](ir_test.rs)、[`ir_impl_test.rs`](ir_impl_test.rs)，并保持测试逻辑独立于源文件。新增响应类型应明确 FIFO、共享范围、错误消费和 Close 语义。
- 重试/错误：修改 `Error`、`WithRetry`、`IsRetryableError` 时同步 [`parity_test.rs`](parity_test.rs)、[`conn_test.rs`](conn_test.rs) 与 [`sql_test.rs`](sql_test.rs)，尤其验证 annotate 后的 MySQL 码、最终错误和 reset/rebuild 次序。
- 存储/模板：修改 `Storage` 或 `OutputTemplate` 时同步 [`writer_test.rs`](writer_test.rs)、[`config_test.rs`](config_test.rs)、[`prepare_test.rs`](prepare_test.rs)；警惕未 Close 数据、同名覆盖、路径转义和分片文件名碰撞。
- 指标/HTTP：修改 factory/registry/gatherer 时同步 [`metrics_test.rs`](metrics_test.rs) 与 [`http_handler_test.rs`](http_handler_test.rs)，并保证全局 sample 清理。若提升并发保证，应先把 Gauge 改为 CAS 更新。
- etcd/PD：修改 `EtcdClient`、`PdClient`、`mockGCStatesClient` 时同步 [`util_test.rs`](util_test.rs)、[`dump_test.rs`](dump_test.rs) 和 [`util_for_test.rs`](util_for_test.rs)，覆盖失败时的调用计数、参数记录及清理。

兼容风险集中在 Go API 形状和错误文本，因为大量迁移代码直接使用 Go 风格方法名；性能风险集中在全局 `SeqCst`、粗粒度 Mutex、克隆完整 rows/files 和 registry 抓取时复制回调 map。若新增非桩生产文件，按仓库规则保留/添加相应版权头；本文件已有 `// Copyright 2026 AsterSQL.`，不应删除。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter dumpling/export` 确认目标与 crate 内实现/测试集合；三段 `node --file dumpling/export/stubs.rs` 读取全部 1578 行并报告 203 个符号、33 个使用文件；对八个关键入口执行精确 callers 查询，记录了 `include!`/再导出场景下的空边限制。
- 源码与配置：完整阅读 [`stubs.rs`](stubs.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)，并核验生产调用点 [`dump.rs`](dump.rs)、[`config.rs`](config.rs)、[`conn.rs`](conn.rs)、[`retry.rs`](retry.rs)、[`writer.rs`](writer.rs)、[`http_handler.rs`](http_handler.rs)、[`util.rs`](util.rs)。
- Go 对照：核验 [`conn.go`](conn.go)、[`dump.go`](dump.go)、[`config.go`](config.go)、[`prepare.go`](prepare.go)、[`http_handler.go`](http_handler.go)、[`metrics.go`](metrics.go)、[`util.go`](util.go) 中使用的真实依赖和调用路径；确认不存在同路径 `stubs.go`。
- 独立 Rust 测试：核验 [`parity_test.rs`](parity_test.rs) 的版本/错误/存储/连接/HTTP/指标资源契约，[`conn_test.rs`](conn_test.rs) 的脚本 rows 与重试重建，[`http_handler_test.rs`](http_handler_test.rs) 的 gatherer 回退，以及 [`util_test.rs`](util_test.rs) 的 etcd 前缀与错误传播。其他直接使用者包括 `config_test.rs`、`dump_test.rs`、`metadata_test.rs`、`sql_test.rs`、`writer_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务指定的 11 章节结构命令，并额外检查 Markdown 链接目标、限定 git diff 与 `plan.md` 未修改。
