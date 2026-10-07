# `pkg/expression/builtin_regexp.rs`

## 文件定位

本文件是 `astersql-expression` crate 内的正则函数内核。`pkg/expression/lib.rs` 通过 `#[path = "builtin_regexp.rs"] mod builtin_regexp_kernel;` 将其作为私有模块装配；测试配置下的 `expression_regexp` 再导出它供迁移测试使用。crate 边界由 `pkg/expression/Cargo.toml` 定义，直接使用其中的 `regex = "1"` 和 `thiserror = "2"`。

生产求值接线中，`pkg/expression/builtin.rs` 已确认会从 `CoreBuiltinKind::RegexpInstr`、`RegexpSubstr`、`RegexpReplace` 调用本文件的 `RegexpEngine`。`RegexpEngine::regexp_like` 及其缓存/向量辅助目前能在独立 Rust 测试中直接使用，但搜索未发现 `builtin.rs` 对它的生产调用，因此不能仅凭内核存在断言 `REGEXP_LIKE` 已经通过该引擎接入 SQL 求值主链。

## 核心职责

- 用 `RegexpBase` 统一解析 `match_type`、编译文本/字节正则，并在 pattern 与可选 match type 为常量时按 `context_id` 缓存编译结果或编译错误。
- 用 `RegexpEngine` 实现 `REGEXP_LIKE`、`REGEXP_SUBSTR`、`REGEXP_INSTR`、`REGEXP_REPLACE` 的标量、简单批量及 binary 辅助路径。
- 维护 MySQL/TiDB 的参数约定：位置从 1 开始、occurrence 的缺省/特殊值、INSTR 的返回起点或终点、NULL 由批量包装层传播。
- 将替换串解析为字面量和 `\0` 至 `\9` 捕获组指令，并分别渲染 UTF-8 文本与任意字节结果。
- 通过 `normalize_go_perl_classes` 缩小 Rust `regex` 与 Go regexp 在 Perl 字符类上的语义差异。

## 主要符号

- `PATTERN_IDX`、`REPLACEMENT_IDX` 及四个 `*_MATCH_TYPE_IDX`：记录 Go 签名中关键参数位置；当前 Rust 方法以具名参数接收，常量主要作为对照契约。
- `RegexpError`：覆盖非法 match type、位置越界、非法 return option、捕获组越界、binary 不支持、空模式、编译失败和缓存锁中毒。
- `CompiledRegexp { text, bytes }`：一次构建同时保存 `regex::Regex` 与 `regex::bytes::Regex`，供字符语义和字节语义复用。
- `RegexpBase`：持有常量性、默认大小写规则与 `Arc<Mutex<HashMap<u64, CachedRegexp>>>`；关键入口为 `build_regexp`、`get_regexp_with_argument`、`try_vec_memorized_regexp`。
- `get_regexp_match_type`：只接受 `c/i/m/s`；ci collation 默认加入 `i`，用户输入中最右侧 `c` 或 `i` 决定大小写，结果按 `i,m,s` 固定顺序输出。
- `RegexpEngine`：对外内核门面；`new`、`with_constants`、`new_binary` 分别构造动态文本、可缓存文本和 binary 模式。
- `RegexpSubstrArgs`、`RegexpInstrArgs`、`RegexpReplaceArgs`：批量方法的借用参数视图；外层 `Option` 表示整行 NULL。
- `Instruction` 与 `get_instructions`：把 replacement 拆成捕获组引用或原始字节字面量。
- `trim_*`、`replace_*`、`render_*`：内部位置换算、匹配遍历和替换渲染辅助函数。
- 四个 `regexp_*_vectorized()`：返回 `true` 的能力标志；它们本身不执行求值，也不证明注册表已经接线。

## 执行流程

1. 调用者先构造 `RegexpEngine`。普通 SQL 接线当前使用 `new(false)`；需要常量缓存的测试路径使用 `with_constants`，binary 辅助测试使用 `new_binary`。
2. `RegexpBase::build_regexp` 拒绝空 pattern，调用 `get_regexp_match_type` 生成内联 flag，再经 `normalize_go_perl_classes` 处理 `\d/\w/\s/\b` 及反类，最后同时编译文本和字节正则。
3. `regexp_like` 直接做 `is_match` 并映射为 `0/1`；`regexp_like_cached` 先按常量性决定直编译或取 `context_id` 缓存。
4. SUBSTR/INSTR/REPLACE 先按字符或字节检查 1-based position 并切出后缀。SUBSTR 取第 occurrence 个匹配；INSTR 根据 `return_option` 返回匹配起点或终点，未命中返回 0；REPLACE 保留 position 前缀，再替换指定 occurrence，0 表示全部。
5. REPLACE 先由 `get_instructions` 解析 replacement。`replace_text_matches`/`replace_byte_matches` 遍历 captures，仅复制尚未输出的区间，并通过 `render_*_replacement` 展开捕获组。
6. `*_vec` 方法逐行调用相应标量路径；行参数为 `None` 时输出 `None`，首个错误通过 `collect` 立即终止整个批次。

