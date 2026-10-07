# `pkg/expression/generator/builtin_threadsafe.rs`

## 文件定位

该文件属于独立 crate `astersql-expression-generator`，由 [`pkg/expression/generator/lib.rs`](lib.rs) 以公开模块 `builtin_threadsafe` 接入。crate 的 [`Cargo.toml`](Cargo.toml) 将其定位到 Go 包 `pkg/expression/generator` 的移植实现，并通过 `tree-sitter`、`tree-sitter-go` 解析 Go 源码；它是离线代码生成工具，不参与 SQL 表达式的运行时求值。

文件的直接 Go 对照是 [`builtin_threadsafe.go`](builtin_threadsafe.go)。Go 主工程当前的真实生成入口仍是 [`pkg/expression/builtin.go`](../builtin.go) 中的 `//go:generate go run generator/builtin_threadsafe.go`，并非调用本 Rust 模块。因此 Rust 文件提供了可独立调用和测试的对等生成能力，但不能据此断言主工程的 `go generate` 已切换到 Rust。

生成目标位于 `pkg/expression`：`builtin_threadsafe_generated.go` 为判定安全的签名生成递归检查与原子缓存方法，`builtin_threadunsafe_generated.go` 为其余签名生成恒为 `false` 的方法。两者均实现各签名的 `SafeToShareAcrossSession` 方法。

## 核心职责

1. 使用 tree-sitter Go 语法树读取 `builtin_*.go` 中的类型声明，而不是以文本或正则猜测结构。
2. 只分类名称满足 `builtin*Sig` 的结构体：显式位于 `SPECIAL_SAFE_FUNCS` 白名单，或仅含一个类型为 `baseBuiltinFunc`/`baseBuiltinCastFunc` 的字段时归为安全；其余候选归为不安全。
3. 稳定扫描目标目录中的生产 Go builtin 文件，排除目录、非 `builtin_*.go` 文件以及名称含 `_test` 的文件。
4. 将安全和不安全名称分别套入 Go 方法模板，调用外部 `gofmt` 完成格式化和语法校验。
5. 提供“只返回生成物”和“顺序写入两个生成文件”两层 API，最后保留一个以当前目录为目标的私有 `main`。

这些职责由 `collect_thread_safe_builtin_funcs`、`gen_builtin_thread_safe_code`、`generate_code`、`generated_outputs` 和 `write_generated_outputs` 依次承担。

## 主要符号

- `SPECIAL_SAFE_FUNCS: &[&str]`：12 个显式安全签名的白名单。它覆盖 `IN` 族和 `IS TRUE`/`IS FALSE` 族，即使这些结构体带有基类之外的状态也按维护者判定生成安全实现。新增条目必须同步验证实际状态共享语义和测试。
- `GoTypeSpec`：解析阶段的最小中间表示，记录类型名、右侧是否为结构体及每个字段可识别的简单类型名。字段公开，但该类型只在本模块解析/分类链中构造和消费。
- `node_text`：从 tree-sitter 节点借用 UTF-8 文本；非法 UTF-8 转为 `String` 错误。
- `visit_type_specs`：递归访问语法树中的 `type_spec` 和 `type_alias`。对于结构体，它只从每个 `field_declaration` 的 `type` 字段提取简单 `type_identifier`/`identifier`；命名字段、匿名字段和字段 tag 因而按 Go AST 的字段类型规则统一处理。
- `parse_go_type_specs`：读取文件、初始化 Go parser、建树、拒绝含语法错误的树，再调用 `visit_type_specs`。这是内部解析边界。
- `collect_thread_safe_builtin_funcs`：公开的单文件分类入口，返回 `(safe_names, unsafe_names)`。安全集合按声明遍历顺序产生；不安全集合是“全部候选减去安全集合”，也保留候选遍历顺序。
- `gen_builtin_thread_safe_code`：公开的目录级生成入口，返回安全与不安全两个 Go 源码字节向量。它先按路径排序输入文件，聚合分类结果，并额外对安全名称做全局字典序排序。
- `generate_code`：公开的模板展开入口。每个名称只替换模板中的第一个 `%s`，随后调用 `format_go_source`。
- `format_go_source`：私有外部进程适配器，通过管道向 `gofmt` 写入源码并读取标准输出/错误。
- `generated_outputs`：公开的无写盘边界，固定返回两个 `(PathBuf, Vec<u8>)`，路径分别指向安全和不安全生成文件。
- `write_generated_outputs`：公开的写盘入口，按 `generated_outputs` 的数组顺序先写安全文件、再写不安全文件。
- `main`：以 `.` 为表达式源码目录调用写盘入口，失败时 panic。由于本 crate 的 `[lib] path = "lib.rs"` 且没有声明 binary target，这个函数只是模块内私有函数，不是 Cargo 可执行入口。
- `SAFE_FUNC_TEMPLATE` / `UNSAFE_FUNC_TEMPLATE`：分别生成递归安全检查方法和恒假方法。
- `SAFE_HEADER` / `UNSAFE_HEADER`：完整 Go 文件头。前者还定义三态原子缓存辅助函数 `safeToShareAcrossSession`，后者仅声明包。

