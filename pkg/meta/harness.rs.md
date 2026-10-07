# `pkg/meta/harness.rs`

## 文件定位

`pkg/meta/harness.rs` 是 `astersql-meta` crate 的依赖兼容层和内存测试支架，不是 TiDB 元数据的生产存储实现。crate 根 `pkg/meta/lib.rs` 以 `mod harness; pub use harness::*;` 挂载并再导出本文件，因此 `pkg/meta/meta.rs`、`pkg/meta/reader.rs`、`pkg/meta/meta_autoid.rs` 可以用 `crate::kv`、`crate::structure`、`crate::model` 等路径复用这些类型。真正的元数据键布局、CRUD 和遍历流程位于上述三个业务文件；本文件负责让这些流程在没有真实 TiKV、完整 Go 包依赖和真实指标系统时仍可编译、运行聚焦测试。

crate 边界由 `pkg/meta/Cargo.toml` 定义，库入口是 `lib.rs`。本文件直接使用 `anyhow`、`futures`、`serde` 和 `serde_json`；清单同时声明了 `astersql-kv`、`astersql-util-codec`、`astersql-meta-model`、`chrono`，但当前 harness 为保持 Go 接口形状而自带精简的 `kv`、`codec`、`model`、`time` 模块，并未把这些接口委托给对应 workspace crate。

## 核心职责

1. 提供 Go `pkg/meta/meta.go` 所依赖的一组包级接口替身：错误、AST 标识符、元数据模型、context、KV、structure、JSON、字节/键编码、partial JSON、异步运行时、指标、时间、调度状态、资源消费、MVCC helper 及常量。
2. 用 `Arc<Mutex<State>>` 实现共享内存 string/hash 存储，使 `Mutator`、`Reader` 与 AutoID accessor 能验证键布局和读写语义，而不连接 TiKV。
3. 保持关键跨语言数据契约：`CiString` 的 `O`/`L` JSON 字段、`DbInfo.db_name`、AutoID 字段名、memcomparable 字节编码、hash/string 类型标志和全局 ID 常量。
4. 明确降级非核心设施：事务选项、快照选项、指标观察为空操作；时间恒为零；`runtime::spawn` 不启动任务；内核类型恒为 classic。这些行为只适合当前 crate 的移植/契约测试，不能据此宣称已具备真实事务、MVCC、调度或可观测性能力。

## 主要符号

- `errors::{Error, new, trace, from}`：把错误统一为 `anyhow::Error`；`new` 从展示文本建错，另两者仅做 `Into` 转换。
- `ast::{MEDIUM_PRIORITY_VALUE, CiString}`：保存标识符原文和小写形式。`CiString::new` 生成二者；自定义 `Deserialize` 同时接受 Go 风格 `{ "O": ..., "L": ... }` 对象和字符串简写。
- `model`：定义 `AutoIdGroup`、`DbInfo`、`TableInfo`、`TableNameInfo`、`PolicyInfo`、`MaskingPolicyInfo`、`ResourceGroupInfo`、`SchemaDiff`、`Job`。`TableInfo::sep_auto_inc` 以 `TABLE_INFO_VERSION_5` 判断 RowID/AUTO_INCREMENT 是否分离；`Job::{encode, decode}` 走 JSON。
- `context::Context`：`Arc<AtomicBool>` 取消标记；`cancel` 写入，`check_error` 在取消后返回 `context canceled`。
- `kv::{Transaction, Snapshot, Storage, Key}`：三者围绕同一个 `Arc<Mutex<structure::State>>`；`Transaction::snapshot` 和 `Storage::get_snapshot` 只克隆共享状态句柄。`Priority`、`DiskFullOption` 以及 request-source 常量保留调用接口。
- `structure::{State, TxStructure, HashPair}`：`State` 分别保存 string 映射和嵌套 hash 映射；外层键是 `(prefix, key/hash)`。`new_structure` 创建可写访问器，`new_snapshot_structure` 创建只读访问器。
- `TxStructure` 的读写族：string 的 `inc/get_i64/set/get/clear`，hash 的 `hset/hget/hget_i64/hinc/hdel/hclear/hget_all/hget_len`，以及回调遍历 `hget_iter/iterate_hash/iterate_hash_bounded`。写入口都先经过内部 `ensure_writable`。
- `TxStructure::{encode_string_data_key, encode_hash_data_key, encode_hash_auto_id_key_value}`：按 Go structure 布局拼接前缀、memcomparable 编码、数据类型标志及 field/value。
- `structure::{ReverseHashIterator, new_hash_reverse_iter, new_hash_reverse_iter_from}`：基于 `BTreeMap` 的有序结果生成逆序 value 迭代器；带 `start` 版本保留 `field <= start`，空 `start` 表示不裁剪。
- `json::{marshal, unmarshal}`、`bytes::index`、`codec::{encode_bytes, encode_uint}`：分别包装 serde JSON、字节子串查找、Go 兼容的 8 字节分组编码和大端 `u64`。
- `partialjson::{extract_id_and_original_name, extract_top_level_members, TopLevelMembers}`：当前实现先解析完整 `serde_json::Value`，再抽取顶层数字、字符串或 `field.O`，并非流式/零拷贝解析。
- `runtime::{spawn, try_join_all}`：`spawn` 原样返回 future，`try_join_all` 才实际轮询传入 future 并在首个错误时返回。
- `metrics::{Histogram, META_HISTOGRAM}` 与 `time::{now, since}`：接口占位；观察方法无副作用，时间结果恒为 `0`。
- `schstatus::TtlTuneFactors`、`rmpb::Consumption`、`helper::{Write, MvccInfo, MvccResponse, Helper}`：提供 meta 序列化和 MVCC 查询所需的精简数据形状；`Helper` 只克隆预注入响应。
- `metadef`、`resourcegroup`、`mysql`、`kerneltype`：集中保留用户全局 ID 上限、系统库 ID、默认资源组、MySQL 系统库/字符集/排序规则及 classic 内核判定。

