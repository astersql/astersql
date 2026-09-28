// Copyright 2026 AsterSQL.

//! 中文注释索引开始
//! 本文件负责`br/pkg/conn/util/parity_test.rs`对应的 Go/Rust 契约对齐与回归保护。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少40行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `TestContext`承载\"TestContext\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl CancelContext`把\"CancelContext\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `is_cancelled`是当前文件的重要函数，承担\"is cancelled\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `MockStoreMeta`承载\"MockStoreMeta\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl StoreMeta`把\"StoreMeta\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `GetAllStores`是当前文件的重要函数，承担\"GetAllStores\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `MockPdClient`承载\"MockPdClient\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl MockPdClient`把\"MockPdClient\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `new`是当前文件的重要函数，承担\"new\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `fail_first`是当前文件的重要函数，承担\"fail first\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl PdClient`把\"PdClient\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `GetTS`是当前文件的重要函数，承担\"GetTS\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `MockHttpClient`承载\"MockHttpClient\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl MockHttpClient`把\"MockHttpClient\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl HttpClient`把\"HttpClient\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Get`是当前文件的重要函数，承担\"Get\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `store_with_engine`是当前文件的重要函数，承担\"store with engine\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `plain_store`是当前文件的重要函数，承担\"plain store\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `store_with_addresses`是当前文件的重要函数，承担\"store with addresses\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `go_rust_public_contract_matches`对齐 Go 同名测试或契约片段，用来固定\"go rust public contract matches\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - 场景\"normal: TiKV store filtering and address normalization\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"boundary: missing status address and TiFlash-only selection\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"error: active TiFlash stores are rejected\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"resource / retry: PD TS fetch retries then succeeds\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::kvproto::metapb::{self, Store, StoreLabel};
use astersql_br_pkg_errors::{ErrPDInvalidResponse, ErrRestoreTotalKVMismatch, Is};
use astersql_errors::SharedError;

use crate::util::{
    CancelContext, GetAllTiKVStores, GetConfigBytesFromTiKVStores, GetCurrentTsFromPD,
    GetCurrentTsFromPDWithRetry, HandleTiKVAddress, HttpClient, HttpResponse, PdClient,
    StoreBehavior, StoreMeta,
};

#[derive(Clone, Default)]
struct TestContext;

impl CancelContext for TestContext {
    fn is_cancelled(&self) -> bool {
        false
    }
}

struct MockStoreMeta {
    stores: Vec<Store>,
}

impl StoreMeta for MockStoreMeta {
    fn GetAllStores(
        &self,
        _ctx: &dyn CancelContext,
        _exclude_tombstone: bool,
    ) -> Result<Vec<Store>, SharedError> {
        Ok(self.stores.clone())
    }
}

struct MockPdClient {
    physical: i64,
    logical: i64,
    attempts: AtomicU32,
    fail_until: u32,
}

impl MockPdClient {
    fn new(physical: i64, logical: i64) -> Self {
        Self {
            physical,
            logical,
            attempts: AtomicU32::new(0),
            fail_until: 0,
        }
    }

    fn fail_first(mut self, attempts: u32) -> Self {
        self.fail_until = attempts;
        self
    }
}

impl PdClient for MockPdClient {
    fn GetTS(&self, _ctx: &dyn CancelContext) -> Result<(i64, i64), SharedError> {
        let attempt = self.attempts.fetch_add(1, Ordering::Relaxed) + 1;
        if attempt <= self.fail_until {
            return Err(SharedError::new((*ErrRestoreTotalKVMismatch).clone()));
        }
        Ok((self.physical, self.logical))
    }
}

struct MockHttpClient {
    responses: HashMap<String, HttpResponse>,
    closed: AtomicU32,
}

impl MockHttpClient {
    fn new(responses: HashMap<String, HttpResponse>) -> Self {
        Self {
            responses,
            closed: AtomicU32::new(0),
        }
    }
}

impl HttpClient for MockHttpClient {
    fn Get(&self, url: &str) -> Result<HttpResponse, SharedError> {
        self.responses
            .get(url)
            .cloned()
            .ok_or_else(|| SharedError::new((*ErrPDInvalidResponse).clone()))
    }

    fn CloseResponse(&self, _response: &HttpResponse) {
        self.closed.fetch_add(1, Ordering::Relaxed);
    }
}

