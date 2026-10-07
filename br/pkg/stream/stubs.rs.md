# `br/pkg/stream/stubs.rs`

源码：[`stubs.rs`](stubs.rs)；crate 入口：[`lib.rs`](lib.rs)；依赖声明：[`Cargo.toml`](Cargo.toml)。

## 文件定位

`stubs.rs` 是 `astersql-br-pkg-stream` crate 的本地兼容边界。`lib.rs` 以 `#[path = "stubs.rs"] pub mod stubs` 挂载它，并通过 `pub use stubs::*` 扁平再导出。因此，crate 内部模块既可写 `crate::stubs::Storage`，外部调用方也可能从 crate 根取得这里的公开项。

它不是一个对应单一 Go 文件的业务模块，而是把 Go `br/pkg/stream` 依赖的多个包裁成当前 Rust 移植所需的最小子集：ID 映射、错误、KV、TiKV 可比较编码、TiDB meta 键、model、backuppb、对象存储和日志。源码开头明确限定：JSON 不能替代 protobuf，`MemStorage` 不能替代远端对象存储，meta codec 也只有部分路径与真实格式兼容。

## 核心职责

- 为 [`rewrite_meta_rawkv.rs`](rewrite_meta_rawkv.rs)、[`table_mapping.rs`](table_mapping.rs) 和 [`logging_helper.rs`](logging_helper.rs) 提供 `UpstreamID`、`DownstreamID`、`TableReplace`、`DBReplace` 与构造函数。
- 为 [`meta_kv.rs`](meta_kv.rs)、[`search.rs`](search.rs) 等模块提供 `codec`、`tablecodec`、`meta`、`utils::EncodeTxnMetaKey` 和 `kv::{Entry, KeyRange}`，使事务 meta 键及流备份数据键能够被构造、解析和排序。
- 为 [`stream_mgr.rs`](stream_mgr.rs)、[`stream_metas.rs`](stream_metas.rs)、[`stream_status.rs`](stream_status.rs) 提供精简的 `model` 与 `backuppb` 数据结构。
- 用 `Storage` trait 隔离对象存储操作，并用 `MemStorage` 为独立 Rust 测试提供确定性的内存实现。
- 用轻量 `errors::Error`、`errors::berrors` 和空操作 `log` 减少对尚未移植完成的 Go 错误/日志基础设施的依赖。

该文件的存在目的，是让流备份模块能够围绕已移植逻辑编译和测试；它不表示这些桩已经具备完整生产能力。

## 主要符号

- `UpstreamID`、`DownstreamID` 均为 `i64` 类型别名。`TableReplace` 保存表名、下游表 ID、分区/索引映射及过滤状态；`DBReplace` 再保存库级表映射、过滤状态和复用状态。`NewTableReplace`、`NewDBReplace` 借助 `Default` 初始化空映射和 `false` 标志。
- `LightningPhysicalImportTxnSource = 1 << 16` 是物理导入事务源标记，供 [`meta_kv.rs`](meta_kv.rs) 判断写记录来源。
- `errors::Error` 只保存字符串。`Errorf` 是构造别名，`Annotate`/`Annotatef` 前置上下文，`Trace` 原样返回；`berrors` 中的值是上下文标签而不是结构化 errno。
- `codec` 提供 `EncodeBytes`/`DecodeBytes`、大端 `EncodeUint`/`DecodeUint`、降序 `EncodeUintDesc`/`DecodeUintDesc` 和 protobuf 风格 uvarint。`EncodeBytes` 即使输入长度恰为 8 的倍数也会生成带 padding marker 的终止组，这是 TiKV memcomparable 编码的重要不变量。
- `tablecodec` 提供 meta hash 键、表 record 前缀及 `PrefixNext`；`meta` 则用 `DB:<id>`、`Table:<id>`、`IID:<id>` 等文本格式模拟 DB、表和自增计数键。后者是桩格式，不等同于 TiDB 完整二进制 meta codec。
- `utils::EncodeTxnMetaKey` 组合 `tablecodec::EncodeMetaKey`、`codec::EncodeBytes` 和 `codec::EncodeUintDesc`；`IsMetaDBKey` 仅检查 `mDB` 前缀；`IsSysOrTempSysDB` 识别五个系统/临时库名常量。
- `model::{CIStr, DBInfo, TableInfo, PartitionInfo, ...}` 是可由 `serde_json` 解析的字段子集。`table_simple_from_value` 提取表 ID、原始表名和分区 ID，`db_name_from_value` 提取库的原始名。
- `backuppb` 定义 migration、metadata、文件组、任务状态和 ID 映射等 JSON 模型；各 `Get*` 方法模仿 protobuf 生成 API。`Metadata::Marshal`、`Migration::{Marshal, Unmarshal}` 实际使用 `serde_json`。
- `Storage: Send + Sync` 定义读、写、列举和删除；默认实现还提供存在性检查、逐文件批量删除、非原子 rename 和 `WalkDir`。`MemStorage` 用 `Arc<Mutex<HashMap<String, Vec<u8>>>>` 实现共享内存存储。
- `log::{Info, Warn, Debug}` 全部为空操作，仅为消除日志依赖。

