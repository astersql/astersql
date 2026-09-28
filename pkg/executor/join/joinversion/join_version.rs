// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Hash Join 版本常量与平台能力探测。
//
// 会话变量 `tidb_hash_join_version` 取 `legacy` 时走 Hash Join v1，取 `optimized` 时走 v2。
// v2 依赖非移动堆与足够宽的 uintptr 来编码行表指针；`is_hash_join_v2_supported` 据此判断。

use std::sync::atomic::{AtomicBool, Ordering};

/// Hash join v1（legacy）版本名。
/// Hash join v1.
pub const HASH_JOIN_VERSION_LEGACY: &str = "legacy";

/// Hash join v2（optimized）版本名。
/// Hash join v2.
pub const HASH_JOIN_VERSION_OPTIMIZED: &str = "optimized";

/// TiFlash 默认使用的 Hash Join 版本（与 legacy 相同）。
/// Default hash join version used by TiFlash.
pub const TIFLASH_HASH_JOIN_VERSION_DEFAULT: &str = HASH_JOIN_VERSION_LEGACY;

/// 控制非 GA（尚未正式发布）的 join 变体是否允许使用 Hash Join v2。
///
/// 取 Go 包初始化后的默认值；原子变量保证并发测试下读写安全。
/// Controls whether non-GA join variants may use hash join v2.
///
/// This has the post-initialization value of the Go package. An atomic keeps
/// the exported, mutable package setting safe when tests run concurrently.
pub static USE_HASH_JOIN_V2_FOR_NON_GA_JOIN: AtomicBool = AtomicBool::new(true);

/// 读取非 GA join 是否允许使用 Hash Join v2。
pub fn use_hash_join_v2_for_non_ga_join() -> bool {
    USE_HASH_JOIN_V2_FOR_NON_GA_JOIN.load(Ordering::SeqCst)
}

/// 设置非 GA join 是否允许使用 Hash Join v2。
pub fn set_use_hash_join_v2_for_non_ga_join(enabled: bool) {
    USE_HASH_JOIN_V2_FOR_NON_GA_JOIN.store(enabled, Ordering::SeqCst);
}

/// 当配置的版本字符串（忽略大小写）等于 `optimized` 时返回 true，表示选择 Hash Join v2。
/// Returns true when the configured hash join version selects hash join v2.
pub fn is_optimized_version(hash_join_version: &str) -> bool {
    hash_join_version
        .chars()
        .zip(HASH_JOIN_VERSION_OPTIMIZED.chars())
        .all(|(actual, expected)| go_simple_lowercase_matches(actual, expected))
        && hash_join_version.chars().count() == HASH_JOIN_VERSION_OPTIMIZED.chars().count()
}

/// Match Go's one-rune `unicode.ToLower` semantics against an ASCII rune.
fn go_simple_lowercase_matches(actual: char, expected: char) -> bool {
    actual.to_ascii_lowercase() == expected
        // Go maps LATIN CAPITAL LETTER I WITH DOT ABOVE directly to `i`.
        || (actual == '\u{0130}' && expected == 'i')
}

const SIZE_OF_UINTPTR: usize = std::mem::size_of::<usize>();
const SIZE_OF_POINTER: usize = std::mem::size_of::<*const ()>();

// Rust 无移动式 GC；移动拥有型指针不会移动堆分配，这是行表指针编码所需的性质。
// Rust has no moving garbage collector. Moving an owning pointer does not move
// its heap allocation, which is the property required by the hash row table.
const fn heap_objects_can_move() -> bool {
    false
}

/// 当前目标是否满足 Hash Join v2 所需的指针表示（非移动堆且 uintptr 足以容纳指针）。
/// Returns true when the current target supports the pointer representation
/// required by hash join v2.
pub fn is_hash_join_v2_supported() -> bool {
    !heap_objects_can_move() && SIZE_OF_UINTPTR >= SIZE_OF_POINTER
}

// 兼容导出：供已机械翻译的下游调用方使用 Go 风格名称；新代码应使用上方惯用名。
// Compatibility exports for ported downstream callers. New
// Rust code should use the idiomatic names above.
#[allow(non_upper_case_globals)]
pub const HashJoinVersionLegacy: &str = HASH_JOIN_VERSION_LEGACY;
#[allow(non_upper_case_globals)]
pub const HashJoinVersionOptimized: &str = HASH_JOIN_VERSION_OPTIMIZED;
#[allow(non_upper_case_globals)]
pub const TiFlashHashJoinVersionDefVal: &str = TIFLASH_HASH_JOIN_VERSION_DEFAULT;
#[allow(non_snake_case)]
pub use USE_HASH_JOIN_V2_FOR_NON_GA_JOIN as UseHashJoinV2ForNonGAJoin;

/// Go 风格别名：`is_optimized_version`。
#[allow(non_snake_case)]
pub fn IsOptimizedVersion(hash_join_version: &str) -> bool {
    is_optimized_version(hash_join_version)
}

/// Go 风格别名：`is_hash_join_v2_supported`。
#[allow(non_snake_case)]
pub fn IsHashJoinV2Supported() -> bool {
    is_hash_join_v2_supported()
}