## 数据与状态

持久状态仅位于 `RegexpBase::memorized_regexp`。键是调用方提供的 `u64 context_id`，值是 `Result<Arc<CompiledRegexp>, RegexpError>`，所以成功对象和失败结果都会缓存。缓存没有主动逐项淘汰；其生命周期跟随共享同一 `Arc` 的 `RegexpBase` clone，`cache_len` 只是测试/诊断接口。

字符路径的 position 和 INSTR 返回值按 Unicode scalar value（Rust `char`）计数，切片时再换算 UTF-8 字节边界。binary 路径直接按字节计数，并将 SUBSTR/REPLACE 结果编码为带 `0x` 前缀的大写十六进制。occurrence 小于 1 时，SUBSTR/INSTR 归一为 1；REPLACE 对负数归一为 1，而 0 保留为“全部替换”。

## 依赖与调用关系

上游直接证据：

- `pkg/expression/lib.rs` 装配 `builtin_regexp_kernel`，并在测试配置下挂载 `builtin_regexp_test.rs`、`builtin_regexp_util_23_aster_unit_test.rs`、`builtin_regexp_vec_const_test.rs`。
- `pkg/expression/builtin.rs` 的 `CoreBuiltinKind::RegexpInstr`、`RegexpSubstr`、`RegexpReplace` 分支完成 SQL 参数求值、默认值和 NULL 传播，再调用对应 `RegexpEngine` 方法并把 `RegexpError` 转为 crate 的通用错误。
- `pkg/expression/builtin_regexp_vec_const_test.rs` 调用 `regexp_like_cached` 和 `try_vec_memorized_regexp`，验证常量 pattern 的 context 级复用。

下游依赖：

- `regex::Regex` 和 `regex::bytes::Regex` 负责编译、查找和 captures 遍历。
- `thiserror::Error` 提供稳定的错误显示文本。
- `crate::builtin_regexp_util_kernel::check_out_range_pos` 复用 Go 对照的空串/位置越界判定。
- 标准库 `Arc + Mutex + HashMap` 提供可共享的上下文缓存；`HashSet` 用于 match flags 去重。

RustCodeGraph 的文件节点显示本文件被 `builtin.rs`、两个正则测试文件、`chunk_executor.rs` 等多个文件使用；精确方法 caller 查询未返回符号级结果，因此本文只把源码搜索确认的直接调用列为已验证边。

## 错误处理与边界

- 空 pattern 返回 `EmptyPattern`；非法 flag 返回 `InvalidMatchType`；正则库错误包装为 `Compile(String)`。
- UTF-8 与 binary 的 position 都是 1-based。非法负数、0 或非空输入末尾之外的位置返回 `InvalidIndex`；空输入的 position 1 在相应路径中被允许。
- INSTR 只接受 `return_option` 0 或 1，否则返回 `InvalidReturnOption`；未找到匹配不是错误，而是 0。
- SUBSTR 未命中返回 `None`；REPLACE 未命中返回原输入。不存在的捕获组编号返回 `InvalidSubstitution`，存在但未参与本次匹配的组输出为空。
- `get_instructions` 将反斜杠加数字视为捕获组，反斜杠加其他字节视为该字节的字面量，末尾孤立反斜杠被忽略。
- 文本 SUBSTR/INSTR/REPLACE 在 `new_binary()` 下返回 `BinaryCollationUnsupported`；专用 binary 方法允许非 UTF-8 输入。
- 缓存互斥锁中毒映射为 `CachePoisoned`。`cache_len` 在锁中毒时返回 0，仅适合观测，不应作为错误检测手段。

## 并发与资源生命周期

`RegexpBase` 的 clone 共享同一个 `Arc<Mutex<...>>`，因此同一底座派生出的引擎状态可跨线程安全共享。缓存锁覆盖“查找—编译—插入”整个区间，避免同一底座并发重复初始化，但正则编译期间也会阻塞其他 context 的缓存访问；扩展时若缩小临界区，必须保留单次初始化和错误缓存语义。