## 执行流程

完整写盘链路如下：

1. `main` 或库调用者把表达式源码目录交给 `write_generated_outputs`。
2. `generated_outputs` 调用 `gen_builtin_thread_safe_code`，并把两份字节内容绑定到固定目标文件名。
3. `gen_builtin_thread_safe_code` 用 `read_dir` 枚举目录。真正的目录被跳过；符号链接只要名称匹配且 `file_type().is_dir()` 为假，就会像 Go `os.ReadDir` 版本一样沿链接读取。
4. 候选名必须以 `builtin_` 开头、以 `.go` 结尾且不包含 `_test`。路径排序后逐文件调用 `collect_thread_safe_builtin_funcs`。
5. `parse_go_type_specs` 建立 Go 语法树；`visit_type_specs` 深度遍历类型声明并形成 `GoTypeSpec`。
6. 分类器忽略非 `builtin*Sig`、非结构体类型。白名单优先判安全；否则只有“恰好一个字段，且字段类型是两个基类之一”才安全。所有已识别候选中未进入安全集合的都进入不安全集合。
7. 安全名称全局排序；不安全名称按“已排序文件顺序 + 文件内 AST 顺序”保留。两组分别传给 `generate_code`。
8. `generate_code` 拼接文件头和逐签名方法模板，`format_go_source` 将结果送入 `gofmt`。只有 `gofmt` 成功退出才返回其标准输出。
9. `write_generated_outputs` 使用 `fs::write` 依次覆盖两个目标文件。任一步错误立即返回，后续步骤不执行。

也可停在较早边界：测试或工具可以直接调用单文件分类、目录生成、模板格式化或 `generated_outputs`，避免写盘副作用。

## 数据与状态

解析状态完全是单次调用内的拥有值：源码为 `Vec<u8>`，语法树只在 `parse_go_type_specs` 内存活，最终转成拥有 `String` 的 `Vec<GoTypeSpec>`。分类阶段以 `HashSet<&str>` 加速白名单和安全集合查询；这些引用只借用静态白名单或当前函数内的名称向量，不逃逸函数。

生成器本身没有全局可变状态。`SPECIAL_SAFE_FUNCS` 和四个模板/文件头常量是只读静态数据。容量预分配（例如候选、安全/不安全名称的容量 32）只影响分配行为，不限制结果数量。

生成出来的 Go 安全文件包含真正的并发状态：每个签名持有的 `safeToShareAcrossSessionFlag` 使用 `uint32` 三态，`0` 表示尚未计算，`1` 表示安全，`2` 表示不安全。首次检查递归遍历 `args`，发现第一个不安全参数即短路；结果使用 `atomic.StoreUint32` 发布。后续调用通过 `atomic.LoadUint32` 直接复用缓存。

## 依赖与调用关系

RustCodeGraph 给出的生产调用边为：

`main → write_generated_outputs → generated_outputs → gen_builtin_thread_safe_code → collect_thread_safe_builtin_funcs → parse_go_type_specs → visit_type_specs → node_text`

另一路生成边为：

`gen_builtin_thread_safe_code → generate_code → format_go_source → 外部 gofmt`

上游方面，`pkg/expression/generator/lib.rs` 公开该模块；仓库索引未显示其他 Rust 生产文件调用这些公开函数，直接调用者主要是本模块的内部链路和两个独立测试文件。RustCodeGraph 对 `write_generated_outputs` 的 callee 结果还出现了 `pkg/parser/parsergen/generate.rs` 中的同名符号，这是基于短名称消歧不足产生的跨文件假边；源码中的真实直接依赖只有本文件的 `generated_outputs` 与 `std::fs::write`。

下游依赖分为三类：标准库负责目录/文件 IO、路径、集合、进程与管道；`tree-sitter` 和 `tree-sitter-go` 负责 Go AST；系统 PATH 中的 `gofmt` 负责最终语法验证与规范格式化。`gtmpl` 虽列在 crate 依赖中，但本文件没有使用它。

