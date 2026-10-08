# `pkg/util/table-router/go_regex.rs`

## 文件定位

`go_regex.rs` 是 `astersql-util-table-router` crate 内部的 Go 正则兼容层。它不是独立公开模块：`pkg/util/table-router/router.rs` 通过 `#[path = "go_regex.rs"] mod go_regex;` 私有挂载，唯一对外接线是 `compile_extractor` 调用本文件的 `pub(super) fn compile`。因此，应用侧仍然经由 `TableRule::Valid`、`NewTableRouter`、`AddRule` 或 `UpdateRule` 提交提取器正则，不直接调用本文件。

该文件存在的原因是 Go 实现 `pkg/util/table-router/router.go` 用标准库 `regexp.Compile`（RE2 语法与语义），而 Rust 路由器最终使用 `regex::Regex`。两套引擎都以线性时间匹配为目标，但可接受语法、字符类、标志、重复次数、Unicode 属性名和若干匹配细节并不完全相同；若直接把 Go 配置交给 Rust `regex`，同一条迁移/同步路由规则可能被错误接受、错误拒绝或匹配出不同内容。本文件先把 Go 方言翻译为受控的 Rust 方言，再编译为 `Regex`。

crate 边界由 `pkg/util/table-router/Cargo.toml` 定义：库入口为 `lib.rs`，本文件直接依赖 `regex` 与 `regex-syntax = "0.8"`；`Cargo.lock` 当前解析为 `regex 1.13.1`、`regex-syntax 0.8.11`。本文件没有 feature gate 或条件编译项，也没有独立公开 API。

## 核心职责

1. `compile(pattern)` 驱动完整流水线：用 `Parser::translate` 将 Go 正则改写成 Rust `regex` 能安全表达的等价形式，用 `regex_syntax` 再解析译文，用 `check_repeats` 补上 Go 的嵌套重复约束，最后构造可供路由提取器保存和匹配的 `Regex`。
2. `Parser` 显式限制 Rust 比 Go 多出的语法。比如只接受 Go 支持的 `i/m/s/U` 标志，拒绝 `x/u/R`；把 `\d/\w/\s` 固定为 Go 的 ASCII 类，避免 Rust 默认 Unicode Perl 类扩大匹配集合；字符类由本文件自行解析，从而避免 Rust 的集合运算改变 Go 的 `[...]` 含义。
3. 翻译 Go 特有或行为不同的构造，包括 `\Q...\E` 字面量、最多三位八进制转义、Go 形式的十六进制/Unicode 转义、重复命名捕获、POSIX 字符类、Unicode 属性别名、全局/分组标志以及 `U` 非贪婪模式。
4. 在引擎编译前拒绝 Go 会拒绝的输入，包括不平衡分组、非法标志、重复量词、非法范围、未知 POSIX/Unicode 类、越界码点，以及单层或嵌套乘积超过 1000 的有界重复。
5. 将本层可识别的语法错误统一折叠为 `regex::Error::Syntax("invalid Go regular expression")`；上层 `TableRule::Valid` 再转换成 table/schema/source 提取器各自的用户可见错误上下文。

## 主要符号

