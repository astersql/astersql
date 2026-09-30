# 任务 7: Extract 视图 AST 遍历

批次：【批次 7】依赖批次：1

状态：未开始

目的：对齐原始 Go `extract.go` 从手工 Accept 改为 `ast.Walk` 的嵌套视图依赖遍历。

来源任务：43，`pkg/domain/extract.go`

预计会话范围：只处理视图 AST 递归访问与表依赖收集，不处理 ZIP 格式。

## 文件

- 修改：`pkg/domain/extract.rs`。
- 测试：`pkg/domain/extract_test.rs`。
- Go 对照：`pkg/domain/extract.go` 的 `handleIsView` 与 `pkg/parser/ast` Walk 语义。

## 上下文

- Go diff 只有一行 `ast.Walk(node, tne)`，目标是遍历嵌套 AST；Rust 已有 `ExtractHandle::new_with_domain` 的 parser 包装源。
- 最接近的现有回归是 `go_merge_43_extract_walks_nested_view_ast`。

## 测试计划

- 行为：视图查询里的嵌套子查询、join 和 CTE 中的真实表被收集，内部 schema 和循环引用遵循 Go 当前语义。
- 失败验证测试：若现有用例遗漏一个 Go Walk 访问分支，在 `extract_test.rs` 扩展 `go_merge_43_extract_walks_nested_view_ast`，先观察漏表。
- 失败/通过命令：`CARGO_TARGET_DIR=/tmp/astersql-plan43-domain cargo test --manifest-path pkg/domain/Cargo.toml --lib go_merge_43_extract_walks_nested_view_ast`
- 预期失败原因：旧遍历遗漏深层表节点。
- 模拟策略：用真实 parser AST 和 Domain 测试 schema，不用手写 AST mock。

## 步骤

1. 阅读 Go 目标函数、Rust AST visitor、现有测试。
2. 发现缺口则补失败用例并修正 visitor。
3. 通过聚焦测试，检查重复/循环依赖是否稳定。

## 完成

报告 AST 节点覆盖、Go 对照及失败→通过证据；无缺口时不添加镜像测试。Ready 通过后删除本文件。
