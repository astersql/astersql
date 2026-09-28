# RustCodeGraph Skill

本目录记录 `rustcodegraph` 技能的补充说明。

## 主动监听

如果你想在本地开发时主动保持 `.rustcodegraph/` 索引新鲜，可以手工启动一个
前台监听命令：

```bash
~/.rustcodegraph/bin/rustcodegraph watch --path <project-root>
```

说明：

- 该命令适合你自己手工在项目里启动，用来监听代码变更。
- 项目已经完成初始化并存在索引后，watcher 会在变更后经过 debounce 自动执行增量
  `sync`。
- 默认 debounce 为 `2000ms`，可按需覆盖：

```bash
~/.rustcodegraph/bin/rustcodegraph watch --path <project-root> --debounce-ms 500
```

## 手动同步

以下场景仍然建议手工运行一次：

```bash
~/.rustcodegraph/bin/rustcodegraph sync --path <project-root>
```

- watcher 没有启动。
- watcher 已退化或暂时不可用。
- 刚切换分支、`git pull`、大批量生成代码，或你怀疑索引已过期。

## 建议

- `watch` 用于主动保鲜，适合你手工启动并常驻。
- `sync` 用于补救和校准，不建议把“每次查询前都手工刷新”作为默认工作流。
