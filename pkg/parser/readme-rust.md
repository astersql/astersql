# AsterSQL Rust SQL 解析器

`astersql-parser` Rust 包负责 SQL 语法定义、解析器运行时和语义动作处理。解析表由开发者显式生成并提交到仓库，parser 的普通编译和运行只读取已提交的静态数据。

## 架构

- `grammar/main.astergram` 定义 SQL 主语法。
- `grammar/hint.astergram` 定义优化器提示语法。
- `astersql-parsergen` 根据两个 `.astergram` 文件确定性地生成 LALR 解析表。
- `generated/main_tables.rs` 保存主 SQL 解析器的 Token、符号、规约、`RuleId` 和状态表。
- `generated/hint_tables.rs` 保存优化器提示解析器的 Token、符号、规约和状态表。
- `generated/lexer_tokens.rs` 保存词法分析器使用的主语法 Token 常量。
- `lib.rs` 通过静态 `include!` 引入这三个已提交文件。
- `parser_runtime.rs` 实现解析运行时，`parser_actions/` 按功能领域组织具名 Rust 语义动作。

## 显式生成与提交

只有新增或修改 `.astergram` 语法时才运行生成命令：

```shell
cargo run -p astersql-parsergen --bin astersql-parsergen -- generate
```

生成后必须同时提交语法变更和以下三个文件：

- `generated/main_tables.rs`
- `generated/hint_tables.rs`
- `generated/lexer_tokens.rs`

提交前使用只读检查确认仓库生成物与当前语法一致：

```shell
cargo run -p astersql-parsergen --bin astersql-parsergen -- check
```

只修改解析运行时、词法器或既有 `RuleId` 对应的语义动作时，不运行 `generate`，也不改动生成文件。

## 普通编译与数据库运行

普通编译、检查和测试不会调用 `astersql-parsergen`，也不会改写仓库中的生成文件：

```shell
cargo build -p astersql-parser
cargo check -p astersql-parser
cargo test -p astersql-parser
```

数据库启动时直接使用已经编译进二进制的静态解析表，不读取 `.astergram`，不调用生成器。数据库收到 SQL 后，`Parser::New()` 创建解析状态，`Parser::ParseSQL()` 重置本次状态，`yyParse()` 使用静态 LALR 表完成移进、规约和错误恢复，并按 `RuleId` 调用 `parser_actions/` 中的语义动作构造 AST。数据库启动和每条 SQL 执行都不会生成或修改解析表。

## 维护流程

1. 新增或修改语法时，编辑相应 `.astergram`；如果产生式带有 `@action`，同步新增或更新稳定 `RuleId` 对应的具名 Rust 语义动作。
2. 在独立的 Rust 测试文件中新增或更新针对性的解析行为测试。
3. 仅当 `.astergram` 发生变更时运行 `generate`，审查并提交三份 `generated/*.rs` 变更。
4. 运行生成物校验和 parser、parsergen 回归：

```shell
cargo run -p astersql-parsergen --bin astersql-parsergen -- check
cargo test -p astersql-parser
cargo test -p astersql-parsergen
cargo check -p astersql-parser
cargo fmt --check -p astersql-parser -p astersql-parsergen
```

`parser_no_legacy_source_dependency` 保护 Rust parser 的静态生成物边界和维护说明；生成物 `check` 负责阻止缺失或过期的提交产物。语义动作所有权测试确保主语法中每个带 `@action` 标记的产生式都恰好只有一个具名所有者。
