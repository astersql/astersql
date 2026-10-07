# `br/pkg/utils/stubs.rs`

## 文件定位

`stubs.rs` 是 Cargo crate `astersql-br-pkg-utils`（入口见 `br/pkg/utils/lib.rs`，crate 声明见 `br/pkg/utils/Cargo.toml`）的本地兼容层。`lib.rs` 通过 `#[path = "stubs.rs"] pub mod stubs` 挂载它，并把 `kvproto`、`KeyRange`、`KvKey` 再导出给同 crate 及下游 crate。它不是 Go `br/pkg/utils` 中某个同名文件的逐文件移植：Go 版本直接依赖 `pkg/kv`、`pkg/tablecodec`、`pkg/util/sqlexec` 和 kvproto 生成类型，而 Cargo 清单明确移除了完整 kv/domain/kvproto/grpcio/tablecodec/sqlexec 依赖，因此本文件把这些边界压缩成可编译、可测试的局部替身。

该文件当前约 1100 行，包含四组能力：KV/事务接口、可取消上下文、受限 SQL 结果接口、meta key 编解码，以及 `kvproto::{encryptionpb,metapb,brpb}` 的消息壳。它参与生产 crate 编译，不能按测试 mock 对待；但除 meta key 编解码与取消传播外，大部分 API 只保存字段或转发调用，不提供真实 RPC、持久化或 protobuf 编解码。

## 核心职责

1. 为 `common.rs` 提供 `KvContext`、`Storage`、`Transaction`、`RunInNewTxn` 和内部事务来源常量，使全局 ID 分配能在进程内 `astersql_meta::kv::Transaction` 上运行。
2. 为 `wait.rs`、`misc.rs` 及下游注册流程提供 `context::Context`，支持克隆共享状态、父到子的取消传播和带超时等待。
3. 为 `db.rs` 定义 `RestrictedSQLExecutor`、`Row`、`ResultField` 等最小接口，让调用方通过 mock 或上层适配器执行受限 SQL；本文件本身不执行 SQL。
4. 用 `astersql-util-codec` 实现 `EncodeMetaKey`/`DecodeMetaKey`，保持 Go `pkg/tablecodec` 的 `m + mem-comparable(key) + h + mem-comparable(field)` 格式。
5. 用普通 Rust struct/enum 模拟 utils 所需的 kvproto 子集，供加密、错误分类、store 管理、备份元数据 JSON 转换与统计逻辑使用。

## 主要符号

- KV 边界：`InternalTxnBR`、`KvContext`、`WithInternalSourceType`、`Storage`、`Transaction`、`RunInNewTxn`。`Storage::meta_transaction` 和 `Transaction::meta_transaction` 默认返回 `shared_meta_transaction()` 的克隆；该共享事务由 `OnceLock` 延迟初始化。
- 取消边界：`context::Context::{new,is_cancelled,cancel,wait_cancelled_timeout,child_token}`。内部 `State` 含 `AtomicBool`、等待用 `Mutex`/`Condvar` 和 `Weak<State>` 子节点表；私有 `cancel_state` 递归传播取消。
- SQL 边界：`ColumnInfo`、`ResultField`、`Datum`、`Row`、`GoError`、`RestrictedSQLExecutor::ExecRestrictedSQL`。`Row` 只存 `Vec<String>`，`Datum::ToString` 只返回字符串副本，不等同于完整 TiDB Datum。
- meta key：`TableKey`、`EncodeMetaKey`、`DecodeMetaKey`，以及私有常量 `META_PREFIX = b"m"`、`HASH_DATA = b'h' as u64`。
- kvproto 加密与 store 壳：`kvproto::encryptionpb::EncryptionMethod`；`kvproto::metapb::{StoreState,StoreLabel,Store}`。
- BR 消息壳：`kvproto::brpb::{File,CipherInfo,Error,RawRange,Schema,BackupMeta,BackupRange,MetaFile,StatsBlock,StatsFile}`。各类型主要由 `new`、`get_*`、`set_*`、`mut_*` 构成；列表 getter 返回切片，`mut_*` 暴露对应 `Vec`。
- `KeyRange` 和 `KvKey` 不是本文件实现，而是从 `astersql_br_pkg_logutil` 再导出。

## 执行流程