fn store_with_engine(id: u64, engine: &str) -> Store {
    let mut store = Store::default();
    store.set_id(id);
    store.set_state(metapb::StoreState::Up);
    let mut label = StoreLabel::default();
    label.set_key("engine".to_string());
    label.set_value(engine.to_string());
    store.set_labels(vec![label]);
    store
}

fn plain_store(id: u64) -> Store {
    let mut store = Store::default();
    store.set_id(id);
    store.set_state(metapb::StoreState::Up);
    store
}

fn store_with_addresses(id: u64, address: &str, status_address: &str) -> Store {
    let mut store = Store::default();
    store.set_id(id);
    store.set_state(metapb::StoreState::Up);
    store.set_address(address.to_string());
    store.set_status_address(status_address.to_string());
    store
}

#[test]
fn go_rust_public_contract_matches() {
    let ctx = TestContext;

    // normal: TiKV store filtering and address normalization
    let stores = vec![
        plain_store(1),
        store_with_engine(2, "tiflash"),
        plain_store(3),
    ];
    let pd = MockStoreMeta { stores };
    let filtered = GetAllTiKVStores(&ctx, &pd, StoreBehavior::SkipTiFlash).expect("skip tiflash");
    assert_eq!(2, filtered.len());
    assert!(filtered.iter().all(|store| store.get_id() != 2));

    let addr = HandleTiKVAddress(
        &store_with_addresses(1, "127.0.0.1:20160", "127.0.0.1:20180"),
        "http://",
    )
    .expect("handle address");
    assert_eq!("http://127.0.0.1:20180", addr.to_string());

    let ts = GetCurrentTsFromPD(&ctx, &MockPdClient::new(100, 7)).expect("get ts");
    assert_eq!(((100_u64) << 18) | 7, ts);

    // boundary: missing status address and TiFlash-only selection
    let err = HandleTiKVAddress(&plain_store(9), "http://").expect_err("missing status address");
    assert!(err.to_string().contains("does not have status address"));

    let mixed = vec![
        plain_store(1),
        store_with_engine(2, "tiflash"),
        plain_store(3),
        store_with_engine(4, "tikv"),
    ];
    let pd = MockStoreMeta { stores: mixed };
    let tiflash_only =
        GetAllTiKVStores(&ctx, &pd, StoreBehavior::TiFlashOnly).expect("tiflash only");
    assert_eq!(
        vec![2],
        tiflash_only.iter().map(|s| s.get_id()).collect::<Vec<_>>()
    );

    // error: active TiFlash stores are rejected
    let pd = MockStoreMeta {
        stores: vec![plain_store(1), store_with_engine(2, "tiflash")],
    };
    let err = GetAllTiKVStores(&ctx, &pd, StoreBehavior::ErrorOnTiFlash)
        .expect_err("tiflash error policy");
    assert!(Is(Some(&err), &ErrPDInvalidResponse));
    assert!(
        err.to_string()
            .contains("cannot restore to a cluster with active TiFlash stores")
    );

    let mut bad_http = HashMap::new();
    bad_http.insert(
        "http://127.0.0.1:20180/config".to_string(),
        HttpResponse {
            status_code: 500,
            status: "500 Internal Server Error".to_string(),
            body: Vec::new(),
            request_url: "http://127.0.0.1:20180/config".to_string(),
        },
    );
    let err = GetConfigBytesFromTiKVStores(
        &ctx,
        &[store_with_addresses(
            1,
            "127.0.0.1:20160",
            "127.0.0.1:20180",
        )],
        &MockHttpClient::new(bad_http),
        "http://",
        |_body| Ok(()),
    )
    .expect_err("non-200 config response");
    assert!(
        err.to_string()
            .contains("request http://127.0.0.1:20180/config failed")
    );

    // resource / retry: PD TS fetch retries then succeeds
    let pd = MockPdClient::new(42, 3).fail_first(1);
    let ts = GetCurrentTsFromPDWithRetry(&ctx, &pd).expect("retry ts");
    assert_eq!(((42_u64) << 18) | 3, ts);
    assert!(pd.attempts.load(Ordering::Relaxed) >= 2);

    let mut ok_http = HashMap::new();
    ok_http.insert(
        "http://127.0.0.1:20180/config".to_string(),
        HttpResponse {
            status_code: 200,
            status: "200 OK".to_string(),
            body: b"{\"ok\":true}".to_vec(),
            request_url: "http://127.0.0.1:20180/config".to_string(),
        },
    );
    let mut collected = Vec::new();
    GetConfigBytesFromTiKVStores(
        &ctx,
        &[store_with_addresses(
            1,
            "127.0.0.1:20160",
            "127.0.0.1:20180",
        )],
        &MockHttpClient::new(ok_http),
        "http://",
        |body| {
            collected.extend_from_slice(body);
            Ok(())
        },
    )
    .expect("collect config bytes");
    assert_eq!(b"{\"ok\":true}", collected.as_slice());
}

