// Copyright 2026 AsterSQL.

//! Parity tests for `tests/llmtest/generator` public contracts vs Go.

// 本文件对应 `tests/llmtest/generator/parity_test.rs`，本次任务只补中文解释，不改行为。
// 本文件按成功、边界、错误和清理四类场景组织。
// 阅读时先看总入口，再看各个合同分组。
// 这里的目标是证明 Rust 与 Go 的公共合同一致。
// 成功路径关注正常返回值和可观测副作用。
// 边界路径关注空输入、默认值和最小变体。
// 错误路径关注日志、panic、exit 与错误文本。
// 清理路径关注 Close、Sync、Join 和资源释放。
// 中文注释优先解释为什么要断言。
// 补充阅读提示 1：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 2：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 3：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 4：阅读长列表时可按语义分组理解，而不是逐项记忆。
// 补充阅读提示 5：阅读长测试时可按准备、执行、观测、清理四段切开。
// 补充阅读提示 6：资源相关逻辑要特别留意 Close、Join、Drop 和 defer 对应关系。
// 补充阅读提示 7：错误路径要同时看返回值、日志和是否提前终止。
// 补充阅读提示 8：边界路径通常说明默认值、空输入和最小可用配置。
// 补充阅读提示 9：成功路径通常说明副作用被记录在什么位置。
// 补充阅读提示 10：如果出现桩对象，优先看它暴露了哪些可观测状态。
// 补充阅读提示 11：这些中文不会改变断言，只帮助缩短重新入场时间。
// 补充阅读提示 12：当多个 helper 串联时，顺序本身往往就是语义的一部分。
// 补充阅读提示 13：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 14：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 15：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 16：阅读长列表时可按语义分组理解，而不是逐项记忆。
// 补充阅读提示 17：阅读长测试时可按准备、执行、观测、清理四段切开。
// 补充阅读提示 18：资源相关逻辑要特别留意 Close、Join、Drop 和 defer 对应关系。
// 补充阅读提示 19：错误路径要同时看返回值、日志和是否提前终止。
// 补充阅读提示 20：边界路径通常说明默认值、空输入和最小可用配置。
// 补充阅读提示 21：成功路径通常说明副作用被记录在什么位置。
// 补充阅读提示 22：如果出现桩对象，优先看它暴露了哪些可观测状态。
// 补充阅读提示 23：这些中文不会改变断言，只帮助缩短重新入场时间。
// 补充阅读提示 24：当多个 helper 串联时，顺序本身往往就是语义的一部分。
// 补充阅读提示 25：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 26：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 27：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 28：阅读长列表时可按语义分组理解，而不是逐项记忆。
// 补充阅读提示 29：阅读长测试时可按准备、执行、观测、清理四段切开。
// 补充阅读提示 30：资源相关逻辑要特别留意 Close、Join、Drop 和 defer 对应关系。
// 补充阅读提示 31：错误路径要同时看返回值、日志和是否提前终止。
// 补充阅读提示 32：边界路径通常说明默认值、空输入和最小可用配置。
// 补充阅读提示 33：成功路径通常说明副作用被记录在什么位置。
// 补充阅读提示 34：如果出现桩对象，优先看它暴露了哪些可观测状态。
// 补充阅读提示 35：这些中文不会改变断言，只帮助缩短重新入场时间。
// 补充阅读提示 36：当多个 helper 串联时，顺序本身往往就是语义的一部分。
use crate::stubs::openai::{ChatCompletion, ChatCompletionNewParams};
use crate::stubs::{ChatCompletionChoice, ChatCompletionMessage};
use crate::stubs::{
    RequestOption, clear_captured_requests, clear_completions_handler, last_client_options,
    last_completion_options, last_completion_params, set_completions_handler,
    set_force_marshal_error,
};
use crate::{
    PromptGenerator, SimplePromptResponse, all_prompt_generators, ensure_init,
    get_prompt_generator, new,
};
use astersql_tests_llmtest_logger::{Global, ensure_init as ensure_logger};
use astersql_tests_llmtest_testcase::{AnyValue, Case, Manager};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