- `compile(pattern: &str) -> Result<Regex, regex::Error>`：模块唯一的 crate 内入口。它设置语法树与最终编译的 `nest_limit(1000)`，并给最终 `RegexBuilder` 设置 `size_limit(128 << 20)`。
- `invalid() -> regex::Error`：构造统一的 Go 正则语法错误。翻译器主动校验和 `regex_syntax` 解析失败均使用该错误；最终 `RegexBuilder::build` 的错误则按 `regex` 原样返回。
- `Parser<'a> { rest: &'a str }`：单遍消费输入切片的状态机。`rest` 始终指向尚未翻译的 UTF-8 后缀，没有独立游标或回溯缓存。
- `Parser::translate`：顶层语法翻译器。局部 `flags: [bool; 4]` 依次表示 `i/m/s/U`，`stack` 保存进入分组前的标志，`last_repeat` 防止一个表达式连续应用两个重复操作符。
- `Parser::take`：读取一个 Unicode 标量并按其 UTF-8 长度推进 `rest`；输入耗尽时返回 `invalid()`。
- `Parser::greediness`：综合紧随量词的显式 `?` 与 `U` 标志。两者不同时向译文追加 `?`，从而实现 Go 的“显式非贪婪”与“默认非贪婪后反转”规则。
- `Parser::class`、`Parser::class_atom`：解析否定类、首字符、范围、POSIX 类和转义。范围两端必须都是单码点且上界不小于下界；集合原子不能作为范围端点。
- `Parser::escape`：处理 ASCII Perl 类、控制字符、八进制、`\xNN`、`\x{...}`、Unicode 属性及可转义 ASCII 标点；不在白名单中的字母数字转义会被拒绝。
- `Atom::{Rune(u32), Set(String)}`：区分“单个码点”和“已展开集合”。这是字符类范围校验所必需的，例如集合 `\d` 不能误当成范围端点。
- `rune(value)`：将码点输出为 `\x{...}`。代理项 `U+D800..U+DFFF` 在 Rust 字符模型中无合法标量，故输出永不匹配的 `[a&&b]`。
- `range(lo, hi)`：把跨代理项区间切成合法标量的两段；完全落在代理项范围时同样返回空集合。
- `emit(out, atom, flags)`：将当前 `i/m/s` 标志以局部分组 `(?ims:atom)` 包裹在原子上；`U` 由量词逻辑处理。逐原子固定标志可保留 `a*(?i)*` 这类 Go 允许的标志组与重复符组合语义。
- `check_repeats(ast) -> Result<u32, regex::Error>`：递归计算一条语法树路径上的有界重复权重。重复节点乘以上界/精确次数/至少次数，连接与分支取子项最大值，权重大于 1000 即报错。
- `GO_UNICODE_CLASSES`：Go 1.25 属性名及类别别名到 Rust `regex` 规范名的静态映射。查询前会移除 `_`、`-`、空格并转小写；`Cs`（代理项）单独映射为空集，其否定映射为任意标量集合。

本文件没有 trait、trait 实现、条件编译模块或持久化全局可变状态；唯一 `impl` 是 `impl Parser<'_>`。

## 执行流程

1. `TableRule::Valid` 按 table、schema、source 的顺序处理可选提取器；每个正则经 `router.rs::compile_extractor` 进入 `go_regex::compile`。
2. `compile` 创建仅持有原始 `pattern` 的 `Parser`，调用 `translate`。顶层循环每次消费一个字符，并依据分组、分支、量词、字符类、引用块、锚点、转义或普通字面量生成译文。
3. 处理分组时，普通 `(` 保存当前标志；命名捕获 `(?P<name>...)` 与 `(?<name>...)` 校验名字但输出普通捕获 `(`，以允许 Go 支持而 Rust 禁止的重复名字。标志组只允许 `i/m/s/U` 和关闭标志的 `-`，带 `:` 的组在 `)` 时恢复进入前标志，不带 `:` 的标志变更持续到当前外层分组结束。
4. 处理量词时，`*`、`+`、`?` 和合法 `{m}`、`{m,}`、`{m,n}` 都检查 `previous_repeat`。计数必须是无多余前导零的十进制数且每个显式数字不超过 1000；不符合计数语法的 `{...}` 按 Go 规则作为字面文本，而不是直接交给 Rust 解析器报错。
5. 字符类由 `class` 解析后整体作为原子发给 `emit`。`\d/\w/\s` 等在 `escape` 中展开为 ASCII 集合；POSIX 名称必须在固定白名单内；范围通过 `range` 排除代理项。
6. `\Q...\E` 逐字符转为字面码点；没有结束 `\E` 时引用持续到输入末尾。`\b/\B` 用 `(?-u:...)` 关闭 Unicode，使词边界遵从 Go 的 ASCII 定义；`\A/\z` 保留为锚点。
7. 输入消费完后，未清空的分组栈使翻译失败。成功译文先由 `regex_syntax::ast::ParserBuilder` 以嵌套上限 1000 解析，防止翻译结果包含 Rust 侧不可接受的结构。
8. `check_repeats` 先验证子节点再乘当前重复次数。这一顺序保证即使外层是 `{0}`，内部超限重复仍像 Go 一样失败；例如 `(a{1000}){0}` 合法，而 `((a{100}){100})` 因乘积 10000 被拒绝。
9. 最终 `RegexBuilder` 以相同嵌套上限和 128 MiB 编译大小上限生成 `Regex`。`TableRule::Valid` 把它保存到相应提取器；`TableRule::extractVal` 后续调用 `captures`，跳过完整匹配并拼接所有已参与匹配的捕获组，供 `FetchExtendColumn` 返回扩展列值。

