// Copyright 2022 PingCAP, Inc.
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

// 配置模块中的公共常量定义。
//
// 本文件存放与统计信息（statistics）采样相关的默认配置常量。
// 统计信息是查询优化器（optimizer）估算执行计划代价的基础数据，
// 通常通过对表数据进行采样（sampling）来收集。

// DefRowsForSampleRate is default sample rows used to calculate samplerate.
/// 默认的采样行数，用于计算采样率（sample rate）。
///
/// 在执行 ANALYZE 收集表统计信息时，若未显式指定采样率，
/// 系统会以该值作为期望采样的行数，结合表的总行数反推出实际采样率：
/// 采样率 ≈ DefRowsForSampleRate / 表总行数。
/// 这样既能保证小表全量采样，又能避免大表采样开销过大。
pub const DefRowsForSampleRate: i64 = 110000;