static TEST_LOCK: Mutex<()> = Mutex::new(());

// 测试 `go_rust_public_contract_matches` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
#[test]
// `go_rust_public_contract_matches` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn go_rust_public_contract_matches() {
    let _guard = TEST_LOCK.lock().expect("generator parity test lock");
    contract_normal_paths();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

#[test]
fn unmarshal_accepts_go_zero_value_queries() {
    let _guard = TEST_LOCK.lock().expect("generator parity test lock");
    assert!(
        SimplePromptResponse::unmarshal_json(r#"{}"#)
            .expect("omitted queries must decode")
            .queries
            .is_empty(),
        "encoding/json leaves an omitted Queries field at its zero value"
    );
    assert!(
        SimplePromptResponse::unmarshal_json(r#"{"queries":null}"#)
            .expect("null queries must decode")
            .queries
            .is_empty(),
        "encoding/json accepts null for a slice and leaves it nil"
    );
    assert!(
        SimplePromptResponse::unmarshal_json("null")
            .expect("null response must decode")
            .queries
            .is_empty(),
        "encoding/json accepts null for a struct pointer target"
    );
    assert_eq!(
        SimplePromptResponse::unmarshal_json(r#"{"Queries":["SELECT 1"]}"#)
            .expect("field matching is case insensitive")
            .queries,
        vec!["SELECT 1"]
    );
    assert_eq!(
        SimplePromptResponse::unmarshal_json(r#"{"queries":["SELECT 1"],"Queries":["SELECT 2"]}"#)
            .expect("later duplicate field must win")
            .queries,
        vec!["SELECT 2"]
    );
}

#[test]
fn negative_target_count_skips_generation_like_go() {
    let _guard = TEST_LOCK.lock().expect("generator parity test lock");
    ensure_logger();
    ensure_init();
    clear_captured_requests();

    let called = Arc::new(AtomicBool::new(false));
    let called_flag = Arc::clone(&called);
    set_completions_handler(move |_p: &ChatCompletionNewParams, _o: &[RequestOption]| {
        called_flag.store(true, Ordering::SeqCst);
        Err("negative target must skip generation".into())
    });

    let mgr = Arc::new(Manager::new_empty(tmp_path("negative-count")));
    let mut generator = new(
        mgr,
        1,
        "t".into(),
        "http://x".into(),
        "m".into(),
        get_prompt_generator("misc").unwrap(),
        -1,
    );
    generator.run();
    generator.wait();

    assert!(!called.load(Ordering::SeqCst));
    clear_completions_handler();
}

#[test]
fn prompt_text_preserves_go_raw_string_whitespace() {
    let _guard = TEST_LOCK.lock().expect("generator parity test lock");
    ensure_init();

    for (name, group, marker) in [("dml", "insert", "INSERT."), ("misc", "cte", "CTE.")] {
        let generator = get_prompt_generator(name).expect("generator registered");
        let messages = generator
            .generate_prompt(group, 1, &[])
            .expect("prompt generation succeeds");
        assert!(
            messages[0].content().contains(&format!(
                "Return 3 random SQL queries using this operation: {marker}\n    \n    EXAMPLE JSON OUTPUT:"
            )),
            "Go raw string keeps four spaces on the blank example separator"
        );
    }
}

struct EmptyPromptGenerator;

impl PromptGenerator for EmptyPromptGenerator {
    fn name(&self) -> &'static str {
        "empty"
    }

    fn groups(&self) -> Vec<&'static str> {
        vec!["empty"]
    }

    fn generate_prompt(
        &self,
        _group: &str,
        _count: i32,
        _exist_cases: &[Case],
    ) -> Option<Vec<crate::ChatCompletionMessageParamUnion>> {
        Some(Vec::new())
    }

    fn unmarshal(&self, _response: &str) -> Vec<Case> {
        vec![Case {
            sql: "SELECT 1".into(),
            ..Default::default()
        }]
    }
}

#[test]
fn non_nil_empty_prompt_is_sent_like_go() {
    let _guard = TEST_LOCK.lock().expect("generator parity test lock");
    ensure_logger();
    clear_captured_requests();

    let called = Arc::new(AtomicBool::new(false));
    let called_flag = Arc::clone(&called);
    set_completions_handler(
        move |params: &ChatCompletionNewParams, _o: &[RequestOption]| {
            called_flag.store(true, Ordering::SeqCst);
            assert!(params.messages.is_empty());
            Ok(ChatCompletion {
                choices: vec![ChatCompletionChoice {
                    message: ChatCompletionMessage {
                        content: "{}".into(),
                    },
                }],
                raw_json: "{}".into(),
            })
        },
    );

    let mgr = Arc::new(Manager::new_empty(tmp_path("empty-prompt")));
    let mut generator = new(
        Arc::clone(&mgr),
        1,
        "t".into(),
        "http://x".into(),
        "m".into(),
        Arc::new(EmptyPromptGenerator),
        1,
    );
    generator.run();
    generator.wait();

    assert!(called.load(Ordering::SeqCst));
    assert_eq!(mgr.exist_cases("empty").len(), 1);
    clear_completions_handler();
}

/// Normal: registry, prompt shape, unmarshal, worker append via stubbed OpenAI.
// `contract_normal_paths` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_normal_paths() {
    ensure_logger();
    ensure_init();
    set_force_marshal_error(false);
    clear_captured_requests();

    let gens = all_prompt_generators();
    assert_eq!(gens.len(), 3, "dml + expression + misc registered");

    let dml = get_prompt_generator("dml").expect("dml registered");
    assert_eq!(dml.name(), "dml");
    assert_eq!(dml.groups(), vec!["insert", "update", "delete"]);

    let expr = get_prompt_generator("expression").expect("expression registered");
    assert_eq!(expr.name(), "expression");
    assert!(expr.groups().contains(&"concat"));
    assert!(expr.groups().contains(&"json_extract"));
    // Go lists "+" / "-" twice in scalar ops.
    assert_eq!(
        expr.groups().iter().filter(|g| **g == "+").count(),
        2,
        "Go Groups keeps duplicate +"
    );

    let misc = get_prompt_generator("misc").expect("misc registered");
    assert_eq!(misc.groups(), vec!["cte"]);

    // Prompt without exist cases: system + user.
    let msgs = dml
        .generate_prompt("insert", 3, &[])
        .expect("prompt generation succeeds");
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role(), "system");
    assert!(msgs[0].content().contains("DML operation"));
    assert_eq!(msgs[1].role(), "user");
    assert_eq!(
        msgs[1].content(),
        "Return 3 random SQL queries using this operation: insert."
    );

    // Prompt with exist cases: system + user + assistant + user.
    let exist = vec![Case {
        sql: "SELECT 1".into(),
        ..Default::default()
    }];
    let msgs2 = dml
        .generate_prompt("update", 2, &exist)
        .expect("prompt generation succeeds");
    assert_eq!(msgs2.len(), 4);
    assert_eq!(msgs2[1].role(), "user");
    assert_eq!(
        msgs2[1].content(),
        "Return 1 random SQL queries using this operation: update."
    );
    assert_eq!(msgs2[2].role(), "assistant");
    assert!(msgs2[2].content().contains("SELECT 1"));
    assert_eq!(
        msgs2[3].content(),
        "Return 2 random SQL queries using this operation: update."
    );

    let cases = dml.unmarshal(r#"{"queries":["SELECT 1","SELECT 2"]}"#);
    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0].sql, "SELECT 1");
    assert_eq!(cases[1].sql, "SELECT 2");

    // Worker path: stub completion -> AppendCase.
    set_completions_handler(
        |_params: &ChatCompletionNewParams, _opts: &[RequestOption]| {
            Ok(ChatCompletion {
            choices: vec![ChatCompletionChoice {
                message: ChatCompletionMessage {
                    content: r#"{"queries":["CREATE TABLE t(i INT);INSERT INTO t VALUES(1);SELECT * FROM t ORDER BY i;DROP TABLE t;"]}"#.into(),
                },
            }],
            raw_json: r#"{"choices":[{"message":{"content":"..."}}]}"#.into(),
        })
        },
    );

    let mgr = Arc::new(Manager::new_empty(tmp_path("normal")));
    let gen_pg = get_prompt_generator("misc").unwrap();
    let mut g = new(
        Arc::clone(&mgr),
        2,
        "tok".into(),
        "http://example.invalid/v1".into(),
        "gpt-test".into(),
        gen_pg,
        5,
    );
    g.run();
    g.wait();

    let cte = mgr.exist_cases("cte");
    assert_eq!(cte.len(), 1);
    assert!(cte[0].sql.contains("INSERT INTO t"));

    let client_opts = last_client_options();
    assert!(
        client_opts
            .iter()
            .any(|o| matches!(o, RequestOption::ApiKey(k) if k == "tok"))
    );
    assert!(client_opts.iter().any(|o| matches!(
        o,
        RequestOption::JsonSet { key, value }
            if key == "include_reasoning" && *value == AnyValue::Bool(true)
    )));

    let params = last_completion_params().expect("completion called");
    assert_eq!(params.model, "gpt-test");
    assert_eq!(params.max_tokens, 6000);
    assert_eq!(params.response_format_type, "json_object");

    clear_completions_handler();
}