## 执行流程

典型的 meta 键路径如下：调用方把库/表 ID 交给 `meta::*Key` 生成字段，再由 `utils::EncodeTxnMetaKey` 先构造 hash meta 键、整体执行 memcomparable bytes 编码，最后追加按位取反的大端时间戳。读取侧的 [`meta_kv.rs`](meta_kv.rs) 逆向拆分该布局，随后由 [`table_mapping.rs`](table_mapping.rs) 或 [`rewrite_meta_rawkv.rs`](rewrite_meta_rawkv.rs) 分类并改写 DB、表或计数器 ID。

模型路径中，`table_mapping` 从 write/default CF 取得 JSON value 后调用 `model::db_name_from_value` 或 `table_simple_from_value`；后者反序列化 `TableInfo`，遍历可选 `Partition.Definitions`，返回供历史追踪与映射回调使用的 `TableSimpleInfo`。

流元数据路径中，`stream_mgr::MetadataHelper` 读取存储字节并形成 `backuppb::Metadata`，`stream_metas` 再读取/合并 `Migration`、编辑 metadata 并调用桩类型的 `Marshal` 写回。这里的 `Metadata`/`Migration` 序列化为 JSON，不能直接消费 Go 生产环境的 protobuf 文件。

存储路径中，业务模块只依赖 `&dyn Storage`。测试用 `MemStorage` 写入数据和 `.meta` 文件，`search`/`stream_metas`/`stream_mgr` 再通过 `ReadFile`、`ListFiles` 等接口访问；默认 `Rename` 严格按“读源文件、写目标、删源文件”执行，中途失败可能同时保留两份文件。

## 数据与状态

`TableReplace` 和 `DBReplace` 是可变映射快照；构造函数只保证内部 map 初始为空，没有全局注册或持久化。`FilteredOut` 控制映射是否被忽略，`DBReplace::Reused` 表示下游复用了既有数据库。

`backuppb::Metadata` 同时保留 V1 `Files` 与 V2 `FileGroups`，版本分支由 `MetaVersion` 表示；压缩算法只枚举 `UNKNOWN` 和 `ZSTD`。`Migration` 的 `EditMeta`、`Compactions`、`TruncatedTo`、销毁前缀和已导入 SST 路径只是数据载体，合并/应用规则位于 [`stream_metas.rs`](stream_metas.rs)。

`MemStorage` 的唯一共享状态是 `files` map。克隆实例只克隆 `Arc`，因此所有克隆观察同一份内容。`ListFiles` 采用字符串前缀过滤，并按路径字典序排序，以保证测试结果稳定；`DeleteFile` 删除不存在路径仍返回成功。

`CIStr` 的 `L` 字段默认空，当前解析帮助函数使用 `O`。所有 serde 模型大量使用 `#[serde(default)]`，缺失字段会静默变为零值或空集合；这适合兼容测试数据，但可能掩盖生产协议中的必填字段缺失。

## 依赖与调用关系

`Cargo.toml` 将该目录声明为 `astersql-br-pkg-stream` library，`lib.rs` 是入口。本文件自身直接依赖标准库 `HashMap`、`Arc`、`Mutex`，以及 crate 依赖中的 `serde`、`serde_json`；ZSTD、SHA-256、加密等完整处理发生在其他模块，而不是这些数据桩中。

RustCodeGraph 将目标文件标记为被 18 个文件使用。源码级直接消费者包括：

