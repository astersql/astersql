# `pkg/ddl/ingest/config.rs` 逻辑说明

## 文件定位

`config.rs` 属于 `astersql-ddl-ingest` crate；crate 入口 `pkg/ddl/ingest/lib.rs` 以公开模块 `pub mod config` 暴露它。该 crate 对应 Go 包 `pkg/ddl/ingest`，用于 DDL reorg/backfill 的本地 ingest 加速路径：索引等键值先在本地排序，再批量导入存储层。仓库 DDL 入口文档 `docs/agents/ddl/03-reorg-backfill.md` 将该目录定位为 Lightning 风格的 ingest 加速实现，而不是 DDL job、schema state 或 checkpoint 的持久化驱动层。

当前 Rust 文件是一组配置数据与纯计算辅助函数。RustCodeGraph 的文件节点显示它直接被 `pkg/ddl/ingest/backend_mgr.rs` 和 `pkg/ddl/ingest/config_test.rs` 使用；源码搜索进一步确认，生产代码目前只引用 `IngestConfig` 类型，配置生成、内存调整、引擎配置和重要变量函数尚未接入 Rust 生产主链。因此本文描述的是现有部分移植状态，不能据此声称 Rust ingest 已具有 Go 版本的完整配置能力。

## 核心职责

- `IngestConfig` 汇集 worker/range 并发度、引擎与 writer 缓存、最大打开文件数和排序目录。
- `generate_config` 依据路径和并发度构造一组固定默认值；其 `_memory_quota` 参数当前未参与计算。
- `cop_read_batch_size` 选择显式批大小，或在零提示时使用固定的 `10 * 256`。
- `generate_local_engine_config` 生成供本地引擎消费的字符串键值配置。
- `adjust_import_memory` 与 `try_aggressive_memory` 根据 `MemRoot` 快照判断默认缓存是否可用，并在特定比例下同步缩小两类缓存。
- `default_important_variables` 提供可能影响 KV 编码结果的系统变量缺省值。

这些职责均是配置计算；文件不创建 DDL job、不执行 schema state 迁移、不持久化 checkpoint，也不直接申请、释放或记账内存。

## 主要符号

- `pub struct IngestConfig`：可克隆、可调试打印并支持完全相等比较。七个字段全部公开：`worker_concurrency`、`range_concurrency`、`engine_memory_cache_size`、`local_writer_memory_cache_size`、`max_open_files`、`sorted_kv_dir`。前两项和打开文件数为 `usize`，两个缓存为有符号字节数 `i64`。
- `pub fn generate_config(path, concurrency, _memory_quota) -> IngestConfig`：worker 并发为 `concurrency.wrapping_mul(2)`，range 并发为原值；引擎缓存固定为 512 MiB，writer 缓存固定为 128 MiB，打开文件数固定为 1024。
- `pub fn cop_read_batch_size(hint_size) -> usize`：正数原样返回，零返回 2560。
- `pub fn generate_local_engine_config(ts) -> BTreeMap<String, String>`：返回 `ts`、`compact=true`、1 GiB compact 阈值、compact 并发 4、16 KiB block、保留排序目录六项配置。`BTreeMap` 使遍历顺序按键稳定，但调用方不应把顺序当作协议。
- `pub fn adjust_import_memory(mem_root, config)`：先走激进额度检查；失败时按另一套内存公式计算整数缩放因子，只有因子大于 1 或为负数时才执行除法。
- `pub fn try_aggressive_memory(mem_root, config) -> bool`：计算 `writer_cache * worker_concurrency / 2 + engine_cache + current_usage` 是否不超过最大配额。签名接收 `&mut IngestConfig`，但当前实现不修改它，也不改变 `MemRoot` 记账。
- `pub fn default_important_variables() -> BTreeMap<&'static str, &'static str>`：每次构造八项静态字符串映射，涵盖 packet 上限、除法精度、时区、本地化、周格式、加密模式、group concat 上限和 TiDB row format 版本。

文件没有模块级常量、trait、`impl` 或条件编译项；所有函数及配置类型都是公开 API。

## 执行流程

典型配置计算可分为以下步骤，但当前 Rust 生产代码尚未把这些步骤串成完整流程：

1. 调用方用 `generate_config` 创建默认 `IngestConfig`。路径被转换为拥有所有权的 `String`，并发数同时派生 worker 与 range 并发。
2. 调用方可调用 `adjust_import_memory`。它先调用 `try_aggressive_memory`，以一个引擎缓存加半数 worker 的 writer 缓存估算默认占用，并加上 `MemRoot::current_usage()` 与上限比较；足够时立即返回。
3. 激进检查失败时，调整逻辑改用四个引擎缓存计算 `default_memory`，再除以 `MemRoot::max_memory_quota()` 得到整数 `scale`。`scale` 为 0 或 1 时保持配置不变，其他值则将 writer 和引擎缓存都除以该比例。
4. 创建本地引擎时可用 `generate_local_engine_config(ts)` 生成稳定的字符串参数；内部 SQL/session 初始化可用 `default_important_variables()` 补缺省值；coprocessor 扫描可用 `cop_read_batch_size` 选择批量大小。

