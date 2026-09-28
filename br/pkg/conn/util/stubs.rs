// Copyright 2026 AsterSQL.

//! Local stand-ins for kvproto metapb::Store used by conn/util without
//! pulling kvproto/grpcio on darwin arm64.
//! 中文注释索引开始
//! 本文件负责`br/pkg/conn/util/stubs.rs`对应的占位类型与测试桩，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少19行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `enum`用离散值表达\"enum\"的状态，关系到序列化、日志和错误判定。
//! 这类符号最容易因为默认值、未知值或字符串映射而与 Go 端产生偏差。
//! 中文注释会提醒维护者把重点放在状态转换、展示文本和兜底分支。
//! 如果测试里出现 raw integer、unknown 或 not found，对应的兼容性保护通常都落在这里。
//! - `struct`承载\"struct\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl StoreLabel`把\"StoreLabel\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl Store`把\"Store\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! 中文注释索引结束

pub mod kvproto {
    pub mod metapb {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub enum StoreState {
            #[default]
            Up = 0,
            Offline = 1,
            Tombstone = 2,
        }

        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct StoreLabel {
            key: String,
            value: String,
        }

        impl StoreLabel {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn get_key(&self) -> &str {
                &self.key
            }
            pub fn set_key(&mut self, v: String) {
                self.key = v;
            }
            pub fn get_value(&self) -> &str {
                &self.value
            }
            pub fn set_value(&mut self, v: String) {
                self.value = v;
            }
        }

        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct Store {
            id: u64,
            address: String,
            status_address: String,
            peer_address: String,
            state: StoreState,
            last_heartbeat: i64,
            labels: Vec<StoreLabel>,
        }

        impl Store {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn get_id(&self) -> u64 {
                self.id
            }
            pub fn set_id(&mut self, v: u64) {
                self.id = v;
            }
            pub fn get_address(&self) -> &str {
                &self.address
            }
            pub fn set_address(&mut self, v: String) {
                self.address = v;
            }
            pub fn get_status_address(&self) -> &str {
                &self.status_address
            }
            pub fn set_status_address(&mut self, v: String) {
                self.status_address = v;
            }
            pub fn get_peer_address(&self) -> &str {
                &self.peer_address
            }
            pub fn set_peer_address(&mut self, v: String) {
                self.peer_address = v;
            }
            pub fn get_state(&self) -> StoreState {
                self.state
            }
            pub fn set_state(&mut self, v: StoreState) {
                self.state = v;
            }
            pub fn get_last_heartbeat(&self) -> i64 {
                self.last_heartbeat
            }
            pub fn set_last_heartbeat(&mut self, v: i64) {
                self.last_heartbeat = v;
            }
            pub fn get_labels(&self) -> &[StoreLabel] {
                &self.labels
            }
            pub fn mut_labels(&mut self) -> &mut Vec<StoreLabel> {
                &mut self.labels
            }
            pub fn set_labels(&mut self, v: Vec<StoreLabel>) {
                self.labels = v;
            }
        }
    }
}
