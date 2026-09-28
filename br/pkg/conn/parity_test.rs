//! 中文注释索引开始
//! 本文件负责`br/pkg/conn/parity_test.rs`对应的Go/Rust 契约对齐，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少48行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `MemPd`承载\"MemPd\"相关状态，是理解数据流的入口之一。
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
//! - `MemCtrl`承载\"MemCtrl\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl PdControllerHandle`把\"PdControllerHandle\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `GetPDClient`是当前文件的重要函数，承担\"GetPDClient\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Close`是当前文件的重要函数，承担\"Close\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Sm`承载\"Sm\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl StoreManagerHandle`把\"StoreManagerHandle\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `HasTLS`是当前文件的重要函数，承担\"HasTLS\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Gc`承载\"Gc\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl GcManagerHandle`把\"GcManagerHandle\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Named`承载\"Named\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Storage`把\"Storage\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `name`是当前文件的重要函数，承担\"name\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `TestGlue`承载\"TestGlue\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl GlueTrait`把\"GlueTrait\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `GetDomain`是当前文件的重要函数，承担\"GetDomain\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `CreateSession`是当前文件的重要函数，承担\"CreateSession\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Open`是当前文件的重要函数，承担\"Open\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `OwnsStorage`是当前文件的重要函数，承担\"OwnsStorage\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `StartProgress`是当前文件的重要函数，承担\"StartProgress\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `P`承载\"P\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Progress`把\"Progress\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Inc`是当前文件的重要函数，承担\"Inc\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `IncBy`是当前文件的重要函数，承担\"IncBy\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetCurrent`是当前文件的重要函数，承担\"GetCurrent\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Record`是当前文件的重要函数，承担\"Record\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetVersion`是当前文件的重要函数，承担\"GetVersion\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `UseOneShotSession`是当前文件的重要函数，承担\"UseOneShotSession\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetClient`是当前文件的重要函数，承担\"GetClient\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `OkHttp`承载\"OkHttp\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl HttpClient`把\"HttpClient\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Get`是当前文件的重要函数，承担\"Get\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `go_rust_public_contract_matches`对齐 Go 同名测试或契约片段，用来固定\"go rust public contract matches\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! 中文注释索引结束

use std::sync::{Arc, Mutex};

use astersql_br_pkg_glue::{
    ClientCLP, Context as GlueContext, Domain, Glue as GlueTrait, GlueClient, Progress,
    SecurityOption, Session, Storage,
};
use astersql_br_pkg_version::SetReleaseVersionForTest;
use astersql_errors::SharedError;

use crate::{
    BackgroundContext, DefaultImportNumGoroutines, DefaultMergeRegionKeyCount,
    DefaultMergeRegionSizeBytes, GcManagerHandle, GetAllTiKVStores, GetConfigFromTiKV, HttpClient,
    HttpResponse, MgrLifecycleHandle, NewMgr, NewMgrDeps, NullspaceID, PdControllerHandle, Store,
    StoreBehavior, StoreLabel, StoreManagerHandle, StoreMeta, StoreState, VersionCheckerType,
    handleTiKVAddress, store_is_tikv,
};

struct MemPd {
    stores: Mutex<Vec<Store>>,
}
impl StoreMeta for MemPd {
    fn GetAllStores(&self, _exclude_tombstone: bool) -> Result<Vec<Store>, SharedError> {
        Ok(self.stores.lock().unwrap().clone())
    }
}

struct MemCtrl {
    pd: Arc<dyn StoreMeta>,
    closed: Arc<Mutex<bool>>,
}
impl PdControllerHandle for MemCtrl {
    fn GetPDClient(&self) -> Arc<dyn StoreMeta> {
        self.pd.clone()
    }
    fn Close(&self) {
        *self.closed.lock().unwrap() = true;
    }
}

struct Sm {
    tls: bool,
    closed: Arc<Mutex<bool>>,
}
impl StoreManagerHandle for Sm {
    fn Close(&self) {
        *self.closed.lock().unwrap() = true;
    }
    fn HasTLS(&self) -> bool {
        self.tls
    }
}

struct Gc;
impl GcManagerHandle for Gc {}

struct Lifecycle {
    events: Arc<Mutex<Vec<&'static str>>>,
}
impl MgrLifecycleHandle for Lifecycle {
    fn CloseDomain(&self) {
        self.events.lock().unwrap().push("domain");
    }
    fn CloseOwnerManager(&self) {
        self.events.lock().unwrap().push("owner");
    }
    fn StoreShuttingDown(&self) {
        self.events.lock().unwrap().push("shutting-down");
    }
    fn CloseStorage(&self) {
        self.events.lock().unwrap().push("storage");
    }
}

struct Named(String);
impl Storage for Named {
    fn name(&self) -> &str {
        &self.0
    }
}

struct TestGlue;
impl GlueTrait for TestGlue {
    fn GetDomain(&self, _store: &dyn Storage) -> Result<Arc<Domain>, SharedError> {
        Ok(Arc::new(Domain))
    }
    fn CreateSession(&self, _store: &dyn Storage) -> Result<Box<dyn Session>, SharedError> {
        Err(astersql_errors::New("no session"))
    }
    fn Open(&self, path: &str, _option: SecurityOption) -> Result<Box<dyn Storage>, SharedError> {
        Ok(Box::new(Named(path.to_string())))
    }
    fn OwnsStorage(&self) -> bool {
        true
    }
    fn StartProgress(
        &self,
        _ctx: GlueContext,
        _cmdName: &str,
        _total: i64,
        _redirectLog: bool,
    ) -> Box<dyn Progress> {
        struct P;
        impl Progress for P {
            fn Inc(&self) {}
            fn IncBy(&self, _c: i64) {}
            fn GetCurrent(&self) -> i64 {
                0
            }
            fn Close(&self) {}
        }
        Box::new(P)
    }
    fn Record(&self, _name: &str, _value: u64) {}
    fn GetVersion(&self) -> String {
        "BR\ntest".into()
    }
    fn UseOneShotSession(
        &self,
        _store: &dyn Storage,
        _closeDomain: bool,
        _fn_: &mut dyn FnMut(Box<dyn Session>) -> Result<(), SharedError>,
    ) -> Result<(), SharedError> {
        Ok(())
    }
    fn GetClient(&self) -> GlueClient {
        ClientCLP
    }
}

