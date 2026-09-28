// Copyright 2026 PingCAP, Inc.
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

// S3 兼容存储权限探测。
//
// 按调用方给定的 `Permission` 列表依次调用 `PrefixClient` 对应检查方法；
// 任一失败则包装上下文错误并短路返回，未知权限立即报错。

#![allow(non_snake_case)]

use anyhow::{Context, Result, anyhow};

use crate::PrefixClient;

/// 依次校验所需对象存储权限；失败时带上权限名上下文。
pub fn CheckPermissions(
    ctx: &storeapi::Context,
    cli: &dyn PrefixClient,
    perms: &[storeapi::Permission],
) -> Result<()> {
    for perm in perms {
        // 将抽象 Permission 映射到 PrefixClient 上的具体探测调用。
        let result = match perm {
            storeapi::Permission::AccessBuckets => cli.CheckBucketExistence(ctx),
            storeapi::Permission::ListObjects => cli.CheckListObjects(ctx),
            storeapi::Permission::GetObject => cli.CheckGetObject(ctx),
            storeapi::Permission::PutAndDeleteObject => cli.CheckPutAndDeleteObject(ctx),
            _ => return Err(anyhow!("unknown permission: {}", perm.as_str())),
        };
        result.map_err(|err| {
            let context = format!("permission {}: {err}", perm.as_str());
            err.context(context)
        })?;
    }
    Ok(())
}