## 数据与状态

- 翻译期唯一跨步骤状态是 `Parser::rest`；它借用调用者的 `&str`，随每次 `take` 或整段语法消费而前移。没有修改原始配置，也没有缓存跨调用结果。
- `translate` 内的 `flags` 只影响后续生成的原子和量词；`stack: Vec<[bool; 4]>` 同时承担括号平衡校验与分组退出时的标志恢复。命名捕获也压栈，但名称本身不会进入译文，因为路由只按捕获序号拼接结果。
- `last_repeat` 只记录上一个有效语法单元是否为量词。每轮先保存为 `previous_repeat` 再清零；标志组不会建立新匹配原子，因此可以检测 `a*(?i)*` 中第二个 `*` 仍作用于前一表达式，同时拒绝普通连续量词。
- `Atom::Rune` 保留可参与范围比较的 `u32` 值；`Atom::Set` 保存已翻译集合文本，阻止集合被错误用于 `lo-hi`。
- `check_repeats` 返回的 `u32` 是校验权重，不是运行时计数。当前单个显式计数先限制到 1000，乘法只会在达到超限后结束本层；在这些界限下不会发生 `u32` 溢出。
- 成功结果 `Regex` 的所有权由上层 extractor 的 `Option<Regex>` 持有。`TableRule` 克隆时正则也随规则克隆；本文件自身不持有编译结果。
- `GO_UNICODE_CLASSES` 是只读静态切片；每次属性查询线性扫描该表，没有延迟初始化或共享可变缓存。

## 依赖与调用关系

上游主链为：

`NewTableRouter` / `Table::AddRule` / `Table::UpdateRule` → `Table::insert_rule` → `TableRule::Valid` → `compile_extractor` → `go_regex::compile`。

RustCodeGraph 对 `go_regex.rs::compile` 的节点追踪确认直接调用者是 `router.rs::compile_extractor`，直接下游包括 `Parser::translate`、`check_repeats` 和 `invalid`；对 `compile_extractor` 的节点追踪确认调用者是 `TableRule::Valid`。`TableRule::Valid` 为三类提取器复用同一入口，因此本文件的任何语义变化会同时影响 table、schema、source 的提取。

下游依赖分工如下：

- `regex_syntax::ast`：仅用于解析已经翻译的表达式并遍历 `Ast`、`RepetitionKind`、`RepetitionRange`，补做嵌套重复乘积校验。
- `regex::{Regex, RegexBuilder}`：产生运行时匹配对象；实际捕获发生在 `router.rs::TableRule::extractVal`。
- `pkg/util/table-router/router.rs`：负责错误上下文化、保存编译结果并把捕获内容暴露给 `FetchExtendColumn`。
- `pkg/util/table-router/lib.rs`：公开 `router` 及其类型，但没有公开 `go_regex`，所以兼容层是 crate 内实现细节。
- `pkg/util/table-router/Cargo.toml`：声明 `regex`、`regex-syntax` 和 selector 依赖，并用 `package.metadata.porting.go-package` 指向 Go 包 `pkg/util/table-router`。

更上层消费者通过 table-router crate 使用这些规则，例如 `lightning/pkg/importer` 的配置持有 `table_router::TableRule`，`pkg/util/regexpr-router` 也复用同一 `TableRule` 类型。它们并不直接依赖本文件，但 extractor 配置最终会受这里的 Go 兼容编译规则约束。

## 错误处理与边界

