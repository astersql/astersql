// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// MySQL/TiDB 静态权限位、文本标识与系统表列名映射。
//
// 权限（Privilege）是 GRANT/REVOKE 控制的能力位；本模块只查询与组合内存元数据，
// 不连接 `mysql.user`、不执行授权 SQL，也不产生外部 IO。

// 本文件由 pkg/parser/mysql/privs.go 迁移而来，保留权限位、映射表和作用域列表。
// 它只查询与组合内存中的权限元数据，不连接 mysql.user、不执行授权 SQL，也不产生外部 IO。

use std::collections::HashMap;

/// AllPrivilegeLiteral 对应 Go 的 ALL 权限显示文本。
pub const AllPrivilegeLiteral: &str = "ALL PRIVILEGES";

/// Priv2Str 对应权限到 SQL 标识文本的映射。
pub fn Priv2Str() -> HashMap<PrivilegeType, &'static str> {
    [
        (CreatePriv, "Create"),
        (SelectPriv, "Select"),
        (InsertPriv, "Insert"),
        (UpdatePriv, "Update"),
        (DeletePriv, "Delete"),
        (ShowDBPriv, "Show Databases"),
        (SuperPriv, "Super"),
        (CreateUserPriv, "Create User"),
        (CreateTablespacePriv, "Create Tablespace"),
        (TriggerPriv, "Trigger"),
        (DropPriv, "Drop"),
        (ProcessPriv, "Process"),
        (GrantPriv, "Grant Option"),
        (ReferencesPriv, "References"),
        (AlterPriv, "Alter"),
        (ExecutePriv, "Execute"),
        (IndexPriv, "Index"),
        (CreateViewPriv, "Create View"),
        (ShowViewPriv, "Show View"),
        (CreateRolePriv, "Create Role"),
        (DropRolePriv, "Drop Role"),
        (CreateTMPTablePriv, "CREATE TEMPORARY TABLES"),
        (LockTablesPriv, "LOCK TABLES"),
        (CreateRoutinePriv, "CREATE ROUTINE"),
        (AlterRoutinePriv, "ALTER ROUTINE"),
        (EventPriv, "EVENT"),
        (ShutdownPriv, "SHUTDOWN"),
        (ReloadPriv, "RELOAD"),
        (FilePriv, "FILE"),
        (ConfigPriv, "CONFIG"),
        (UsagePriv, "USAGE"),
        (ReplicationClientPriv, "REPLICATION CLIENT"),
        (ReplicationSlavePriv, "REPLICATION SLAVE"),
        (AllPriv, AllPrivilegeLiteral),
    ]
    .into_iter()
    .collect()
}

/// Priv2SetStr 对应权限到 Table_priv/Column_priv 集合枚举文本的映射。
pub fn Priv2SetStr() -> HashMap<PrivilegeType, &'static str> {
    [
        (CreatePriv, "Create"),
        (SelectPriv, "Select"),
        (InsertPriv, "Insert"),
        (UpdatePriv, "Update"),
        (DeletePriv, "Delete"),
        (DropPriv, "Drop"),
        (GrantPriv, "Grant"),
        (ReferencesPriv, "References"),
        (LockTablesPriv, "Lock Tables"),
        (CreateTMPTablePriv, "Create Temporary Tables"),
        (EventPriv, "Event"),
        (CreateRoutinePriv, "Create Routine"),
        (AlterRoutinePriv, "Alter Routine"),
        (AlterPriv, "Alter"),
        (ExecutePriv, "Execute"),
        (IndexPriv, "Index"),
        (CreateViewPriv, "Create View"),
        (ShowViewPriv, "Show View"),
        (CreateRolePriv, "Create Role"),
        (DropRolePriv, "Drop Role"),
        (ShutdownPriv, "Shutdown Role"),
        (TriggerPriv, "Trigger"),
    ]
    .into_iter()
    .collect()
}

/// SetStr2Priv 对应集合枚举文本到权限位的反向映射。
pub fn SetStr2Priv() -> HashMap<&'static str, PrivilegeType> {
    [
        ("Create", CreatePriv),
        ("Select", SelectPriv),
        ("Insert", InsertPriv),
        ("Update", UpdatePriv),
        ("Delete", DeletePriv),
        ("Drop", DropPriv),
        ("Grant", GrantPriv),
        ("References", ReferencesPriv),
        ("Lock Tables", LockTablesPriv),
        ("Create Temporary Tables", CreateTMPTablePriv),
        ("Event", EventPriv),
        ("Create Routine", CreateRoutinePriv),
        ("Alter Routine", AlterRoutinePriv),
        ("Alter", AlterPriv),
        ("Execute", ExecutePriv),
        ("Index", IndexPriv),
        ("Create View", CreateViewPriv),
        ("Show View", ShowViewPriv),
        ("Trigger", TriggerPriv),
    ]
    .into_iter()
    .collect()
}

