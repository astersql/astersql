# `cmd/importer/main.rs`

## 文件定位

[`cmd/importer/main.rs`](./main.rs) 是 workspace 包 `astersql-cmd-importer` 的可测试主流程装配层，不是操作系统直接调用的二进制入口。完整入口链为 `cmd/importer/bin_main.rs::main` → `astersql_cmd_importer::main`（`cmd/importer/lib.rs`）→ `entry::main`（本文件）。`cmd/importer/Cargo.toml` 通过 `[lib] path = "lib.rs"`、`[[bin]] path = "bin_main.rs"` 固定了这层结构，并以 `package.metadata.porting.go-package = "cmd/importer"` 标明 Go 对照目录。

本文件位于配置、DDL 解析、统计加载、数据库操作与并发作业模块之上，只决定它们的调用顺序和失败策略，不实现具体的 SQL 解析、造数或 worker 算法。当前 crate 只声明 `toml` 和 `serde_json` 两个外部依赖；数据库、TiDB 元数据、直方图等接口均由同目录 Rust 模块及 `stubs.rs` 提供。因此当前 Rust importer 保存并验证了 Go 控制流，但其 DB 是记录型本地桩，不能描述为已接通真实 TiDB/MySQL 驱动。

## 核心职责

本文件承担两个层次的职责：`main` 负责进程参数与退出码边界，`run_with_args` 负责可由测试复用的完整 importer 编排。主流程依次完成配置解析、表/索引元数据解析、连接取得、可选统计装配、DDL 执行、数据导入和连接关闭。

它还维持三项关键顺序约束：先解析表再解析索引，使索引列偏移可引用已建立的列；在 worker 启动前执行建表和建索引 SQL；索引直方图先于列直方图写入列状态，使列统计不会覆盖已经选中的索引统计。具体实现分别由 `config.rs::Config::Parse`、`parser.rs`、`stats.rs`、`db.rs` 和 `job.rs::doProcess` 承担。

## 主要符号

- `main()`：公开的共享进程入口。它用 `stubs::args_from_env` 获取参数（对应 Go 的 `os.Args[1:]`），调用 `run_with_args(&args, None)`；成功直接返回，`Err(code)` 则交给 `stubs::os_exit` 终止进程。
- `run_with_args(args: &[String], dbs_override: Option<Vec<DB>>) -> Result<(), i32>`：本文件真正的业务入口，也是 parity 测试的注入点。`None` 表示按 `WorkerCount` 创建连接，`Some` 表示直接使用调用者提供的句柄；注入只替换连接来源，后续统计、DDL、作业和关闭顺序不变。
- 本文件没有模块级常量、结构体、trait、`impl` 或条件编译项。唯一显式共享状态是把解析完成的 `table` 包入 `Arc` 后交给 `doProcess`；进程/测试条件差异封装在下游 `stubs::os_exit` 中。

## 执行流程

1. `main` 读取环境参数并调用 `run_with_args`；只有配置解析类错误通过 `Result<(), i32>` 返回到这里。
2. `run_with_args` 用 `NewConfig` 建立默认配置，再调用 `Config::Parse`。帮助请求返回 `Err(0)`；其他 flag、配置文件或尾随参数错误先写 stderr，再返回 `Err(2)`。
3. 通过 `newTable` 建立空表模型，依次执行 `parseTableSQL` 和 `parseIndexSQL`。任一失败都调用 `stubs::fatal`，不会返回 `Err(i32)`。
4. 若有 `dbs_override`，直接采用注入句柄；否则以数据库配置和 `WorkerCount` 调用 `createDBs`。创建失败同样进入 fatal 路径。
5. `StatsCfg.Path` 非空时调用 `loadStats`。对每个索引，只处理存在首列且有非空 bucket 的索引直方图，并按首列 `Offset` 写入对应 `table.columns[offset].hist`；越界 offset 被忽略。随后遍历列统计，仅在该列尚无直方图且 bucket 非空时补入列直方图，因此索引信息优先。
6. 使用 `dbs[0]` 顺序执行建表 SQL 和建索引 SQL。`db.rs::execSQL` 把空 SQL 当作成功 no-op，所以空索引 SQL 无需额外分支；执行错误为 fatal。
7. 将完成装配的表模型放入 `Arc`，把连接切片以及 job、worker、batch 配置传给 `doProcess`。该调用同步等待所有 worker 完成，并传播 worker panic。
8. `doProcess` 正常返回后调用 `closeDBs` 尽力关闭所有连接，最后返回 `Ok(())`。

## 数据与状态

配置状态集中在局部变量 `cfg`，由 `DBCfg`、`DDLCfg`、`StatsCfg` 和 `SysCfg` 四部分组成。表状态由 `newTable` 创建并在解析和统计阶段独占可变；进入 `doProcess` 前转为 `Arc<table>`，此后只读共享。列的 `hist` 是 `Option<Arc<histogram>>`：索引统计携带 `IndexInfo`，列统计的 index 信息为 `None`。