Rust 当前实际生产调用只到 `BackendContextBuilder::build(config, ...)`：`pkg/ddl/ingest/backend_mgr.rs` 接受 `&IngestConfig` 后通过 `let _ = config` 明确忽略。因此上述第 1 至 4 步目前只能在 `config_test.rs` 中观察到直接执行证据。

## 数据与状态

`IngestConfig` 是普通拥有型值，没有内部同步原语和隐藏状态。调用方可以直接修改全部字段；`adjust_import_memory` 原地改写两个缓存字段，其余字段不变。`generate_local_engine_config` 和 `default_important_variables` 每次返回新的 `BTreeMap`，调用者拥有容器；后一函数的键和值引用静态字符串。

`MemRoot` 定义于 `pkg/ddl/ingest/mem_root.rs`。本文件只读取 `current_usage()` 与 `max_memory_quota()` 的瞬时值，不调用 `consume`/`release`，不预留额度，也不保证两次读取之间状态不变。`try_aggressive_memory` 的“成功”仅表示读取快照下公式成立。

内存计算有意使用 `wrapping_mul`、`wrapping_add`，并将 `usize` worker 数转换为 `i64`；极端输入可能环绕。缩放使用整数除法，会向零截断。正常设计域应是非负缓存、正配额和实际可用的并发数，但类型和函数本身没有强制这些不变量。

## 依赖与调用关系

- 标准库依赖仅为 `std::collections::BTreeMap`。
- crate 内唯一直接依赖是 `crate::mem_root::MemRoot`，用于读取内存用量和上限。
- `pkg/ddl/ingest/Cargo.toml` 声明 crate 名 `astersql-ddl-ingest`、库入口 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/ddl/ingest"` 标记 Go 来源。`config.rs` 自身没有使用 Cargo 中的外部 crate。
- RustCodeGraph：目标文件节点列出 10 个符号，并报告直接使用文件为 `backend_mgr.rs`、`config_test.rs`。精确 `query` 找到 `IngestConfig` 及六个公开函数；`callers/callees` 查询在当前本地索引上超时且没有返回边，故调用事实又用限定于 `pkg/ddl/ingest/**/*.rs` 的源码搜索核对。
- 上游生产边：`BackendContextBuilder::build(config: &IngestConfig, ...)` 接收配置；当前函数体不读取其字段。
- 上游测试边：`pkg/ddl/ingest/config_test.rs` 调用除 `default_important_variables` 外的所有函数，并直接构造 `IngestConfig`。
- 下游边：`adjust_import_memory -> try_aggressive_memory`；两者读取 `MemRoot::current_usage`、`MemRoot::max_memory_quota`。其他函数没有项目内函数调用。

## 错误处理与边界

该文件没有 `Result`/`Option` 错误通道，也不记录日志。输入不合法时没有主动校验：

- `generate_config` 忽略 `_memory_quota`，零并发可生成零 worker/range；极大并发在 worker 倍增时按 `usize` 环绕。
- `cop_read_batch_size` 的 `usize` 不能表达负提示；零是唯一回退信号。
- `adjust_import_memory` 在激进检查失败后以最大配额为除数；若 `max_memory_quota() == 0`，Rust 整数除法会 panic。负配额、负缓存或环绕后的负中间值也可能得到不符合资源语义的比例。调用方必须在进入该函数前保证正配额和合法配置。
- `scale == 0 || scale == 1` 不调整；公式不会向上取整，因此小于两倍配额但已经超过配额的估算仍可能保持原缓存。这与当前 Go 实现一致。
- 两个内存函数的算术采用 wrapping 操作避免 debug/release 溢出行为差异，但环绕值不会被报告为错误。
- 固定字符串 map 不校验下游是否认识配置键或系统变量，真正应用这些值时仍需由消费层处理兼容性和解析错误。

## 并发与资源生命周期

本文件不启动线程、任务或通道，不持有文件、网络连接、锁或事务。`IngestConfig` 的数值影响未来 worker 数、缓存和文件句柄资源，但这里不创建或回收这些资源；`sorted_kv_dir` 也只是路径字符串，目录生命周期由 ingest 环境/后端层负责。

`MemRoot` trait 要求 `Send + Sync`，其默认实现以互斥锁保护配额和用量；本文件通过共享引用读取它。由于检查与后续实际资源申请不是一个原子操作，其他线程可在读取后改变用量，所以这些函数只能用于配置估算，不能替代资源申请时的额度检查。返回的 map 和配置值彼此独立，可以由调用方自行在线程间传递，是否同步取决于外层所有权结构。

## 与 Go 版本的对应关系

直接对照为 `pkg/ddl/ingest/config.go`：

