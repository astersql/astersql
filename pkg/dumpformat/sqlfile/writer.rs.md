# `pkg/dumpformat/sqlfile/writer.rs`

## 文件定位

[`writer.rs`](writer.rs) 属于 `astersql-dumpformat-sqlfile` crate。该 crate 的入口 [`lib.rs`](lib.rs) 将本文件的 `Config`、`Writer` 与 [`sql.rs`](sql.rs) 的 `append_value` 一并公开；[`Cargo.toml`](Cargo.toml) 表明它只直接依赖上层 `astersql-dumpformat` crate，后者提供共享列分类 `FieldKind`。

在应用链路中，本文件是 Dumpling SQL 数据导出的“行到 INSERT 文本”编码器。生产调用点 [`dumpling/export/writer_util.rs`](../../../dumpling/export/writer_util.rs) 的 `writeSQLFile`（Go 对应入口名为 `WriteInsert`）生成 INSERT 前缀和列类型，构造 `SQLWriter`，逐行调用 `write`，再用 `estimate_file_size` 驱动进度统计与文件大小切分，最终显式调用 `close`。它不负责查询数据库、选择输出文件、缓冲、文件轮转或特殊注释；这些职责均由调用者承担。

## 核心职责

- `Writer<W>` 把一行 `&[Option<Vec<u8>>]` 编码为 SQL 元组，按 `FieldKind` 委托 `append_value` 生成数值、字符串、二进制或 `NULL` 字面量。
- 它在多行之间生成 `,\n`，在语句之间生成 `;\n` 和新的 `prefix`，形成 `INSERT ... VALUES\n(...),\n(...);\n`。
- `Config::statement_size` 非零时，下一次 `write` 会在“当前语句的逻辑大小已经达到阈值”后先结束旧语句，再开始新语句。它不是在写当前行前预测该行是否越界。
- 它维护当前 INSERT 语句及整个 SQL writer 的逻辑字节数，供调用者统计进度和决定文件轮转。
- 它把底层 `std::io::Write` 错误原样向上传播，但不拥有底层 writer 的 flush、关闭或轮转生命周期。

## 主要符号

- `Config { statement_size: u64, escape_backslash: bool }`：公开且 `Clone + Copy + Default` 的行为配置。默认值均为零值，即不按语句大小拆分、字符串使用单引号加倍而非反斜杠转义。
- `Writer<W>`：泛型公开编码器，要求方法实现中的 `W: std::io::Write`。它持有底层 `writer`、配置 `cfg`、逐列 `kinds`、INSERT `prefix`、可复用临时 `buf`，以及三个状态字段 `statement_size`、`file_size`、`in_statement`。
- `Writer::new(writer, prefix, kinds, cfg) -> Self`：取得 writer、前缀和列分类的所有权，初始化空缓冲、零计数和“没有打开语句”的状态；构造时不产生 I/O。
- `Writer::write(&mut self, row) -> io::Result<()>`：校验列宽、决定是否切分语句、编码一行并写向底层 writer，是本文件的核心状态转换入口。
- `Writer::estimate_file_size(&self) -> u64`：返回逻辑文件大小计数，不读取实际 sink 位置，也不包含调用者绕过本 writer 直接写出的前导特殊注释。
- `Writer::close(&mut self) -> io::Result<()>`：若有打开的 INSERT，写出 `;\n`；否则为空操作。它不会调用底层 `flush`，也没有 `Drop` 自动补终止符。

本文件没有模块级常量、trait、枚举、条件编译项或私有辅助函数。SQL 字面值编码规则位于相邻的 `append_value`，不在此处重复实现。

## 执行流程

1. `new` 保存 `W`、前缀、列分类和配置；此时 `in_statement == false`，两个尺寸计数均为零。
2. `write` 首先要求 `row.len() == kinds.len()`。不匹配时立即返回 `io::Error::other`，不清空缓冲、不改变计数，也不触碰 sink。
3. 清空并复用 `buf`。若已有语句、阈值非零且 `statement_size >= cfg.statement_size`，先把 `;\n` 放入本次缓冲，并把 `in_statement` 置为 `false`。
4. 若当前没有语句，将 `prefix` 追加到缓冲，把 `statement_size` 重置为前缀长度，把前缀长度加入 `file_size`，并标记语句已打开；否则追加行分隔符 `,\n`。
5. 记录元组起点，写入 `(`；遍历字段，在字段间加入逗号，并调用 `append_value(buf, bytes, is_null, kinds[i], escape_backslash)`；最后写入 `)`。
6. 以“本行元组字节数 + 2”计算 `size`。这两个预记字节代表该行之后将出现的 `,\n` 或最终 `;\n`，随后同时累加到语句和文件计数。
7. 调用底层 `writer.write(&buf)`。返回错误直接交给调用者；成功时忽略实际写入字节数并返回 `Ok(())`。
8. `close` 在语句已打开时先清除 `in_statement`，再写 `;\n`。终止符早已由最后一行的 `+ 2` 计入尺寸，因此这里不再增加计数。重复 `close` 是空操作。