连接集合 `dbs: Vec<DB>` 的所有权保留在 `run_with_args`，`execSQL`、`doProcess` 和 `closeDBs` 都只借用它。测试注入的 `DB` 是可克隆句柄，因此 `parity_test.rs::contract_normal_path` 可以在调用结束后观察执行 SQL、提交次数和关闭状态。生产路径的连接数由 `WorkerCount` 决定；本文件假设至少有一个连接，并假设连接数足以供 `doProcess` 按 worker 下标取用。

本文件不保存全局可变状态。进程退出记录、DB 执行记录、随机状态以及统计文件读取均属于下游模块；这里仅编排其生命周期。

## 依赖与调用关系

上游生产调用链是 `bin_main.rs::main` → `lib.rs::main` → 本文件 `main` → `run_with_args`。RustCodeGraph 将 `run_with_args` 定位在本文件第 52 行，并给出两个直接调用者：本文件 `main` 与 `cmd/importer/parity_test.rs::contract_normal_path`。crate 根还在 `#[cfg(test)]` 下把 `parity_test.rs` 作为独立测试模块接入，测试逻辑没有放进生产源文件。

直接下游包括：`config::NewConfig/Config::Parse`；`parser::newTable/parseTableSQL/parseIndexSQL`；`stats::loadStats/histogram::from_core`；`db::createDBs/execSQL/closeDBs`；`job::doProcess`；以及 `stubs::args_from_env/log_error/fatal/os_exit/DB`。RustCodeGraph 的 `callees` 查询在当前索引上没有产生可用输出，因此这些下游边以本文件导入和调用点为直接证据，而不把图工具的文件级 `used by` 摘要误当作函数调用边。

`Cargo.toml` 的 `toml`、`serde_json` 并非本文件直接调用：前者服务配置读取，后者服务统计 JSON。真实 SQL parser、数据库驱动和 TiDB stats crate 并未列为依赖，对应能力目前由本地移植实现与 stubs 承担。

## 错误处理与边界

错误被刻意分成两类。命令行帮助和解析失败是可映射为进程码的 `Err(0)`/`Err(2)`；`main` 最终调用 `os_exit`，非测试构建使用 `std::process::exit`，测试构建用可捕获 panic 模拟。DDL 解析、连接创建、统计加载、DDL 执行错误则直接调用 `stubs::fatal`；当前桩以 panic 模拟 Go `log.Fatal`，不通过 `run_with_args` 的返回类型传播。

重要边界包括：空 `IndexSQL` 由 `execSQL` 视为 no-op；空 stats 路径完全跳过统计加载；无列索引在 Rust 中被跳过，而 Go `main.go` 直接访问 `idxInfo.Columns[0]`，这是 Rust 的防越界保护差异；索引首列 offset 越界也被 Rust 忽略。列统计循环仍直接按 `table.columns[i]` 索引，依赖 `tblInfo.Columns` 与运行时列数组保持等长。

`dbs[0]` 是未显式校验的前置条件。`dbs_override = Some(vec![])` 或 `WorkerCount == 0` 会在 DDL 阶段越界 panic；连接数少于正 worker 数时，`job.rs::doProcess` 会在 `dbs[i]` 取值处 panic。负 `WorkerCount` 还会在 `createDBs` 的容量转换处产生不可接受的容量请求。配置层当前没有在本文件之前强制这些数值为正，因此扩展或调用时不能假称已经安全处理。

## 并发与资源生命周期

本文件自身不创建线程或通道；并发从同步调用 `doProcess` 开始。`job.rs::doProcess` 创建有界 job/done 通道、一个分发线程和多个 worker 线程；发现任一 `incremental` 列后会把 worker 数降为 1，以保护生成顺序。表通过 `Arc` 在 worker 间共享，连接按 worker 下标克隆；主调用会等待完成通知并 join worker，若 worker panic 则恢复该 panic。

正常路径只在全部 DDL 和导入工作结束后执行 `closeDBs`。`closeDBs` 会逐个关闭连接，单个关闭失败只记录错误并继续关闭剩余连接。若 `parse*`、`loadStats`、`execSQL` 或 `doProcess` 进入 fatal/panic 路径，末尾的显式 `closeDBs` 不会执行；这在进程级行为上接近 Go `log.Fatal`（`os.Exit` 不运行 defer），但在测试中捕获 panic 后可观察到注入句柄没有被该入口清理。当前代码也没有 RAII guard 来保证异常路径回收。

## 与 Go 版本的对应关系

