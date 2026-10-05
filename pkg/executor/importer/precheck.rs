// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// IMPORT INTO 导入前置检查（precheck）。
//
// 在真正解析数据文件或启动引擎前，校验目标表状态、并发 job、数据体量上限，
// 以及与 CDC（变更数据捕获）/PiTR（基于时间点恢复的日志流）任务的冲突；
// Global Sort 场景还会校验云对象存储 URI 与读写权限。

use crate::{DataSourceTypeFile, LoadDataController};

/// 前置检查所需的外部依赖：job 计数、表行探测、部署模式、CDC/PiTR 与云存储权限。
pub trait ImportPrecheckService {
    /// 目标库表上仍在运行的导入 job 数量。
    fn ActiveJobCount(&mut self, database: &str, table: &str) -> Result<i64, String>;
    /// 目标表是否已有行数据（IMPORT INTO 要求空表）。
    fn TableHasRows(&mut self, database: &str, table: &str) -> Result<bool, String>;
    /// 是否为 Starter 部署模式（有更严格的导入体量上限）。
    fn IsStarterDeployment(&self) -> bool;
    /// Starter 模式下允许的最大导入数据字节数；0 表示不限制。
    fn StarterMaxImportDataSize(&self) -> u64;
    /// 当前存在的 PiTR 日志流任务名列表。
    fn PiTRTaskNames(&mut self) -> Result<Vec<String>, String>;
    /// 若有运行中的 CDC changefeed，返回人类可读错误信息。
    fn RunningCDCChangefeedsMessage(&mut self) -> Result<Option<String>, String>;
    /// 校验 Global Sort 所用云存储 URI 是否具备给定权限集合。
    fn CheckGlobalSortStorePrivileges(
        &mut self,
        uri: &str,
        permissions: &[GlobalSortPermission],
    ) -> Result<(), String>;
}

/// Global Sort 写云存储时需要检查的对象存储权限集合。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlobalSortPermission {
    /// 读取单个对象。
    GetObject,
    /// 列举桶内对象。
    ListObjects,
    /// 写入并删除对象。
    PutAndDeleteObject,
}

/// Reject enabled TTL before any import side effects, even with DISABLE_PRECHECK.
/// An asynchronous TTL job can race with import mode and invalidate the checksum.
pub fn CheckImportTableTTL(
    table: &astersql_meta_model::TableInfo,
) -> Result<(), astersql_util_dbterror::errors::SharedError> {
    if table.TTLInfo.as_ref().is_some_and(|ttl| ttl.Enable) {
        return Err(
            astersql_util_dbterror_exeerrors::exeerrors::ErrLoadDataPreCheckFailed.FastGenByArgs(
                &["target table has TTL enabled, please disable TTL before IMPORT INTO".into()],
            ),
        );
    }
    Ok(())
}

impl LoadDataController {
    /// 完整前置检查：含文件总大小；用于正式提交导入前。
    pub fn CheckRequirements(&self, service: &mut dyn ImportPrecheckService) -> Result<(), String> {
        self.checkRequirements(service, true)
    }

    /// InitDataFiles 之前的轻量检查：跳过文件总大小并探测数据源访问。
    pub fn CheckRequirementsBeforeInitDataFiles(
        &self,
        context: &astersql_objstore_storeapi::Context,
        service: &mut dyn ImportPrecheckService,
    ) -> Result<(), String> {
        self.checkRequirements(service, false)?;
        if self.Plan.DataSourceType == DataSourceTypeFile {
            return self.CheckDataSourceAccess(context);
        }
        Ok(())
    }

    /// 按数据源类型串联各项检查；`check_total_file_size` 控制是否校验文件体量。
    fn checkRequirements(
        &self,
        service: &mut dyn ImportPrecheckService,
        check_total_file_size: bool,
    ) -> Result<(), String> {
        let table_info = self
            .Plan
            .TableInfo
            .as_deref()
            .unwrap_or_else(|| self.Table.Meta());
        CheckImportTableTTL(table_info).map_err(|error| error.to_string())?;
        // 文件源：禁止同表已有活跃 job，并按需校验导入体量。
        if self.Plan.DataSourceType == DataSourceTypeFile {
            let table_name = self.Table.Meta().Name.L.as_str();
            if service.ActiveJobCount(&self.Plan.DBName, table_name)? > 0 {
                return Err("there is active job on the target table already".into());
            }
            if check_total_file_size {
                self.CheckImportDataSize(service)?;
            }
        }
        self.checkTableEmpty(service)?;
        // DisablePrecheck 时跳过 CDC/PiTR 冲突探测（例如测试或显式关闭）。
        if !self.Plan.DisablePrecheck {
            self.checkCDCPiTRTasks(service)?;
        }
        if self.Plan.IsGlobalSort() {
            self.checkGlobalSortStorePrivilege(service)?;
        }
        Ok(())
    }

    /// 校验匹配到的文件非空，并在 Starter 部署下检查真实导入体量上限。
    pub fn CheckImportDataSize(&self, service: &dyn ImportPrecheckService) -> Result<(), String> {
        self.CheckImportDataSizeWithLimit(
            service.IsStarterDeployment(),
            service.StarterMaxImportDataSize(),
        )
    }

