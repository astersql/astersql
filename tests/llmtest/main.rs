// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! llmtest CLI (Go `tests/llmtest/main.go`): `generate` and `verify` subcommands.

// 本文件对应 `tests/llmtest/main.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
#[cfg(test)]
#[path = "stubs.rs"]
// 模块 `stubs` 在这里被显式接线，方便按既定边界编译。
// 阅读这一行时，可以把它看成当前 crate 的依赖入口说明。
// 保持导出关系稳定比改命名更重要。
mod stubs;

#[cfg(test)]
use crate::stubs::{cobra, mysql, os_exit, sql};
#[cfg(not(test))]
use astersql_tests_llmtest::stubs::{cobra, mysql, os_exit, sql};

use astersql_tests_llmtest_generator as generator;
use astersql_tests_llmtest_logger::{Global, ensure_init as ensure_logger, zap};
use astersql_tests_llmtest_testcase as testcase;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Process entry matching Go `main`.
// `run_main` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
// 这里的行为需要尽量贴近 Go 版本。
pub fn run_main() {
    ensure_logger();
    generator::ensure_init();

    let mut root_cmd = cobra::Command::new("llmtest");

    let generate_cmd = create_generate_cmd();
    let verify_cmd = create_verify_cmd();

    root_cmd.add_command(generate_cmd);
    root_cmd.add_command(verify_cmd);
    if let Err(err) = root_cmd.execute() {
        println!("{err}");
        os_exit(1);
    }
}

/// createGenerateCmd — OpenAI-backed case generation into testdata JSON.
// `create_generate_cmd` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
// 这里的行为需要尽量贴近 Go 版本。
pub fn create_generate_cmd() -> cobra::Command {
    let openai_token = Rc::new(RefCell::new(String::new()));
    let openai_base_url = Rc::new(RefCell::new(String::new()));
    let openai_model = Rc::new(RefCell::new(String::new()));
    let prompt_generator_name = Rc::new(RefCell::new(String::new()));
    let test_count = Rc::new(RefCell::new(0_i32));
    let generate_parallism = Rc::new(RefCell::new(20_i32));

    let mut generate_cmd = cobra::Command::new("generate");
    generate_cmd.set_short("Generate something using OpenAI");

    {
        let openai_token = Rc::clone(&openai_token);
        let openai_base_url = Rc::clone(&openai_base_url);
        let openai_model = Rc::clone(&openai_model);
        let prompt_generator_name = Rc::clone(&prompt_generator_name);
        let test_count = Rc::clone(&test_count);
        let generate_parallism = Rc::clone(&generate_parallism);

        generate_cmd.set_run(move |_args| {
            ensure_logger();
            generator::ensure_init();

            let prompt_generator_name = prompt_generator_name.borrow().clone();
            let prompt_generator = generator::get_prompt_generator(&prompt_generator_name);
            let Some(prompt_generator) = prompt_generator else {
                Global.Info(
                    "Unknown prompt generator",
                    &[zap::String("name", &prompt_generator_name)],
                );
                os_exit(1);
            };

            let case_manager =
                match testcase::open(format!("testdata/{prompt_generator_name}.json")) {
                    Ok(m) => Arc::new(m),
                    Err(err) => {
                        Global.Error("Failed to open test case", &[zap::Error(err)]);
                        os_exit(1);
                    }
                };

            let mut case_generator = generator::new(
                Arc::clone(&case_manager),
                (*generate_parallism.borrow()).max(0) as usize,
                openai_token.borrow().clone(),
                openai_base_url.borrow().clone(),
                openai_model.borrow().clone(),
                prompt_generator,
                *test_count.borrow(),
            );
            case_generator.run();
            case_generator.wait();

            if let Err(err) = case_manager.save() {
                Global.Error("Failed to save test case", &[zap::Error(err)]);
                os_exit(1);
            }
        });
    }

    generate_cmd
        .flags()
        .string_var(Rc::clone(&openai_token), "openai_token", "", "OpenAI token");
    generate_cmd.flags().string_var(
        Rc::clone(&openai_base_url),
        "openai_base_url",
        "",
        "OpenAI base URL",
    );
    generate_cmd
        .flags()
        .string_var(Rc::clone(&openai_model), "openai_model", "", "OpenAI model");
    generate_cmd.flags().string_var(
        Rc::clone(&prompt_generator_name),
        "prompt_generator",
        "",
        "Prompt generator",
    );
    generate_cmd
        .flags()
        .int_var(Rc::clone(&test_count), "test_count", 0, "Test count");
    generate_cmd.flags().int_var(
        Rc::clone(&generate_parallism),
        "parallel",
        20,
        "Generate parallism",
    );
    generate_cmd
}

