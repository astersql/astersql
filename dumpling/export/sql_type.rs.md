# `dumpling/export/sql_type.rs`

## 文件定位

`sql_type.rs` 属于 `astersql-dumpling-export` library crate。crate 根文件 [`dumpling/export/lib.rs`](lib.rs) 通过 `include!("sql_type.rs")` 将它并入与 Go `dumpling/export` 包相近的单包命名空间，而不是声明独立 Rust module；因此本文件可直接使用 crate 根引入的 `HashSet`、`OnceLock`，以及先于它载入的 [`stubs.rs`](stubs.rs) 中的 `RawBytes` 和 [`ir.rs`](ir.rs) 中的 `RowReceiver`。

它位于数据库结果解码和导出格式 writer 之间：一部分代码把数据库原生类型名分为字符串、整数、数值和二进制集合；另一部分代码把一行的各列保存为 `Option<Vec<u8>>`，供 SQL、CSV、Parquet writer 消费。它不解析 SQL 类型声明，也不把原始字节转换成 Rust 数值。

[`dumpling/export/Cargo.toml`](Cargo.toml) 将 crate 定义为 `astersql-dumpling-export`，`[lib] path = "lib.rs"`，porting metadata 对应 Go 包 `dumpling/export`。本文件没有自己的 feature 或直接外部依赖；最终的 SQL/CSV/Parquet 编码由 crate 依赖的 dumpformat crates 和 `parquet` 完成。

## 核心职责

1. `initColumnTypeSets` 以进程级、惰性且线程安全的方式构造四个 MySQL/TiDB `DATA_TYPE` 名集合。
2. `dataType*Contains` 系列函数提供大小写敏感的精确分类查询。实际生产调用中，`dataTypeIntContains` 限制可作为导出分片键的整数类型；`dataTypeNumContains` 和 `dataTypeBinContains` 决定 CSV/SQL writer 的字段类别。当前 Rust 生产路径未引用 `dataTypeStringContains`，它仍保留 Go 对齐的分类能力。
3. `MakeRowReceiver` 和 `RowReceiverArr` 为指定列数建立行接收缓冲区，并在 `SQLRowIter::Decode` 后以借用或克隆方式向不同 writer 暴露原始值。

类型集合只影响后续编码策略或分片键选择，不决定 `RowReceiverArr` 的槽位类型：所有列统一表示为原始字节，`None` 表示 SQL `NULL`，`Some(vec![])` 表示非 NULL 的空值（`RawBytes` 定义见 [`stubs.rs`](stubs.rs)）。

## 主要符号

- `COLUMN_TYPES: OnceLock<()>`：四个集合的统一初始化哨兵。闭包完成后才发布 `()`，保证其他线程不会观察到部分初始化状态。
- `DATA_TYPE_STRING`：包含字符、日期时间、文本、`ENUM`/`SET`、`JSON`、`NULL`、`VAR_STRING` 等名称。
- `DATA_TYPE_INT`：包含有符号整数别名及 Go MySQL driver 可返回的若干 `UNSIGNED ...` 名称。
- `DATA_TYPE_NUM`：包含 `DATA_TYPE_INT` 的全部名称，再加浮点、定点、布尔类型。Rust 源码显式重复整数列表，而不是在运行时从整数集合派生。
- `DATA_TYPE_BIN`：包含 blob、binary、`BIT` 和 `GEOMETRY` 名称。
- `initColumnTypeSets()`：通过 `COLUMN_TYPES.get_or_init` 一次性填充四个私有 `OnceLock<HashSet<&'static str>>`。
- `dataTypeStringContains`、`dataTypeIntContains`、`dataTypeNumContains`、`dataTypeBinContains`：先确保初始化，再查询对应集合。
- `MakeRowReceiver(col_types: &[String]) -> RowReceiverArr`：只使用 `col_types.len()`，创建同长度、全为 `None` 的缓冲区；具体类型不会改变接收表示。
- `RowReceiverArr { pub bound: bool, data: Vec<Option<Vec<u8>>> }`：持有当前行。`bound` 是可观察的迁移兼容状态；`data` 保持私有，避免 writer 直接改写。
- `RowReceiver for RowReceiverArr::BindAddress`：把传入 `RawBytes` 的内容克隆到内部槽位并设置 `bound = true`。
- `rawValues(&self)`：零拷贝借用内部 `Option<Vec<u8>>` 切片，SQL writer 热路径使用它。
- `GetRawBytes(&self)`：克隆整行并包装为新的 `Vec<RawBytes>`，适合需要拥有值的调用者。
- `appendRawBytes(&self, dst: &mut Vec<RawBytes>)`：把当前行克隆追加到调用方缓冲区，不会先清空 `dst`。