    /// Go OnPrepare 只需部署模式与 Starter 数据量上限，无需完整 precheck 会话。
    pub fn CheckImportDataSizeWithLimit(
        &self,
        is_starter: bool,
        maximum: u64,
    ) -> Result<(), String> {
        if self.Plan.TotalFileSize == 0 {
            return Err(
                "No file matched, or the file is empty. Please provide a valid file location."
                    .into(),
            );
        }
        self.checkStarterMaxImportDataSize(is_starter, maximum)
    }

    /// Starter 部署下：解压后真实体量不得超过配置上限。
    fn checkStarterMaxImportDataSize(&self, is_starter: bool, maximum: u64) -> Result<(), String> {
        check_import_size_limit(
            self.Plan.TotalFileSize,
            self.TotalRealSize,
            is_starter,
            maximum,
        )
    }
    /// 目标表必须为空，避免覆盖既有数据。
    fn checkTableEmpty(&self, service: &mut dyn ImportPrecheckService) -> Result<(), String> {
        if service.TableHasRows(&self.Plan.DBName, &self.Table.Meta().Name.L)? {
            return Err("target table is not empty".into());
        }
        Ok(())
    }

    /// 拒绝与 PiTR 日志流或运行中 CDC changefeed 并存，以免导入与增量同步冲突。
    fn checkCDCPiTRTasks(&self, service: &mut dyn ImportPrecheckService) -> Result<(), String> {
        let pitr_tasks = service.PiTRTaskNames()?;
        if !pitr_tasks.is_empty() {
            return Err(format!("found PiTR log streaming task(s): {pitr_tasks:?},"));
        }
        if let Some(message) = service.RunningCDCChangefeedsMessage()? {
            return Err(message);
        }
        Ok(())
    }

    /// Global Sort：URI scheme 必须受支持，并具备读写删对象权限。
    fn checkGlobalSortStorePrivilege(
        &self,
        service: &mut dyn ImportPrecheckService,
    ) -> Result<(), String> {
        validate_global_sort_uri(&self.Plan.CloudStorageURI)?;
        service.CheckGlobalSortStorePrivileges(
            &self.Plan.CloudStorageURI,
            &[
                GlobalSortPermission::GetObject,
                GlobalSortPermission::ListObjects,
                GlobalSortPermission::PutAndDeleteObject,
            ],
        )
    }
}

/// Validate the URI before invoking the external permission boundary.
pub(crate) fn validate_global_sort_uri(uri: &str) -> Result<(), String> {
    let invalid_uri = |reason: String| {
        astersql_util_dbterror_exeerrors::exeerrors::ErrLoadDataInvalidURI
            .GenWithStackByArgs(&["cloud storage".into(), reason.into()])
            .to_string()
    };
    let mut url = astersql_objstore::parse::ParseRawURL(uri)
        .map_err(|error| invalid_uri(error.to_string()))?;
    let backend = astersql_objstore::parse::ParseBackendFromURL(&mut url, None)
        .map_err(|error| invalid_uri(error.to_string()))?;
    if !matches!(
        backend,
        astersql_objstore::parse::StorageBackend::S3(_)
            | astersql_objstore::parse::StorageBackend::Gcs(_)
            | astersql_objstore::parse::StorageBackend::AzureBlobStorage(_)
    ) {
        return Err(format!(
            "unsupported cloud storage uri scheme: {}",
            url.scheme
        ));
    }
    Ok(())
}

pub(crate) fn check_import_size_limit(
    total_file_size: i64,
    total_real_size: i64,
    is_starter: bool,
    maximum: u64,
) -> Result<(), String> {
    if !is_starter || maximum == 0 || total_real_size <= 0 || total_real_size as u64 <= maximum {
        return Ok(());
    }
    Err(format!(
        "total real import data size {} exceeds maximum import size limit {} (total file size {})",
        display_bytes(total_real_size as u64),
        display_bytes(maximum),
        display_bytes(total_file_size.max(0) as u64),
    ))
}

/// 判断云存储 URI 是否为支持的 scheme，且 authority（桶名）非空。
pub(crate) fn is_supported_cloud_uri(uri: &str) -> bool {
    let Some((scheme, authority)) = uri.split_once("://") else {
        return false;
    };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "s3" | "gcs" | "gs" | "azure" | "azblob"
    ) && authority
        .split('/')
        .next()
        .is_some_and(|host| !host.is_empty())
}

/// 将字节数格式化为可读的 TiB/GiB/MiB/KiB/B 字符串。
pub(crate) fn display_bytes(bytes: u64) -> String {
    const UNITS: &[(&str, u64)] = &[
        ("TiB", 1_u64 << 40),
        ("GiB", 1_u64 << 30),
        ("MiB", 1_u64 << 20),
        ("KiB", 1_u64 << 10),
    ];
    for (unit, size) in UNITS {
        if bytes >= *size {
            let scaled = bytes as f64 / *size as f64;
            // docker/go-units uses %.4g: retain four significant digits.
            let integer_digits = scaled.log10().floor().max(0.0) as usize + 1;
            let decimals = 4usize.saturating_sub(integer_digits);
            let mut value = format!("{scaled:.decimals$}");
            while value.ends_with('0') {
                value.pop();
            }
            if value.ends_with('.') {
                value.pop();
            }
            return format!("{value}{unit}");
        }
    }
    format!("{bytes}B")
}