/// createVerifyCmd — TiDB/MySQL A/B verification of recorded cases.
// `create_verify_cmd` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
// 这里的行为需要尽量贴近 Go 版本。
pub fn create_verify_cmd() -> cobra::Command {
    let prompt_generator_name = Rc::new(RefCell::new(String::new()));
    let tidb_dsn = Rc::new(RefCell::new(String::new()));
    let mysql_dsn = Rc::new(RefCell::new(String::new()));
    let recheck_passed = Rc::new(RefCell::new(false));

    let mut verify_cmd = cobra::Command::new("verify");
    verify_cmd.set_short("Verify something using TiDB and MySQL");

    {
        let prompt_generator_name = Rc::clone(&prompt_generator_name);
        let tidb_dsn = Rc::clone(&tidb_dsn);
        let mysql_dsn = Rc::clone(&mysql_dsn);
        let recheck_passed = Rc::clone(&recheck_passed);

        verify_cmd.set_run(move |_args| {
            ensure_logger();

            let prompt_generator_name = prompt_generator_name.borrow().clone();
            let case_manager =
                match testcase::open(format!("testdata/{prompt_generator_name}.json")) {
                    Ok(m) => m,
                    Err(err) => {
                        Global.Error("Failed to open test case", &[zap::Error(err)]);
                        os_exit(1);
                    }
                };

            let tidb = match sql::open("mysql", &unify_dsn(&tidb_dsn.borrow())) {
                Ok(db) => db,
                Err(err) => {
                    Global.Error("Failed to open TiDB", &[zap::Error(err)]);
                    os_exit(1);
                }
            };
            // Go: defer tidb.Close()
            let tidb_guard = CloseOnDrop(tidb);

            let mysql = match sql::open("mysql", &unify_dsn(&mysql_dsn.borrow())) {
                Ok(db) => db,
                Err(err) => {
                    Global.Error("Failed to open MySQL", &[zap::Error(err)]);
                    os_exit(1);
                }
            };

            case_manager.run_ab_test(tidb_guard.db(), mysql.inner(), *recheck_passed.borrow());
            if let Err(err) = case_manager.save() {
                Global.Error("Failed to save test case", &[zap::Error(err)]);
                os_exit(1);
            }
            // tidb_guard drops here → Close (Go defer)
        });
    }

    verify_cmd.flags().string_var(
        Rc::clone(&prompt_generator_name),
        "prompt_generator",
        "",
        "Prompt generator",
    );
    verify_cmd
        .flags()
        .string_var(Rc::clone(&tidb_dsn), "tidb_dsn", "", "TiDB DSN");
    verify_cmd
        .flags()
        .string_var(Rc::clone(&mysql_dsn), "mysql_dsn", "", "MySQL DSN");
    verify_cmd.flags().bool_var(
        Rc::clone(&recheck_passed),
        "recheck_passed",
        false,
        "Recheck passed cases",
    );

    verify_cmd
}

/// unifyDSN — parse MySQL DSN and force Collation=`utf8mb4_bin`.
///
/// On parse failure, logs and exits (Go `os.Exit(1)`).
// `unify_dsn` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
pub fn unify_dsn(dsn: &str) -> String {
    let mut cfg = match mysql::parse_dsn(dsn) {
        Ok(cfg) => cfg,
        Err(err) => {
            ensure_logger();
            Global.Error("Failed to parse DSN", &[zap::Error(err)]);
            os_exit(1);
        }
    };

    // TODO: allow to configure different collation
    cfg.collation = "utf8mb4_bin".to_string();

    cfg.format_dsn()
}

/// RAII stand-in for Go `defer tidb.Close()`.
// `CloseOnDrop` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct CloseOnDrop(sql::SqlDb);

// 这里实现 `CloseOnDrop` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl CloseOnDrop {
    // `db` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn db(&self) -> &astersql_tests_llmtest_testcase::Db {
        self.0.inner()
    }
}

// 这里实现 `Drop` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Drop for CloseOnDrop {
    // `drop` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Binary crate entry when `main.rs` is the bin root.
#[cfg(not(test))]
// `main` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn main() {
    run_main();
}
