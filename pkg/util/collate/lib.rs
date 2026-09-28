// Copyright 2026 AsterSQL.

// 字符集排序规则（collation）crate 入口。
//
// 对应 Go `pkg/util/collate`：汇集 Collator 实现、字符集切换、UCA 权重表与各编码
// （bin / GBK / GB18030 / general_ci / unicode / 拼音）子模块，并对外 re-export。
// Collation 决定字符串比较与排序 key 的生成方式，直接影响索引与 ORDER BY 语义。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    ambiguous_glob_reexports
)]

extern crate self as util_collate;

pub use parser_charset::{charset, mysql};

// 字符集名 ↔ 默认 collation 的切换逻辑（对齐 Go charset 辅助包）。
#[path = "charset.rs"]
pub mod charset_switch;
// 字符串工具：尾部空格裁剪等，供各 Collator 复用。
#[path = "../stringutil/string_util.rs"]
pub mod stringutil;
// UCA（Unicode Collation Algorithm）权重表与生成器。
#[path = "ucadata/lib.rs"]
pub mod ucadata;

// 核心 Collator / WildcardPattern trait 与工厂注册。
#[path = "collate.rs"]
pub mod collate;
pub use collate::*;
// 二进制（按字节）比较的 Collator。
#[path = "bin.rs"]
pub mod bin;
pub use bin::*;
// GBK chinese_ci 权重数据表。
#[path = "gbk_chinese_ci_data.rs"]
pub mod gbk_chinese_ci_data;
pub use gbk_chinese_ci_data::*;
#[path = "gbk_bin.rs"]
pub mod gbk_bin;
pub use gbk_bin::*;
#[path = "gbk_chinese_ci.rs"]
pub mod gbk_chinese_ci;
pub use gbk_chinese_ci::*;
#[path = "gb18030_bin.rs"]
pub mod gb18030_bin;
pub use gb18030_bin::*;
#[path = "gb18030_chinese_ci.rs"]
pub mod gb18030_chinese_ci;
pub use gb18030_chinese_ci::*;
#[path = "general_ci.rs"]
pub mod general_ci;
pub use general_ci::*;
// 拼音排序规则占位实现（Go 侧尚未完成）。
#[path = "pinyin_tidb_as_cs.rs"]
pub mod pinyin_tidb_as_cs;
pub use pinyin_tidb_as_cs::*;
// Unicode 4.0.0 / 9.0.0 权重表驱动的 Collator 实现与生成数据。
#[path = "unicode_0400_ci_impl.rs"]
pub mod unicode_0400_ci_impl;
pub use unicode_0400_ci_impl::*;
#[path = "unicode_0400_ci_generated.rs"]
pub mod unicode_0400_ci_generated;
pub use unicode_0400_ci_generated::*;
#[path = "unicode_0900_ai_ci_impl.rs"]
pub mod unicode_0900_ai_ci_impl;
pub use unicode_0900_ai_ci_impl::*;
#[path = "unicode_0900_ai_ci_generated.rs"]
pub mod unicode_0900_ai_ci_generated;
pub use unicode_0900_ai_ci_generated::*;

#[cfg(test)]
#[path = "bin_1_aster_unit_test.rs"]
mod bin_aster_unit_test;
#[cfg(test)]
#[path = "charset_test.rs"]
mod charset_test;
#[cfg(test)]
#[path = "collate_bench_test.rs"]
mod collate_bench_test;
#[cfg(test)]
#[path = "collate_test.rs"]
mod collate_test;
#[cfg(test)]
#[path = "gb18030_bin_test.rs"]
mod gb18030_bin_test;
#[cfg(test)]
#[path = "gbk_bin_test.rs"]
mod gbk_bin_test;
#[cfg(test)]
#[path = "general_ci_2_aster_unit_test.rs"]
mod general_ci_aster_unit_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