由 [`writer_test.rs`](writer_test.rs) 的 `statement_split_counts_pending_separator` 和 `limit_checks_previous_statement_size_not_next_row_projection` 可见，阈值判断基于上一行写完后的逻辑计数：一行可以把语句推过阈值，实际切分发生在下一行开始之前。

## 数据与状态

`prefix` 和 `kinds` 在构造后只读。`kinds[i]` 必须与 `row[i]` 的 SQL 类型对应；`None` 表示 SQL `NULL`，而 `Some(Vec::new())` 是非 NULL 空值，两者由 `append_value` 区分。空 `kinds` 配合空行是合法输入，会输出空元组 `()`，用于所有列均为生成列的导出场景。

`buf` 在每次 `write` 开头 `clear`，保留容量以减少逐行分配；它只承载本次底层 `write` 需要新增的字节，不保存完整文件。`statement_size` 在新语句开始时重置为前缀长度，随后为每行累加“元组 + 待写的两字节分隔符”；`file_size` 不重置，按同一口径累计所有前缀和行。因此成功 `close` 后，`file_size` 与本 writer 生成的实际总字节数一致；写入最终分隔符之前，它是包含待写 `;\n` 的逻辑估算值。

`in_statement` 是状态机标志：`new`/成功或失败后的某些 `close` 路径为 `false`，写行准备阶段为 `true`。本实现先更新计数和状态，再调用 sink，所以 I/O 失败不会回滚这些内存状态。`estimate_file_size` 因而不是底层已确认持久化字节数，而是编码器已接受并记账的逻辑大小。

## 依赖与调用关系

上游生产调用边为 `dumpling/export/writer_util.rs::writeSQLFile -> Writer::new/write/estimate_file_size/close`。该函数从 `TableMeta` 生成前缀，以 `columnKinds` 把数据库列类型映射为 `astersql_dumpformat::FieldKind`，把 `StatementSize` 和 `EscapeBackslash` 传入本文件；它还单独记录特殊注释的 `preamble`，因为这些字节直接写入 sink，不计入 `Writer::file_size`。crate 依赖由 [`dumpling/export/Cargo.toml`](../../../dumpling/export/Cargo.toml) 中的 `astersql-dumpformat-sqlfile` 路径依赖接入。

本文件的关键下游调用边是 `Writer::write -> crate::append_value` 和 `Writer::write/close -> W::write`。`append_value` 的实际定义在 [`sql.rs`](sql.rs)：数值原样输出，字节编码为小写十六进制 `x'...'`，字符串按配置使用 MySQL 风格反斜杠转义或 SQL 单引号加倍，空值输出 `NULL`。

RustCodeGraph 的文件索引将 `writer.rs` 标为含 10 个符号，并显示直接文件引用来自 `writer_test.rs`；索引的泛型 impl 方法精确查询没有完整解析。因此生产调用边另由仓库文本引用和 `dumpling/export/writer_util.rs` 源码确认，不能把图工具的直接引用结果解释为“仅测试使用”。

## 错误处理与边界

- 列数不匹配返回固定格式 `sqlfile: row has {actual} fields, want {expected}` 的 `io::Error`；[`writer_test.rs`](writer_test.rs) 的 `width_mismatch_has_no_side_effects` 锁定了错误文本和零副作用。
- `append_value` 不返回错误；字段编码完成后，唯一运行时错误来源是底层 `W::write`。`write_errors_propagate_on_row_and_close` 证明行写入与终止符写入错误均会向上传播。
- 状态在 I/O 前更新。行写失败后，尺寸和 `in_statement` 已前进；`close` 写失败前也已将 `in_statement` 清除，因此再次 `close` 不会重试终止符。调用者应把写失败视为该 writer 不再可安全续写，并由外层重试整个输出单元。
- 代码调用的是 `Write::write` 而不是 `write_all`，并丢弃 `Ok(n)` 中的 `n`。若自定义 sink 合法地短写且不报错，编码器会误认为整段成功，实际输出和逻辑尺寸可能不一致。当前 Dumpling sink 的契约需要保证一次调用完整消费，或未来在本符号处改用 `write_all` 并补短写回归测试。
- `statement_size == 0` 表示一个文件内不按语句大小切分。非零阈值是软边界：首行总会写入，超限只在下一次 `write` 时触发。
- 未调用 `close` 会留下没有分号和换行的最后一条 INSERT；`Drop` 不会补写。空 writer 上 `close` 不输出任何内容。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务。所有变更方法都要求 `&mut self`，同一实例的行顺序和状态转换是串行的；能否跨线程移动或共享取决于泛型 `W` 的自动 trait，但本类型没有内部同步，调用者不能并发调用同一实例。