/// Priv2UserCol 对应权限到 mysql.user/mysql.db 列名的映射；这里只返回列名，不访问系统表。
pub fn Priv2UserCol() -> HashMap<PrivilegeType, &'static str> {
    [
        (CreatePriv, "Create_priv"),
        (SelectPriv, "Select_priv"),
        (InsertPriv, "Insert_priv"),
        (UpdatePriv, "Update_priv"),
        (DeletePriv, "Delete_priv"),
        (ShowDBPriv, "Show_db_priv"),
        (SuperPriv, "Super_priv"),
        (CreateUserPriv, "Create_user_priv"),
        (CreateTablespacePriv, "Create_tablespace_priv"),
        (TriggerPriv, "Trigger_priv"),
        (DropPriv, "Drop_priv"),
        (ProcessPriv, "Process_priv"),
        (GrantPriv, "Grant_priv"),
        (ReferencesPriv, "References_priv"),
        (AlterPriv, "Alter_priv"),
        (ExecutePriv, "Execute_priv"),
        (IndexPriv, "Index_priv"),
        (CreateViewPriv, "Create_view_priv"),
        (ShowViewPriv, "Show_view_priv"),
        (CreateRolePriv, "Create_role_priv"),
        (DropRolePriv, "Drop_role_priv"),
        (CreateTMPTablePriv, "Create_tmp_table_priv"),
        (LockTablesPriv, "Lock_tables_priv"),
        (CreateRoutinePriv, "Create_routine_priv"),
        (AlterRoutinePriv, "Alter_routine_priv"),
        (EventPriv, "Event_priv"),
        (ShutdownPriv, "Shutdown_priv"),
        (ReloadPriv, "Reload_priv"),
        (FilePriv, "File_priv"),
        (ConfigPriv, "Config_priv"),
        (ReplicationClientPriv, "Repl_client_priv"),
        (ReplicationSlavePriv, "Repl_slave_priv"),
    ]
    .into_iter()
    .collect()
}

/// Col2PrivType 对应权限表列名到权限位的反向映射。
pub fn Col2PrivType() -> HashMap<&'static str, PrivilegeType> {
    [
        ("Create_priv", CreatePriv),
        ("Select_priv", SelectPriv),
        ("Insert_priv", InsertPriv),
        ("Update_priv", UpdatePriv),
        ("Delete_priv", DeletePriv),
        ("Show_db_priv", ShowDBPriv),
        ("Super_priv", SuperPriv),
        ("Create_user_priv", CreateUserPriv),
        ("Create_tablespace_priv", CreateTablespacePriv),
        ("Trigger_priv", TriggerPriv),
        ("Drop_priv", DropPriv),
        ("Process_priv", ProcessPriv),
        ("Grant_priv", GrantPriv),
        ("References_priv", ReferencesPriv),
        ("Alter_priv", AlterPriv),
        ("Execute_priv", ExecutePriv),
        ("Index_priv", IndexPriv),
        ("Create_view_priv", CreateViewPriv),
        ("Show_view_priv", ShowViewPriv),
        ("Create_role_priv", CreateRolePriv),
        ("Drop_role_priv", DropRolePriv),
        ("Create_tmp_table_priv", CreateTMPTablePriv),
        ("Lock_tables_priv", LockTablesPriv),
        ("Create_routine_priv", CreateRoutinePriv),
        ("Alter_routine_priv", AlterRoutinePriv),
        ("Event_priv", EventPriv),
        ("Shutdown_priv", ShutdownPriv),
        ("Reload_priv", ReloadPriv),
        ("File_priv", FilePriv),
        ("Config_priv", ConfigPriv),
        ("Repl_client_priv", ReplicationClientPriv),
        ("Repl_slave_priv", ReplicationSlavePriv),
    ]
    .into_iter()
    .collect()
}

/// PrivilegeType 对应 Go 的 uint64 权限位类型。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PrivilegeType(pub u64);

/// NewPrivFromColumn 对应 Go 的列名解析；None 等价于返回零值和 false。
pub fn NewPrivFromColumn(col: &str) -> Option<PrivilegeType> {
    Col2PrivType().get(col).copied()
}

/// NewPrivFromSetEnum 对应 Go 的集合枚举解析；未知字符串不会猜测权限。
pub fn NewPrivFromSetEnum(value: &str) -> Option<PrivilegeType> {
    SetStr2Priv().get(value).copied()
}

