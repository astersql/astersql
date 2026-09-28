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

// DDL 参数版本（DDL args V1 / V2）特性开关的单元测试。
//
// DDL Job 参数格式在 TiDB 8.4.0 起默认使用 V2；为兼容旧路径，
// 可通过 `ddlargsv1` feature 强制走 V1。本模块验证：启用该 feature
// 时 `FORCE_V1`/`ForceV1` 为 true，默认构建下为 false。
// 同时检查模块内常量与 crate 根再导出是否一致。

#[cfg(test)]
mod tests {
    /// 启用 `ddlargsv1` 时，强制使用 DDL Job V1 参数。
    #[cfg(feature = "ddlargsv1")]
    #[test]
    fn ddlargsv1_feature_forces_v1_jobs() {
        // 模块路径与 crate 再导出都应指向同一强制 V1 开关。
        assert!(crate::force_v1::FORCE_V1);
        assert!(crate::force_v1::ForceV1);
        assert!(crate::FORCE_V1);
        assert!(crate::ForceV1);
    }

    /// 默认构建（未启用 `ddlargsv1`）使用当前 DDL 参数格式（V2）。
    #[cfg(not(feature = "ddlargsv1"))]
    #[test]
    fn default_build_uses_current_ddl_args() {
        // normal 模块与 crate 再导出均应为 false，表示不强制 V1。
        assert!(!crate::normal::FORCE_V1);
        assert!(!crate::normal::ForceV1);
        assert!(!crate::FORCE_V1);
        assert!(!crate::ForceV1);
    }
}