- Rust `generate_config` 对应 Go `genConfig` 的一小部分。两者都有 `2 * concurrency` worker、原始 concurrency/range、512 MiB 引擎缓存、128 MiB writer 缓存和排序路径；Rust 省略 Go 的 context、资源组、keyspace、写速率、重复检测、global sort、checkpoint、TiKV 检查和大量 Lightning 参数，并固定最大打开文件数为 1024。Go `genConfig` 还会立即调用 `adjustImportMemory`，Rust `generate_config` 不会。
- Rust `cop_read_batch_size` 对应 Go `CopReadBatchSize`。正提示语义相同；Rust 零值固定使用 `10 * 256`，Go 则读取动态的 `vardef.GetDDLReorgBatchSize()` 后乘十，所以运行期变量变化不会反映到 Rust。
- Rust `generate_local_engine_config` 对应 Go `generateLocalEngineConfig` 的字段子集，用字符串 map 表达 Go 的强类型 `backend.EngineConfig`。核心默认值一致，但 Rust 没有构造空 `TableInfo`。
- `adjust_import_memory` 与 `try_aggressive_memory` 保留 Go 的两套内存公式、整数缩放规则和不消费 `MemRoot` 的行为。Rust 省略 `context.Context` 和调整日志，并使用 wrapping 算术。
- `default_important_variables` 的八组键值与 Go `defaultImportantVariables` 一致；Go 在 `backend_mgr.go` 创建 backend context 时传入该 map，Rust 当前没有生产调用。
- Go `backend_mgr.go:CreateLocalBackend` 已调用 `genConfig`，并把配置传给 `ingestctrl.NewBackend`；Rust `BackendContextBuilder::build` 目前忽略 `IngestConfig`。这是一项明确的迁移缺口，而不是等价接线。

相关 Rust 单元测试在独立文件 `pkg/ddl/ingest/config_test.rs`，验证默认值、批大小分支、本地引擎字段、激进检查不记账和整数缩放规则。限定搜索未发现 Go `*_test.go` 对这些私有函数的直接测试，因此当前细粒度回归证据主要来自 Rust 测试及 Go 生产实现本身。

## 扩展指南

- 若要让 Rust 配置真正生效，最小接入点是 `BackendContextBuilder::build` 及实际创建本地 ingest backend/engine 的代码；先确认各字段的真实消费者，避免仅移除 `let _ = config` 却没有落实资源参数。
- 扩充 `generate_config` 时应逐项对照 Go `genConfig`，尤其是重复检测、资源组/keyspace、global sort、写速率和动态系统配置；不要用固定值冒充 Go 的运行期配置。同步扩充 `config_test.rs`，并为最终生产接线添加相邻 backend manager 的独立测试。
- 修改内存公式前同时更新 `adjust_import_memory`、`try_aggressive_memory` 和 `config_test.rs`，覆盖正配额、边界比例、当前用量、零/非法配额与大数溢出策略。若决定把零配额变成可恢复错误，需要改变 API 错误模型并检查所有调用方。
- 若使 `try_aggressive_memory` 保持只读，应考虑把参数收窄为 `&IngestConfig`；这是 API 兼容性变化，必须先检查仓库外调用者。若未来让它预留额度，则必须明确回滚/释放生命周期并处理并发竞态。
- 增加本地引擎键或重要系统变量时，应同时核对 Go 的强类型结构/变量清单、消费层解析规则以及 KV 编码兼容性；不要依赖 `BTreeMap` 的遍历顺序传达语义。
- 性能风险集中在并发倍增、缓存缩放、cop batch 大小和 compact 参数；兼容风险集中在系统变量默认值、动态 DDL reorg batch 与 Go/Rust 配置字段缺失。

## 验证依据

- 源码与模块边界：`pkg/ddl/ingest/config.rs`、`pkg/ddl/ingest/lib.rs`、`pkg/ddl/ingest/Cargo.toml`、`pkg/ddl/doc.go`。
- 直接生产证据：`pkg/ddl/ingest/backend_mgr.rs` 的 `BackendContextBuilder::build`；内存接口证据：`pkg/ddl/ingest/mem_root.rs` 的 `MemRoot` 与 `MemRootImpl`。
- Go 对照：`pkg/ddl/ingest/config.go` 的 `genConfig`、`CopReadBatchSize`、`generateLocalEngineConfig`、`adjustImportMemory`、`tryAggressiveMemory`、`defaultImportantVariables`，以及 `pkg/ddl/ingest/backend_mgr.go` 的 `CreateLocalBackend` 和 backend context 创建路径。
- 测试证据：`pkg/ddl/ingest/config_test.rs` 的五个独立单元测试；没有把测试逻辑内嵌进生产源文件。
- RustCodeGraph：`status` 显示索引含目标文件；`files --filter pkg/ddl/ingest/config.rs` 命中一文件；文件 `node` 显示 133 行、10 个符号以及两个直接使用文件；`query` 核对主要类型和函数。`callers/callees` 在本机索引查询超时，调用边由文件节点与限定源码搜索交叉核实。
- 人工复核结论：该文件存在于 DDL ingest 配置边界；当前能运行的是纯配置计算，生产侧仅有类型参数接线且未消费；安全扩展必须同时补消费链和独立测试，不能把 Go 的完整行为视为 Rust 现状。