impl PrivilegeType {
    /// String 返回 SQL 中使用的权限标识，未知权限保持 Go 的空字符串回退。
    pub fn String(self) -> &'static str {
        Priv2Str().get(&self).copied().unwrap_or("")
    }

    /// ColumnString 返回 mysql.user/mysql.db 中对应的列名。
    pub fn ColumnString(self) -> &'static str {
        Priv2UserCol().get(&self).copied().unwrap_or("")
    }

    /// SetString 返回 tables_priv/columns_priv 集合列中的枚举文本。
    pub fn SetString(self) -> &'static str {
        Priv2SetStr().get(&self).copied().unwrap_or("")
    }
}

// 以下权限位严格按 Go iota 顺序显式展开；新增权限仍必须放在 AllPriv 之前以保持版本兼容。
/// USAGE：占位权限，表示“无额外特权但仍可登录”。
pub const UsagePriv: PrivilegeType = PrivilegeType(1_u64 << 0);
/// CREATE：创建库/表等对象。
pub const CreatePriv: PrivilegeType = PrivilegeType(1_u64 << 1);
/// SELECT：查询表数据。
pub const SelectPriv: PrivilegeType = PrivilegeType(1_u64 << 2);
/// INSERT：插入行。
pub const InsertPriv: PrivilegeType = PrivilegeType(1_u64 << 3);
/// UPDATE：更新行。
pub const UpdatePriv: PrivilegeType = PrivilegeType(1_u64 << 4);
/// DELETE：删除行。
pub const DeletePriv: PrivilegeType = PrivilegeType(1_u64 << 5);
/// SHOW DATABASES：列出全部数据库名。
pub const ShowDBPriv: PrivilegeType = PrivilegeType(1_u64 << 6);
/// SUPER：超级用户类管理能力。
pub const SuperPriv: PrivilegeType = PrivilegeType(1_u64 << 7);
/// CREATE USER：创建/改名/删除账户。
pub const CreateUserPriv: PrivilegeType = PrivilegeType(1_u64 << 8);
/// TRIGGER：创建/删除触发器。
pub const TriggerPriv: PrivilegeType = PrivilegeType(1_u64 << 9);
/// DROP：删除库/表等对象。
pub const DropPriv: PrivilegeType = PrivilegeType(1_u64 << 10);
/// PROCESS：查看其它会话进程信息。
pub const ProcessPriv: PrivilegeType = PrivilegeType(1_u64 << 11);
/// GRANT OPTION：可将自身权限再授予他人。
pub const GrantPriv: PrivilegeType = PrivilegeType(1_u64 << 12);
/// REFERENCES：创建外键引用。
pub const ReferencesPriv: PrivilegeType = PrivilegeType(1_u64 << 13);
/// ALTER：修改表结构。
pub const AlterPriv: PrivilegeType = PrivilegeType(1_u64 << 14);
/// EXECUTE：执行存储过程/函数。
pub const ExecutePriv: PrivilegeType = PrivilegeType(1_u64 << 15);
/// INDEX：创建/删除索引。
pub const IndexPriv: PrivilegeType = PrivilegeType(1_u64 << 16);
/// CREATE VIEW：创建视图。
pub const CreateViewPriv: PrivilegeType = PrivilegeType(1_u64 << 17);
/// SHOW VIEW：查看视图定义。
pub const ShowViewPriv: PrivilegeType = PrivilegeType(1_u64 << 18);
/// CREATE ROLE：创建角色。
pub const CreateRolePriv: PrivilegeType = PrivilegeType(1_u64 << 19);
/// DROP ROLE：删除角色。
pub const DropRolePriv: PrivilegeType = PrivilegeType(1_u64 << 20);
/// CREATE TEMPORARY TABLES：创建临时表。
pub const CreateTMPTablePriv: PrivilegeType = PrivilegeType(1_u64 << 21);
/// LOCK TABLES：显式锁表。
pub const LockTablesPriv: PrivilegeType = PrivilegeType(1_u64 << 22);
/// CREATE ROUTINE：创建存储过程/函数。
pub const CreateRoutinePriv: PrivilegeType = PrivilegeType(1_u64 << 23);
/// ALTER ROUTINE：修改/删除例程。
pub const AlterRoutinePriv: PrivilegeType = PrivilegeType(1_u64 << 24);
/// EVENT：管理事件调度器对象。
pub const EventPriv: PrivilegeType = PrivilegeType(1_u64 << 25);
/// SHUTDOWN：关闭服务器。
pub const ShutdownPriv: PrivilegeType = PrivilegeType(1_u64 << 26);
/// RELOAD：执行 FLUSH 等重载操作。
pub const ReloadPriv: PrivilegeType = PrivilegeType(1_u64 << 27);
/// FILE：读写服务器宿主文件。
pub const FilePriv: PrivilegeType = PrivilegeType(1_u64 << 28);
/// CONFIG：动态修改配置类能力。
pub const ConfigPriv: PrivilegeType = PrivilegeType(1_u64 << 29);
/// CREATE TABLESPACE：创建表空间。
pub const CreateTablespacePriv: PrivilegeType = PrivilegeType(1_u64 << 30);
/// REPLICATION CLIENT：查询复制状态。
pub const ReplicationClientPriv: PrivilegeType = PrivilegeType(1_u64 << 31);
/// REPLICATION SLAVE：作为从库读取 binlog。
pub const ReplicationSlavePriv: PrivilegeType = PrivilegeType(1_u64 << 32);
/// ALL PRIVILEGES 哨兵位；具体掩码见 AllPrivMask。
pub const AllPriv: PrivilegeType = PrivilegeType(1_u64 << 33);
/// 动态/扩展权限占位，位序在 AllPriv 之后。
pub const ExtendedPriv: PrivilegeType = PrivilegeType(1_u64 << 34);

