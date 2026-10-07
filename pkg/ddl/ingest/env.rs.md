# `pkg/ddl/ingest/env.rs` 逻辑说明

## 文件定位

`env.rs` 属于 `astersql-ddl-ingest` crate，并由 `pkg/ddl/ingest/lib.rs` 以公开模块 `env` 暴露。它位于 DDL ingest（本地排序并批量导入）链路的进程级环境边界：保存本进程的 ingest 临时数据根目录，按 DDL job 生成后端目录，计算活跃/过期目录集合，并把已初始化的路径转换为磁盘预检所需的 `DiskRoot`。

生产链已有两个直接入口：`pkg/ddl/backfilling_read_index.rs` 的 `ReadIndexStepExecutor::init` 读取 `ingest_temp_data_dir` 后执行本地排序磁盘预检；`pkg/ddl/index.rs` 将 `init_global_lightning_env` 和 `initialized_disk_root` 再导出，`pkg/session/runtime/system_session.rs` 的 `ConcreteJobExecutionContext::{ingest_initialized,pre_check_ingest_disk}` 通过该门面检查环境。相对地，`generate_ingest_temp_data_dir`、`processing_job_ids` 和 `stale_temp_directories` 当前只在 `pkg/ddl/ingest/env_test.rs` 出现，尚未接入 Rust 生产清理循环。

## 核心职责

本文件承担四项聚焦职责：

1. 用 `INGEST_ROOT` 保存一个进程级、可并发访问的可选根路径。
2. 用 `backend_mgr::{encode_backend_tag,decode_backend_tag}` 统一 job 目录命名与解析，避免环境模块另造编码规则。
3. 以纯函数方式从目录名和活跃 job ID 集合计算“仍在处理的 job”及“应删除的目录”；函数本身不访问数据库、不遍历文件系统，也不删除文件。
4. 用 `initialized_disk_root` 把已注册路径适配成 `disk_root::DiskRoot`，供 DDL job 初始化前执行真实文件系统空间检查。

它不负责 Go 版本中的完整启动初始化（存储类型判断、内存根、文件句柄上限和日志设置），也不负责查询 `mysql.tidb_ddl_job` 或执行定时清理。当前 Rust 事实应以这些有限职责理解，不能把 Go 的完整行为视为已经移植。

## 主要符号

- `INGEST_ROOT: OnceLock<Mutex<Option<PathBuf>>>`：懒创建的进程全局容器。`OnceLock` 固定的是互斥容器本身，内部 `Option<PathBuf>` 仍可被测试替换。
- `init_global_lightning_env(path) -> bool`：持锁检查根路径；首次由 `None` 写为 `Some(path)` 时返回 `true`，已设置时返回 `false` 且不覆盖旧值。
- `ingest_temp_data_dir() -> Option<PathBuf>`：克隆并返回当前根路径；全局容器尚未创建或内部为 `None` 时返回 `None`。
- `generate_ingest_temp_data_dir(job_id, check_duplicates) -> Result<PathBuf, String>`：要求环境已初始化，再拼接 `encode_backend_tag` 生成 job 目录；未初始化时返回固定字符串错误。
- `processing_job_ids(directory_names, active_job_ids) -> Vec<i64>`：忽略不能解码的名称，只保留活跃 ID，并排序、去重。
- `stale_temp_directories(root, directory_names, active_job_ids) -> Vec<PathBuf>`：从可解码且不活跃的普通目录识别过期 job；每个 job 同时展开普通目录和 `-dup` 目录路径。
- `initialized_disk_root() -> Option<DiskRoot>`：有根路径时创建 `DiskRoot::new(path, 0, 0)`；它返回新实例而非持久共享的全局磁盘状态。
- `replace_global_lightning_env_for_test(path) -> Option<PathBuf>`：隐藏文档的测试钩子，原子式替换 `Option<PathBuf>` 并返回旧值，供测试用 RAII 恢复环境。

文件内没有 trait、结构体定义或条件编译项；八个符号中除静态量外均为公开函数。

## 执行流程

启动/使用主线可概括为：调用方先用 `init_global_lightning_env` 注册根路径；本地 ingest 执行器随后通过 `ingest_temp_data_dir` 取得路径；需要 DDL 环境检查时，`initialized_disk_root` 基于同一路径创建 `DiskRoot`，其 `pre_check_usage` 再创建目录并读取真实磁盘容量/可用空间（实现位于 `pkg/ddl/ingest/disk_root.rs`）。若尚未初始化，读取函数返回 `None`，上游分别转换为 `ReadIndexError::LocalSortDisk` 或字符串错误。