Rust 的 `run_with_args` 基本逐段对应 `cmd/importer/main.go::main`：相同的配置解析退出码、表后索引解析顺序、按 worker 建连接、可选 stats、索引统计优先、先 DDL 后 `doProcess`。`main` 则保留了 Go 对 `os.Args[1:]` 和 `os.Exit` 的进程边界。

为可测试性，Rust 额外拆出 `run_with_args` 并加入 `dbs_override`；Go 始终调用 `createDBs`。Rust 把 `table` 转为 `Arc` 适配线程共享，并以 `histogram::from_core` 把 stats 核心数据转成本地包装。Rust 对空索引列和越界 offset 有额外保护，Go 当前直接索引。Go 在连接创建后立即 `defer closeDBs(dbs)`，Rust 在成功流程末尾显式关闭；因为 Go 的 fatal 使用进程退出，错误路径实际上也不会执行该 defer。

迁移真实性上存在明确差异：Go 版使用 `database/sql`、真实 MySQL driver、TiDB parser/model/statistics；Rust crate 只依赖 `toml`、`serde_json`，DB 和多个 TiDB 类型来自 `stubs.rs`。因此 parity 测试证明的是控制流、SQL 记录、batch/commit 和错误契约，不是对真实集群的端到端兼容性。

## 扩展指南

- 增加新的入口阶段时，最小接入点是 `run_with_args`，并应明确放在“元数据完成、DDL 完成、worker 启动、资源关闭”哪一条边界；在 `cmd/importer/parity_test.rs` 增加独立回归测试，不要把测试写入 `main.rs`。
- 调整参数或退出语义时同时核对 `config.rs::Config::Parse`、本文件 `main/run_with_args`、`stubs.rs::os_exit/fatal` 与 Go `main.go`。帮助必须保持 0，普通解析错误保持 2，fatal 类错误不得悄悄降级为继续执行。
- 扩展统计装配时应维护“索引直方图优先、非空 bucket 才安装”的不变量，并为无列索引、错误 offset、列/运行时数组不一致和缺失/损坏 stats 文件增加测试。若要消除与 Go 的保护差异，应先决定两端共同契约，而不是单边猜测。
- 引入真实数据库实现前，应保留连接注入边界，并补充连接数与 worker/batch/job 参数校验、创建部分成功时的回滚、DDL/worker 失败时的可靠清理及真实数据库集成测试。外部 Rust 依赖必须按仓库规则在独立上游移植、提交和打 tag，再由 Cargo 统一引用。
- 性能敏感点不在本文件的顺序代码，而在下游 stats 文件加载、直方图共享、worker 数、batch 大小和连接数。优化时要验证 DDL 先行、增量列单 worker、连接与 worker 一一对应以及最终 join/close，不可只以编译成功替代行为证据。

## 验证依据

- RustCodeGraph：`status` 显示当前索引包含 7032 个 Rust 文件；`files --filter cmd/importer` 列出目标、装配、Go 对照和测试文件；`node --file cmd/importer/main.rs --offset 1 --limit 240` 读取完整 139 行；`query run_with_args --kind function` 和 `node run_with_args` 定位该符号并给出 `main`、`contract_normal_path` 两个调用者。另执行了 `callers/callees`，当前索引未给出可用输出，故下游关系由源码调用点核验。
- 生产与装配文件：`cmd/importer/main.rs`、`cmd/importer/lib.rs`、`cmd/importer/bin_main.rs`、`cmd/importer/config.rs`、`cmd/importer/db.rs`、`cmd/importer/job.rs`、`cmd/importer/stats.rs`、`cmd/importer/stubs.rs`。
- crate 边界：`cmd/importer/Cargo.toml`，确认 package 是 binary 移植、库/二进制入口路径以及仅有 `toml`、`serde_json` 依赖。
- Go 对照：`cmd/importer/main.go`，并通过同目录 `parser.go`、`stats.go`、`db.go`、`job.go` 的符号搜索核对主流程下游。Go 独立测试仅发现 `cmd/importer/db_test.go`，其 `TestIntToDecimalString` 不直接覆盖本入口。
- Rust 独立测试：`cmd/importer/parity_test.rs::go_rust_public_contract_matches` 及其四组 helper；其中 `contract_normal_path` 直接以两个注入 DB 运行 `run_with_args` 并验证 DDL、INSERT、commit 和关闭，`contract_boundary` 覆盖 help/空连接创建，`contract_error_paths` 覆盖 SQL 执行 fatal，`contract_resource_cleanup` 覆盖连接关闭、批次提交和 stats 文件缺失。`cmd/importer/stats_test.rs` 另验证 `loadStats` 的真实 fixture 解析，但不直接调用本入口。
- 本任务只生成说明文档，按总计划不运行 Cargo。交付前执行任务指定的标题计数命令，并人工复核本文能回答文件存在原因、执行顺序、stub 限制、错误/资源边界和安全扩展入口。
