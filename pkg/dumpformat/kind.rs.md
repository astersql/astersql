# `pkg/dumpformat/kind.rs`

## 文件定位

源文件 [`kind.rs`](./kind.rs) 定义 `astersql-dumpformat` crate 的共享字段分类 `FieldKind`。该 crate 由 [`Cargo.toml`](./Cargo.toml) 声明，库入口是 [`lib.rs`](./lib.rs)；入口以私有 `mod kind` 装载本文件，再用 `pub use kind::FieldKind` 将类型暴露为 `astersql_dumpformat::FieldKind`。因此调用方依赖 crate 的公共根路径，而不是直接访问 `kind` 模块。

这个类型位于 Dumpling 格式化链的“上游 SQL 类型判断”和“下游具体格式编码”之间。`dumpling/export/writer_util.rs::columnKinds` 将列类型名归并为三类；`pkg/dumpformat/csvfile` 与 `pkg/dumpformat/sqlfile` 的 writer 保存逐列 `FieldKind`，在写每一行时把它交给各自的字段编码器。本文件只表达分类，不拥有 SQL 类型表、CSV/SQL 语法、行缓冲或输出资源。

`astersql-dumpformat` 自身没有声明依赖项或 feature。直接消费它的两个相邻 crate 在 `pkg/dumpformat/csvfile/Cargo.toml` 和 `pkg/dumpformat/sqlfile/Cargo.toml` 中通过 `astersql-dumpformat = { path = ".." }` 建立依赖；Parquet 子系统不消费这个共享分类。

## 核心职责

- 用 `FieldKind::Number`、`FieldKind::String`、`FieldKind::Bytes` 表达格式 writer 真正需要的三种渲染语义，而不把完整 MySQL 类型系统耦合到各格式 crate。
- 为 CSV 和 SQL writer 提供同一份、可按值复制的列分类，使同一张表的列类型判断可以复用于逐行编码。
- 通过 Rust 枚举和下游穷尽 `match` 约束分类处理：新增变体会迫使显式匹配它的下游代码在编译期重新决定行为。

本文件不负责从数据库元数据推导分类。当前实际分类入口是 `dumpling/export/writer_util.rs::columnKinds`：二进制类型映射为 `Bytes`，数值类型映射为 `Number`，其余类型默认映射为 `String`。它也不负责验证传入字节是否真是合法数字或字符串；分类正确性由构造 `Vec<FieldKind>` 的上游承担。

## 主要符号

- `pub enum FieldKind`：文件内唯一类型，也是唯一公共 API。它派生 `Clone`、`Copy`、`Debug`、`PartialEq`、`Eq`，因此可廉价按值传递、打印调试并进行相等比较；没有派生 `Default`、哈希、排序或序列化能力。
- `FieldKind::Number`：表示应作为数值文本直接写出的字段。CSV 和 SQL 编码器都不为其添加引号，也不校验其词法形式。
- `FieldKind::String`：表示需要按目标格式进行包围与转义的文本字段。CSV 使用配置的包围/转义规则，SQL 使用单引号及相应转义规则。
- `FieldKind::Bytes`：表示二进制字段。CSV 根据 `BinaryFormat` 选择 HEX、Base64 或文本式转义；SQL 固定生成小写十六进制的 `x'...'` 字面量。

文件中没有模块级常量、函数、trait、`impl` 或条件编译项。枚举没有 `#[repr(...)]`，也没有显式判别值；其内存布局和数值表示不是公共协议，调用方不应通过整数转换、裸内存或跨进程序列化依赖当前变体顺序。

## 执行流程

1. Dumpling 从 `TableMeta::ColumnTypes` 取得每列类型名；`dumpling/export/writer_util.rs::columnKinds` 依次调用 `dataTypeBinContains` 和 `dataTypeNumContains`，产生与列顺序一致的 `Vec<FieldKind>`，未命中两者的类型归为 `String`。
2. CSV 路径把该向量交给 `pkg/dumpformat/csvfile/writer.rs::Writer::new`。每行通过宽度校验后，`Writer::write_borrowed` 以列下标取出一个可复制的 `FieldKind`，调用 `csv.rs::append_field`。
3. CSV 编码先独立处理空值；非空 `Number` 原样输出，`Bytes` 可进入 HEX/Base64 分支，其余情况（包括 `String` 和 UTF8 模式的 `Bytes`）进入包围与转义流程。表头不使用数据列分类，而是由 `write_header` 明确传 `FieldKind::String`。
4. SQL 路径把同类向量交给 `pkg/dumpformat/sqlfile/writer.rs::Writer::new`。`Writer::write` 校验行宽后逐列调用 `sql.rs::append_value`；空值优先输出 `NULL`，否则按三种变体分别输出裸数值、`x'十六进制'` 或带引号的转义字符串。
5. 各 writer 在分类完成后继续处理行/语句分隔、大小统计和 sink I/O。本文件不参与这些步骤，也不会在运行时改变某列的分类。

这条链路的关键不变量是 `kinds[i]` 必须描述行内第 `i` 个值。两个 Rust writer 都先验证值数量与 `kinds.len()` 一致，避免下标错位；但它们无法判断上游是否给某列选择了错误变体。

