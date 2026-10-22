# `lightning/pkg/importer/opts/precheck_opts.rs`

## 文件定位

文件提供一个两层的函数式选项模型：`PrecheckItemBuilderConfig` 保存已归并的两组选项，`PrecheckItemBuilderOption` 则表示“如何修改这份配置”的可发送、可共享闭包。

## 核心职责

文件提供一个两层的函数式选项模型：`PrecheckItemBuilderConfig` 保存已归并的两组选项，`PrecheckItemBuilderOption` 则表示“如何修改这份配置”的可发送、可共享闭包。两个公开工厂函数分别把调用方提供的 getter 选项和 loader 选项捕获进闭包。

关键行为是克隆隔离。`WithPreInfoGetterOptions` 和 `WithMDLoaderSetupOptions` 都在工厂函数被调用时执行 `to_vec()`，避免调用方随后修改原向量影响已创建的 builder option；闭包每次执行时又把捕获值 `clone()` 后写入目标配置，使同一个 option 可以安全地重复应用，且不同目标配置不会共享可变的向量容器。这里的写入是整字段替换，不是追加或合并。

## 主要符号

- `PrecheckItemBuilderConfig`：公开、`Clone + Default` 的配置快照，字段 `PreInfoGetterOptions: Vec<GetPreInfoOption>` 与 `MDLoaderSetupOptions: Vec<MDLoaderSetupOption>` 分别承载预检信息采集选项和 mydump loader 初始化选项。默认值是两个空向量。
- `PrecheckItemBuilderOption`：`Box<dyn Fn(&mut PrecheckItemBuilderConfig) + Send + Sync>` 类型别名。`Fn` 而非 `FnOnce` 允许重复调用，`Send + Sync` 允许该 option 跨线程移动或通过共享引用调用；但本文件本身不创建线程。
- `WithPreInfoGetterOptions(opts: &[GetPreInfoOption])`：在创建 option 时克隆切片，在应用时用新的克隆替换 `PreInfoGetterOptions`。元素类型定义在 [`get_pre_info_opts.rs`](get_pre_info_opts.rs)，是基于 `Arc` 的配置闭包。
- `WithMDLoaderSetupOptions(opts: &[MDLoaderSetupOption])`：采用相同的两阶段克隆语义，替换 `MDLoaderSetupOptions`。本 crate 使用 [`stubs.rs`](stubs.rs) 中基于 `Arc` 的 `MDLoaderSetupOption` 替身，以保持 Go 函数值可复制、可重复执行的契约。

两个工厂函数保留 Go 风格名称，并以 `#[allow(non_snake_case)]` 局部允许非 Rust 惯用命名；文件没有常量、trait、条件编译分支或私有辅助函数。

## 执行流程

1. 调用方准备一组 `GetPreInfoOption` 或 `MDLoaderSetupOption`，并调用对应的 `With...Options` 工厂。
2. 工厂立即对输入切片执行 `to_vec()`；因此返回值不借用输入，调用方可在之后清空、增长或释放原向量。
3. 工厂返回装箱的 `PrecheckItemBuilderOption`。该闭包捕获步骤 2 的向量所有权。
4. `NewPrecheckItemBuilderFromConfig` 创建默认的 `PrecheckItemBuilderConfig`，按传入顺序逐个执行 builder option。若多个 option 写同一个字段，后执行者整字段覆盖前者。
5. 上游随后遍历最终的 `MDLoaderSetupOptions`，先归并当前本地边界可观察的 loader 设置，再追加 Lightning 自身的 scan-file concurrency 覆盖并构造 loader；`PreInfoGetterOptions` 则传给 `NewPreImportInfoGetter`。

[`parity_test.rs`](parity_test.rs) 的 `contract_normal` 证明两组选项能进入配置并实际驱动下游配置；`contract_boundary` 证明空切片保持为空且工厂创建后的调用方变更不会穿透；`contract_resource_cleanup` 证明同一 builder option 重复应用会产生彼此独立的向量快照。

## 数据与状态

本文件维护的状态完全由调用方持有。`PrecheckItemBuilderConfig` 是短生命周期的组装快照，不含全局变量、缓存、静态单例或内部可变性。两字段中的元素是引用计数闭包：`GetPreInfoOption` 在 `get_pre_info_opts.rs` 中定义为 `Arc<dyn Fn(...) + Send + Sync>`，本地 `MDLoaderSetupOption` 在 `stubs.rs` 中采用相同结构，因此向量克隆只复制 `Arc` 句柄，不复制闭包捕获的底层数据。

