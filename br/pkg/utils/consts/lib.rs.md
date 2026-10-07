# `br/pkg/utils/consts/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-utils-consts` 的 crate 根文件，而不是常量的实际定义文件。`br/pkg/utils/consts/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定该入口，并在 `package.metadata.porting` 中把它映射到 Go 包 `br/pkg/utils/consts`。该 crate 位于 BR 工具链的公共工具层，为流式备份/恢复代码提供统一的 TiKV 列族名称。

本文件只负责模块装配和公开 API 整形：生产构建加载 `consts.rs`，测试构建额外挂载 `parity_test.rs`，随后把 `consts` 模块中的公开项提升到 crate 根。两个列族常量的真实定义位于 `br/pkg/utils/consts/consts.rs`，因此本文件是门面而非业务实现。

## 核心职责

1. 用 `#[path = "consts.rs"] pub mod consts;` 固定实现模块的文件位置，并将模块本身公开。
2. 用 `pub use consts::*;` 再导出实现模块的公开符号，使调用方可以直接写 `astersql_br_pkg_utils_consts::{DefaultCF, WriteCF}`，无需经过 `consts::` 层级。
3. 在 `cfg(test)` 条件下以 `#[path = "parity_test.rs"] mod parity_test;` 挂载独立测试，保持生产源码和 Rust 测试逻辑分文件。
4. 通过 crate 级 `allow` 属性容纳从 Go 迁移而来的命名和阶段性未使用项；这些属性影响编译告警，不改变常量值或运行时行为。

## 主要符号

- `pub mod consts`：公开子模块声明。`#[path]` 明确指向同目录 `consts.rs`，其中定义 `pub const DefaultCF: &str = "default"` 与 `pub const WriteCF: &str = "write"`。
- `mod parity_test`：仅在测试配置下存在的私有测试模块。`br/pkg/utils/consts/parity_test.rs::go_rust_public_contract_matches` 从 crate 根导入两个常量，验证字面量、非空、互异、长度和转换为所有权字符串后的可用性。
- `pub use consts::*`：glob 再导出，是调用方当前直接导入 `DefaultCF`、`WriteCF` 的依据。RustCodeGraph/源码引用显示 `br/pkg/stream/search.rs`、`table_mapping.rs`、`rewrite_meta_rawkv.rs` 和 `stream_metas.rs` 均使用该 crate 根 API。
- crate 级 `#![allow(...)]`：允许 `dead_code`、Go 风格大小写以及若干未使用告警。它是迁移兼容设置，不是对外数据契约；新增原生 Rust API 不应据此放弃惯用命名。

本文件不声明函数、类型、trait、实现块、可变静态量或 feature 分支。

## 执行流程

该文件没有可在运行时调用的入口，其“执行”发生在编译和链接阶段：

1. Cargo 按 `br/pkg/utils/consts/Cargo.toml` 将 `lib.rs` 作为库入口编译。
2. 编译器根据 `pub mod consts` 解析 `consts.rs`，建立公开子模块。
3. 普通构建跳过 `cfg(test)` 模块；测试构建则同时编译独立的 `parity_test.rs`。
4. `pub use consts::*` 把公开常量放入 crate 根命名空间。
5. 下游 crate 通过 Cargo 依赖名 `astersql-br-pkg-utils-consts` 链接该库，并以 Rust 路径 `astersql_br_pkg_utils_consts::{DefaultCF, WriteCF}` 导入常量。
6. 业务代码把常量作为列族标识参与分支判断。例如 `br/pkg/stream/search.rs::Search`/`searchFromDataFile` 将读取结果分为 DefaultCF 和 WriteCF；`br/pkg/stream/table_mapping.rs::ParseMetaKvAndUpdateIdMapping` 的后续处理分别暂存默认列族值或解析写列族提交记录。

因此，门面本身不读取文件、不处理 KV；它通过稳定的符号路径让下游业务逻辑共享相同的协议字符串。

