// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::{
    CreateUserPriv, MySQLPrivilege, RoleIdentity, SelectPriv, SkipWithGrant, UpdatePriv,
    UserPrivileges,
};

#[derive(Clone, Copy)]
enum AttrVisMode {
    All,
    NonSystem,
    SelfOnly,
}

/// Snapshot filter for INFORMATION_SCHEMA.USER_ATTRIBUTES (MySQL 8.0.22+).
pub struct UserAttrFilter {
    privilege: Option<MySQLPrivilege>,
    viewer_user: String,
    viewer_host: String,
    mode: AttrVisMode,
}

impl UserAttrFilter {
    /// Test the target account, recognizing self before excluding system users.
    pub fn Visible(&self, user: &str, host: &str) -> bool {
        if matches!(self.mode, AttrVisMode::All) {
            return true;
        }
        let privilege = self
            .privilege
            .as_ref()
            .expect("restricted filter has a cache");
        if privilege
            .matchUser(user, host)
            .is_some_and(|record| record.base.r#match(&self.viewer_user, &self.viewer_host))
        {
            return true;
        }
        if matches!(self.mode, AttrVisMode::SelfOnly) {
            return false;
        }
        !privilege.RequestDynamicVerification(&[], user, host, "SYSTEM_USER", false)
    }
}

/// A missing/foreign manager is represented by None, matching Go's type assertion fallback.
pub fn NewUserAttrFilter(
    active_roles: &[RoleIdentity],
    viewer_user: &str,
    viewer_host: &str,
    manager: Option<&UserPrivileges>,
) -> UserAttrFilter {
    let mut filter = UserAttrFilter {
        privilege: None,
        viewer_user: viewer_user.into(),
        viewer_host: viewer_host.into(),
        mode: AttrVisMode::All,
    };
    if SkipWithGrant() || manager.is_none() || (viewer_user.is_empty() && viewer_host.is_empty()) {
        return filter;
    }
    let privilege = manager.unwrap().Handle.Get();
    filter.mode = if privilege.RequestVerification(
        active_roles,
        viewer_user,
        viewer_host,
        "mysql",
        "user",
        "",
        SelectPriv,
    ) || privilege.RequestVerification(
        active_roles,
        viewer_user,
        viewer_host,
        "mysql",
        "user",
        "",
        UpdatePriv,
    ) {
        AttrVisMode::All
    } else if privilege.RequestVerification(
        active_roles,
        viewer_user,
        viewer_host,
        "",
        "",
        "",
        CreateUserPriv,
    ) {
        if privilege.RequestDynamicVerification(
            active_roles,
            viewer_user,
            viewer_host,
            "SYSTEM_USER",
            false,
        ) {
            AttrVisMode::All
        } else {
            AttrVisMode::NonSystem
        }
    } else {
        AttrVisMode::SelfOnly
    };
    filter.privilege = Some(privilege);
    filter
}
