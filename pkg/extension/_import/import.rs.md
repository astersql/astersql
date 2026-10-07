# `pkg/extension/_import/import.rs`

## 文件定位

本文件属于 `astersql-extension` crate。crate 入口 `pkg/extension/lib.rs` 通过 `#[path = "_import/import.rs"] pub mod extension_import;` 将它公开为 `astersql_extension::extension_import`。它不是扩展框架的注册表实现；注册、manifest 和会话扩展等实际逻辑分别位于同 crate 的 `registry.rs`、`manifest.rs`、`session.rs` 等文件。

当前文件是仓库外扩展的**注册落点/生成代码挂载点**。源文件只有许可证和用途说明，没有 Rust item，因此当前模块本身不执行注册，也不参与请求处理。其存在价值是为以后放置或挂接生成的兄弟模块提供稳定目录和公开模块边界。

## 核心职责

1. 保留与 Go `pkg/extension/_import` 包相对应的 Rust 路径，表达“外部扩展导入集中放置于此”的约定。
2. 由 `pkg/extension/lib.rs::extension_import` 暴露一个稳定的模块名，使生成器将来无需在 crate 根临时发明新的挂载方式。
3. 明确当前边界：该文件不实现扩展发现、动态加载或注册；真正的扩展能力由 `pkg/extension` 其他模块提供，生成代码必须显式连接到那些 API。

## 主要符号

当前没有模块级常量、类型、trait、函数、`impl`、宏调用或条件编译项，也没有公开 API 符号。唯一相关的公开符号定义在父文件中：

- `pkg/extension/lib.rs::extension_import`：使用 `#[path = "_import/import.rs"]` 指向本文件的公开模块。

因此，不能把文件注释描述的未来生成模块当成已经存在的符号或已支持的运行时能力。RustCodeGraph 可以读取该文件，但对文件执行 callers/callees 查询时报告没有 definition，与“没有可调用符号”的源码事实一致。

## 执行流程

当前 Rust 构建中的流程止于模块装配：

1. Cargo 以 `pkg/extension/lib.rs` 作为 `astersql-extension` 的库入口。
2. `lib.rs` 按显式 `#[path]` 解析本文件，并建立公开的 `extension_import` 模块。
3. 编译器在本文件中找不到任何 item，因而不生成初始化函数、注册调用或运行时状态。
4. 当前仓库也没有 Rust 调用点通过该模块启动扩展注册。

未来若生成器增加 Rust 注册代码，必须同时明确生成 item 如何被 `extension_import` 纳入，以及应用启动路径如何触发注册；仅把文件写入本目录并不会自动产生 Go `init` 式副作用。

## 数据与状态

本文件不定义静态变量、集合、配置、句柄或其他状态，也不读取环境、manifest 或注册表。它不持有扩展实例，不规定注册顺序，不提供名称去重，并且没有序列化格式。扩展名称及选项等状态属于扩展框架的实际注册实现，不属于此落点模块。

当前最重要的不变量是：空模块的装配应当没有可观察的运行时副作用。若后续生成内容引入全局注册或初始化状态，应在实际实现与独立测试中说明初始化时机、重复执行语义和失败处理，不能依赖本文件现有的空模块语义推导。

## 依赖与调用关系

上游装配关系只有 `pkg/extension/lib.rs` 到本文件的模块声明。`pkg/extension/Cargo.toml` 指定 `[lib] path = "lib.rs"`，且没有为本模块设置 feature；所以只要构建该 crate，本模块就会被解析，但不会因此运行任何注册逻辑。

本文件没有 `use`、函数调用或类型引用，故不存在下游 Rust 调用边，也不直接使用 Cargo 中的 `etcd-client`、parser、session context、types 等依赖。RustCodeGraph 的精确文件查询未发现可供 `callers` 或 `callees` 分析的 definition。

Go 路径不同：`cmd/tidb-server/main.go` 使用空白导入 `_ "github.com/pingcap/tidb/pkg/extension/_import"`，让该包中生成文件的包级初始化在服务启动时执行。这个 Go 启动边不能视为 Rust 当前已有的调用边。

## 错误处理与边界

由于没有可执行代码，本文件没有 `Result`、错误类型、panic、日志、重试或降级路径。它也不会验证生成内容是否存在、扩展名是否冲突或注册是否成功。

