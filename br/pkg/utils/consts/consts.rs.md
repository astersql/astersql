# `br/pkg/utils/consts/consts.rs`

## 文件定位

本文件是 Cargo 包 `astersql-br-pkg-utils-consts` 的实际常量定义文件，源文件为 [`consts.rs`](./consts.rs)。[`lib.rs`](./lib.rs) 通过 `#[path = "consts.rs"] pub mod consts` 挂载模块，并用 `pub use consts::*` 将常量提升到 crate 根，因此调用方使用 `astersql_br_pkg_utils_consts::{DefaultCF, WriteCF}`，无需感知内部模块路径。

[`Cargo.toml`](./Cargo.toml) 将该目录声明为不发布的 workspace library，库入口是 `lib.rs`，没有运行时依赖或 feature。当前可确认的 Cargo 直接依赖者是 `br/pkg/stream/Cargo.toml` 与 `br/pkg/task/Cargo.toml`；它们分别在流备份/恢复逻辑和任务层测试中使用这组列族标识。

## 核心职责

本文件只负责集中定义 BR 识别 TiKV MVCC 两个核心列族时使用的稳定字符串：默认列族 `"default"` 和写列族 `"write"`。它不解析键值、不访问存储，也不决定恢复策略；真正的分流逻辑位于调用者，例如 `br/pkg/stream/table_mapping.rs`、`search.rs` 和 `rewrite_meta_rawkv.rs`。

集中定义的价值在于避免各模块重复硬编码协议字符串。对 BR 而言，这两个值不仅是展示名称，还会与备份元数据中的 `Cf`/`CFName` 字段比较，从而决定值体如何解析、时间戳如何处理，以及 default/write 两侧是否需要配对。字面量变化因此属于跨 Go/Rust 和存储格式的兼容性变化，而不是普通重命名。

## 主要符号

- `pub const DefaultCF: &str = "default"`：公开的默认列族名。该列族保存未内联在 write 记录中的用户值；在 `stream/search.rs::searchFromDataFile` 中，其键尾时间戳记为 `StartTs`，值被编码到搜索结果的 `Value`。
- `pub const WriteCF: &str = "write"`：公开的写列族名。该列族保存事务写记录；调用方会解析 write type、start timestamp 和可选 short value。例如 `stream/search.rs::searchFromDataFile` 将其解析为 `RawWriteCFValue`，`stream/rewrite_meta_rawkv.rs::rewriteValue` 则按 Rollback/Delete/Put 分支处理。

两者类型都是不可变的 `&'static str` 常量，没有构造函数、trait、`impl`、条件编译项或私有辅助符号。名称保留 Go 风格的 PascalCase；`lib.rs` 在 crate 级允许 `non_upper_case_globals`，说明这是一项有意的跨语言 API 对齐。

## 执行流程

本文件自身没有可执行流程；常量在编译期内联为静态字符串引用。它们进入应用逻辑的典型路径如下：

1. 调用方从 crate 根导入 `DefaultCF`/`WriteCF`；入口再导出由 `lib.rs::pub use consts::*` 提供。
2. 备份元数据或调用参数携带一个 CF 字符串，业务代码将其与常量做精确、区分大小写的字符串比较。
3. `table_mapping.rs::ParseMetaKvAndUpdateIdMapping` 按 CF 分支：DefaultCF 暂存 DB/表值，WriteCF 消费提交记录并完成配对；其他 CF 在相关 DB/表分支返回 `unsupported column family`。
4. `search.rs::Search`/`searchFromDataFile` 将 default 与 write 条目分别建表，解析各自格式，再由 `mergeCFEntries` 按事务时间戳关系合并。
5. `rewrite_meta_rawkv.rs` 在 WriteCF 路径重写提交时间戳并解析写记录，在 DefaultCF 路径直接重写完整值；未知 CF 在 `rewriteValue` 中不受支持。

这说明常量是分流判别值，而不是列族行为的实现位置。

## 数据与状态

两个常量都是 UTF-8 ASCII 字面量，生命周期为 `'static`，分别为 7 字节和 5 字节。它们不持有可变状态、不进行分配，也没有初始化顺序问题。直接比较 `&str` 不改变输入；仅当调用方写入拥有型字段时才显式调用 `.into()` 或 `.to_string()` 产生 `String`。

关键不变量由 [`parity_test.rs`](./parity_test.rs) 明确覆盖：值必须分别等于 `"default"` 和 `"write"`，两者非空且互不相等，字节长度稳定为 7 和 5，并可安全转换为拥有型字符串。这些长度断言是当前测试契约；业务分流的根本契约仍是完整字节值匹配。

## 依赖与调用关系

本文件不导入任何 Rust 模块或外部 crate，下游依赖为零。它经 `lib.rs` 暴露后，已确认的生产使用集中在 `br/pkg/stream`：

- `stream_metas.rs` 导入 `DefaultCF`，用于识别 default CF 的备份元数据。
- `table_mapping.rs` 同时导入两个常量，在 `ParseMetaKvAndUpdateIdMapping` 中选择 default 暂存或 write 提交流程。
- `search.rs` 同时导入两个常量，在 `Search` 和 `searchFromDataFile` 中拆分、解析并合并两类条目。
- `rewrite_meta_rawkv.rs` 同时导入两个常量，在 `rewriteKeyForDB`、`rewriteValue` 等路径中区分时间戳和值体重写规则。

`br/pkg/task/Cargo.toml` 也直接依赖此 crate；`br/pkg/task/stream_test.rs` 使用两常量验证 WriteCF 日志文件识别。另有若干独立 Rust 测试直接导入它们，包括 `br/pkg/stream/parity_test.rs`、`search_test.rs`、`rewrite_meta_rawkv_test.rs` 和 `table_mapping_test.rs`。