`Writer` 拥有 `W`，但没有取回内部 writer 的 `into_inner` 接口，也不负责 flush。典型调用生命周期是：外层创建/包装 sink → `Writer::new` → 多次 `write` → 读取逻辑尺寸 → `close` → `Writer` 离开作用域并释放 sink。文件轮转发生在外层：`writeSQLFile` 根据 `preamble + estimate_file_size()` 停止当前行迭代，后续由更高层创建新输出对象。

临时内存主要是持久复用的 `buf`、拥有的 `prefix`/`kinds` 和每行输入 `Vec<u8>`；编码二进制字段时，`append_value` 会将每个输入字节扩为两个十六进制字符。writer 本身不缓存历史行，因此除缓冲容量外，内存不会随已写行数线性增长。

## 与 Go 版本的对应关系

同路径 [`writer.go`](writer.go) 是直接语义基准：Go `Config`/`Writer`/`NewWriter`/`Write`/`EstimateFileSize`/`Close` 分别对应 Rust 的同名概念（构造器在 Rust 中为 `Writer::new`）。字段、分支顺序、列宽错误文本、尺寸预记、阈值判断和幂等关闭逻辑逐项一致。

输入表示的语言差异是 Go 使用 `[]sql.RawBytes`，其中 `nil` 表示 NULL；Rust 使用 `&[Option<Vec<u8>>]`，其中 `None` 表示 NULL。Go 保存 `*Config`，Rust 按值保存可复制的 `Config`，因此 Rust writer 构造后的行为不会随外部配置变量变化。Go 的 `w.Write` 与 Rust 的 `Write::write` 都忽略返回的写入长度，只传播错误，短写风险也一致。

[`writer_test.go`](writer_test.go) 覆盖基本 framing、两种字符串转义、语句切分和空元组；Rust 独立测试 [`writer_test.rs`](writer_test.rs) 保留这些意图，并额外锁定二进制字节编码、阈值 6/7 的待分隔符计数、重复关闭、宽度错误无副作用、sink 错误传播及“不预测下一行”的边界。Rust 生产接线 [`dumpling/export/writer_util.rs`](../../../dumpling/export/writer_util.rs) 与 Go [`dumpling/export/writer_util.go`](../../../dumpling/export/writer_util.go) 都在特殊注释之后创建 writer、逐行导出、按估算尺寸切文件并显式关闭。

## 扩展指南

- 新增 SQL framing 选项时，在 `Config`、`Writer::write`/`close` 和 Go `Config`/`Writer` 同步修改；至少扩展独立的 [`writer_test.rs`](writer_test.rs)，不要把测试内嵌进生产源文件。
- 新增字段类别或字面值规则时，优先修改共享 [`kind.rs`](../kind.rs) 与 [`sql.rs`](sql.rs) 的 `append_value`，同时检查 Dumpling 的 `columnKinds` 映射；不要在 `Writer::write` 复制一套转义逻辑。
- 改变语句或文件尺寸语义时，必须同时审查 `statement_size`、`file_size`、切分判断、最终分隔符记账，以及 `dumpling/export/writer_util.rs` 的指标/文件轮转计算。兼容风险包括输出分块位置和监控字节数变化，性能风险包括额外分配或重复扫描行。
- 改善 I/O 完整性最集中的入口是两处 `W::write`。若改为 `write_all`，应新增会返回短写 `Ok(n)` 的自定义 sink 测试，并验证状态及错误后的重试契约；同时保持 Go 版本对齐或明确记录差异。
- 若增加自动关闭、flush 或取回 sink 的能力，需先决定失败可见性和所有权语义。不能依赖 `Drop` 报告 I/O 错误；显式 `close` 仍应是可验证的主路径。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标目录 7 个已索引文件；`files --filter pkg/dumpformat/sqlfile` 找到 `writer.rs`、`writer_test.rs`、`writer.go`、`writer_test.go`、`sql.rs` 和模块入口；`node --file pkg/dumpformat/sqlfile/writer.rs --offset 1 --limit 500` 返回目标文件全部 104 行、10 个符号及对 `writer_test.rs` 的直接引用信息。`explore` 和泛型方法精确查询未给出可用调用边，该限制已在“依赖与调用关系”中说明。
- Rust 源与模块证据：[`writer.rs`](writer.rs)、[`sql.rs`](sql.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、共享 [`kind.rs`](../kind.rs)。
- Rust 调用与测试证据：[`dumpling/export/writer_util.rs`](../../../dumpling/export/writer_util.rs)、[`dumpling/export/Cargo.toml`](../../../dumpling/export/Cargo.toml)、[`dumpling/export/parity_test.rs`](../../../dumpling/export/parity_test.rs)、[`writer_test.rs`](writer_test.rs)。
- Go 对照证据：[`writer.go`](writer.go)、[`writer_test.go`](writer_test.go)、[`dumpling/export/writer_util.go`](../../../dumpling/export/writer_util.go)。
- 人工复核结论：本文件存在于数据库行与 Dumpling 对象 sink 之间，负责稳定生成多行 INSERT framing、尺寸估算和语句终止；安全扩展必须保持字段分类/转义、尺寸预记、显式关闭和外层轮转契约一致。
