// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 对象存储后端 CLI/配置旗标：注册 S3、GCS、Azure Blob 选项并解析为后端配置结构。

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, anyhow};

use crate::azblob::{
    AZBLOB_ACCESS_TIER_OPTION, AZBLOB_ACCOUNT_KEY_OPTION, AZBLOB_ACCOUNT_NAME_OPTION,
    AZBLOB_ENCRYPTION_KEY_OPTION, AZBLOB_ENCRYPTION_SCOPE_OPTION, AZBLOB_ENDPOINT_OPTION,
    AZBLOB_SAS_TOKEN_OPTION, AzblobBackendOptions,
};
use crate::gcs::{
    GCS_CREDENTIALS_FILE_OPTION, GCS_ENDPOINT_OPTION, GCS_PREDEFINED_ACL_OPTION,
    GCS_STORAGE_CLASS_OPTION, GCSBackendOptions,
};

/// S3 相关旗标名列表（endpoint、region、存储类、SSE、ACL、AssumeRole 等）。
const S3_FLAGS: &[&str] = &[
    "s3.endpoint",
    "s3.region",
    "s3.storage-class",
    "s3.sse",
    "s3.sse-kms-key-id",
    "s3.acl",
    "s3.provider",
    "s3.role-arn",
    "s3.external-id",
    "s3.profile",
];

/// GCS 相关旗标名列表。
const GCS_FLAGS: &[&str] = &[
    GCS_ENDPOINT_OPTION,
    GCS_STORAGE_CLASS_OPTION,
    GCS_PREDEFINED_ACL_OPTION,
    GCS_CREDENTIALS_FILE_OPTION,
];

/// Azure Blob 相关旗标名列表。
const AZURE_FLAGS: &[&str] = &[
    AZBLOB_ENDPOINT_OPTION,
    AZBLOB_ACCESS_TIER_OPTION,
    AZBLOB_ACCOUNT_NAME_OPTION,
    AZBLOB_ACCOUNT_KEY_OPTION,
    AZBLOB_SAS_TOKEN_OPTION,
    AZBLOB_ENCRYPTION_SCOPE_OPTION,
    AZBLOB_ENCRYPTION_KEY_OPTION,
];

/// 轻量旗标集合：有序键值表 + 隐藏集合，模拟 Go pflag 子集。
#[derive(Clone, Debug, Default)]
pub struct FlagSet {
    values: BTreeMap<String, String>,
    hidden: BTreeSet<String>,
}

impl FlagSet {
    /// 注册旗标名与默认值；重复注册与 Go pflag 一样触发 panic。
    pub fn register(&mut self, name: &str, default_value: &str) {
        assert!(!self.values.contains_key(name), "flag redefined: {name}");
        self.values
            .insert(name.to_owned(), default_value.to_owned());
    }

    /// 设置已注册旗标的值；未注册则报错。
    pub fn set(&mut self, name: &str, value: &str) -> Result<()> {
        let flag = self
            .values
            .get_mut(name)
            .ok_or_else(|| anyhow!("flag not defined: {name}"))?;
        *flag = value.to_owned();
        Ok(())
    }

    /// 读取旗标当前值。
    pub fn get(&self, name: &str) -> Result<String> {
        self.values
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow!("flag not defined: {name}"))
    }

    /// 将旗标标记为隐藏（如帮助输出中不展示）。
    pub fn hide(&mut self, name: &str) -> Result<()> {
        if !self.values.contains_key(name) {
            return Err(anyhow!("flag not defined: {name}"));
        }
        self.hidden.insert(name.to_owned());
        Ok(())
    }

    /// 查询旗标是否已隐藏。
    pub fn is_hidden(&self, name: &str) -> bool {
        self.hidden.contains(name)
    }
}

/// S3 后端可调选项快照。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct S3BackendOptions {
    pub endpoint: String,
    pub region: String,
    pub storage_class: String,
    pub sse: String,
    pub sse_kms_key_id: String,
    pub acl: String,
    pub provider: String,
    pub role_arn: String,
    pub external_id: String,
    pub profile: String,
    pub force_path_style: bool,
}

impl S3BackendOptions {
    /// 从 [`FlagSet`] 填充各 S3 字段。
    fn parse_from_flags(&mut self, flags: &FlagSet) -> Result<()> {
        self.endpoint = flags.get("s3.endpoint")?;
        self.endpoint = self
            .endpoint
            .strip_suffix('/')
            .unwrap_or(&self.endpoint)
            .to_owned();
        self.region = flags.get("s3.region")?;
        self.sse = flags.get("s3.sse")?;
        self.sse_kms_key_id = flags.get("s3.sse-kms-key-id")?;
        self.acl = flags.get("s3.acl")?;
        self.storage_class = flags.get("s3.storage-class")?;
        self.force_path_style = true;
        self.provider = flags.get("s3.provider")?;
        self.role_arn = flags.get("s3.role-arn")?;
        self.external_id = flags.get("s3.external-id")?;
        self.profile = flags.get("s3.profile")?;
        Ok(())
    }
}

/// 聚合 S3 / GCS / Azure 三套后端选项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackendOptions {
    pub s3: S3BackendOptions,
    pub gcs: GCSBackendOptions,
    pub azblob: AzblobBackendOptions,
}

impl BackendOptions {
    /// 从旗标解析全部后端选项（GCS 走其自身 `parse_from_flags`）。
    pub fn parse_from_flags(&mut self, flags: &FlagSet) -> Result<()> {
        self.s3.parse_from_flags(flags)?;
        self.gcs.parse_from_flags(flags)?;
        self.azblob.endpoint = flags.get(AZBLOB_ENDPOINT_OPTION)?;
        self.azblob.access_tier = flags.get(AZBLOB_ACCESS_TIER_OPTION)?;
        self.azblob.account_name = flags.get(AZBLOB_ACCOUNT_NAME_OPTION)?;
        self.azblob.account_key = flags.get(AZBLOB_ACCOUNT_KEY_OPTION)?;
        self.azblob.sas_token = flags.get(AZBLOB_SAS_TOKEN_OPTION)?;
        self.azblob.encryption_scope = flags.get(AZBLOB_ENCRYPTION_SCOPE_OPTION)?;
        self.azblob.encryption_key = flags.get(AZBLOB_ENCRYPTION_KEY_OPTION)?;
        Ok(())
    }
}

/// 向旗标集注册 S3 + GCS + Azure 全部选项（默认空串）。
pub fn define_flags(flags: &mut FlagSet) {
    for name in S3_FLAGS.iter().chain(GCS_FLAGS).chain(AZURE_FLAGS) {
        flags.register(name, "");
    }
}

/// 流式备份场景隐藏 GCS / Azure 旗标（仅保留 S3 可见）。
pub fn hidden_flags_for_stream(flags: &mut FlagSet) -> Result<()> {
    for name in GCS_FLAGS.iter().chain(AZURE_FLAGS) {
        let _ = flags.hide(name);
    }
    Ok(())
}

/// Go 风格导出名：`DefineFlags`。
#[allow(non_snake_case)]
pub fn DefineFlags(flags: &mut FlagSet) {
    define_flags(flags);
}

/// Go 风格导出名：`HiddenFlagsForStream`。
#[allow(non_snake_case)]
pub fn HiddenFlagsForStream(flags: &mut FlagSet) -> Result<()> {
    hidden_flags_for_stream(flags)
}