## 执行流程

典型写路径从 `meta::new_mutator` 开始：调用方交入 `kv::Transaction`，构造器设置高优先级和磁盘满策略（在 harness 中均为空操作），读取 `start_ts`，再以固定 `META_PREFIX = b"m"` 调用 `structure::new_structure`。后续数据库、表、策略、SchemaDiff 与 AutoID 操作把逻辑 key 映射到 `TxStructure` 的 string/hash 方法；JSON 模型通过 `json::marshal/unmarshal` 转为字节。

一次 string/hash 自增先检查访问器可写，随后锁住共享 `State`，把缺失项视为 `0`，把已有字节按 UTF-8 十进制解析，使用 `i64::wrapping_add` 计算并写回十进制字节。测试 `integer_and_byte_edges_match_go` 验证 `i64::MAX + 1` 回绕到 `i64::MIN`；格式错误或空字节不会降级为零，而是返回解析错误。

只读路径由 `reader::new_reader` 或遍历逻辑把 `kv::Snapshot` 传入 `new_snapshot_structure`。读取仍锁住同一个共享状态；任何 set/inc/clear/hset/hinc/hdel/hclear 都由 `ensure_writable` 拒绝。需要强调：该 snapshot 没有复制版本数据，因此在创建后仍可能看到其他共享访问器的新写入；它只实现“禁止经此句柄写”，没有真实 MVCC 的时间点一致性。

`meta::iter_all_tables` 为每段范围取得 snapshot，包装成只读 structure，用 `runtime::spawn` 收集 future，最后由 `runtime::try_join_all` 并发轮询并传播错误。由于 `spawn` 只是恒等函数，调度发生在 `try_join_all`，不存在独立后台任务或 join handle。

DDL 历史读取先由 `hget_all` 得到按 field 升序的 `BTreeMap` 内容，再 reverse；`new_hash_reverse_iter_from` 还会在 reverse 前过滤 `field <= start`。迭代器只保存 value，不暴露 field；调用者以 `valid -> value -> next` 推进。

## 数据与状态

核心可变状态是 `structure::State`：`strings` 的键为 `(prefix, key)`，`hashes` 的键为 `(prefix, hash)`，后者的值又是 `field -> value` 的 `BTreeMap`。prefix 被纳入状态键，测试 `prefixes_are_isolated_and_snapshot_is_read_only` 证明相同逻辑 key 在不同 prefix 下互不覆盖。

`Transaction`、`Snapshot`、`Storage` 和所有由它们派生的 `TxStructure` 都克隆同一个 `Arc`，不复制数据；`start_ts` 只是保存和返回，`get_snapshot(start_ts)` 忽略参数。`TxStructure.read_only` 是句柄局部状态，不在共享 `State` 中。

排序依赖 `BTreeMap<Vec<u8>, ...>` 的字节序。`hget_all`、范围遍历和反向迭代因此具有确定顺序；`iterate_hash_bounded` 对 hash key 采用 `[start, end)`，先在锁内复制匹配三元组，再释放锁执行用户回调，从而避免回调期间持锁。

