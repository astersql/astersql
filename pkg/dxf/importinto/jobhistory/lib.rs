// Copyright 2026 AsterSQL.

// IMPORT INTO 作业历史（jobhistory）crate 入口。
//
// 聚合任务存储、协议步骤常量与历史查询实现，供从
// `tidb_global_task_history` / `tidb_background_subtask_history` 读取已结束导入作业的汇总信息。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as astersql_dxf_importinto_jobhistory;

/// 再导出存储层上下文、错误与单元格值类型。
pub use storage_crate::{Context, Error, GoError, Value};

/// 错误注解工具（Annotatef）再导出。
pub mod errors {
    pub use storage_crate::errors::Annotatef;
}

/// Failpoint 注入：以约 1% 概率返回随机 DXF 重试错误，用于测试容错路径。
pub mod injectfailpoint {
    use crate::Error;

    /// 触发一次约 1% 概率的随机错误；成功则返回 Ok(())。
    pub fn DXFRandomErrorWithOnePercent() -> Result<(), Error> {
        injectfailpoint_crate::random_retry::DXFRandomErrorWithOnePercent()
            .map_err(|error| Error::new(error.to_string()))
    }
}

/// 分布式框架协议常量：IMPORT INTO 各步骤、任务状态与任务类型名。
pub mod proto {
    pub use proto_crate::step::{
        ImportStepCollectConflicts, ImportStepConflictResolution, ImportStepEncodeAndSort,
        ImportStepMergeSort, ImportStepPostProcess, ImportStepWriteAndIngest, Step,
    };
    pub use proto_crate::task::TaskStatePending;
    pub use proto_crate::r#type::ImportInto;
}

/// 任务管理器与“任务未找到”错误再导出。
pub mod storage {
    pub use storage_crate::{ErrTaskNotFound, TaskManager};
}

/// 按 keyspace 与 job id 构造全局任务键。
pub mod taskkey {
    pub use taskkey_crate::taskkey::ForJobInKeyspace;
}

/// 查询结果行类型（一行多列 Value）。
pub use storage_crate::chunk::Row;

/// 历史查询实现模块（history.rs）。
#[path = "history.rs"]
pub mod jobhistory;

pub use jobhistory::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "history_test.rs"]
mod history_test;