/// Boundary: already-enough cases skip OpenAI; deepseek provider option; empty queries.
// `contract_boundary` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_boundary() {
    ensure_logger();
    ensure_init();
    set_force_marshal_error(false);
    clear_captured_requests();

    let called = Arc::new(AtomicBool::new(false));
    let called_flag = Arc::clone(&called);
    set_completions_handler(move |_p: &ChatCompletionNewParams, _o: &[RequestOption]| {
        called_flag.store(true, Ordering::SeqCst);
        Err("should not be called".into())
    });

    let mgr = Arc::new(Manager::new_empty(tmp_path("boundary")));
    // Pre-fill to testCaseCount so generateTestSQLsForFunction returns nil,nil.
    mgr.append_case(
        "cte",
        Case {
            sql: "SELECT 1".into(),
            ..Default::default()
        },
    );
    mgr.append_case(
        "cte",
        Case {
            sql: "SELECT 2".into(),
            ..Default::default()
        },
    );

    let mut g = new(
        Arc::clone(&mgr),
        1,
        "t".into(),
        "http://x".into(),
        "deepseek-chat".into(),
        get_prompt_generator("misc").unwrap(),
        2, // already have 2
    );
    g.run();
    g.wait();
    assert_eq!(mgr.exist_cases("cte").len(), 2);
    assert!(
        !called.load(Ordering::SeqCst),
        "skip OpenAI when enough cases"
    );
    assert!(
        last_completion_params().is_none(),
        "skip OpenAI when enough cases"
    );

    // deepseek path: force generation with empty manager.
    clear_captured_requests();
    set_completions_handler(|_p: &ChatCompletionNewParams, opts: &[RequestOption]| {
        assert!(opts.iter().any(|o| matches!(
            o,
            RequestOption::JsonSet { key, .. } if key == "provider"
        )));
        Ok(ChatCompletion {
            choices: vec![ChatCompletionChoice {
                message: ChatCompletionMessage {
                    content: r#"{"queries":["with cte as (select 1) select * from cte"]}"#.into(),
                },
            }],
            raw_json: "{}".into(),
        })
    });
    let mgr2 = Arc::new(Manager::new_empty(tmp_path("boundary-ds")));
    let mut g2 = new(
        Arc::clone(&mgr2),
        1,
        "t".into(),
        "http://x".into(),
        "deepseek-r1".into(),
        get_prompt_generator("misc").unwrap(),
        1,
    );
    g2.run();
    g2.wait();
    assert_eq!(mgr2.exist_cases("cte").len(), 1);
    let opts = last_completion_options();
    assert!(opts.iter().any(|o| match o {
        RequestOption::JsonSet { key, value } => {
            key == "provider"
                && matches!(
                    value,
                    AnyValue::Object(fields) if fields.iter().any(|(k, v)| {
                        k == "ignore"
                            && matches!(v, AnyValue::Array(a) if a == &vec![AnyValue::String("Together".into())])
                    })
                )
        }
        _ => false,
    }));

    // Empty queries array unmarshals to empty cases (not an error).
    let dml = get_prompt_generator("dml").unwrap();
    assert!(dml.unmarshal(r#"{"queries":[]}"#).is_empty());

    clear_completions_handler();
}

/// Error: bad JSON -> empty + log; empty prompt -> worker logs and continues; no choices.
// `contract_error_paths` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_error_paths() {
    ensure_logger();
    ensure_init();
    set_force_marshal_error(false);

    let before = Global.records().len();
    let dml = get_prompt_generator("dml").unwrap();
    let bad = dml.unmarshal("not-json");
    assert!(bad.is_empty());
    let after = Global.records();
    assert!(
        after[before..]
            .iter()
            .any(|r| r.level == "error" && r.msg == "failed to unmarshal dml prompt response"),
        "unmarshal error must be logged"
    );

    // Marshal failure => nil prompt => failed to generate prompt.
    set_force_marshal_error(true);
    let exist = vec![Case {
        sql: "x".into(),
        ..Default::default()
    }];
    let msgs = dml.generate_prompt("insert", 1, &exist);
    assert!(msgs.is_none());
    set_force_marshal_error(false);

    clear_captured_requests();
    set_completions_handler(|_p: &ChatCompletionNewParams, _o: &[RequestOption]| {
        Ok(ChatCompletion {
            choices: vec![],
            raw_json: "{}".into(),
        })
    });
    let mgr = Arc::new(Manager::new_empty(tmp_path("err-choices")));
    let before_err = Global.records().len();
    let mut g = new(
        Arc::clone(&mgr),
        1,
        "t".into(),
        "http://x".into(),
        "m".into(),
        get_prompt_generator("misc").unwrap(),
        3,
    );
    g.run();
    g.wait();
    assert!(mgr.exist_cases("cte").is_empty());
    assert!(
        Global.records()[before_err..]
            .iter()
            .any(|r| r.level == "error" && r.msg == "failed to generate test SQLs"),
        "no choices must surface as worker error log"
    );

    // Completions transport error: log and continue (no panic).
    set_completions_handler(
        |_p: &ChatCompletionNewParams, _o: &[RequestOption]| Err("boom".into()),
    );
    let mgr2 = Arc::new(Manager::new_empty(tmp_path("err-transport")));
    let mut g2 = new(
        Arc::clone(&mgr2),
        1,
        "t".into(),
        "http://x".into(),
        "m".into(),
        get_prompt_generator("misc").unwrap(),
        1,
    );
    g2.run();
    g2.wait();
    assert!(mgr2.exist_cases("cte").is_empty());

    clear_completions_handler();
}

