// Copyright 2026 AsterSQL.

// 排序执行器（sort executor）包入口：导出 Sort / TopN 及 spill（落盘）相关子模块。
//
// SortExec 对输入行按排序键（SortKey）全排序；TopNExec 在 Limit 约束下保留前 N 行。
// 内存不足时可 spill 到临时存储，再经多路归并（multi-way merge）产出有序结果。

#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

pub mod multi_way_merge;
pub mod parallel_sort_spill_helper;
pub mod parallel_sort_worker;
pub mod sort;
pub mod sort_partition;
pub mod sort_spill;
pub mod sort_util;
pub mod topn;
pub mod topn_chunk_heap;
pub mod topn_spill;
pub mod topn_worker;

pub use sort::{RowSource, SortExec};
pub use sort_util::{DataChunk, Row, SortError, SortKey, SortValue};
pub use topn::{Limit, RankInfo, TopNExec};

#[cfg(test)]
mod benchmark_test;
#[cfg(test)]
mod multi_way_merge_test;
#[cfg(test)]
mod parallel_sort_spill_helper_test;
#[cfg(test)]
mod parallel_sort_spill_test;
#[cfg(test)]
mod parallel_sort_test;
#[cfg(test)]
mod parallel_sort_worker_test;
#[cfg(test)]
mod rank_topn_test;
#[cfg(test)]
mod sort_partition_test;
#[cfg(test)]
mod sort_spill_test;
#[cfg(test)]
mod sort_test;
#[cfg(test)]
mod sortexec_pkg_test;
#[cfg(test)]
mod topn_spill_test;