模型状态是 Go 结构的子集。许多生产字段未出现在这些精简 struct 中；`TableInfo.db_id` 还带 `skip_serializing`。扩展模型时必须先核对 Go JSON 标签、缺省值和未知字段兼容，而不能仅按 Rust 字段名添加。

## 依赖与调用关系

上游装配点是 `pkg/meta/lib.rs`，其公开再导出使 harness 模块成为 crate API。直接业务调用关系经 RustCodeGraph 文件/符号查询及 crate 内引用搜索核验如下：

- `pkg/meta/meta.rs::new_mutator -> structure::new_structure`，所有 `Mutator` CRUD 再调用 `TxStructure` 的 string/hash API。
- `pkg/meta/reader.rs::new_reader -> structure::new_snapshot_structure`，只读 Reader 委托 `Mutator`/`TxStructure` 查询。
- `pkg/meta/meta_autoid.rs::{AutoIdAccessorImpl, AutoIdAccessorsImpl} -> TxStructure::{hget_i64,hset,hinc,hdel}`。
- `pkg/meta/meta.rs::iter_all_tables -> kv::Storage::get_snapshot -> new_snapshot_structure -> runtime::{spawn,try_join_all}`。
- `pkg/meta/meta.rs` 的快速名称/Job 读取调用 `partialjson`，schema key 编码调用 `codec`，历史 Job 调用反向 hash 迭代器，耗时记录调用 `metrics` 与 `time`。

下游第三方依赖仅用于错误、future 汇合和序列化。`State` 不调用真实 `astersql-kv`，键编码也不调用 `astersql-util-codec`；它们的等价性由测试与 Go 对照约束，而非类型共享保证。RustCodeGraph 对 `new_structure` 的精确 `callers` 查询在本次取证中超过 60 秒无结果，因此高扇出边以成功的文件/符号查询和限定于 `pkg/meta/*.rs` 的引用搜索补证。

## 错误处理与边界

所有可失败接口使用 `anyhow::Error`。UTF-8、整数和 serde JSON 错误通过 `?` 保留因果链；partial JSON 对缺失/类型不符字段返回 `missing <field>`；只读访问器写入返回 `write on snapshot`；取消返回 `context canceled`。遍历回调和 `try_join_all` 的首个错误会立即向上传播。

内存锁使用 `Mutex::lock().unwrap()`：若持锁线程 panic 导致 poisoning，后续访问会 panic，而不是返回 `Result` 错误。`ReverseHashIterator::value` 要求调用方先检查 `valid`，越界会 panic。`encode_bytes` 总会写结束分组：即使输入长度是 8 的整数倍，也会追加全 padding 组，这是 memcomparable 格式的一部分。

边界限制还包括：快照不是固定版本；事务无提交/回滚/隔离；option 设置无效；`get_snapshot` 忽略时间戳；时间与指标不真实；nextgen 恒为 false；helper 不查询存储；partial JSON 实际完整解析；`spawn` 不创建任务；模型字段不完整。若生产代码开始依赖这些被省略的语义，必须先替换/增强对应模块并添加独立测试，不能继续把空操作视为满足契约。

## 并发与资源生命周期

共享状态由 `Arc<Mutex<State>>` 管理，克隆 transaction/snapshot/structure 只增加引用计数；最后一个句柄释放时内存自动回收，没有显式 close、commit 或 rollback。每个单次 KV 操作在一把全局状态锁内完成，因此 string/hash 自增对所有 clone 原子；`harness_test.rs::increments_are_atomic_across_clones` 用 8 个线程各执行 1,000 次 string/hash 自增，期望二者均为 8,000。

这把粗粒度锁同时覆盖所有 prefix、string 和 hash，正确性简单但并发度有限；它不模拟 TiKV 的冲突检测、锁等待、版本选择或事务提交。`Context` 的 `AtomicBool` 使用 `Relaxed` 顺序，仅承载单比特取消观察，不提供其他内存状态的发布/获取关系。

回调遍历通过 `hget_all` 或预先收集 `entries` 克隆数据后再调用用户代码，避免回调重入同一 `Mutex` 造成自死锁。future 生命周期由调用 `try_join_all` 的任务持有；因为 `spawn` 不脱离当前任务，取消外层 future 会直接丢弃尚未完成的子 future。

## 与 Go 版本的对应关系