- 翻译器把空输入视为合法空正则；不平衡右括号会因 `stack.pop` 失败报错，不平衡左括号会在循环结束后因栈非空报错。
- 命名捕获名不能为空，且只能含 ASCII 字母、数字或下划线；不同捕获可以重名，因为输出丢弃名字、保留索引顺序。
- 标志只接受 `i/m/s/U`；重复 `-`、只有 `-` 而未关闭任何标志、未知标志或意外终止均报错。Rust 特有的 `x`、`u` 等不会漏入最终引擎。
- 连续量词报错；合法计数量词中的任一数字大于 1000 报错，嵌套有界重复的路径乘积大于 1000 也报错。连接和交替只取最大路径权重，不把互不嵌套的重复相乘。
- 类范围必须升序且两端为单码点。未知 POSIX 类、集合作为范围端点、类未闭合都会报错。Go 语义中的字面 `-` 与 Rust 集合运算符通过自行编码码点避免歧义。
- 八进制转义最多取三位；非零的一位八进制形式被拒绝。`\x{...}` 必须非空、全十六进制且不超过 `U+10FFFF`；代理项不是语法错误，但被编译为空集合，以反映 Go 可表示字节序列与 Rust Unicode 标量模型之间的边界。
- Unicode 属性名使用 Go 1.25 映射；未知名称报错。该表是版本化兼容事实，未来 Go Unicode 表变化不会自动同步。
- 语法树和最终编译均限制嵌套深度为 1000，最终自动机大小限制为 128 MiB。后一个失败可能保留 `regex` crate 的具体错误，而不是统一的 `invalid()`。
- 上层 `TableRule::Valid` 会隐藏底层错误细节，只报告诸如 `source extractor source regexp illegal <pattern>`。它按 table → schema → source 顺序更新状态，所以后续提取器失败时，之前已经成功编译的字段会保留；`router_test.rs::test_go_valid_partial_state_and_update_atomicity` 固定了这一行为。
- 绕过 `Valid`、直接向公开 selector 注入含未初始化 extractor 的规则，会在 `extractVal` 的 `expect` 处 panic；这属于编程错误，测试 `test_go_unvalidated_extractor_panics` 明确记录了该边界。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、文件句柄、网络连接或事务。每次 `compile` 都在当前线程同步完成，所有翻译期 `String`、`Vec` 和 AST 在返回前释放，只有成功生成的 `Regex` 交给调用者。

`Parser` 只借用不可变输入，`GO_UNICODE_CLASSES` 只读，函数内没有共享可变状态，因此并发调用本文件不需要额外同步。最终 `Regex` 的并发能力由 `regex` crate 类型保证；本文件不包裹锁，也不建立全局缓存。

主要资源风险来自恶意或意外复杂的配置正则，而非运行时竞争：`nest_limit(1000)`、重复权重上限和 `size_limit(128 << 20)` 限制编译深度与自动机体积；输入翻译和 Unicode 属性查找仍分别随输入长度、属性表长度线性增长。正则只在规则 `Valid` 时编译，匹配阶段复用 extractor 中的已编译对象。

## 与 Go 版本的对应关系

Go 对照实现位于 `pkg/util/table-router/router.go`：`TableRule.Valid` 对三类 extractor 直接调用 `regexp.Compile`，`extractVal` 使用 `FindStringSubmatch` 并拼接下标 1 以后的捕获组。Rust 的 `router.rs` 保持相同的调用顺序、保存位置和捕获拼接规则；本文件专门补偿 Rust `regex` 与 Go `regexp` 的方言差异。

已由 `pkg/util/table-router/router_test.rs` 对照 Go 1.25 行为验证的差异包括：

- `\d/\w/\s` 及其否定形式只匹配 ASCII 集合，`\b/\B` 也使用 ASCII 词边界；
- `\141`、`\12`、`\Q...\E`、畸形计数的字面处理、字符类中的连字符/方括号、重复命名捕获可用；
- `x/u/R` 标志、未知 POSIX 类、集合范围端点、连续量词和超限嵌套重复被拒绝；
- `U` 标志与显式 `?` 共同决定贪婪性；标志可在组内或后续表达式上开启/关闭；
- Go Unicode 属性别名可用，代理项类别不匹配 Rust 字符串中的标量；
- Rust `regex` 的捕获优先级与 Go RE2 仍可能有引擎级差异，测试以 `((a|ab)*)` 等案例固定了当前需要的捕获结果。

Go 文件没有单独的 `go_regex.go`，因为兼容目标就是 Go 标准库本身。Go 的 `router_test.go::TestFetchExtendColumn` 提供基本多捕获组拼接基线；Rust 的 `migration_aster_unit_test.rs::fetch_extend_columns_matches_go_capture_concatenation` 复刻该业务场景，额外的 `test_go_regexp_*` 与 `test_go_extractor_syntax_and_capture_oracle` 则覆盖跨引擎兼容层特有边界。

## 扩展指南