边界风险主要出现在未来接线处：生成代码语法错误会成为编译错误；声明了模块但未接入启动路径会形成“已编译但未注册”；若注册 API 可失败，则错误必须由实际调用者处理或明确终止策略，不能在这个空挂载点静默丢弃。对外部扩展的兼容性约束应以其调用的公开注册 API 为准，而不是以本文件为空这一实现细节为准。

## 并发与资源生命周期

当前没有线程、异步任务、锁、通道、事务、网络连接或资源所有权，因而没有并发行为和清理阶段。模块解析发生在编译期；运行期不存在本文件发起的初始化或析构。

若未来模仿 Go 的启动注册语义，必须在实际接线中定义一次性初始化、并发重复注册、注册顺序及失败后的状态一致性，并用独立 Rust 测试覆盖。Rust 不会自动执行任意模块的“包初始化”，所以生命周期必须由显式函数、静态初始化机制或应用启动调用来建立。

## 与 Go 版本的对应关系

Go 对照文件 `pkg/extension/_import/import.go` 同样不含注册实现，只声明 `package extensionimport`，并说明每个扩展的自动生成注册文件会放在该目录。`pkg/extension/README.md` 给出的例子是生成 `import_example.go`，在其中空白导入扩展包；扩展包自己的 `init` 再调用 `extension.Register`。`cmd/tidb-server/main.go` 对 `_import` 包的空白导入保证这条初始化链在 TiDB 启动时发生。

Rust 文件保留了目录意图，但目前只被 `pkg/extension/lib.rs` 声明为空模块，没有对应的生成文件、注册 item 或应用启动调用。因此两端当前只在“集中式注册落点”概念上对应，运行语义尚不等价；文档不能声称 Rust 已经复刻 Go 的自动注册链。

## 扩展指南

新增 Rust 外部扩展接线时，最可能修改的是本文件或由它声明的独立生成模块，同时还需检查应用启动入口；建议遵循以下约束：

1. 让生成内容通过显式 `mod`/函数形成可追踪的 Rust 符号，不要假设同目录文件会被自动编译。
2. 使用 `pkg/extension` 已有公开注册能力，避免在 `_import` 内复制注册表、manifest 或会话逻辑。
3. 明确启动方及调用顺序，并定义重复注册、部分失败与多扩展顺序的行为。
4. 将测试放在独立的 `*_test.rs` 文件中，不把测试嵌入本生产文件；当前没有直接针对本模块的 Rust 测试，新增接线时应补充独立测试，并验证“生成模块被纳入”和“启动时确实注册”两个层面。
5. 同步评估 Go 生成器/构建规则与 Rust Cargo 模块声明，避免只更新一端。若增加依赖或 feature，应同步 `pkg/extension/Cargo.toml`；若改变 Go 文件集合，还需遵循仓库 Bazel 元数据流程。

主要兼容性风险是破坏既有模块路径或让 Rust 与 Go 注册时机分叉；主要正确性风险是漏接启动调用或重复注册；空模块本身没有性能成本，未来注册工作的性能取决于新增初始化逻辑。

## 验证依据

- `pkg/extension/_import/import.rs`：完整源码为许可证与中英文用途注释，没有 Rust item 或运行时逻辑。
- `pkg/extension/lib.rs:25-27`：以显式路径公开声明 `extension_import`，是本文件当前唯一 Rust 装配入口。
- `pkg/extension/Cargo.toml`：crate 名为 `astersql-extension`，库入口为 `lib.rs`，本模块没有独立 feature 或直接依赖声明。
- `pkg/extension/_import/import.go:15-18`：Go 包声明及“生成注册文件放入本目录”的原始约定。
- `pkg/extension/README.md:58-80`：`import_example.go` 空白导入扩展包、借助 Go `init` 完成注册的示例和解释。
- `cmd/tidb-server/main.go:46`：服务入口对 Go `_import` 包的空白导入。
- `pkg/extension/_import/BUILD.bazel`：当前 Go 目标只列出 `import.go`；仓库中未发现实际生成的同包注册文件。
- RustCodeGraph：`status` 显示索引包含本 Rust/Go 文件；`files --filter pkg/extension/_import` 列出二者；`node --file` 核对完整源码；对目标文件执行 `callers`/`callees` 均返回 `No definition found`，与无符号事实相符。
- 测试搜索：在 `*_test.rs` 与 `*_test.go` 中未发现 `extension_import`、`extensionimport` 或该路径的直接测试引用；因此当前没有可声称的本模块行为覆盖。