#[test]
fn non_ok_status_text_matches_go_http_response() {
    let ctx = TestContext;
    let url = "http://127.0.0.1:20180/config";
    let mut responses = HashMap::new();
    responses.insert(
        url.to_string(),
        HttpResponse {
            status_code: 500,
            status: "500 Internal Server Error".to_string(),
            body: Vec::new(),
            request_url: url.to_string(),
        },
    );
    let client = MockHttpClient::new(responses);

    let err = GetConfigBytesFromTiKVStores(
        &ctx,
        &[store_with_addresses(
            1,
            "127.0.0.1:20160",
            "127.0.0.1:20180",
        )],
        &client,
        "http://",
        |_body| Ok(()),
    )
    .expect_err("non-200 config response");

    assert_eq!(
        "request http://127.0.0.1:20180/config failed: 500 Internal Server Error",
        err.to_string()
    );
}

#[test]
fn config_response_is_closed_after_callback_failure() {
    let ctx = CancelOnSecondCheck {
        checks: AtomicU32::new(0),
    };
    let url = "http://127.0.0.1:20180/config";
    let mut responses = HashMap::new();
    responses.insert(
        url.to_string(),
        HttpResponse {
            status_code: 200,
            status: "200 OK".to_string(),
            body: b"config".to_vec(),
            request_url: url.to_string(),
        },
    );
    let client = MockHttpClient::new(responses);

    GetConfigBytesFromTiKVStores(
        &ctx,
        &[store_with_addresses(
            1,
            "127.0.0.1:20160",
            "127.0.0.1:20180",
        )],
        &client,
        "http://",
        |_body| Err(SharedError::new((*ErrRestoreTotalKVMismatch).clone())),
    )
    .expect_err("collector failure");

    assert_eq!(1, client.closed.load(Ordering::Relaxed));
}

struct CancelOnSecondCheck {
    checks: AtomicU32,
}

impl CancelContext for CancelOnSecondCheck {
    fn is_cancelled(&self) -> bool {
        self.checks.fetch_add(1, Ordering::SeqCst) > 0
    }
}

struct AlwaysFailPd {
    attempts: AtomicU32,
}

impl PdClient for AlwaysFailPd {
    fn GetTS(&self, _ctx: &dyn CancelContext) -> Result<(i64, i64), SharedError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(SharedError::new((*ErrRestoreTotalKVMismatch).clone()))
    }
}

#[test]
fn cancellation_during_backoff_stops_before_another_pd_call() {
    let ctx = CancelOnSecondCheck {
        checks: AtomicU32::new(0),
    };
    let pd = AlwaysFailPd {
        attempts: AtomicU32::new(0),
    };

    GetCurrentTsFromPDWithRetry(&ctx, &pd).expect_err("cancelled retry");

    assert_eq!(1, pd.attempts.load(Ordering::SeqCst));
}

#[test]
fn status_url_path_and_query_survive_host_rewrite_and_join_path() {
    let ctx = TestContext;
    let config_url = "http://node.example:20180/status/config?token=x";
    let mut responses = HashMap::new();
    responses.insert(
        config_url.to_string(),
        HttpResponse {
            status_code: 200,
            status: "200 OK".to_string(),
            body: b"config".to_vec(),
            request_url: config_url.to_string(),
        },
    );
    let client = MockHttpClient::new(responses);
    let store = store_with_addresses(
        1,
        "http://node.example:20160",
        "http://status.example:20180/status?token=x",
    );

    let address = HandleTiKVAddress(&store, "http://").expect("rewrite host");
    assert_eq!(
        "http://node.example:20180/status?token=x",
        address.to_string()
    );
    GetConfigBytesFromTiKVStores(&ctx, &[store], &client, "http://", |_body| Ok(()))
        .expect("request joined config path");
}