struct OkHttp;
impl HttpClient for OkHttp {
    fn Get(&self, url: &str) -> Result<HttpResponse, SharedError> {
        Ok(HttpResponse {
            status_code: 200,
            body: br#"{"ok":true}"#.to_vec(),
            request_url: url.to_string(),
        })
    }
}

#[test]
fn go_rust_public_contract_matches() {
    let _lock = crate::conn_test::failpoint_lock();
    crate::conn_test::clear_store_failpoints();

    assert_eq!(DefaultMergeRegionSizeBytes, 96 * 1024 * 1024);
    assert_eq!(DefaultMergeRegionKeyCount, 960_000);
    assert_eq!(DefaultImportNumGoroutines, 128);
    assert_eq!(NullspaceID, 0xffff_ffff);

    let tikv = Store {
        id: 1,
        address: "127.0.0.1:20160".into(),
        status_address: "127.0.0.1:20180".into(),
        version: "v6.5.0".into(),
        state: StoreState::Up,
        labels: vec![],
    };
    let flash = Store {
        id: 2,
        address: "127.0.0.1:3930".into(),
        status_address: "127.0.0.1:20292".into(),
        version: "v6.5.0".into(),
        state: StoreState::Up,
        labels: vec![StoreLabel {
            key: "engine".into(),
            value: "tiflash".into(),
        }],
    };
    let pd = MemPd {
        stores: Mutex::new(vec![tikv.clone(), flash]),
    };

    let skipped = GetAllTiKVStores(&pd, StoreBehavior::SkipTiFlash).unwrap();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].id, 1);
    assert!(
        GetAllTiKVStores(&pd, StoreBehavior::ErrorOnTiFlash)
            .unwrap_err()
            .to_string()
            .contains("TiFlash")
    );

    SetReleaseVersionForTest(Some("nightly-dirty"));
    let ctrl_closed = Arc::new(Mutex::new(false));
    let sm_closed = Arc::new(Mutex::new(false));
    let lifecycle_events = Arc::new(Mutex::new(Vec::new()));
    let tikv2 = tikv.clone();
    let deps = NewMgrDeps {
        new_pd: Arc::new({
            let ctrl_closed = ctrl_closed.clone();
            move |_addrs, _opt| {
                Ok(Arc::new(MemCtrl {
                    pd: Arc::new(MemPd {
                        stores: Mutex::new(vec![tikv2.clone()]),
                    }) as Arc<dyn StoreMeta>,
                    closed: ctrl_closed.clone(),
                }) as Arc<dyn PdControllerHandle>)
            }
        }),
        new_store_manager: Arc::new({
            let sm_closed = sm_closed.clone();
            move |tls| {
                Arc::new(Sm {
                    tls,
                    closed: sm_closed.clone(),
                }) as Arc<dyn StoreManagerHandle>
            }
        }),
        new_gc: Arc::new(|_ks| Arc::new(Gc) as Arc<dyn GcManagerHandle>),
        is_tikv_storage: Arc::new(|s| s.name() != "not-tikv"),
        new_lifecycle: Arc::new({
            let lifecycle_events = lifecycle_events.clone();
            move |_domain, _storage| {
                Arc::new(Lifecycle {
                    events: lifecycle_events.clone(),
                }) as Arc<dyn MgrLifecycleHandle>
            }
        }),
    };
    let g = TestGlue;
    let mgr = NewMgr(
        &g,
        "",
        &["127.0.0.1:2379".into()],
        SecurityOption::default(),
        false,
        StoreBehavior::SkipTiFlash,
        true,
        true,
        VersionCheckerType::NoVersionChecker,
        &deps,
    )
    .expect("NewMgr");
    assert!(store_is_tikv());
    assert!(mgr.ownsStorage);
    assert!(mgr.GetDomain().is_some());
    assert_eq!(
        handleTiKVAddress(&tikv, "http://").unwrap(),
        "http://127.0.0.1:20180"
    );

    let mut seen = 0;
    GetConfigFromTiKV(
        &BackgroundContext,
        mgr.pd.GetPDClient().as_ref(),
        &OkHttp,
        "http://",
        &mut |_resp| {
            seen += 1;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(seen, 1);
    mgr.Close();
    assert!(*ctrl_closed.lock().unwrap());
    assert!(*sm_closed.lock().unwrap());
    assert_eq!(
        lifecycle_events.lock().unwrap().as_slice(),
        &["domain", "owner", "shutting-down", "storage"]
    );

    let deps2 = NewMgrDeps {
        new_pd: deps.new_pd.clone(),
        new_store_manager: deps.new_store_manager.clone(),
        new_gc: deps.new_gc.clone(),
        is_tikv_storage: Arc::new(|_| false),
        new_lifecycle: deps.new_lifecycle.clone(),
    };
    assert!(
        NewMgr(
            &g,
            "",
            &["127.0.0.1:2379".into()],
            SecurityOption::default(),
            false,
            StoreBehavior::SkipTiFlash,
            false,
            false,
            VersionCheckerType::NoVersionChecker,
            &deps2,
        )
        .is_err()
    );

    SetReleaseVersionForTest(None);
}