这些公开名称保留 Go 风格大小写；`lib.rs` 在迁移期允许 `non_snake_case` 等 lint。

## 执行流程

类型分类流程如下：调用方把 `ColumnType::DatabaseTypeName` 或元数据中的大写类型名传给 `dataType*Contains`；第一次查询触发 `initColumnTypeSets`；`OnceLock` 闭包建立所有集合；查询函数对输入字符串做精确 `HashSet::contains`。例如 [`writer_util.rs`](writer_util.rs) 的 `columnKinds` 先查 binary，再查 numeric，其余均降级为 string；[`sql.rs`](sql.rs) 的 `getNumericIndex` 只接受 `dataTypeIntContains` 命中的首索引列。

行数据主流程是：

1. writer 以 `meta.ColumnTypes()` 调用 `MakeRowReceiver`，得到与列数相同的空槽位。
2. `SQLRowIter::Decode` 最终进入 [`ir.rs`](ir.rs) 的 `decodeFromRows`。它在 `Rows::Scan` 前调用一次 `BindAddress`，保留 Go 路径“先绑定扫描目标”的可观察副作用；扫描成功后再次调用，把已填充的 `RawBytes` 克隆进 receiver。
3. SQL writer 通过 `rawValues` 借用数据；CSV writer 先清空复用的 `Vec`，再调用 `appendRawBytes`；Parquet writer 通过 `GetRawBytes` 获得独立所有权，然后提取内部 `Option<Vec<u8>>`。
4. 下一行解码会再次调用 `BindAddress` 并覆盖已有槽位。Rust 的 `bound` 不用于短路复制，这一点是相对于 Go 指针绑定方式的必要适配。

若结果没有选中字段，writer 路径会跳过 `Decode` 并输出空行/空字段集合，避免读取初始化为 NULL 的 receiver 槽位（见 `writer_util.rs` 的 SQL 与 CSV 循环）。

## 数据与状态

四个分类表保存 `&'static str`，初始化后只读，生命周期覆盖整个进程。集合的包含关系中，所有整数名同时属于 `DATA_TYPE_INT` 与 `DATA_TYPE_NUM`；binary 与 string 集合各自独立。查询不做 trim、大小写转换或类型修饰解析，因此 `"INT"` 命中，而 `"int"`、`"INT(11)"` 或未知名称不会命中，调用方必须传数据库驱动提供的规范类型名。

每个 `RowReceiverArr` 拥有自己的 `Vec<Option<Vec<u8>>>`。`None`、空字节和任意非 UTF-8 字节彼此可区分；本层不假定文本编码。`rawValues` 的借用只在 receiver 下一次可变绑定前有效；`GetRawBytes` 和 `appendRawBytes` 通过深克隆让结果脱离 receiver 生命周期，但成本与该行总字节数成正比。

`BindAddress` 使用 `zip`：若 `args` 比接收槽少，多余参数被忽略；若 `args` 更短，未配对槽位保留上一行值。正常路径由相同列元数据构造 receiver 和扫描参数，必须维持长度一致这一隐含不变量。本文件自身不检查或报告长度不匹配。

## 依赖与调用关系

上游与下游的关键边如下：