- [`rewrite_meta_rawkv.rs`](rewrite_meta_rawkv.rs)：错误、model、meta、ID 映射和键改写结果。
- [`table_mapping.rs`](table_mapping.rs)：backuppb PITR 映射、JSON model 解析、DB/表替换对象。
- [`meta_kv.rs`](meta_kv.rs)：codec、meta、tablecodec、KV entry 及事务源标记。
- [`search.rs`](search.rs)：`Storage`、`DataFileInfo`、`Metadata` 和 bytes codec。
- [`stream_metas.rs`](stream_metas.rs)：`Storage`、migration/metadata 模型及错误标签。
- [`stream_mgr.rs`](stream_mgr.rs)：`Storage`、model、key range、metadata、meta/table codec。
- [`stream_status.rs`](stream_status.rs)：任务信息与错误状态模型。

图查询精确定位了 `stubs.rs::EncodeTxnMetaKey`、`stubs.rs::NewDBReplace`、`stubs.rs::table_simple_from_value` 和本文件的 `MemStorage`；调用边命令在本次环境中长时间无输出后被中止，因此调用关系以上述已索引文件使用关系和源码直接引用交叉核实，不把该异常解释成“无调用者”。

## 错误处理与边界

编码解码函数用 `Result<_, String>` 返回长度不足、marker 非法、padding 非零、uvarint 溢出、前缀不匹配及 UTF-8/整数解析错误。`DecodeBytes` 会清空传入的复用 buffer；调用者不能期待保留其原内容。`DecodeMetaKey` 验证 `m` 前缀和 hash 标志，但不会验证解码完 field 后是否仍有尾随字节。

`Error::Annotate` 只拼接字符串，`Trace` 不增加调用栈，`berrors` 也没有类型化匹配能力。把它们替换为真实错误体系时，需要保留现有错误文本中被测试断言的部分，同时避免继续依赖字符串等值判断。

JSON 反序列化会拒绝非法 JSON 和字段类型不匹配，但默认字段使缺项合法。`MigrationVersion` 仅列出 M0/M1/M2，`CompressionType` 仅列出 UNKNOWN/ZSTD；新增协议枚举时，需同时处理未知值策略，而不能只扩枚举。

`Storage::FileExists` 把任何 `ReadFile` 错误都解释为“不存在”，会吞掉权限或网络错误。默认 `DeleteFiles` 遇到首个错误立即停止；默认 `Rename` 非原子。`MemStorage` 的 `Mutex::lock().unwrap()` 在锁中毒时 panic，且不存在容量、路径合法性、持久化或远端一致性语义。

## 并发与资源生命周期

`Storage` 要求实现者满足 `Send + Sync`，允许被 `Arc<dyn Storage>` 跨线程共享。`MemStorage` 的每次操作只在访问 map 时持有 `Mutex`，返回的字节和路径均为克隆值，锁不会跨调用方处理阶段持有。克隆的 `MemStorage` 共享 `Arc`，最后一个克隆释放时整张 map 被回收。

文件中没有后台任务、channel、事务或显式 close。`Storage` 默认批量操作是串行的，`Rename` 也没有互斥整个三步序列，因此并发 rename/write/delete 的最终状态没有原子保证。

[`stream_misc_test.rs`](stream_misc_test.rs) 的 `GatedReadStorage` 包装 `MemStorage`，用 `Barrier` 和原子计数验证 `MetadataHelper` 对不同路径的读取可以重叠；这证明上层不会把自己的全局缓存锁跨存储 I/O 持有，但不代表 `MemStorage` 提供分布式一致性。

## 与 Go 版本的对应关系

Go [`rewrite_meta_rawkv.go`](rewrite_meta_rawkv.go) 定义同名 `UpstreamID`、`DownstreamID`、`TableReplace`、`DBReplace` 和构造函数。字段语义一致；差异是 Go 构造函数返回指针且 `TableMap` 的值也是指针，而 Rust 返回拥有所有权的值并存储 `HashMap<UpstreamID, TableReplace>`。

Go [`../utils/key.go`](../utils/key.go) 的 `IsMetaDBKey` 与 `EncodeTxnMetaKey` 是本文件 `utils` 模块的直接语义来源：两者都检查 `mDB` 前缀，并按 EncodeMetaKey → EncodeBytes → EncodeUintDesc 的顺序构造键。Rust `codec`/`tablecodec` 是移植所需子集，并非 Go/TiDB codec 包的完整替代。