需要区分的是，`br/pkg/restore/log_client/stubs.rs` 还存在同名字面量，restore/log_client 内部通过 `crate::stubs::consts` 使用那组桩常量；它不是本 crate 的直接调用者。这是当前移植结构中的重复定义，不能把该 restore 路径误记为本文件的调用边。

## 错误处理与边界

常量定义本身不会返回错误，也没有 `unsafe`、索引、解析或 I/O。边界风险来自消费者对字符串的分支处理：比较是精确匹配，大小写变化、前后空白或其他列族名均不会命中这两个常量。

调用者对未知 CF 的策略并不完全相同：`table_mapping.rs::ParseMetaKvAndUpdateIdMapping` 在相关元数据分支返回带 CF 值的错误；`rewrite_meta_rawkv.rs::rewriteValue` 对既非 DefaultCF 又非 WriteCF 的输入执行 `panic!`；`search.rs::Search` 对未命中的条目不纳入 default/write 合并表。因此新增列族不能只在本文件增加常量，必须逐一审查消费者的拒绝、跳过和解析行为。

另一个兼容边界是空 CF：本 crate 不将空串定义为 DefaultCF；某些 Go/Rust 恢复逻辑可能在自己的边界层把空值视为 default，但那是调用者的兼容规则，不应扩写成本文件的常量语义。

## 并发与资源生命周期

两个 `&'static str` 常量天然可被任意线程共享，没有锁、原子变量、通道、异步任务、事务或析构顺序。读取常量不分配资源，也无需关闭或清理。

当调用方把常量转换为 `String` 写入备份元数据结构时，所有权和释放由该结构管理；这不改变常量自身的静态生命周期。并发正确性位于消费模块，例如文件读取、映射表或批处理的同步策略，本文件不提供任何并发保证之外的不可变共享值。

## 与 Go 版本的对应关系

Go 对照文件为 [`consts.go`](./consts.go)，包名同为 `consts`，并在一个 `const` 块中定义 `DefaultCF = "default"` 与 `WriteCF = "write"`。Rust 版本在名称、可见性意图和字节值上逐项一致；`parity_test.rs::go_rust_public_contract_matches` 专门锁定这一公开契约。

语言层差异仅在表示方式：Go 常量是无显式类型的字符串常量，Rust 常量显式为 `&str`，并由独立的 crate 入口再导出。Rust 消费者给拥有型 `String` 字段赋值时需要 `.into()`/`.to_string()`，Go 侧则可直接赋给 string 字段。当前文件不是门面或桩；它是 Rust crate 中的真实定义，但 restore/log_client 的移植桩仍保留另一组局部同名常量，体现了尚未完全统一依赖边界的现状。

## 扩展指南

若要新增或调整列族标识，应把它视为协议与分流扩展，而不是仅添加一行常量：

1. 先核对 TiKV/备份元数据使用的真实 CF 字节值，并同步 `consts.go` 与本文件；若 Go 上游没有对应增量，应明确记录 Rust 特有依据，不能自行发明兼容语义。
2. 在本 crate 的独立测试文件 `parity_test.rs` 增加字面量、非空/互异等契约断言；遵守仓库要求，不把测试内嵌到 `consts.rs`。
3. 审查所有 CF 分流点，至少包括 `table_mapping.rs`、`search.rs`、`rewrite_meta_rawkv.rs` 及其同目录独立测试，决定新 CF 应解析、透传、拒绝还是跳过。
4. 如果希望 restore/log_client 共享此 crate，应作为单独的依赖接线任务处理，移除重复桩之前先证明所有行为与测试保持一致；不要在常量文档任务中悄然改变依赖图。

主要兼容风险是旧备份无法识别变更后的字面量或错误进入另一解析分支；正确性风险是把不同值体格式按 WriteCF/DefaultCF 错解；性能风险通常不在常量本身，而在新增分流是否引入额外复制、哈希表或跨文件配对。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/utils/consts` 确认本目录的 `consts.rs`、`lib.rs`、`parity_test.rs` 与 Go 对照文件均被索引。
- RustCodeGraph `node --file`：完整读取 `consts.rs` 和 `lib.rs`，确认仅有两个常量，以及模块挂载、测试模块和 crate 根再导出关系。
- RustCodeGraph `query DefaultCF/WriteCF`：定位到 `br/pkg/stream` 的导入和相关分流符号；图对常量的 `callers/callees` 未给出有效直接边，因此用精确文本引用查询补齐直接证据，未把“0 个使用文件”误当成无人使用。
- Cargo 证据：读取 `br/pkg/utils/consts/Cargo.toml`，并核对 `br/pkg/stream/Cargo.toml`、`br/pkg/task/Cargo.toml` 对 `astersql-br-pkg-utils-consts` 的路径依赖。
- Go/测试证据：读取 `consts.go` 与 `parity_test.rs`；引用搜索还核对了 `br/pkg/stream/{parity_test,search_test,rewrite_meta_rawkv_test,table_mapping_test}.rs` 和 `br/pkg/task/stream_test.rs`。
- 行为抽样：通过 RustCodeGraph 读取 `table_mapping.rs::ParseMetaKvAndUpdateIdMapping`、`search.rs::Search/searchFromDataFile`、`rewrite_meta_rawkv.rs::rewriteValue` 的相关源码，验证常量实际承担 CF 分流判别作用。
- 按任务约束，本任务是纯文档分析，未运行 Cargo 或代码测试；交付验证采用固定章节结构检查和人工事实复核。
