// Copyright 2026 AsterSQL.

// TopSQL reporter crate 根：汇聚采集、数据模型、上报通道与远程 reporter。
//
// 对应 Go `pkg/util/topsql/reporter`。导出 tipb protobuf、语句统计（stmtstats）、
// DataSink 注册、pubsub、RU（Request Unit）窗口聚合与远程上报入口；测试模块
// 通过 `#[path]`/`include!` 挂接各子文件。

#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]

extern crate self as topsql_reporter;

pub use topsql_collector as collector;
pub use topsql_state;
pub use topsql_state as topsqlstate;
pub extern crate tipb as tipb_protobuf;
pub use topsql_protocol::tipb;
pub use topsql_stmtstats as stmtstats;

pub use stmtstats::{
    BinaryDigest, RUIncrement, RUIncrementMap, RUKey, RUVersion, SQLPlanDigest, StatementStatsItem,
    StatementStatsMap,
};

/// SQL 规范化摘要（digest）解析入口。
pub mod parser {
    pub use topsql_parser::digester_impl::Digest;
}

/// 时间序列与规范化元数据模型；测试内嵌 `datamodel_test.rs`。
pub mod datamodel {
    use protobuf::Message;

    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/datamodel.rs"));

    #[cfg(test)]
    mod datamodel_test {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/datamodel_test.rs"));
    }
}
pub use datamodel::*;
#[path = "datasink.rs"]
pub mod datasink;
pub use datasink::*;
#[path = "pubsub.rs"]
pub mod pubsub;
pub use pubsub::*;

#[path = "report_ticker.rs"]
pub mod report_ticker;
#[path = "ru_datamodel.rs"]
mod ru_datamodel;
pub use ru_datamodel::*;
#[path = "ru_window_aggregator.rs"]
mod ru_window_aggregator;
pub use ru_window_aggregator::*;
#[path = "reporter.rs"]
pub mod reporter;
pub use reporter::{NewRemoteTopSQLReporter, RemoteTopSQLReporter, findKthNetworkBytes};
#[path = "single_target.rs"]
pub mod single_target;
pub use single_target::*;

#[cfg(test)]
#[path = "datamodel_1_aster_unit_test.rs"]
mod datamodel_1_aster_unit_test;
#[cfg(test)]
#[path = "reporter_2_aster_unit_test.rs"]
mod reporter_2_aster_unit_test;
#[cfg(test)]
#[path = "single_target_3_aster_unit_test.rs"]
mod single_target_3_aster_unit_test;

#[cfg(test)]
mod datasink_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod pubsub_test;
#[cfg(test)]
mod reporter_test;
#[cfg(test)]
mod ru_datamodel_test;
#[cfg(test)]
mod ru_window_aggregator_test;
#[cfg(test)]
mod single_target_test;
#[cfg(test)]
mod topru_case_runner_test;
#[cfg(test)]
mod topru_generated_cases_test;
