// Copyright 2026 AsterSQL.

// Copyright 2015 PingCAP, Inc.
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

use super::super::*;
use super::{Context, Rhs};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SecurityRule {
    RenameUserStmtAlt01,
    UserToUserListAlt01,
    UserToUserListAlt02,
    UserToUserAlt01,
    DropUserStmtAlt01,
    DropUserStmtAlt02,
    DropRoleStmtAlt01,
    DropRoleStmtAlt02,
    OptionLevelAlt01,
    OptionLevelAlt02,
    OptionLevelAlt03,
    UserVariableListAlt01,
    UserVariableListAlt02,
    SetRoleStmtAlt01,
    SetDefaultRoleStmtAlt01,
    SetDefaultRoleOptAlt01,
    SetDefaultRoleOptAlt02,
    SetDefaultRoleOptAlt03,
    SetRoleOptAlt01,
    SetRoleOptAlt03,
    UsernameAlt01,
    UsernameAlt02,
    UsernameAlt03,
    UsernameAlt04,
    UsernameListAlt01,
    UsernameListAlt02,
    PasswordOptAlt02,
    RolenameComposedAlt01,
    RolenameComposedAlt02,
    RolenameWithoutIdentAlt01,
    RolenameWithoutIdentAlt02,
    RolenameAlt01,
    RolenameAlt02,
    RolenameListAlt01,
    RolenameListAlt02,
    BDRRoleAlt01,
    BDRRoleAlt02,
    CreateUserStmtAlt01,
    CreateRoleStmtAlt01,
    AlterUserStmtAlt01,
    AlterUserStmtAlt02,
    AlterUserStmtAlt03,
    AlterUserStmtAlt04,
    UserSpecAlt01,
    UserSpecListAlt01,
    UserSpecListAlt02,
    AlterUserSpecAlt01,
    AlterUserSpecAlt02,
    AlterUserSpecAlt03,
    AlterUserSpecListAlt01,
    AlterUserSpecListAlt02,
    AuthOptionWithPasswordAlt01,
    AuthOptionWithPasswordAlt02,
    ConnectionOptionsAlt01,
    ConnectionOptionsAlt02,
    ConnectionOptionListAlt01,
    ConnectionOptionListAlt02,
    ConnectionOptionAlt01,
    ConnectionOptionAlt02,
    ConnectionOptionAlt03,
    ConnectionOptionAlt04,
    RequireClauseOptAlt01,
    RequireClauseAlt01,
    RequireClauseAlt02,
    RequireClauseAlt03,
    RequireClauseAlt04,
    RequireListAlt01,
    RequireListAlt02,
    RequireListAlt03,
    RequireListElementAlt01,
    RequireListElementAlt02,
    RequireListElementAlt03,
    RequireListElementAlt04,
    RequireListElementAlt05,
    CommentOrAttributeOptionAlt01,
    CommentOrAttributeOptionAlt02,
    CommentOrAttributeOptionAlt03,
    ResourceGroupNameOptionAlt01,
    ResourceGroupNameOptionAlt02,
    AlterPasswordOrLockOptionsAlt01,
    AlterPasswordOrLockOptionsAlt02,
    AlterPasswordOrLockOptionListAlt01,
    AlterPasswordOrLockOptionListAlt02,
    AlterPasswordOrLockOptionAlt01,
    AlterPasswordOrLockOptionAlt02,
    AlterPasswordOrLockOptionAlt03,
    AlterPasswordOrLockOptionAlt04,
    AlterPasswordOrLockOptionAlt05,
    AlterPasswordOrLockOptionAlt06,
    AlterPasswordOrLockOptionAlt07,
    AlterPasswordOrLockOptionAlt08,
    AlterPasswordOrLockOptionAlt09,
    AlterPasswordOrLockOptionAlt10,
    AlterPasswordOrLockOptionAlt11,
    AlterPasswordOrLockOptionAlt12,
    AlterPasswordOrLockOptionAlt13,
    AlterPasswordOrLockOptionAlt14,
    PasswordOrLockOptionsAlt01,
    PasswordOrLockOptionsAlt02,
    PasswordOrLockOptionListAlt01,
    PasswordOrLockOptionListAlt02,
    PasswordOrLockOptionAlt01,
    AuthOptionAlt01,
    AuthOptionAlt02,
    AuthOptionAlt03,
    AuthOptionAlt04,
    AuthOptionAlt05,
    AuthOptionAlt06,
    RoleSpecAlt01,
    RoleSpecListAlt01,
    RoleSpecListAlt02,
    StringLitOrUserVariableListAlt01,
    StringLitOrUserVariableListAlt02,
    StringLitOrUserVariableAlt01,
    StringLitOrUserVariableAlt02,
    GrantProxyStmtAlt01,
    GrantRoleStmtAlt01,
    WithGrantOptionOptAlt01,
    WithGrantOptionOptAlt02,
    WithGrantOptionOptAlt03,
    WithGrantOptionOptAlt04,
    WithGrantOptionOptAlt05,
    WithGrantOptionOptAlt06,
    ExtendedPrivAlt01,
    ExtendedPrivAlt02,
    RoleOrPrivElemAlt01,
    RoleOrPrivElemAlt02,
    RoleOrPrivElemAlt03,
    RoleOrPrivElemAlt04,
    RoleOrPrivElemAlt05,
    RoleOrPrivElemListAlt01,
    RoleOrPrivElemListAlt02,
    PrivElemAlt01,
    PrivElemAlt02,
    PrivTypeAlt01,
    PrivTypeAlt02,
    PrivTypeAlt03,
    PrivTypeAlt04,
    PrivTypeAlt05,
    PrivTypeAlt06,
    PrivTypeAlt07,
    PrivTypeAlt08,
    PrivTypeAlt09,
    PrivTypeAlt10,
    PrivTypeAlt11,
    PrivTypeAlt12,
    PrivTypeAlt13,
    PrivTypeAlt14,
    PrivTypeAlt15,
    PrivTypeAlt16,
    PrivTypeAlt17,
    PrivTypeAlt18,
    PrivTypeAlt19,
    PrivTypeAlt20,
    PrivTypeAlt21,
    PrivTypeAlt22,
    PrivTypeAlt23,
    PrivTypeAlt24,
    PrivTypeAlt25,
    PrivTypeAlt26,
    PrivTypeAlt27,
    PrivTypeAlt28,
    PrivTypeAlt29,
    PrivTypeAlt30,
    PrivTypeAlt31,
    PrivTypeAlt32,
    PrivTypeAlt33,
    PrivTypeAlt34,
    PrivTypeAlt35,
    PrivTypeAlt36,
    ObjectTypeAlt01,
    ObjectTypeAlt02,
    ObjectTypeAlt03,
    ObjectTypeAlt04,
    PrivLevelAlt01,
    PrivLevelAlt02,
    PrivLevelAlt03,
    PrivLevelAlt04,
    PrivLevelAlt05,
    RevokeRoleStmtAlt01,
    EncryptionOptAlt01,
}

