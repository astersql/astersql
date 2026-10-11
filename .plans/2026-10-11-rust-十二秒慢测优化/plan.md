# Rust 十二秒慢测优化计划

目标：将 `target/rust-test.gnCQRN` 中 5 个超过 12 秒的 Rust 测试在保留 Go 对齐语义下稳定降至 12 秒内。

范围：优化 vectorsearch 三个 golden 测试、RealTiKV 外键自动建索引测试和 testutils 公共契约测试；以三次串行热运行和一次工作区复测持续验证耗时、有效测试数与行为断言。

范围外：日志中的 14 个功能失败、7 个 10 秒超时、其他 5–12 秒测试、放宽 nextest 超时、减少 Go 对齐的数据规模/规划器模式/断言；既有 Go 回归不阻塞 Rust 交付。

假设：来源日志记录 18158 个测试、总耗时 789.281 秒；5 个目标的基线分别为 16.989、16.572、16.448、14.982、14.575 秒。仓库已有 `perf-audit` profile 和 `tools/check/rust-test-performance.sh`，可执行三次串行热测。

## 设计决策

- 方案一是缩小 6000 行向量 fixture、131072 行外键 fixture 或契约矩阵，速度快但破坏 Go 测试意图，拒绝采用；方案二是提高超时，仅隐藏开销，也拒绝采用。
- 采用分段计时/采样定位后，优先批量化等价 SQL、复用同一测试内可安全复用的初始化、消除重复解析/建库；若必须改生产路径，只处理采样证明的热点并补独立回归测试。
- 每个目标以三次串行热运行均通过、每轮选中非零测试、最大耗时不超过 12 秒为最低门槛；同时记录相对来源基线的降幅，未达标则继续同一任务内迭代。
- 将工作拆成十个单一验证故事：先冻结基线，再分别处理 vector fixture、vector golden、vector 验收、testutils 拆分、testutils 两组契约、RealTiKV 基线、RealTiKV 优化，最后聚合与全量交付。
- 写同一文件或使用同一性能/RealTiKV 资源的任务全部串行；只有最终聚合已明确依赖全部局部验收，不以 Cargo 槽位数量推断可并行性。

## 架构说明

- vectorsearch 在 `pkg/planner/core/casetest/vectorsearch/vector_index_test.rs` 中为两种 Cascades 模式重复建立 6000 行 fixture，再执行 Go golden SQL。
- addindextest 保留 Go 的 14 次倍增、131072 行、外键成功/失败/恢复状态流；只能优化等价执行路径，不能降低行数。
- testutils parity 已复用 mock store，但仍反复重建 `addindex` fixture；复用必须保证 reset、取消、错误注入和资源清理隔离。
- Rust 测试逻辑继续放在独立测试文件；所有真正修复的 Rust 源文件保留 PingCAP 许可并具有 `// Copyright 2026 AsterSQL.`。

## 开发策略

- 对会改变行为的代码，使用失败验证-通过验证-重构。
- 性能失败验证为三次热测中至少一次超过 12 秒，并保存 TSV；功能失败验证为新增等价性/生命周期回归在优化前暴露错误共享或批处理差异。
- 将实施限制在来源 5 个热点及其不可缺少的局部接线，禁止递归补建子系统。
- 每次修改后先 `cargo fmt --all`，再运行目标功能测试与三次性能审计；最终使用 Ready 验证并审查差异。