## 数据与状态

`FieldKind` 是无负载（fieldless）枚举，每个值只携带三选一的分类身份，不保存列名、原始 SQL 类型、字符集、排序规则、是否为空、二进制输出格式或转义配置。后几项分别来自当前值、CSV/SQL 配置及调用参数。

派生 `Copy` 使 writer 可以从 `Vec<FieldKind>` 按下标取值而无需转移所有权或克隆堆数据；枚举本身没有堆分配。`Vec<FieldKind>` 的所有权和列顺序由 `csvfile::Writer` / `sqlfile::Writer` 保存，`FieldKind` 没有内部可变状态，也不存在分类缓存。

Rust 类型没有 `Default`。这与 Go 对照的零值行为不同：Go 的 `FieldKind uint8` 零值等于 `KindNumber`，Rust 调用方则必须显式构造某个变体。Rust 枚举在安全代码中也不能表示任意未知整数，因此错误分类通常来自上游选择错误，而不是出现第四种未定义值。

## 依赖与调用关系

主要上游与下游关系为：

`TableMeta::ColumnTypes` → `dumpling/export/writer_util.rs::columnKinds` → `FieldKind` 向量 → CSV/SQL `Writer` → CSV `append_field` 或 SQL `append_value`。

公共暴露和 crate 接线如下：

- `pkg/dumpformat/lib.rs` 私有声明 `mod kind` 并公开再导出 `FieldKind`。
- `pkg/dumpformat/csvfile/csvfile.rs` 再导出 `astersql_dumpformat::FieldKind`，所以 Dumpling CSV 路径使用的是 `astersql_dumpformat_csvfile::FieldKind`，但实际类型仍由本文件定义。
- `pkg/dumpformat/csvfile/writer.rs` 的 `Writer::new` 接收 `Vec<FieldKind>`；`csv.rs::append_field` 读取单个值并按 `Number` / `Bytes` / 其他路径分支。
- `pkg/dumpformat/sqlfile/writer.rs` 的 `Writer::new` 同样接收 `Vec<FieldKind>`；`sql.rs::append_value` 对三个变体做穷尽匹配。

本文件只依赖 Rust 核心语言及编译器提供的派生 trait，不调用任何函数。RustCodeGraph 的文件节点报告 `kind.rs` 被多个文件使用，但其中夹有 Parquet 文件的同名符号关联；精确调用边查询没有返回函数边。上述关系因此以唯一 `FieldKind` 定义、crate 再导出、Cargo 依赖及 `rg` 得到的真实导入/匹配点交叉核验，没有把同名 `FieldKind` 当成目标类型的调用者。

## 错误处理与边界

本文件没有可失败操作，也不定义 `Result` 或错误类型。真正可观察的边界位于分类者和消费者：

- 上游把非数值列误标成 `Number` 时，其字节会不经引号和转义直接进入导出内容，可能生成无效或语义错误的 CSV/SQL；枚举无法自行验证输入。
- 上游把二进制列误标成 `String` 时，SQL 不会生成 `x'...'`，CSV 也不会采用配置的 HEX/Base64 二进制分支；任意字节的可移植性可能受影响。
- `None`/`is_null` 在两个编码器中都先于 `FieldKind` 处理，所以空值输出不受分类影响；空的非空字节值仍按分类输出。
- CSV 的 `Bytes + UTF8` 会落入通用转义路径，而不是做十六进制或 Base64 转换；这由 `BinaryFormat` 决定，不表示 `Bytes` 分类失效。
- 当前 Rust 下游使用穷尽匹配或明确分支。为枚举新增变体会引发相关穷尽匹配编译错误，但使用通配分支的代码仍需人工审查，不能只依赖编译器。
- 因为没有 `repr` 与稳定判别值承诺，不能把枚举的内存或 `as` 转换结果作为与 Go、磁盘或网络交互的格式。

相关行为边界由独立测试覆盖：`pkg/dumpformat/csvfile/csv_test.rs::null_and_kinds`、`bytes_hex`、`bytes_base64` 验证三类与空值；`pkg/dumpformat/sqlfile/writer_test.rs::framing_and_size`、`escaping_and_binary_bytes` 验证数值、字符串、字节及空值的 SQL 输出。目标文件没有同名测试文件，也没有把测试内嵌在生产源码中。

## 并发与资源生命周期

`FieldKind` 不创建或持有线程、任务、锁、通道、文件、网络连接、缓冲区或外部句柄。它是不可变、无引用的值类型；只要包含它的外层结构满足相应条件，值可在线程间移动或共享。派生 `Copy` 意味着逐字段传参不借用枚举，也没有析构顺序或资源释放要求。

实际生命周期由消费者管理：分类向量在 writer 构造时移入 `Writer`，在 writer 存活期间保持列顺序；单个分类在每次写行时按值复制给编码函数。输出缓冲、sink 写入、关闭和错误传播属于相邻 writer，不属于本文件。多个线程若各自持有 `FieldKind` 副本不会互相影响；共享同一个 writer 是否安全则由 writer 的 `&mut self` 和 sink 类型决定。

