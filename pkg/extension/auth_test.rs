// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 鉴权插件校验的规范（canonical）回归测试。
//
// 覆盖缺回调拒绝、合法插件通过，以及同名重复注册失败。

use crate::{AuthPlugin, validate_auth_plugins};
use std::sync::Arc;

/// 缺默认回调应失败；补齐后成功；重复同名再次失败。
#[test]
fn canonical_auth_plugin_validation_rejects_missing_callbacks_and_duplicates() {
    assert!(validate_auth_plugins(&[Arc::new(AuthPlugin::default())]).is_err());
    let plugin = || {
        Arc::new(AuthPlugin {
            Name: "aster_auth".into(),
            AuthenticateUser: Some(Arc::new(|_| Ok(()))),
            GenerateAuthString: Some(Arc::new(|value| (value, true))),
            ValidateAuthString: Some(Arc::new(|value| !value.is_empty())),
            ..AuthPlugin::default()
        })
    };
    assert!(validate_auth_plugins(&[plugin()]).is_ok());
    assert!(validate_auth_plugins(&[plugin(), plugin()]).is_err());
}