Go 主链由 `pkg/expression/builtin.go` 的 `go:generate` 调用 `generator/builtin_threadsafe.go`，生成文件再被 `pkg/expression/BUILD.bazel` 纳入 Go 构建。Rust 实现目前是对等库能力和迁移验证面，不是该 Go 主链的直接调用节点。

## 错误处理与边界

所有公开 Rust 工作函数以 `Result<_, String>` 传播失败；底层 IO、UTF-8、parser language 设置、进程创建/等待和写管道错误都被转成字符串，因此保留可读消息但丢失结构化错误类型和部分路径上下文。

关键失败边界如下：

- 文件不可读、目录不可枚举或目录项元数据不可取时立即失败。
- tree-sitter 无法返回语法树，或根节点带语法错误时拒绝分类；后者错误中包含源文件路径。
- 类型声明缺少名称或右侧类型时失败，而不是静默跳过损坏节点。
- 只有简单标识符字段类型会写入 `field_type_names`；指针、选择器、泛型等复杂类型记为空字符串，因此不会误判为两个允许的基类。
- 文件筛选使用“名称包含 `_test`”而不只是 `_test.go` 后缀，与 Go 对照实现一致；这会排除任何中间位置含该片段的候选名。
- `generate_code` 只替换每份模板的第一个 `%s`。当前模板只有一个占位符；扩展模板时不能假定所有占位符都会展开。
- `gofmt` 不存在、不能启动、标准输入写入失败或以非零状态退出都会导致错误；非零退出时返回其标准错误。
- 写盘不是事务性的：安全文件成功写入而不安全文件失败时会留下部分更新；`fs::write` 也不是显式原子替换。调用者需要把这一点纳入生成失败恢复策略。
- 模块内 `main` 用 panic 把错误升级，但库 API 本身不 panic（标准库分配失败等进程级情况除外）。

分类规则是保守的：未知额外状态默认不安全。白名单则绕过结构形状检查，错误加入白名单可能把携带会话状态的签名错误标为可共享，是该文件最高风险的维护边界。

## 并发与资源生命周期

Rust 生成过程是同步、单线程、逐文件和顺序写盘的，没有锁、线程、异步任务或通道。每次 `format_go_source` 都创建一个独立 `gofmt` 子进程：父进程取得其 stdin，写完源码后释放管道句柄，再通过 `wait_with_output` 等待进程结束并收集 stdout/stderr，不留下后台任务。

文件句柄和目录迭代器遵循 Rust RAII 自动释放。tree-sitter 节点借用语法树及源字节，只在解析调用内使用；输出的 `GoTypeSpec` 不持有这些借用，因此离开解析函数后仍有效。

并发语义主要存在于生成的 Go 代码。原子三态缓存允许多个会话/调用者并发读取和发布相同结论，不会产生数据竞争。多个首次调用者可能同时重复遍历参数并分别存入相同结果；由于参数安全性应是稳定属性，代码不做 compare-and-swap，也不承诺只计算一次。若这一不变量未来改变，当前缓存协议就不再充分。

不要并发运行多个 `write_generated_outputs` 指向同一目录：本文件没有跨进程锁或临时文件重命名协议，两个生成器可能互相覆盖，读者也可能观察到两份文件不一致的中间状态。

## 与 Go 版本的对应关系

Rust 版本逐段对应 [`builtin_threadsafe.go`](builtin_threadsafe.go)：

- `SPECIAL_SAFE_FUNCS` 对应 `specialSafeFuncs`，当前 12 个名称一致。
- `collect_thread_safe_builtin_funcs` 对应 `collectThreadSafeBuiltinFuncs`。Go 用标准库 `go/parser`/`go/ast`，Rust 用 tree-sitter，但都检查 `TypeSpec`、`builtin*Sig` 命名、结构体形状、白名单和单一基类字段。
- `gen_builtin_thread_safe_code` 对应 `genBuiltinThreadSafeCode`，文件筛选、输入排序、安全集合排序及双份输出一致。两者都没有对不安全集合单独全局排序。
- `generate_code` 对应 `generateCode`；Go 直接调用 `format.Source`，Rust 通过外部 `gofmt` 达到格式化和语法拒绝效果，因此 Rust 多了系统工具可用性这一运行条件。
- `generated_outputs` 是 Rust 为分离生成与 IO 增加的安全接口，Go 版本没有同名层。
- `write_generated_outputs` 与私有 `main` 合起来对应 Go `main` 的两次顺序写盘。Go 失败后 `log.Fatalln` 退出；Rust 写盘函数返回错误，私有 `main` 再 panic。
- 两套模板和文件头的行为一致：安全方法递归检查参数并使用原子三态缓存，不安全方法恒为 `false`。

