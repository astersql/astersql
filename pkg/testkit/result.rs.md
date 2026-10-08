# `pkg/testkit/result.rs`

## 文件定位

`pkg/testkit/result.rs` 属于 `astersql-testkit` crate。`pkg/testkit/Cargo.toml` 以 `lib.rs` 为 crate 根，`pkg/testkit/lib.rs` 通过 `pub mod result` 暴露模块，并在 crate 根再导出 `Result`、`Rows`、`RowsWithSep`。本文件不参与数据库服务端的 SQL 执行主链；它处在测试查询链的末端，把已经物化、字符串化的查询结果包装成可重复读取和断言的值对象。

主要入口来自三条测试工具链：`pkg/testkit/testkit.rs::TestKit::MustQuery` 用 `Result::with_comment` 保存查询行和 TestKit 注释；`pkg/testkit/asynctestkit.rs::AsyncTestKit::new` 的 worker 查询分支用 `Result::new` 回传行集；`pkg/testkit/stepped.rs::SteppedTestKit::SteppedMustQuery` 用 `Result::new` 保存分步查询的最后结果。RustCodeGraph 对目标文件的索引还显示它被 67 个文件引用，说明其公开断言接口是跨子系统 Rust 测试共用的基础设施，而不是仅供 `pkg/testkit` 自测。

## 核心职责

- `Result` 保存 `Vec<Vec<String>>` 行集和断言失败时附带的 `comment`，提供全量比较、列投影比较、排序、包含性检查及只读克隆访问。
- `render_rows` 将每行所有单元格以一个空格连接，并在每行末尾追加换行；`Check`、`Equal` 和 `CheckAt` 都比较这一规范化文本，而不是直接比较二维向量。这刻意复现 Go `fmt.Fprintf(buffer, "%s\n", row)` 的语义：行边界有意义，单元格边界只要渲染文本相同即可忽略。
- `Rows` 与 `RowsWithSep` 把紧凑的字符串期望值转换成二维字符串矩阵，便于测试书写；`Rows` 固定使用单个空格作为分隔符。
- 断言失败统一通过 Rust `assert!`/`assert_eq!` panic，并尽量携带 `comment`、实际结果或失败子串，符合 TestKit 的“必须满足”测试接口定位。

本文件只处理已经字符串化的数据，不执行 SQL、不转换数据库值、不管理结果集游标，也不持有 session、事务或存储资源。上游的 `QueryRows::string_rows` 才负责把数据库值变为 `String`。

## 主要符号

- `pub struct Result { rows: Vec<Vec<String>>, comment: String }`：模块唯一类型。字段保持私有，调用者只能通过构造器和方法观察或改变状态；派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`。
- `Result::new(rows)`：保存行集并把注释初始化为空字符串，供异步和分步 TestKit 使用。
- `Result::with_comment(rows, comment)`：同时保存行集与任意 `Into<String>` 注释；`TestKit::MustQuery` 传入当前 comments 的换行拼接文本。
- `Check<T: Display>(expected)` 与 `Equal<T: Display>(expected)`：分别执行 panic 式全量断言和无副作用布尔比较。泛型期望单元格只要求 `Display`，实际行始终是字符串。
- `AddComment(&mut self, comment)`：无条件先追加一个换行，再追加文本，包括第一条注释；这是对 Go 行为的显式兼容。
- `CheckWithFunc<T, F>(expected, compare)`：先要求期望与实际行数一致，再逐行调用 `Fn(&[String], &[T]) -> bool`。比较器失败时只输出实际行，不输出期望行。
- `Sort(&mut self) -> &mut Self`：原地使用 `Vec<String>` 的字典序排列行，并返回自身以支持链式调用。
- `Rows(&self)`：深克隆内部二维字符串矩阵；调用者修改返回值不会影响 `Result`。
- `CheckAt(columns, expected)`：先验证每个期望行的宽度等于选列数，再按给定下标和顺序投影每个实际行，最后用 `render_rows` 比较。
- `CheckContain`/`CheckNotContain`：在单个单元格内搜索子串；跨单元格形成的拼接子串不算命中。
- `MultiCheckContain`/`MultiCheckNotContain`：先调用 `String` 得到整张表的展示文本，再逐个搜索，因此可以命中跨相邻单元格空格拼接或跨行换行形成的片段。这与单值版本的搜索域不同。
- `String()`：每行以空格连接、各行以换行连接；末尾不附加换行，与 `render_rows` 的比较格式不同。
- `len()`/`is_empty()`：分别报告行数和是否无行，不关心列数。
- 私有 `render_rows<T: Display>`：`Check`、`Equal`、`CheckAt` 的共同规范化核心；每行即使为空也会贡献一个换行。
- `Rows(rows)`/`RowsWithSep(separator, rows)`：公开自由函数。非空分隔符使用 `str::split`，会保留由首尾或连续分隔符产生的空字段；空分隔符走 Unicode `char` 拆分，并把空输入变为空行向量。

文件中没有 trait、枚举、模块级常量、静态变量或条件编译项。Go 风格的大写方法名由 crate 根 `#![allow(non_snake_case)]` 接受。