`RunInNewTxn` 的实际流程是：忽略传入 `KvContext` 与 `_retry`；从 `Storage::meta_transaction` 取得进程内 meta 事务；构造只实现 `Transaction` 的局部 `EmptyTxn`；新建 `KvContext::todo()`；同步调用一次用户闭包并原样返回其 `Result`。`common.rs::GenGlobalIDs` 先调用 `WithInternalSourceType`（当前只原样返回上下文），再进入该流程，通过 `Transaction::meta_transaction` 构造 meta mutator。这里没有真实 begin/commit/rollback、冲突重试或 TiKV I/O。

`context::Context::child_token` 创建独立子 `State`，清理父节点中已失效的弱引用后登记新子节点；若父节点已取消，子节点立即被取消。`cancel` 通过 `cancel_state` 原子地将当前节点从未取消改为已取消、唤醒等待者、收集仍存活的子节点，再递归向下传播。重复取消因 `AtomicBool::swap` 返回旧值而成为幂等操作。`wait_cancelled_timeout` 在加锁前后各检查一次取消状态，再在 `Condvar` 上等待至通知或超时，最终以原子状态决定返回值。

`EncodeMetaKey` 预分配目标容量，依次追加 `m`、mem-comparable 编码的 key、8 字节无符号 `h` 标志、mem-comparable 编码的 field。`DecodeMetaKey` 反向检查前缀，逐段调用 `DecodeBytes`/`DecodeUint`，校验 hash 标志并返回 `(key, field)`。最后一次 `DecodeBytes` 的剩余字节被忽略，因此调用方若要求“输入必须完整消费”，需要在更外层另行校验。

消息壳没有统一执行流程：调用方先用 `Default`/`new` 创建零值对象，再用 setter 或 `mut_*` 填充，最后由 `json.rs`、`misc.rs`、`encryption.rs` 等消费。它们没有实现 protobuf wire encode/decode，也不验证字段之间的业务约束。

## 数据与状态

- `shared_meta_transaction` 是进程级 `OnceLock<astersql_meta::kv::Transaction>`；所有使用默认实现的 `Storage` 共享同一底层 meta 状态。`common_test.rs::gen_global_ids_reuses_storage_state_like_go` 依赖这一点，连续两次分配得到连续 ID。
- 顶层 `KvContext` 含 `Arc<AtomicBool>`，但当前没有公开取消/查询方法，`RunInNewTxn` 也不用它；真正可用的取消实现是独立的 `context::Context`。两者名称相近但不可互换。
- `context::Context` 的克隆共享同一 `Arc<State>`；子 token 拥有独立状态并以弱引用挂在父节点下，避免父子形成强引用环。父取消向下传播，子取消不反向影响父。
- `Row.cells` 和 `Datum(String)` 是字符串化 SQL 数据模型。越界 `GetDatum` 返回空字符串；`set_cell` 会以空字符串扩容，因此缺列与显式空值在此层不可区分。
- proto 壳字段均为拥有型 `String`、`Vec<u8>`、数值、布尔或消息 `Vec`。`Default` 使用空/零/false；枚举默认分别是 `EncryptionMethod::Unknown` 和 `StoreState::Up`。
- `File` 保存文件名、CF、校验摘要、键范围、版本、计数、CRC、大小和 IV；`BackupMeta`/`MetaFile` 聚合文件、范围、schema 与 DDL；`StatsFile` 聚合 `StatsBlock`。字段只表示内存状态，不自动加密、校验 CRC 或维护统计一致性。

## 依赖与调用关系

crate 入口 `br/pkg/utils/lib.rs` 直接声明本模块，并再导出 `kvproto`、`KeyRange`、`KvKey`。`br/pkg/utils/Cargo.toml` 说明该 crate 是 `br/pkg/utils` 的 library 移植，直接依赖 `astersql-meta`、`astersql-parser-types`、`astersql-util-codec`、`astersql-errors` 和 `astersql-br-pkg-logutil`；本文件正是这些依赖的交汇处。

已核对的直接 Rust 消费关系如下：

- `common.rs` → KV/事务桩；`GenGlobalIDs` 是 `RunInNewTxn` 的生产调用者。
- `db.rs` → `RestrictedSQLExecutor`、`ResultField`、`Row`；受限 SQL 的真实执行由 trait 实现者注入。
- `key.rs` → `EncodeMetaKey`、`DecodeMetaKey`、`TableKey`；用于事务 meta key 编解码和 auto-ID 字段识别。
- `json.rs` → `brpb::{BackupMeta,MetaFile,StatsFile,...}`；负责这些普通 struct 的 JSON 转换。
- `encryption.rs` → `CipherInfo`/`EncryptionMethod`；`error_handling.rs` → `brpb::Error`；`misc.rs` → `brpb::File` 与 `metapb::Store`；`store_manager.rs` → `metapb::Store`。
- crate 外，`lightning/pkg/importer/import.rs` 直接构造 `stubs::context::Context`；`br/pkg/task/stream.rs` 的 session 边界返回 `RestrictedSQLExecutor` trait object；`br/pkg/metautil/stubs.rs` 再导出 `CipherInfo` 与 `encryptionpb`。

