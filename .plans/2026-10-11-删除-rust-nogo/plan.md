# 删除 Rust nogo 镜像计划

目标：移除未接入生产链的 Rust nogo/linter 镜像，保留 Go/Bazel 静态检查链不变。

范围：删除 `build/config.rs`、`build/debug-linter` 与 `build/linter` 下的 Rust 源码、测试和对应说明，清理根 crate 接线、专用依赖及 Rust 文档索引中的失效链接。

范围外：不修改 Go 源码、Go 测试、`BUILD.bazel`、`build/nogo_config.json` 或 Bazel 的 `tidb_nogo` 配置。

假设：仓库搜索与 RustCodeGraph 均表明这些 Rust 文件没有生产执行入口；根 crate 仅直接暴露配置模块并在测试配置下挂接 linter 测试。

## 设计决策

- 以完整子系统为删除边界，避免只删配置后留下不能工作的 Rust analyzer 草稿。
- 保留 Go/Bazel nogo 链，因为它仍是当前仓库实际生效的静态检查机制。
- 删除 Rust 文件后同步清理索引和依赖，禁止留下悬空文档链接。

## 架构说明

- 根 `Cargo.toml` 的 `astersql` 包通过 `pkg/lib.rs` 暴露 Rust facade；本次只移除其中指向 `build/**` Rust linter 的接线。
- `build/**/*.go`、`build/**/BUILD.bazel` 与 `build/nogo_config.json` 构成 Go/Bazel 路径，不在删除范围。

## 开发策略

- 先用文件清单和引用搜索固定删除边界，再执行删除和接线清理。
- 删除属于结构简化，不适用失败测试；以零残留引用、Cargo 格式检查、根 crate 测试和 Ready 检查作为完成证据。
- 若验证发现非 linter Rust 生产代码依赖这些模块，恢复该文件并重新收窄范围。