目录分类流程与生产初始化相互独立。`processing_job_ids` 对输入逐项解码、按 `active_job_ids` 过滤、排序并去重。`stale_temp_directories` 同样只接受能被 `decode_backend_tag` 识别的普通 tag；确认 job 不活跃后，为该 ID 同时生成普通路径和查重路径。这样复现 Go 清理逻辑“用普通目录判定 job，再成对删除普通/查重目录”的核心规则，但 Rust 函数只返回候选路径。

## 数据与状态

唯一长期状态是 `INGEST_ROOT` 内的 `Option<PathBuf>`。正常初始化是只写一次语义：`init_global_lightning_env` 不允许第二次覆盖；测试钩子例外地允许保存、替换和恢复。路径读取会克隆 `PathBuf`，因此调用方不能借返回值修改全局状态。

活跃 job 集合采用 `BTreeSet<i64>`，成员判断确定且不依赖哈希随机性。`processing_job_ids` 输出显式排序去重；`stale_temp_directories` 则保持可解码输入的迭代顺序，并且没有对重复 job ID 去重，所以重复普通目录名会产生重复候选路径。这不是文件系统扫描器：输入是否只包含目录、目录是否存在，均由调用者保证。

目录命名完全委托 `pkg/ddl/ingest/backend_mgr.rs`：普通目录如 `42`，查重目录如 `42-dup`。当前解码器只接受普通数值 tag；测试明确证明 `42-dup`、空白包围的数字和畸形后缀都不会被解码。

## 依赖与调用关系

标准库依赖为 `BTreeSet`、`Path/PathBuf`、`Mutex` 和 `OnceLock`。crate 内下游依赖有两条：`backend_mgr::encode_backend_tag/decode_backend_tag` 提供目录协议，`disk_root::DiskRoot::new` 提供磁盘预检对象。`pkg/ddl/ingest/Cargo.toml` 声明 crate 名为 `astersql-ddl-ingest`，`lib.rs` 是入口；本文件本身不直接使用第三方 crate，但它构造的 `DiskRoot` 依赖 `fs2` 与 `astersql-util-dbterror` 完成空间查询和错误映射。

已核实的上游边如下：

- `pkg/ddl/backfilling_read_index.rs` → `env::ingest_temp_data_dir` → `disk_root::check_local_sort_disk_space_at_path`，用于非云、本地排序子任务的磁盘空间门禁。
- `pkg/ddl/index.rs` 再导出 `init_global_lightning_env`、`initialized_disk_root` 和测试替换函数。
- `pkg/session/runtime/system_session.rs` → `astersql_ddl::index::initialized_disk_root` → `DiskRoot::{pre_check_usage}`，把全局环境接入 `JobExecutionContext`。
- `pkg/session/runtime/normal_ddl_index_reorg_initialization_test.rs` 与 `normal_ddl_masking_policy_test.rs` 通过再导出的替换函数隔离全局路径。

RustCodeGraph 将 `env.rs` 标记为被上述四个文件使用，但对逐符号 `callers/callees` 未返回静态边；因此调用点又通过 `rg` 和对应源码核实。没有证据表明三个目录分类/生成函数已被 Rust 生产代码调用。

## 错误处理与边界

`generate_ingest_temp_data_dir` 在未初始化时返回 `Err("ingest environment is not initialized")`；其余路径计算函数不返回错误，而是跳过无法解码的输入。`init_global_lightning_env` 用 `false` 表示重复初始化，而非错误。`initialized_disk_root` 用 `None` 表示缺少环境，上游负责添加业务错误上下文。

所有全局锁调用都使用 `lock().unwrap()`：若持锁线程 panic 导致互斥锁 poisoned，后续初始化、读取或测试替换会继续 panic，而不是返回可恢复错误。`path.into()` 和路径拼接不创建目录，也不验证可写性；真实 I/O 错误直到 `DiskRoot::pre_check_usage` 或其他调用方操作文件系统时才出现。

`stale_temp_directories` 对无效名称采取保守策略，不生成删除候选。它不会查询活跃 job，`active_job_ids` 的完整性和时效性由调用方负责；若集合过期，可能错误分类，因此接入实际删除前必须先从持久 job 状态得到一致快照并再次审查竞态。

## 并发与资源生命周期

`OnceLock` 保证全局 `Mutex` 的线程安全懒初始化，`Mutex` 串行化路径的初始化、读取克隆与测试替换。正常生产生命周期预期为进程启动阶段设置一次、之后只读；重复初始化不会改变已有值。

测试通过 `replace_global_lightning_env_for_test` 暂时突破只写一次约束。`normal_ddl_masking_policy_test.rs` 的 `IngestEnvironment::Drop` 以及 `normal_ddl_index_reorg_initialization_test.rs` 的 `Restore::Drop` 都保存旧值并在离开作用域时恢复；相关测试还用额外互斥锁串行修改共享配置。新增测试若修改该全局状态，也必须采用同样的保存/恢复和串行策略，避免并行测试互相污染。