RustCodeGraph `status` 显示索引包含 7032 个 Rust 文件；`query EncodeMetaKey --kind function` 能定位本文件第 283 行的函数节点。不过对该哈希节点执行 `node/callers/callees` 时返回了无关的 `pkg/parser/ast/base.rs::functionExpression`，因此调用边没有采用该错误结果，而由上述限定路径的 `rg` 引用结果核实。

## 错误处理与边界

`RunInNewTxn` 不包装闭包错误，直接传播 `SharedError`；它也没有重试，即使 `_retry` 为 true。默认事务是进程内共享状态，不能把其成功视为真实集群事务提交成功。

`DecodeMetaKey` 对错误前缀、codec 解码失败和非 `h` 标志返回 `SharedError`；非 `h` 标志以字符显示，与 Go `%c` 文案保持一致。它不拒绝 field 后的尾随字节。`EncodeMetaKey` 仅构造字节，不可能在当前签名内返回错误。

`context::Context` 的两个 `Mutex` 使用 `expect`：锁中毒会 panic；取消递归的深度等于仍存活的后代链深度。`wait_cancelled_timeout` 只返回布尔值，不能区分超时、虚假唤醒和其他等待原因。父取消和子登记并发时通过 children mutex 与取消状态复查保证新子不会漏掉已发生的父取消。

`Row::GetDatum` 对越界静默返回空字符串，`Datum::ToString` 永远成功；这比 Go chunk/Datum 的类型和错误面窄。`RestrictedSQLExecutor` 只定义契约，不提供默认实现。proto 壳没有 required-field、枚举合法性、消息大小、wire compatibility 或敏感字段清零检查，尤其 `CipherInfo.cipher_key` 是普通 `Vec<u8>`。

## 并发与资源生命周期

共享 meta 事务通过 `OnceLock` 只初始化一次并克隆句柄；是否允许并发访问及其内部同步由 `astersql_meta::kv::Transaction` 决定，本文件没有外层串行化。默认 `Storage` 会把所有实例汇聚到同一共享状态；自定义实现可以覆盖 `meta_transaction` 以隔离状态。

`context::Context` 可跨线程克隆：取消标志使用 `SeqCst`，等待者由 `Condvar::notify_all` 唤醒，子节点表由 mutex 保护。父只保存 `Weak<State>`，子释放后不会被父延长生命周期；下次创建子 token 时会清理失效弱引用。取消过程先复制强引用列表再递归，避免递归期间长期持有 children 锁。

`Storage: Send + Sync`、`Transaction: Send` 限定了 trait object 的线程能力；`RestrictedSQLExecutor` 本身没有 `Send`/`Sync` 上界，是否跨线程由具体上层签名决定。消息壳和 SQL 行没有内部锁，变更需通过独占 `&mut`；列表 `mut_*` 借用期间由 Rust 借用规则阻止同一对象上的并发访问。本文件不创建后台任务、网络连接、文件句柄或显式事务 guard。

## 与 Go 版本的对应关系

仓库不存在 `br/pkg/utils/stubs.go`。对应关系应按 API 来源理解：

- `common.go` 使用真实 `kv.WithInternalSourceType`、`kv.InternalTxnBR`、`kv.RunInNewTxn` 与 `kv.Transaction`；Rust 的同名接口仅保证 `GenGlobalIDs` 所需的局部行为，缺少真实事务来源标记和重试/提交语义。
- `db.go` 使用 Go `context.Context`、`sqlexec.RestrictedSQLExecutor`、chunk row 和 planner `ResultField`；Rust 版本把它们缩成取消 token、字符串 cell 和最小字段元数据。Go 调用为受限 SQL 传入内部 BR source context，而当前 Rust `db.rs` 使用默认 `context::Context`，本桩不会附加 source type。
- `key.go` 调用真实 `tablecodec.EncodeMetaKey`/`DecodeMetaKey`。本文件的 codec 顺序、`m` 前缀、hash-data 标志与 Go `pkg/tablecodec/tablecodec.go` 一致；`parity_test.rs` 专门校验非法 flag 的字符错误文案。
- `EncryptionMethod`、`Store` 与 brpb 类型对应 kvproto 生成消息的被使用子集，但当前只是手写字段容器，不承诺 protobuf tag、未知字段保留或 wire compatibility。
- `KeyRange`/`KvKey` 复用 `br/pkg/logutil` 的 Rust 类型，而不是重新仿造。