- `lib.rs -> include!("sql_type.rs")`：把所有符号放入 export crate 根命名空间，并在测试配置下挂载 `sql_type_test.rs`。
- `writer_util.rs::{writeSQLFile, writeCSVFile, WriteInsertInParquet} -> MakeRowReceiver`：分别形成 SQL、CSV、Parquet 的行缓冲区。
- `writer_util.rs::columnKinds -> dataTypeBinContains/dataTypeNumContains`：把数据库类型映射为 dumpformat 的 `Bytes`、`Number`、`String`。
- `sql.rs::getNumericIndex -> dataTypeIntContains`：只允许整数列参与导出任务的数值分片键选择。
- `ir.rs::decodeFromRows -> RowReceiver::BindAddress`：扫描前后绑定；`sql_type.rs` 提供实际的 `RowReceiverArr` 实现。
- SQL writer 使用 `rawValues`，CSV writer 使用 `appendRawBytes`，Parquet writer 使用 `GetRawBytes`。`parity_test.rs` 还把 `GetRawBytes` 结果送入 SQL dumpformat writer，验证数值不被加引号。

本文件直接依赖 crate 根可见的标准库 `HashSet`/`OnceLock`、`stubs.rs::RawBytes` 和 `ir.rs::RowReceiver`。它不直接执行 I/O、持有数据库连接或调用 dumpformat；格式化和错误传播都留给调用者。

## 错误处理与边界

本文件 API 均不返回 `Result`。分类失败表现为 `false`，未知类型在 `columnKinds` 中安全降级为 string，在整数分片键选择中被排除。分类严格区分大小写，不会自动纠正非规范输入。

初始化闭包内对每个私有 `OnceLock::set` 使用 `unwrap`。在当前封装下只有 `COLUMN_TYPES` 闭包能设置这些静态值，所以正常调用不会失败；若将来新增其他初始化入口，重复 `set` 会 panic，必须同步重构统一哨兵设计。

行接收不验证参数长度；长度不一致可能静默忽略值或遗留旧值。`appendRawBytes` 的契约是“追加”而非“替换”，调用者若希望只得到当前行，应先 `clear` 或传空 vector。克隆分配失败属于 Rust 进程级内存分配失败，不在本层恢复。

`RawBytes(None)` 与 `RawBytes(Some(vec![]))` 必须保持不同：[`sql_type_test.rs`](sql_type_test.rs) 明确验证 NULL、空字节以及未知列类型；非 UTF-8 字节也按原样保存。`decodeFromRows` 的扫描错误由 `ir.rs` 关闭 rows 并返回错误，本文件不会吞掉或制造该错误。

## 并发与资源生命周期

分类集合由 `OnceLock` 同步初始化，可由多个线程并发查询；初始化完成后只读，无锁修改或清理阶段。四个集合依托静态存储，不持有外部资源。

`RowReceiverArr` 没有内部锁，也没有显式 `Send`/`Sync` 实现；其字段本身可安全移动，但设计用途是由单个导出迭代循环独占并逐行修改。writer 必须在下一次 `BindAddress` 前消费 `rawValues` 的借用；需要跨迭代或缓冲行时应使用会克隆的两个 owned 输出方法。

本文件不创建线程、任务、通道、文件、数据库连接或事务，也不负责关闭资源。结果集推进与关闭属于 `SQLRowIter`/`Rows`，writer 的 flush/close 属于各格式 writer。

## 与 Go 版本的对应关系

Go 对照实现是 [`dumpling/export/sql_type.go`](sql_type.go)。四组类型名和总体用途保持一致：Go 在 `initColumnTypeSets` 中填充 package-level map；Rust 用 `OnceLock<HashSet<_>>` 实现可从任一查询函数安全惰性调用的线程安全初始化。Go 的 `dataTypeNumArr` 由整数数组追加构造，Rust 显式列出同一批整数名，因此扩展整数类型时必须同时维护 Rust 的整数与数值列表。

Go 的 `MakeRowReceiver` 返回指针，Rust 返回拥有值；两者都只使用列数。Go `BindAddress` 首次调用时把 `args[i]` 设为内部 `sql.RawBytes` 的指针，此后因 `bound` 而直接返回；Rust 的 stub `Rows::Scan` 接收值槽位，无法保留同样的指针关系，所以 Rust 每次都从 `args` 克隆数据，且 `ir.rs::decodeFromRows` 在成功扫描后第二次绑定。这不是简单逐句翻译，而是为当前 Rust Rows adapter 保留实际行为的适配。

