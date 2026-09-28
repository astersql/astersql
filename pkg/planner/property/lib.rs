// Copyright 2026 AsterSQL.

// 规划器物理/逻辑属性（property）crate 根。
//
// 汇总 Cascades/代价优化所需的逻辑属性、物理属性、统计信息与执行任务类型，
// 并桥接 expression、funcdep、collate 等依赖。物理属性描述排序、MPP 分区、
// 任务落点等；逻辑属性描述 Schema、FD、行数估计等与物理实现无关的输出特征。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

/// 表达式与 Schema 等类型再导出（打破 crate 循环依赖）。
pub mod expression {
    pub use expression_dependency::*;
}
/// 函数依赖（FD）集合再导出。
pub mod funcdep {
    pub use funcdep_dependency::*;
}
/// Cascades base 哈希/相等抽象再导出。
pub mod base {
    pub use base_dependency::*;
}
/// 编解码工具再导出（用于属性指纹 HashCode）。
pub mod codec {
    pub use codec_dependency::*;
}
/// 排序规则（collation）再导出，供 MPP 分区列比较使用。
pub mod collate {
    pub use collate_dependency::*;
}
/// 整数集合（FastIntSet）再导出，用于 FD 闭包等。
pub mod intset {
    pub use intset_dependency::*;
}
/// 会话变量类型再导出（统计缩放等需要）。
pub mod variable {
    pub use session_variable_dependency::session::SessionVars;
}
/// 内存占用估算常量再导出。
pub mod size {
    pub use size_dependency::*;
}

/// 逻辑属性：Schema、统计、FD 等。
mod logical_property;
/// 物理属性：排序项、任务类型、MPP 分区等。
mod physical_property;
/// 计划输出的统计摘要（行数、NDV）。
mod stats_info;
/// 执行任务类型（root/cop/mpp）。
mod task_type;

/// 再导出逻辑属性 API。
pub use logical_property::*;
/// 再导出物理属性 API。
pub use physical_property::*;
/// 再导出统计信息 API。
pub use stats_info::*;
/// 再导出任务类型常量与方法。
pub use task_type::*;

#[cfg(test)]
#[path = "physical_property_test.rs"]
/// 物理属性与交换器规则相关单元测试。
mod physical_property_test;