fn identify(rule_id: RuleId) -> Option<SecurityRule> {
    Some(match rule_id.as_str() {
        "alterpasswordorlockoption_account_lock--88b011fd14fc80c6" => {
            SecurityRule::AlterPasswordOrLockOptionAlt02
        }
        "alterpasswordorlockoption_account_unlock--abbd6f64276f4a97" => {
            SecurityRule::AlterPasswordOrLockOptionAlt01
        }
        "alterpasswordorlockoption_failed_login_attempts--43ff16dd1c39e1bb" => {
            SecurityRule::AlterPasswordOrLockOptionAlt11
        }
        "alterpasswordorlockoption_password_expire--ddc85400ec1c6d98" => {
            SecurityRule::AlterPasswordOrLockOptionAlt07
        }
        "alterpasswordorlockoption_password_expire_defaul--1d958c5cbe5bcad3" => {
            SecurityRule::AlterPasswordOrLockOptionAlt10
        }
        "alterpasswordorlockoption_password_expire_interv--79a9abd83564e21a" => {
            SecurityRule::AlterPasswordOrLockOptionAlt08
        }
        "alterpasswordorlockoption_password_expire_never--56775ca09a897aee" => {
            SecurityRule::AlterPasswordOrLockOptionAlt09
        }
        "alterpasswordorlockoption_password_history_defau--a96ec790c956c10c" => {
            SecurityRule::AlterPasswordOrLockOptionAlt03
        }
        "alterpasswordorlockoption_password_history_num--aebd3eef4a05b1db" => {
            SecurityRule::AlterPasswordOrLockOptionAlt04
        }
        "alterpasswordorlockoption_password_lock_time_int--fb6a3dca2d49ce86" => {
            SecurityRule::AlterPasswordOrLockOptionAlt12
        }
        "alterpasswordorlockoption_password_lock_time_unb--e912eecc5a1dd9a5" => {
            SecurityRule::AlterPasswordOrLockOptionAlt13
        }
        "alterpasswordorlockoption_password_require_curre--0d1435e5cca3b55a" => {
            SecurityRule::AlterPasswordOrLockOptionAlt14
        }
        "alterpasswordorlockoption_password_reuse_interva--1c380f933edc5630" => {
            SecurityRule::AlterPasswordOrLockOptionAlt06
        }
        "alterpasswordorlockoption_password_reuse_interva--ee4913ac0b4de829" => {
            SecurityRule::AlterPasswordOrLockOptionAlt05
        }
        "alterpasswordorlockoptionlist_alterpasswordorloc--4173ebcc259c1870" => {
            SecurityRule::AlterPasswordOrLockOptionListAlt02
        }
        "alterpasswordorlockoptionlist_alterpasswordorloc--73eea367a811a492" => {
            SecurityRule::AlterPasswordOrLockOptionListAlt01
        }
        "alterpasswordorlockoptions--0bfe2e2ca9a178a6" => {
            SecurityRule::AlterPasswordOrLockOptionsAlt01
        }
        "alterpasswordorlockoptions_alterpasswordorlockop--bb31992a560dc6b3" => {
            SecurityRule::AlterPasswordOrLockOptionsAlt02
        }
        "alteruserspec_username_authoption--f45734539335b3c1" => SecurityRule::AlterUserSpecAlt01,
        "alteruserspec_username_authoptionwithpassword_re--519f85cb4d8e24cf" => {
            SecurityRule::AlterUserSpecAlt02
        }
        "alteruserspec_username_discard_old_password--78e65a5da5d53c22" => {
            SecurityRule::AlterUserSpecAlt03
        }
        "alteruserspeclist_alteruserspec--724a64c4678c226e" => SecurityRule::AlterUserSpecListAlt01,
        "alteruserspeclist_alteruserspeclist_alteruserspe--7b5e40d06c770ade" => {
            SecurityRule::AlterUserSpecListAlt02
        }
        "alteruserstmt_alter_user_ifexists_alteruserspecl--a24d64509298cd73" => {
            SecurityRule::AlterUserStmtAlt01
        }
        "alteruserstmt_alter_user_ifexists_user_discard_o--b68ba40f26c2634d" => {
            SecurityRule::AlterUserStmtAlt04
        }
        "alteruserstmt_alter_user_ifexists_user_identifie--376d1f3c68f7105f" => {
            SecurityRule::AlterUserStmtAlt03
        }
        "alteruserstmt_alter_user_ifexists_user_identifie--94973cf4c6cd44b8" => {
            SecurityRule::AlterUserStmtAlt02
        }
        "authoption--8248806a70478582" => SecurityRule::AuthOptionAlt01,
        "authoption_identified_by_authstring--bfc370d6cc1f538e" => SecurityRule::AuthOptionAlt02,
        "authoption_identified_by_password_hashstring--f1cd539c83839953" => {
            SecurityRule::AuthOptionAlt06
        }
        "authoption_identified_with_authplugin--316105124cb5fa25" => SecurityRule::AuthOptionAlt03,
        "authoption_identified_with_authplugin_as_hashstr--933e2ba920692618" => {
            SecurityRule::AuthOptionAlt05
        }
        "authoption_identified_with_authplugin_by_authstr--e3ba270faad6b5af" => {
            SecurityRule::AuthOptionAlt04
        }
        "authoptionwithpassword_identified_by_authstring--7e2e4d48e1002c87" => {
            SecurityRule::AuthOptionWithPasswordAlt01
        }
        "authoptionwithpassword_identified_with_authplugi--727b9125db2ff82e" => {
            SecurityRule::AuthOptionWithPasswordAlt02
        }
        "bdrrole_primary--6ea8f2be8b0c6898" => SecurityRule::BDRRoleAlt01,
        "bdrrole_secondary--48149fad5322f99c" => SecurityRule::BDRRoleAlt02,
        "commentorattributeoption--0cc1ca90ce72e450" => SecurityRule::CommentOrAttributeOptionAlt01,
        "commentorattributeoption_attribute_stringlit--6754d4b78479d0ed" => {
            SecurityRule::CommentOrAttributeOptionAlt03
        }
        "commentorattributeoption_comment_stringlit--fa40f25a4edc8fd2" => {
            SecurityRule::CommentOrAttributeOptionAlt02
        }
        "connectionoption_max_connections_per_hour_int64n--6ba2da6fe2dc5af9" => {
            SecurityRule::ConnectionOptionAlt03
        }
        "connectionoption_max_queries_per_hour_int64num--ac6948463ce1b16a" => {
            SecurityRule::ConnectionOptionAlt01
        }
        "connectionoption_max_updates_per_hour_int64num--41216b4f48602924" => {
            SecurityRule::ConnectionOptionAlt02
        }
        "connectionoption_max_user_connections_int64num--eb57e3348d49cd4c" => {
            SecurityRule::ConnectionOptionAlt04
        }
        "connectionoptionlist_connectionoption--fd916072df1d4dcc" => {
            SecurityRule::ConnectionOptionListAlt01
        }
        "connectionoptionlist_connectionoptionlist_connec--5f3de181ac6d5ff5" => {
            SecurityRule::ConnectionOptionListAlt02
        }
        "connectionoptions--5e046709e5749471" => SecurityRule::ConnectionOptionsAlt01,
        "connectionoptions_with_connectionoptionlist--9ad17f9d9d4d6271" => {
            SecurityRule::ConnectionOptionsAlt02
        }
        "createrolestmt_create_role_ifnotexists_rolespecl--99e8e65d9e9d8b25" => {
            SecurityRule::CreateRoleStmtAlt01
        }
        "createuserstmt_create_user_ifnotexists_userspecl--e0ab3487fdea6b20" => {
            SecurityRule::CreateUserStmtAlt01
        }
        "droprolestmt_drop_role_if_exists_rolenamelist--8146a1db78ad7a12" => {
            SecurityRule::DropRoleStmtAlt02
        }
        "droprolestmt_drop_role_rolenamelist--63cf2907b78be693" => SecurityRule::DropRoleStmtAlt01,
        "dropuserstmt_drop_user_if_exists_usernamelist--9848657a49561f29" => {
            SecurityRule::DropUserStmtAlt02
        }
        "dropuserstmt_drop_user_usernamelist--7b86a634a54d9340" => SecurityRule::DropUserStmtAlt01,
        "encryptionopt_stringlit--573865b0c7f36de2" => SecurityRule::EncryptionOptAlt01,
        "extendedpriv_extendedpriv_identifier--d586b80b2373e5f7" => SecurityRule::ExtendedPrivAlt02,
        "extendedpriv_identifier--13b1f7d8dab61487" => SecurityRule::ExtendedPrivAlt01,
        "grantproxystmt_grant_proxy_on_username_to_userna--f4a8327b4b1e43f2" => {
            SecurityRule::GrantProxyStmtAlt01
        }
        "grantrolestmt_grant_roleorprivelemlist_to_userna--014f7dad69376a9e" => {
            SecurityRule::GrantRoleStmtAlt01
        }
        "objecttype_function--f8c7c33971aca19f" => SecurityRule::ObjectTypeAlt03,
        "objecttype_prec_lowerthanfunction--53622b29ebb2383d" => SecurityRule::ObjectTypeAlt01,
        "objecttype_procedure--ea946761843d0f2a" => SecurityRule::ObjectTypeAlt04,
        "objecttype_table--d1318f10a626a6e1" => SecurityRule::ObjectTypeAlt02,
        "optionlevel_off--69497d1688ea2772" => SecurityRule::OptionLevelAlt01,
        "optionlevel_optional--ac2bb3d509088477" => SecurityRule::OptionLevelAlt02,
        "optionlevel_required--fab297c724d608f0" => SecurityRule::OptionLevelAlt03,
        "passwordopt_password_authstring--d5cd772e03006137" => SecurityRule::PasswordOptAlt02,
        "passwordorlockoption_alterpasswordorlockoption--a39e45e5a5bb1a3c" => {
            SecurityRule::PasswordOrLockOptionAlt01
        }
        "passwordorlockoptionlist_passwordorlockoption--8a1fb08252bc132c" => {
            SecurityRule::PasswordOrLockOptionListAlt01
        }
        "passwordorlockoptionlist_passwordorlockoptionlis--3e1208abd96e686e" => {
            SecurityRule::PasswordOrLockOptionListAlt02
        }
        "passwordorlockoptions--a7db8e1987024894" => SecurityRule::PasswordOrLockOptionsAlt01,
        "passwordorlockoptions_passwordorlockoptionlist--18a73ba424207427" => {
            SecurityRule::PasswordOrLockOptionsAlt02
        }
        "privelem_privtype--444d4a7ab8426e17" => SecurityRule::PrivElemAlt01,
        "privelem_privtype_columnnamelist--03a7cc2807e78f91" => SecurityRule::PrivElemAlt02,
        "privlevel--66d2ee0a855f7dbf" => SecurityRule::PrivLevelAlt01,
        "privlevel--7be66aada77462db" => SecurityRule::PrivLevelAlt02,
        "privlevel_identifier--0f91bb1e92f26658" => SecurityRule::PrivLevelAlt03,
        "privlevel_identifier--9d097a56458fce94" => SecurityRule::PrivLevelAlt05,
        "privlevel_identifier_identifier--6eeb0337e05b017d" => SecurityRule::PrivLevelAlt04,
        "privtype_all--fd3a63079f69aa6a" => SecurityRule::PrivTypeAlt01,
        "privtype_all_privileges--8546165a89e6940a" => SecurityRule::PrivTypeAlt02,
        "privtype_alter--1dc6652f293f3c13" => SecurityRule::PrivTypeAlt03,
        "privtype_alter_routine--6ac0471f983a6fc7" => SecurityRule::PrivTypeAlt34,
        "privtype_binlog_monitor--2d4ab747591e5c98" => SecurityRule::PrivTypeAlt22,
        "privtype_config--41190c667b923d37" => SecurityRule::PrivTypeAlt26,
        "privtype_create--b1b0fdf4a140ea55" => SecurityRule::PrivTypeAlt04,
        "privtype_create_role--2737e6815d4e00d3" => SecurityRule::PrivTypeAlt31,
        "privtype_create_routine--062a7e519ae50635" => SecurityRule::PrivTypeAlt33,
        "privtype_create_tablespace--29309f7f3a6a8541" => SecurityRule::PrivTypeAlt06,
        "privtype_create_temporary_tables--9238f0fb0e1b0f07" => SecurityRule::PrivTypeAlt27,
        "privtype_create_user--2d1704fbac183bf2" => SecurityRule::PrivTypeAlt05,
        "privtype_create_view--8964f965e02026b0" => SecurityRule::PrivTypeAlt29,
        "privtype_delete--0c0ad44dbc751950" => SecurityRule::PrivTypeAlt08,
        "privtype_drop--b23f06e80cab0962" => SecurityRule::PrivTypeAlt09,
        "privtype_drop_role--1f4ad6bd4ae19ec2" => SecurityRule::PrivTypeAlt32,
        "privtype_event--f318194a5fd8ad4f" => SecurityRule::PrivTypeAlt35,
        "privtype_execute--e9ad4cc9ebcef996" => SecurityRule::PrivTypeAlt11,
        "privtype_file--d990f50ff509b6fb" => SecurityRule::PrivTypeAlt25,
        "privtype_grant_option--074fcccc74831470" => SecurityRule::PrivTypeAlt18,
        "privtype_index--db02ebe68b601dd3" => SecurityRule::PrivTypeAlt12,
        "privtype_insert--d3b32285e7056a26" => SecurityRule::PrivTypeAlt13,
        "privtype_lock_tables--607170e38a627da3" => SecurityRule::PrivTypeAlt28,
        "privtype_process--aabef9e23166b2d0" => SecurityRule::PrivTypeAlt10,
        "privtype_references--22224838b7127663" => SecurityRule::PrivTypeAlt19,
        "privtype_reload--e8ad0c9b7ac49052" => SecurityRule::PrivTypeAlt24,
        "privtype_replication_client--1dd616e48a4b24ba" => SecurityRule::PrivTypeAlt21,
        "privtype_replication_slave--e3019d507cb360b0" => SecurityRule::PrivTypeAlt20,
        "privtype_select--de6de75ac8500df5" => SecurityRule::PrivTypeAlt14,
        "privtype_show_databases--f43ff549bc2578fc" => SecurityRule::PrivTypeAlt16,
        "privtype_show_view--7281fa84760e2f35" => SecurityRule::PrivTypeAlt30,
        "privtype_shutdown--759d4eebec286143" => SecurityRule::PrivTypeAlt36,
        "privtype_super--ab5db355bd5a517a" => SecurityRule::PrivTypeAlt15,
        "privtype_trigger--957ed174b417b4fb" => SecurityRule::PrivTypeAlt07,
        "privtype_update--2f6b7fe72255652a" => SecurityRule::PrivTypeAlt17,
        "privtype_usage--e9cfe2dc0564dad6" => SecurityRule::PrivTypeAlt23,
        "renameuserstmt_rename_user_usertouserlist--83f36f7936cfb9df" => {
            SecurityRule::RenameUserStmtAlt01
        }
        "requireclause_require_none--a6a612af66bf7bd1" => SecurityRule::RequireClauseAlt01,
        "requireclause_require_requirelist--347b2d5dafae2d16" => SecurityRule::RequireClauseAlt04,
        "requireclause_require_ssl--d29c0d111697d1c3" => SecurityRule::RequireClauseAlt02,
        "requireclause_require_x509--211b8970bc40b2a3" => SecurityRule::RequireClauseAlt03,
        "requireclauseopt--c8d0433c91cefb9e" => SecurityRule::RequireClauseOptAlt01,
        "requirelist_requirelist_and_requirelistelement--087badf729fdd4f0" => {
            SecurityRule::RequireListAlt02
        }
        "requirelist_requirelist_requirelistelement--e57d5678ce7ab2fd" => {
            SecurityRule::RequireListAlt03
        }
        "requirelist_requirelistelement--dca94a2271ba572c" => SecurityRule::RequireListAlt01,
        "requirelistelement_cipher_stringlit--8b673721f576a19a" => {
            SecurityRule::RequireListElementAlt03
        }
        "requirelistelement_issuer_stringlit--2b80ea1fd3918586" => {
            SecurityRule::RequireListElementAlt01
        }
        "requirelistelement_san_stringlit--4c32a8cf3eac3d9f" => {
            SecurityRule::RequireListElementAlt04
        }
        "requirelistelement_subject_stringlit--b86fea6e8f7efdd1" => {
            SecurityRule::RequireListElementAlt02
        }
        "requirelistelement_token_issuer_stringlit--87770db95ed5341c" => {
            SecurityRule::RequireListElementAlt05
        }
        "resourcegroupnameoption--af73453b70e3fada" => SecurityRule::ResourceGroupNameOptionAlt01,
        "resourcegroupnameoption_resource_group_resourceg--f9cea9043f015b7e" => {
            SecurityRule::ResourceGroupNameOptionAlt02
        }
        "revokerolestmt_revoke_roleorprivelemlist_from_us--e220d45c75158bf1" => {
            SecurityRule::RevokeRoleStmtAlt01
        }
        "rolename_rolenamecomposed--105bd256c464eb30" => SecurityRule::RolenameAlt02,
        "rolename_rolenamestring--853d7dec3d15fd51" => SecurityRule::RolenameAlt01,
        "rolenamecomposed_stringname_singleatidentifier--ebe99612ce70839d" => {
            SecurityRule::RolenameComposedAlt02
        }
        "rolenamecomposed_stringname_stringname--6e43be7785b52645" => {
            SecurityRule::RolenameComposedAlt01
        }
        "rolenamelist_rolename--eb2343d5b89bc054" => SecurityRule::RolenameListAlt01,
        "rolenamelist_rolenamelist_rolename--d1d163320aa73edb" => SecurityRule::RolenameListAlt02,
        "rolenamewithoutident_rolenamecomposed--e5ec4bd73ed45362" => {
            SecurityRule::RolenameWithoutIdentAlt02
        }
        "rolenamewithoutident_stringlit--aaedc381e7f6b689" => {
            SecurityRule::RolenameWithoutIdentAlt01
        }
        "roleorprivelem_extendedpriv--f33e5062463aea7b" => SecurityRule::RoleOrPrivElemAlt03,
        "roleorprivelem_load_from_s3--6a4d9fd39b115f39" => SecurityRule::RoleOrPrivElemAlt04,
        "roleorprivelem_privelem--00ae777e2df54d95" => SecurityRule::RoleOrPrivElemAlt01,
        "roleorprivelem_rolenamewithoutident--f2ecbde96a50fcba" => {
            SecurityRule::RoleOrPrivElemAlt02
        }
        "roleorprivelem_select_into_s3--f50d92c714d0cc73" => SecurityRule::RoleOrPrivElemAlt05,
        "roleorprivelemlist_roleorprivelem--4b2a30e0c9cf4738" => {
            SecurityRule::RoleOrPrivElemListAlt01
        }
        "roleorprivelemlist_roleorprivelemlist_roleorpriv--f97ddd9bd546eecb" => {
            SecurityRule::RoleOrPrivElemListAlt02
        }
        "rolespec_rolename--175468f29a4e31f0" => SecurityRule::RoleSpecAlt01,
        "rolespeclist_rolespec--dbd21d8aa20afe28" => SecurityRule::RoleSpecListAlt01,
        "rolespeclist_rolespeclist_rolespec--9b9feb3f1edfe007" => SecurityRule::RoleSpecListAlt02,
        "setdefaultroleopt_all--2ad6242cad2e1ca5" => SecurityRule::SetDefaultRoleOptAlt02,
        "setdefaultroleopt_none--3afb39eb4454126a" => SecurityRule::SetDefaultRoleOptAlt01,
        "setdefaultroleopt_rolenamelist--ffb9545d3ee6f25b" => SecurityRule::SetDefaultRoleOptAlt03,
        "setdefaultrolestmt_set_default_role_setdefaultro--5816768d67701939" => {
            SecurityRule::SetDefaultRoleStmtAlt01
        }
        "setroleopt_all_except_rolenamelist--dce43007c1a8ec48" => SecurityRule::SetRoleOptAlt01,
        "setroleopt_default--9d1e2b9e2bd390e4" => SecurityRule::SetRoleOptAlt03,
        "setrolestmt_set_role_setroleopt--06d527b953e1166d" => SecurityRule::SetRoleStmtAlt01,
        "stringlitoruservariable_stringlit--634fd44f7ead56e4" => {
            SecurityRule::StringLitOrUserVariableAlt01
        }
        "stringlitoruservariable_uservariable--9d321ea3569959a1" => {
            SecurityRule::StringLitOrUserVariableAlt02
        }
        "stringlitoruservariablelist_stringlitoruservaria--5103ab7e40520f62" => {
            SecurityRule::StringLitOrUserVariableListAlt02
        }
        "stringlitoruservariablelist_stringlitoruservaria--e422d1c21ded51fa" => {
            SecurityRule::StringLitOrUserVariableListAlt01
        }
        "username_current_user_optionalbraces--bb86ec80314b5f7d" => SecurityRule::UsernameAlt04,
        "username_stringname--b1d64d61eeffe25e" => SecurityRule::UsernameAlt01,
        "username_stringname_singleatidentifier--895fd9a32af839dc" => SecurityRule::UsernameAlt03,
        "username_stringname_stringname--152375c31be7fca8" => SecurityRule::UsernameAlt02,
        "usernamelist_username--7a9a2b5c0a518490" => SecurityRule::UsernameListAlt01,
        "usernamelist_usernamelist_username--9a56c55acdf69314" => SecurityRule::UsernameListAlt02,
        "userspec_username_authoption--9e87dc7c772707f3" => SecurityRule::UserSpecAlt01,
        "userspeclist_userspec--82c7657529bb2ca0" => SecurityRule::UserSpecListAlt01,
        "userspeclist_userspeclist_userspec--9e59d7a48c8f9190" => SecurityRule::UserSpecListAlt02,
        "usertouser_username_to_username--b317d80c48d89314" => SecurityRule::UserToUserAlt01,
        "usertouserlist_usertouser--9e7b19e90a6f208c" => SecurityRule::UserToUserListAlt01,
        "usertouserlist_usertouserlist_usertouser--ef13ea8e7bb11381" => {
            SecurityRule::UserToUserListAlt02
        }
        "uservariablelist_uservariable--58ed532bbc652acc" => SecurityRule::UserVariableListAlt01,
        "uservariablelist_uservariablelist_uservariable--83e75932770b11ef" => {
            SecurityRule::UserVariableListAlt02
        }
        "withgrantoptionopt--504fda6467cf9f99" => SecurityRule::WithGrantOptionOptAlt01,
        "withgrantoptionopt_with_grant_option--8c02a07249a42d11" => {
            SecurityRule::WithGrantOptionOptAlt02
        }
        "withgrantoptionopt_with_max_connections_per_hour--85efef42239b46c5" => {
            SecurityRule::WithGrantOptionOptAlt05
        }
        "withgrantoptionopt_with_max_queries_per_hour_num--ebf219ec5065097c" => {
            SecurityRule::WithGrantOptionOptAlt03
        }
        "withgrantoptionopt_with_max_updates_per_hour_num--923c729f6961bfc2" => {
            SecurityRule::WithGrantOptionOptAlt04
        }
        "withgrantoptionopt_with_max_user_connections_num--63d1b22ee3e0ae2a" => {
            SecurityRule::WithGrantOptionOptAlt06
        }
        _ => return None,
    })
}