Go `GetRawBytes`/`appendRawBytes` 复制 slice header 中的 `sql.RawBytes`，底层字节可能共享；Rust 的 `Vec<u8>` 所有权模型使这两个方法深克隆字节。Go 生产代码仍直接查询 package maps，Rust 通过 contains helpers 查询；当前 `dataTypeStringContains` 在 Rust 生产路径未接线，而 Go 的 string map仍见于 Go 测试分类逻辑。

仓库未提供独立的 `sql_type_test.go`；Go 侧相关行为分散在 `sql_test.go`、`writer_util.go`/其测试及 dump 路径中。Rust 的直接对齐回归集中在 `sql_type_test.rs`，另由 `parity_test.rs` 和 `writer_util_test.rs` 覆盖 writer 集成行为。

## 扩展指南

- 新增或修正数据库类型名时，修改 `initColumnTypeSets` 的对应数组，并同步检查 Go `sql_type.go`。整数类型必须同时加入 `DATA_TYPE_INT` 和 `DATA_TYPE_NUM`；否则分片键选择与 writer 数值分类会漂移。
- 若类型名来源可能变为小写、带长度或带 `UNSIGNED` 后缀的新形式，不要只在调用点零散规范化；应先确认 driver/INFORMATION_SCHEMA 契约，再统一调整 contains 边界并增加大小写、修饰符和未知类型测试。
- 改变 binary/numeric 优先级会影响 CSV 与 SQL 的转义/引号行为，应同步扩展 `sql_type_test.rs`、`writer_util_test.rs` 和 `parity_test.rs`，并检查 Go 同类测试预期。
- 改动 `RowReceiverArr` 时必须保留 NULL、空字节、非 UTF-8、跨行覆盖和列数一致性。新增测试继续放在独立的 `sql_type_test.rs`，不要嵌入生产文件。
- 如需消除每行深克隆，应从 `Rows::Scan -> decodeFromRows -> RowReceiver -> writer` 整条所有权链设计借用方案，不能只让 `GetRawBytes` 返回内部引用后跨下一次 Decode 使用。该改动同时影响 SQL、CSV、Parquet 三条路径，兼容和生命周期风险较高。
- 若要把长度不匹配变成错误，需要修改 `RowReceiver::BindAddress` trait 签名及所有实现/调用者，属于跨文件协议变更；单独在这里 panic 会改变当前静默行为。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 7,032 个 Rust 文件；`files --filter dumpling/export` 确认目标、Go 对照、测试及模块入口均已索引；`query MakeRowReceiver`、`query dataTypeStringContains`、`query BindAddress`、`query RawBytes` 确认定义位置与签名。精确 callers/callees 命令未产出稳定结果，自然语言查询出现跨模块同名串扰，因此调用边又以限定在 `dumpling/export` 的 `rg` 引用搜索核验，未采用串扰结果。
- 目标实现：[`sql_type.rs`](sql_type.rs) 的四个静态集合、五个分类函数、`MakeRowReceiver`、`RowReceiverArr` 及其 trait/inherent impl。
- crate 与协议：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`ir.rs`](ir.rs) 的 `RowReceiver`/`decodeFromRows`、[`stubs.rs`](stubs.rs) 的 `RawBytes`/`Rows`。
- 生产调用：[`writer_util.rs`](writer_util.rs) 的 SQL、CSV、Parquet 行循环与 `columnKinds`；[`sql.rs`](sql.rs) 的整数分片键筛选。
- Go 对照：[`sql_type.go`](sql_type.go)，以及引用分类 map/receiver 的 `sql_test.go`、`writer_util.go`、`dump.go`。
- Rust 测试：[`sql_type_test.rs`](sql_type_test.rs) 验证数值分类、未知类型、NULL/空字节、追加语义、非 UTF-8 与跨行刷新；[`parity_test.rs`](parity_test.rs) 验证 `INT` 原始值进入 SQL writer 后输出 `(42)`；[`writer_util_test.rs`](writer_util_test.rs) 验证 SQL/CSV/Parquet 集成路径。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收以固定十一个二级标题检查；人工复核重点是文件存在理由、初始化/解码/消费流程、真实调用边、Go 差异和安全扩展点。
