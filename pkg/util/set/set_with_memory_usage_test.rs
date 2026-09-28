// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 带内存用量集合的基准/边界测试。
//
// 将 Go 侧 float64/int64/string 三组 MemoryUsage benchmark 的行数边界表迁为
// 确定性测试：完整插入后核对 Count，覆盖 6.5×(1<<16/17) 边界与越界一位。

use super::*;

// memoryUsageBenchCase 对应 Go benchmark 内部的匿名 testCase 结构体。
// Go 在三个 benchmark 中重复声明该结构；Rust 测试提取同一字段形状，便于人工核对 rowNum 表。
/// 单组场景的插入行数配置。
struct MemoryUsageBenchCase {
    rowNum: usize,
}

// benchmark_cases 保留 Go 中三组 benchmark 共享的行数边界。
// 851968/851969 和 425984/425985 分别覆盖 6.5*(1<<17) 与 6.5*(1<<16) 的边界和越界一位场景。
/// 返回与 Go 三组 benchmark 共享的 rowNum 表。
fn benchmark_cases() -> Vec<MemoryUsageBenchCase> {
    vec![
        MemoryUsageBenchCase { rowNum: 0 },
        MemoryUsageBenchCase { rowNum: 100 },
        MemoryUsageBenchCase { rowNum: 10000 },
        MemoryUsageBenchCase { rowNum: 1000000 },
        MemoryUsageBenchCase { rowNum: 851968 }, // 6.5 * (1 << 17)
        MemoryUsageBenchCase { rowNum: 851969 }, // 6.5 * (1 << 17) + 1
        MemoryUsageBenchCase { rowNum: 425984 }, // 6.5 * (1 << 16)
        MemoryUsageBenchCase { rowNum: 425985 }, // 6.5 * (1 << 16) + 1
    ]
}

// BenchmarkFloat64SetMemoryUsage 对应 Go 的同名基准测试；稳定 Rust test harness
// 没有 ReportAllocs/b.N，因此每个子场景执行一轮并验证完整插入结果。
/// float64 带内存统计集合：按 rowNum 插入 0..n-1 并断言 Count。
#[test]
pub fn BenchmarkFloat64SetMemoryUsage() {
    for c in benchmark_cases() {
        let (mut float64Set, _) = NewFloat64SetWithMemoryUsage(&[]);
        for num in 0..c.rowNum {
            float64Set.Insert(num as f64);
        }
        assert_eq!(c.rowNum, float64Set.Count(), "MapRows {}", c.rowNum);
    }
}

// BenchmarkInt64SetMemoryUsage 对应 Go 的 int64 带内存统计集合 benchmark。
// 它与 float64 版本只在构造函数和插入值类型上不同，行数表完全一致。
/// int64 带内存统计集合：同表行数插入并断言 Count。
#[test]
pub fn BenchmarkInt64SetMemoryUsage() {
    for c in benchmark_cases() {
        let (mut int64Set, _) = NewInt64SetWithMemoryUsage(&[]);
        for num in 0..c.rowNum {
            int64Set.Insert(num as i64);
        }
        assert_eq!(c.rowNum, int64Set.Count(), "MapRows {}", c.rowNum);
    }
}

// BenchmarkStringSetMemoryUsage 对应 Go 的字符串带内存统计集合 benchmark。
// strconv.Itoa(num) 迁移为 num.to_string()，保留每次插入都生成十进制字符串的分配行为意图。
/// 字符串带内存统计集合：插入十进制字符串并断言 Count。
#[test]
pub fn BenchmarkStringSetMemoryUsage() {
    for c in benchmark_cases() {
        let (mut stringSet, _) = NewStringSetWithMemoryUsage(&[]);
        for num in 0..c.rowNum {
            stringSet.Insert(num.to_string());
        }
        assert_eq!(c.rowNum, stringSet.Count(), "MapRows {}", c.rowNum);
    }
}