## 执行流程

典型同步查询断言链如下：

1. `TestKit::MustQuery` 调用 `TestKit::Query`，后者委托 `Database::query` 获得 `QueryRows`。
2. `QueryRows::string_rows` 将结果物化为 `Vec<Vec<String>>`；`Result::with_comment` 同时保存 `TestKit` 已登记的注释。
3. 测试通常调用 `Rows(&[...])` 构造期望值。每个输入字符串按单个空格切分，连续空格会产生空字符串单元格。
4. `Result::Check` 分别将期望矩阵和实际矩阵交给 `render_rows`。每个单元格先经 `Display::to_string`，同一行以空格连接，末尾补换行。
5. 两段渲染文本不等时，`assert_eq!` panic，并附上 `comment`；相等则无返回值地结束。若调用 `Equal`，相同步骤只返回布尔值。

列投影链中，`CheckAt` 首先检查所有期望行的宽度，随后对每个实际行按 `columns` 的顺序逐一下标访问并克隆值。重复列下标会重复投影；空列列表会把每个实际行投影为空行。完成投影后仍走 `render_rows`，所以不同单元格边界只要行文本相同也可判等。

包含性链分成两类：`CheckContain`/`CheckNotContain` 直接遍历 `rows.iter().flatten()`，保持单元格边界；`MultiCheckContain`/`MultiCheckNotContain` 则只生成一次 `String()` 并在整段展示文本中检查每个片段。失败路径都会 panic，且包含单值版本会把完整 `String()` 加入诊断。

## 数据与状态

`Result` 完全拥有行和注释；没有借用字段。`new`/`with_comment` 接管输入 `Vec`，正常构造不再复制行数据；`Rows()` 才执行完整深克隆。`Sort` 是唯一直接改变 `rows` 的公开方法，`AddComment` 是唯一改变 `comment` 的方法。其余方法只读。

二维矩阵允许零行、空行和不等宽行。本文件没有全局列数不变量：`Check`、`Equal`、`String` 和包含性检查均接受不等宽数据；`CheckAt` 只要求每个期望行宽等于选列数，并要求每个实际行都存在每个被选下标。期望行数最终由渲染文本比较间接约束，`CheckWithFunc` 则在调用比较器前显式约束行数。

文本规范化存在两个有意不同的表示：`render_rows` 在最后一行之后也有 `\n`，用于精确复刻 Go 行比较；`String` 只在行之间放 `\n`，用于人类可读诊断与多片段搜索。扩展代码不能把二者随意合并，否则会改变空行、尾换行和包含检查语义。

`RowsWithSep("", ...)` 采用 Rust Unicode scalar value（`char`）拆分。例如独立测试验证 `"Aé"` 得到 `"A"`、`"é"`；这不是按 UTF-8 字节拆分，也不是按用户感知的 grapheme cluster 拆分。

## 依赖与调用关系

- crate 接线：`pkg/testkit/Cargo.toml` 定义 `astersql-testkit` 且 `[lib] path = "lib.rs"`；没有针对本模块的 feature gate。`pkg/testkit/lib.rs` 声明 `pub mod result` 并 `pub use result::{Result, Rows, RowsWithSep}`。
- 同步上游：`TestKit::MustQuery → TestKit::Query → Database::query → QueryRows::string_rows → Result::with_comment`。`MustPointGet`、`MustUseIndex`、`HasPlan` 等 `testkit.rs` 辅助方法随后通过 `Rows()` 检查查询或 EXPLAIN 结果。
- 异步上游：`AsyncTestKit` worker 的 `Command::Query → TestKit::Query → QueryRows::string_rows → Result::new`，再通过专属回复通道把拥有所有权的 `Result` 发回调用线程。
- 分步上游：`SteppedTestKit::SteppedMustQuery → TestKit::Query → QueryRows::string_rows → Result::new`，结果进入 `Arc<Mutex<Option<Result>>>` 管理的分步状态；锁属于 `stepped.rs`，不在本文件内。
- 下游：本文件运行时只依赖标准库 `std::fmt::Display` 和 `Vec`/`String` 的排序、迭代、连接、切分、字符遍历能力；不直接使用 `Cargo.toml` 中的数据库相关依赖。
- 自身调用边：`Rows → RowsWithSep`；`Check/Equal/CheckAt → render_rows`；`CheckContain/CheckNotContain/MultiCheckContain/MultiCheckNotContain → String`（前两者只在失败诊断中调用）。
- 测试挂载：`pkg/testkit/lib.rs` 在 `#[cfg(test)]` 下以 `#[path = "result_test.rs"] mod result_test;` 引入独立测试，符合生产逻辑与测试逻辑分文件要求。