/// AllPrivMask 对应 Go 的 AllPriv - 1，覆盖 AllPriv 之前定义的全部静态权限位。
pub const AllPrivMask: PrivilegeType = PrivilegeType(AllPriv.0 - 1);

/// Privileges 对应 Go 的 []PrivilegeType。
pub type Privileges = Vec<PrivilegeType>;

/// PrivilegesExt 承载 Go slice 的 Has 方法；线性 contains 与 slices.Contains 语义一致。
pub trait PrivilegesExt {
    fn Has(&self, privilege: PrivilegeType) -> bool;
}

impl PrivilegesExt for Privileges {
    fn Has(&self, privilege: PrivilegeType) -> bool {
        self.contains(&privilege)
    }
}

/// AllGlobalPrivs 返回全局作用域的全部静态权限，顺序与 Go 列表一致。
pub fn AllGlobalPrivs() -> Privileges {
    vec![
        SelectPriv,
        InsertPriv,
        UpdatePriv,
        DeletePriv,
        CreatePriv,
        DropPriv,
        ProcessPriv,
        ReferencesPriv,
        AlterPriv,
        ShowDBPriv,
        SuperPriv,
        ExecutePriv,
        IndexPriv,
        CreateUserPriv,
        CreateTablespacePriv,
        TriggerPriv,
        CreateViewPriv,
        ShowViewPriv,
        CreateRolePriv,
        DropRolePriv,
        CreateTMPTablePriv,
        LockTablesPriv,
        CreateRoutinePriv,
        AlterRoutinePriv,
        EventPriv,
        ShutdownPriv,
        ReloadPriv,
        FilePriv,
        ConfigPriv,
        ReplicationClientPriv,
        ReplicationSlavePriv,
    ]
}

/// AllDBPrivs 返回数据库作用域权限。
pub fn AllDBPrivs() -> Privileges {
    vec![
        SelectPriv,
        InsertPriv,
        UpdatePriv,
        DeletePriv,
        CreatePriv,
        DropPriv,
        ReferencesPriv,
        LockTablesPriv,
        CreateTMPTablePriv,
        EventPriv,
        CreateRoutinePriv,
        AlterRoutinePriv,
        AlterPriv,
        ExecutePriv,
        IndexPriv,
        CreateViewPriv,
        ShowViewPriv,
        TriggerPriv,
    ]
}

/// AllTablePrivs 返回表作用域权限。
pub fn AllTablePrivs() -> Privileges {
    vec![
        SelectPriv,
        InsertPriv,
        UpdatePriv,
        DeletePriv,
        CreatePriv,
        DropPriv,
        IndexPriv,
        ReferencesPriv,
        AlterPriv,
        CreateViewPriv,
        ShowViewPriv,
        TriggerPriv,
    ]
}

/// AllColumnPrivs 返回列作用域权限。
pub fn AllColumnPrivs() -> Privileges {
    vec![SelectPriv, InsertPriv, UpdatePriv, ReferencesPriv]
}

/// StaticGlobalOnlyPrivs 返回仅能用于全局作用域、且区别于动态权限的静态权限。
pub fn StaticGlobalOnlyPrivs() -> Privileges {
    vec![
        ProcessPriv,
        ShowDBPriv,
        SuperPriv,
        CreateUserPriv,
        CreateTablespacePriv,
        ShutdownPriv,
        ReloadPriv,
        FilePriv,
        ReplicationClientPriv,
        ReplicationSlavePriv,
        ConfigPriv,
    ]
}