因此，这个文件的“对齐”目标是让 utils 当前调用点保持必要的数据与控制流，不是替代 Go 的完整基础设施。任何依赖真实事务隔离、RPC、protobuf 序列化或完整 Datum 类型的功能都必须接入 canonical Rust crate，而不能继续扩写桩来伪装完整实现。

## 扩展指南

新增或修改 KV 行为时，先判断它是否仍属于 `GenGlobalIDs` 所需的最小局部接线。若需要真实提交、重试、snapshot 或 TiKV I/O，应替换 `Storage`/`Transaction` 边界并同步 `common.rs`，不要在 `EmptyTxn` 中堆叠假实现；测试应放在独立的 `common_test.rs` 或新的独立 `*_test.rs`，不要内嵌到 `stubs.rs`。

扩展取消行为应集中修改 `context::Context`/`cancel_state`，同时覆盖父取消传播、子取消不反传、取消前后创建 child、超时与多等待者。已有相关独立测试在 `parity_test.rs` 和 `misc_test.rs`。若要支持 deadline 或取消原因，需要明确它们与现有布尔 API 的兼容策略。

扩展 SQL 数据类型时，应优先引入真实或共享的 Datum/row 抽象；若仍扩展 `Row`，必须同步 `db.rs` 与 `db_test.rs`，明确越界、NULL、类型转换和错误语义。实现新的 `RestrictedSQLExecutor` 适配器时，不应假设 trait 会自动附加 BR 内部 source context。

增加 brpb/metapb 字段时，需同步所有 getter/setter/mutator、`json.rs` 的双向映射、`json_test.rs` 夹具以及使用该字段的业务测试。字段敏感性、默认值和 Go protobuf JSON 形态必须逐项核对；若需要 wire 编解码，应迁移到正式 kvproto 依赖，而不是手写一套不兼容协议。

修改 meta key 时必须同时对照 `pkg/tablecodec/tablecodec.go` 和 canonical Rust `pkg/tablecodec/tablecodec.rs`，并更新 `key.rs`、`key_test.rs`/`parity_test.rs` 的正常、非法前缀、非法 flag、截断输入及尾随字节用例。兼容性风险是已有备份 key 无法解析；性能风险主要来自额外分配或复制，当前容量计算避免了常规增长重分配。

## 验证依据

- 源与 crate 边界：`br/pkg/utils/stubs.rs`、`br/pkg/utils/lib.rs`、`br/pkg/utils/Cargo.toml`。
- Rust 直接消费者：`br/pkg/utils/common.rs`、`db.rs`、`key.rs`、`json.rs`、`encryption.rs`、`error_handling.rs`、`misc.rs`、`store_manager.rs`；crate 外抽样为 `lightning/pkg/importer/import.rs`、`br/pkg/task/stream.rs`、`br/pkg/metautil/stubs.rs`。
- Go/canonical 对照：`br/pkg/utils/common.go`、`db.go`、`key.go`、`pkg/tablecodec/tablecodec.go`、`pkg/tablecodec/tablecodec.rs`。同路径没有 `stubs.go`，所以没有把桩特有结构误写成 Go 现有实现。
- 独立测试证据：`br/pkg/utils/common_test.rs` 验证共享 meta 状态；`db_test.rs` 验证 SQL trait mock；`parity_test.rs` 验证取消方向、加密壳与 meta-key 错误文案；`json_test.rs` 验证 `BackupMeta`/`MetaFile`/`StatsFile` 往返；`misc_test.rs` 验证取消等待与 `File` 统计；另有 `key_test.rs`、`error_handling_test.rs`、`store_manager_test.rs` 使用相应边界。
- RustCodeGraph：运行过 `status`、文件级 `explore/node/files`、`query EncodeMetaKey --kind function` 及该节点的 `node/callers/callees`。索引成功识别本文件的 `EncodeMetaKey`，但节点详情/边查询发生错误解析，故调用关系改用限定到目标路径和符号的 `rg` 结果，不采纳错误图边。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务规定的 11 章节结构命令，并人工复核：文件角色、实际流程、桩限制、调用方、Go 差异与安全扩展入口均有路径或符号依据。