## 错误处理与边界

本文件没有返回 `Result<T, E>` 的可恢复错误路径；所有检查失败都被视为测试失败并 panic。

- `Check` 与 `CheckAt` 的最终不等失败由 `assert_eq!` 报告左右渲染文本及 comment；`Equal` 永不 panic，除非某个自定义 `Display` 实现自身 panic。
- `CheckWithFunc` 在行数不一致时先 panic，保证 `zip` 不会静默忽略多余行；比较闭包返回 `false` 时 panic。闭包本身的 panic 原样传播。
- `CheckAt` 在任何实际行缺少所选列时以 `column {column} out of range` panic。它先检查所有期望行宽，因此“期望宽度错误”会早于实际投影越界暴露。列下标不要求排序或唯一。
- `CheckContain`/`CheckNotContain` 对空结果分别表现为“找不到期望值”和“未找到禁用值”；多值版本对空待检查切片是 vacuous success，不执行断言。
- `RowsWithSep` 的非空分隔符完全沿用 `str::split`。调用者若希望按任意空白折叠切分，不能用当前 `Rows`，因为多个空格会保留空字段。
- `Sort` 比较完整的字符串行，遵循 Rust 字符串/向量的字典序，不做数值、locale、SQL collation 或稳定结果顺序推断。
- `String`/`render_rows` 用普通空格和换行作为无转义展示格式；单元格自身含空格或换行时表示不可逆。因此本文件适合断言和诊断，不是序列化协议。

## 并发与资源生命周期

`Result` 不含锁、通道、任务、事务、文件或数据库游标，也没有自定义 `Drop`。它的生命周期仅由所拥有的 `Vec<Vec<String>>` 与 `String` 决定，离开作用域即由 Rust 自动释放。

类型的字段组成允许其在元素类型约束下自动获得 `Send`/`Sync`；本文件没有显式 unsafe 或线程同步。`AsyncTestKit` 可以在线程间发送一个已经完全物化的 `Result`，但之后对同一个值的可变操作仍受 Rust 借用规则约束。`Sort` 和 `AddComment` 需要 `&mut self`，只读断言方法需要 `&self`，本文件没有内部可变性或并发写入机制。

资源边界的重要事实是：上游在构造 `Result` 前已经调用 `QueryRows::string_rows` 完成物化。因此保留或克隆 `Result` 不会延长 session、RecordSet、事务或网络响应的生命周期；相应资源的关闭和错误处理必须在上游数据库适配层完成。

## 与 Go 版本的对应关系

`pkg/testkit/result.go` 是直接语义基准。Rust 保留了 `Result`、`Check`、`Equal`、`AddComment`、`CheckWithFunc`、`Sort`、`Rows`、`RowsWithSep`、`CheckAt`、包含性方法和 `String` 等 Go 风格名称，并让 `render_rows` 明确复刻 Go 对每行执行 `fmt.Fprintf(..., "%s\n", row)` 的比较结果。

已对齐的关键行为包括：

- `AddComment` 对第一条和后续注释都先加换行。
- `Check`/`Equal` 按渲染行文本比较，忽略产生同一文本的单元格边界；`result_test.rs` 用 EXPLAIN 风格行直接覆盖该行为。
- `CheckWithFunc` 在逐行比较前要求行数一致。
- `Sort` 原地按整行字典序排序并返回自身。
- `CheckAt` 保留选列顺序并做行文本比较。
- 单值包含检查按单元格搜索，多值包含检查按整张表的 `String` 搜索。

当前实现差异和迁移边界包括：

- Go `Result` 还持有 testify 的 `require`/`assert` 对象；Rust 直接使用标准 `assert!`/`assert_eq!`，因此失败消息、调用栈和断言归属格式不会逐字一致。
- Go `Rows`/`RowsWithSep` 返回 `[][]any` 且使用 `strings.Split`；Rust 返回 `Vec<Vec<String>>`。对空分隔符，Go 按 Unicode code point 拆分且空字符串得到空切片，Rust 用 `chars()` 显式对齐，独立测试覆盖了非 ASCII 与空字符串。
- Go `Rows()` 返回 `[][]any` 的重新装箱副本；Rust 返回 `Vec<Vec<String>>` 的深克隆。
- Rust 额外提供 `new`、`with_comment`、`len`、`is_empty` 以适配所有权和常用集合接口；这些不是 Go `result.go` 的同名 API。
- Rust `CheckAt` 对期望宽度与实际下标越界给出显式 panic 文案；Go 依赖 testify 断言及 slice 下标 panic，诊断文字不同但约束意图一致。

