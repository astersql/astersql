---
name: rustcodegraph
description: 通过 `rustcodegraph` 命令行界面使用 RustCodeGraph 理解、导航或脚本化操作已索引代码库。当用户要求使用 RustCodeGraph、需要高性能搜索检索代码、需要符号/源码/调用流上下文、调用方/被调用方/影响分析或受影响测试选择时使用。
---

# RustCodeGraph

将 RustCodeGraph 作为预计算代码地图使用：符号搜索、源码片段、
调用边、影响半径和受影响测试都来自本地 `.rustcodegraph/` 索引。
优先通过本技能调用 `~/.rustcodegraph/bin/rustcodegraph` 命令行界面。

## 首次检查

1. 从项目根目录工作，或传入 `-p <project-root>` / `--path <project-root>`。
2. 如果 `.rustcodegraph/` 存在，用 `rustcodegraph status` 检查新鲜度。
3. 如果没有 `.rustcodegraph/`，不要自动初始化，除非用户要求启用/索引
   RustCodeGraph。告诉用户命令是 `rustcodegraph init -i`，然后在本轮继续
   使用普通工具。

## 选择入口

优先使用 `rustcodegraph` 命令完成搜索、导航和影响分析。

| 意图 | 命令 |
| --- | --- |
| 检查索引 | `rustcodegraph status` |
| 调研某个区域或回答“X 是如何工作的？” | `rustcodegraph explore "<symbols or question>"` |
| 展示单个符号的源码 | `rustcodegraph node <symbol>` |
| 按行号读取已索引文件 | `rustcodegraph node --file <path> --offset <n> --limit <n>` |
| 按名称搜索 | `rustcodegraph query <name> --kind <kind> --limit 10` |
| 查找调用方/被调用方 | `rustcodegraph callers <symbol>` / `rustcodegraph callees <symbol>` |
| 估算重构影响 | `rustcodegraph impact <symbol> --depth 3` |
| 列出已索引文件 | `rustcodegraph files --filter <dir>` |

## 命令行工作流

大多数问题优先使用 `explore`。在查询中放入符号名、文件名，或用户的自然语言问题：

```bash
rustcodegraph explore "mutateElement renderStaticScene"
rustcodegraph explore "how does login reach the session middleware?"
```

只有在需要查找精确候选名称，或消除常见符号歧义时，才使用 `query`：

```bash
rustcodegraph query UserService --kind class --limit 10
rustcodegraph query handleRequest --json
```

当某个符号或文件需要更深入检查时，在 `explore` 之后使用 `node`：

```bash
rustcodegraph node AuthMiddleware
rustcodegraph node --file src/auth.ts --offset 80 --limit 120
```

在重构或行为变更前使用图命令：

```bash
rustcodegraph callers saveUser --limit 30
rustcodegraph callees handleRequest --limit 30
rustcodegraph impact AuthMiddleware --depth 3
```

在脚本或持续集成中使用 `affected`，根据已变更文件选择测试：

```bash
git diff --name-only | rustcodegraph affected --stdin --quiet
rustcodegraph affected src/auth.ts --filter "e2e/*"
```

当其他脚本或工具需要稳定的机器可读输出时，添加 `--json`。

## 新鲜度

- 切换分支、更新生成代码或批量编辑后，先运行 `rustcodegraph sync`，
  再依赖命令行界面的回答。
- 如果 `status` 显示没有文件，或某个命令提示项目未初始化，在用户选择索引之前，
  停止为该项目使用 RustCodeGraph。

## 使用规则

- 将 `explore` 输出视为已经读过的源码上下文；不要立刻用 `grep` 或文件读取重复检查。
- 只有在处理未索引文件、图未覆盖的文档/配置，或新鲜度警告点名了已变更文件时，
  才直接使用原始 `rg`/文件读取。
- 对于流程问题，保持查询精确：当用户提供端点符号和关键桥接名称时，把它们包含进去。
- 对重载名称或常见名称，使用带类型/文件提示的 `node <symbol>` 或
  `query <symbol>`，而不是逐个读取候选文件。