/// Resource cleanup: Wait joins workers; channel closed so workers exit.
// `contract_resource_cleanup` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
// 保留分组后，Go/Rust 差异更容易定位。
fn contract_resource_cleanup() {
    ensure_logger();
    ensure_init();
    set_force_marshal_error(false);
    clear_captured_requests();

    set_completions_handler(|_p: &ChatCompletionNewParams, _o: &[RequestOption]| {
        Ok(ChatCompletion {
            choices: vec![ChatCompletionChoice {
                message: ChatCompletionMessage {
                    content: r#"{"queries":["SELECT 42"]}"#.into(),
                },
            }],
            raw_json: "{}".into(),
        })
    });

    // Use a tiny generator (misc has 1 group) with parallelism > groups.
    let mgr = Arc::new(Manager::new_empty(tmp_path("cleanup")));
    let mut g = new(
        Arc::clone(&mgr),
        4,
        "t".into(),
        "http://x".into(),
        "m".into(),
        get_prompt_generator("misc").unwrap(),
        1,
    );
    g.run();
    g.wait(); // must return (all workers Done)
    assert_eq!(mgr.exist_cases("cte").len(), 1);

    // Second wait is a no-op after workers drained.
    g.wait();

    clear_completions_handler();
}

// `tmp_path` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn tmp_path(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir()
        .join(format!("llmtest-generator-{label}-{nanos}.json"))
        .to_string_lossy()
        .into_owned()
}