`CompiledRegexp` 以 `Arc` 返回，离开互斥区后匹配不持锁。批量路径当前是同步顺序遍历，没有任务、通道、异步 I/O 或事务。临时 `String`/`Vec<u8>` 在一次调用内拥有资源，正则及缓存随最后一个 `Arc` 释放。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/expression/builtin_regexp.go`。`RegexpBase` 对应 `regexpBaseFuncSig`：二者都只在 pattern 和可选 match type 达到上下文常量级别时记忆化，并按 statement/context id 隔离；`builtin_regexp_test.go` 直接验证动态参数不入缓存、常量参数入缓存。

`get_regexp_match_type` 对应 Go `getRegexpMatchType`，保留 ci collation 默认忽略大小写、`c/i` 后出现者覆盖先出现者以及 `m/s` 独立生效。Rust 固定 flag 输出顺序，避免 Go set 迭代顺序不稳定，但语义等价。

Go regexp 的 Perl 字符类采用 ASCII 语义，Rust regex 默认 Unicode；Rust 额外用 `normalize_go_perl_classes` 显式改写类和词边界。`builtin_regexp_test.rs` 对阿拉伯数字、`é`、Unicode 空白、转义和词边界进行了针对性回归。

替换指令解析与 Go `getInstructions` 对齐，包括 `\0..\9`、普通转义和尾随反斜杠。差异是 Go 的完整签名层还缓存常量 replacement 指令并直接拥有 function class、collation 和 chunk 向量求值；本 Rust 文件只提供内核参数视图及逐行批量包装，不包含完整表达式对象构建。生产接线目前明确看到 INSTR/SUBSTR/REPLACE 使用 `new(false)`，因此 collation 派生、binary 选择和常量缓存尚不能从这些调用边证明已完整等价。

## 扩展指南

- 新增 flag 或调整 collation 行为时，先修改 `get_regexp_match_type`/`build_regexp`，同步 `pkg/expression/builtin_regexp_util_23_aster_unit_test.rs` 和 Go 对照用例，并评估文本、字节两种引擎是否接受相同语法。
- 修改位置或 occurrence 语义时，集中调整 `trim_utf8_*`、`trim_bytes_*` 及四个公开操作，必须覆盖空串、多字节字符、末尾位置、0/负数和未命中。
- 扩展 replacement 语法时，保持 `Instruction` 为解析与渲染的边界，同时修改文本/字节 renderer；测试放在独立的 `builtin_regexp_test.rs` 或 `builtin_regexp_util_23_aster_unit_test.rs`，不要嵌入生产文件。
- 将 LIKE 或更多向量路径接入生产时，应在 `builtin.rs`/注册层传入真实 collation、常量级别与 statement context id，不能只调用 `new(false)` 后宣称已利用缓存。
- 调整缓存并发策略时，保留按 context 隔离、成功与失败都缓存、clone 共享生命周期三项不变量，并增加并发回归测试。
- 任何行为变化都需与 `builtin_regexp.go` 和 `builtin_regexp_test.go` 的实际分支逐项核对；binary 返回格式、错误文本和 NULL 传播属于兼容风险，正则重复编译和全局锁竞争属于性能风险。

## 验证依据

- RustCodeGraph：`status` 确认项目索引包含 11,467 个文件；`node --file pkg/expression/builtin_regexp.rs --offset 1 --limit 500` 与 `--offset 480 --limit 420` 覆盖目标文件 1–836 行，并给出文件级使用者；`query RegexpEngine --kind struct` 定位结构体；`callers regexp_like_cached` 未返回方法级结果，故未据此推断调用边。
- 源与装配：`pkg/expression/builtin_regexp.rs`、`pkg/expression/lib.rs`、`pkg/expression/builtin.rs`、`pkg/expression/Cargo.toml`。目标目录未发现 `doc.go`，包契约以 crate 根 `lib.rs` 为最近入口。
- Go 对照：`pkg/expression/builtin_regexp.go`、`pkg/expression/builtin_regexp_test.go`、`pkg/expression/builtin_regexp_vec_const_test.go`。
- Rust 独立测试：`pkg/expression/builtin_regexp_test.rs`、`pkg/expression/builtin_regexp_util_23_aster_unit_test.rs`、`pkg/expression/builtin_regexp_vec_const_test.rs`，另有 `pkg/expression/builtin_like_test.rs` 直接覆盖基础 LIKE 引擎用例。
- 本任务为纯文档分析，按计划不运行 Cargo；交付检查只执行任务指定的 11 章节结构命令，并人工核对上述符号、调用边和已声明限制。