pub(super) fn owns(rule_id: RuleId) -> bool {
    identify(rule_id).is_some()
}

fn role_from_semantic(value: RoleOrPrivSemantic) -> Option<auth::RoleIdentity> {
    match value {
        RoleOrPrivSemantic::Role(role) => Some(role),
        RoleOrPrivSemantic::Dynamic(username) => Some(auth::RoleIdentity {
            username,
            hostname: "%".to_owned(),
        }),
        RoleOrPrivSemantic::Priv(_) => None,
    }
}

pub(super) fn apply(
    rule_id: RuleId,
    rhs: Rhs<'_>,
    context: Context<'_>,
) -> Option<Result<bool, isize>> {
    Some(apply_rule(identify(rule_id)?, rhs, context))
}

fn apply_rule(rule: SecurityRule, mut rhs: Rhs<'_>, context: Context<'_>) -> Result<bool, isize> {
    let rhs_len = rhs.len();
    let Context {
        output: out,
        parser_state,
        lexer: yylex,
    } = context;
    match rule {
        SecurityRule::RenameUserStmtAlt01 => {
            let mappings = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::UserToUser>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::RenameUserStmt {
                node_text: Default::default(),
                UserToUsers: mappings,
            }));
        }
        SecurityRule::UserToUserListAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::UserToUser>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        SecurityRule::UserToUserListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::UserToUser>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::UserToUser>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        SecurityRule::UserToUserAlt01 => {
            let Some(old_user) = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<auth::UserIdentity>())
                .cloned()
            else {
                return Ok(false);
            };
            let Some(new_user) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<auth::UserIdentity>())
                .cloned()
            else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::UserToUser {
                OldUser: old_user,
                NewUser: new_user,
            }));
        }
        SecurityRule::DropUserStmtAlt01 | SecurityRule::DropUserStmtAlt02 => {
            let users = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<auth::UserIdentity>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::DropUserStmt {
                node_text: Default::default(),
                IsDropRole: false,
                IfExists: rule == SecurityRule::DropUserStmtAlt02,
                UserList: users,
            }));
        }
        SecurityRule::DropRoleStmtAlt01 | SecurityRule::DropRoleStmtAlt02 => {
            let roles = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<auth::RoleIdentity>>())
                .cloned()
                .unwrap_or_default();
            let users = roles
                .into_iter()
                .map(|role| auth::UserIdentity {
                    username: role.username,
                    hostname: role.hostname,
                    ..Default::default()
                })
                .collect();
            out.statement = Some(Box::new(parser_ast::DropUserStmt {
                node_text: Default::default(),
                IsDropRole: true,
                IfExists: rule == SecurityRule::DropRoleStmtAlt02,
                UserList: users,
            }));
        }
        SecurityRule::OptionLevelAlt01 => out.item = Some(Box::new(0u64)),
        SecurityRule::OptionLevelAlt02 => out.item = Some(Box::new(2u64)),
        SecurityRule::OptionLevelAlt03 => out.item = Some(Box::new(1u64)),
        SecurityRule::WithGrantOptionOptAlt01
        | SecurityRule::WithGrantOptionOptAlt02
        | SecurityRule::WithGrantOptionOptAlt03
        | SecurityRule::WithGrantOptionOptAlt04
        | SecurityRule::WithGrantOptionOptAlt05
        | SecurityRule::WithGrantOptionOptAlt06 => {
            out.item = Some(Box::new(rule == SecurityRule::WithGrantOptionOptAlt02))
        }
        SecurityRule::ExtendedPrivAlt01 => {
            out.item = Some(Box::new(vec![rhs[rhs_len - (0)].ident.clone()]))
        }
        SecurityRule::ExtendedPrivAlt02 => {
            let mut values = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<String>>())
                .cloned()
                .unwrap_or_default();
            values.push(rhs[rhs_len - (0)].ident.clone());
            out.item = Some(Box::new(values));
        }
        SecurityRule::PrivElemAlt01 | SecurityRule::PrivElemAlt02 => {
            let priv_back = if rule == SecurityRule::PrivElemAlt01 {
                0
            } else {
                3
            };
            let Some(privilege) = rhs[rhs_len - (priv_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_mysql::privs::PrivilegeType>())
                .copied()
            else {
                return Ok(false);
            };
            let columns = if rule == SecurityRule::PrivElemAlt02 {
                rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnName>>())
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            out.item = Some(Box::new(parser_ast::PrivElem {
                Priv: privilege,
                Cols: columns,
                Name: String::new(),
            }));
        }
        SecurityRule::PrivTypeAlt01
        | SecurityRule::PrivTypeAlt02
        | SecurityRule::PrivTypeAlt03
        | SecurityRule::PrivTypeAlt04
        | SecurityRule::PrivTypeAlt05
        | SecurityRule::PrivTypeAlt06
        | SecurityRule::PrivTypeAlt07
        | SecurityRule::PrivTypeAlt08
        | SecurityRule::PrivTypeAlt09
        | SecurityRule::PrivTypeAlt10
        | SecurityRule::PrivTypeAlt11
        | SecurityRule::PrivTypeAlt12
        | SecurityRule::PrivTypeAlt13
        | SecurityRule::PrivTypeAlt14
        | SecurityRule::PrivTypeAlt15
        | SecurityRule::PrivTypeAlt16
        | SecurityRule::PrivTypeAlt17
        | SecurityRule::PrivTypeAlt18
        | SecurityRule::PrivTypeAlt19
        | SecurityRule::PrivTypeAlt20
        | SecurityRule::PrivTypeAlt21
        | SecurityRule::PrivTypeAlt22
        | SecurityRule::PrivTypeAlt23
        | SecurityRule::PrivTypeAlt24
        | SecurityRule::PrivTypeAlt25
        | SecurityRule::PrivTypeAlt26
        | SecurityRule::PrivTypeAlt27
        | SecurityRule::PrivTypeAlt28
        | SecurityRule::PrivTypeAlt29
        | SecurityRule::PrivTypeAlt30
        | SecurityRule::PrivTypeAlt31
        | SecurityRule::PrivTypeAlt32
        | SecurityRule::PrivTypeAlt33
        | SecurityRule::PrivTypeAlt34
        | SecurityRule::PrivTypeAlt35
        | SecurityRule::PrivTypeAlt36 => {
            let privilege = match rule {
                SecurityRule::PrivTypeAlt01 | SecurityRule::PrivTypeAlt02 => {
                    parser_mysql::privs::AllPriv
                }
                SecurityRule::PrivTypeAlt03 => parser_mysql::privs::AlterPriv,
                SecurityRule::PrivTypeAlt04 => parser_mysql::privs::CreatePriv,
                SecurityRule::PrivTypeAlt05 => parser_mysql::privs::CreateUserPriv,
                SecurityRule::PrivTypeAlt06 => parser_mysql::privs::CreateTablespacePriv,
                SecurityRule::PrivTypeAlt07 => parser_mysql::privs::TriggerPriv,
                SecurityRule::PrivTypeAlt08 => parser_mysql::privs::DeletePriv,
                SecurityRule::PrivTypeAlt09 => parser_mysql::privs::DropPriv,
                SecurityRule::PrivTypeAlt10 => parser_mysql::privs::ProcessPriv,
                SecurityRule::PrivTypeAlt11 => parser_mysql::privs::ExecutePriv,
                SecurityRule::PrivTypeAlt12 => parser_mysql::privs::IndexPriv,
                SecurityRule::PrivTypeAlt13 => parser_mysql::privs::InsertPriv,
                SecurityRule::PrivTypeAlt14 => parser_mysql::privs::SelectPriv,
                SecurityRule::PrivTypeAlt15 => parser_mysql::privs::SuperPriv,
                SecurityRule::PrivTypeAlt16 => parser_mysql::privs::ShowDBPriv,
                SecurityRule::PrivTypeAlt17 => parser_mysql::privs::UpdatePriv,
                SecurityRule::PrivTypeAlt18 => parser_mysql::privs::GrantPriv,
                SecurityRule::PrivTypeAlt19 => parser_mysql::privs::ReferencesPriv,
                SecurityRule::PrivTypeAlt20 => parser_mysql::privs::ReplicationSlavePriv,
                SecurityRule::PrivTypeAlt21 | SecurityRule::PrivTypeAlt22 => {
                    parser_mysql::privs::ReplicationClientPriv
                }
                SecurityRule::PrivTypeAlt23 => parser_mysql::privs::UsagePriv,
                SecurityRule::PrivTypeAlt24 => parser_mysql::privs::ReloadPriv,
                SecurityRule::PrivTypeAlt25 => parser_mysql::privs::FilePriv,
                SecurityRule::PrivTypeAlt26 => parser_mysql::privs::ConfigPriv,
                SecurityRule::PrivTypeAlt27 => parser_mysql::privs::CreateTMPTablePriv,
                SecurityRule::PrivTypeAlt28 => parser_mysql::privs::LockTablesPriv,
                SecurityRule::PrivTypeAlt29 => parser_mysql::privs::CreateViewPriv,
                SecurityRule::PrivTypeAlt30 => parser_mysql::privs::ShowViewPriv,
                SecurityRule::PrivTypeAlt31 => parser_mysql::privs::CreateRolePriv,
                SecurityRule::PrivTypeAlt32 => parser_mysql::privs::DropRolePriv,
                SecurityRule::PrivTypeAlt33 => parser_mysql::privs::CreateRoutinePriv,
                SecurityRule::PrivTypeAlt34 => parser_mysql::privs::AlterRoutinePriv,
                SecurityRule::PrivTypeAlt35 => parser_mysql::privs::EventPriv,
                _ => parser_mysql::privs::ShutdownPriv,
            };
            if rule == SecurityRule::PrivTypeAlt22 && !parser_state.enableMariaDB {
                yylex.AppendError(yylex.Errorf("syntax error", &[]));
                return Err(1);
            }
            out.item = Some(Box::new(privilege));
        }
        SecurityRule::ObjectTypeAlt01
        | SecurityRule::ObjectTypeAlt02
        | SecurityRule::ObjectTypeAlt03
        | SecurityRule::ObjectTypeAlt04 => {
            out.item = Some(Box::new(match rule {
                SecurityRule::ObjectTypeAlt01 => parser_ast::ObjectTypeType::None,
                SecurityRule::ObjectTypeAlt02 => parser_ast::ObjectTypeType::Table,
                SecurityRule::ObjectTypeAlt03 => parser_ast::ObjectTypeType::Function,
                _ => parser_ast::ObjectTypeType::Procedure,
            }))
        }
        SecurityRule::PrivLevelAlt01
        | SecurityRule::PrivLevelAlt02
        | SecurityRule::PrivLevelAlt03
        | SecurityRule::PrivLevelAlt04
        | SecurityRule::PrivLevelAlt05 => {
            out.item = Some(Box::new(match rule {
                SecurityRule::PrivLevelAlt01 => parser_ast::GrantLevel {
                    Level: parser_ast::GrantLevelType::DB,
                    ..Default::default()
                },
                SecurityRule::PrivLevelAlt02 => parser_ast::GrantLevel {
                    Level: parser_ast::GrantLevelType::Global,
                    ..Default::default()
                },
                SecurityRule::PrivLevelAlt03 => parser_ast::GrantLevel {
                    Level: parser_ast::GrantLevelType::DB,
                    DBName: rhs[rhs_len - (2)].ident.clone(),
                    ..Default::default()
                },
                SecurityRule::PrivLevelAlt04 => parser_ast::GrantLevel {
                    Level: parser_ast::GrantLevelType::Table,
                    DBName: rhs[rhs_len - (2)].ident.clone(),
                    TableName: rhs[rhs_len - (0)].ident.clone(),
                },
                _ => parser_ast::GrantLevel {
                    Level: parser_ast::GrantLevelType::Table,
                    TableName: rhs[rhs_len - (0)].ident.clone(),
                    ..Default::default()
                },
            }))
        }
        SecurityRule::UserVariableListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .expr
                    .clone()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        SecurityRule::UserVariableListAlt02 => {
            let mut vars = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                .cloned()
                .unwrap_or_default();
            if let Some(var) = rhs[rhs_len - (0)].expr.clone() {
                vars.push(var);
            }
            out.item = Some(Box::new(vars));
        }
        SecurityRule::SetRoleStmtAlt01 => {
            out.statement = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::SetRoleStmt>().ok())
                .map(|item| item as Box<dyn parser_ast::Node>)
        }
        SecurityRule::SetDefaultRoleStmtAlt01 => {
            let Some(role) = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SetRoleStmt>())
                .cloned()
            else {
                return Ok(false);
            };
            let users = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| {
                    item.downcast_ref::<Vec<parser_auth::parser::auth::auth::UserIdentity>>()
                })
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::SetDefaultRoleStmt {
                node_text: Default::default(),
                SetRoleOpt: role.SetRoleOpt,
                RoleList: role.RoleList,
                UserList: users,
            }));
        }
        SecurityRule::SetDefaultRoleOptAlt01
        | SecurityRule::SetDefaultRoleOptAlt02
        | SecurityRule::SetDefaultRoleOptAlt03
        | SecurityRule::SetRoleOptAlt01
        | SecurityRule::SetRoleOptAlt03 => {
            let opt = match rule {
                SecurityRule::SetDefaultRoleOptAlt01 => parser_ast::SetRoleOpt::None,
                SecurityRule::SetDefaultRoleOptAlt02 => parser_ast::SetRoleOpt::All,
                SecurityRule::SetDefaultRoleOptAlt03 => parser_ast::SetRoleOpt::Regular,
                SecurityRule::SetRoleOptAlt01 => parser_ast::SetRoleOpt::AllExcept,
                _ => parser_ast::SetRoleOpt::Default,
            };
            let roles = if matches!(
                rule,
                SecurityRule::SetDefaultRoleOptAlt03 | SecurityRule::SetRoleOptAlt01
            ) {
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| {
                        item.downcast_ref::<Vec<parser_auth::parser::auth::auth::RoleIdentity>>()
                    })
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            out.item = Some(Box::new(parser_ast::SetRoleStmt {
                node_text: Default::default(),
                SetRoleOpt: opt,
                RoleList: roles,
            }));
        }
        SecurityRule::BDRRoleAlt01 | SecurityRule::BDRRoleAlt02 => {
            out.item = Some(Box::new(if rule == SecurityRule::BDRRoleAlt01 {
                parser_ast::BDRRole::Primary
            } else {
                parser_ast::BDRRole::Secondary
            }))
        }
        SecurityRule::UsernameAlt01 => {
            out.item = Some(Box::new(auth::UserIdentity {
                username: rhs[rhs_len - (0)].ident.clone(),
                hostname: "%".to_owned(),
                ..Default::default()
            }))
        }
        SecurityRule::UsernameAlt02 => {
            out.item = Some(Box::new(auth::UserIdentity {
                username: rhs[rhs_len - (2)].ident.clone(),
                hostname: rhs[rhs_len - (0)].ident.to_lowercase(),
                ..Default::default()
            }))
        }
        SecurityRule::UsernameAlt03 => {
            out.item = Some(Box::new(auth::UserIdentity {
                username: rhs[rhs_len - (1)].ident.clone(),
                hostname: rhs[rhs_len - (0)]
                    .ident
                    .trim_start_matches('@')
                    .to_lowercase(),
                ..Default::default()
            }))
        }
        SecurityRule::UsernameAlt04 => {
            out.item = Some(Box::new(auth::UserIdentity {
                current_user: true,
                ..Default::default()
            }))
        }
        SecurityRule::UsernameListAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<auth::UserIdentity>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        SecurityRule::UsernameListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<auth::UserIdentity>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<auth::UserIdentity>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        SecurityRule::PasswordOptAlt02 => out.ident = rhs[rhs_len - (1)].ident.clone(),
        SecurityRule::RolenameComposedAlt01 => {
            out.item = Some(Box::new(auth::RoleIdentity {
                username: rhs[rhs_len - (2)].ident.clone(),
                hostname: rhs[rhs_len - (0)].ident.to_lowercase(),
            }))
        }
        SecurityRule::RolenameComposedAlt02 => {
            out.item = Some(Box::new(auth::RoleIdentity {
                username: rhs[rhs_len - (1)].ident.clone(),
                hostname: rhs[rhs_len - (0)]
                    .ident
                    .trim_start_matches('@')
                    .to_lowercase(),
            }))
        }
        SecurityRule::RolenameWithoutIdentAlt01 | SecurityRule::RolenameAlt01 => {
            out.item = Some(Box::new(auth::RoleIdentity {
                username: rhs[rhs_len - (0)].ident.clone(),
                hostname: "%".to_owned(),
            }))
        }
        SecurityRule::RolenameWithoutIdentAlt02 | SecurityRule::RolenameAlt02 => {
            out.item = rhs[rhs_len - (0)].item.take()
        }
        SecurityRule::RolenameListAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<auth::RoleIdentity>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        SecurityRule::RolenameListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<auth::RoleIdentity>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<auth::RoleIdentity>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        SecurityRule::CreateUserStmtAlt01
        | SecurityRule::CreateRoleStmtAlt01
        | SecurityRule::AlterUserStmtAlt01 => {
            let (if_back, specs_back, tls_back, resource_back, password_back, create_role) =
                match rule {
                    SecurityRule::CreateUserStmtAlt01 => (6, 5, Some(4), Some(3), Some(2), false),
                    SecurityRule::CreateRoleStmtAlt01 => (1, 0, None, None, None, true),
                    _ => (6, 5, Some(4), Some(3), Some(2), false),
                };
            let if_flag = rhs[rhs_len - (if_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            let specs = rhs[rhs_len - (specs_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::UserSpec>>())
                .cloned()
                .unwrap_or_default();
            let tls = tls_back
                .and_then(|back| {
                    rhs[rhs_len - (back)]
                        .item
                        .as_deref()
                        .and_then(|item| {
                            item.downcast_ref::<Vec<parser_ast::AuthTokenOrTLSOption>>()
                        })
                        .cloned()
                })
                .unwrap_or_default();
            let resources = resource_back
                .and_then(|back| {
                    rhs[rhs_len - (back)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::ResourceOption>>())
                        .cloned()
                })
                .unwrap_or_default();
            let passwords = password_back
                .and_then(|back| {
                    rhs[rhs_len - (back)]
                        .item
                        .as_deref()
                        .and_then(|item| {
                            item.downcast_ref::<Vec<parser_ast::PasswordOrLockOption>>()
                        })
                        .cloned()
                })
                .unwrap_or_default();
            let metadata = if rule != SecurityRule::CreateRoleStmtAlt01 {
                rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::CommentOrAttributeOption>())
                    .cloned()
            } else {
                None
            };
            let group = if rule != SecurityRule::CreateRoleStmtAlt01 {
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ResourceGroupNameOption>())
                    .cloned()
            } else {
                None
            };
            if rule == SecurityRule::AlterUserStmtAlt01 {
                out.statement = Some(Box::new(parser_ast::AlterUserStmt {
                    IfExists: if_flag,
                    Specs: specs,
                    AuthTokenOrTLSOptions: tls,
                    ResourceOptions: resources,
                    PasswordOrLockOptions: passwords,
                    CommentOrAttributeOption: metadata,
                    ResourceGroupNameOption: group,
                    ..Default::default()
                }));
            } else {
                out.statement = Some(Box::new(parser_ast::CreateUserStmt {
                    node_text: Default::default(),
                    IsCreateRole: create_role,
                    IfNotExists: if_flag,
                    Specs: specs,
                    AuthTokenOrTLSOptions: tls,
                    ResourceOptions: resources,
                    PasswordOrLockOptions: passwords,
                    CommentOrAttributeOption: metadata,
                    ResourceGroupNameOption: group,
                }));
            }
        }
        SecurityRule::AlterUserStmtAlt02
        | SecurityRule::AlterUserStmtAlt03
        | SecurityRule::AlterUserStmtAlt04 => {
            let (if_back, auth, dual) = match rule {
                SecurityRule::AlterUserStmtAlt02 => (
                    6,
                    Some(parser_ast::AuthOption {
                        AuthString: rhs[rhs_len - (0)].ident.clone(),
                        ByAuthString: true,
                        ..Default::default()
                    }),
                    parser_ast::DualPasswordOptionType::None,
                ),
                SecurityRule::AlterUserStmtAlt03 => (
                    9,
                    Some(parser_ast::AuthOption {
                        AuthString: rhs[rhs_len - (3)].ident.clone(),
                        ByAuthString: true,
                        ..Default::default()
                    }),
                    parser_ast::DualPasswordOptionType::RetainCurrent,
                ),
                _ => (6, None, parser_ast::DualPasswordOptionType::DiscardOld),
            };
            out.statement = Some(Box::new(parser_ast::AlterUserStmt {
                IfExists: rhs[rhs_len - (if_back)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                CurrentAuth: auth,
                CurrentDualPasswordOption: dual,
                ..Default::default()
            }));
        }
        SecurityRule::UserSpecAlt01
        | SecurityRule::AlterUserSpecAlt01
        | SecurityRule::AlterUserSpecAlt02
        | SecurityRule::AlterUserSpecAlt03 => {
            let (user_back, auth_back, dual) = match rule {
                SecurityRule::UserSpecAlt01 | SecurityRule::AlterUserSpecAlt01 => {
                    (1, Some(0), parser_ast::DualPasswordOptionType::None)
                }
                SecurityRule::AlterUserSpecAlt02 => (
                    4,
                    Some(3),
                    parser_ast::DualPasswordOptionType::RetainCurrent,
                ),
                _ => (3, None, parser_ast::DualPasswordOptionType::DiscardOld),
            };
            let Some(user) = rhs[rhs_len - (user_back)]
                .item
                .as_deref()
                .and_then(|item| {
                    item.downcast_ref::<parser_auth::parser::auth::auth::UserIdentity>()
                })
                .cloned()
            else {
                return Ok(false);
            };
            let auth = auth_back.and_then(|back| {
                rhs[rhs_len - (back)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::AuthOption>())
                    .cloned()
            });
            out.item = Some(Box::new(parser_ast::UserSpec {
                User: user,
                AuthOpt: auth,
                DualPasswordOption: dual,
                IsRole: false,
            }));
        }
        SecurityRule::UserSpecListAlt01 | SecurityRule::AlterUserSpecListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::UserSpec>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        SecurityRule::UserSpecListAlt02 | SecurityRule::AlterUserSpecListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::UserSpec>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::UserSpec>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        SecurityRule::AuthOptionWithPasswordAlt01
        | SecurityRule::AuthOptionWithPasswordAlt02
        | SecurityRule::AuthOptionAlt02
        | SecurityRule::AuthOptionAlt03
        | SecurityRule::AuthOptionAlt04
        | SecurityRule::AuthOptionAlt05
        | SecurityRule::AuthOptionAlt06 => {
            let mut auth = parser_ast::AuthOption::default();
            match rule {
                SecurityRule::AuthOptionWithPasswordAlt01 | SecurityRule::AuthOptionAlt02 => {
                    auth.AuthString = rhs[rhs_len - (0)].ident.clone();
                    auth.ByAuthString = true;
                }
                SecurityRule::AuthOptionWithPasswordAlt02 | SecurityRule::AuthOptionAlt04 => {
                    auth.AuthPlugin = rhs[rhs_len - (2)].ident.clone();
                    auth.AuthString = rhs[rhs_len - (0)].ident.clone();
                    auth.ByAuthString = true;
                }
                SecurityRule::AuthOptionAlt03 => auth.AuthPlugin = rhs[rhs_len - (0)].ident.clone(),
                SecurityRule::AuthOptionAlt05 => {
                    auth.AuthPlugin = rhs[rhs_len - (2)].ident.clone();
                    auth.HashString = rhs[rhs_len - (0)].ident.clone();
                    auth.ByHashString = true;
                }
                _ => {
                    auth.AuthPlugin = parser_mysql::r#const::AuthNativePassword.to_owned();
                    auth.HashString = rhs[rhs_len - (0)].ident.clone();
                    auth.ByHashString = true;
                }
            }
            out.item = Some(Box::new(auth));
        }
        SecurityRule::ConnectionOptionsAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::ResourceOption>::new()))
        }
        SecurityRule::ConnectionOptionsAlt02 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ResourceOption>>())
                .cloned()
                .unwrap_or_default();
            if values
                .iter()
                .any(|option| option.Type != parser_ast::ResourceOptionType::MaxUserConnections)
            {
                yylex.AppendError(yylex.Errorf("TiDB does not support WITH ConnectionOptions but MAX_USER_CONNECTIONS now, they would be parsed but ignored.", &[]));
                yylex.LastErrorAsWarn();
            }
            out.item = Some(Box::new(values));
        }
        SecurityRule::ConnectionOptionListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ResourceOption>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        SecurityRule::ConnectionOptionListAlt02 => {
            let mut values = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ResourceOption>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ResourceOption>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        SecurityRule::ConnectionOptionAlt01
        | SecurityRule::ConnectionOptionAlt02
        | SecurityRule::ConnectionOptionAlt03
        | SecurityRule::ConnectionOptionAlt04 => {
            out.item = Some(Box::new(parser_ast::ResourceOption {
                Type: match rule {
                    SecurityRule::ConnectionOptionAlt01 => {
                        parser_ast::ResourceOptionType::MaxQueriesPerHour
                    }
                    SecurityRule::ConnectionOptionAlt02 => {
                        parser_ast::ResourceOptionType::MaxUpdatesPerHour
                    }
                    SecurityRule::ConnectionOptionAlt03 => {
                        parser_ast::ResourceOptionType::MaxConnectionsPerHour
                    }
                    _ => parser_ast::ResourceOptionType::MaxUserConnections,
                },
                Count: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default() as i64,
            }))
        }
        SecurityRule::RequireClauseOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::AuthTokenOrTLSOption>::new()))
        }
        SecurityRule::RequireClauseAlt01
        | SecurityRule::RequireClauseAlt02
        | SecurityRule::RequireClauseAlt03 => {
            out.item = Some(Box::new(vec![parser_ast::AuthTokenOrTLSOption {
                Type: match rule {
                    SecurityRule::RequireClauseAlt01 => {
                        parser_ast::AuthTokenOrTLSOptionType::TlsNone
                    }
                    SecurityRule::RequireClauseAlt02 => parser_ast::AuthTokenOrTLSOptionType::Ssl,
                    _ => parser_ast::AuthTokenOrTLSOptionType::X509,
                },
                ..Default::default()
            }]))
        }
        SecurityRule::RequireClauseAlt04 => out.item = rhs[rhs_len - (0)].item.take(),
        SecurityRule::RequireListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::AuthTokenOrTLSOption>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        SecurityRule::RequireListAlt02 | SecurityRule::RequireListAlt03 => {
            let list_back = if rule == SecurityRule::RequireListAlt02 {
                2
            } else {
                1
            };
            let mut values = rhs[rhs_len - (list_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::AuthTokenOrTLSOption>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::AuthTokenOrTLSOption>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        SecurityRule::RequireListElementAlt01
        | SecurityRule::RequireListElementAlt02
        | SecurityRule::RequireListElementAlt03
        | SecurityRule::RequireListElementAlt04
        | SecurityRule::RequireListElementAlt05 => {
            out.item = Some(Box::new(parser_ast::AuthTokenOrTLSOption {
                Type: match rule {
                    SecurityRule::RequireListElementAlt01 => {
                        parser_ast::AuthTokenOrTLSOptionType::Issuer
                    }
                    SecurityRule::RequireListElementAlt02 => {
                        parser_ast::AuthTokenOrTLSOptionType::Subject
                    }
                    SecurityRule::RequireListElementAlt03 => {
                        parser_ast::AuthTokenOrTLSOptionType::Cipher
                    }
                    SecurityRule::RequireListElementAlt04 => {
                        parser_ast::AuthTokenOrTLSOptionType::SAN
                    }
                    _ => parser_ast::AuthTokenOrTLSOptionType::TokenIssuer,
                },
                Value: rhs[rhs_len - (0)].ident.clone(),
            }))
        }
        SecurityRule::CommentOrAttributeOptionAlt01
        | SecurityRule::ResourceGroupNameOptionAlt01 => out.item = None,
        SecurityRule::CommentOrAttributeOptionAlt02
        | SecurityRule::CommentOrAttributeOptionAlt03 => {
            out.item = Some(Box::new(parser_ast::CommentOrAttributeOption {
                Type: if rule == SecurityRule::CommentOrAttributeOptionAlt02 {
                    parser_ast::CommentOrAttributeOptionType::UserComment
                } else {
                    parser_ast::CommentOrAttributeOptionType::UserAttribute
                },
                Value: rhs[rhs_len - (0)].ident.clone(),
            }))
        }
        SecurityRule::ResourceGroupNameOptionAlt02 => {
            out.item = Some(Box::new(parser_ast::ResourceGroupNameOption {
                Value: rhs[rhs_len - (0)].ident.clone(),
            }))
        }
        SecurityRule::AlterPasswordOrLockOptionsAlt01
        | SecurityRule::PasswordOrLockOptionsAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::PasswordOrLockOption>::new()))
        }
        SecurityRule::AlterPasswordOrLockOptionsAlt02
        | SecurityRule::PasswordOrLockOptionsAlt02
        | SecurityRule::PasswordOrLockOptionAlt01 => out.item = rhs[rhs_len - (0)].item.take(),
        SecurityRule::AlterPasswordOrLockOptionListAlt01
        | SecurityRule::PasswordOrLockOptionListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::PasswordOrLockOption>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        SecurityRule::AlterPasswordOrLockOptionListAlt02
        | SecurityRule::PasswordOrLockOptionListAlt02 => {
            let mut values = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::PasswordOrLockOption>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::PasswordOrLockOption>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        SecurityRule::AlterPasswordOrLockOptionAlt01
        | SecurityRule::AlterPasswordOrLockOptionAlt02
        | SecurityRule::AlterPasswordOrLockOptionAlt03
        | SecurityRule::AlterPasswordOrLockOptionAlt04
        | SecurityRule::AlterPasswordOrLockOptionAlt05
        | SecurityRule::AlterPasswordOrLockOptionAlt06
        | SecurityRule::AlterPasswordOrLockOptionAlt07
        | SecurityRule::AlterPasswordOrLockOptionAlt08
        | SecurityRule::AlterPasswordOrLockOptionAlt09
        | SecurityRule::AlterPasswordOrLockOptionAlt10
        | SecurityRule::AlterPasswordOrLockOptionAlt11
        | SecurityRule::AlterPasswordOrLockOptionAlt12
        | SecurityRule::AlterPasswordOrLockOptionAlt13
        | SecurityRule::AlterPasswordOrLockOptionAlt14 => {
            let tp = match rule {
                SecurityRule::AlterPasswordOrLockOptionAlt01 => {
                    parser_ast::PasswordOrLockOptionType::Unlock
                }
                SecurityRule::AlterPasswordOrLockOptionAlt02 => {
                    parser_ast::PasswordOrLockOptionType::Lock
                }
                SecurityRule::AlterPasswordOrLockOptionAlt03 => {
                    parser_ast::PasswordOrLockOptionType::PasswordHistoryDefault
                }
                SecurityRule::AlterPasswordOrLockOptionAlt04 => {
                    parser_ast::PasswordOrLockOptionType::PasswordHistory
                }
                SecurityRule::AlterPasswordOrLockOptionAlt05 => {
                    parser_ast::PasswordOrLockOptionType::PasswordReuseDefault
                }
                SecurityRule::AlterPasswordOrLockOptionAlt06 => {
                    parser_ast::PasswordOrLockOptionType::PasswordReuseInterval
                }
                SecurityRule::AlterPasswordOrLockOptionAlt07 => {
                    parser_ast::PasswordOrLockOptionType::PasswordExpire
                }
                SecurityRule::AlterPasswordOrLockOptionAlt08 => {
                    parser_ast::PasswordOrLockOptionType::PasswordExpireInterval
                }
                SecurityRule::AlterPasswordOrLockOptionAlt09 => {
                    parser_ast::PasswordOrLockOptionType::PasswordExpireNever
                }
                SecurityRule::AlterPasswordOrLockOptionAlt10 => {
                    parser_ast::PasswordOrLockOptionType::PasswordExpireDefault
                }
                SecurityRule::AlterPasswordOrLockOptionAlt11 => {
                    parser_ast::PasswordOrLockOptionType::FailedLoginAttempts
                }
                SecurityRule::AlterPasswordOrLockOptionAlt12 => {
                    parser_ast::PasswordOrLockOptionType::PasswordLockTime
                }
                SecurityRule::AlterPasswordOrLockOptionAlt13 => {
                    parser_ast::PasswordOrLockOptionType::PasswordLockTimeUnbounded
                }
                _ => parser_ast::PasswordOrLockOptionType::PasswordRequireCurrentDefault,
            };
            let count_back = if matches!(
                rule,
                SecurityRule::AlterPasswordOrLockOptionAlt06
                    | SecurityRule::AlterPasswordOrLockOptionAlt08
            ) {
                1
            } else {
                0
            };
            let count = if matches!(
                rule,
                SecurityRule::AlterPasswordOrLockOptionAlt04
                    | SecurityRule::AlterPasswordOrLockOptionAlt06
                    | SecurityRule::AlterPasswordOrLockOptionAlt08
                    | SecurityRule::AlterPasswordOrLockOptionAlt11
                    | SecurityRule::AlterPasswordOrLockOptionAlt12
            ) {
                rhs[rhs_len - (count_back)]
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default() as i64
            } else {
                0
            };
            out.item = Some(Box::new(parser_ast::PasswordOrLockOption {
                Type: tp,
                Count: count,
            }));
        }
        SecurityRule::AuthOptionAlt01 => out.item = None,
        SecurityRule::RoleSpecAlt01 => {
            let Some(role) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| {
                    item.downcast_ref::<parser_auth::parser::auth::auth::RoleIdentity>()
                })
                .cloned()
            else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::UserSpec {
                User: parser_auth::parser::auth::auth::UserIdentity {
                    username: role.username,
                    hostname: role.hostname,
                    ..Default::default()
                },
                IsRole: true,
                ..Default::default()
            }));
        }
        SecurityRule::RoleSpecListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::UserSpec>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        SecurityRule::RoleSpecListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::UserSpec>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::UserSpec>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        SecurityRule::StringLitOrUserVariableListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::StringOrUserVar>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        SecurityRule::StringLitOrUserVariableListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::StringOrUserVar>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::StringOrUserVar>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        SecurityRule::StringLitOrUserVariableAlt01 => {
            out.item = Some(Box::new(parser_ast::StringOrUserVar {
                StringLit: rhs[rhs_len - (0)].ident.clone(),
                ..Default::default()
            }))
        }
        SecurityRule::StringLitOrUserVariableAlt02 => {
            out.item = Some(Box::new(parser_ast::StringOrUserVar {
                UserVar: rhs[rhs_len - (0)].expr.clone(),
                ..Default::default()
            }))
        }
        SecurityRule::RoleOrPrivElemAlt01 | SecurityRule::RoleOrPrivElemAlt02 => {
            let semantic = if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::PrivElem>())
                .cloned()
            {
                RoleOrPrivSemantic::Priv(value)
            } else if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<auth::RoleIdentity>())
                .cloned()
            {
                RoleOrPrivSemantic::Role(value)
            } else {
                yylex.AppendError(yylex.Errorf("invalid role or privilege", &[]));
                return Err(1);
            };
            out.item = Some(Box::new(semantic));
        }
        SecurityRule::RoleOrPrivElemAlt03
        | SecurityRule::RoleOrPrivElemAlt04
        | SecurityRule::RoleOrPrivElemAlt05 => {
            out.item = Some(Box::new(RoleOrPrivSemantic::Dynamic(match rule {
                SecurityRule::RoleOrPrivElemAlt03 => rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<String>>())
                    .cloned()
                    .unwrap_or_default()
                    .join(" "),
                SecurityRule::RoleOrPrivElemAlt04 => "LOAD FROM S3".to_owned(),
                _ => "SELECT INTO S3".to_owned(),
            })))
        }
        SecurityRule::RoleOrPrivElemListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<RoleOrPrivSemantic>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        SecurityRule::RoleOrPrivElemListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<RoleOrPrivSemantic>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<RoleOrPrivSemantic>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        SecurityRule::GrantProxyStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::GrantProxyStmt {
                node_text: Default::default(),
                LocalUser: rhs[rhs_len - (3)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<auth::UserIdentity>())
                    .cloned()
                    .unwrap_or_default(),
                ExternalUsers: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<auth::UserIdentity>>())
                    .cloned()
                    .unwrap_or_default(),
                WithGrant: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
            }))
        }
        SecurityRule::GrantRoleStmtAlt01 => {
            let values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<RoleOrPrivSemantic>>())
                .cloned()
                .unwrap_or_default();
            let mut roles = Vec::new();
            for value in values {
                let Some(role) = role_from_semantic(value) else {
                    yylex.AppendError(yylex.Errorf("expected role", &[]));
                    return Err(1);
                };
                roles.push(role);
            }
            out.statement = Some(Box::new(parser_ast::GrantRoleStmt {
                node_text: Default::default(),
                Roles: roles,
                Users: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<auth::UserIdentity>>())
                    .cloned()
                    .unwrap_or_default(),
            }));
        }
        SecurityRule::RevokeRoleStmtAlt01 => {
            let values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<RoleOrPrivSemantic>>())
                .cloned()
                .unwrap_or_default();
            let users = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<auth::UserIdentity>>())
                .cloned()
                .unwrap_or_default();
            let revoke_all = matches!(values.as_slice(), [RoleOrPrivSemantic::Priv(a), RoleOrPrivSemantic::Priv(b)] if a.Priv == parser_mysql::privs::AllPriv && b.Priv == parser_mysql::privs::GrantPriv);
            if revoke_all {
                out.statement = Some(Box::new(parser_ast::RevokeStmt {
                    Privs: vec![
                        parser_ast::PrivElem {
                            Priv: parser_mysql::privs::AllPriv,
                            ..Default::default()
                        },
                        parser_ast::PrivElem {
                            Priv: parser_mysql::privs::GrantPriv,
                            ..Default::default()
                        },
                    ],
                    Level: parser_ast::GrantLevel {
                        Level: parser_ast::GrantLevelType::Global,
                        ..Default::default()
                    },
                    Users: users
                        .into_iter()
                        .map(|User| parser_ast::UserSpec {
                            User,
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                }));
            } else {
                let mut roles = Vec::new();
                for value in values {
                    let Some(role) = role_from_semantic(value) else {
                        yylex.AppendError(yylex.Errorf("expected role", &[]));
                        return Err(1);
                    };
                    roles.push(role);
                }
                out.statement = Some(Box::new(parser_ast::RevokeRoleStmt {
                    node_text: Default::default(),
                    Roles: roles,
                    Users: users,
                }));
            }
        }
        SecurityRule::EncryptionOptAlt01 => match rhs[rhs_len - (0)].ident.as_str() {
            "Y" | "y" => {
                yylex.AppendError(yylex.Errorf(
                    "The ENCRYPTION clause is parsed but ignored by all storage engines.",
                    &[],
                ));
                yylex.LastErrorAsWarn();
            }
            "N" | "n" => {}
            _ => {
                yylex.AppendError(yylex.Errorf("argument should be Y or N", &[]));
                return Err(1);
            }
        },
    }
    Ok(true)
}