## 数据与状态

门面自身不持有运行时状态。被再导出的两项数据都是进程静态、不可变的 `&'static str`：

- `DefaultCF` 的字节内容为 `default`，表示 TiKV 默认列族，主要承载较长的事务值。
- `WriteCF` 的字节内容为 `write`，表示 TiKV 写列族，承载事务写入类型、开始时间戳及可能的 short value。

`parity_test.rs` 还锁定两者非空、互异，字节长度分别为 7 和 5。常量没有初始化顺序、缓存失效或可变性问题；调用方若需要所有权，可显式转换为 `String`，不会反向修改静态值。

## 依赖与调用关系

`br/pkg/utils/consts/Cargo.toml` 没有 `[dependencies]`，所以本 crate 的实现不依赖外部 Rust crate。它的下游依赖由其他 Cargo 清单声明：`br/pkg/stream/Cargo.toml` 和 `br/pkg/task/Cargo.toml` 都以相对路径 `../utils/consts` 引入该包。

已核对的主要 Rust 使用点如下：

- `br/pkg/stream/search.rs`：按 `DataFileInfo.Cf`/`StreamKVInfo.CFName` 区分两个列族，并在 `mergeCFEntries` 前分别建表。
- `br/pkg/stream/table_mapping.rs`：按列族选择 DefaultCF 暂存路径或 WriteCF 提交/删除/回滚路径，进而维护数据库与表 ID 映射。
- `br/pkg/stream/rewrite_meta_rawkv.rs`：在元数据键和值重写时用 `WriteCF` 决定是否改写时间戳，并用两个常量选择不同的值处理分支。
- `br/pkg/stream/stream_metas.rs`：用 `DefaultCF` 识别需要忽略或参与 shift TS 统计的数据文件。
- `br/pkg/task/stream_test.rs`：测试代码通过该 crate 判断日志文件集合是否含 WriteCF；生产使用面主要集中在 `br/pkg/stream`。

RustCodeGraph 对 `lib.rs` 的文件节点只报告模块装配关系，常量本身也没有可用的函数调用边；这是常量引用和 `pub use` 不形成常规 call graph 的正常结果。直接导入与分支用途通过上述源文件和 Cargo 依赖交叉核验。

## 错误处理与边界

本文件不返回 `Result`、不构造错误，也不执行输入验证。其关键边界是协议和值域边界：

- 常量必须与 TiKV/BR 元数据中的列族名逐字节匹配；大小写或拼写变化会让下游 `==` 分支漏判，而不是在本 crate 内产生显式错误。
- 当前公开契约只覆盖 `default` 与 `write`。遇到其他列族时，具体行为由调用方决定；例如搜索逻辑只为这两个名称建立对应条目，本门面不会兜底或拒绝未知值。
- `pub use consts::*` 会自动扩大 crate 根公开面：以后在 `consts.rs` 新增任意 `pub` 项都会被再导出。扩展时需把这视为公共 API 变更并检查命名冲突。
- crate 级宽松 lint 可能掩盖未使用符号，但不会掩盖类型错误或模块缺失；新代码仍应主动保持精简并接受局部审查。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、事务句柄、文件描述符或网络连接。`&'static str` 常量只读且生命周期覆盖整个程序，可在线程间安全共享，不需要初始化或清理。

测试中的 `Vec<String>` 仅用于证明调用方可以复制常量内容；其分配在测试函数结束或显式 `drop` 时释放，与生产 crate 没有资源生命周期耦合。列族对应的存储读写、事务配对和临时映射生命周期均由 `br/pkg/stream` 等下游模块管理，不属于本文件职责。

## 与 Go 版本的对应关系