- 新增或修正 Go 语法时，优先修改 `Parser::translate`、`class`、`class_atom` 或 `escape` 中最窄的负责分支，不要把未经审查的原始片段直接透传给 Rust `regex`；否则 Rust 独有标志、Unicode 默认值或集合运算可能重新泄漏进来。
- 新增需要参与 `a-b` 范围的语法时必须返回 `Atom::Rune`；展开为集合的语法必须返回 `Atom::Set`。这一区分是 Go 范围语法的不变量。
- 调整量词时要同时审查局部计数上限、`last_repeat`、`greediness` 和 AST 后置的 `check_repeats`。仅让 `RegexBuilder` 接受表达式不能证明 Go 会接受它。
- 升级 Go 兼容基线或 Unicode 版本时，应从目标 Go 版本的 `unicode.Categories`、`unicode.Scripts` 与类别别名重新核对 `GO_UNICODE_CLASSES`，并特别保留 `Cs`/代理项处理；同时确认 Cargo 锁定的 `regex-syntax` 是否接受映射后的规范名。
- 改变错误分类时要检查 `router.rs::TableRule::Valid` 的错误折叠行为；调用者当前只依赖提取器类型、原始 pattern 和成功/失败，不应无意暴露 Rust 引擎特有诊断。
- 性能改动应重点评估长 pattern 的重复字符串分配、逐原子 flag 包装、Unicode 表线性扫描和 128 MiB 编译上限。若引入缓存，需明确 key、淘汰策略与跨线程生命周期，不能改变 extractor 持有独立已编译 `Regex` 的当前语义。
- 测试必须继续放在独立文件中，不要内嵌到 `go_regex.rs`。方言/捕获回归应扩展 `pkg/util/table-router/router_test.rs`；与原 Go 业务用例逐项对齐的场景可扩展 `migration_aster_unit_test.rs`，并同步用本机目标 Go 版本的 `regexp.Compile`/`FindStringSubmatch` 取得 oracle。若 Go 版本本身发生变化，还应同步检查 `router_test.go` 与映射表注释。
- 由于 `go_regex` 是私有模块，上层新增入口仍应复用 `compile_extractor`/`TableRule::Valid`，避免创建第二条绕过兼容校验的编译路径。

## 验证依据

- RustCodeGraph 索引状态：项目索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/table-router` 确认本 crate 的生产文件、独立 Rust 测试和 Go 对照文件均已索引。
- RustCodeGraph 文件读取：`node --file pkg/util/table-router/go_regex.rs --offset 1 --limit 400` 与 `--offset 400 --limit 320` 覆盖本文件全部 685 行，确认模块级符号、分支和 `GO_UNICODE_CLASSES`。
- RustCodeGraph 符号证据：`node go_regex.rs::compile` 显示直接调用 `Parser::translate`、`check_repeats`、`invalid`，且被 `router.rs::compile_extractor` 调用；`node router.rs::compile_extractor` 显示它只转发到 `go_regex::compile`，调用者为 `TableRule::Valid`。单独的 `callers go_regex.rs::compile` 命令在本地 30 秒窗口内未返回文本，因此调用边以同一索引的 `node` Trail 和相邻源码共同确认。
- crate/入口证据：`pkg/util/table-router/Cargo.toml`、`pkg/util/table-router/lib.rs`、根 `Cargo.toml` 与 `Cargo.lock`；确认 crate 名、库入口、workspace 成员、依赖及当前锁定版本。
- Rust 调用与状态证据：`pkg/util/table-router/router.rs`，重点是 `TableRule::Valid`、`Table::insert_rule`、`TableRule::extractVal`、`FetchExtendColumn`、私有模块声明和 `compile_extractor`。
- Go 对照证据：`pkg/util/table-router/router.go` 的 `TableRule.Valid`、`extractVal`、`FetchExtendColumn`；`pkg/util/table-router/router_test.go::TestFetchExtendColumn`。
- 独立 Rust 测试证据：`pkg/util/table-router/router_test.rs::test_go_regexp_ascii_character_classes`、`test_go_regexp_dialect_validation`、`test_go_extractor_syntax_and_capture_oracle`、`test_go_valid_partial_state_and_update_atomicity`、`test_go_unvalidated_extractor_panics`，以及 `pkg/util/table-router/migration_aster_unit_test.rs::fetch_extend_columns_matches_go_capture_concatenation`。
- 人工复核结论：本文能从上游规则校验追踪到翻译、AST 校验、最终编译和捕获消费，并明确列出 Go 兼容原因、当前限制、错误/资源边界及安全扩展与独立测试位置；没有把未接线能力描述为已支持。