## 与 Go 版本的对应关系

直接对照文件是 [`kind.go`](./kind.go)。两边都表达相同的三种语义，顺序对应为 Rust `Number` / `String` / `Bytes` 与 Go `KindNumber` / `KindString` / `KindBytes`。`dumpling/export/writer_util.rs::columnKinds` 和 `writer_util.go::columnKinds` 也采用相同优先级：二进制优先、数值其次、其余归字符串；CSV/SQL 的 Rust 与 Go 编码器据此选择同类输出。

表示层存在有意差异：

- Go 定义为 `type FieldKind uint8` 并用 `iota` 赋值，因此常量当前为 0、1、2，零值自然是 `KindNumber`，且理论上可构造未声明的其他 `uint8` 值。
- Rust 定义为强类型枚举，没有 `#[repr(u8)]`、显式判别值或 `Default`，安全代码必须从三个合法变体中显式选择一个，布局也没有跨语言稳定性承诺。
- Go 的 CSV/SQL `switch` 用 `default` 承担字符串行为，所以未知数值也会按字符串处理；Rust SQL 使用穷尽 `match`，CSV 用“Number 提前返回、Bytes 特例、其余通用转义”的结构。对当前三个合法变体输出语义一致，但未知值行为不可类比。

因此 Go/Rust 的兼容契约是分类含义和最终导出字节，而不是 `FieldKind` 的二进制表示。若未来要求 FFI 或持久化，应另行设计显式、可验证的编码，而不是依赖当前声明顺序。

## 扩展指南

- 新增分类前，先确认现有三类是否已足够表达目标格式行为。若确需新增，修改 `FieldKind` 后必须同步审查 `dumpling/export/writer_util.rs::columnKinds`、CSV `append_field`、SQL `append_value`、两个 writer 的构造调用及 Go 对照；不能只让 Rust 编译通过而保持 Go 语义缺口。
- 分类规则变化应优先落在 `columnKinds` 及其独立测试，而不是让字段编码器重新解析 SQL 类型名。这样可维持“分类一次、多个格式消费”的边界。
- 若只增加某个格式的输出选项（例如新的二进制编码），通常应扩展该格式的配置和编码器，而不是增加共享 `FieldKind`，除非 CSV 与 SQL 都需要区分新的字段语义。
- 修改 `Number` 行为时要覆盖非法/特殊数值文本以及 SQL 与 CSV 输出；修改 `String` 时覆盖引号、换行、NUL 与反斜杠；修改 `Bytes` 时覆盖空值、空字节串、非 UTF-8 字节和 HEX/Base64。Rust 测试应继续放在 `csv_test.rs`、`writer_test.rs` 或新的独立测试文件中，不要写进 `kind.rs`。
- 若需要跨语言数值稳定性，应显式增加 `#[repr(u8)]`、固定判别值和受控转换 API，并为非法值写测试；这会形成新的兼容承诺，需评估已有序列化/FFI 消费者，而不能仅模仿 Go 的 `uint8`。
- 性能风险主要不在枚举本身，而在错误扩展导致每字段重复分类或额外分配。保持 `columnKinds` 每列一次分类、writer 内保存紧凑向量的现有结构，可避免在每行热路径解析类型名。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/dumpformat` 定位目标、模块与测试；`node --file pkg/dumpformat/kind.rs` 确认文件共 22 行、唯一业务符号为 `FieldKind`；`query FieldKind --json` 确认目标枚举及三个变体，并暴露仓库内多个同名类型。对消歧后的目标枚举执行 `callers`、`callees`、`impact` 未返回可用边，因此调用关系改由真实导入和匹配点核验。
- 目标与 crate 边界：`pkg/dumpformat/kind.rs`、`pkg/dumpformat/lib.rs`、`pkg/dumpformat/Cargo.toml`；消费者依赖：`pkg/dumpformat/csvfile/Cargo.toml`、`pkg/dumpformat/sqlfile/Cargo.toml`。
- Rust 调用与行为：`dumpling/export/writer_util.rs::columnKinds`、`pkg/dumpformat/csvfile/csvfile.rs`、`csvfile/writer.rs`、`csvfile/csv.rs`、`sqlfile/writer.rs`、`sqlfile/sql.rs`。
- Go 对照：`pkg/dumpformat/kind.go`、`pkg/dumpformat/csvfile/csv.go`、`pkg/dumpformat/sqlfile/sql.go`、`dumpling/export/writer_util.go::columnKinds`。
- 独立测试：`pkg/dumpformat/csvfile/csv_test.rs` 和 `pkg/dumpformat/sqlfile/writer_test.rs`；Go 对照测试为同目录的 `csv_test.go` 与 `writer_test.go`。这些测试提供既有行为证据，本次按纯文档计划未运行 Cargo。
- 人工复核重点：公共再导出路径、分类产生位置、三变体在 CSV/SQL 的实际分支、空值优先级、Rust/Go 表示差异、独立测试位置，以及本文件没有错误、并发或资源管理逻辑这一边界。