`initialized_disk_root` 每次创建独立 `DiskRoot`，只共享路径、不共享 tracker、quota 状态或 `updating` 原子标志。其初始容量和可用空间均为零；当前生产用途紧接 `pre_check_usage`，该方法直接向文件系统查询空间，因而不依赖构造时的缓存值。若未来用于 `startup_check` 或 `should_import`，不能假设这个临时实例等同于 Go 的全局 `LitDiskRoot`。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/ddl/ingest/env.go`，相关测试是 `env_test.go`。Rust 与 Go 已对齐的语义包括：维护 ingest 根路径、沿用 backend tag 协议、忽略不可解码名称，以及对过期 job 同时处理普通目录和查重目录。Rust 独立测试 `env_test.rs` 覆盖了这些目录规则。

关键差异如下：

- Go `InitGlobalLightningEnv` 校验 TiKV store，初始化 `LitMemRoot`/`LitDiskRoot`，读取内存、刷新和检查磁盘、生成 rlimit、设置 Lightning logger 与初始化标志；Rust 同名函数只注册路径，且拒绝覆盖。
- Go `GetIngestTempDataDir`/`GenIngestTempDataDir` 从全局配置的 `TempDir` 和服务端口生成 `${temp-dir}/tmp_ddl-${port}` 并创建目录；Rust `generate_ingest_temp_data_dir(job_id, check_duplicates)` 是在已注册根路径下生成 job 子目录，名称相似但层级和职责不同。
- Go `CleanUpTempDir` 自行读取目录、查询 `mysql.tidb_ddl_job` 并执行 `os.RemoveAll`，且由 `pkg/ddl/ddl.go` 每分钟运行；Rust 只提供 `processing_job_ids` 和 `stale_temp_directories` 的纯计算，没有数据库会话、定时循环或删除操作。
- Go 的 `LitDiskRoot` 是长期全局资源跟踪器；Rust `initialized_disk_root` 每次返回新建的零缓存 `DiskRoot`，当前只适合作为路径驱动的预检适配器。

因此本文件是部分语义移植和局部接线，不能据此宣称 Rust 已具备 Go 的完整 ingest 环境与清理生命周期。

## 扩展指南

若要扩展启动初始化，应优先修改 `init_global_lightning_env` 及其上层启动调用，并决定是否需要把长期 `DiskRoot`/内存根纳入共享状态；同时补充独立的 `env_test.rs`，验证重复初始化、失败原子性和资源恢复。不要把测试放进 `env.rs`。

若要把清理能力接入生产，应复用 `processing_job_ids`/`stale_temp_directories` 和 backend tag 协议，但数据库活跃状态查询、目录枚举、定时调度及实际删除应放在合适的 DDL 会话/生命周期层。必须复刻 `env.go` 的保守错误策略，并增加“活跃 job 不删除、结束 job 成对删除、未知路径/无效名称安全忽略”的回归测试；删除属于高风险操作，还需处理查询与删除之间 job 状态变化的竞态。

若改变 tag 格式，必须同步 `pkg/ddl/ingest/backend_mgr.rs`、`env_test.rs` 及 Go 对照协议，评估升级期间旧临时目录兼容性。若让 `initialized_disk_root` 承担用量跟踪或启动检查，则应避免每次新建造成状态丢失，并同步 `disk_root.rs` 相关测试。性能上，目录分类目前对活跃集合查询为对数复杂度，`processing_job_ids` 还会排序；大目录场景接线前应评估枚举、排序和重复候选成本。

## 验证依据

事实依据包括：目标源码 `pkg/ddl/ingest/env.rs`；crate 边界 `pkg/ddl/ingest/{Cargo.toml,lib.rs}`；目录协议与磁盘对象 `pkg/ddl/ingest/{backend_mgr.rs,disk_root.rs}`；Rust 单元测试 `pkg/ddl/ingest/env_test.rs`；生产调用点 `pkg/ddl/backfilling_read_index.rs`、`pkg/ddl/index.rs`、`pkg/session/runtime/system_session.rs`；共享环境回归测试 `pkg/session/runtime/{normal_ddl_index_reorg_initialization_test.rs,normal_ddl_masking_policy_test.rs}`；Go 对照 `pkg/ddl/ingest/{env.go,env_test.go}` 与启动/清理调用 `pkg/ddl/ddl.go`。

RustCodeGraph 验证包括 `status`、`files --filter pkg/ddl/ingest`、对 `env.rs` 的 `node --file`、主要符号 `query` 以及逐符号 `callers/callees`；索引识别 12 个文件节点符号并报告四个使用文件，但逐符号边为空，故使用源码搜索补证。按任务约束未运行 Cargo；最终以固定十一章节的结构命令验证文档形状，并人工核对当前支持范围、错误边界与 Go 差异没有被写成理想状态。