不变量包括：默认配置的两组向量均为空；每个工厂只写自己的字段；每次应用均替换整个字段；工厂创建后不再依赖原切片；重复应用不会复用同一向量容器。闭包元素本身通过 `Arc` 共享且只能以不可变引用调用，当前类型没有为闭包内部可变状态提供额外约束。

## 依赖与调用关系

直接类型依赖只有 `crate::get_pre_info_opts::GetPreInfoOption` 和 `crate::mydump::MDLoaderSetupOption`。后者由 `lib.rs` 先从 `stubs.rs` 再导出；`Cargo.toml` 的 `[dependencies]` 为空，并注明真实 mydump 移植使用不可克隆的 `FnOnce`，所以此 crate 暂以本地 `Arc` 替身保存 `slices.Clone` 语义。

RustCodeGraph 将 `WithPreInfoGetterOptions` 的直接调用者定位到 `parity_test.rs` 的 `contract_normal`、`contract_boundary` 和 `contract_resource_cleanup`。生产链通过类型消费体现：`precheck.rs::NewPrecheckItemBuilderFromConfig` 接收 builder option，循环调用 `o(&mut builderCfg)`，再读取两个字段；该函数的下游包括 `NewTargetInfoGetterImpl`、`NewPreImportInfoGetter`、mydump loader 和 `NewPrecheckItemBuilder`。`lib.rs` 的公开再导出使调用方无需访问私有模块路径。

## 错误处理与边界

本文件 API 不返回 `Result`，也没有主动 panic、I/O 或错误转换。空切片合法，并在应用后得到空向量；默认配置同样可直接使用。闭包要求传入有效的可变配置引用，Rust 类型系统排除了空指针形式的调用。

错误边界位于上游消费者：`NewPrecheckItemBuilderFromConfig` 在数据库连接、目标信息 getter、loader、预检信息 getter或 checkpoint 初始化失败时负责传播错误；其中 loader 可返回“部分 loader + 告警错误”。这些错误不是本文件产生或吞掉的。本文件需要特别注意的兼容边界是整字段覆盖语义：若扩展者误改成 `extend`，多 option 顺序应用的含义会偏离 Go；若移除捕获时或应用时克隆，则会破坏调用方变更隔离或重复应用隔离。

## 并发与资源生命周期

`PrecheckItemBuilderOption`、`GetPreInfoOption` 与本地 `MDLoaderSetupOption` 都带 `Send + Sync`，因此类型层面允许跨线程传递和共享调用。不过文件没有锁、通道、任务、事务或线程启动逻辑；实际预检构建流程当前是顺序应用 option。

所有资源通过 Rust 所有权和引用计数管理：builder option 拥有捕获向量，配置拥有应用时生成的新向量，各向量元素共享 `Arc` 闭包；丢弃 option 或配置时相应引用计数自动递减，没有显式清理协议。`parity_test.rs::contract_resource_cleanup` 验证配置与 option 可直接 `drop`，且一次应用后修改目标配置不会污染下一次应用。

## 与 Go 版本的对应关系

Go 工厂在 option 执行时调用 `slices.Clone(opts)`；Rust 为满足无借用返回值与可重复调用，先在工厂创建时 `to_vec()`，应用时再 `clone()`。

签名差异是 Go 使用可变参数，而 Rust 接收切片；Go 函数值天然可复制，Rust 的依赖 option 使用 `Arc` 才能克隆；Go option 类型没有显式并发约束，Rust trait object 显式要求 `Send + Sync`。另一个迁移限制来自 `Cargo.toml` 和 `stubs.rs`：此 crate 尚未直接依赖真实 mydump option 类型，而是用可克隆替身覆盖当前测试观察到的字段。

## 扩展指南

新增一类 builder 子选项时，应在 `PrecheckItemBuilderConfig` 增加独立字段，在本文件增加只修改该字段的工厂，并在 `precheck.rs::NewPrecheckItemBuilderFromConfig` 明确消费最终值；不要把下游 I/O 或错误处理塞入本文件。若新增项可克隆，应维持“创建时脱离调用方容器、每次应用生成独立容器”的语义；若真实依赖是 `FnOnce` 或持有不可克隆资源，则需先重新设计所有权契约，不能仅删除克隆来勉强编译。

测试应继续放在独立文件 [`parity_test.rs`](parity_test.rs)，至少同步覆盖默认值、空输入、调用方输入后续变更、同字段多 option 的覆盖顺序、重复应用和实际下游消费。若把本地 mydump 替身替换为真实 crate 类型，还需同步检查 `Cargo.toml`、`stubs.rs`、`lib.rs` 与 `precheck.rs` 的翻译层，关注 API 兼容、闭包可重复性、额外分配和 `Arc` 克隆成本；测试逻辑应尽量保持与 Go 同路径行为一致。