Go 对照文件 `br/pkg/utils/consts/consts.go` 在包 `consts` 中定义同名常量：`DefaultCF = "default"`、`WriteCF = "write"`。Rust 的 `consts.rs` 保留了相同名称和字面量；`lib.rs` 则承担 Go 包边界在 Cargo 中的对应入口，并用再导出提供接近 Go 包级符号的使用体验。

两种实现的语义差异主要来自语言组织方式：Go 目录天然形成包且无需单独入口；Rust 需要 Cargo 清单、crate 根、子模块声明和显式 `pub use`。Rust 版本还用 `parity_test.rs::go_rust_public_contract_matches` 直接锁定值和基本边界。Go 同目录未发现专门针对这两个声明的独立单元测试，但 `br/pkg/stream/search_test.go`、`rewrite_meta_rawkv_test.go`、`table_mapping_test.go` 和 `stream_metas_test.go` 通过实际业务路径广泛使用它们；对应 Rust 测试位于各自独立的 `*_test.rs` 文件。

## 扩展指南

新增或调整列族常量时，应在真实定义文件 `br/pkg/utils/consts/consts.rs` 修改公开常量，而不是把定义塞入 `lib.rs`。同时应：

1. 与 `br/pkg/utils/consts/consts.go` 的公开契约核对名称和值；如果 Go 侧没有对应项，要明确这是 Rust 特有扩展及其兼容理由。
2. 更新独立测试 `br/pkg/utils/consts/parity_test.rs`，覆盖字面量、合法边界和误用风险；不要把单元测试内嵌回生产源文件。
3. 检查 `pub use consts::*` 是否应继续自动再导出新符号。若符号只应模块内可见，应降低其可见性或改为显式再导出列表。
4. 搜索所有字符串分支和 Cargo 使用者，重点复查 `br/pkg/stream/search.rs`、`table_mapping.rs`、`rewrite_meta_rawkv.rs`、`stream_metas.rs` 及其独立测试。新增常量本身不会自动使这些流程支持新列族。
5. 评估协议兼容风险：修改现有字面量是高风险行为，可能导致备份文件分类、元数据重写或事务值配对失效。增加静态常量本身几乎无性能成本，但下游新增分支和缓存才可能影响性能。

## 验证依据

- RustCodeGraph `status`：索引包含 7032 个 Rust 文件；`files --filter br/pkg/utils/consts` 确认该目录的 `lib.rs`、`consts.rs`、`parity_test.rs` 与 Go 对照文件均已覆盖。
- RustCodeGraph `node --file br/pkg/utils/consts/lib.rs`：核对 crate 级 allow、`pub mod consts`、条件测试模块和 glob 再导出。
- RustCodeGraph `node --file br/pkg/utils/consts/consts.rs`：核对 `DefaultCF`、`WriteCF` 的类型和值。
- RustCodeGraph `node --file br/pkg/utils/consts/parity_test.rs`：核对独立 parity 测试及其值、非空、互异、长度和所有权转换断言。
- RustCodeGraph 对 `br/pkg/stream/search.rs`、`table_mapping.rs`、`stream_metas.rs`、`rewrite_meta_rawkv.rs` 的文件节点：核对列族常量在搜索分类、事务元数据配对、shift TS 和重写路径中的真实用途。
- `br/pkg/utils/consts/Cargo.toml`：核对 crate 名、库入口、无外部依赖以及 Go 包移植元数据；`br/pkg/stream/Cargo.toml`、`br/pkg/task/Cargo.toml` 核对直接 Cargo 依赖。
- `br/pkg/utils/consts/consts.go`：核对 Go 包级常量字面量；`br/pkg/stream/*.go` 与对应测试引用核对 Go 业务语义。
- 相关 Rust 测试：`br/pkg/utils/consts/parity_test.rs`、`br/pkg/stream/search_test.rs`、`rewrite_meta_rawkv_test.rs`、`table_mapping_test.rs`、`parity_test.rs` 和 `br/pkg/task/stream_test.rs`。本任务为纯文档分析，按计划不运行 Cargo。