Go 侧不存在单一 `pkg/meta/harness.go`；本文件把 `pkg/meta/meta.go` 导入的多个生产包压缩为 crate 内模块。主要映射是：`errors` 对应 `github.com/pingcap/errors`，`ast` 对应 `pkg/parser/ast`，`model` 对应 `pkg/meta/model`，`kv` 对应 `pkg/kv`，`structure` 对应 `pkg/structure`，`codec` 对应 `pkg/util/codec`，`partialjson` 对应 `pkg/util/partialjson`，`helper` 对应 `pkg/store/helper`，其余常量/类型对应 `pkg/meta/metadef`、`pkg/resourcegroup`、`pkg/parser/mysql`、`pkg/config/kerneltype`、`pkg/dxf/framework/schstatus`、kvproto resource-manager 类型以及 Go 标准库 context/json/time/bytes。

已验证的兼容点包括：`codec::encode_bytes` 与 structure 数据 key 布局、`STRING_DATA = b's'`、prefix 隔离、空 needle 返回位置 0、整数自增溢出回绕、空 start 的逆序遍历、系统/用户 ID 常量、`CiString` 字符串反序列化，以及 `DbInfo`/`AutoIdGroup` 的 JSON 字段名，证据均在 `pkg/meta/harness_test.rs`。

差异必须保留为显式限制：Go `kv.Transaction`/`Snapshot` 与 `structure.TxStructure` 具备真实事务和 MVCC 语义，而 harness 只有共享内存；Go goroutine/context/metrics/time/helper/kerneltype 是真实设施，本文件对应模块为精简或固定行为；Go model 类型字段远多于此处子集；Go partialjson 面向局部提取优化，本实现解析完整 JSON。因而该文件的目标是支撑当前 Rust meta 移植范围，而不是替代这些 Go 包的完整实现。

## 扩展指南

- 新增 meta 业务调用前，先确认真实依赖应进入 workspace crate 还是仍属于 harness。若需要事务隔离、版本快照、提交回滚或真实指标，应接入对应正式实现，而不是继续扩充空操作。
- 新增 string/hash 原语时，在 `TxStructure` 中保持 prefix 隔离、只读写保护、确定排序和回调不持锁；测试放在独立的 `pkg/meta/harness_test.rs`，不要内嵌到源文件。
- 修改键编码时同步核对 Go `pkg/structure` 与 `pkg/util/codec`，并增加精确字节断言。任何数据类型标志、分组 padding 或大端顺序变化都可能破坏已有元数据兼容。
- 扩展模型/JSON 时同步核对 `pkg/meta/model/*.go` 的 JSON tag、默认值和版本门槛，并覆盖对象/字符串两种 `CiString` 输入、缺失字段和旧数据读取。
- 改动并发状态时保留跨 clone 原子性，评估 mutex poisoning、回调重入和锁粒度；若引入异步任务，需重新定义 `runtime::spawn` 的取消与资源回收语义。
- 扩展 partial JSON 或反向迭代时，分别覆盖字段缺失/类型错误、起止边界、空 hash、非法 `value()` 调用策略，以及首个回调错误的传播。
- 当前生产 Rust 文件已有 `// Copyright 2026 AsterSQL.`；后续不得删除对应版权信息。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/meta/harness.rs` 确认目标已索引；分段 `node --file` 读取了 1–959 行；`query` 精确确认 `new_structure`、`new_snapshot_structure`、`extract_id_and_original_name` 及 `Mutator` 等符号。宽泛 `explore` 出现大量全仓库同名项，未将其作为直接调用证据；精确 `callers new_structure` 超过 60 秒无输出后中止，并以限定引用搜索补证。
- Rust 源与 crate：`pkg/meta/harness.rs`、`pkg/meta/lib.rs`、`pkg/meta/meta.rs`、`pkg/meta/reader.rs`、`pkg/meta/meta_autoid.rs`、`pkg/meta/Cargo.toml`。
- 独立 Rust 测试：`pkg/meta/harness_test.rs` 的 `codec_and_structure_keys_match_go_layout`、`prefixes_are_isolated_and_snapshot_is_read_only`、`integer_and_byte_edges_match_go`、`reverse_iterator_empty_start_and_meta_constants_match_go`、`ci_string_and_model_json_match_go_compatibility`、`increments_are_atomic_across_clones`。
- Go 对照：`pkg/meta/meta.go`、`pkg/meta/reader.go`、`pkg/meta/meta_autoid.go`，以及其直接依赖目录 `pkg/structure`、`pkg/util/codec`、`pkg/util/partialjson`、`pkg/meta/model`、`pkg/meta/metadef`、`pkg/store/helper`、`pkg/config/kerneltype`、`pkg/dxf/framework/schstatus`、`pkg/resourcegroup`。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付结构检查要求目标文件存在且固定二级标题恰好为 11 个；完成前另行执行该命令并人工复核内容没有把桩能力描述成生产能力。