Go `stream_mgr.go` 使用真实 `backuppb` protobuf、`storage.ExternalStorage`、ZSTD decoder 和加密管理器；Rust [`stream_mgr.rs`](stream_mgr.rs) 承担部分 helper 行为，但本文件中的消息只用 JSON，`Storage` 也只暴露少量同步方法。两边 wire format 与远端存储能力不能混用。

Go 的 model/backuppb 类型主要来自 TiDB model 与生成代码，本文件只保留当前调用路径读取的字段并手写 `Get*` 方法。新增 Go 字段不应机械地全部塞进本文件；只有 Rust 业务路径实际需要时才扩展，并应核对默认值、序列化字段名和 protobuf/JSON 差异。

## 扩展指南

- 扩展 DB/表改写时，优先修改真实业务模块；仅在需要新的边界类型或字段时调整 `TableReplace`、`DBReplace`，并同步 [`rewrite_meta_rawkv_test.rs`](rewrite_meta_rawkv_test.rs) 与 [`table_mapping_test.rs`](table_mapping_test.rs)。注意 Rust 拥有值与 Go 指针共享语义的差异。
- 扩展 key codec 时，保持 memcomparable 终止组、marker、padding、符号位翻转和降序 TS 不变量；同步 [`meta_kv_test.rs`](meta_kv_test.rs)、[`parity_test.rs`](parity_test.rs)，必要时与 Go `br/pkg/utils/key_test.go` 的字节向量逐项对照。
- 扩展 model/backuppb 时，在本文件补字段、serde 默认/rename 和 getter，并同步实际消费模块的独立测试。若目标是读取真实备份产物，应迁移到真实 protobuf 类型，而不是继续扩大 JSON 桩后宣称协议兼容。
- 接入真实对象存储时实现 `Storage`，明确区分 NotFound 与其他错误，并决定原子 rename、分页列举、重试、取消和一致性语义；不要把 `MemStorage` 的前缀匹配行为当作 S3/GCS 契约。相关测试入口是 [`search_test.rs`](search_test.rs)、[`stream_metas_test.rs`](stream_metas_test.rs)、[`stream_mgr_test.rs`](stream_mgr_test.rs) 和 [`stream_misc_test.rs`](stream_misc_test.rs)。
- 若增加测试，继续放在独立 `*_test.rs` 文件并由 `lib.rs` 的 `#[cfg(test)]` 模块声明接入，不要把测试内嵌进 `stubs.rs`。

兼容性风险主要是 JSON/protobuf wire format、简化 meta key 与真实 TiDB key 的差异；正确性风险主要是默认字段、字符串错误分类和非原子存储默认方法；性能风险主要是文件内容全量克隆、单 `Mutex` 串行访问以及 `ListFiles` 全量排序。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`node --file br/pkg/stream/stubs.rs --offset 1 --limit 400` 与后续 offset 401 查询读取全部 1071 行；`query` 精确定位 `MemStorage`、`EncodeTxnMetaKey`、`NewDBReplace`、`table_simple_from_value`。目标文件的索引摘要列出 18 个使用文件。
- Rust 源与配置：[`stubs.rs`](stubs.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`meta_kv.rs`](meta_kv.rs)、[`rewrite_meta_rawkv.rs`](rewrite_meta_rawkv.rs)、[`table_mapping.rs`](table_mapping.rs)、[`search.rs`](search.rs)、[`stream_metas.rs`](stream_metas.rs)、[`stream_mgr.rs`](stream_mgr.rs)、[`stream_status.rs`](stream_status.rs)。
- Rust 独立测试：[`meta_kv_test.rs`](meta_kv_test.rs) 验证事务 meta 键和 write-CF 边界；[`search_test.rs`](search_test.rs) 验证 codec、SHA-256 和内存存储搜索；[`parity_test.rs`](parity_test.rs) 串联公开契约；[`stream_misc_test.rs`](stream_misc_test.rs) 验证存储包装和并发读取；另有 `rewrite_meta_rawkv_test.rs`、`table_mapping_test.rs`、`stream_metas_test.rs`、`stream_mgr_test.rs` 覆盖直接消费者。未发现专门以 `stubs.rs` 命名的独立测试。
- Go 对照：[`rewrite_meta_rawkv.go`](rewrite_meta_rawkv.go)、[`../utils/key.go`](../utils/key.go)、[`stream_mgr.go`](stream_mgr.go)，以及 `meta_kv_test.go`、`rewrite_meta_rawkv_test.go`、`table_mapping_test.go`、`br/pkg/utils/key_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查固定章节、路径和文档 diff。