独立 Rust 测试补充验证了 Go AST 的细节对齐：结构体别名、命名字段、带 tag 的匿名字段、额外状态、白名单、非结构体别名、扫描过滤、字典序、符号链接以及格式化失败。仓库中未找到针对 Go 生成器函数本身的同目录 `*_test.go`；本任务因此以 Go 源实现和 Rust 对抗/迁移测试作为对照证据。

## 扩展指南

- 新增显式安全签名时，修改 `SPECIAL_SAFE_FUNCS`，并先证明其自身状态不携带会话相关可变数据。同步扩展 [`builtin_threadsafe_1_aster_unit_test.rs`](builtin_threadsafe_1_aster_unit_test.rs) 或 [`builtin_threadsafe_test.rs`](builtin_threadsafe_test.rs)，同时保持 Go `specialSafeFuncs` 一致。
- 调整“仅嵌入基类即安全”的结构判定时，修改 `visit_type_specs`/`collect_thread_safe_builtin_funcs`，重点覆盖匿名与命名字段、tag、别名、复杂字段类型、零字段和多字段。规则应继续保守处理无法识别的类型。
- 改变扫描范围时，在 `gen_builtin_thread_safe_code` 接入，并同步验证目录、测试文件、普通 Go 文件、符号链接和确定性排序。不要无意把生成文件自身或测试 fixture 纳入输入。
- 修改生成方法时，更新对应模板或 header，并用 `generate_code_formats_and_validates_go_source` 验证合法 Go 语法；还需核对 `pkg/expression` 中 `BuiltinFunc.SafeToShareAcrossSession` 的契约和生成结果使用方。
- 改变输出路径或写盘策略时，优先在 `generated_outputs`/`write_generated_outputs` 层处理，保留纯生成函数便于测试。若需要可靠发布，应考虑同目录临时文件、完整生成成功后再原子重命名，并明确双文件一致性策略。
- 若要让 Rust 生成器取代当前 Go `go:generate` 入口，需要另行接线 Cargo 可执行目标和 `pkg/expression/builtin.go`；本文件当前的私有 `main` 不会自动成为 binary。该迁移还应验证 CI/开发环境中 `gofmt` 和 Rust 工具的可用性，不能只改注释。
- 测试逻辑必须继续保留在独立测试文件中，不要嵌入本生产文件。性能风险主要来自重复解析大量 Go 文件和每次生成启动两个 `gofmt` 进程；兼容风险集中在 tree-sitter Go AST 形状与 Go 标准 AST 的差异，以及白名单误分类。

## 验证依据

- 目标源码：[`pkg/expression/generator/builtin_threadsafe.rs`](builtin_threadsafe.rs)，完整核对了 344 行源码中的常量、类型、函数、模板和文件头。
- crate 与模块入口：[`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs)，确认这是 edition 2024、`publish = false` 的独立 library crate，依赖 `tree-sitter`/`tree-sitter-go`，且两个测试文件通过 `#[path]` 独立接入。
- Go 对照与主工程接线：[`builtin_threadsafe.go`](builtin_threadsafe.go)、[`pkg/expression/builtin.go`](../builtin.go) 和 `pkg/expression/BUILD.bazel`，确认 Go 算法、当前 `go:generate` 入口及生成文件构建关系。
- 独立测试：[`builtin_threadsafe_1_aster_unit_test.rs`](builtin_threadsafe_1_aster_unit_test.rs) 验证基本分类、白名单、扫描过滤和安全名称排序；[`builtin_threadsafe_test.rs`](builtin_threadsafe_test.rs) 验证结构体别名/命名字段/tag、Unix 符号链接、双文件写盘、`gofmt` 格式化和非法源码拒绝。
- RustCodeGraph：索引覆盖 11,467 个文件，目标文件识别出 12 个符号；`query` 定位主要函数，`node --file` 读取目标、Go 对照、模块入口与测试；`callers`/`callees` 验证了正文列出的生产调用链及测试调用者。对同名 `write_generated_outputs` 产生的跨模块假边已通过源码限定排除。
- 文档交付只进行结构与事实验证，不运行 Cargo，符合总计划对纯文档任务的限制。