## 扩展指南

- 修改相等语义时应集中评估 `render_rows`，并同步覆盖 `Check`、`Equal`、`CheckAt`。不能直接改成二维向量相等，因为现有测试明确要求跨单元格边界但渲染文本一致时通过。
- 新增结果转换方法时应先判断其应返回克隆还是借用。当前 `Rows()` 的深克隆是公开兼容行为；若为性能增加借用视图，建议另起清晰名称并避免破坏调用者可独立修改返回值的假设。
- 扩展包含检查时要明确搜索域是“单元格”还是 `String()` 的“整表展示文本”，并保持方法命名和诊断一致；当前单值与多值版本的边界不同，不能无说明地互相委托。
- 调整 `RowsWithSep` 时必须覆盖空分隔符、连续/首尾分隔符、空输入字符串、非 ASCII 字符；若需要 grapheme cluster，应新增明确 API，而不是悄然改变 `char` 语义。
- 新增排序模式应另增方法或显式比较器，避免改变 `Sort` 现有字符串字典序；数值排序、NULL 次序和 SQL collation 都需要独立设计。
- 所有 Rust 测试继续放在同目录独立文件 `pkg/testkit/result_test.rs`，并由 `lib.rs` 的 `#[cfg(test)]` 模块接线；不要把测试内嵌进 `result.rs`。涉及 Go 对齐的修改还应同步阅读 `pkg/testkit/result.go`，必要时补充能锁定差异意图的测试。
- 若新增断言失败上下文，注意 `with_comment` 接收的初始注释当前不自动补换行，而 `AddComment` 每次都补；不要让两种构造路径产生意外双换行或丢失 SQL 上下文。

## 验证依据

- RustCodeGraph `status`：索引目标仓库，包含 11,467 个文件、307,296 个节点和 1,848,419 条边，其中 Rust 文件 7,032 个。
- RustCodeGraph `files --filter pkg/testkit`：确认目标、crate 根、Go 对照、独立测试以及同步/异步/分步 TestKit 文件均在索引中。
- RustCodeGraph `node --file pkg/testkit/result.rs --offset 1 --limit 260`：读取目标文件 242 行全貌，核对 `Result`、全部方法、私有 `render_rows`、`Rows`、`RowsWithSep`；索引报告该文件被 67 个文件使用。
- RustCodeGraph `query`：定位 `result.rs::with_comment`、`AddComment`、`CheckWithFunc`、`CheckAt`、包含性方法、`render_rows` 与 `RowsWithSep` 等主要符号；对 `RowsWithSep` 同时定位到 Go `pkg/testkit/result.go::RowsWithSep`。
- RustCodeGraph `node` 读取 `pkg/testkit/testkit.rs`、`asynctestkit.rs`、`stepped.rs`：核对三条直接构造链 `MustQuery → Result::with_comment`、异步 Query worker `→ Result::new`、`SteppedMustQuery → Result::new`，以及 `Rows()` 在计划检查中的调用。
- 读取 `pkg/testkit/Cargo.toml` 和 RustCodeGraph `node` 对 `pkg/testkit/lib.rs` 的结果：核对 crate 名、根路径、公开再导出、无模块 feature gate，以及独立 `result_test.rs` 的 `#[cfg(test)]` 挂载。
- RustCodeGraph `node --file pkg/testkit/result.go` 与 `testkit.go`：核对 Go 的行渲染比较、注释追加、排序、投影、包含检查、`MustQuery` 和 `ResultSetToResultWithCtx` 语义。
- RustCodeGraph `node --file pkg/testkit/result_test.rs`：核对六组直接边界证据，包括注释换行、`CheckAt` 宽度前置检查、跨单元格边界的渲染等价、多值整表包含检查及空分隔符 Unicode 行为。
- 使用 `rg` 补充图命令未完整枚举的构造调用点：确认 `Result::with_comment` 位于 `testkit.rs`，`Result::new` 位于 `asynctestkit.rs`、`stepped.rs` 和独立测试；未把同名的其他 crate `Result` 误认作调用者。
- 本任务是纯文档分析，未运行 Cargo；结构校验按任务文件给定命令执行。
