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
enum DdlRule {
    AlterTableStmtAlt01,
    AlterTableStmtAlt02,
    AlterTableStmtAlt03,
    AlterTableStmtAlt04,
    AlterTableStmtAlt05,
    AlterTableStmtAlt06,
    AlterTableStmtAlt07,
    SplitIndexListOptAlt01,
    SplitIndexListOptAlt02,
    SplitIndexListAlt01,
    SplitIndexListAlt02,
    SplitIndexOptionAlt01,
    SplitIndexOptionAlt02,
    SplitIndexOptionAlt03,
    PlacementOptionListAlt01,
    PlacementOptionListAlt02,
    PlacementOptionListAlt03,
    DirectPlacementOptionAlt01,
    DirectPlacementOptionAlt02,
    DirectPlacementOptionAlt03,
    DirectPlacementOptionAlt04,
    DirectPlacementOptionAlt05,
    DirectPlacementOptionAlt06,
    DirectPlacementOptionAlt07,
    DirectPlacementOptionAlt08,
    DirectPlacementOptionAlt09,
    DirectPlacementOptionAlt10,
    DirectPlacementOptionAlt11,
    DirectPlacementOptionAlt12,
    PlacementPolicyOptionAlt01,
    PlacementPolicyOptionAlt02,
    PlacementPolicyOptionAlt03,
    PlacementPolicyOptionAlt04,
    AttributesOptAlt01,
    AttributesOptAlt02,
    StatsOptionsOptAlt01,
    StatsOptionsOptAlt02,
    AlterTableSpecSingleOptAlt01,
    AlterTableSpecSingleOptAlt02,
    AlterTableSpecSingleOptAlt03,
    AlterTableSpecSingleOptAlt04,
    AlterTableSpecSingleOptAlt05,
    AlterTableSpecSingleOptAlt06,
    AlterTableSpecSingleOptAlt07,
    AlterTableSpecSingleOptAlt08,
    AlterTableSpecSingleOptAlt09,
    LocationLabelListAlt01,
    LocationLabelListAlt02,
    AlterTableSpecAlt01,
    AlterTableSpecAlt02,
    AlterTableSpecAlt03,
    AlterTableSpecAlt04,
    AlterTableSpecAlt05,
    AlterTableSpecAlt06,
    AlterTableSpecAlt07,
    AlterTableSpecAlt08,
    AlterTableSpecAlt09,
    AlterTableSpecAlt10,
    AlterTableSpecAlt11,
    AlterTableSpecAlt12,
    AlterTableSpecAlt13,
    AlterTableSpecAlt14,
    AlterTableSpecAlt15,
    AlterTableSpecAlt16,
    AlterTableSpecAlt17,
    AlterTableSpecAlt18,
    AlterTableSpecAlt19,
    AlterTableSpecAlt20,
    AlterTableSpecAlt21,
    AlterTableSpecAlt22,
    AlterTableSpecAlt23,
    AlterTableSpecAlt24,
    AlterTableSpecAlt25,
    AlterTableSpecAlt26,
    AlterTableSpecAlt27,
    AlterTableSpecAlt28,
    AlterTableSpecAlt29,
    AlterTableSpecAlt30,
    AlterTableSpecAlt31,
    AlterTableSpecAlt32,
    AlterTableSpecAlt33,
    AlterTableSpecAlt34,
    AlterTableSpecAlt35,
    AlterTableSpecAlt36,
    AlterTableSpecAlt37,
    AlterTableSpecAlt38,
    AlterTableSpecAlt39,
    AlterTableSpecAlt40,
    AlterTableSpecAlt41,
    AlterTableSpecAlt42,
    AlterTableSpecAlt43,
    AlterTableSpecAlt44,
    AlterTableSpecAlt45,
    AlterTableSpecAlt46,
    AlterTableSpecAlt47,
    AlterTableSpecAlt48,
    AlterTableSpecAlt49,
    AlterTableSpecAlt50,
    AlterTableSpecAlt51,
    AlterTableSpecAlt52,
    AlterTableSpecAlt53,
    AlterTableSpecAlt54,
    AlterTableSpecAlt55,
    AlterTableSpecAlt56,
    AlterTableSpecAlt57,
    AlterTableSpecAlt58,
    AlterTableSpecAlt59,
    AlterTableSpecAlt60,
    AlterTableSpecAlt61,
    AlterTableSpecAlt62,
    AlterTableSpecAlt63,
    AlterTableSpecAlt64,
    AlterTableSpecAlt65,
    AlterTableSpecAlt66,
    AlterTableSpecAlt67,
    AlterTableSpecAlt68,
    AlterTableSpecAlt69,
    AlterTableSpecAlt70,
    ReorganizePartitionRuleOptAlt01,
    ReorganizePartitionRuleOptAlt02,
    AllOrPartitionNameListAlt01,
    WithValidationOptAlt01,
    WithValidationAlt01,
    WithValidationAlt02,
    WithClusteredAlt01,
    WithClusteredAlt02,
    GlobalOrLocalOptAlt01,
    GlobalOrLocalOptAlt02,
    GlobalOrLocalOptAlt03,
    AlgorithmClauseAlt01,
    AlgorithmClauseAlt02,
    AlgorithmClauseAlt03,
    AlgorithmClauseAlt04,
    AlgorithmClauseAlt05,
    LockClauseAlt01,
    LockClauseAlt02,
    WriteableAlt01,
    WriteableAlt02,
    ColumnPositionAlt01,
    ColumnPositionAlt02,
    ColumnPositionAlt03,
    AlterTableSpecListOptAlt01,
    AlterTableSpecListAlt01,
    AlterTableSpecListAlt02,
    PartitionNameListAlt01,
    PartitionNameListAlt02,
    ConstraintKeywordOptAlt01,
    ConstraintKeywordOptAlt02,
    ConstraintKeywordOptAlt03,
    RenameTableStmtAlt01,
    TableToTableListAlt01,
    TableToTableListAlt02,
    TableToTableAlt01,
    RecoverTableStmtAlt01,
    RecoverTableStmtAlt02,
    RecoverTableStmtAlt03,
    FlashbackToTimestampStmtAlt01,
    FlashbackToTimestampStmtAlt02,
    FlashbackToTimestampStmtAlt03,
    FlashbackToTimestampStmtAlt04,
    FlashbackToTimestampStmtAlt05,
    FlashbackToTimestampStmtAlt06,
    FlashbackTableStmtAlt01,
    FlashbackToNewNameAlt01,
    FlashbackToNewNameAlt02,
    FlashbackDatabaseStmtAlt01,
    DistributeTableStmtAlt01,
    DistributeTableStmtAlt02,
    CancelDistributionJobStmtAlt01,
    SplitRegionStmtAlt01,
    SplitRegionStmtAlt02,
    SplitOptionBetweenAlt01,
    SplitOptionAlt01,
    SplitOptionAlt02,
    SplitSyntaxOptionAlt01,
    SplitSyntaxOptionAlt02,
    SplitSyntaxOptionAlt03,
    SplitSyntaxOptionAlt04,
    ColumnDefAlt01,
    ColumnDefAlt02,
    EnforcedOrNotAlt01,
    EnforcedOrNotAlt02,
    EnforcedOrNotOptAlt01,
    EnforcedOrNotOrNotNullOptAlt01,
    EnforcedOrNotOrNotNullOptAlt02,
    ColumnOptionAlt01,
    ColumnOptionAlt02,
    ColumnOptionAlt03,
    ColumnOptionAlt04,
    ColumnOptionAlt05,
    ColumnOptionAlt06,
    ColumnOptionAlt07,
    ColumnOptionAlt08,
    ColumnOptionAlt09,
    ColumnOptionAlt10,
    ColumnOptionAlt11,
    ColumnOptionAlt12,
    ColumnOptionAlt13,
    ColumnOptionAlt14,
    ColumnOptionAlt15,
    ColumnOptionAlt16,
    ColumnOptionAlt17,
    ColumnOptionAlt18,
    ColumnOptionAlt19,
    ColumnOptionAlt20,
    ColumnOptionAlt21,
    ColumnOptionAlt22,
    ColumnOptionAlt23,
    AutoRandomOptAlt01,
    AutoRandomOptAlt02,
    AutoRandomOptAlt03,
    ColumnFormatAlt01,
    ColumnFormatAlt02,
    ColumnFormatAlt03,
    VirtualOrStoredAlt01,
    VirtualOrStoredAlt02,
    VirtualOrStoredAlt03,
    ColumnOptionListAlt01,
    ColumnOptionListAlt02,
    ColumnOptionListOptAlt01,
    ConstraintElemAlt01,
    ConstraintElemAlt02,
    ConstraintElemAlt03,
    ConstraintElemAlt04,
    ConstraintElemAlt05,
    ConstraintElemAlt06,
    MatchAlt01,
    MatchAlt02,
    MatchAlt03,
    MatchOptAlt01,
    MatchOptAlt02,
    ReferDefAlt01,
    OnDeleteAlt01,
    OnUpdateAlt01,
    OnDeleteUpdateOptAlt01,
    OnDeleteUpdateOptAlt02,
    OnDeleteUpdateOptAlt03,
    OnDeleteUpdateOptAlt04,
    OnDeleteUpdateOptAlt05,
    ReferOptAlt01,
    ReferOptAlt02,
    ReferOptAlt03,
    ReferOptAlt04,
    ReferOptAlt05,
    DefaultValueExprAlt05,
    DefaultValueExprAlt06,
    CreateIndexStmtAlt01,
    IndexPartSpecificationListOptAlt01,
    IndexPartSpecificationListOptAlt02,
    IndexPartSpecificationListAlt01,
    IndexPartSpecificationListAlt02,
    IndexPartSpecificationAlt01,
    IndexPartSpecificationAlt02,
    IndexLockAndAlgorithmOptAlt01,
    IndexLockAndAlgorithmOptAlt02,
    IndexLockAndAlgorithmOptAlt03,
    IndexLockAndAlgorithmOptAlt04,
    IndexLockAndAlgorithmOptAlt05,
    IndexKeyTypeOptAlt01,
    IndexKeyTypeOptAlt02,
    IndexKeyTypeOptAlt03,
    IndexKeyTypeOptAlt04,
    IndexKeyTypeOptAlt05,
    IndexKeyTypeOptAlt06,
    AlterDatabaseStmtAlt01,
    AlterDatabaseStmtAlt02,
    CreateDatabaseStmtAlt01,
    DatabaseOptionAlt01,
    DatabaseOptionAlt02,
    DatabaseOptionAlt03,
    DatabaseOptionAlt04,
    DatabaseOptionAlt05,
    DatabaseOptionAlt06,
    DatabaseOptionListOptAlt01,
    DatabaseOptionListAlt01,
    DatabaseOptionListAlt02,
    CreateTableStmtAlt01,
    CreateTableStmtAlt02,
    OnCommitOptAlt01,
    OnCommitOptAlt02,
    OnCommitOptAlt03,
    PartitionOptAlt01,
    PartitionOptAlt02,
    GlobalOrLocalAlt01,
    GlobalOrLocalAlt02,
    UpdateIndexElemAlt01,
    UpdateIndexesListAlt01,
    UpdateIndexesListAlt02,
    UpdateIndexesOptAlt01,
    UpdateIndexesOptAlt02,
    SubPartitionMethodAlt01,
    SubPartitionMethodAlt02,
    PartitionKeyAlgorithmOptAlt01,
    PartitionKeyAlgorithmOptAlt02,
    PartitionMethodAlt02,
    PartitionMethodAlt03,
    PartitionMethodAlt04,
    PartitionMethodAlt05,
    PartitionMethodAlt06,
    PartitionMethodAlt07,
    PartitionMethodAlt08,
    PartitionIntervalOptAlt01,
    PartitionIntervalOptAlt02,
    IntervalExprAlt01,
    IntervalExprAlt02,
    NullPartOptAlt01,
    NullPartOptAlt02,
    MaxValPartOptAlt01,
    MaxValPartOptAlt02,
    FirstAndLastPartOptAlt01,
    FirstAndLastPartOptAlt02,
    LinearOptAlt01,
    SubPartitionOptAlt01,
    SubPartitionOptAlt02,
    SubPartitionNumOptAlt01,
    SubPartitionNumOptAlt02,
    PartitionNumOptAlt01,
    PartitionNumOptAlt02,
    PartitionDefinitionListOptAlt01,
    PartitionDefinitionListOptAlt02,
    PartitionDefinitionListAlt01,
    PartitionDefinitionListAlt02,
    PartitionDefinitionAlt01,
    SubPartDefinitionListOptAlt01,
    SubPartDefinitionListOptAlt02,
    SubPartDefinitionListAlt01,
    SubPartDefinitionListAlt02,
    SubPartDefinitionAlt01,
    PartDefOptionListAlt01,
    PartDefOptionListAlt02,
    PartDefOptionAlt01,
    PartDefOptionAlt02,
    PartDefOptionAlt03,
    PartDefOptionAlt04,
    PartDefOptionAlt05,
    PartDefOptionAlt06,
    PartDefOptionAlt07,
    PartDefOptionAlt08,
    PartDefOptionAlt09,
    PartDefOptionAlt10,
    PartDefOptionAlt11,
    PartDefOptionAlt12,
    PartDefOptionAlt13,
    PartDefValuesOptAlt01,
    PartDefValuesOptAlt02,
    PartDefValuesOptAlt03,
    PartDefValuesOptAlt04,
    PartDefValuesOptAlt05,
    PartDefValuesOptAlt06,
    PartDefValuesOptAlt07,
    DuplicateOptAlt01,
    DuplicateOptAlt02,
    DuplicateOptAlt03,
    CreateTableSelectOptAlt01,
    CreateTableSelectOptAlt02,
    CreateTableSelectOptAlt03,
    CreateTableSelectOptAlt04,
    CreateTableSelectOptAlt05,
    CreateViewSelectOptAlt04,
    LikeTableWithOrWithoutParenAlt01,
    LikeTableWithOrWithoutParenAlt02,
    CreateViewStmtAlt01,
    OrReplaceAlt01,
    OrReplaceAlt02,
    ViewAlgorithmAlt01,
    ViewAlgorithmAlt02,
    ViewAlgorithmAlt03,
    ViewAlgorithmAlt04,
    ViewDefinerAlt01,
    ViewDefinerAlt02,
    ViewSQLSecurityAlt01,
    ViewSQLSecurityAlt02,
    ViewSQLSecurityAlt03,
    ViewFieldListAlt01,
    ViewFieldListAlt02,
    ColumnListAlt01,
    ColumnListAlt02,
    ViewCheckOptionAlt01,
    ViewCheckOptionAlt02,
    ViewCheckOptionAlt03,
    DropDatabaseStmtAlt01,
    DropIndexStmtAlt01,
    DropIndexStmtAlt02,
    DropTableStmtAlt01,
    OptTemporaryAlt01,
    OptTemporaryAlt02,
    OptTemporaryAlt03,
    DropViewStmtAlt01,
    DropViewStmtAlt02,
    IndexNameAlt01,
    IndexNameAlt02,
    IndexOptionListAlt01,
    IndexOptionListAlt02,
    IndexOptionAlt01,
    IndexOptionAlt02,
    IndexOptionAlt03,
    IndexOptionAlt04,
    IndexOptionAlt05,
    IndexOptionAlt06,
    IndexOptionAlt07,
    IndexOptionAlt08,
    IndexOptionAlt09,
    IndexOptionAlt10,
    IndexOptionAlt11,
    IndexOptionAlt12,
    IndexOptionAlt13,
    IndexOptionAlt14,
    IndexNameAndTypeOptAlt01,
    IndexNameAndTypeOptAlt02,
    IndexNameAndTypeOptAlt03,
    IndexTypeOptAlt01,
    IndexTypeAlt01,
    IndexTypeAlt02,
    IndexTypeNameAlt01,
    IndexTypeNameAlt02,
    IndexTypeNameAlt03,
    IndexTypeNameAlt04,
    IndexTypeNameAlt05,
    IndexTypeNameAlt06,
    IndexInvisibleAlt01,
    IndexInvisibleAlt02,
    ConstraintAlt01,
    ConstraintVectorIndexAlt01,
    ConstraintColumnarIndexAlt01,
    ConstraintWithColumnarIndexAlt03,
    TableElementListAlt01,
    TableElementListAlt02,
    TableElementListOptAlt01,
    TableElementListOptAlt02,
    TableOptionAlt02,
    TableOptionAlt03,
    TableOptionAlt04,
    TableOptionAlt05,
    TableOptionAlt06,
    TableOptionAlt07,
    TableOptionAlt08,
    TableOptionAlt09,
    TableOptionAlt10,
    TableOptionAlt11,
    TableOptionAlt12,
    TableOptionAlt13,
    TableOptionAlt14,
    TableOptionAlt15,
    TableOptionAlt16,
    TableOptionAlt17,
    TableOptionAlt18,
    TableOptionAlt19,
    TableOptionAlt20,
    TableOptionAlt21,
    TableOptionAlt22,
    TableOptionAlt23,
    TableOptionAlt24,
    TableOptionAlt25,
    TableOptionAlt26,
    TableOptionAlt27,
    TableOptionAlt28,
    TableOptionAlt29,
    TableOptionAlt30,
    TableOptionAlt31,
    TableOptionAlt32,
    TableOptionAlt33,
    TableOptionAlt34,
    TableOptionAlt35,
    TableOptionAlt36,
    TableOptionAlt37,
    TableOptionAlt38,
    TableOptionAlt39,
    TableOptionAlt40,
    TableOptionAlt41,
    TableOptionAlt42,
    TableOptionAlt43,
    TableOptionAlt44,
    TableOptionAlt45,
    TableOptionAlt46,
    ForceOptAlt01,
    ForceOptAlt02,
    CreateTableOptionListOptAlt01,
    CreateTableOptionListAlt01,
    CreateTableOptionListAlt02,
    CreateTableOptionListAlt03,
    CreateTableOptionAlt02,
    TableOptionListAlt01,
    TableOptionListAlt02,
    TableOptionListAlt03,
    TruncateTableStmtAlt01,
    RowFormatAlt01,
    RowFormatAlt02,
    RowFormatAlt03,
    RowFormatAlt04,
    RowFormatAlt05,
    RowFormatAlt06,
    RowFormatAlt07,
    RowFormatAlt08,
    RowFormatAlt09,
    RowFormatAlt10,
    RowFormatAlt11,
    RowFormatAlt12,
    RowFormatAlt13,
    RowFormatAlt14,
    RowFormatAlt15,
    NumericTypeAlt01,
    NumericTypeAlt02,
    NumericTypeAlt03,
    NumericTypeAlt04,
    NumericTypeAlt05,
    IntegerTypeAlt01,
    IntegerTypeAlt02,
    IntegerTypeAlt03,
    IntegerTypeAlt04,
    IntegerTypeAlt05,
    IntegerTypeAlt06,
    IntegerTypeAlt07,
    IntegerTypeAlt08,
    IntegerTypeAlt09,
    IntegerTypeAlt10,
    IntegerTypeAlt11,
    IntegerTypeAlt12,
    BooleanTypeAlt01,
    BooleanTypeAlt02,
    FixedPointTypeAlt01,
    FixedPointTypeAlt02,
    FixedPointTypeAlt03,
    FloatingPointTypeAlt01,
    FloatingPointTypeAlt02,
    FloatingPointTypeAlt03,
    FloatingPointTypeAlt04,
    FloatingPointTypeAlt05,
    FloatingPointTypeAlt06,
    BitValueTypeAlt01,
    StringTypeAlt01,
    StringTypeAlt02,
    StringTypeAlt03,
    StringTypeAlt04,
    StringTypeAlt05,
    StringTypeAlt06,
    StringTypeAlt07,
    StringTypeAlt08,
    StringTypeAlt09,
    StringTypeAlt10,
    StringTypeAlt11,
    StringTypeAlt12,
    StringTypeAlt13,
    StringTypeAlt14,
    StringTypeAlt15,
    StringTypeAlt16,
    StringTypeAlt17,
    BlobTypeAlt01,
    BlobTypeAlt02,
    BlobTypeAlt03,
    BlobTypeAlt04,
    BlobTypeAlt05,
    TextTypeAlt01,
    TextTypeAlt02,
    TextTypeAlt03,
    TextTypeAlt04,
    OptCharsetWithOptBinaryAlt02,
    OptCharsetWithOptBinaryAlt03,
    OptCharsetWithOptBinaryAlt04,
    DateAndTimeTypeAlt01,
    DateAndTimeTypeAlt02,
    DateAndTimeTypeAlt03,
    DateAndTimeTypeAlt04,
    DateAndTimeTypeAlt05,
    FieldLenAlt01,
    OptFieldLenAlt01,
    FieldOptAlt01,
    FieldOptAlt02,
    FieldOptAlt03,
    FieldOptsAlt01,
    FieldOptsAlt02,
    FloatOptAlt01,
    FloatOptAlt02,
    PrecisionAlt01,
    OptBinModAlt01,
    OptBinModAlt02,
    OptVectorElementTypeAlt01,
    OptVectorElementTypeAlt02,
    OptVectorElementTypeAlt03,
    OptBinaryAlt01,
    OptBinaryAlt02,
    OptBinaryAlt03,
    OptCharsetAlt01,
    OptCharsetAlt02,
    OptCollateAlt01,
    OptCollateAlt02,
    StringListAlt01,
    StringListAlt02,
    TextStringAlt01,
    TextStringAlt02,
    TextStringAlt03,
    TextStringListAlt01,
    TextStringListAlt02,
    AlterRangeStmtAlt01,
    DropPolicyStmtAlt01,
    CreatePolicyStmtAlt01,
    AlterPolicyStmtAlt01,
}

fn identify(rule_id: RuleId) -> Option<DdlRule> {
    match rule_id.as_str() {
        "altertablestmt_alter_ignoreoptional_table_tablen--3c9d9af3e9fc0b6b" => {
            Some(DdlRule::AlterTableStmtAlt01)
        }
        "altertablestmt_alter_ignoreoptional_table_tablen--99ab6c1014e6a3f6" => {
            Some(DdlRule::AlterTableStmtAlt02)
        }
        "altertablestmt_alter_ignoreoptional_table_tablen--19be1adc04b6deab" => {
            Some(DdlRule::AlterTableStmtAlt03)
        }
        "altertablestmt_alter_ignoreoptional_table_tablen--12424e3bdae7591c" => {
            Some(DdlRule::AlterTableStmtAlt04)
        }
        "altertablestmt_alter_ignoreoptional_table_tablen--66f2bb1e3588a9ed" => {
            Some(DdlRule::AlterTableStmtAlt05)
        }
        "altertablestmt_alter_ignoreoptional_table_tablen--aee07250cd0a7ebb" => {
            Some(DdlRule::AlterTableStmtAlt06)
        }
        "altertablestmt_alter_ignoreoptional_table_tablen--e66e4f30a536fb4a" => {
            Some(DdlRule::AlterTableStmtAlt07)
        }
        "splitindexlistopt_prec_lowerthancreatetableselec--619af33b489034d5" => {
            Some(DdlRule::SplitIndexListOptAlt01)
        }
        "splitindexlistopt_splitindexlist_prec_lowerthanc--3e255be6ebe820f7" => {
            Some(DdlRule::SplitIndexListOptAlt02)
        }
        "splitindexlist_splitindexoption--e3d4ab88bad5929d" => Some(DdlRule::SplitIndexListAlt01),
        "splitindexlist_splitindexlist_splitindexoption--31b1e6fd1739c3c1" => {
            Some(DdlRule::SplitIndexListAlt02)
        }
        "splitindexoption_split_primary_key_splitoptionbe--556bc57a21d5487b" => {
            Some(DdlRule::SplitIndexOptionAlt01)
        }
        "splitindexoption_split_index_identifier_splitopt--0cdb52fa657a777d" => {
            Some(DdlRule::SplitIndexOptionAlt02)
        }
        "splitindexoption_split_splitoptionbetween--d20bcbf47f7e6426" => {
            Some(DdlRule::SplitIndexOptionAlt03)
        }
        "placementoptionlist_directplacementoption--0edf2253c4b16385" => {
            Some(DdlRule::PlacementOptionListAlt01)
        }
        "placementoptionlist_placementoptionlist_directpl--4376bce2060d56ef" => {
            Some(DdlRule::PlacementOptionListAlt02)
        }
        "placementoptionlist_placementoptionlist_directpl--31d8f51aaee77df3" => {
            Some(DdlRule::PlacementOptionListAlt03)
        }
        "directplacementoption_primary_region_eqopt_strin--73a872201b725197" => {
            Some(DdlRule::DirectPlacementOptionAlt01)
        }
        "directplacementoption_regions_eqopt_stringlit--6aba1f5d0e6544f7" => {
            Some(DdlRule::DirectPlacementOptionAlt02)
        }
        "directplacementoption_followers_eqopt_lengthnum--f1ec4590b54ab743" => {
            Some(DdlRule::DirectPlacementOptionAlt03)
        }
        "directplacementoption_voters_eqopt_lengthnum--b44db91c839acd55" => {
            Some(DdlRule::DirectPlacementOptionAlt04)
        }
        "directplacementoption_learners_eqopt_lengthnum--8866cca412dfad52" => {
            Some(DdlRule::DirectPlacementOptionAlt05)
        }
        "directplacementoption_schedule_eqopt_stringlit--44973064de21c6ad" => {
            Some(DdlRule::DirectPlacementOptionAlt06)
        }
        "directplacementoption_constraints_eqopt_stringli--a9c137b7053d51b8" => {
            Some(DdlRule::DirectPlacementOptionAlt07)
        }
        "directplacementoption_leader_constraints_eqopt_s--69982ab01fd317c8" => {
            Some(DdlRule::DirectPlacementOptionAlt08)
        }
        "directplacementoption_follower_constraints_eqopt--bac10c0aacba6619" => {
            Some(DdlRule::DirectPlacementOptionAlt09)
        }
        "directplacementoption_voter_constraints_eqopt_st--cb0b86fe8c875ff7" => {
            Some(DdlRule::DirectPlacementOptionAlt10)
        }
        "directplacementoption_learner_constraints_eqopt--7e2bc7b7740cd32a" => {
            Some(DdlRule::DirectPlacementOptionAlt11)
        }
        "directplacementoption_survival_preferences_eqopt--4fff0449e4c07cab" => {
            Some(DdlRule::DirectPlacementOptionAlt12)
        }
        "placementpolicyoption_placement_policy_eqopt_str--b4eb7eee3b1493de" => {
            Some(DdlRule::PlacementPolicyOptionAlt01)
        }
        "placementpolicyoption_placement_policy_eqopt_pol--6651101ca1c42bb5" => {
            Some(DdlRule::PlacementPolicyOptionAlt02)
        }
        "placementpolicyoption_placement_policy_eqopt_def--a709aa720912ccff" => {
            Some(DdlRule::PlacementPolicyOptionAlt03)
        }
        "placementpolicyoption_placement_policy_set_defau--36125768f5e4c3f4" => {
            Some(DdlRule::PlacementPolicyOptionAlt04)
        }
        "attributesopt_attributes_eqopt_default--d3d9eb313056613f" => {
            Some(DdlRule::AttributesOptAlt01)
        }
        "attributesopt_attributes_eqopt_stringlit--bd7298a3e5aae91e" => {
            Some(DdlRule::AttributesOptAlt02)
        }
        "statsoptionsopt_stats_options_eqopt_default--a3d93edfe55f0830" => {
            Some(DdlRule::StatsOptionsOptAlt01)
        }
        "statsoptionsopt_stats_options_eqopt_stringlit--7498884ee2d1a285" => {
            Some(DdlRule::StatsOptionsOptAlt02)
        }
        "altertablespecsingleopt_partitionopt--69b88077c65dc8bd" => {
            Some(DdlRule::AlterTableSpecSingleOptAlt01)
        }
        "altertablespecsingleopt_remove_partitioning--c71698a5c75168e4" => {
            Some(DdlRule::AlterTableSpecSingleOptAlt02)
        }
        "altertablespecsingleopt_reorganize_partition_now--fff820bdcdd3958c" => {
            Some(DdlRule::AlterTableSpecSingleOptAlt03)
        }
        "altertablespecsingleopt_splitindexoption--2dc3a7bdd17b5267" => {
            Some(DdlRule::AlterTableSpecSingleOptAlt04)
        }
        "altertablespecsingleopt_split_maxvalue_partition--239448c9b06d8150" => {
            Some(DdlRule::AlterTableSpecSingleOptAlt05)
        }
        "altertablespecsingleopt_merge_first_partition_le--5f6f45611194e987" => {
            Some(DdlRule::AlterTableSpecSingleOptAlt06)
        }
        "altertablespecsingleopt_partition_identifier_att--dfa88f3c85bb0e91" => {
            Some(DdlRule::AlterTableSpecSingleOptAlt07)
        }
        "altertablespecsingleopt_partition_identifier_par--d346fbec5f84bea6" => {
            Some(DdlRule::AlterTableSpecSingleOptAlt08)
        }
        "altertablespecsingleopt_remove_ttl--30d00f04ec57f906" => {
            Some(DdlRule::AlterTableSpecSingleOptAlt09)
        }
        "locationlabellist--096206921aa2166c" => Some(DdlRule::LocationLabelListAlt01),
        "locationlabellist_location_labels_stringlist--e808df7b8ac6caaa" => {
            Some(DdlRule::LocationLabelListAlt02)
        }
        "altertablespec_tableoptionlist_prec_higherthanco--ed818ef885f3e170" => {
            Some(DdlRule::AlterTableSpecAlt01)
        }
        "altertablespec_set_tiflash_replica_lengthnum_loc--8dd5ee7be3197ae3" => {
            Some(DdlRule::AlterTableSpecAlt02)
        }
        "altertablespec_set_hypo_tiflash_replica_lengthnu--c409de2fdeae932f" => {
            Some(DdlRule::AlterTableSpecAlt03)
        }
        "altertablespec_convert_to_charsetkw_charsetname--f833ed8a42864b2f" => {
            Some(DdlRule::AlterTableSpecAlt04)
        }
        "altertablespec_convert_to_charsetkw_default_optc--f09f927ef70a0ab7" => {
            Some(DdlRule::AlterTableSpecAlt05)
        }
        "altertablespec_add_columnkeywordopt_ifnotexists--c41007b316c3490c" => {
            Some(DdlRule::AlterTableSpecAlt06)
        }
        "altertablespec_add_columnkeywordopt_ifnotexists--13ca710d2b3734af" => {
            Some(DdlRule::AlterTableSpecAlt07)
        }
        "altertablespec_add_constraintwithcolumnarindex--5c20ec87e30dedc2" => {
            Some(DdlRule::AlterTableSpecAlt08)
        }
        "altertablespec_add_partition_ifnotexists_nowrite--7a2ff581f8d4fd1f" => {
            Some(DdlRule::AlterTableSpecAlt09)
        }
        "altertablespec_add_partition_ifnotexists_nowrite--e9d4d4f525d17e76" => {
            Some(DdlRule::AlterTableSpecAlt10)
        }
        "altertablespec_last_partition_less_than_bitexpr--629693c45009742b" => {
            Some(DdlRule::AlterTableSpecAlt11)
        }
        "altertablespec_add_stats_extended_ifnotexists_id--67969fc60c94f507" => {
            Some(DdlRule::AlterTableSpecAlt12)
        }
        "altertablespec_add_masking_type_columnoptionlist--b44e3e38670b0225" => {
            Some(DdlRule::AlterTableSpecAlt13)
        }
        "altertablespec_add_masking_serial_columnoptionli--d3959101a0e9d355" => {
            Some(DdlRule::AlterTableSpecAlt14)
        }
        "altertablespec_add_masking_policy_policyname_on--29e52121f078c16e" => {
            Some(DdlRule::AlterTableSpecAlt15)
        }
        "altertablespec_enable_masking_policy_policyname--f4495693daeadaf5" => {
            Some(DdlRule::AlterTableSpecAlt16)
        }
        "altertablespec_disable_masking_policy_policyname--3b579cfadb8ba78e" => {
            Some(DdlRule::AlterTableSpecAlt17)
        }
        "altertablespec_drop_masking_policy_policyname--b474700b2a328d8f" => {
            Some(DdlRule::AlterTableSpecAlt18)
        }
        "altertablespec_drop_masking_restrictorcascadeopt--b6290c09d1b4e9b2" => {
            Some(DdlRule::AlterTableSpecAlt19)
        }
        "altertablespec_modify_masking_policy_policyname--235476c1843c13e6" => {
            Some(DdlRule::AlterTableSpecAlt20)
        }
        "altertablespec_modify_masking_type_columnoptionl--2d49a4ac99e78ce0" => {
            Some(DdlRule::AlterTableSpecAlt21)
        }
        "altertablespec_modify_masking_serial_columnoptio--716833b92b656e9c" => {
            Some(DdlRule::AlterTableSpecAlt22)
        }
        "altertablespec_modify_masking_policy_policyname--f11ac666e8172d29" => {
            Some(DdlRule::AlterTableSpecAlt23)
        }
        "altertablespec_modify_masking_policy_policyname--362e918978a5dc21" => {
            Some(DdlRule::AlterTableSpecAlt24)
        }
        "altertablespec_attributesopt--99a19d0a1784ee6f" => Some(DdlRule::AlterTableSpecAlt25),
        "altertablespec_statsoptionsopt--e6e806bd5531ec91" => Some(DdlRule::AlterTableSpecAlt26),
        "altertablespec_check_partition_allorpartitionnam--d99aa4089b14493e" => {
            Some(DdlRule::AlterTableSpecAlt27)
        }
        "altertablespec_coalesce_partition_nowritetobinlo--54420ad97be7e63b" => {
            Some(DdlRule::AlterTableSpecAlt28)
        }
        "altertablespec_drop_columnkeywordopt_ifexists_co--02ce87b6a0502674" => {
            Some(DdlRule::AlterTableSpecAlt29)
        }
        "altertablespec_drop_primary_key--8ac9f74e2f6e398f" => Some(DdlRule::AlterTableSpecAlt30),
        "altertablespec_drop_partition_ifexists_partition--4e8f61d13a001a72" => {
            Some(DdlRule::AlterTableSpecAlt31)
        }
        "altertablespec_first_partition_less_than_bitexpr--4b09b6763730ce4b" => {
            Some(DdlRule::AlterTableSpecAlt32)
        }
        "altertablespec_drop_stats_extended_ifexists_iden--b03e3ea30caf9d05" => {
            Some(DdlRule::AlterTableSpecAlt33)
        }
        "altertablespec_exchange_partition_identifier_wit--2b24ff3f31b95c94" => {
            Some(DdlRule::AlterTableSpecAlt34)
        }
        "altertablespec_truncate_partition_allorpartition--7367a2762ecc9fda" => {
            Some(DdlRule::AlterTableSpecAlt35)
        }
        "altertablespec_optimize_partition_nowritetobinlo--3b3e57b017caf386" => {
            Some(DdlRule::AlterTableSpecAlt36)
        }
        "altertablespec_repair_partition_nowritetobinloga--d5cd59db20c69d3c" => {
            Some(DdlRule::AlterTableSpecAlt37)
        }
        "altertablespec_import_partition_allorpartitionna--ff9fed50bef44bf9" => {
            Some(DdlRule::AlterTableSpecAlt38)
        }
        "altertablespec_discard_partition_allorpartitionn--0cf13ce7230345e6" => {
            Some(DdlRule::AlterTableSpecAlt39)
        }
        "altertablespec_import_tablespace--01cc5417adb5ec32" => Some(DdlRule::AlterTableSpecAlt40),
        "altertablespec_discard_tablespace--ce1ece9e957a3713" => Some(DdlRule::AlterTableSpecAlt41),
        "altertablespec_rebuild_partition_nowritetobinlog--6be57f78f3fac304" => {
            Some(DdlRule::AlterTableSpecAlt42)
        }
        "altertablespec_drop_keyorindex_ifexists_identifi--5d1499a73af47108" => {
            Some(DdlRule::AlterTableSpecAlt43)
        }
        "altertablespec_drop_foreign_key_symbol--be05224532d69623" => {
            Some(DdlRule::AlterTableSpecAlt44)
        }
        "altertablespec_order_by_alterorderlist_prec_lowe--424086da5fcd1f27" => {
            Some(DdlRule::AlterTableSpecAlt45)
        }
        "altertablespec_disable_keys--be824d5384a19b07" => Some(DdlRule::AlterTableSpecAlt46),
        "altertablespec_enable_keys--0bc1a1753e90958e" => Some(DdlRule::AlterTableSpecAlt47),
        "altertablespec_modify_columnkeywordopt_ifexists--f7a55813f3ef1ea6" => {
            Some(DdlRule::AlterTableSpecAlt48)
        }
        "altertablespec_change_columnkeywordopt_ifexists--2f69d6f20cde04f3" => {
            Some(DdlRule::AlterTableSpecAlt49)
        }
        "altertablespec_alter_columnkeywordopt_columnname--0683549ff8149a2e" => {
            Some(DdlRule::AlterTableSpecAlt50)
        }
        "altertablespec_alter_columnkeywordopt_columnname--f6ae59c1dcba1818" => {
            Some(DdlRule::AlterTableSpecAlt51)
        }
        "altertablespec_alter_columnkeywordopt_columnname--ad706756649056d2" => {
            Some(DdlRule::AlterTableSpecAlt52)
        }
        "altertablespec_rename_column_identifier_to_ident--0b026fb6780aa95a" => {
            Some(DdlRule::AlterTableSpecAlt53)
        }
        "altertablespec_rename_to_tablename--3155039642c45eab" => {
            Some(DdlRule::AlterTableSpecAlt54)
        }
        "altertablespec_rename_eqopt_tablename--1806d598ddb2f16d" => {
            Some(DdlRule::AlterTableSpecAlt55)
        }
        "altertablespec_rename_as_tablename--d530d32693688ba4" => {
            Some(DdlRule::AlterTableSpecAlt56)
        }
        "altertablespec_rename_keyorindex_identifier_to_i--f114df71e642328e" => {
            Some(DdlRule::AlterTableSpecAlt57)
        }
        "altertablespec_lockclause--b22a30b2d8b2b3d9" => Some(DdlRule::AlterTableSpecAlt58),
        "altertablespec_writeable--f7918df424c734f2" => Some(DdlRule::AlterTableSpecAlt59),
        "altertablespec_algorithmclause--0d8c45fb80899165" => Some(DdlRule::AlterTableSpecAlt60),
        "altertablespec_force--e1091938cd8ceafc" => Some(DdlRule::AlterTableSpecAlt61),
        "altertablespec_with_validation--c1ed647cf7f2c79c" => Some(DdlRule::AlterTableSpecAlt62),
        "altertablespec_without_validation--57f9ebc0030e9bee" => Some(DdlRule::AlterTableSpecAlt63),
        "altertablespec_secondary_load--87873e2d71e1acea" => Some(DdlRule::AlterTableSpecAlt64),
        "altertablespec_secondary_unload--3725b33f63f52333" => Some(DdlRule::AlterTableSpecAlt65),
        "altertablespec_alter_checkconstraintkeyword_iden--8cb56fc776f0d8f4" => {
            Some(DdlRule::AlterTableSpecAlt66)
        }
        "altertablespec_drop_checkconstraintkeyword_ident--f885dc393e368d85" => {
            Some(DdlRule::AlterTableSpecAlt67)
        }
        "altertablespec_alter_index_identifier_indexinvis--5afc3d3376bce087" => {
            Some(DdlRule::AlterTableSpecAlt68)
        }
        "altertablespec_cache--3608e68af330e9a7" => Some(DdlRule::AlterTableSpecAlt69),
        "altertablespec_nocache--e209a9bc4b463318" => Some(DdlRule::AlterTableSpecAlt70),
        "reorganizepartitionruleopt_prec_lowerthanremove--be4ffba10eb113ef" => {
            Some(DdlRule::ReorganizePartitionRuleOptAlt01)
        }
        "reorganizepartitionruleopt_partitionnamelist_int--10e5de24a5f36eae" => {
            Some(DdlRule::ReorganizePartitionRuleOptAlt02)
        }
        "allorpartitionnamelist_all--c515ae174484d508" => {
            Some(DdlRule::AllOrPartitionNameListAlt01)
        }
        "withvalidationopt--cba1a7a208f044b7" => Some(DdlRule::WithValidationOptAlt01),
        "withvalidation_with_validation--7c6c7ad92f487b24" => Some(DdlRule::WithValidationAlt01),
        "withvalidation_without_validation--567a49b0bd4622b6" => Some(DdlRule::WithValidationAlt02),
        "withclustered_clustered--625a41175a04e746" => Some(DdlRule::WithClusteredAlt01),
        "withclustered_nonclustered--e8cd31f7a43342d3" => Some(DdlRule::WithClusteredAlt02),
        "globalorlocalopt--c4a9e59be799c413" => Some(DdlRule::GlobalOrLocalOptAlt01),
        "globalorlocalopt_local--eb60c17b3ac53f8b" => Some(DdlRule::GlobalOrLocalOptAlt02),
        "globalorlocalopt_global--6c45ccccd872c44f" => Some(DdlRule::GlobalOrLocalOptAlt03),
        "algorithmclause_algorithm_eqopt_default--bf327a658932b47d" => {
            Some(DdlRule::AlgorithmClauseAlt01)
        }
        "algorithmclause_algorithm_eqopt_copy--b57c0db739c71dfd" => {
            Some(DdlRule::AlgorithmClauseAlt02)
        }
        "algorithmclause_algorithm_eqopt_inplace--ac88a0c82b98c7b4" => {
            Some(DdlRule::AlgorithmClauseAlt03)
        }
        "algorithmclause_algorithm_eqopt_instant--50c585ec59dc3c31" => {
            Some(DdlRule::AlgorithmClauseAlt04)
        }
        "algorithmclause_algorithm_eqopt_identifier--0e3ba5503b9197f1" => {
            Some(DdlRule::AlgorithmClauseAlt05)
        }
        "lockclause_lock_eqopt_default--7c121cc2b429492f" => Some(DdlRule::LockClauseAlt01),
        "lockclause_lock_eqopt_identifier--b91f39178fc32ddf" => Some(DdlRule::LockClauseAlt02),
        "writeable_read_write--32039ee04c918008" => Some(DdlRule::WriteableAlt01),
        "writeable_read_only--882a2ce1b18eebc1" => Some(DdlRule::WriteableAlt02),
        "columnposition--31b7870c6036df8c" => Some(DdlRule::ColumnPositionAlt01),
        "columnposition_first--ffdba5f1067b82f9" => Some(DdlRule::ColumnPositionAlt02),
        "columnposition_after_columnname--9715bb86c398de02" => Some(DdlRule::ColumnPositionAlt03),
        "altertablespeclistopt--0f6e68ab9e9131c3" => Some(DdlRule::AlterTableSpecListOptAlt01),
        "altertablespeclist_altertablespec--ee98e36f8ea78430" => {
            Some(DdlRule::AlterTableSpecListAlt01)
        }
        "altertablespeclist_altertablespeclist_altertable--b295020efa9a615b" => {
            Some(DdlRule::AlterTableSpecListAlt02)
        }
        "partitionnamelist_identifier--784170aff4895558" => Some(DdlRule::PartitionNameListAlt01),
        "partitionnamelist_partitionnamelist_identifier--ad859bb1ac8c752b" => {
            Some(DdlRule::PartitionNameListAlt02)
        }
        "constraintkeywordopt_prec_empty--368cc49cc012148c" => {
            Some(DdlRule::ConstraintKeywordOptAlt01)
        }
        "constraintkeywordopt_constraint--2bef3e082038ba76" => {
            Some(DdlRule::ConstraintKeywordOptAlt02)
        }
        "constraintkeywordopt_constraint_symbol--f05d408448a48092" => {
            Some(DdlRule::ConstraintKeywordOptAlt03)
        }
        "renametablestmt_rename_table_tabletotablelist--6b2ef74c70328f2f" => {
            Some(DdlRule::RenameTableStmtAlt01)
        }
        "tabletotablelist_tabletotable--e14a793ee8dba034" => Some(DdlRule::TableToTableListAlt01),
        "tabletotablelist_tabletotablelist_tabletotable--fa0fa05e7625e63f" => {
            Some(DdlRule::TableToTableListAlt02)
        }
        "tabletotable_tablename_to_tablename--4719168b437561e6" => Some(DdlRule::TableToTableAlt01),
        "recovertablestmt_recover_table_by_job_int64num--bf4d31af2553fd1d" => {
            Some(DdlRule::RecoverTableStmtAlt01)
        }
        "recovertablestmt_recover_table_tablename--6334d17d46b927d5" => {
            Some(DdlRule::RecoverTableStmtAlt02)
        }
        "recovertablestmt_recover_table_tablename_int64nu--df570175da27dfaa" => {
            Some(DdlRule::RecoverTableStmtAlt03)
        }
        "flashbacktotimestampstmt_flashback_cluster_totim--d8562a1e1693f04e" => {
            Some(DdlRule::FlashbackToTimestampStmtAlt01)
        }
        "flashbacktotimestampstmt_flashback_table_tablena--30580689e192cc43" => {
            Some(DdlRule::FlashbackToTimestampStmtAlt02)
        }
        "flashbacktotimestampstmt_flashback_databasesym_d--b7750d0a971df577" => {
            Some(DdlRule::FlashbackToTimestampStmtAlt03)
        }
        "flashbacktotimestampstmt_flashback_cluster_totso--24d5b2413eb6af3a" => {
            Some(DdlRule::FlashbackToTimestampStmtAlt04)
        }
        "flashbacktotimestampstmt_flashback_table_tablena--94cbc10b3cc29e83" => {
            Some(DdlRule::FlashbackToTimestampStmtAlt05)
        }
        "flashbacktotimestampstmt_flashback_databasesym_d--fb4ebb3fbeec2a27" => {
            Some(DdlRule::FlashbackToTimestampStmtAlt06)
        }
        "flashbacktablestmt_flashback_table_tablename_fla--c266bf2a25b7a324" => {
            Some(DdlRule::FlashbackTableStmtAlt01)
        }
        "flashbacktonewname--07423843bdc8dcf8" => Some(DdlRule::FlashbackToNewNameAlt01),
        "flashbacktonewname_to_identifier--c3ac5af33ff5f259" => {
            Some(DdlRule::FlashbackToNewNameAlt02)
        }
        "flashbackdatabasestmt_flashback_databasesym_dbna--44748d0b42756ac5" => {
            Some(DdlRule::FlashbackDatabaseStmtAlt01)
        }
        "distributetablestmt_distribute_table_tablename_p--35c7d11128b5a399" => {
            Some(DdlRule::DistributeTableStmtAlt01)
        }
        "distributetablestmt_distribute_table_tablename_p--6ac9b35baf790989" => {
            Some(DdlRule::DistributeTableStmtAlt02)
        }
        "canceldistributionjobstmt_cancel_distribution_jo--5e12e455bfbbcdfd" => {
            Some(DdlRule::CancelDistributionJobStmtAlt01)
        }
        "splitregionstmt_split_splitsyntaxoption_table_ta--31710abb774f7d30" => {
            Some(DdlRule::SplitRegionStmtAlt01)
        }
        "splitregionstmt_split_splitsyntaxoption_table_ta--3f8a6b9e1668a519" => {
            Some(DdlRule::SplitRegionStmtAlt02)
        }
        "splitoptionbetween_between_rowvalue_and_rowvalue--77fb2ea1ce5f0d26" => {
            Some(DdlRule::SplitOptionBetweenAlt01)
        }
        "splitoption_splitoptionbetween--3b9956f213240f28" => Some(DdlRule::SplitOptionAlt01),
        "splitoption_by_valueslist--071b658aa8cd533e" => Some(DdlRule::SplitOptionAlt02),
        "splitsyntaxoption--fb22e29637a5b281" => Some(DdlRule::SplitSyntaxOptionAlt01),
        "splitsyntaxoption_region_for--b561b93e0d601891" => Some(DdlRule::SplitSyntaxOptionAlt02),
        "splitsyntaxoption_partition--2557bafcec1b6b86" => Some(DdlRule::SplitSyntaxOptionAlt03),
        "splitsyntaxoption_region_for_partition--29479a8abd4de6cf" => {
            Some(DdlRule::SplitSyntaxOptionAlt04)
        }
        "columndef_columnname_type_columnoptionlistopt--484184a07fa8f812" => {
            Some(DdlRule::ColumnDefAlt01)
        }
        "columndef_columnname_serial_columnoptionlistopt--3306cdb768a739a2" => {
            Some(DdlRule::ColumnDefAlt02)
        }
        "enforcedornot_enforced--dfa4e17f140d75a4" => Some(DdlRule::EnforcedOrNotAlt01),
        "enforcedornot_notsym_enforced--123e1a8ce8112cbc" => Some(DdlRule::EnforcedOrNotAlt02),
        "enforcedornotopt_prec_lowerthannot--bc167e5a9d0c6834" => {
            Some(DdlRule::EnforcedOrNotOptAlt01)
        }
        "enforcedornotornotnullopt_notsym_null--e6f964f87f52a025" => {
            Some(DdlRule::EnforcedOrNotOrNotNullOptAlt01)
        }
        "enforcedornotornotnullopt_enforcedornotopt--7578e6e5b90483cf" => {
            Some(DdlRule::EnforcedOrNotOrNotNullOptAlt02)
        }
        "columnoption_notsym_null--654b7429c753a31a" => Some(DdlRule::ColumnOptionAlt01),
        "columnoption_null--db1f9ff144b901a0" => Some(DdlRule::ColumnOptionAlt02),
        "columnoption_auto_increment--34e83af6a3c8bf28" => Some(DdlRule::ColumnOptionAlt03),
        "columnoption_primaryopt_key_globalorlocalopt--92fd689b9d52e429" => {
            Some(DdlRule::ColumnOptionAlt04)
        }
        "columnoption_primaryopt_key_withclustered_global--93589d7316f46bf0" => {
            Some(DdlRule::ColumnOptionAlt05)
        }
        "columnoption_unique_global--2df051da3fb36cdb" => Some(DdlRule::ColumnOptionAlt06),
        "columnoption_unique_local--efea6734d39d67e7" => Some(DdlRule::ColumnOptionAlt07),
        "columnoption_unique_prec_lowerthankey--287ee3057dc51620" => {
            Some(DdlRule::ColumnOptionAlt08)
        }
        "columnoption_unique_key_globalorlocalopt--42ac3d85429977f7" => {
            Some(DdlRule::ColumnOptionAlt09)
        }
        "columnoption_default_defaultvalueexpr--cecb2bfdc8a46b0b" => {
            Some(DdlRule::ColumnOptionAlt10)
        }
        "columnoption_serial_default_value--fbbaf5b4f896504f" => Some(DdlRule::ColumnOptionAlt11),
        "columnoption_on_update_nowsymoptionfraction--68f3b1fc757cc6fd" => {
            Some(DdlRule::ColumnOptionAlt12)
        }
        "columnoption_comment_stringlit--b73488faa40b57b8" => Some(DdlRule::ColumnOptionAlt13),
        "columnoption_constraintkeywordopt_check_expressi--61c1a985282d3f83" => {
            Some(DdlRule::ColumnOptionAlt14)
        }
        "columnoption_generatedalways_as_expression_virtu--60fb88a8b6c83c8f" => {
            Some(DdlRule::ColumnOptionAlt15)
        }
        "columnoption_referdef--76a0658572fbbe4e" => Some(DdlRule::ColumnOptionAlt16),
        "columnoption_collate_collationname--19353fb9832a9bf3" => Some(DdlRule::ColumnOptionAlt17),
        "columnoption_column_format_columnformat--a6c8ab0ba990bb6a" => {
            Some(DdlRule::ColumnOptionAlt18)
        }
        "columnoption_storage_storagemedia--09a350e998c91935" => Some(DdlRule::ColumnOptionAlt19),
        "columnoption_auto_random_autorandomopt--b821192ef66ddb25" => {
            Some(DdlRule::ColumnOptionAlt20)
        }
        "columnoption_secondary_engine_attribute_eqopt_st--091a75fc44aa99d8" => {
            Some(DdlRule::ColumnOptionAlt21)
        }
        "columnoption_generatedalways_as_row_start--d74d71447e1a02a3" => {
            Some(DdlRule::ColumnOptionAlt22)
        }
        "columnoption_generatedalways_as_row_end--92a665790a65952c" => {
            Some(DdlRule::ColumnOptionAlt23)
        }
        "autorandomopt--039fe1ee91cb6a26" => Some(DdlRule::AutoRandomOptAlt01),
        "autorandomopt_lengthnum--00581d542bbfa954" => Some(DdlRule::AutoRandomOptAlt02),
        "autorandomopt_lengthnum_lengthnum--8e182718b0c4bdd8" => Some(DdlRule::AutoRandomOptAlt03),
        "columnformat_default--3db244e9a4ae5b18" => Some(DdlRule::ColumnFormatAlt01),
        "columnformat_fixed--afea4668016a60bd" => Some(DdlRule::ColumnFormatAlt02),
        "columnformat_dynamic--89c926a32c6b5400" => Some(DdlRule::ColumnFormatAlt03),
        "virtualorstored--6807fc32cd906c42" => Some(DdlRule::VirtualOrStoredAlt01),
        "virtualorstored_virtual--62010ccc31c04080" => Some(DdlRule::VirtualOrStoredAlt02),
        "virtualorstored_stored--e03ae99d5f34de2a" => Some(DdlRule::VirtualOrStoredAlt03),
        "columnoptionlist_columnoption--bde2ae1ca08281a8" => Some(DdlRule::ColumnOptionListAlt01),
        "columnoptionlist_columnoptionlist_columnoption--ce01ff161ebd9baf" => {
            Some(DdlRule::ColumnOptionListAlt02)
        }
        "columnoptionlistopt--8e7e18484c21f057" => Some(DdlRule::ColumnOptionListOptAlt01),
        "constraintelem_primary_key_indexnameandtypeopt_i--d1338d12d0c94f1c" => {
            Some(DdlRule::ConstraintElemAlt01)
        }
        "constraintelem_fulltext_keyorindexopt_indexname--70317f0637475bbc" => {
            Some(DdlRule::ConstraintElemAlt02)
        }
        "constraintelem_keyorindex_ifnotexists_indexnamea--067eaed649e8fc17" => {
            Some(DdlRule::ConstraintElemAlt03)
        }
        "constraintelem_unique_keyorindexopt_indexnameand--f4a7aac237b0f4b3" => {
            Some(DdlRule::ConstraintElemAlt04)
        }
        "constraintelem_foreign_key_ifnotexists_indexname--69a2ec66cc3b538a" => {
            Some(DdlRule::ConstraintElemAlt05)
        }
        "constraintelem_check_expression_enforcedornotopt--c9b096428ed0673c" => {
            Some(DdlRule::ConstraintElemAlt06)
        }
        "match_match_full--98874c08b8ac5751" => Some(DdlRule::MatchAlt01),
        "match_match_partial--243bf41a2e3d81b5" => Some(DdlRule::MatchAlt02),
        "match_match_simple--c51a43f4684b9d0c" => Some(DdlRule::MatchAlt03),
        "matchopt--00d4269323a1890f" => Some(DdlRule::MatchOptAlt01),
        "matchopt_match--31220b5297933707" => Some(DdlRule::MatchOptAlt02),
        "referdef_references_tablename_indexpartspecifica--88d8a8497267c08f" => {
            Some(DdlRule::ReferDefAlt01)
        }
        "ondelete_on_delete_referopt--dc2f13d560113679" => Some(DdlRule::OnDeleteAlt01),
        "onupdate_on_update_referopt--d0e10fe624709119" => Some(DdlRule::OnUpdateAlt01),
        "ondeleteupdateopt_prec_lowerthanon--fedd5063342b7143" => {
            Some(DdlRule::OnDeleteUpdateOptAlt01)
        }
        "ondeleteupdateopt_ondelete_prec_lowerthanon--3383f934deb99170" => {
            Some(DdlRule::OnDeleteUpdateOptAlt02)
        }
        "ondeleteupdateopt_onupdate_prec_lowerthanon--2487dfefc5406f5a" => {
            Some(DdlRule::OnDeleteUpdateOptAlt03)
        }
        "ondeleteupdateopt_ondelete_onupdate--81a67107e93d0bf8" => {
            Some(DdlRule::OnDeleteUpdateOptAlt04)
        }
        "ondeleteupdateopt_onupdate_ondelete--0bec5703dd7c458c" => {
            Some(DdlRule::OnDeleteUpdateOptAlt05)
        }
        "referopt_restrict--54ba8abd5f1725f7" => Some(DdlRule::ReferOptAlt01),
        "referopt_cascade--58c2e444357fcba7" => Some(DdlRule::ReferOptAlt02),
        "referopt_set_null--09a14460483cbc10" => Some(DdlRule::ReferOptAlt03),
        "referopt_no_action--e15092dddc15ce0c" => Some(DdlRule::ReferOptAlt04),
        "referopt_set_default--b4c995049b1cf19e" => Some(DdlRule::ReferOptAlt05),
        "defaultvalueexpr_identifier--ddcbab35eef8a443" => Some(DdlRule::DefaultValueExprAlt05),
        "defaultvalueexpr_signedliteral--abcedb20e3a1a9d1" => Some(DdlRule::DefaultValueExprAlt06),
        "createindexstmt_create_indexkeytypeopt_index_ifn--385d4ba1c3b4e6c8" => {
            Some(DdlRule::CreateIndexStmtAlt01)
        }
        "indexpartspecificationlistopt--c857b7f009f3360c" => {
            Some(DdlRule::IndexPartSpecificationListOptAlt01)
        }
        "indexpartspecificationlistopt_indexpartspecifica--5c9bb249ba5cfc6a" => {
            Some(DdlRule::IndexPartSpecificationListOptAlt02)
        }
        "indexpartspecificationlist_indexpartspecificatio--0fe9bf53bfee0524" => {
            Some(DdlRule::IndexPartSpecificationListAlt01)
        }
        "indexpartspecificationlist_indexpartspecificatio--fc50c3e04bf38e6e" => {
            Some(DdlRule::IndexPartSpecificationListAlt02)
        }
        "indexpartspecification_columnname_optfieldlen_op--aa3098b22203d7cc" => {
            Some(DdlRule::IndexPartSpecificationAlt01)
        }
        "indexpartspecification_expression_optorder--d667daa8437783a0" => {
            Some(DdlRule::IndexPartSpecificationAlt02)
        }
        "indexlockandalgorithmopt--dd0a18b05cade32b" => {
            Some(DdlRule::IndexLockAndAlgorithmOptAlt01)
        }
        "indexlockandalgorithmopt_lockclause--e76b2bbd4311f9d8" => {
            Some(DdlRule::IndexLockAndAlgorithmOptAlt02)
        }
        "indexlockandalgorithmopt_algorithmclause--55bb82c076a0cdd2" => {
            Some(DdlRule::IndexLockAndAlgorithmOptAlt03)
        }
        "indexlockandalgorithmopt_lockclause_algorithmcla--2942df42c2148148" => {
            Some(DdlRule::IndexLockAndAlgorithmOptAlt04)
        }
        "indexlockandalgorithmopt_algorithmclause_lockcla--4af34cbfcd98366c" => {
            Some(DdlRule::IndexLockAndAlgorithmOptAlt05)
        }
        "indexkeytypeopt--7be42cc79f6dca73" => Some(DdlRule::IndexKeyTypeOptAlt01),
        "indexkeytypeopt_unique--4b2f22dc650e207f" => Some(DdlRule::IndexKeyTypeOptAlt02),
        "indexkeytypeopt_spatial--64188444e54e18d6" => Some(DdlRule::IndexKeyTypeOptAlt03),
        "indexkeytypeopt_fulltext--351a6700d6cc4224" => Some(DdlRule::IndexKeyTypeOptAlt04),
        "indexkeytypeopt_vector--649d8fcad06210c7" => Some(DdlRule::IndexKeyTypeOptAlt05),
        "indexkeytypeopt_columnar--aacd67564c440671" => Some(DdlRule::IndexKeyTypeOptAlt06),
        "alterdatabasestmt_alter_databasesym_dbname_datab--0e2c6500627ef3ac" => {
            Some(DdlRule::AlterDatabaseStmtAlt01)
        }
        "alterdatabasestmt_alter_databasesym_databaseopti--12311dd8098bad4d" => {
            Some(DdlRule::AlterDatabaseStmtAlt02)
        }
        "createdatabasestmt_create_databasesym_ifnotexist--b7e12b4b9113555d" => {
            Some(DdlRule::CreateDatabaseStmtAlt01)
        }
        "databaseoption_defaultkwdopt_charsetkw_eqopt_cha--179752537c5eabe0" => {
            Some(DdlRule::DatabaseOptionAlt01)
        }
        "databaseoption_defaultkwdopt_collate_eqopt_colla--137e31b0c5c7ee1b" => {
            Some(DdlRule::DatabaseOptionAlt02)
        }
        "databaseoption_defaultkwdopt_encryption_eqopt_en--536189cd5baf0650" => {
            Some(DdlRule::DatabaseOptionAlt03)
        }
        "databaseoption_defaultkwdopt_placementpolicyopti--6612f91c4b3f61ae" => {
            Some(DdlRule::DatabaseOptionAlt04)
        }
        "databaseoption_placementpolicyoption--e4581a34bcd492a4" => {
            Some(DdlRule::DatabaseOptionAlt05)
        }
        "databaseoption_set_tiflash_replica_lengthnum_loc--3cda20a4795578ee" => {
            Some(DdlRule::DatabaseOptionAlt06)
        }
        "databaseoptionlistopt--268311174423db6a" => Some(DdlRule::DatabaseOptionListOptAlt01),
        "databaseoptionlist_databaseoption--1a4220207c88bfdc" => {
            Some(DdlRule::DatabaseOptionListAlt01)
        }
        "databaseoptionlist_databaseoptionlist_databaseop--3b7b5e76a917ca30" => {
            Some(DdlRule::DatabaseOptionListAlt02)
        }
        "createtablestmt_create_opttemporary_table_ifnote--7b35988758e9d2e7" => {
            Some(DdlRule::CreateTableStmtAlt01)
        }
        "createtablestmt_create_opttemporary_table_ifnote--d489c385579ef98a" => {
            Some(DdlRule::CreateTableStmtAlt02)
        }
        "oncommitopt--37580b76d79d5b1a" => Some(DdlRule::OnCommitOptAlt01),
        "oncommitopt_on_commit_delete_rows--3134eafc51059637" => Some(DdlRule::OnCommitOptAlt02),
        "oncommitopt_on_commit_preserve_rows--b6332227ce48a7c8" => Some(DdlRule::OnCommitOptAlt03),
        "partitionopt--4b3c898e221320ae" => Some(DdlRule::PartitionOptAlt01),
        "partitionopt_partition_by_partitionmethod_partit--f797297e4042079b" => {
            Some(DdlRule::PartitionOptAlt02)
        }
        "globalorlocal_local--5bfbf23384cf0674" => Some(DdlRule::GlobalOrLocalAlt01),
        "globalorlocal_global--127d6b91af671212" => Some(DdlRule::GlobalOrLocalAlt02),
        "updateindexelem_identifier_globalorlocal--98a66199d3f32e90" => {
            Some(DdlRule::UpdateIndexElemAlt01)
        }
        "updateindexeslist_updateindexelem--c9543c153a5bb843" => {
            Some(DdlRule::UpdateIndexesListAlt01)
        }
        "updateindexeslist_updateindexeslist_updateindexe--aace264175a9c184" => {
            Some(DdlRule::UpdateIndexesListAlt02)
        }
        "updateindexesopt--54022a2981f50455" => Some(DdlRule::UpdateIndexesOptAlt01),
        "updateindexesopt_update_indexes_updateindexeslis--d4b073fed946234d" => {
            Some(DdlRule::UpdateIndexesOptAlt02)
        }
        "subpartitionmethod_linearopt_key_partitionkeyalg--c60d13bf40f803dc" => {
            Some(DdlRule::SubPartitionMethodAlt01)
        }
        "subpartitionmethod_linearopt_hash_bitexpr--a3fc286d48716dc6" => {
            Some(DdlRule::SubPartitionMethodAlt02)
        }
        "partitionkeyalgorithmopt--2194096d02302a2a" => {
            Some(DdlRule::PartitionKeyAlgorithmOptAlt01)
        }
        "partitionkeyalgorithmopt_algorithm_eq_num--4292aacec37f9230" => {
            Some(DdlRule::PartitionKeyAlgorithmOptAlt02)
        }
        "partitionmethod_range_bitexpr_partitionintervalo--40a345e932f0bb3b" => {
            Some(DdlRule::PartitionMethodAlt02)
        }
        "partitionmethod_range_fieldsorcolumns_columnname--e2a25ecb70b10517" => {
            Some(DdlRule::PartitionMethodAlt03)
        }
        "partitionmethod_list_bitexpr--2cb7566b26d4a094" => Some(DdlRule::PartitionMethodAlt04),
        "partitionmethod_list_fieldsorcolumns_columnnamel--602ff6637d675292" => {
            Some(DdlRule::PartitionMethodAlt05)
        }
        "partitionmethod_system_time_interval_expression--b3619a1e1a19d3e0" => {
            Some(DdlRule::PartitionMethodAlt06)
        }
        "partitionmethod_system_time_limit_lengthnum--1e58d8f21f25aad7" => {
            Some(DdlRule::PartitionMethodAlt07)
        }
        "partitionmethod_system_time--f4c4da638e0b209e" => Some(DdlRule::PartitionMethodAlt08),
        "partitionintervalopt--179d72a36b58d0c5" => Some(DdlRule::PartitionIntervalOptAlt01),
        "partitionintervalopt_interval_intervalexpr_first--98aaf6282a61ecc5" => {
            Some(DdlRule::PartitionIntervalOptAlt02)
        }
        "intervalexpr_bitexpr--033a8713c7f64afa" => Some(DdlRule::IntervalExprAlt01),
        "intervalexpr_bitexpr_timeunit--1151e9c69c322fe5" => Some(DdlRule::IntervalExprAlt02),
        "nullpartopt--75fd252cc8d209d6" => Some(DdlRule::NullPartOptAlt01),
        "nullpartopt_null_partition--e191af49763256a6" => Some(DdlRule::NullPartOptAlt02),
        "maxvalpartopt--ca98091762df558a" => Some(DdlRule::MaxValPartOptAlt01),
        "maxvalpartopt_maxvalue_partition--6e48eb91f03aaf90" => Some(DdlRule::MaxValPartOptAlt02),
        "firstandlastpartopt--3af6cda4315568bc" => Some(DdlRule::FirstAndLastPartOptAlt01),
        "firstandlastpartopt_first_partition_less_than_bi--d568cffeb30da719" => {
            Some(DdlRule::FirstAndLastPartOptAlt02)
        }
        "linearopt--1dcbefacbe456b79" => Some(DdlRule::LinearOptAlt01),
        "subpartitionopt--11ee5655a1f927de" => Some(DdlRule::SubPartitionOptAlt01),
        "subpartitionopt_subpartition_by_subpartitionmeth--182da443938cf278" => {
            Some(DdlRule::SubPartitionOptAlt02)
        }
        "subpartitionnumopt--4a5dca94e62069cc" => Some(DdlRule::SubPartitionNumOptAlt01),
        "subpartitionnumopt_subpartitions_lengthnum--0d4b08298fb78b30" => {
            Some(DdlRule::SubPartitionNumOptAlt02)
        }
        "partitionnumopt--7f8d53e37c98329c" => Some(DdlRule::PartitionNumOptAlt01),
        "partitionnumopt_partitions_lengthnum--94eb24d9ce0456d0" => {
            Some(DdlRule::PartitionNumOptAlt02)
        }
        "partitiondefinitionlistopt_prec_lowerthancreatet--a496f8dc164d5d58" => {
            Some(DdlRule::PartitionDefinitionListOptAlt01)
        }
        "partitiondefinitionlistopt_partitiondefinitionli--407eebebfafd7042" => {
            Some(DdlRule::PartitionDefinitionListOptAlt02)
        }
        "partitiondefinitionlist_partitiondefinition--c559a2587ebe976c" => {
            Some(DdlRule::PartitionDefinitionListAlt01)
        }
        "partitiondefinitionlist_partitiondefinitionlist--010807db9626f27f" => {
            Some(DdlRule::PartitionDefinitionListAlt02)
        }
        "partitiondefinition_partition_identifier_partdef--c5ab128733ecc809" => {
            Some(DdlRule::PartitionDefinitionAlt01)
        }
        "subpartdefinitionlistopt--9e628e71326386ae" => {
            Some(DdlRule::SubPartDefinitionListOptAlt01)
        }
        "subpartdefinitionlistopt_subpartdefinitionlist--ed41347fbebdf9de" => {
            Some(DdlRule::SubPartDefinitionListOptAlt02)
        }
        "subpartdefinitionlist_subpartdefinition--8bae966a7f48c82a" => {
            Some(DdlRule::SubPartDefinitionListAlt01)
        }
        "subpartdefinitionlist_subpartdefinitionlist_subp--027631746799fbc4" => {
            Some(DdlRule::SubPartDefinitionListAlt02)
        }
        "subpartdefinition_subpartition_identifier_partde--efe0f843909f32be" => {
            Some(DdlRule::SubPartDefinitionAlt01)
        }
        "partdefoptionlist--889be41565240eaa" => Some(DdlRule::PartDefOptionListAlt01),
        "partdefoptionlist_partdefoptionlist_partdefoptio--c30b41046b1c643d" => {
            Some(DdlRule::PartDefOptionListAlt02)
        }
        "partdefoption_comment_eqopt_stringlit--e5dbfa454fc6c0fb" => {
            Some(DdlRule::PartDefOptionAlt01)
        }
        "partdefoption_engine_eqopt_stringname--4f1df6ae4c15ba24" => {
            Some(DdlRule::PartDefOptionAlt02)
        }
        "partdefoption_storage_engine_eqopt_stringname--3e6e9e84c94c3d4f" => {
            Some(DdlRule::PartDefOptionAlt03)
        }
        "partdefoption_engine_attribute_eqopt_stringname--88b8c9b7f70e595f" => {
            Some(DdlRule::PartDefOptionAlt04)
        }
        "partdefoption_secondary_engine_attribute_eqopt_s--0b7854434714603c" => {
            Some(DdlRule::PartDefOptionAlt05)
        }
        "partdefoption_insert_method_eqopt_stringname--10c667d418009a03" => {
            Some(DdlRule::PartDefOptionAlt06)
        }
        "partdefoption_data_directory_eqopt_stringlit--f6405b9ac86d47db" => {
            Some(DdlRule::PartDefOptionAlt07)
        }
        "partdefoption_index_directory_eqopt_stringlit--e5a5f6e0522982e5" => {
            Some(DdlRule::PartDefOptionAlt08)
        }
        "partdefoption_max_rows_eqopt_lengthnum--fe706a4226f3f2de" => {
            Some(DdlRule::PartDefOptionAlt09)
        }
        "partdefoption_min_rows_eqopt_lengthnum--d913d42f557834d0" => {
            Some(DdlRule::PartDefOptionAlt10)
        }
        "partdefoption_tablespace_eqopt_identifier--386944c723abc3bd" => {
            Some(DdlRule::PartDefOptionAlt11)
        }
        "partdefoption_nodegroup_eqopt_lengthnum--fef2476b38f09717" => {
            Some(DdlRule::PartDefOptionAlt12)
        }
        "partdefoption_placementpolicyoption--a11313da1318f75d" => {
            Some(DdlRule::PartDefOptionAlt13)
        }
        "partdefvaluesopt--ef42a89713b7ce16" => Some(DdlRule::PartDefValuesOptAlt01),
        "partdefvaluesopt_values_less_than_maxvalue--e293069af7251c8e" => {
            Some(DdlRule::PartDefValuesOptAlt02)
        }
        "partdefvaluesopt_values_less_than_maxvalueorexpr--b75853e96d1f15d6" => {
            Some(DdlRule::PartDefValuesOptAlt03)
        }
        "partdefvaluesopt_default--a9d59fc792ee7162" => Some(DdlRule::PartDefValuesOptAlt04),
        "partdefvaluesopt_values_in_defaultorexpressionli--c3f323fff0db4461" => {
            Some(DdlRule::PartDefValuesOptAlt05)
        }
        "partdefvaluesopt_history--c08141cd514408ad" => Some(DdlRule::PartDefValuesOptAlt06),
        "partdefvaluesopt_current--c206cd7ada7e4aca" => Some(DdlRule::PartDefValuesOptAlt07),
        "duplicateopt--270d37797227becf" => Some(DdlRule::DuplicateOptAlt01),
        "duplicateopt_ignore--ba2cf73f308b6e6c" => Some(DdlRule::DuplicateOptAlt02),
        "duplicateopt_replace--31371c01fc4c117a" => Some(DdlRule::DuplicateOptAlt03),
        "createtableselectopt--62942f2a7c38258c" => Some(DdlRule::CreateTableSelectOptAlt01),
        "createtableselectopt_setoprstmt--089efaa3c25ded6c" => {
            Some(DdlRule::CreateTableSelectOptAlt02)
        }
        "createtableselectopt_selectstmt--c5a9477677b32b03" => {
            Some(DdlRule::CreateTableSelectOptAlt03)
        }
        "createtableselectopt_selectstmtwithclause--12dab08b0c62afce" => {
            Some(DdlRule::CreateTableSelectOptAlt04)
        }
        "createtableselectopt_subselect--5f7a3718852c02bd" => {
            Some(DdlRule::CreateTableSelectOptAlt05)
        }
        "createviewselectopt_subselect--50ca4b0f711dcf1e" => {
            Some(DdlRule::CreateViewSelectOptAlt04)
        }
        "liketablewithorwithoutparen_like_tablename--55fe3548d7ce6610" => {
            Some(DdlRule::LikeTableWithOrWithoutParenAlt01)
        }
        "liketablewithorwithoutparen_like_tablename--bf28882f989296e5" => {
            Some(DdlRule::LikeTableWithOrWithoutParenAlt02)
        }
        "createviewstmt_create_orreplace_viewalgorithm_vi--c3c438addde65285" => {
            Some(DdlRule::CreateViewStmtAlt01)
        }
        "orreplace--14b9be3fc9557e04" => Some(DdlRule::OrReplaceAlt01),
        "orreplace_or_replace--1339d32d960ca8ca" => Some(DdlRule::OrReplaceAlt02),
        "viewalgorithm--513da9a513490c75" => Some(DdlRule::ViewAlgorithmAlt01),
        "viewalgorithm_algorithm_undefined--afbbdb63f83bce36" => Some(DdlRule::ViewAlgorithmAlt02),
        "viewalgorithm_algorithm_merge--37f5f264ab274f4e" => Some(DdlRule::ViewAlgorithmAlt03),
        "viewalgorithm_algorithm_temptable--f29f68a8791b7eac" => Some(DdlRule::ViewAlgorithmAlt04),
        "viewdefiner--d62677b31085ef63" => Some(DdlRule::ViewDefinerAlt01),
        "viewdefiner_definer_username--4727b866a992da5a" => Some(DdlRule::ViewDefinerAlt02),
        "viewsqlsecurity--7d38c1e3a33b2dda" => Some(DdlRule::ViewSQLSecurityAlt01),
        "viewsqlsecurity_sql_security_definer--1988d86c22118af0" => {
            Some(DdlRule::ViewSQLSecurityAlt02)
        }
        "viewsqlsecurity_sql_security_invoker--f9d45436d5db25a7" => {
            Some(DdlRule::ViewSQLSecurityAlt03)
        }
        "viewfieldlist--5240a7c36b69fae4" => Some(DdlRule::ViewFieldListAlt01),
        "viewfieldlist_columnlist--658de3113062179a" => Some(DdlRule::ViewFieldListAlt02),
        "columnlist_identifier--fba69b02088f0b8d" => Some(DdlRule::ColumnListAlt01),
        "columnlist_columnlist_identifier--27f40bb1c61d87db" => Some(DdlRule::ColumnListAlt02),
        "viewcheckoption--bbe158ab33f08659" => Some(DdlRule::ViewCheckOptionAlt01),
        "viewcheckoption_with_cascaded_check_option--4865ebbaf826d771" => {
            Some(DdlRule::ViewCheckOptionAlt02)
        }
        "viewcheckoption_with_local_check_option--0939e3018ff72080" => {
            Some(DdlRule::ViewCheckOptionAlt03)
        }
        "dropdatabasestmt_drop_databasesym_ifexists_dbnam--0c63b955250bf475" => {
            Some(DdlRule::DropDatabaseStmtAlt01)
        }
        "dropindexstmt_drop_index_ifexists_identifier_on--91625fc0dde6f734" => {
            Some(DdlRule::DropIndexStmtAlt01)
        }
        "dropindexstmt_drop_hypo_index_ifexists_identifie--ca74defcfae40550" => {
            Some(DdlRule::DropIndexStmtAlt02)
        }
        "droptablestmt_drop_opttemporary_tableortables_if--9482f67de3dfd116" => {
            Some(DdlRule::DropTableStmtAlt01)
        }
        "opttemporary--77c4ff4cfe485681" => Some(DdlRule::OptTemporaryAlt01),
        "opttemporary_temporary--ef39c8ab85f77e21" => Some(DdlRule::OptTemporaryAlt02),
        "opttemporary_global_temporary--d2a22b74d6105104" => Some(DdlRule::OptTemporaryAlt03),
        "dropviewstmt_drop_view_tablenamelist_restrictorc--38ec8617ff0ef497" => {
            Some(DdlRule::DropViewStmtAlt01)
        }
        "dropviewstmt_drop_view_if_exists_tablenamelist_r--60a93662c005f5a2" => {
            Some(DdlRule::DropViewStmtAlt02)
        }
        "indexname--15093ed756e3efd8" => Some(DdlRule::IndexNameAlt01),
        "indexname_identifier--4964b09f88a234ca" => Some(DdlRule::IndexNameAlt02),
        "indexoptionlist--00ef3b9c111802aa" => Some(DdlRule::IndexOptionListAlt01),
        "indexoptionlist_indexoptionlist_indexoption--282b04ab93fd79f1" => {
            Some(DdlRule::IndexOptionListAlt02)
        }
        "indexoption_key_block_size_eqopt_lengthnum--760da59246975bb1" => {
            Some(DdlRule::IndexOptionAlt01)
        }
        "indexoption_add_columnar_replica_on_demand--eb4f64c55a7a20cf" => {
            Some(DdlRule::IndexOptionAlt02)
        }
        "indexoption_indextype--824763f80c469869" => Some(DdlRule::IndexOptionAlt03),
        "indexoption_with_parser_identifier--116fa46e546eb17f" => Some(DdlRule::IndexOptionAlt04),
        "indexoption_comment_stringlit--04dd905940e17afc" => Some(DdlRule::IndexOptionAlt05),
        "indexoption_indexinvisible--8ac9e47674d8ed64" => Some(DdlRule::IndexOptionAlt06),
        "indexoption_withclustered--c1e7eea7b6b1d02a" => Some(DdlRule::IndexOptionAlt07),
        "indexoption_global--adc93bd4b39d1556" => Some(DdlRule::IndexOptionAlt08),
        "indexoption_local--fc5c3baacc1aace8" => Some(DdlRule::IndexOptionAlt09),
        "indexoption_pre_split_regions_eqopt_splitoption--df45595c5f25992e" => {
            Some(DdlRule::IndexOptionAlt10)
        }
        "indexoption_pre_split_regions_eqopt_int64num--7b24a8327329344b" => {
            Some(DdlRule::IndexOptionAlt11)
        }
        "indexoption_secondary_engine_attribute_eqopt_str--1513bac204c7b43c" => {
            Some(DdlRule::IndexOptionAlt12)
        }
        "indexoption_where_expression--95c29119b3b81632" => Some(DdlRule::IndexOptionAlt13),
        "indexoption_pre_split_regions_eqopt_auto--24c39646c1dcd9f3" => {
            Some(DdlRule::IndexOptionAlt14)
        }
        "indexnameandtypeopt_indexname--eb606e683a56f4c2" => {
            Some(DdlRule::IndexNameAndTypeOptAlt01)
        }
        "indexnameandtypeopt_indexname_using_indextypenam--cd0a678e3a7bf165" => {
            Some(DdlRule::IndexNameAndTypeOptAlt02)
        }
        "indexnameandtypeopt_identifier_type_indextypenam--28feae7d7fe05043" => {
            Some(DdlRule::IndexNameAndTypeOptAlt03)
        }
        "indextypeopt--36955d3679f39360" => Some(DdlRule::IndexTypeOptAlt01),
        "indextype_using_indextypename--8c3ab4c675a35e61" => Some(DdlRule::IndexTypeAlt01),
        "indextype_type_indextypename--384e8f6abc31ea2d" => Some(DdlRule::IndexTypeAlt02),
        "indextypename_btree--5659ca6e89b95ff9" => Some(DdlRule::IndexTypeNameAlt01),
        "indextypename_hash--b74b57c8bb63e24d" => Some(DdlRule::IndexTypeNameAlt02),
        "indextypename_rtree--4b928ba41e4396e9" => Some(DdlRule::IndexTypeNameAlt03),
        "indextypename_hypo--6bc15ffe374b808f" => Some(DdlRule::IndexTypeNameAlt04),
        "indextypename_hnsw--8770cf9f03b02ee1" => Some(DdlRule::IndexTypeNameAlt05),
        "indextypename_inverted--d9add7199c4bbcf2" => Some(DdlRule::IndexTypeNameAlt06),
        "indexinvisible_visible--30bb62c75ed63d2b" => Some(DdlRule::IndexInvisibleAlt01),
        "indexinvisible_invisible--d0a696de61eb8ee4" => Some(DdlRule::IndexInvisibleAlt02),
        "constraint_constraintkeywordopt_constraintelem--66931a345b4cd480" => {
            Some(DdlRule::ConstraintAlt01)
        }
        "constraintvectorindex_vector_index_ifnotexists_i--b0c3790fc5692b58" => {
            Some(DdlRule::ConstraintVectorIndexAlt01)
        }
        "constraintcolumnarindex_columnar_index_ifnotexis--a00fed6f94c670a0" => {
            Some(DdlRule::ConstraintColumnarIndexAlt01)
        }
        "constraintwithcolumnarindex_constraintcolumnarin--5f069e537aa14d46" => {
            Some(DdlRule::ConstraintWithColumnarIndexAlt03)
        }
        "tableelementlist_tableelement--c37ebd2f236b6edc" => Some(DdlRule::TableElementListAlt01),
        "tableelementlist_tableelementlist_tableelement--c8054c3fed8e555c" => {
            Some(DdlRule::TableElementListAlt02)
        }
        "tableelementlistopt_prec_lowerthancreatetablesel--e47a06ba7cc73bdd" => {
            Some(DdlRule::TableElementListOptAlt01)
        }
        "tableelementlistopt_tableelementlist--c649cca95fda7546" => {
            Some(DdlRule::TableElementListOptAlt02)
        }
        "tableoption_defaultkwdopt_charsetkw_eqopt_charse--9fd98902a024fd0d" => {
            Some(DdlRule::TableOptionAlt02)
        }
        "tableoption_defaultkwdopt_collate_eqopt_collatio--2ab51924fa4b1332" => {
            Some(DdlRule::TableOptionAlt03)
        }
        "tableoption_forceopt_auto_increment_eqopt_length--748fc52721c63011" => {
            Some(DdlRule::TableOptionAlt04)
        }
        "tableoption_auto_id_cache_eqopt_lengthnum--15d211056d394306" => {
            Some(DdlRule::TableOptionAlt05)
        }
        "tableoption_forceopt_auto_random_base_eqopt_leng--debd59767b05186d" => {
            Some(DdlRule::TableOptionAlt06)
        }
        "tableoption_avg_row_length_eqopt_lengthnum--dc274774158f36a8" => {
            Some(DdlRule::TableOptionAlt07)
        }
        "tableoption_connection_eqopt_stringlit--3a2fe2d871ea7f2a" => {
            Some(DdlRule::TableOptionAlt08)
        }
        "tableoption_checksum_eqopt_lengthnum--b7dd7ae2d91fd309" => Some(DdlRule::TableOptionAlt09),
        "tableoption_table_checksum_eqopt_lengthnum--fd6cdafbf1b2ce82" => {
            Some(DdlRule::TableOptionAlt10)
        }
        "tableoption_password_eqopt_stringlit--d9634180fe3f4ea1" => Some(DdlRule::TableOptionAlt11),
        "tableoption_compression_eqopt_stringlit--8dca116219cd8282" => {
            Some(DdlRule::TableOptionAlt12)
        }
        "tableoption_key_block_size_eqopt_lengthnum--07667832d8e89bdd" => {
            Some(DdlRule::TableOptionAlt13)
        }
        "tableoption_delay_key_write_eqopt_lengthnum--b9c2ba5515d5a7bf" => {
            Some(DdlRule::TableOptionAlt14)
        }
        "tableoption_rowformat--cc709b7c56507daa" => Some(DdlRule::TableOptionAlt15),
        "tableoption_stats_persistent_eqopt_statspersiste--3b799e8a3101097c" => {
            Some(DdlRule::TableOptionAlt16)
        }
        "tableoption_stats_auto_recalc_eqopt_lengthnum--a90df478553edfc6" => {
            Some(DdlRule::TableOptionAlt17)
        }
        "tableoption_stats_auto_recalc_eqopt_default--9451e58b3131adf5" => {
            Some(DdlRule::TableOptionAlt18)
        }
        "tableoption_stats_sample_pages_eqopt_lengthnum--5787b26d03774f3f" => {
            Some(DdlRule::TableOptionAlt19)
        }
        "tableoption_stats_sample_pages_eqopt_default--b04cf22fb614dba0" => {
            Some(DdlRule::TableOptionAlt20)
        }
        "tableoption_stats_buckets_eqopt_lengthnum--49785559bb8fa9cf" => {
            Some(DdlRule::TableOptionAlt21)
        }
        "tableoption_stats_topn_eqopt_lengthnum--7f2851238bcc0a15" => {
            Some(DdlRule::TableOptionAlt22)
        }
        "tableoption_stats_sample_rate_eqopt_numliteral--921c4b65b8d29958" => {
            Some(DdlRule::TableOptionAlt23)
        }
        "tableoption_stats_col_choice_eqopt_stringlit--97f22caaabb8e9d6" => {
            Some(DdlRule::TableOptionAlt24)
        }
        "tableoption_stats_col_list_eqopt_stringlit--60a23f27d0298157" => {
            Some(DdlRule::TableOptionAlt25)
        }
        "tableoption_shard_row_id_bits_eqopt_lengthnum--b88ba1421d28f024" => {
            Some(DdlRule::TableOptionAlt26)
        }
        "tableoption_pre_split_regions_eqopt_lengthnum--c6183d68596fec7e" => {
            Some(DdlRule::TableOptionAlt27)
        }
        "tableoption_pack_keys_eqopt_statspersistentval--5386ef1f70461fa5" => {
            Some(DdlRule::TableOptionAlt28)
        }
        "tableoption_storage_memory--3f68052b73b0ddef" => Some(DdlRule::TableOptionAlt29),
        "tableoption_storage_disk--a4bd7b22d5e809d1" => Some(DdlRule::TableOptionAlt30),
        "tableoption_secondary_engine_eqopt_null--dfcdd7acc7b35182" => {
            Some(DdlRule::TableOptionAlt31)
        }
        "tableoption_secondary_engine_eqopt_stringname--53bd104ea743638b" => {
            Some(DdlRule::TableOptionAlt32)
        }
        "tableoption_union_eqopt_tablenamelistopt--7d6e1e22ecbd6140" => {
            Some(DdlRule::TableOptionAlt33)
        }
        "tableoption_encryption_eqopt_encryptionopt--910de355513dc9d1" => {
            Some(DdlRule::TableOptionAlt34)
        }
        "tableoption_ttl_eqopt_identifier_interval_litera--db1c3b42706fab7d" => {
            Some(DdlRule::TableOptionAlt35)
        }
        "tableoption_ttl_enable_eqopt_stringlit--069a867789bfdf30" => {
            Some(DdlRule::TableOptionAlt36)
        }
        "tableoption_ttl_job_interval_eqopt_stringlit--2853396e6bd0cd46" => {
            Some(DdlRule::TableOptionAlt37)
        }
        "tableoption_autoextend_size_eqopt_stringname--043a7799569bcdc1" => {
            Some(DdlRule::TableOptionAlt38)
        }
        "tableoption_affinity_eqopt_stringname--181b48224c254702" => {
            Some(DdlRule::TableOptionAlt39)
        }
        "tableoption_page_checksum_eqopt_lengthnum--bac85c9f7ad71863" => {
            Some(DdlRule::TableOptionAlt40)
        }
        "tableoption_page_compressed_eqopt_lengthnum--f9a4fcd42bb628e5" => {
            Some(DdlRule::TableOptionAlt41)
        }
        "tableoption_page_compression_level_eqopt_lengthn--4830a24c72385b15" => {
            Some(DdlRule::TableOptionAlt42)
        }
        "tableoption_transactional_eqopt_lengthnum--d961fc3dcd5d8721" => {
            Some(DdlRule::TableOptionAlt43)
        }
        "tableoption_sequence_eqopt_lengthnum--b43bc5cc4fcbcb53" => Some(DdlRule::TableOptionAlt44),
        "tableoption_ietf_quotes_eqopt_stringname--9711f0fefbb702bc" => {
            Some(DdlRule::TableOptionAlt45)
        }
        "tableoption_storage_class_eqopt_stringname--ac2c3f5d9cce8d76" => {
            Some(DdlRule::TableOptionAlt46)
        }
        "forceopt--0414b46c40bc0ea5" => Some(DdlRule::ForceOptAlt01),
        "forceopt_force--c15469c14f426f25" => Some(DdlRule::ForceOptAlt02),
        "createtableoptionlistopt_prec_lowerthancreatetab--15b3af128ce3a60a" => {
            Some(DdlRule::CreateTableOptionListOptAlt01)
        }
        "createtableoptionlist_createtableoption--2484793c867406e4" => {
            Some(DdlRule::CreateTableOptionListAlt01)
        }
        "createtableoptionlist_createtableoptionlist_crea--0c4c46e5e204447d" => {
            Some(DdlRule::CreateTableOptionListAlt02)
        }
        "createtableoptionlist_createtableoptionlist_crea--6a3e3e45fcebd449" => {
            Some(DdlRule::CreateTableOptionListAlt03)
        }
        "createtableoption_start_transaction--fdf9ea8f33ddd6f1" => {
            Some(DdlRule::CreateTableOptionAlt02)
        }
        "tableoptionlist_tableoption--a670551b2fd5ed9c" => Some(DdlRule::TableOptionListAlt01),
        "tableoptionlist_tableoptionlist_tableoption--3312fc2ee3c074d9" => {
            Some(DdlRule::TableOptionListAlt02)
        }
        "tableoptionlist_tableoptionlist_tableoption--3eae42744435809d" => {
            Some(DdlRule::TableOptionListAlt03)
        }
        "truncatetablestmt_truncate_opttable_tablename--085b7d5dc89c46ec" => {
            Some(DdlRule::TruncateTableStmtAlt01)
        }
        "rowformat_row_format_eqopt_default--bf7856769b385783" => Some(DdlRule::RowFormatAlt01),
        "rowformat_row_format_eqopt_dynamic--086035675125b80b" => Some(DdlRule::RowFormatAlt02),
        "rowformat_row_format_eqopt_fixed--a8a8e4009d2685ee" => Some(DdlRule::RowFormatAlt03),
        "rowformat_row_format_eqopt_compressed--7188e0932e03da2f" => Some(DdlRule::RowFormatAlt04),
        "rowformat_row_format_eqopt_redundant--4e5a190f2243f2eb" => Some(DdlRule::RowFormatAlt05),
        "rowformat_row_format_eqopt_compact--38ac861b88ae73c3" => Some(DdlRule::RowFormatAlt06),
        "rowformat_row_format_eqopt_tokudb_default--42a9ac76a3310f9f" => {
            Some(DdlRule::RowFormatAlt07)
        }
        "rowformat_row_format_eqopt_tokudb_fast--7e95784e71600586" => Some(DdlRule::RowFormatAlt08),
        "rowformat_row_format_eqopt_tokudb_small--7db74d0cc4b31ef1" => {
            Some(DdlRule::RowFormatAlt09)
        }
        "rowformat_row_format_eqopt_tokudb_zlib--2bdb8006d020c897" => Some(DdlRule::RowFormatAlt10),
        "rowformat_row_format_eqopt_tokudb_zstd--97ed353eeefd6161" => Some(DdlRule::RowFormatAlt11),
        "rowformat_row_format_eqopt_tokudb_quicklz--c125360fbcd9e74b" => {
            Some(DdlRule::RowFormatAlt12)
        }
        "rowformat_row_format_eqopt_tokudb_lzma--1c72a000b9a23f28" => Some(DdlRule::RowFormatAlt13),
        "rowformat_row_format_eqopt_tokudb_snappy--0f62c959fecd6251" => {
            Some(DdlRule::RowFormatAlt14)
        }
        "rowformat_row_format_eqopt_tokudb_uncompressed--2ac5f28e0e6192b6" => {
            Some(DdlRule::RowFormatAlt15)
        }
        "numerictype_integertype_optfieldlen_fieldopts--ec97fd5d088f6c01" => {
            Some(DdlRule::NumericTypeAlt01)
        }
        "numerictype_booleantype_fieldopts--31d2f06d6f785bd1" => Some(DdlRule::NumericTypeAlt02),
        "numerictype_fixedpointtype_floatopt_fieldopts--8115ff4a7ffc7c56" => {
            Some(DdlRule::NumericTypeAlt03)
        }
        "numerictype_floatingpointtype_floatopt_fieldopts--2e26e3642483ce28" => {
            Some(DdlRule::NumericTypeAlt04)
        }
        "numerictype_bitvaluetype_optfieldlen--b4c96e0960ab3eaf" => Some(DdlRule::NumericTypeAlt05),
        "integertype_tinyint--374ded357676a1f9" => Some(DdlRule::IntegerTypeAlt01),
        "integertype_smallint--02b3d3056019c344" => Some(DdlRule::IntegerTypeAlt02),
        "integertype_mediumint--7f66c7ee847923ec" => Some(DdlRule::IntegerTypeAlt03),
        "integertype_middleint--ef1903dfb53b834a" => Some(DdlRule::IntegerTypeAlt04),
        "integertype_int--01d1e697874a09ef" => Some(DdlRule::IntegerTypeAlt05),
        "integertype_int1--6181a67ae2a357f0" => Some(DdlRule::IntegerTypeAlt06),
        "integertype_int2--617e407ae2a074c7" => Some(DdlRule::IntegerTypeAlt07),
        "integertype_int3--617b1e7ae29e052a" => Some(DdlRule::IntegerTypeAlt08),
        "integertype_int4--6177b87ae29b2201" => Some(DdlRule::IntegerTypeAlt09),
        "integertype_int8--61a0407ae2bd5b2d" => Some(DdlRule::IntegerTypeAlt10),
        "integertype_integer--05302240cf9bb566" => Some(DdlRule::IntegerTypeAlt11),
        "integertype_bigint--89e6f65448ccb561" => Some(DdlRule::IntegerTypeAlt12),
        "booleantype_bool--04a2d4a6a90c5bb2" => Some(DdlRule::BooleanTypeAlt01),
        "booleantype_boolean--d94c02981acb045e" => Some(DdlRule::BooleanTypeAlt02),
        "fixedpointtype_decimal--3553e46d8cfe9bd5" => Some(DdlRule::FixedPointTypeAlt01),
        "fixedpointtype_numeric--02dadb0debbd62d1" => Some(DdlRule::FixedPointTypeAlt02),
        "fixedpointtype_fixed--1fd9746252131306" => Some(DdlRule::FixedPointTypeAlt03),
        "floatingpointtype_float--f500a976ae7ec6e0" => Some(DdlRule::FloatingPointTypeAlt01),
        "floatingpointtype_real--004e52322d3031b2" => Some(DdlRule::FloatingPointTypeAlt02),
        "floatingpointtype_double--d763dbe82f8d2c65" => Some(DdlRule::FloatingPointTypeAlt03),
        "floatingpointtype_double_precision--b50ea61f96d65481" => {
            Some(DdlRule::FloatingPointTypeAlt04)
        }
        "floatingpointtype_float4--cf31b6aa81aba720" => Some(DdlRule::FloatingPointTypeAlt05),
        "floatingpointtype_float8--cf3f4eaa81b733c4" => Some(DdlRule::FloatingPointTypeAlt06),
        "bitvaluetype_bit--d3001e5bddc335d5" => Some(DdlRule::BitValueTypeAlt01),
        "stringtype_char_fieldlen_optbinary--59c0aea1f2867b7c" => Some(DdlRule::StringTypeAlt01),
        "stringtype_char_optbinary--9facf0752d8f8655" => Some(DdlRule::StringTypeAlt02),
        "stringtype_nchar_fieldlen_optbinary--24d6da6875c81860" => Some(DdlRule::StringTypeAlt03),
        "stringtype_nchar_optbinary--c4a48f4390d06179" => Some(DdlRule::StringTypeAlt04),
        "stringtype_varchar_fieldlen_optbinary--7d0b2c6c6ae88fd9" => Some(DdlRule::StringTypeAlt05),
        "stringtype_nvarchar_fieldlen_optbinary--2bfddd00da1c7bbd" => {
            Some(DdlRule::StringTypeAlt06)
        }
        "stringtype_binary_optfieldlen--e126a614c0799a5c" => Some(DdlRule::StringTypeAlt07),
        "stringtype_varbinary_fieldlen--478c5408468254b2" => Some(DdlRule::StringTypeAlt08),
        "stringtype_blobtype--4c148c3a58aa81d6" => Some(DdlRule::StringTypeAlt09),
        "stringtype_texttype_optcharsetwithoptbinary--4a960efc9680e5c7" => {
            Some(DdlRule::StringTypeAlt10)
        }
        "stringtype_enum_textstringlist_optcharsetwithopt--c1f9827261aab888" => {
            Some(DdlRule::StringTypeAlt11)
        }
        "stringtype_set_textstringlist_optcharsetwithoptb--901429312c337f2f" => {
            Some(DdlRule::StringTypeAlt12)
        }
        "stringtype_json--0057dc67b827339f" => Some(DdlRule::StringTypeAlt13),
        "stringtype_uuid--7ff57e1ae01c7c44" => Some(DdlRule::StringTypeAlt14),
        "stringtype_long_varchar_optcharsetwithoptbinary--9c442880e89d65af" => {
            Some(DdlRule::StringTypeAlt15)
        }
        "stringtype_long_optcharsetwithoptbinary--0cd0d35de3590246" => {
            Some(DdlRule::StringTypeAlt16)
        }
        "stringtype_vector_optvectorelementtype_optfieldl--4beeb6894f61c0a0" => {
            Some(DdlRule::StringTypeAlt17)
        }
        "blobtype_tinyblob--246312c0e5df2374" => Some(DdlRule::BlobTypeAlt01),
        "blobtype_blob_optfieldlen--75d0e0d76b6fd384" => Some(DdlRule::BlobTypeAlt02),
        "blobtype_mediumblob--4dcea3c04920ca8b" => Some(DdlRule::BlobTypeAlt03),
        "blobtype_longblob--744672b6af11131c" => Some(DdlRule::BlobTypeAlt04),
        "blobtype_long_varbinary--cc72242aeb29a4c5" => Some(DdlRule::BlobTypeAlt05),
        "texttype_tinytext--bb05d9f639f6ceb4" => Some(DdlRule::TextTypeAlt01),
        "texttype_text_optfieldlen--2a35c7b529efa6f8" => Some(DdlRule::TextTypeAlt02),
        "texttype_mediumtext--3aa097a5820a09ab" => Some(DdlRule::TextTypeAlt03),
        "texttype_longtext--1e1dd8cf8c6b1bdc" => Some(DdlRule::TextTypeAlt04),
        "optcharsetwithoptbinary_ascii--c84151e41d86bed8" => {
            Some(DdlRule::OptCharsetWithOptBinaryAlt02)
        }
        "optcharsetwithoptbinary_unicode--233c60e1c3cffa2c" => {
            Some(DdlRule::OptCharsetWithOptBinaryAlt03)
        }
        "optcharsetwithoptbinary_byte--17b83f71093c5a69" => {
            Some(DdlRule::OptCharsetWithOptBinaryAlt04)
        }
        "dateandtimetype_date--1291b2631724242a" => Some(DdlRule::DateAndTimeTypeAlt01),
        "dateandtimetype_datetime_optfieldlen--9b9bc1dab8240dcf" => {
            Some(DdlRule::DateAndTimeTypeAlt02)
        }
        "dateandtimetype_timestamp_optfieldlen--be0e7ff1039bd60c" => {
            Some(DdlRule::DateAndTimeTypeAlt03)
        }
        "dateandtimetype_time_optfieldlen--6b9c4a01f2a785e3" => Some(DdlRule::DateAndTimeTypeAlt04),
        "dateandtimetype_year_optfieldlen_fieldopts--7742abed9ab9c467" => {
            Some(DdlRule::DateAndTimeTypeAlt05)
        }
        "fieldlen_lengthnum--fc2418898f7f626c" => Some(DdlRule::FieldLenAlt01),
        "optfieldlen--74183abafc285bf5" => Some(DdlRule::OptFieldLenAlt01),
        "fieldopt_unsigned--b67f4934ace6e15c" => Some(DdlRule::FieldOptAlt01),
        "fieldopt_signed--8ef8266ffc136865" => Some(DdlRule::FieldOptAlt02),
        "fieldopt_zerofill--6857e48841eab240" => Some(DdlRule::FieldOptAlt03),
        "fieldopts--da59d64cd2e451f5" => Some(DdlRule::FieldOptsAlt01),
        "fieldopts_fieldopts_fieldopt--591ab58ac4c81e93" => Some(DdlRule::FieldOptsAlt02),
        "floatopt--0bb30da5257e14c2" => Some(DdlRule::FloatOptAlt01),
        "floatopt_fieldlen--c716affbb9fc9a48" => Some(DdlRule::FloatOptAlt02),
        "precision_lengthnum_lengthnum--1d9fa268461343c3" => Some(DdlRule::PrecisionAlt01),
        "optbinmod--6d2f0b51950c1625" => Some(DdlRule::OptBinModAlt01),
        "optbinmod_binary--5188e5fbf7394b5f" => Some(DdlRule::OptBinModAlt02),
        "optvectorelementtype--3b1d6f82e6dfb00b" => Some(DdlRule::OptVectorElementTypeAlt01),
        "optvectorelementtype_float--29c488953ed0aea2" => Some(DdlRule::OptVectorElementTypeAlt02),
        "optvectorelementtype_double--2b58d14add8f903f" => Some(DdlRule::OptVectorElementTypeAlt03),
        "optbinary--b2d4e7bcc043d19b" => Some(DdlRule::OptBinaryAlt01),
        "optbinary_binary_optcharset--3b321f5355549880" => Some(DdlRule::OptBinaryAlt02),
        "optbinary_charsetkw_charsetname_optbinmod--d1e856ede9c1e8ed" => {
            Some(DdlRule::OptBinaryAlt03)
        }
        "optcharset--330528076d2b2dbe" => Some(DdlRule::OptCharsetAlt01),
        "optcharset_charsetkw_charsetname--9d146f9e6a90529c" => Some(DdlRule::OptCharsetAlt02),
        "optcollate--d44dfa0e3e601e14" => Some(DdlRule::OptCollateAlt01),
        "optcollate_collate_collationname--cc951a9a572fadf9" => Some(DdlRule::OptCollateAlt02),
        "stringlist_stringlit--eb943cfcd50e2b2d" => Some(DdlRule::StringListAlt01),
        "stringlist_stringlist_stringlit--b78d964ab0c729e2" => Some(DdlRule::StringListAlt02),
        "textstring_stringlit--9e73755502753dac" => Some(DdlRule::TextStringAlt01),
        "textstring_hexlit--4b9850759f611034" => Some(DdlRule::TextStringAlt02),
        "textstring_bitlit--660e8dcaf0734fe2" => Some(DdlRule::TextStringAlt03),
        "textstringlist_textstring--6436aa35431e43c8" => Some(DdlRule::TextStringListAlt01),
        "textstringlist_textstringlist_textstring--35ad1d03a104966e" => {
            Some(DdlRule::TextStringListAlt02)
        }
        "alterrangestmt_alter_range_identifier_placementp--74da15928aa8ea7d" => {
            Some(DdlRule::AlterRangeStmtAlt01)
        }
        "droppolicystmt_drop_placement_policy_ifexists_po--e2931fef4b16eeef" => {
            Some(DdlRule::DropPolicyStmtAlt01)
        }
        "createpolicystmt_create_orreplace_placement_poli--2937f2dfcec38dd7" => {
            Some(DdlRule::CreatePolicyStmtAlt01)
        }
        "alterpolicystmt_alter_placement_policy_ifexists--5e88afd4982e13e5" => {
            Some(DdlRule::AlterPolicyStmtAlt01)
        }
        _ => None,
    }
}

pub(super) fn owns(rule_id: RuleId) -> bool {
    identify(rule_id).is_some()
}

pub(super) fn apply(
    rule_id: RuleId,
    rhs: Rhs<'_>,
    context: Context<'_>,
) -> Option<Result<bool, isize>> {
    let rule = identify(rule_id)?;
    Some(apply_rule(rule, rhs, context))
}

fn apply_rule(rule: DdlRule, mut rhs: Rhs<'_>, context: Context<'_>) -> Result<bool, isize> {
    let rhs_len = rhs.len();
    let Context {
        output: out,
        parser_state,
        lexer: yylex,
    } = context;
    match rule {
        DdlRule::SplitIndexListOptAlt02 => {
            out.item = rhs
                .borrow_mut(rhs_len - (0))
                .and_then(|value| value.item.take())
        }
        DdlRule::SplitIndexListAlt01 | DdlRule::SplitIndexListAlt02 => {
            let mut values = if rule == DdlRule::SplitIndexListAlt02 {
                rhs.borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::SplitIndexOption>>())
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SplitIndexOption>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::SplitIndexOptionAlt01
        | DdlRule::SplitIndexOptionAlt02
        | DdlRule::SplitIndexOptionAlt03 => {
            let split = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SplitOption>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::SplitIndexOption {
                PrimaryKey: rule == DdlRule::SplitIndexOptionAlt01,
                IndexName: if rule == DdlRule::SplitIndexOptionAlt01 {
                    parser_ast::NewCIStr("PRIMARY")
                } else if rule == DdlRule::SplitIndexOptionAlt02 {
                    parser_ast::NewCIStr(
                        &rhs.borrow(rhs_len - (1)).expect("DDL RHS position").ident,
                    )
                } else {
                    parser_ast::CIStr::default()
                },
                TableLevel: rule == DdlRule::SplitIndexOptionAlt03,
                SplitOpt: split,
            }));
        }
        DdlRule::PlacementOptionListAlt01
        | DdlRule::PlacementOptionListAlt02
        | DdlRule::PlacementOptionListAlt03 => {
            let list_back = if rule == DdlRule::PlacementOptionListAlt01 {
                None
            } else if rule == DdlRule::PlacementOptionListAlt02 {
                Some(1)
            } else {
                Some(2)
            };
            let mut values = list_back
                .and_then(|back| {
                    rhs.borrow(rhs_len - (back))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::PlacementOption>>())
                        .cloned()
                })
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::PlacementOption>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::DirectPlacementOptionAlt01
        | DdlRule::DirectPlacementOptionAlt02
        | DdlRule::DirectPlacementOptionAlt03
        | DdlRule::DirectPlacementOptionAlt04
        | DdlRule::DirectPlacementOptionAlt05
        | DdlRule::DirectPlacementOptionAlt06
        | DdlRule::DirectPlacementOptionAlt07
        | DdlRule::DirectPlacementOptionAlt08
        | DdlRule::DirectPlacementOptionAlt09
        | DdlRule::DirectPlacementOptionAlt10
        | DdlRule::DirectPlacementOptionAlt11
        | DdlRule::DirectPlacementOptionAlt12
        | DdlRule::PlacementPolicyOptionAlt01
        | DdlRule::PlacementPolicyOptionAlt02
        | DdlRule::PlacementPolicyOptionAlt03
        | DdlRule::PlacementPolicyOptionAlt04 => {
            let tp = match rule {
                DdlRule::DirectPlacementOptionAlt01 => {
                    parser_ast::PlacementOptionType::PrimaryRegion
                }
                DdlRule::DirectPlacementOptionAlt02 => parser_ast::PlacementOptionType::Regions,
                DdlRule::DirectPlacementOptionAlt03 => {
                    parser_ast::PlacementOptionType::FollowerCount
                }
                DdlRule::DirectPlacementOptionAlt04 => parser_ast::PlacementOptionType::VoterCount,
                DdlRule::DirectPlacementOptionAlt05 => {
                    parser_ast::PlacementOptionType::LearnerCount
                }
                DdlRule::DirectPlacementOptionAlt06 => parser_ast::PlacementOptionType::Schedule,
                DdlRule::DirectPlacementOptionAlt07 => parser_ast::PlacementOptionType::Constraints,
                DdlRule::DirectPlacementOptionAlt08 => {
                    parser_ast::PlacementOptionType::LeaderConstraints
                }
                DdlRule::DirectPlacementOptionAlt09 => {
                    parser_ast::PlacementOptionType::FollowerConstraints
                }
                DdlRule::DirectPlacementOptionAlt10 => {
                    parser_ast::PlacementOptionType::VoterConstraints
                }
                DdlRule::DirectPlacementOptionAlt11 => {
                    parser_ast::PlacementOptionType::LearnerConstraints
                }
                DdlRule::DirectPlacementOptionAlt12 => {
                    parser_ast::PlacementOptionType::SurvivalPreferences
                }
                _ => parser_ast::PlacementOptionType::Policy,
            };
            let numeric = matches!(
                rule,
                DdlRule::DirectPlacementOptionAlt03
                    | DdlRule::DirectPlacementOptionAlt04
                    | DdlRule::DirectPlacementOptionAlt05
            );
            let uint_value = if numeric {
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default()
                    .max(0) as u64
            } else {
                0
            };
            if rule == DdlRule::DirectPlacementOptionAlt03 && uint_value == 0 {
                yylex.AppendError(yylex.Errorf("FOLLOWERS must be positive", &[]));
                return Err(1);
            }
            out.item = Some(Box::new(parser_ast::PlacementOption {
                Tp: tp,
                StrValue: if numeric {
                    String::new()
                } else {
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone()
                },
                UintValue: uint_value,
            }));
        }
        DdlRule::AttributesOptAlt01 | DdlRule::AttributesOptAlt02 => {
            out.item = Some(Box::new(parser_ast::AttributesSpec {
                Default: rule == DdlRule::AttributesOptAlt01,
                Attributes: if rule == DdlRule::AttributesOptAlt02 {
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone()
                } else {
                    String::new()
                },
            }))
        }
        DdlRule::StatsOptionsOptAlt01 | DdlRule::StatsOptionsOptAlt02 => {
            out.item = Some(Box::new(parser_ast::StatsOptionsSpec {
                Default: rule == DdlRule::StatsOptionsOptAlt01,
                StatsOptions: if rule == DdlRule::StatsOptionsOptAlt02 {
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone()
                } else {
                    String::new()
                },
            }))
        }
        DdlRule::AlterTableSpecSingleOptAlt03 => {
            let Some(mut spec) = rhs
                .borrow_mut(rhs_len - (0))
                .and_then(|value| value.item.take())
                .and_then(|item| item.downcast::<parser_ast::AlterTableSpec>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            spec.NoWriteToBinlog = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            out.item = Some(Box::new(spec));
        }
        DdlRule::AlterTableSpecSingleOptAlt04 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::SplitIndex,
                SplitIndex: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::SplitIndexOption>())
                    .cloned(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecSingleOptAlt05 | DdlRule::AlterTableSpecSingleOptAlt06 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: if rule == DdlRule::AlterTableSpecSingleOptAlt05 {
                    parser_ast::AlterTableType::ReorganizeLastPartition
                } else {
                    parser_ast::AlterTableType::ReorganizeFirstPartition
                },
                PartitionExpr: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .expr
                    .clone(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecSingleOptAlt07 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::PartitionAttributes,
                PartitionNames: vec![parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (1)).expect("DDL RHS position").ident,
                )],
                AttributesSpec: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::AttributesSpec>())
                    .cloned(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecSingleOptAlt08 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::PartitionOptions,
                PartitionNames: vec![parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (1)).expect("DDL RHS position").ident,
                )],
                Options: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOption>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecSingleOptAlt09 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::RemoveTTL,
                ..Default::default()
            }))
        }
        DdlRule::LocationLabelListAlt01 => out.item = Some(Box::new(Vec::<String>::new())),
        DdlRule::LocationLabelListAlt02 => {
            out.item = rhs
                .borrow_mut(rhs_len - (0))
                .and_then(|value| value.item.take())
        }
        DdlRule::AlterTableSpecAlt01 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::Option,
                Options: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOption>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt02 | DdlRule::AlterTableSpecAlt03 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::SetTiFlashReplica,
                TiFlashReplica: Some(parser_ast::TiFlashReplicaSpec {
                    Count: rhs
                        .borrow(rhs_len - (1))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(semantic_numeric_isize)
                        .unwrap_or_default()
                        .max(0) as u64,
                    Labels: rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<String>>())
                        .cloned()
                        .unwrap_or_default(),
                    Hypo: rule == DdlRule::AlterTableSpecAlt03,
                }),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt04 | DdlRule::AlterTableSpecAlt05 => {
            let mut options = vec![parser_ast::TableOption {
                Tp: parser_ast::TableOptionType::Charset,
                StrValue: if rule == DdlRule::AlterTableSpecAlt04 {
                    rhs.borrow(rhs_len - (1))
                        .expect("DDL RHS position")
                        .ident
                        .clone()
                } else {
                    String::new()
                },
                Default: rule == DdlRule::AlterTableSpecAlt05,
                UintValue: 1,
                ..Default::default()
            }];
            if !rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .ident
                .is_empty()
            {
                options.push(parser_ast::TableOption {
                    Tp: parser_ast::TableOptionType::Collate,
                    StrValue: rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone(),
                    ..Default::default()
                });
            }
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::Option,
                Options: options,
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt08 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::AddConstraint,
                Constraint: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::Constraint>())
                    .cloned(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt09 => {
            let no_write = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            if no_write {
                yylex.AppendError(yylex.Errorf(
                    "The NO_WRITE_TO_BINLOG option is parsed but ignored for now.",
                    &[],
                ));
                yylex.LastErrorAsWarn();
            }
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                IfNotExists: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                NoWriteToBinlog: no_write,
                Tp: parser_ast::AlterTableType::AddPartitions,
                PartDefinitions: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::PartitionDefinition>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt13 | DdlRule::AlterTableSpecAlt21 => {
            let Some(tp) = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_types::types::FieldType>())
                .cloned()
            else {
                return Ok(false);
            };
            let column = parser_ast::ColumnDef {
                Name: parser_ast::ColumnName {
                    Name: parser_ast::NewCIStr("masking"),
                    ..Default::default()
                },
                Tp: tp,
                Options: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnOption>>())
                    .cloned()
                    .unwrap_or_default(),
            };
            if let Err(error) = validate_column_def(&column) {
                yylex.AppendError(error);
                return Err(1);
            }
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: if rule == DdlRule::AlterTableSpecAlt13 {
                    parser_ast::AlterTableType::AddColumns
                } else {
                    parser_ast::AlterTableType::ModifyColumn
                },
                NewColumns: vec![column],
                Position: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ColumnPosition>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt14 | DdlRule::AlterTableSpecAlt22 => {
            let mut tp = parser_types::types::NewFieldType(parser_mysql::r#type::TypeLonglong);
            tp.AddFlag(parser_mysql::r#type::UnsignedFlag);
            let mut options = vec![
                parser_ast::ColumnOption {
                    Tp: parser_ast::ColumnOptionType::NotNull,
                    ..Default::default()
                },
                parser_ast::ColumnOption {
                    Tp: parser_ast::ColumnOptionType::AutoIncrement,
                    ..Default::default()
                },
                parser_ast::ColumnOption {
                    Tp: parser_ast::ColumnOptionType::UniqueKey,
                    ..Default::default()
                },
            ];
            options.extend(
                rhs.borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnOption>>())
                    .cloned()
                    .unwrap_or_default(),
            );
            let column = parser_ast::ColumnDef {
                Name: parser_ast::ColumnName {
                    Name: parser_ast::NewCIStr("masking"),
                    ..Default::default()
                },
                Tp: tp,
                Options: options,
            };
            if let Err(error) = validate_column_def(&column) {
                yylex.AppendError(error);
                return Err(1);
            }
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: if rule == DdlRule::AlterTableSpecAlt14 {
                    parser_ast::AlterTableType::AddColumns
                } else {
                    parser_ast::AlterTableType::ModifyColumn
                },
                NewColumns: vec![column],
                Position: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ColumnPosition>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt15 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::AddMaskingPolicy,
                MaskingPolicyName: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (8)).expect("DDL RHS position").ident,
                ),
                MaskingPolicyColumn: Some(parser_ast::ColumnName {
                    Name: parser_ast::NewCIStr(
                        &rhs.borrow(rhs_len - (5)).expect("DDL RHS position").ident,
                    ),
                    ..Default::default()
                }),
                MaskingPolicyExpr: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .expr
                    .clone(),
                MaskingPolicyRestrictOps: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::MaskingPolicyRestrictOps>())
                    .copied()
                    .unwrap_or_default(),
                MaskingPolicyState: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::MaskingPolicyState>())
                    .copied()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt16
        | DdlRule::AlterTableSpecAlt17
        | DdlRule::AlterTableSpecAlt18 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: match rule {
                    DdlRule::AlterTableSpecAlt16 => parser_ast::AlterTableType::EnableMaskingPolicy,
                    DdlRule::AlterTableSpecAlt17 => {
                        parser_ast::AlterTableType::DisableMaskingPolicy
                    }
                    _ => parser_ast::AlterTableType::DropMaskingPolicy,
                },
                MaskingPolicyName: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (0)).expect("DDL RHS position").ident,
                ),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt19 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::DropColumn,
                OldColumnName: Some(parser_ast::ColumnName {
                    Name: parser_ast::NewCIStr("masking"),
                    ..Default::default()
                }),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt20 => {
            if !rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .ident
                .eq_ignore_ascii_case("expression")
            {
                yylex.AppendError(yylex.Errorf(
                    "unsupported masking policy modify option: %s",
                    &[&rhs.borrow(rhs_len - (2)).expect("DDL RHS position").ident],
                ));
                return Err(1);
            }
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::ModifyMaskingPolicyExpression,
                MaskingPolicyName: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (4)).expect("DDL RHS position").ident,
                ),
                MaskingPolicyExpr: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .expr
                    .clone(),
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt23 | DdlRule::AlterTableSpecAlt24 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::ModifyMaskingPolicyRestrictOn,
                MaskingPolicyName: parser_ast::NewCIStr(
                    &rhs.borrow(
                        rhs_len
                            - (if rule == DdlRule::AlterTableSpecAlt23 {
                                6
                            } else {
                                4
                            }),
                    )
                    .expect("DDL RHS position")
                    .ident,
                ),
                MaskingPolicyRestrictOps: if rule == DdlRule::AlterTableSpecAlt23 {
                    rhs.borrow(rhs_len - (1))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| {
                            item.downcast_ref::<parser_ast::MaskingPolicyRestrictOps>()
                        })
                        .copied()
                        .unwrap_or_default()
                } else {
                    parser_ast::MaskingPolicyRestrictOpNone
                },
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt25 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::Attributes,
                AttributesSpec: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::AttributesSpec>())
                    .cloned(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt26 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::StatsOptions,
                StatsOptionsSpec: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::StatsOptionsSpec>())
                    .cloned(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt27 => {
            yylex.AppendError(yylex.Errorf(
                "The CHECK PARTITIONING clause is parsed but not implement yet.",
                &[],
            ));
            yylex.LastErrorAsWarn();
            let names = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned();
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::CheckPartitions,
                OnAllPartitions: names.is_none(),
                PartitionNames: names.unwrap_or_default(),
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt28 => {
            let no_write = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            if no_write {
                yylex.AppendError(yylex.Errorf(
                    "The NO_WRITE_TO_BINLOG option is parsed but ignored for now.",
                    &[],
                ));
                yylex.LastErrorAsWarn();
            }
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::CoalescePartitions,
                NoWriteToBinlog: no_write,
                Num: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .map(getUint64FromNUM)
                    .unwrap_or_default(),
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt10 => {
            let no_write = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            if no_write {
                yylex.AppendError(yylex.Errorf(
                    "The NO_WRITE_TO_BINLOG option is parsed but ignored for now.",
                    &[],
                ));
                yylex.LastErrorAsWarn();
            }
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                IfNotExists: rhs
                    .borrow(rhs_len - (3))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                NoWriteToBinlog: no_write,
                Tp: parser_ast::AlterTableType::AddPartitions,
                Num: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default()
                    .max(0) as u64,
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt11 => {
            let no_write = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            if no_write {
                yylex.AppendError(yylex.Errorf(
                    "The NO_WRITE_TO_BINLOG option is parsed but ignored for now.",
                    &[],
                ));
                yylex.LastErrorAsWarn();
            }
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::AddLastPartition,
                NoWriteToBinlog: no_write,
                PartitionExpr: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .expr
                    .clone(),
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt12 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::AddStatistics,
                IfNotExists: rhs
                    .borrow(rhs_len - (5))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Statistics: Some(parser_ast::StatisticsSpec {
                    StatsName: rhs
                        .borrow(rhs_len - (4))
                        .expect("DDL RHS position")
                        .ident
                        .clone(),
                    StatsType: rhs
                        .borrow(rhs_len - (3))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(semantic_numeric_isize)
                        .unwrap_or_default() as u8,
                    Columns: rhs
                        .borrow(rhs_len - (1))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnName>>())
                        .cloned()
                        .unwrap_or_default(),
                }),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt30 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::DropPrimaryKey,
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt31 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::DropPartition,
                IfExists: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                PartitionNames: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt32 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::DropFirstPartition,
                IfExists: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                PartitionExpr: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .expr
                    .clone(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt33 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::DropStatistics,
                IfExists: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Statistics: Some(parser_ast::StatisticsSpec {
                    StatsName: rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone(),
                    ..Default::default()
                }),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt34 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::ExchangePartition,
                PartitionNames: vec![parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (4)).expect("DDL RHS position").ident,
                )],
                NewTable: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned(),
                WithValidation: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt35
        | DdlRule::AlterTableSpecAlt36
        | DdlRule::AlterTableSpecAlt37
        | DdlRule::AlterTableSpecAlt38
        | DdlRule::AlterTableSpecAlt39
        | DdlRule::AlterTableSpecAlt42 => {
            let (tp, names_back, no_write_back) = match rule {
                DdlRule::AlterTableSpecAlt35 => {
                    (parser_ast::AlterTableType::TruncatePartition, 0, None)
                }
                DdlRule::AlterTableSpecAlt36 => {
                    (parser_ast::AlterTableType::OptimizePartition, 0, Some(1))
                }
                DdlRule::AlterTableSpecAlt37 => {
                    (parser_ast::AlterTableType::RepairPartition, 0, Some(1))
                }
                DdlRule::AlterTableSpecAlt38 => (
                    parser_ast::AlterTableType::ImportPartitionTablespace,
                    1,
                    None,
                ),
                DdlRule::AlterTableSpecAlt39 => (
                    parser_ast::AlterTableType::DiscardPartitionTablespace,
                    1,
                    None,
                ),
                _ => (parser_ast::AlterTableType::RebuildPartition, 0, Some(1)),
            };
            let names = rhs
                .borrow(rhs_len - (names_back))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned();
            let spec = parser_ast::AlterTableSpec {
                Tp: tp,
                OnAllPartitions: names.is_none(),
                PartitionNames: names.unwrap_or_default(),
                NoWriteToBinlog: no_write_back
                    .and_then(|back| {
                        rhs.borrow(rhs_len - (back))
                            .expect("DDL RHS position")
                            .item
                            .as_deref()
                            .and_then(|item| item.downcast_ref::<bool>())
                            .copied()
                    })
                    .unwrap_or(false),
                ..Default::default()
            };
            if matches!(
                rule,
                DdlRule::AlterTableSpecAlt38 | DdlRule::AlterTableSpecAlt39
            ) {
                yylex.AppendError(yylex.Errorf(
                    "The partition tablespace clause is parsed but ignored by all storage engines.",
                    &[],
                ));
                yylex.LastErrorAsWarn();
            }
            out.item = Some(Box::new(spec));
        }
        DdlRule::AlterTableSpecAlt40 | DdlRule::AlterTableSpecAlt41 => {
            yylex.AppendError(yylex.Errorf(
                "The TABLESPACE clause is parsed but ignored by all storage engines.",
                &[],
            ));
            yylex.LastErrorAsWarn();
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: if rule == DdlRule::AlterTableSpecAlt40 {
                    parser_ast::AlterTableType::ImportTablespace
                } else {
                    parser_ast::AlterTableType::DiscardTablespace
                },
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt45 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::OrderByColumns,
                OrderByList: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::AlterOrderItem>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt50
        | DdlRule::AlterTableSpecAlt51
        | DdlRule::AlterTableSpecAlt52 => {
            let column_back = match rule {
                DdlRule::AlterTableSpecAlt50 => 3,
                DdlRule::AlterTableSpecAlt51 => 5,
                _ => 2,
            };
            let column = rhs
                .borrow(rhs_len - (column_back))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                .cloned()
                .unwrap_or_default();
            let mut options = Vec::new();
            if rule != DdlRule::AlterTableSpecAlt52 {
                options.push(parser_ast::ColumnOption {
                    Expr: rhs
                        .borrow(
                            rhs_len
                                - (if rule == DdlRule::AlterTableSpecAlt50 {
                                    0
                                } else {
                                    1
                                }),
                        )
                        .expect("DDL RHS position")
                        .expr
                        .clone(),
                    ..Default::default()
                });
            }
            let column = parser_ast::ColumnDef {
                Name: column,
                Tp: parser_types::types::NewFieldType(parser_mysql::r#type::TypeUnspecified),
                Options: options,
            };
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::AlterColumn,
                NewColumns: vec![column],
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt66 | DdlRule::AlterTableSpecAlt67 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: if rule == DdlRule::AlterTableSpecAlt66 {
                    parser_ast::AlterTableType::AlterCheck
                } else {
                    parser_ast::AlterTableType::DropCheck
                },
                Constraint: Some(parser_ast::Constraint {
                    Name: rhs
                        .borrow(
                            rhs_len
                                - (if rule == DdlRule::AlterTableSpecAlt66 {
                                    1
                                } else {
                                    0
                                }),
                        )
                        .expect("DDL RHS position")
                        .ident
                        .clone(),
                    Enforced: rule == DdlRule::AlterTableSpecAlt66
                        && rhs
                            .borrow(rhs_len - (0))
                            .expect("DDL RHS position")
                            .item
                            .as_deref()
                            .and_then(|item| item.downcast_ref::<bool>())
                            .copied()
                            .unwrap_or(false),
                    ..Default::default()
                }),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt68 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::IndexInvisible,
                IndexName: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (1)).expect("DDL RHS position").ident,
                ),
                Visibility: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::IndexVisibility>())
                    .copied()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt69 | DdlRule::AlterTableSpecAlt70 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: if rule == DdlRule::AlterTableSpecAlt69 {
                    parser_ast::AlterTableType::Cache
                } else {
                    parser_ast::AlterTableType::NoCache
                },
                ..Default::default()
            }))
        }
        DdlRule::ReorganizePartitionRuleOptAlt01 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::ReorganizePartition,
                OnAllPartitions: true,
                ..Default::default()
            }))
        }
        DdlRule::ReorganizePartitionRuleOptAlt02 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::ReorganizePartition,
                PartitionNames: rhs
                    .borrow(rhs_len - (4))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                    .cloned()
                    .unwrap_or_default(),
                PartDefinitions: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::PartitionDefinition>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        DdlRule::AllOrPartitionNameListAlt01 => out.item = None,
        DdlRule::WithValidationOptAlt01
        | DdlRule::WithValidationAlt01
        | DdlRule::WithValidationAlt02 => {
            out.item = Some(Box::new(rule != DdlRule::WithValidationAlt02))
        }
        DdlRule::WithClusteredAlt01 | DdlRule::WithClusteredAlt02 => {
            out.item = Some(Box::new(if rule == DdlRule::WithClusteredAlt01 {
                parser_ast::PrimaryKeyType::Clustered
            } else {
                parser_ast::PrimaryKeyType::NonClustered
            }))
        }
        DdlRule::GlobalOrLocalOptAlt03 => out.ident = "Global".to_owned(),
        DdlRule::AlterTableStmtAlt01 => {
            let mut specs = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::AlterTableSpec>>())
                .cloned()
                .unwrap_or_default();
            if let Some(spec) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::AlterTableSpec>())
            {
                specs.push(spec.clone());
            }
            let table = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::AlterTableStmt {
                node_text: Default::default(),
                Table: table,
                Specs: specs,
            }));
        }
        DdlRule::AlterTableStmtAlt02 | DdlRule::AlterTableStmtAlt03 => {
            let table_back = if rule == DdlRule::AlterTableStmtAlt02 {
                4
            } else {
                6
            };
            let Some(table) = rhs
                .borrow(rhs_len - (table_back))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            let partition_back = if rule == DdlRule::AlterTableStmtAlt02 {
                1
            } else {
                3
            };
            let partitions = rhs
                .borrow(rhs_len - (partition_back))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            let options = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::AnalyzeOpt>>())
                .cloned()
                .unwrap_or_default();
            let indexes = if rule == DdlRule::AlterTableStmtAlt03 {
                rhs.borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            out.statement = Some(Box::new(parser_ast::AnalyzeTableStmt {
                TableNames: vec![table],
                PartitionNames: partitions,
                IndexNames: indexes,
                AnalyzeOpts: options,
                IndexFlag: rule == DdlRule::AlterTableStmtAlt03,
                ..Default::default()
            }));
        }
        DdlRule::AlterTableStmtAlt04
        | DdlRule::AlterTableStmtAlt05
        | DdlRule::AlterTableStmtAlt06
        | DdlRule::AlterTableStmtAlt07 => {
            let (table_back, partition_back, replica) = match rule {
                DdlRule::AlterTableStmtAlt04 => (1, None, parser_ast::CompactReplicaKind::All),
                DdlRule::AlterTableStmtAlt05 => (3, None, parser_ast::CompactReplicaKind::TiFlash),
                DdlRule::AlterTableStmtAlt06 => (3, Some(0), parser_ast::CompactReplicaKind::All),
                _ => (5, Some(2), parser_ast::CompactReplicaKind::TiFlash),
            };
            let Some(table) = rhs
                .borrow(rhs_len - (table_back))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            let partitions = partition_back
                .and_then(|back| {
                    rhs.borrow(rhs_len - (back))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                })
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::CompactTableStmt {
                node_text: Default::default(),
                Table: table,
                PartitionNames: partitions,
                ReplicaKind: replica,
            }));
        }
        DdlRule::SplitIndexListOptAlt01 => out.item = None,
        DdlRule::AlterTableSpecSingleOptAlt01 => {
            let partition = rhs
                .borrow(rhs_len - 0)
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::PartitionOptions>())
                .cloned();
            out.item = partition.map(|partition| {
                Box::new(parser_ast::AlterTableSpec {
                    Tp: parser_ast::AlterTableType::Partition,
                    Partition: Some(partition),
                    ..Default::default()
                }) as Box<dyn std::any::Any>
            });
        }
        DdlRule::AlterTableSpecSingleOptAlt02 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::RemovePartitioning,
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt06 => {
            let column = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnDef>())
                .cloned()
                .into_iter()
                .collect();
            let position = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnPosition>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                IfNotExists: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Tp: parser_ast::AlterTableType::AddColumns,
                NewColumns: column,
                Position: position,
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt07 => {
            let Some(elements) = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<CreateTableElementsSemantic>())
                .cloned()
            else {
                return Ok(false);
            };
            if !elements.complete {
                return Ok(false);
            }
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                IfNotExists: rhs
                    .borrow(rhs_len - (3))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Tp: parser_ast::AlterTableType::AddColumns,
                NewColumns: elements.columns,
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt29 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                IfExists: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Tp: parser_ast::AlterTableType::DropColumn,
                OldColumnName: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                    .cloned(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt43 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                IfExists: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Tp: parser_ast::AlterTableType::DropIndex,
                Name: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt44 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::DropForeignKey,
                Name: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt46 | DdlRule::AlterTableSpecAlt47 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: if rule == DdlRule::AlterTableSpecAlt46 {
                    parser_ast::AlterTableType::DisableKeys
                } else {
                    parser_ast::AlterTableType::EnableKeys
                },
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt48 | DdlRule::AlterTableSpecAlt49 => {
            let column_back = 1;
            let columns = rhs
                .borrow(rhs_len - (column_back))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnDef>())
                .cloned()
                .into_iter()
                .collect();
            let position = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnPosition>())
                .cloned()
                .unwrap_or_default();
            let old_column = if rule == DdlRule::AlterTableSpecAlt49 {
                rhs.borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                    .cloned()
            } else {
                None
            };
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                IfExists: rhs
                    .borrow(
                        rhs_len
                            - (if rule == DdlRule::AlterTableSpecAlt48 {
                                2
                            } else {
                                3
                            }),
                    )
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Tp: if rule == DdlRule::AlterTableSpecAlt48 {
                    parser_ast::AlterTableType::ModifyColumn
                } else {
                    parser_ast::AlterTableType::ChangeColumn
                },
                NewColumns: columns,
                OldColumnName: old_column,
                Position: position,
                ..Default::default()
            }));
        }
        DdlRule::AlterTableSpecAlt53 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::RenameColumn,
                OldColumnName: Some(parser_ast::ColumnName {
                    Name: parser_ast::NewCIStr(
                        &rhs.borrow(rhs_len - (2)).expect("DDL RHS position").ident,
                    ),
                    ..Default::default()
                }),
                NewColumnName: Some(parser_ast::ColumnName {
                    Name: parser_ast::NewCIStr(
                        &rhs.borrow(rhs_len - (0)).expect("DDL RHS position").ident,
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt54
        | DdlRule::AlterTableSpecAlt55
        | DdlRule::AlterTableSpecAlt56 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::RenameTable,
                NewTable: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt57 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::RenameIndex,
                FromKey: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (2)).expect("DDL RHS position").ident,
                ),
                ToKey: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (0)).expect("DDL RHS position").ident,
                ),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt58 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::Lock,
                LockType: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::LockType>())
                    .copied()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt59 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::Writeable,
                Writeable: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt60 => {
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: parser_ast::AlterTableType::Algorithm,
                Algorithm: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::AlgorithmType>())
                    .copied()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        DdlRule::AlterTableSpecAlt61
        | DdlRule::AlterTableSpecAlt62
        | DdlRule::AlterTableSpecAlt63
        | DdlRule::AlterTableSpecAlt64
        | DdlRule::AlterTableSpecAlt65 => {
            let tp = match rule {
                DdlRule::AlterTableSpecAlt61 => parser_ast::AlterTableType::Force,
                DdlRule::AlterTableSpecAlt62 => parser_ast::AlterTableType::WithValidation,
                DdlRule::AlterTableSpecAlt63 => parser_ast::AlterTableType::WithoutValidation,
                DdlRule::AlterTableSpecAlt64 => parser_ast::AlterTableType::SecondaryLoad,
                _ => parser_ast::AlterTableType::SecondaryUnload,
            };
            out.item = Some(Box::new(parser_ast::AlterTableSpec {
                Tp: tp,
                ..Default::default()
            }));
        }
        DdlRule::AlgorithmClauseAlt01
        | DdlRule::AlgorithmClauseAlt02
        | DdlRule::AlgorithmClauseAlt03
        | DdlRule::AlgorithmClauseAlt04 => {
            out.item = Some(Box::new(match rule {
                DdlRule::AlgorithmClauseAlt01 => parser_ast::AlgorithmType::Default,
                DdlRule::AlgorithmClauseAlt02 => parser_ast::AlgorithmType::Copy,
                DdlRule::AlgorithmClauseAlt03 => parser_ast::AlgorithmType::Inplace,
                _ => parser_ast::AlgorithmType::Instant,
            }))
        }
        DdlRule::AlgorithmClauseAlt05 => {
            yylex.AppendError(yylex.Errorf(
                "Unknown ALGORITHM '%s'",
                &[&rhs.borrow(rhs_len - (2)).expect("DDL RHS position").ident],
            ));
            return Err(1);
        }
        DdlRule::WriteableAlt01 | DdlRule::WriteableAlt02 => {
            out.item = Some(Box::new(rule == DdlRule::WriteableAlt01))
        }
        DdlRule::GlobalOrLocalOptAlt01 | DdlRule::GlobalOrLocalOptAlt02 => out.ident.clear(),
        DdlRule::LockClauseAlt01 => out.item = Some(Box::new(parser_ast::LockType::Default)),
        DdlRule::LockClauseAlt02 => {
            let lock_type = match rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .ident
                .to_ascii_uppercase()
                .as_str()
            {
                "NONE" => parser_ast::LockType::None,
                "SHARED" => parser_ast::LockType::Shared,
                "EXCLUSIVE" => parser_ast::LockType::Exclusive,
                _ => {
                    yylex.AppendError(yylex.Errorf("Unknown LOCK type", &[]));
                    return Err(1);
                }
            };
            out.item = Some(Box::new(lock_type));
        }
        DdlRule::ColumnPositionAlt01 => {
            out.item = Some(Box::new(parser_ast::ColumnPosition::default()))
        }
        DdlRule::ColumnPositionAlt02 => {
            out.item = Some(Box::new(parser_ast::ColumnPosition {
                Tp: parser_ast::ColumnPositionType::First,
                ..Default::default()
            }))
        }
        DdlRule::ColumnPositionAlt03 => {
            out.item = Some(Box::new(parser_ast::ColumnPosition {
                Tp: parser_ast::ColumnPositionType::After,
                RelativeColumn: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                    .cloned(),
            }))
        }
        DdlRule::AlterTableSpecListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::AlterTableSpec>::new()))
        }
        DdlRule::AlterTableSpecListAlt01 => {
            let values = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::AlterTableSpec>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        DdlRule::AlterTableSpecListAlt02 => {
            let mut values = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::AlterTableSpec>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::AlterTableSpec>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::PartitionNameListAlt01 => {
            out.item = Some(Box::new(vec![parser_ast::NewCIStr(
                &rhs.borrow(rhs_len - (0)).expect("DDL RHS position").ident,
            )]))
        }
        DdlRule::PartitionNameListAlt02 => {
            let mut names = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            names.push(parser_ast::NewCIStr(
                &rhs.borrow(rhs_len - (0)).expect("DDL RHS position").ident,
            ));
            out.item = Some(Box::new(names));
        }
        DdlRule::ConstraintKeywordOptAlt01 | DdlRule::ConstraintKeywordOptAlt02 => out.item = None,
        DdlRule::ConstraintKeywordOptAlt03 => {
            out.item = Some(Box::new(
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
            ))
        }
        DdlRule::RenameTableStmtAlt01 => {
            let mappings = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableToTable>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::RenameTableStmt {
                node_text: Default::default(),
                TableToTables: mappings,
            }));
        }
        DdlRule::TableToTableListAlt01 => {
            let values = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableToTable>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        DdlRule::TableToTableListAlt02 => {
            let mut values = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableToTable>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableToTable>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::TableToTableAlt01 => {
            let Some(old_table) = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            let Some(new_table) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::TableToTable {
                OldTable: old_table,
                NewTable: new_table,
            }));
        }
        DdlRule::RecoverTableStmtAlt01 => {
            let job_id = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<i64>())
                .copied()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::RecoverTableStmt {
                JobID: job_id,
                ..Default::default()
            }));
        }
        DdlRule::RecoverTableStmtAlt02 | DdlRule::RecoverTableStmtAlt03 => {
            let table_back = if rule == DdlRule::RecoverTableStmtAlt02 {
                0
            } else {
                1
            };
            let Some(table) = rhs
                .borrow(rhs_len - (table_back))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            let job_num = if rule == DdlRule::RecoverTableStmtAlt03 {
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<i64>())
                    .copied()
                    .unwrap_or_default()
            } else {
                0
            };
            out.statement = Some(Box::new(parser_ast::RecoverTableStmt {
                Table: Some(table),
                JobNum: job_num,
                ..Default::default()
            }));
        }
        DdlRule::FlashbackToTimestampStmtAlt01
        | DdlRule::FlashbackToTimestampStmtAlt02
        | DdlRule::FlashbackToTimestampStmtAlt03
        | DdlRule::FlashbackToTimestampStmtAlt04
        | DdlRule::FlashbackToTimestampStmtAlt05
        | DdlRule::FlashbackToTimestampStmtAlt06 => {
            let mut statement = parser_ast::FlashBackToTimestampStmt::default();
            if matches!(
                rule,
                DdlRule::FlashbackToTimestampStmtAlt02 | DdlRule::FlashbackToTimestampStmtAlt05
            ) {
                statement.Tables = rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                    .cloned()
                    .unwrap_or_default();
            } else if matches!(
                rule,
                DdlRule::FlashbackToTimestampStmtAlt03 | DdlRule::FlashbackToTimestampStmtAlt06
            ) {
                statement.DBName = parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (2)).expect("DDL RHS position").ident,
                );
            }
            if rule <= DdlRule::FlashbackToTimestampStmtAlt03 {
                statement.FlashbackTS = Some(parser_ast::ExprNode::Value(
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone(),
                ));
            } else {
                let tso = rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .map(getUint64FromNUM)
                    .unwrap_or_default();
                if tso == 0 {
                    yylex.AppendError(yylex.Errorf("Invalid TSO value provided", &[]));
                    return Err(1);
                }
                statement.FlashbackTSO = tso;
            }
            out.statement = Some(Box::new(statement));
        }
        DdlRule::FlashbackTableStmtAlt01 => {
            let Some(table) = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::FlashBackTableStmt {
                node_text: Default::default(),
                Table: table,
                NewName: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
            }));
        }
        DdlRule::FlashbackToNewNameAlt01 => out.ident.clear(),
        DdlRule::FlashbackToNewNameAlt02 => {
            out.ident = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .ident
                .clone()
        }
        DdlRule::FlashbackDatabaseStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::FlashBackDatabaseStmt {
                node_text: Default::default(),
                DBName: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (1)).expect("DDL RHS position").ident,
                ),
                NewName: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
            }))
        }
        DdlRule::DistributeTableStmtAlt01 | DdlRule::DistributeTableStmtAlt02 => {
            let table_back = if rule == DdlRule::DistributeTableStmtAlt01 {
                7
            } else {
                10
            };
            let Some(table) = rhs
                .borrow(rhs_len - (table_back))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::DistributeTableStmt {
                node_text: Default::default(),
                Table: table,
                PartitionNames: rhs
                    .borrow(
                        rhs_len
                            - (if rule == DdlRule::DistributeTableStmtAlt01 {
                                6
                            } else {
                                9
                            }),
                    )
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                    .cloned()
                    .unwrap_or_default(),
                Rule: rhs
                    .borrow(
                        rhs_len
                            - (if rule == DdlRule::DistributeTableStmtAlt01 {
                                3
                            } else {
                                6
                            }),
                    )
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
                Engine: rhs
                    .borrow(
                        rhs_len
                            - (if rule == DdlRule::DistributeTableStmtAlt01 {
                                0
                            } else {
                                3
                            }),
                    )
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
                Timeout: if rule == DdlRule::DistributeTableStmtAlt02 {
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone()
                } else {
                    String::new()
                },
            }));
        }
        DdlRule::CancelDistributionJobStmtAlt01 => {
            let job_id = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<i64>())
                .copied()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::CancelDistributionJobStmt {
                node_text: Default::default(),
                JobID: job_id,
            }));
        }
        DdlRule::SplitRegionStmtAlt01 | DdlRule::SplitRegionStmtAlt02 => {
            let syntax_back = if rule == DdlRule::SplitRegionStmtAlt01 {
                4
            } else {
                6
            };
            let table_back = if rule == DdlRule::SplitRegionStmtAlt01 {
                2
            } else {
                4
            };
            let partition_back = if rule == DdlRule::SplitRegionStmtAlt01 {
                1
            } else {
                3
            };
            let Some(table) = rhs
                .borrow(rhs_len - (table_back))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::SplitRegionStmt {
                node_text: Default::default(),
                SplitSyntaxOpt: rhs
                    .borrow(rhs_len - (syntax_back))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::SplitSyntaxOption>())
                    .cloned()
                    .unwrap_or_default(),
                Table: table,
                PartitionNames: rhs
                    .borrow(rhs_len - (partition_back))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                    .cloned()
                    .unwrap_or_default(),
                IndexName: if rule == DdlRule::SplitRegionStmtAlt02 {
                    parser_ast::NewCIStr(
                        &rhs.borrow(rhs_len - (1)).expect("DDL RHS position").ident,
                    )
                } else {
                    parser_ast::CIStr::default()
                },
                SplitOpt: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::SplitOption>())
                    .cloned()
                    .unwrap_or_default(),
            }));
        }
        DdlRule::SplitOptionBetweenAlt01 => {
            out.item = Some(Box::new(parser_ast::SplitOption {
                Lower: rhs
                    .borrow(rhs_len - (4))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                    .cloned()
                    .unwrap_or_default(),
                Upper: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                    .cloned()
                    .unwrap_or_default(),
                Num: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default() as i64,
                ..Default::default()
            }))
        }
        DdlRule::SplitOptionAlt01 => {
            out.item = rhs
                .borrow_mut(rhs_len - (0))
                .and_then(|value| value.item.take())
        }
        DdlRule::SplitOptionAlt02 => {
            out.item = Some(Box::new(parser_ast::SplitOption {
                ValueLists: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<Vec<parser_ast::ExprNode>>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        DdlRule::SplitSyntaxOptionAlt01
        | DdlRule::SplitSyntaxOptionAlt02
        | DdlRule::SplitSyntaxOptionAlt03
        | DdlRule::SplitSyntaxOptionAlt04 => {
            out.item = Some(Box::new(parser_ast::SplitSyntaxOption {
                HasRegionFor: matches!(
                    rule,
                    DdlRule::SplitSyntaxOptionAlt02 | DdlRule::SplitSyntaxOptionAlt04
                ),
                HasPartition: matches!(
                    rule,
                    DdlRule::SplitSyntaxOptionAlt03 | DdlRule::SplitSyntaxOptionAlt04
                ),
            }))
        }
        DdlRule::ColumnDefAlt01 => {
            let Some(name) = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                .cloned()
            else {
                return Ok(false);
            };
            let Some(tp) = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_types::types::FieldType>())
                .cloned()
            else {
                return Ok(false);
            };
            let options = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnOption>>())
                .cloned()
                .unwrap_or_default();
            let column = parser_ast::ColumnDef {
                Name: name,
                Tp: tp,
                Options: options,
            };
            if let Err(error) = validate_column_def(&column) {
                yylex.AppendError(error);
                return Err(1);
            }
            out.item = Some(Box::new(column));
        }
        DdlRule::ColumnDefAlt02 => {
            let Some(name) = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                .cloned()
            else {
                return Ok(false);
            };
            let mut tp = parser_types::types::NewFieldType(parser_mysql::r#type::TypeLonglong);
            tp.AddFlag(parser_mysql::r#type::UnsignedFlag);
            let mut options = vec![
                parser_ast::ColumnOption {
                    Tp: parser_ast::ColumnOptionType::NotNull,
                    ..Default::default()
                },
                parser_ast::ColumnOption {
                    Tp: parser_ast::ColumnOptionType::AutoIncrement,
                    ..Default::default()
                },
                parser_ast::ColumnOption {
                    Tp: parser_ast::ColumnOptionType::UniqueKey,
                    ..Default::default()
                },
            ];
            options.extend(
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnOption>>())
                    .cloned()
                    .unwrap_or_default(),
            );
            let column = parser_ast::ColumnDef {
                Name: name,
                Tp: tp,
                Options: options,
            };
            if let Err(error) = validate_column_def(&column) {
                yylex.AppendError(error);
                return Err(1);
            }
            out.item = Some(Box::new(column));
        }
        DdlRule::EnforcedOrNotAlt01
        | DdlRule::EnforcedOrNotAlt02
        | DdlRule::EnforcedOrNotOptAlt01 => {
            out.item = Some(Box::new(rule != DdlRule::EnforcedOrNotAlt02))
        }
        DdlRule::EnforcedOrNotOrNotNullOptAlt01 => out.item = Some(Box::new(0i32)),
        DdlRule::EnforcedOrNotOrNotNullOptAlt02 => {
            out.item = Some(Box::new(
                if rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false)
                {
                    1i32
                } else {
                    2i32
                },
            ))
        }
        DdlRule::ColumnOptionAlt01
        | DdlRule::ColumnOptionAlt02
        | DdlRule::ColumnOptionAlt03
        | DdlRule::ColumnOptionAlt04
        | DdlRule::ColumnOptionAlt05
        | DdlRule::ColumnOptionAlt06
        | DdlRule::ColumnOptionAlt07
        | DdlRule::ColumnOptionAlt08
        | DdlRule::ColumnOptionAlt09
        | DdlRule::ColumnOptionAlt10
        | DdlRule::ColumnOptionAlt12
        | DdlRule::ColumnOptionAlt13 => {
            let kind = match rule {
                DdlRule::ColumnOptionAlt01 => parser_ast::ColumnOptionType::NotNull,
                DdlRule::ColumnOptionAlt02 => parser_ast::ColumnOptionType::Null,
                DdlRule::ColumnOptionAlt03 => parser_ast::ColumnOptionType::AutoIncrement,
                DdlRule::ColumnOptionAlt04 | DdlRule::ColumnOptionAlt05 => {
                    parser_ast::ColumnOptionType::PrimaryKey
                }
                DdlRule::ColumnOptionAlt06
                | DdlRule::ColumnOptionAlt07
                | DdlRule::ColumnOptionAlt08
                | DdlRule::ColumnOptionAlt09 => parser_ast::ColumnOptionType::UniqueKey,
                DdlRule::ColumnOptionAlt10 => parser_ast::ColumnOptionType::DefaultValue,
                DdlRule::ColumnOptionAlt12 => parser_ast::ColumnOptionType::OnUpdate,
                _ => parser_ast::ColumnOptionType::Comment,
            };
            let expr = if matches!(
                rule,
                DdlRule::ColumnOptionAlt10 | DdlRule::ColumnOptionAlt12
            ) {
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .expr
                    .clone()
            } else if rule == DdlRule::ColumnOptionAlt13 {
                Some(parser_ast::ExprNode::Value(
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone(),
                ))
            } else {
                None
            };
            out.item = Some(Box::new(parser_ast::ColumnOption {
                Tp: kind,
                Expr: expr,
                StrValue: match rule {
                    DdlRule::ColumnOptionAlt04 | DdlRule::ColumnOptionAlt09 => rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone(),
                    DdlRule::ColumnOptionAlt06 => "Global".to_owned(),
                    _ => String::new(),
                },
                PrimaryKeyTp: if rule == DdlRule::ColumnOptionAlt05 {
                    rhs.borrow(rhs_len - (1))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::PrimaryKeyType>())
                        .copied()
                        .unwrap_or_default()
                } else {
                    Default::default()
                },
                ..Default::default()
            }));
        }
        DdlRule::ColumnOptionAlt11 => {
            out.item = Some(Box::new(vec![
                parser_ast::ColumnOption {
                    Tp: parser_ast::ColumnOptionType::NotNull,
                    ..Default::default()
                },
                parser_ast::ColumnOption {
                    Tp: parser_ast::ColumnOptionType::AutoIncrement,
                    ..Default::default()
                },
                parser_ast::ColumnOption {
                    Tp: parser_ast::ColumnOptionType::UniqueKey,
                    ..Default::default()
                },
            ]))
        }
        DdlRule::ColumnOptionAlt14 => {
            let mut option = parser_ast::ColumnOption {
                Tp: parser_ast::ColumnOptionType::Check,
                Expr: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .expr
                    .clone(),
                Enforced: true,
                ConstraintName: rhs
                    .borrow(rhs_len - (5))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<String>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            };
            match rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(semantic_numeric_isize)
                .unwrap_or_default()
            {
                0 => {
                    out.item = Some(Box::new(vec![
                        option,
                        parser_ast::ColumnOption {
                            Tp: parser_ast::ColumnOptionType::NotNull,
                            ..Default::default()
                        },
                    ]))
                }
                1 => out.item = Some(Box::new(option)),
                2 => {
                    option.Enforced = false;
                    out.item = Some(Box::new(option));
                }
                _ => out.item = Some(Box::new(option)),
            }
        }
        DdlRule::ColumnOptionAlt15 => {
            out.item = Some(Box::new(parser_ast::ColumnOption {
                Tp: parser_ast::ColumnOptionType::Generated,
                Expr: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .expr
                    .clone(),
                Stored: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ..Default::default()
            }))
        }
        DdlRule::ColumnOptionAlt16 => {
            out.item = Some(Box::new(parser_ast::ColumnOption {
                Tp: parser_ast::ColumnOptionType::Reference,
                Refer: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ReferenceDef>())
                    .cloned(),
                ..Default::default()
            }))
        }
        DdlRule::ColumnOptionAlt17
        | DdlRule::ColumnOptionAlt18
        | DdlRule::ColumnOptionAlt19
        | DdlRule::ColumnOptionAlt20
        | DdlRule::ColumnOptionAlt21 => {
            let tp = match rule {
                DdlRule::ColumnOptionAlt17 => parser_ast::ColumnOptionType::Collate,
                DdlRule::ColumnOptionAlt18 => parser_ast::ColumnOptionType::ColumnFormat,
                DdlRule::ColumnOptionAlt19 => parser_ast::ColumnOptionType::Storage,
                DdlRule::ColumnOptionAlt20 => parser_ast::ColumnOptionType::AutoRandom,
                _ => parser_ast::ColumnOptionType::SecondaryEngineAttribute,
            };
            if rule == DdlRule::ColumnOptionAlt19 {
                yylex.AppendError(yylex.Errorf(
                    "The STORAGE clause is parsed but ignored by all storage engines.",
                    &[],
                ));
                yylex.LastErrorAsWarn();
            }
            out.item = Some(Box::new(parser_ast::ColumnOption {
                Tp: tp,
                StrValue: if rule == DdlRule::ColumnOptionAlt20 {
                    String::new()
                } else {
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone()
                },
                AutoRandOpt: if rule == DdlRule::ColumnOptionAlt20 {
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::AutoRandomOption>())
                        .copied()
                        .unwrap_or_default()
                } else {
                    Default::default()
                },
                ..Default::default()
            }));
        }
        DdlRule::ColumnOptionAlt22 | DdlRule::ColumnOptionAlt23 => {
            if !parser_state.enableMariaDB {
                yylex.AppendError(yylex.Errorf("syntax error", &[]));
                return Err(1);
            }
            out.item = Some(Box::new(parser_ast::ColumnOption {
                Tp: if rule == DdlRule::ColumnOptionAlt22 {
                    parser_ast::ColumnOptionType::MariaDBRowStart
                } else {
                    parser_ast::ColumnOptionType::MariaDBRowEnd
                },
                ..Default::default()
            }));
        }
        DdlRule::AutoRandomOptAlt01 | DdlRule::AutoRandomOptAlt02 | DdlRule::AutoRandomOptAlt03 => {
            out.item = Some(Box::new(parser_ast::AutoRandomOption {
                ShardBits: if rule == DdlRule::AutoRandomOptAlt01 {
                    parser_types::types::UnspecifiedLength
                } else {
                    rhs.borrow(
                        rhs_len
                            - (if rule == DdlRule::AutoRandomOptAlt02 {
                                1
                            } else {
                                3
                            }),
                    )
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default()
                },
                RangeBits: if rule == DdlRule::AutoRandomOptAlt03 {
                    rhs.borrow(rhs_len - (1))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(semantic_numeric_isize)
                        .unwrap_or_default()
                } else {
                    parser_types::types::UnspecifiedLength
                },
            }))
        }
        DdlRule::ColumnOptionListAlt01 => {
            let item = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref();
            let values = item
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnOption>>())
                .cloned()
                .or_else(|| {
                    item.and_then(|item| item.downcast_ref::<parser_ast::ColumnOption>())
                        .cloned()
                        .map(|option| vec![option])
                })
                .unwrap_or_default();
            out.item = Some(Box::new(values));
        }
        DdlRule::ColumnOptionListAlt02 => {
            let mut values = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnOption>>())
                .cloned()
                .unwrap_or_default();
            let item = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref();
            let incoming = item
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnOption>>())
                .cloned()
                .or_else(|| {
                    item.and_then(|item| item.downcast_ref::<parser_ast::ColumnOption>())
                        .cloned()
                        .map(|option| vec![option])
                })
                .unwrap_or_default();
            let has_collate = values
                .iter()
                .any(|option| option.Tp == parser_ast::ColumnOptionType::Collate);
            let incoming_collate = incoming
                .iter()
                .any(|option| option.Tp == parser_ast::ColumnOptionType::Collate);
            if has_collate && incoming_collate {
                yylex.AppendError(yylex.Errorf("Multiple COLLATE clauses", &[]));
                return Err(1);
            }
            values.extend(incoming);
            out.item = Some(Box::new(values));
        }
        DdlRule::ColumnOptionListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::ColumnOption>::new()))
        }
        DdlRule::ColumnFormatAlt01 => out.ident = "DEFAULT".to_owned(),
        DdlRule::ColumnFormatAlt02 => out.ident = "FIXED".to_owned(),
        DdlRule::ColumnFormatAlt03 => out.ident = "DYNAMIC".to_owned(),
        DdlRule::VirtualOrStoredAlt01 | DdlRule::VirtualOrStoredAlt02 => {
            out.item = Some(Box::new(false))
        }
        DdlRule::VirtualOrStoredAlt03 => out.item = Some(Box::new(true)),
        DdlRule::ConstraintElemAlt01
        | DdlRule::ConstraintElemAlt02
        | DdlRule::ConstraintElemAlt03
        | DdlRule::ConstraintElemAlt04
        | DdlRule::ConstraintElemAlt05
        | DdlRule::ConstraintElemAlt06 => {
            let mut constraint = parser_ast::Constraint::default();
            constraint.Tp = match rule {
                DdlRule::ConstraintElemAlt01 => parser_ast::ConstraintType::PrimaryKey,
                DdlRule::ConstraintElemAlt02 => parser_ast::ConstraintType::Fulltext,
                DdlRule::ConstraintElemAlt03 => parser_ast::ConstraintType::Index,
                DdlRule::ConstraintElemAlt04 => parser_ast::ConstraintType::Unique,
                DdlRule::ConstraintElemAlt05 => parser_ast::ConstraintType::ForeignKey,
                _ => parser_ast::ConstraintType::Check,
            };
            if rule == DdlRule::ConstraintElemAlt06 {
                constraint.Expr = rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .expr
                    .clone();
                constraint.Enforced = rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false);
            } else {
                constraint.Keys = rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::IndexPartSpecification>>())
                    .cloned()
                    .unwrap_or_default();
                constraint.IfNotExists = matches!(
                    rule,
                    DdlRule::ConstraintElemAlt03 | DdlRule::ConstraintElemAlt05
                ) && rhs
                    .borrow(rhs_len - (5))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false);
                if matches!(
                    rule,
                    DdlRule::ConstraintElemAlt01
                        | DdlRule::ConstraintElemAlt03
                        | DdlRule::ConstraintElemAlt04
                ) {
                    if let Some(name_and_type) = rhs
                        .borrow(rhs_len - (4))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<IndexNameAndTypeSemantic>())
                    {
                        constraint.Name = name_and_type.name.String.clone();
                        constraint.IsEmptyIndex = name_and_type.name.Empty;
                        constraint.Option = rhs
                            .borrow(rhs_len - (0))
                            .expect("DDL RHS position")
                            .item
                            .as_deref()
                            .and_then(|item| item.downcast_ref::<parser_ast::IndexOption>())
                            .cloned();
                        if let Some(index_type) = name_and_type.index_type {
                            constraint.Option.get_or_insert_with(Default::default).Tp = index_type;
                        }
                    }
                } else if let Some(name) = rhs
                    .borrow(rhs_len - (4))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::NullString>())
                {
                    constraint.Name = name.String.clone();
                    constraint.IsEmptyIndex = name.Empty;
                    if rule == DdlRule::ConstraintElemAlt02 {
                        constraint.Option = Some(
                            rhs.borrow(rhs_len - (0))
                                .expect("DDL RHS position")
                                .item
                                .as_deref()
                                .and_then(|item| item.downcast_ref::<parser_ast::IndexOption>())
                                .cloned()
                                .unwrap_or_default(),
                        );
                    }
                }
                if rule == DdlRule::ConstraintElemAlt05 {
                    constraint.Refer = rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::ReferenceDef>())
                        .cloned();
                }
            }
            out.item = Some(Box::new(constraint));
        }
        DdlRule::MatchAlt01
        | DdlRule::MatchAlt02
        | DdlRule::MatchAlt03
        | DdlRule::MatchOptAlt01 => {
            out.item = Some(Box::new(match rule {
                DdlRule::MatchAlt01 => parser_ast::MatchType::Full,
                DdlRule::MatchAlt02 => parser_ast::MatchType::Partial,
                DdlRule::MatchAlt03 => parser_ast::MatchType::Simple,
                _ => parser_ast::MatchType::None,
            }))
        }
        DdlRule::MatchOptAlt02 => {
            out.item = rhs
                .borrow_mut(rhs_len - (0))
                .and_then(|value| value.item.take());
            yylex.AppendError(yylex.Errorf(
                "The MATCH clause is parsed but ignored by all storage engines.",
                &[],
            ));
            yylex.LastErrorAsWarn();
        }
        DdlRule::ReferDefAlt01 => {
            let pair = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| {
                    item.downcast_ref::<(parser_ast::OnDeleteOpt, parser_ast::OnUpdateOpt)>()
                })
                .copied()
                .unwrap_or_default();
            let Some(table) = rhs
                .borrow(rhs_len - (3))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::ReferenceDef {
                Table: table,
                IndexPartSpecifications: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::IndexPartSpecification>>())
                    .cloned()
                    .unwrap_or_default(),
                Match: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::MatchType>())
                    .copied()
                    .unwrap_or_default(),
                OnDelete: pair.0,
                OnUpdate: pair.1,
            }));
        }
        DdlRule::OnDeleteAlt01 => {
            out.item = Some(Box::new(parser_ast::OnDeleteOpt {
                ReferOpt: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ReferOptionType>())
                    .copied()
                    .unwrap_or_default(),
            }))
        }
        DdlRule::OnUpdateAlt01 => {
            out.item = Some(Box::new(parser_ast::OnUpdateOpt {
                ReferOpt: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ReferOptionType>())
                    .copied()
                    .unwrap_or_default(),
            }))
        }
        DdlRule::OnDeleteUpdateOptAlt01
        | DdlRule::OnDeleteUpdateOptAlt02
        | DdlRule::OnDeleteUpdateOptAlt03
        | DdlRule::OnDeleteUpdateOptAlt04
        | DdlRule::OnDeleteUpdateOptAlt05 => {
            let delete = if matches!(
                rule,
                DdlRule::OnDeleteUpdateOptAlt02 | DdlRule::OnDeleteUpdateOptAlt04
            ) {
                rhs.borrow(
                    rhs_len
                        - (if rule == DdlRule::OnDeleteUpdateOptAlt02 {
                            0
                        } else {
                            1
                        }),
                )
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::OnDeleteOpt>())
                .copied()
                .unwrap_or_default()
            } else if rule == DdlRule::OnDeleteUpdateOptAlt05 {
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::OnDeleteOpt>())
                    .copied()
                    .unwrap_or_default()
            } else {
                Default::default()
            };
            let update = if matches!(
                rule,
                DdlRule::OnDeleteUpdateOptAlt03 | DdlRule::OnDeleteUpdateOptAlt04
            ) {
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::OnUpdateOpt>())
                    .copied()
                    .unwrap_or_default()
            } else if rule == DdlRule::OnDeleteUpdateOptAlt05 {
                rhs.borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::OnUpdateOpt>())
                    .copied()
                    .unwrap_or_default()
            } else {
                Default::default()
            };
            out.item = Some(Box::new((delete, update)));
        }
        DdlRule::ReferOptAlt01
        | DdlRule::ReferOptAlt02
        | DdlRule::ReferOptAlt03
        | DdlRule::ReferOptAlt04
        | DdlRule::ReferOptAlt05 => {
            let value = match rule {
                DdlRule::ReferOptAlt01 => parser_ast::ReferOptionType::Restrict,
                DdlRule::ReferOptAlt02 => parser_ast::ReferOptionType::Cascade,
                DdlRule::ReferOptAlt03 => parser_ast::ReferOptionType::SetNull,
                DdlRule::ReferOptAlt04 => parser_ast::ReferOptionType::NoAction,
                _ => parser_ast::ReferOptionType::SetDefault,
            };
            if rule == DdlRule::ReferOptAlt05 {
                yylex.AppendError(yylex.Errorf(
                    "The SET DEFAULT clause is parsed but ignored by all storage engines.",
                    &[],
                ));
                yylex.LastErrorAsWarn();
            }
            out.item = Some(Box::new(value));
        }
        DdlRule::DefaultValueExprAlt05 => {
            out.expr = Some(parser_ast::ExprNode::Column(parser_ast::ColumnName {
                Name: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (1)).expect("DDL RHS position").ident,
                ),
                ..Default::default()
            }));
        }
        DdlRule::DefaultValueExprAlt06 => {
            out.expr = rhs
                .borrow(
                    rhs_len
                        - (if rule == DdlRule::DefaultValueExprAlt06 {
                            1
                        } else {
                            0
                        }),
                )
                .expect("DDL RHS position")
                .expr
                .clone()
        }
        DdlRule::CreateIndexStmtAlt01 => {
            let lock_algorithm = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::IndexLockAndAlgorithm>())
                .cloned();
            out.statement = Some(Box::new(parser_ast::CreateIndexStmt {
                node_text: Default::default(),
                IfNotExists: rhs
                    .borrow(rhs_len - (9))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                IndexName: rhs
                    .borrow(rhs_len - (8))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
                Table: rhs
                    .borrow(rhs_len - (5))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned()
                    .unwrap_or_default(),
                IndexPartSpecifications: rhs
                    .borrow(rhs_len - (3))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::IndexPartSpecification>>())
                    .cloned()
                    .unwrap_or_default(),
                KeyType: rhs
                    .borrow(rhs_len - (11))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::IndexKeyType>())
                    .copied()
                    .unwrap_or_default(),
                Option: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::IndexOption>())
                    .cloned(),
                LockAlg: lock_algorithm,
            }));
        }
        DdlRule::IndexPartSpecificationListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::IndexPartSpecification>::new()))
        }
        DdlRule::IndexPartSpecificationListOptAlt02 => {
            out.item = rhs
                .borrow_mut(rhs_len - (1))
                .and_then(|value| value.item.take())
        }
        DdlRule::IndexPartSpecificationListAlt01 => {
            let values = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::IndexPartSpecification>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        DdlRule::IndexPartSpecificationListAlt02 => {
            let mut values = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::IndexPartSpecification>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::IndexPartSpecification>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::IndexPartSpecificationAlt01 => {
            out.item = Some(Box::new(parser_ast::IndexPartSpecification {
                Column: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                    .cloned(),
                Length: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<isize>())
                    .copied()
                    .unwrap_or(-1),
                Desc: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ..Default::default()
            }))
        }
        DdlRule::IndexPartSpecificationAlt02 => {
            out.item = Some(Box::new(parser_ast::IndexPartSpecification {
                Expr: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .expr
                    .clone(),
                Desc: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ..Default::default()
            }))
        }
        DdlRule::IndexLockAndAlgorithmOptAlt01 => out.item = None,
        DdlRule::IndexLockAndAlgorithmOptAlt02
        | DdlRule::IndexLockAndAlgorithmOptAlt03
        | DdlRule::IndexLockAndAlgorithmOptAlt04
        | DdlRule::IndexLockAndAlgorithmOptAlt05 => {
            let (lock_back, algorithm_back) = match rule {
                DdlRule::IndexLockAndAlgorithmOptAlt02 => (Some(0), None),
                DdlRule::IndexLockAndAlgorithmOptAlt03 => (None, Some(0)),
                DdlRule::IndexLockAndAlgorithmOptAlt04 => (Some(1), Some(0)),
                _ => (Some(0), Some(1)),
            };
            out.item = Some(Box::new(parser_ast::IndexLockAndAlgorithm {
                LockTp: lock_back
                    .and_then(|back| {
                        rhs.borrow(rhs_len - (back))
                            .expect("DDL RHS position")
                            .item
                            .as_deref()
                    })
                    .and_then(|item| item.downcast_ref::<parser_ast::LockType>())
                    .copied()
                    .unwrap_or_default(),
                AlgorithmTp: algorithm_back
                    .and_then(|back| {
                        rhs.borrow(rhs_len - (back))
                            .expect("DDL RHS position")
                            .item
                            .as_deref()
                    })
                    .and_then(|item| item.downcast_ref::<parser_ast::AlgorithmType>())
                    .copied()
                    .unwrap_or_default(),
            }));
        }
        DdlRule::IndexKeyTypeOptAlt01
        | DdlRule::IndexKeyTypeOptAlt02
        | DdlRule::IndexKeyTypeOptAlt03
        | DdlRule::IndexKeyTypeOptAlt04
        | DdlRule::IndexKeyTypeOptAlt05
        | DdlRule::IndexKeyTypeOptAlt06 => {
            out.item = Some(Box::new(match rule {
                DdlRule::IndexKeyTypeOptAlt01 => parser_ast::IndexKeyType::None,
                DdlRule::IndexKeyTypeOptAlt02 => parser_ast::IndexKeyType::Unique,
                DdlRule::IndexKeyTypeOptAlt03 => parser_ast::IndexKeyType::Spatial,
                DdlRule::IndexKeyTypeOptAlt04 => parser_ast::IndexKeyType::Fulltext,
                DdlRule::IndexKeyTypeOptAlt05 => parser_ast::IndexKeyType::Vector,
                _ => parser_ast::IndexKeyType::Columnar,
            }))
        }
        DdlRule::AlterDatabaseStmtAlt01 | DdlRule::AlterDatabaseStmtAlt02 => {
            out.statement = Some(Box::new(parser_ast::AlterDatabaseStmt {
                node_text: Default::default(),
                Name: if rule == DdlRule::AlterDatabaseStmtAlt01 {
                    parser_ast::NewCIStr(
                        &rhs.borrow(rhs_len - (1)).expect("DDL RHS position").ident,
                    )
                } else {
                    parser_ast::CIStr::default()
                },
                AlterDefaultDatabase: rule == DdlRule::AlterDatabaseStmtAlt02,
                Options: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::DatabaseOption>>())
                    .cloned()
                    .unwrap_or_default(),
            }));
        }
        DdlRule::CreateDatabaseStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::CreateDatabaseStmt {
                node_text: Default::default(),
                IfNotExists: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Name: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
                Options: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::DatabaseOption>>())
                    .cloned()
                    .unwrap_or_default(),
            }));
        }
        DdlRule::OnCommitOptAlt01 | DdlRule::PartitionOptAlt01 => out.item = None,
        DdlRule::DuplicateOptAlt01 => out.item = Some(Box::new(0u8)),
        DdlRule::CreateTableSelectOptAlt01 => {
            out.item = Some(Box::new(parser_ast::CreateTableStmt::default()))
        }
        DdlRule::LikeTableWithOrWithoutParenAlt01 => {
            out.item = rhs
                .borrow_mut(rhs_len - (0))
                .and_then(|value| value.item.take())
        }
        DdlRule::LikeTableWithOrWithoutParenAlt02 => {
            out.item = rhs
                .borrow_mut(rhs_len - (1))
                .and_then(|value| value.item.take())
        }
        DdlRule::CreateViewStmtAlt01 => {
            let Some(select) = rhs
                .borrow_mut(rhs_len - (1))
                .and_then(|value| value.statement.take())
            else {
                return Ok(false);
            };
            let Some(view_name) = rhs
                .borrow(rhs_len - (4))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            let definer = rhs
                .borrow(rhs_len - (7))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<auth::UserIdentity>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::CreateViewStmt {
                node_text: Default::default(),
                OrReplace: rhs
                    .borrow(rhs_len - (9))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ViewName: view_name,
                Cols: rhs
                    .borrow(rhs_len - (3))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                    .cloned()
                    .unwrap_or_default(),
                Select: select,
                SchemaCols: Vec::new(),
                Algorithm: rhs
                    .borrow(rhs_len - (8))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ViewAlgorithm>())
                    .copied()
                    .unwrap_or_default(),
                Definer: definer,
                Security: rhs
                    .borrow(rhs_len - (6))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ViewSecurity>())
                    .copied()
                    .unwrap_or_default(),
                CheckOption: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ViewCheckOption>())
                    .copied()
                    .unwrap_or_default(),
            }));
        }
        DdlRule::OrReplaceAlt01 => out.item = Some(Box::new(false)),
        DdlRule::OrReplaceAlt02 => out.item = Some(Box::new(true)),
        DdlRule::ViewAlgorithmAlt01 | DdlRule::ViewAlgorithmAlt02 => {
            out.item = Some(Box::new(parser_ast::ViewAlgorithm::Undefined))
        }
        DdlRule::ViewAlgorithmAlt03 => out.item = Some(Box::new(parser_ast::ViewAlgorithm::Merge)),
        DdlRule::ViewAlgorithmAlt04 => {
            out.item = Some(Box::new(parser_ast::ViewAlgorithm::Temptable))
        }
        DdlRule::ViewDefinerAlt01 => {
            out.item = Some(Box::new(auth::UserIdentity {
                current_user: true,
                ..Default::default()
            }))
        }
        DdlRule::ViewDefinerAlt02 => {
            out.item = rhs
                .borrow_mut(rhs_len - (0))
                .and_then(|value| value.item.take())
        }
        DdlRule::ViewSQLSecurityAlt01 | DdlRule::ViewSQLSecurityAlt02 => {
            out.item = Some(Box::new(parser_ast::ViewSecurity::Definer))
        }
        DdlRule::ViewSQLSecurityAlt03 => {
            out.item = Some(Box::new(parser_ast::ViewSecurity::Invoker))
        }
        DdlRule::ViewFieldListAlt01 | DdlRule::ViewCheckOptionAlt01 => out.item = None,
        DdlRule::ViewFieldListAlt02 => {
            out.item = rhs
                .borrow_mut(rhs_len - (1))
                .and_then(|value| value.item.take())
        }
        DdlRule::ColumnListAlt01 => {
            out.item = Some(Box::new(vec![parser_ast::NewCIStr(
                &rhs.borrow(rhs_len - (0)).expect("DDL RHS position").ident,
            )]))
        }
        DdlRule::ColumnListAlt02 => {
            let mut columns = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            columns.push(parser_ast::NewCIStr(
                &rhs.borrow(rhs_len - (0)).expect("DDL RHS position").ident,
            ));
            out.item = Some(Box::new(columns));
        }
        DdlRule::ViewCheckOptionAlt02 => {
            out.item = Some(Box::new(parser_ast::ViewCheckOption::Cascaded))
        }
        DdlRule::ViewCheckOptionAlt03 => {
            out.item = Some(Box::new(parser_ast::ViewCheckOption::Local))
        }
        DdlRule::CreateViewSelectOptAlt04 => {
            let Some(statement) =
                take_subquery_statement(rhs.borrow_mut(rhs_len - (0)).expect("DDL RHS position"))
            else {
                return Ok(false);
            };
            let statement = match statement.into_any().downcast::<parser_ast::SelectStmt>() {
                Ok(mut select) => {
                    select.IsInBraces = true;
                    select as Box<dyn parser_ast::Node>
                }
                Err(statement) => match statement.downcast::<parser_ast::SetOprStmt>() {
                    Ok(mut set_op) => {
                        set_op.IsInBraces = true;
                        set_op as Box<dyn parser_ast::Node>
                    }
                    Err(_) => return Ok(false),
                },
            };
            out.statement = Some(statement);
        }
        DdlRule::IndexOptionListAlt01 | DdlRule::IndexTypeOptAlt01 => out.item = None,
        DdlRule::DatabaseOptionAlt01
        | DdlRule::DatabaseOptionAlt02
        | DdlRule::DatabaseOptionAlt03 => {
            out.item = Some(Box::new(parser_ast::DatabaseOption {
                Tp: match rule {
                    DdlRule::DatabaseOptionAlt01 => parser_ast::DatabaseOptionType::Charset,
                    DdlRule::DatabaseOptionAlt02 => parser_ast::DatabaseOptionType::Collate,
                    _ => parser_ast::DatabaseOptionType::Encryption,
                },
                Value: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
                ..Default::default()
            }))
        }
        DdlRule::DatabaseOptionAlt04 | DdlRule::DatabaseOptionAlt05 => {
            let placement = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::PlacementOption>())
                .cloned()
                .unwrap_or_default();
            let tp = match placement.Tp {
                parser_ast::PlacementOptionType::PrimaryRegion => {
                    parser_ast::DatabaseOptionType::PrimaryRegion
                }
                parser_ast::PlacementOptionType::Regions => parser_ast::DatabaseOptionType::Regions,
                parser_ast::PlacementOptionType::FollowerCount => {
                    parser_ast::DatabaseOptionType::FollowerCount
                }
                parser_ast::PlacementOptionType::VoterCount => {
                    parser_ast::DatabaseOptionType::VoterCount
                }
                parser_ast::PlacementOptionType::LearnerCount => {
                    parser_ast::DatabaseOptionType::LearnerCount
                }
                parser_ast::PlacementOptionType::Schedule => {
                    parser_ast::DatabaseOptionType::Schedule
                }
                parser_ast::PlacementOptionType::Constraints => {
                    parser_ast::DatabaseOptionType::Constraints
                }
                parser_ast::PlacementOptionType::LeaderConstraints => {
                    parser_ast::DatabaseOptionType::LeaderConstraints
                }
                parser_ast::PlacementOptionType::FollowerConstraints => {
                    parser_ast::DatabaseOptionType::FollowerConstraints
                }
                parser_ast::PlacementOptionType::VoterConstraints => {
                    parser_ast::DatabaseOptionType::VoterConstraints
                }
                parser_ast::PlacementOptionType::LearnerConstraints => {
                    parser_ast::DatabaseOptionType::LearnerConstraints
                }
                parser_ast::PlacementOptionType::SurvivalPreferences => {
                    parser_ast::DatabaseOptionType::SurvivalPreferences
                }
                parser_ast::PlacementOptionType::Policy => parser_ast::DatabaseOptionType::Policy,
            };
            out.item = Some(Box::new(parser_ast::DatabaseOption {
                Tp: tp,
                Value: placement.StrValue,
                UintValue: placement.UintValue,
                ..Default::default()
            }));
        }
        DdlRule::DatabaseOptionAlt06 => {
            out.item = Some(Box::new(parser_ast::DatabaseOption {
                Tp: parser_ast::DatabaseOptionType::TiFlashReplica,
                TiFlashReplica: Some(parser_ast::TiFlashReplicaSpec {
                    Count: rhs
                        .borrow(rhs_len - (1))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(semantic_numeric_isize)
                        .unwrap_or_default()
                        .max(0) as u64,
                    Labels: rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<String>>())
                        .cloned()
                        .unwrap_or_default(),
                    ..Default::default()
                }),
                ..Default::default()
            }))
        }
        DdlRule::DatabaseOptionListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::DatabaseOption>::new()))
        }
        DdlRule::DatabaseOptionListAlt01 => {
            let values = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::DatabaseOption>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        DdlRule::DatabaseOptionListAlt02 => {
            let mut values = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::DatabaseOption>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::DatabaseOption>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::CreateTableStmtAlt01 => {
            let Some(semantic) = rhs
                .borrow_mut(rhs_len - (7))
                .and_then(|value| value.item.take())
                .and_then(|item| item.downcast::<CreateTableSemantic>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            if !semantic.complete {
                return Ok(false);
            }
            let mut statement = semantic.statement;
            statement.Table = rhs
                .borrow(rhs_len - (8))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
                .unwrap_or_default();
            statement.IfNotExists = rhs
                .borrow(rhs_len - (9))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            statement.TemporaryKeyword = rhs
                .borrow(rhs_len - (11))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TemporaryKeyword>())
                .copied()
                .unwrap_or_default();
            statement.Options = rhs
                .borrow(rhs_len - (6))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOption>>())
                .cloned()
                .unwrap_or_default();
            statement.Partition = rhs
                .borrow(rhs_len - (5))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::PartitionOptions>())
                .cloned();
            statement.SplitIndex = rhs
                .borrow(rhs_len - (4))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::SplitIndexOption>>())
                .cloned()
                .unwrap_or_default();
            statement.OnDuplicate = rhs
                .borrow(rhs_len - (3))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::OnDuplicateKeyHandlingType>())
                .copied()
                .unwrap_or_default();
            if let Some(mut select_holder) = rhs
                .borrow_mut(rhs_len - (1))
                .and_then(|value| value.item.take())
                .and_then(|item| item.downcast::<CreateTableSemantic>().ok())
            {
                statement.Select = select_holder.statement.Select.take();
            }
            let on_commit = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied();
            if (on_commit.is_some()
                && statement.TemporaryKeyword != parser_ast::TemporaryKeyword::Global)
                || (statement.TemporaryKeyword == parser_ast::TemporaryKeyword::Global
                    && on_commit.is_none())
            {
                yylex.AppendError(yylex.Errorf(
                    "GLOBAL TEMPORARY and ON COMMIT DELETE ROWS must appear together",
                    &[],
                ));
            } else if statement.TemporaryKeyword == parser_ast::TemporaryKeyword::Global {
                statement.OnCommitDelete = on_commit.unwrap_or(false);
            }
            out.statement = Some(Box::new(statement));
        }
        DdlRule::CreateTableStmtAlt02 => {
            let temporary = rhs
                .borrow(rhs_len - (5))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TemporaryKeyword>())
                .copied()
                .unwrap_or_default();
            let on_commit = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied();
            if (on_commit.is_some() && temporary != parser_ast::TemporaryKeyword::Global)
                || (temporary == parser_ast::TemporaryKeyword::Global && on_commit.is_none())
            {
                yylex.AppendError(yylex.Errorf(
                    "GLOBAL TEMPORARY and ON COMMIT DELETE ROWS must appear together",
                    &[],
                ));
            }
            out.statement = Some(Box::new(parser_ast::CreateTableStmt {
                Table: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned()
                    .unwrap_or_default(),
                ReferTable: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned(),
                IfNotExists: rhs
                    .borrow(rhs_len - (3))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                TemporaryKeyword: temporary,
                OnCommitDelete: temporary == parser_ast::TemporaryKeyword::Global
                    && on_commit.unwrap_or(false),
                ..Default::default()
            }));
        }
        DdlRule::OnCommitOptAlt02 | DdlRule::OnCommitOptAlt03 => {
            out.item = Some(Box::new(rule == DdlRule::OnCommitOptAlt02))
        }
        DdlRule::PartitionOptAlt02 => {
            let Some(mut method) = rhs
                .borrow(rhs_len - (4))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::PartitionMethod>())
                .cloned()
            else {
                return Ok(false);
            };
            method.Num = rhs
                .borrow(rhs_len - (3))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<u64>())
                .copied()
                .unwrap_or_default();
            let mut options = parser_ast::PartitionOptions {
                PartitionMethod: method,
                Sub: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::PartitionMethod>())
                    .cloned(),
                Definitions: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::PartitionDefinition>>())
                    .cloned()
                    .unwrap_or_default(),
                UpdateIndexes: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::Constraint>>())
                    .cloned()
                    .unwrap_or_default(),
            };
            if let Err(error) = validate_partition_options(&mut options) {
                yylex.AppendError(error);
                return Err(1);
            }
            out.item = Some(Box::new(options));
        }
        DdlRule::GlobalOrLocalAlt01 | DdlRule::GlobalOrLocalAlt02 => {
            out.item = Some(Box::new(rule == DdlRule::GlobalOrLocalAlt02))
        }
        DdlRule::UpdateIndexElemAlt01 => {
            out.item = Some(Box::new(parser_ast::Constraint {
                Name: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
                Option: Some(parser_ast::IndexOption {
                    Global: rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false),
                    ..Default::default()
                }),
                ..Default::default()
            }))
        }
        DdlRule::UpdateIndexesListAlt01 => {
            out.item = Some(Box::new(
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::Constraint>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        DdlRule::UpdateIndexesListAlt02 => {
            let mut values = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::Constraint>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Constraint>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::UpdateIndexesOptAlt01
        | DdlRule::PartitionKeyAlgorithmOptAlt01
        | DdlRule::PartitionIntervalOptAlt01
        | DdlRule::SubPartitionOptAlt01
        | DdlRule::PartitionDefinitionListOptAlt01 => out.item = None,
        DdlRule::UpdateIndexesOptAlt02
        | DdlRule::PartitionDefinitionListOptAlt02
        | DdlRule::SubPartDefinitionListOptAlt02 => {
            out.item = rhs
                .borrow_mut(rhs_len - (1))
                .and_then(|value| value.item.take())
        }
        DdlRule::SubPartitionMethodAlt01 => {
            out.item = Some(Box::new(parser_ast::PartitionMethod {
                Tp: parser_ast::PartitionType::Key,
                Linear: !rhs
                    .borrow(rhs_len - (5))
                    .expect("DDL RHS position")
                    .ident
                    .is_empty(),
                ColumnNames: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnName>>())
                    .cloned()
                    .unwrap_or_default(),
                KeyAlgorithm: rhs
                    .borrow(rhs_len - (3))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::PartitionKeyAlgorithm>())
                    .copied()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        DdlRule::SubPartitionMethodAlt02 => {
            out.item = Some(Box::new(parser_ast::PartitionMethod {
                Tp: parser_ast::PartitionType::Hash,
                Linear: !rhs
                    .borrow(rhs_len - (4))
                    .expect("DDL RHS position")
                    .ident
                    .is_empty(),
                Expr: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .expr
                    .clone(),
                ..Default::default()
            }))
        }
        DdlRule::PartitionKeyAlgorithmOptAlt02 => {
            let value = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .map(getUint64FromNUM)
                .unwrap_or_default();
            if !matches!(value, 1 | 2) {
                yylex.AppendError(yylex.Errorf("syntax error", &[]));
                return Err(1);
            }
            out.item = Some(Box::new(parser_ast::PartitionKeyAlgorithm { Type: value }));
        }
        DdlRule::PartitionMethodAlt02 | DdlRule::PartitionMethodAlt03 => {
            out.item = Some(Box::new(parser_ast::PartitionMethod {
                Tp: parser_ast::PartitionType::Range,
                Expr: if rule == DdlRule::PartitionMethodAlt02 {
                    rhs.borrow(rhs_len - (2))
                        .expect("DDL RHS position")
                        .expr
                        .clone()
                } else {
                    None
                },
                ColumnNames: if rule == DdlRule::PartitionMethodAlt03 {
                    rhs.borrow(rhs_len - (2))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnName>>())
                        .cloned()
                        .unwrap_or_default()
                } else {
                    Vec::new()
                },
                Interval: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::PartitionInterval>())
                    .cloned(),
                ..Default::default()
            }))
        }
        DdlRule::PartitionMethodAlt04 | DdlRule::PartitionMethodAlt05 => {
            out.item = Some(Box::new(parser_ast::PartitionMethod {
                Tp: parser_ast::PartitionType::List,
                Expr: if rule == DdlRule::PartitionMethodAlt04 {
                    rhs.borrow(rhs_len - (1))
                        .expect("DDL RHS position")
                        .expr
                        .clone()
                } else {
                    None
                },
                ColumnNames: if rule == DdlRule::PartitionMethodAlt05 {
                    rhs.borrow(rhs_len - (1))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnName>>())
                        .cloned()
                        .unwrap_or_default()
                } else {
                    Vec::new()
                },
                ..Default::default()
            }))
        }
        DdlRule::PartitionMethodAlt06
        | DdlRule::PartitionMethodAlt07
        | DdlRule::PartitionMethodAlt08 => {
            out.item = Some(Box::new(parser_ast::PartitionMethod {
                Tp: parser_ast::PartitionType::SystemTime,
                Expr: if rule == DdlRule::PartitionMethodAlt06 {
                    rhs.borrow(rhs_len - (1))
                        .expect("DDL RHS position")
                        .expr
                        .clone()
                } else {
                    None
                },
                Unit: if rule == DdlRule::PartitionMethodAlt06 {
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::TimeUnitType>())
                        .copied()
                        .unwrap_or_default()
                } else {
                    Default::default()
                },
                Limit: if rule == DdlRule::PartitionMethodAlt07 {
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<u64>())
                        .copied()
                        .unwrap_or_default()
                } else {
                    0
                },
                ..Default::default()
            }))
        }
        DdlRule::PartitionIntervalOptAlt02 => {
            let range = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::PartitionInterval>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::PartitionInterval {
                IntervalExpr: rhs
                    .borrow(rhs_len - (4))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::PartitionIntervalExpr>())
                    .cloned()
                    .unwrap_or_default(),
                FirstRangeEnd: range.FirstRangeEnd,
                LastRangeEnd: range.LastRangeEnd,
                NullPart: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                MaxValPart: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
            }));
        }
        DdlRule::IntervalExprAlt01 | DdlRule::IntervalExprAlt02 => {
            out.item = Some(Box::new(parser_ast::PartitionIntervalExpr {
                Expr: rhs
                    .borrow(
                        rhs_len
                            - (if rule == DdlRule::IntervalExprAlt01 {
                                0
                            } else {
                                1
                            }),
                    )
                    .expect("DDL RHS position")
                    .expr
                    .clone(),
                TimeUnit: if rule == DdlRule::IntervalExprAlt02 {
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::TimeUnitType>())
                        .copied()
                        .unwrap_or_default()
                } else {
                    Default::default()
                },
            }))
        }
        DdlRule::NullPartOptAlt01
        | DdlRule::NullPartOptAlt02
        | DdlRule::MaxValPartOptAlt01
        | DdlRule::MaxValPartOptAlt02 => {
            out.item = Some(Box::new(matches!(
                rule,
                DdlRule::NullPartOptAlt02 | DdlRule::MaxValPartOptAlt02
            )))
        }
        DdlRule::FirstAndLastPartOptAlt01 => {
            out.item = Some(Box::new(parser_ast::PartitionInterval::default()))
        }
        DdlRule::FirstAndLastPartOptAlt02 => {
            out.item = Some(Box::new(parser_ast::PartitionInterval {
                FirstRangeEnd: rhs
                    .borrow(rhs_len - (8))
                    .expect("DDL RHS position")
                    .expr
                    .clone(),
                LastRangeEnd: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .expr
                    .clone(),
                ..Default::default()
            }))
        }
        DdlRule::LinearOptAlt01 => out.ident.clear(),
        DdlRule::SubPartitionOptAlt02 => {
            let Some(mut method) = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::PartitionMethod>())
                .cloned()
            else {
                return Ok(false);
            };
            method.Num = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<u64>())
                .copied()
                .unwrap_or_default();
            out.item = Some(Box::new(method));
        }
        DdlRule::SubPartitionNumOptAlt01 | DdlRule::PartitionNumOptAlt01 => {
            out.item = Some(Box::new(0u64))
        }
        DdlRule::SubPartitionNumOptAlt02 | DdlRule::PartitionNumOptAlt02 => {
            let value = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<u64>())
                .copied()
                .unwrap_or_default();
            if value == 0 {
                yylex.AppendError(yylex.Errorf(
                    if rule == DdlRule::SubPartitionNumOptAlt02 {
                        "subpartitions must be positive"
                    } else {
                        "partitions must be positive"
                    },
                    &[],
                ));
                return Err(1);
            }
            out.item = Some(Box::new(value));
        }
        DdlRule::PartitionDefinitionListAlt01 => {
            out.item = Some(Box::new(
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::PartitionDefinition>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        DdlRule::PartitionDefinitionListAlt02 => {
            let mut values = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::PartitionDefinition>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::PartitionDefinition>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::PartitionDefinitionAlt01 => {
            out.item = Some(Box::new(parser_ast::PartitionDefinition {
                Name: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (3)).expect("DDL RHS position").ident,
                ),
                Clause: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::PartitionDefinitionClause>())
                    .cloned()
                    .unwrap_or_default(),
                Options: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOption>>())
                    .cloned()
                    .unwrap_or_default(),
                Sub: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::SubPartitionDefinition>>())
                    .cloned()
                    .unwrap_or_default(),
            }))
        }
        DdlRule::SubPartDefinitionListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::SubPartitionDefinition>::new()))
        }
        DdlRule::SubPartDefinitionListAlt01 => {
            out.item = Some(Box::new(
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::SubPartitionDefinition>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        DdlRule::SubPartDefinitionListAlt02 => {
            let mut values = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::SubPartitionDefinition>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SubPartitionDefinition>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::SubPartDefinitionAlt01 => {
            out.item = Some(Box::new(parser_ast::SubPartitionDefinition {
                Name: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (1)).expect("DDL RHS position").ident,
                ),
                Options: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOption>>())
                    .cloned()
                    .unwrap_or_default(),
            }))
        }
        DdlRule::PartDefOptionListAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::TableOption>::new()))
        }
        DdlRule::PartDefOptionListAlt02 => {
            let mut values = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOption>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableOption>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::PartDefOptionAlt01
        | DdlRule::PartDefOptionAlt02
        | DdlRule::PartDefOptionAlt03
        | DdlRule::PartDefOptionAlt04
        | DdlRule::PartDefOptionAlt05
        | DdlRule::PartDefOptionAlt06
        | DdlRule::PartDefOptionAlt07
        | DdlRule::PartDefOptionAlt08
        | DdlRule::PartDefOptionAlt09
        | DdlRule::PartDefOptionAlt10
        | DdlRule::PartDefOptionAlt11
        | DdlRule::PartDefOptionAlt12
        | DdlRule::PartDefOptionAlt13 => {
            if rule == DdlRule::PartDefOptionAlt13 {
                let placement = rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::PlacementOption>())
                    .cloned()
                    .unwrap_or_default();
                let tp = match placement.Tp {
                    parser_ast::PlacementOptionType::PrimaryRegion => {
                        parser_ast::TableOptionType::PrimaryRegion
                    }
                    parser_ast::PlacementOptionType::Regions => {
                        parser_ast::TableOptionType::Regions
                    }
                    parser_ast::PlacementOptionType::FollowerCount => {
                        parser_ast::TableOptionType::FollowerCount
                    }
                    parser_ast::PlacementOptionType::VoterCount => {
                        parser_ast::TableOptionType::VoterCount
                    }
                    parser_ast::PlacementOptionType::LearnerCount => {
                        parser_ast::TableOptionType::LearnerCount
                    }
                    parser_ast::PlacementOptionType::Schedule => {
                        parser_ast::TableOptionType::Schedule
                    }
                    parser_ast::PlacementOptionType::Constraints => {
                        parser_ast::TableOptionType::Constraints
                    }
                    parser_ast::PlacementOptionType::LeaderConstraints => {
                        parser_ast::TableOptionType::LeaderConstraints
                    }
                    parser_ast::PlacementOptionType::LearnerConstraints => {
                        parser_ast::TableOptionType::LearnerConstraints
                    }
                    parser_ast::PlacementOptionType::FollowerConstraints => {
                        parser_ast::TableOptionType::FollowerConstraints
                    }
                    parser_ast::PlacementOptionType::VoterConstraints => {
                        parser_ast::TableOptionType::VoterConstraints
                    }
                    parser_ast::PlacementOptionType::SurvivalPreferences => {
                        parser_ast::TableOptionType::SurvivalPreferences
                    }
                    parser_ast::PlacementOptionType::Policy => parser_ast::TableOptionType::Policy,
                };
                out.item = Some(Box::new(parser_ast::TableOption {
                    Tp: tp,
                    StrValue: placement.StrValue,
                    UintValue: placement.UintValue,
                    ..Default::default()
                }));
            } else {
                let tp = match rule {
                    DdlRule::PartDefOptionAlt01 => parser_ast::TableOptionType::Comment,
                    DdlRule::PartDefOptionAlt02 | DdlRule::PartDefOptionAlt03 => {
                        parser_ast::TableOptionType::Engine
                    }
                    DdlRule::PartDefOptionAlt04 => parser_ast::TableOptionType::EngineAttribute,
                    DdlRule::PartDefOptionAlt05 => {
                        parser_ast::TableOptionType::SecondaryEngineAttribute
                    }
                    DdlRule::PartDefOptionAlt06 => parser_ast::TableOptionType::InsertMethod,
                    DdlRule::PartDefOptionAlt07 => parser_ast::TableOptionType::DataDirectory,
                    DdlRule::PartDefOptionAlt08 => parser_ast::TableOptionType::IndexDirectory,
                    DdlRule::PartDefOptionAlt09 => parser_ast::TableOptionType::MaxRows,
                    DdlRule::PartDefOptionAlt10 => parser_ast::TableOptionType::MinRows,
                    DdlRule::PartDefOptionAlt11 => parser_ast::TableOptionType::Tablespace,
                    _ => parser_ast::TableOptionType::Nodegroup,
                };
                let numeric = matches!(
                    rule,
                    DdlRule::PartDefOptionAlt09
                        | DdlRule::PartDefOptionAlt10
                        | DdlRule::PartDefOptionAlt12
                );
                out.item = Some(Box::new(parser_ast::TableOption {
                    Tp: tp,
                    StrValue: if numeric {
                        String::new()
                    } else {
                        rhs.borrow(rhs_len - (0))
                            .expect("DDL RHS position")
                            .ident
                            .clone()
                    },
                    UintValue: if numeric {
                        rhs.borrow(rhs_len - (0))
                            .expect("DDL RHS position")
                            .item
                            .as_deref()
                            .map(getUint64FromNUM)
                            .unwrap_or_default()
                    } else {
                        0
                    },
                    ..Default::default()
                }));
            }
        }
        DdlRule::PartDefValuesOptAlt01 => {
            out.item = Some(Box::new(parser_ast::PartitionDefinitionClause::None))
        }
        DdlRule::PartDefValuesOptAlt02 => {
            out.item = Some(Box::new(parser_ast::PartitionDefinitionClause::LessThan(
                vec![parser_ast::ExprNode {
                    node_text: Default::default(),
                    Kind: parser_ast::ExprKind::MaxValue,
                    OriginTextPosition: 0,
                    Flag: Default::default(),
                }],
            )))
        }
        DdlRule::PartDefValuesOptAlt03 => {
            out.item = Some(Box::new(parser_ast::PartitionDefinitionClause::LessThan(
                rhs.borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                    .cloned()
                    .unwrap_or_default(),
            )))
        }
        DdlRule::PartDefValuesOptAlt04 => {
            out.item = Some(Box::new(parser_ast::PartitionDefinitionClause::In(vec![
                vec![parser_ast::ExprNode {
                    node_text: Default::default(),
                    Kind: parser_ast::ExprKind::DefaultValue,
                    OriginTextPosition: 0,
                    Flag: Default::default(),
                }],
            ])))
        }
        DdlRule::PartDefValuesOptAlt05 => {
            let exprs = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                .cloned()
                .unwrap_or_default();
            let values = exprs
                .into_iter()
                .map(|expr| match expr.Kind {
                    parser_ast::ExprKind::Row(values) => values,
                    _ => vec![expr],
                })
                .collect();
            out.item = Some(Box::new(parser_ast::PartitionDefinitionClause::In(values)));
        }
        DdlRule::PartDefValuesOptAlt06 | DdlRule::PartDefValuesOptAlt07 => {
            out.item = Some(Box::new(parser_ast::PartitionDefinitionClause::History {
                Current: rule == DdlRule::PartDefValuesOptAlt07,
            }))
        }
        DdlRule::DuplicateOptAlt02 | DdlRule::DuplicateOptAlt03 => {
            out.item = Some(Box::new(if rule == DdlRule::DuplicateOptAlt02 {
                parser_ast::OnDuplicateKeyHandlingType::Ignore
            } else {
                parser_ast::OnDuplicateKeyHandlingType::Replace
            }))
        }
        DdlRule::CreateTableSelectOptAlt02
        | DdlRule::CreateTableSelectOptAlt03
        | DdlRule::CreateTableSelectOptAlt04 => {
            let select = rhs
                .borrow_mut(rhs_len - (0))
                .and_then(|value| value.statement.take());
            out.item = Some(Box::new(CreateTableSemantic {
                statement: parser_ast::CreateTableStmt {
                    Select: select,
                    ..Default::default()
                },
                complete: true,
            }));
        }
        DdlRule::CreateTableSelectOptAlt05 => {
            let select =
                take_subquery_statement(rhs.borrow_mut(rhs_len - (0)).expect("DDL RHS position"));
            if let Some(node) = select {
                if let Some(value) = node.as_any().downcast_ref::<parser_ast::SelectStmt>() {
                    let _ = value;
                }
                out.item = Some(Box::new(CreateTableSemantic {
                    statement: parser_ast::CreateTableStmt {
                        Select: Some(node),
                        ..Default::default()
                    },
                    complete: true,
                }));
            } else {
                return Ok(false);
            }
        }
        DdlRule::DropDatabaseStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::DropDatabaseStmt {
                node_text: Default::default(),
                IfExists: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Name: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
            }));
        }
        DdlRule::DropIndexStmtAlt01 | DdlRule::DropIndexStmtAlt02 => {
            let (if_back, name_back, table_back, lock_back, hypo) =
                if rule == DdlRule::DropIndexStmtAlt01 {
                    (4, 3, 1, Some(0), false)
                } else {
                    (3, 2, 0, None, true)
                };
            out.statement = Some(Box::new(parser_ast::DropIndexStmt {
                node_text: Default::default(),
                IfExists: rhs
                    .borrow(rhs_len - (if_back))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                IndexName: rhs
                    .borrow(rhs_len - (name_back))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
                Table: rhs
                    .borrow(rhs_len - (table_back))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned()
                    .unwrap_or_default(),
                IsHypo: hypo,
                LockAlg: lock_back
                    .and_then(|back| {
                        rhs.borrow(rhs_len - (back))
                            .expect("DDL RHS position")
                            .item
                            .as_deref()
                    })
                    .and_then(|item| item.downcast_ref::<parser_ast::IndexLockAndAlgorithm>())
                    .cloned(),
            }));
        }
        DdlRule::DropTableStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::DropTableStmt {
                node_text: Default::default(),
                IfExists: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Tables: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                    .cloned()
                    .unwrap_or_default(),
                IsView: false,
                TemporaryKeyword: rhs
                    .borrow(rhs_len - (4))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TemporaryKeyword>())
                    .copied()
                    .unwrap_or_default(),
            }));
        }
        DdlRule::OptTemporaryAlt01 | DdlRule::OptTemporaryAlt02 | DdlRule::OptTemporaryAlt03 => {
            out.item = Some(Box::new(match rule {
                DdlRule::OptTemporaryAlt01 => parser_ast::TemporaryKeyword::None,
                DdlRule::OptTemporaryAlt02 => parser_ast::TemporaryKeyword::Local,
                _ => parser_ast::TemporaryKeyword::Global,
            }))
        }
        DdlRule::DropViewStmtAlt01 | DdlRule::DropViewStmtAlt02 => {
            out.statement = Some(Box::new(parser_ast::DropTableStmt {
                IfExists: rule == DdlRule::DropViewStmtAlt02,
                Tables: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                    .cloned()
                    .unwrap_or_default(),
                IsView: true,
                ..Default::default()
            }));
        }
        DdlRule::IndexNameAlt01 | DdlRule::IndexNameAlt02 => {
            out.item = Some(Box::new(parser_ast::NullString {
                String: if rule == DdlRule::IndexNameAlt01 {
                    String::new()
                } else {
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone()
                },
                Empty: rule == DdlRule::IndexNameAlt02
                    && rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .is_empty(),
            }))
        }
        DdlRule::IndexOptionListAlt02 => {
            let left = rhs
                .borrow_mut(rhs_len - (1))
                .and_then(|value| value.item.take())
                .and_then(|item| item.downcast::<parser_ast::IndexOption>().ok())
                .map(|item| *item);
            let right = rhs
                .borrow_mut(rhs_len - (0))
                .and_then(|value| value.item.take())
                .and_then(|item| item.downcast::<parser_ast::IndexOption>().ok())
                .map(|item| *item);
            out.item = match (left, right) {
                (None, value) => value.map(|value| Box::new(value) as Box<dyn Any>),
                (Some(mut left), Some(right)) => {
                    if !right.Comment.is_empty() {
                        left.Comment = right.Comment;
                    } else if right.Tp != parser_ast::IndexType::Invalid {
                        left.Tp = right.Tp;
                    } else if right.KeyBlockSize > 0 {
                        left.KeyBlockSize = right.KeyBlockSize;
                    } else if !right.ParserName.O.is_empty() {
                        left.ParserName = right.ParserName;
                    } else if right.Visibility != parser_ast::IndexVisibility::Default {
                        left.Visibility = right.Visibility;
                    } else if right.PrimaryKeyTp != parser_ast::PrimaryKeyType::Default {
                        left.PrimaryKeyTp = right.PrimaryKeyTp;
                    } else if right.AddColumnarReplicaOnDemand > 0 {
                        left.AddColumnarReplicaOnDemand = right.AddColumnarReplicaOnDemand;
                    } else if right.Global {
                        left.Global = true;
                    } else if right.SplitOpt.is_some() {
                        left.SplitOpt = right.SplitOpt;
                        left.AutoPreSplit = false;
                    } else if right.AutoPreSplit {
                        if left.SplitOpt.is_none() {
                            left.AutoPreSplit = true;
                        }
                    } else if !right.SecondaryEngineAttr.is_empty() {
                        left.SecondaryEngineAttr = right.SecondaryEngineAttr;
                    } else if right.Condition.is_some() {
                        left.Condition = right.Condition;
                    }
                    Some(Box::new(left))
                }
                (Some(left), None) => Some(Box::new(left)),
            };
        }
        DdlRule::IndexOptionAlt01
        | DdlRule::IndexOptionAlt02
        | DdlRule::IndexOptionAlt03
        | DdlRule::IndexOptionAlt04
        | DdlRule::IndexOptionAlt05
        | DdlRule::IndexOptionAlt06
        | DdlRule::IndexOptionAlt07
        | DdlRule::IndexOptionAlt08
        | DdlRule::IndexOptionAlt09
        | DdlRule::IndexOptionAlt10
        | DdlRule::IndexOptionAlt11
        | DdlRule::IndexOptionAlt12
        | DdlRule::IndexOptionAlt13
        | DdlRule::IndexOptionAlt14 => {
            let mut option = parser_ast::IndexOption::default();
            match rule {
                DdlRule::IndexOptionAlt01 => {
                    option.KeyBlockSize = rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(semantic_numeric_isize)
                        .unwrap_or_default()
                        .max(0) as u64
                }
                DdlRule::IndexOptionAlt02 => option.AddColumnarReplicaOnDemand = 1,
                DdlRule::IndexOptionAlt03 => {
                    option.Tp = rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::IndexType>())
                        .copied()
                        .unwrap_or_default()
                }
                DdlRule::IndexOptionAlt04 => {
                    option.ParserName = parser_ast::NewCIStr(
                        &rhs.borrow(rhs_len - (0)).expect("DDL RHS position").ident,
                    )
                }
                DdlRule::IndexOptionAlt05 => {
                    option.Comment = rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone()
                }
                DdlRule::IndexOptionAlt06 => {
                    option.Visibility = rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::IndexVisibility>())
                        .copied()
                        .unwrap_or_default()
                }
                DdlRule::IndexOptionAlt07 => {
                    option.PrimaryKeyTp = rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::PrimaryKeyType>())
                        .copied()
                        .unwrap_or_default()
                }
                DdlRule::IndexOptionAlt08 => option.Global = true,
                DdlRule::IndexOptionAlt09 => option.Global = false,
                DdlRule::IndexOptionAlt10 => {
                    option.SplitOpt = rhs
                        .borrow(rhs_len - (1))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::SplitOption>())
                        .cloned()
                }
                DdlRule::IndexOptionAlt11 => {
                    option.SplitOpt = Some(parser_ast::SplitOption {
                        Num: rhs
                            .borrow(rhs_len - (0))
                            .expect("DDL RHS position")
                            .item
                            .as_deref()
                            .and_then(semantic_numeric_isize)
                            .unwrap_or_default() as i64,
                        ..Default::default()
                    })
                }
                DdlRule::IndexOptionAlt12 => {
                    option.SecondaryEngineAttr = rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .ident
                        .clone()
                }
                DdlRule::IndexOptionAlt13 => {
                    option.Condition = rhs
                        .borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .expr
                        .clone()
                }
                DdlRule::IndexOptionAlt14 => option.AutoPreSplit = true,
                _ => {}
            }
            out.item = Some(Box::new(option));
        }
        DdlRule::IndexNameAndTypeOptAlt01
        | DdlRule::IndexNameAndTypeOptAlt02
        | DdlRule::IndexNameAndTypeOptAlt03 => {
            let name = if rule == DdlRule::IndexNameAndTypeOptAlt03 {
                parser_ast::NullString {
                    String: rhs
                        .borrow(rhs_len - (2))
                        .expect("DDL RHS position")
                        .ident
                        .clone(),
                    Empty: rhs
                        .borrow(rhs_len - (2))
                        .expect("DDL RHS position")
                        .ident
                        .is_empty(),
                }
            } else {
                rhs.borrow(
                    rhs_len
                        - (if rule == DdlRule::IndexNameAndTypeOptAlt01 {
                            0
                        } else {
                            2
                        }),
                )
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::NullString>())
                .cloned()
                .unwrap_or_default()
            };
            let index_type = if rule == DdlRule::IndexNameAndTypeOptAlt01 {
                None
            } else {
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::IndexType>())
                    .copied()
            };
            out.item = Some(Box::new(IndexNameAndTypeSemantic { name, index_type }));
        }
        DdlRule::IndexTypeAlt01 | DdlRule::IndexTypeAlt02 => {
            out.item = rhs
                .borrow_mut(rhs_len - (0))
                .and_then(|value| value.item.take())
        }
        DdlRule::IndexTypeNameAlt01
        | DdlRule::IndexTypeNameAlt02
        | DdlRule::IndexTypeNameAlt03
        | DdlRule::IndexTypeNameAlt04
        | DdlRule::IndexTypeNameAlt05
        | DdlRule::IndexTypeNameAlt06 => {
            out.item = Some(Box::new(match rule {
                DdlRule::IndexTypeNameAlt01 => parser_ast::IndexType::Btree,
                DdlRule::IndexTypeNameAlt02 => parser_ast::IndexType::Hash,
                DdlRule::IndexTypeNameAlt03 => parser_ast::IndexType::Rtree,
                DdlRule::IndexTypeNameAlt04 => parser_ast::IndexType::Hypo,
                DdlRule::IndexTypeNameAlt05 => parser_ast::IndexType::HNSW,
                _ => parser_ast::IndexType::Inverted,
            }))
        }
        DdlRule::IndexInvisibleAlt01 | DdlRule::IndexInvisibleAlt02 => {
            out.item = Some(Box::new(if rule == DdlRule::IndexInvisibleAlt01 {
                parser_ast::IndexVisibility::Visible
            } else {
                parser_ast::IndexVisibility::Invisible
            }))
        }
        DdlRule::StringTypeAlt17 => {
            let element_back = 1;
            let element = rhs
                .borrow(rhs_len - (element_back))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<VectorElementTypeSemantic>())
                .copied()
                .unwrap_or_default();
            if element.tp != parser_mysql::r#type::TypeFloat {
                yylex.AppendError(yylex.Errorf("Only VECTOR is supported for now", &[]));
            }
            let mut field_type =
                parser_types::types::NewFieldType(parser_mysql::r#type::TypeTiDBVectorFloat32);
            field_type.SetFlen(
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or(-1),
            );
            field_type.SetDecimal(0);
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
            out.item = Some(Box::new(field_type));
        }
        DdlRule::TableOptionAlt02
        | DdlRule::TableOptionAlt03
        | DdlRule::TableOptionAlt04
        | DdlRule::TableOptionAlt05
        | DdlRule::TableOptionAlt06
        | DdlRule::TableOptionAlt07
        | DdlRule::TableOptionAlt08
        | DdlRule::TableOptionAlt09
        | DdlRule::TableOptionAlt10
        | DdlRule::TableOptionAlt11
        | DdlRule::TableOptionAlt12
        | DdlRule::TableOptionAlt13
        | DdlRule::TableOptionAlt14
        | DdlRule::TableOptionAlt15
        | DdlRule::TableOptionAlt16
        | DdlRule::TableOptionAlt17
        | DdlRule::TableOptionAlt18
        | DdlRule::TableOptionAlt19
        | DdlRule::TableOptionAlt20
        | DdlRule::TableOptionAlt21
        | DdlRule::TableOptionAlt22
        | DdlRule::TableOptionAlt23
        | DdlRule::TableOptionAlt24
        | DdlRule::TableOptionAlt25
        | DdlRule::TableOptionAlt26
        | DdlRule::TableOptionAlt27
        | DdlRule::TableOptionAlt28
        | DdlRule::TableOptionAlt29
        | DdlRule::TableOptionAlt30
        | DdlRule::TableOptionAlt31
        | DdlRule::TableOptionAlt32
        | DdlRule::TableOptionAlt33
        | DdlRule::TableOptionAlt34
        | DdlRule::TableOptionAlt35
        | DdlRule::TableOptionAlt36
        | DdlRule::TableOptionAlt37
        | DdlRule::TableOptionAlt38
        | DdlRule::TableOptionAlt39
        | DdlRule::TableOptionAlt40
        | DdlRule::TableOptionAlt41
        | DdlRule::TableOptionAlt42
        | DdlRule::TableOptionAlt43
        | DdlRule::TableOptionAlt44
        | DdlRule::TableOptionAlt45
        | DdlRule::TableOptionAlt46 => {
            let tp = match rule {
                DdlRule::TableOptionAlt02 => parser_ast::TableOptionType::Charset,
                DdlRule::TableOptionAlt03 => parser_ast::TableOptionType::Collate,
                DdlRule::TableOptionAlt04 => parser_ast::TableOptionType::AutoIncrement,
                DdlRule::TableOptionAlt05 => parser_ast::TableOptionType::AutoIdCache,
                DdlRule::TableOptionAlt06 => parser_ast::TableOptionType::AutoRandomBase,
                DdlRule::TableOptionAlt07 => parser_ast::TableOptionType::AvgRowLength,
                DdlRule::TableOptionAlt08 => parser_ast::TableOptionType::Connection,
                DdlRule::TableOptionAlt09 => parser_ast::TableOptionType::CheckSum,
                DdlRule::TableOptionAlt10 => parser_ast::TableOptionType::TableCheckSum,
                DdlRule::TableOptionAlt11 => parser_ast::TableOptionType::Password,
                DdlRule::TableOptionAlt12 => parser_ast::TableOptionType::Compression,
                DdlRule::TableOptionAlt13 => parser_ast::TableOptionType::KeyBlockSize,
                DdlRule::TableOptionAlt14 => parser_ast::TableOptionType::DelayKeyWrite,
                DdlRule::TableOptionAlt15 => parser_ast::TableOptionType::RowFormat,
                DdlRule::TableOptionAlt16 => parser_ast::TableOptionType::StatsPersistent,
                DdlRule::TableOptionAlt17 | DdlRule::TableOptionAlt18 => {
                    parser_ast::TableOptionType::StatsAutoRecalc
                }
                DdlRule::TableOptionAlt19 | DdlRule::TableOptionAlt20 => {
                    parser_ast::TableOptionType::StatsSamplePages
                }
                DdlRule::TableOptionAlt21 => parser_ast::TableOptionType::StatsBuckets,
                DdlRule::TableOptionAlt22 => parser_ast::TableOptionType::StatsTopN,
                DdlRule::TableOptionAlt23 => parser_ast::TableOptionType::StatsSampleRate,
                DdlRule::TableOptionAlt24 => parser_ast::TableOptionType::StatsColsChoice,
                DdlRule::TableOptionAlt25 => parser_ast::TableOptionType::StatsColList,
                DdlRule::TableOptionAlt26 => parser_ast::TableOptionType::ShardRowID,
                DdlRule::TableOptionAlt27 => parser_ast::TableOptionType::PreSplitRegion,
                DdlRule::TableOptionAlt28 => parser_ast::TableOptionType::PackKeys,
                DdlRule::TableOptionAlt29 | DdlRule::TableOptionAlt30 => {
                    parser_ast::TableOptionType::StorageMedia
                }
                DdlRule::TableOptionAlt31 => parser_ast::TableOptionType::SecondaryEngineNull,
                DdlRule::TableOptionAlt32 => parser_ast::TableOptionType::SecondaryEngine,
                DdlRule::TableOptionAlt33 => parser_ast::TableOptionType::Union,
                DdlRule::TableOptionAlt34 => parser_ast::TableOptionType::Encryption,
                DdlRule::TableOptionAlt35 => parser_ast::TableOptionType::TTL,
                DdlRule::TableOptionAlt36 => parser_ast::TableOptionType::TTLEnable,
                DdlRule::TableOptionAlt37 => parser_ast::TableOptionType::TTLJobInterval,
                DdlRule::TableOptionAlt38 => parser_ast::TableOptionType::AutoextendSize,
                DdlRule::TableOptionAlt39 => parser_ast::TableOptionType::Affinity,
                DdlRule::TableOptionAlt40 => parser_ast::TableOptionType::PageChecksum,
                DdlRule::TableOptionAlt41 => parser_ast::TableOptionType::PageCompressed,
                DdlRule::TableOptionAlt42 => parser_ast::TableOptionType::PageCompressionLevel,
                DdlRule::TableOptionAlt43 => parser_ast::TableOptionType::Transactional,
                DdlRule::TableOptionAlt44 => parser_ast::TableOptionType::Sequence,
                DdlRule::TableOptionAlt46 => parser_ast::TableOptionType::StorageClass,
                _ => parser_ast::TableOptionType::IetfQuotes,
            };
            let mut option = parser_ast::TableOption {
                Tp: tp,
                ..Default::default()
            };
            if matches!(rule, DdlRule::TableOptionAlt04 | DdlRule::TableOptionAlt06) {
                option.BoolValue = rhs
                    .borrow(rhs_len - (3))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false);
            }
            if matches!(
                rule,
                DdlRule::TableOptionAlt04
                    | DdlRule::TableOptionAlt05
                    | DdlRule::TableOptionAlt06
                    | DdlRule::TableOptionAlt07
                    | DdlRule::TableOptionAlt09
                    | DdlRule::TableOptionAlt10
                    | DdlRule::TableOptionAlt13
                    | DdlRule::TableOptionAlt14
                    | DdlRule::TableOptionAlt15
                    | DdlRule::TableOptionAlt17
                    | DdlRule::TableOptionAlt19
                    | DdlRule::TableOptionAlt21
                    | DdlRule::TableOptionAlt22
                    | DdlRule::TableOptionAlt26
                    | DdlRule::TableOptionAlt27
                    | DdlRule::TableOptionAlt40
                    | DdlRule::TableOptionAlt41
                    | DdlRule::TableOptionAlt42
                    | DdlRule::TableOptionAlt43
                    | DdlRule::TableOptionAlt44
            ) {
                option.UintValue = rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default()
                    .max(0) as u64;
            }
            if matches!(
                rule,
                DdlRule::TableOptionAlt02
                    | DdlRule::TableOptionAlt03
                    | DdlRule::TableOptionAlt08
                    | DdlRule::TableOptionAlt11
                    | DdlRule::TableOptionAlt12
                    | DdlRule::TableOptionAlt24
                    | DdlRule::TableOptionAlt25
                    | DdlRule::TableOptionAlt32
                    | DdlRule::TableOptionAlt34
                    | DdlRule::TableOptionAlt37
                    | DdlRule::TableOptionAlt38
                    | DdlRule::TableOptionAlt39
                    | DdlRule::TableOptionAlt45
                    | DdlRule::TableOptionAlt46
            ) {
                option.StrValue = rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .clone();
                if rule == DdlRule::TableOptionAlt46 {
                    option.StrValue.make_ascii_uppercase();
                }
            }
            if rule == DdlRule::TableOptionAlt29 {
                option.StrValue = "MEMORY".to_owned();
            }
            if rule == DdlRule::TableOptionAlt30 {
                option.StrValue = "DISK".to_owned();
            }
            if matches!(rule, DdlRule::TableOptionAlt18 | DdlRule::TableOptionAlt20) {
                option.Default = true;
            }
            if rule == DdlRule::TableOptionAlt17 && option.UintValue > 1 {
                yylex.AppendError(yylex.Errorf(
                    "The value of STATS_AUTO_RECALC must be one of [0|1|DEFAULT].",
                    &[],
                ));
                return Err(1);
            }
            if rule == DdlRule::TableOptionAlt23 {
                option.Value = Some(parser_ast::ExprNode::Value(
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .map(semantic_value_text)
                        .unwrap_or_default(),
                ));
            }
            if rule == DdlRule::TableOptionAlt33 {
                option.TableNames = rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                    .cloned()
                    .unwrap_or_default();
            }
            if rule == DdlRule::TableOptionAlt35 {
                option.ColumnName = Some(parser_ast::ColumnName {
                    Name: parser_ast::NewCIStr(
                        &rhs.borrow(rhs_len - (4)).expect("DDL RHS position").ident,
                    ),
                    ..Default::default()
                });
                option.Value = rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .expr
                    .clone();
                option.TimeUnitValue = rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TimeUnitType>())
                    .copied();
            }
            if rule == DdlRule::TableOptionAlt36 {
                match rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "on" => option.BoolValue = true,
                    "off" => option.BoolValue = false,
                    _ => {
                        yylex.AppendError(
                            yylex.Errorf("The TTL_ENABLE option has to be set 'ON' or 'OFF'", &[]),
                        );
                        return Err(1);
                    }
                }
            }
            if rule == DdlRule::TableOptionAlt37 {
                if let Err(error) = parser_duration::ParseDuration(&option.StrValue) {
                    yylex.AppendError(yylex.Errorf(
                        &format!("The TTL_JOB_INTERVAL option is not a valid duration: {error}"),
                        &[],
                    ));
                    return Err(1);
                }
            }
            if matches!(
                rule,
                DdlRule::TableOptionAlt11
                    | DdlRule::TableOptionAlt17
                    | DdlRule::TableOptionAlt18
                    | DdlRule::TableOptionAlt19
                    | DdlRule::TableOptionAlt20
                    | DdlRule::TableOptionAlt29
                    | DdlRule::TableOptionAlt30
                    | DdlRule::TableOptionAlt31
                    | DdlRule::TableOptionAlt32
                    | DdlRule::TableOptionAlt33
                    | DdlRule::TableOptionAlt34
                    | DdlRule::TableOptionAlt38
                    | DdlRule::TableOptionAlt40
                    | DdlRule::TableOptionAlt41
                    | DdlRule::TableOptionAlt42
                    | DdlRule::TableOptionAlt43
                    | DdlRule::TableOptionAlt44
                    | DdlRule::TableOptionAlt45
            ) {
                yylex.AppendError(yylex.Errorf(
                    "The table option is parsed but ignored by all storage engines.",
                    &[],
                ));
                yylex.LastErrorAsWarn();
            }
            out.item = Some(Box::new(option));
        }
        DdlRule::ForceOptAlt01 | DdlRule::ForceOptAlt02 => {
            out.item = Some(Box::new(rule == DdlRule::ForceOptAlt02))
        }
        DdlRule::CreateTableOptionAlt02 => {
            if !parser_state.enableUnsupportedMySQLSyntax {
                yylex.AppendError(yylex.Errorf("syntax error", &[]));
                return Err(1);
            }
            out.item = Some(Box::new(parser_ast::TableOption {
                Tp: parser_ast::TableOptionType::StartTransaction,
                ..Default::default()
            }));
        }
        DdlRule::TableOptionListAlt01 | DdlRule::CreateTableOptionListAlt01 => {
            let values = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableOption>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        DdlRule::TableOptionListAlt02
        | DdlRule::TableOptionListAlt03
        | DdlRule::CreateTableOptionListAlt02
        | DdlRule::CreateTableOptionListAlt03 => {
            let list_back = if matches!(rule, DdlRule::TableOptionListAlt02 | DdlRule::CreateTableOptionListAlt02) {
                1
            } else {
                2
            };
            let mut values = rhs
                .borrow(rhs_len - (list_back))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOption>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableOption>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::RowFormatAlt01
        | DdlRule::RowFormatAlt02
        | DdlRule::RowFormatAlt03
        | DdlRule::RowFormatAlt04
        | DdlRule::RowFormatAlt05
        | DdlRule::RowFormatAlt06
        | DdlRule::RowFormatAlt07
        | DdlRule::RowFormatAlt08
        | DdlRule::RowFormatAlt09
        | DdlRule::RowFormatAlt10
        | DdlRule::RowFormatAlt11
        | DdlRule::RowFormatAlt12
        | DdlRule::RowFormatAlt13
        | DdlRule::RowFormatAlt14
        | DdlRule::RowFormatAlt15 => {
            out.item = Some(Box::new(match rule {
                DdlRule::RowFormatAlt01 => parser_ast::RowFormatType::Default,
                DdlRule::RowFormatAlt02 => parser_ast::RowFormatType::Dynamic,
                DdlRule::RowFormatAlt03 => parser_ast::RowFormatType::Fixed,
                DdlRule::RowFormatAlt04 => parser_ast::RowFormatType::Compressed,
                DdlRule::RowFormatAlt05 => parser_ast::RowFormatType::Redundant,
                DdlRule::RowFormatAlt06 => parser_ast::RowFormatType::Compact,
                DdlRule::RowFormatAlt07 => parser_ast::RowFormatType::TokuDefault,
                DdlRule::RowFormatAlt08 => parser_ast::RowFormatType::TokuFast,
                DdlRule::RowFormatAlt09 => parser_ast::RowFormatType::TokuSmall,
                DdlRule::RowFormatAlt10 => parser_ast::RowFormatType::TokuZlib,
                DdlRule::RowFormatAlt11 => parser_ast::RowFormatType::TokuZstd,
                DdlRule::RowFormatAlt12 => parser_ast::RowFormatType::TokuQuickLz,
                DdlRule::RowFormatAlt13 => parser_ast::RowFormatType::TokuLzma,
                DdlRule::RowFormatAlt14 => parser_ast::RowFormatType::TokuSnappy,
                _ => parser_ast::RowFormatType::TokuUncompressed,
            }))
        }
        DdlRule::NumericTypeAlt05 => {
            let Some(tp) = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<u8>())
                .copied()
            else {
                return Ok(false);
            };
            let mut field_type = parser_types::types::NewFieldType(tp);
            let flen = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(semantic_numeric_isize)
                .unwrap_or(-1);
            field_type.SetFlen(if flen == -1 { 1 } else { flen });
            out.item = Some(Box::new(field_type));
        }
        DdlRule::StringTypeAlt09 => {
            let Some(mut field_type) = rhs
                .borrow_mut(rhs_len - (0))
                .and_then(|value| value.item.take())
                .and_then(|item| item.downcast::<parser_types::types::FieldType>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
            field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
            out.item = Some(Box::new(field_type));
        }
        DdlRule::StringTypeAlt10 => {
            let Some(mut field_type) = rhs
                .borrow_mut(rhs_len - (1))
                .and_then(|value| value.item.take())
                .and_then(|item| item.downcast::<parser_types::types::FieldType>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let option = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<OptBinarySemantic>())
                .cloned()
                .unwrap_or_default();
            field_type.SetCharset(option.charset.clone());
            if option.charset == "binary" {
                field_type.SetCollate("binary".to_owned());
                field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
            }
            if option.binary {
                field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
            }
            out.item = Some(Box::new(field_type));
        }
        DdlRule::StringTypeAlt13 | DdlRule::StringTypeAlt15 | DdlRule::StringTypeAlt16 => {
            let tp = if rule == DdlRule::StringTypeAlt13 {
                parser_mysql::r#type::TypeJSON
            } else {
                parser_mysql::r#type::TypeMediumBlob
            };
            let mut field_type = parser_types::types::NewFieldType(tp);
            if rule == DdlRule::StringTypeAlt13 {
                field_type.SetDecimal(0);
                field_type.SetCharset("binary".to_owned());
                field_type.SetCollate("binary".to_owned());
            } else {
                let option = rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<OptBinarySemantic>())
                    .cloned()
                    .unwrap_or_default();
                field_type.SetCharset(option.charset.clone());
                if option.charset == "binary" {
                    field_type.SetCollate("binary".to_owned());
                    field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
                }
                if option.binary {
                    field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
                }
            }
            out.item = Some(Box::new(field_type));
        }
        DdlRule::BlobTypeAlt01
        | DdlRule::BlobTypeAlt02
        | DdlRule::BlobTypeAlt03
        | DdlRule::BlobTypeAlt04
        | DdlRule::BlobTypeAlt05
        | DdlRule::TextTypeAlt01
        | DdlRule::TextTypeAlt02
        | DdlRule::TextTypeAlt03
        | DdlRule::TextTypeAlt04 => {
            let tp = match rule {
                DdlRule::BlobTypeAlt01 | DdlRule::TextTypeAlt01 => {
                    parser_mysql::r#type::TypeTinyBlob
                }
                DdlRule::BlobTypeAlt02 | DdlRule::TextTypeAlt02 => parser_mysql::r#type::TypeBlob,
                DdlRule::BlobTypeAlt03 | DdlRule::BlobTypeAlt05 | DdlRule::TextTypeAlt03 => {
                    parser_mysql::r#type::TypeMediumBlob
                }
                _ => parser_mysql::r#type::TypeLongBlob,
            };
            let mut field_type = parser_types::types::NewFieldType(tp);
            if matches!(rule, DdlRule::BlobTypeAlt02 | DdlRule::TextTypeAlt02) {
                field_type.SetFlen(
                    rhs.borrow(rhs_len - (0))
                        .expect("DDL RHS position")
                        .item
                        .as_deref()
                        .and_then(semantic_numeric_isize)
                        .unwrap_or(-1),
                );
            }
            out.item = Some(Box::new(field_type));
        }
        DdlRule::OptCharsetWithOptBinaryAlt02
        | DdlRule::OptCharsetWithOptBinaryAlt03
        | DdlRule::OptCharsetWithOptBinaryAlt04 => {
            out.item = Some(Box::new(OptBinarySemantic {
                binary: false,
                charset: match rule {
                    DdlRule::OptCharsetWithOptBinaryAlt02 => "latin1",
                    DdlRule::OptCharsetWithOptBinaryAlt03 => "ucs2",
                    _ => "binary",
                }
                .to_owned(),
            }))
        }
        DdlRule::DateAndTimeTypeAlt02
        | DdlRule::DateAndTimeTypeAlt03
        | DdlRule::DateAndTimeTypeAlt04
        | DdlRule::DateAndTimeTypeAlt05 => {
            let tp = match rule {
                DdlRule::DateAndTimeTypeAlt02 => parser_mysql::r#type::TypeDatetime,
                DdlRule::DateAndTimeTypeAlt03 => parser_mysql::r#type::TypeTimestamp,
                DdlRule::DateAndTimeTypeAlt04 => parser_mysql::r#type::TypeDuration,
                _ => parser_mysql::r#type::TypeYear,
            };
            let mut field_type = parser_types::types::NewFieldType(tp);
            if rule == DdlRule::DateAndTimeTypeAlt05 {
                let flen = rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or(-1);
                if flen != -1 && flen != 4 {
                    yylex.AppendError(ErrInvalidYearColumnLength.GenWithStackByArgs(&[]));
                    return Err(-1);
                }
                field_type.SetFlen(flen);
            } else {
                let decimal = rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default();
                let base = if rule == DdlRule::DateAndTimeTypeAlt04 {
                    parser_mysql::r#const::MaxDurationWidthNoFsp
                } else {
                    parser_mysql::r#const::MaxDatetimeWidthNoFsp
                } as isize;
                field_type.SetDecimal(decimal);
                field_type.SetFlen(base + if decimal > 0 { 1 + decimal } else { 0 });
            }
            out.item = Some(Box::new(field_type));
        }
        DdlRule::OptBinModAlt01 | DdlRule::OptBinModAlt02 => {
            out.item = Some(Box::new(rule == DdlRule::OptBinModAlt02))
        }
        DdlRule::OptVectorElementTypeAlt01
        | DdlRule::OptVectorElementTypeAlt02
        | DdlRule::OptVectorElementTypeAlt03 => {
            out.item = Some(Box::new(VectorElementTypeSemantic {
                tp: if rule == DdlRule::OptVectorElementTypeAlt03 {
                    parser_mysql::r#type::TypeDouble
                } else {
                    parser_mysql::r#type::TypeFloat
                },
            }))
        }
        DdlRule::ConstraintAlt01 => {
            let Some(mut constraint) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Constraint>())
                .cloned()
            else {
                return Ok(false);
            };
            if let Some(name) = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<String>())
            {
                constraint.Name = name.clone();
                constraint.IsEmptyIndex = name.is_empty();
            }
            out.item = Some(Box::new(constraint));
        }
        DdlRule::ConstraintVectorIndexAlt01 | DdlRule::ConstraintColumnarIndexAlt01 => {
            let Some(name_and_type) = rhs
                .borrow(rhs_len - (4))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<IndexNameAndTypeSemantic>())
                .cloned()
            else {
                return Ok(false);
            };
            let mut option = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::IndexOption>())
                .cloned()
                .unwrap_or_default();
            if let Some(tp) = name_and_type.index_type {
                option.Tp = tp;
            }
            out.item = Some(Box::new(parser_ast::Constraint {
                IfNotExists: rhs
                    .borrow(rhs_len - (5))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Tp: if rule == DdlRule::ConstraintVectorIndexAlt01 {
                    parser_ast::ConstraintType::Vector
                } else {
                    parser_ast::ConstraintType::Columnar
                },
                Keys: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::IndexPartSpecification>>())
                    .cloned()
                    .unwrap_or_default(),
                Name: name_and_type.name.String,
                IsEmptyIndex: name_and_type.name.Empty,
                Option: Some(option),
                ..Default::default()
            }));
        }
        DdlRule::ConstraintWithColumnarIndexAlt03 => {
            out.item = rhs
                .borrow_mut(rhs_len - (0))
                .and_then(|value| value.item.take())
        }
        DdlRule::TableElementListAlt01 => {
            let column = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnDef>())
                .cloned();
            let constraint = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Constraint>())
                .cloned();
            out.item = Some(Box::new(CreateTableElementsSemantic {
                complete: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .is_none()
                    || column.is_some()
                    || constraint.is_some(),
                columns: column.into_iter().collect(),
                constraints: constraint.into_iter().collect(),
            }));
        }
        DdlRule::TableElementListAlt02 => {
            let mut semantic = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<CreateTableElementsSemantic>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnDef>())
            {
                semantic.columns.push(value.clone());
            } else if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Constraint>())
            {
                semantic.constraints.push(value.clone());
            } else if rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .is_some()
            {
                semantic.complete = false;
            }
            out.item = Some(Box::new(semantic));
        }
        DdlRule::TableElementListOptAlt01 => {
            out.item = Some(Box::new(CreateTableSemantic {
                statement: parser_ast::CreateTableStmt::default(),
                complete: true,
            }))
        }
        DdlRule::TableElementListOptAlt02 => {
            let semantic = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<CreateTableElementsSemantic>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(CreateTableSemantic {
                statement: parser_ast::CreateTableStmt {
                    Cols: semantic.columns,
                    Constraints: semantic.constraints,
                    ..Default::default()
                },
                complete: semantic.complete,
            }));
        }
        DdlRule::CreateTableOptionListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::TableOption>::new()))
        }
        DdlRule::TruncateTableStmtAlt01 => {
            let table = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::TruncateTableStmt {
                node_text: Default::default(),
                Table: table,
            }));
        }
        DdlRule::NumericTypeAlt01 => {
            let Some(tp) = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<u8>())
                .copied()
            else {
                return Ok(false);
            };
            let flen = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<isize>())
                .copied()
                .unwrap_or(-1);
            let mut field_type = parser_types::types::NewFieldType(tp);
            field_type.SetFlen(flen);
            if let Some(options) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<TypeOptSemantic>>())
            {
                for option in options {
                    if option.unsigned {
                        field_type.AddFlag(parser_mysql::r#type::UnsignedFlag);
                    }
                    if option.zerofill {
                        field_type.AddFlag(parser_mysql::r#type::ZerofillFlag);
                    }
                }
            }
            out.item = Some(Box::new(field_type));
        }
        DdlRule::StringTypeAlt14 => {
            if !parser_state.enableMariaDB {
                yylex.AppendError(yylex.Errorf("syntax error", &[]));
                return Err(1);
            }
            let mut tp = parser_types::types::NewFieldType(parser_mysql::r#type::TypeString);
            tp.SetFlen(36);
            out.item = Some(Box::new(tp));
        }
        DdlRule::AlterRangeStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::AlterRangeStmt {
                node_text: Default::default(),
                RangeName: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (1)).expect("DDL RHS position").ident,
                ),
                PlacementOption: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::PlacementOption>())
                    .cloned()
                    .unwrap_or_default(),
            }))
        }
        DdlRule::NumericTypeAlt02 => {
            let Some(tp) = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<u8>())
                .copied()
            else {
                return Ok(false);
            };
            let mut field_type = parser_types::types::NewFieldType(tp);
            field_type.SetFlen(1);
            if let Some(options) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<TypeOptSemantic>>())
            {
                for option in options {
                    if option.unsigned {
                        field_type.AddFlag(parser_mysql::r#type::UnsignedFlag);
                    }
                    if option.zerofill {
                        field_type.AddFlag(parser_mysql::r#type::ZerofillFlag);
                    }
                }
            }
            out.item = Some(Box::new(field_type));
        }
        DdlRule::NumericTypeAlt03 | DdlRule::NumericTypeAlt04 => {
            let Some(tp) = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<u8>())
                .copied()
            else {
                return Ok(false);
            };
            let Some(float_opt) = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<FloatOptSemantic>())
                .copied()
            else {
                return Ok(false);
            };
            let mut field_type = parser_types::types::NewFieldType(tp);
            if rule == DdlRule::NumericTypeAlt04
                && field_type.GetType() == parser_mysql::r#type::TypeDouble
                && parser_state.strictDoubleFieldType
                && float_opt.flen != parser_types::types::UnspecifiedLength
                && float_opt.decimal == parser_types::types::UnspecifiedLength
            {
                yylex.AppendError(ErrSyntax.GenWithStackByArgs(&[]));
                return Err(1);
            }
            field_type.SetFlen(float_opt.flen);
            if rule == DdlRule::NumericTypeAlt04
                && field_type.GetType() == parser_mysql::r#type::TypeFloat
                && float_opt.decimal == parser_types::types::UnspecifiedLength
                && field_type.GetFlen() <= parser_mysql::r#const::MaxDoublePrecisionLength as isize
            {
                if field_type.GetFlen() > parser_mysql::r#const::MaxFloatPrecisionLength as isize {
                    field_type.SetType(parser_mysql::r#type::TypeDouble);
                }
                field_type.SetFlen(parser_types::types::UnspecifiedLength);
            }
            field_type.SetDecimal(float_opt.decimal);
            if let Some(options) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<TypeOptSemantic>>())
            {
                for option in options {
                    if option.unsigned {
                        field_type.AddFlag(parser_mysql::r#type::UnsignedFlag);
                    }
                    if option.zerofill {
                        field_type.AddFlag(parser_mysql::r#type::ZerofillFlag);
                    }
                }
            }
            out.item = Some(Box::new(field_type));
        }
        DdlRule::IntegerTypeAlt01
        | DdlRule::IntegerTypeAlt02
        | DdlRule::IntegerTypeAlt03
        | DdlRule::IntegerTypeAlt04
        | DdlRule::IntegerTypeAlt05
        | DdlRule::IntegerTypeAlt06
        | DdlRule::IntegerTypeAlt07
        | DdlRule::IntegerTypeAlt08
        | DdlRule::IntegerTypeAlt09
        | DdlRule::IntegerTypeAlt10
        | DdlRule::IntegerTypeAlt11
        | DdlRule::IntegerTypeAlt12 => {
            let tp = match rule {
                DdlRule::IntegerTypeAlt01 | DdlRule::IntegerTypeAlt06 => {
                    parser_mysql::r#type::TypeTiny
                }
                DdlRule::IntegerTypeAlt02 | DdlRule::IntegerTypeAlt07 => {
                    parser_mysql::r#type::TypeShort
                }
                DdlRule::IntegerTypeAlt03
                | DdlRule::IntegerTypeAlt04
                | DdlRule::IntegerTypeAlt08 => parser_mysql::r#type::TypeInt24,
                DdlRule::IntegerTypeAlt05
                | DdlRule::IntegerTypeAlt09
                | DdlRule::IntegerTypeAlt11 => parser_mysql::r#type::TypeLong,
                _ => parser_mysql::r#type::TypeLonglong,
            };
            out.item = Some(Box::new(tp));
        }
        DdlRule::BooleanTypeAlt01 | DdlRule::BooleanTypeAlt02 => {
            out.item = Some(Box::new(parser_mysql::r#type::TypeTiny))
        }
        DdlRule::FixedPointTypeAlt01
        | DdlRule::FixedPointTypeAlt02
        | DdlRule::FixedPointTypeAlt03
        | DdlRule::FloatingPointTypeAlt01
        | DdlRule::FloatingPointTypeAlt02
        | DdlRule::FloatingPointTypeAlt03
        | DdlRule::FloatingPointTypeAlt04
        | DdlRule::FloatingPointTypeAlt05
        | DdlRule::FloatingPointTypeAlt06
        | DdlRule::BitValueTypeAlt01 => {
            let tp = match rule {
                DdlRule::FixedPointTypeAlt01
                | DdlRule::FixedPointTypeAlt02
                | DdlRule::FixedPointTypeAlt03 => parser_mysql::r#type::TypeNewDecimal,
                DdlRule::FloatingPointTypeAlt01 | DdlRule::FloatingPointTypeAlt05 => {
                    parser_mysql::r#type::TypeFloat
                }
                DdlRule::FloatingPointTypeAlt02
                | DdlRule::FloatingPointTypeAlt03
                | DdlRule::FloatingPointTypeAlt04
                | DdlRule::FloatingPointTypeAlt06 => parser_mysql::r#type::TypeDouble,
                _ => parser_mysql::r#type::TypeBit,
            };
            out.item = Some(Box::new(tp));
        }
        DdlRule::StringTypeAlt01
        | DdlRule::StringTypeAlt02
        | DdlRule::StringTypeAlt03
        | DdlRule::StringTypeAlt04
        | DdlRule::StringTypeAlt05
        | DdlRule::StringTypeAlt06
        | DdlRule::StringTypeAlt07
        | DdlRule::StringTypeAlt08 => {
            let (tp, flen_back, opt_back, force_binary) = match rule {
                DdlRule::StringTypeAlt01 | DdlRule::StringTypeAlt03 => {
                    (parser_mysql::r#type::TypeString, Some(1), Some(0), false)
                }
                DdlRule::StringTypeAlt02 | DdlRule::StringTypeAlt04 => {
                    (parser_mysql::r#type::TypeString, None, Some(0), false)
                }
                DdlRule::StringTypeAlt05 | DdlRule::StringTypeAlt06 => {
                    (parser_mysql::r#type::TypeVarchar, Some(1), Some(0), false)
                }
                DdlRule::StringTypeAlt07 => (parser_mysql::r#type::TypeString, Some(0), None, true),
                _ => (parser_mysql::r#type::TypeVarchar, Some(0), None, true),
            };
            let mut field_type = parser_types::types::NewFieldType(tp);
            if let Some(back) = flen_back {
                let flen = rhs
                    .borrow(rhs_len - (back))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<isize>())
                    .copied()
                    .unwrap_or(-1);
                field_type.SetFlen(flen);
            }
            if let Some(back) = opt_back {
                if let Some(option) = rhs
                    .borrow(rhs_len - (back))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<OptBinarySemantic>())
                {
                    field_type.SetCharset(option.charset.clone());
                    if option.binary {
                        field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
                    }
                }
            }
            if force_binary {
                field_type.SetCharset("binary".to_owned());
                field_type.SetCollate("binary".to_owned());
                field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
            }
            out.item = Some(Box::new(field_type));
        }
        DdlRule::StringTypeAlt11 | DdlRule::StringTypeAlt12 => {
            let elements = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<TextStringSemantic>>())
                .cloned()
                .unwrap_or_default();
            let option = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<OptBinarySemantic>())
                .cloned()
                .unwrap_or_default();
            let mut field_type =
                parser_types::types::NewFieldType(if rule == DdlRule::StringTypeAlt11 {
                    parser_mysql::r#type::TypeEnum
                } else {
                    parser_mysql::r#type::TypeSet
                });
            let values = elements
                .iter()
                .map(|element| element.value.trim_end_matches(' ').to_owned())
                .collect::<Vec<_>>();
            let flen = if rule == DdlRule::StringTypeAlt11 {
                values.iter().map(String::len).max().unwrap_or(0)
            } else {
                values.iter().map(String::len).sum::<usize>() + values.len().saturating_sub(1)
            };
            field_type.SetFlen(flen as isize);
            field_type.SetElems(values);
            field_type.SetCharset(option.charset);
            if option.binary || elements.iter().any(|element| element.binary) {
                field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
            }
            out.item = Some(Box::new(field_type));
        }
        DdlRule::DateAndTimeTypeAlt01 => {
            out.item = Some(Box::new(parser_types::types::NewFieldType(
                parser_mysql::r#type::TypeDate,
            )))
        }
        DdlRule::FieldLenAlt01 => {
            let value = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| {
                    if let Some(value) = item.downcast_ref::<u64>() {
                        Some(*value as isize)
                    } else {
                        item.downcast_ref::<i64>().map(|value| *value as isize)
                    }
                })
                .unwrap_or(-1);
            out.item = Some(Box::new(value));
        }
        DdlRule::OptFieldLenAlt01 => out.item = Some(Box::new(-1isize)),
        DdlRule::FieldOptAlt01 => {
            out.item = Some(Box::new(TypeOptSemantic {
                unsigned: true,
                zerofill: false,
            }))
        }
        DdlRule::FieldOptAlt02 => out.item = Some(Box::new(TypeOptSemantic::default())),
        DdlRule::FieldOptAlt03 => {
            out.item = Some(Box::new(TypeOptSemantic {
                unsigned: true,
                zerofill: true,
            }))
        }
        DdlRule::FieldOptsAlt01 => out.item = Some(Box::new(Vec::<TypeOptSemantic>::new())),
        DdlRule::FieldOptsAlt02 => {
            let mut values = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<TypeOptSemantic>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<TypeOptSemantic>())
            {
                values.push(*value);
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::FloatOptAlt01 => {
            out.item = Some(Box::new(FloatOptSemantic {
                flen: -1,
                decimal: -1,
            }))
        }
        DdlRule::FloatOptAlt02 => {
            let flen = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<isize>())
                .copied()
                .unwrap_or(-1);
            out.item = Some(Box::new(FloatOptSemantic { flen, decimal: -1 }));
        }
        DdlRule::PrecisionAlt01 => {
            let flen = rhs
                .borrow(rhs_len - (3))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(semantic_numeric_isize)
                .unwrap_or_default();
            let decimal = rhs
                .borrow(rhs_len - (1))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(semantic_numeric_isize)
                .unwrap_or_default();
            out.item = Some(Box::new(FloatOptSemantic { flen, decimal }));
        }
        DdlRule::OptBinaryAlt01 => out.item = Some(Box::new(OptBinarySemantic::default())),
        DdlRule::OptBinaryAlt02 => {
            out.item = Some(Box::new(OptBinarySemantic {
                binary: true,
                charset: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
            }))
        }
        DdlRule::OptBinaryAlt03 => {
            out.item = Some(Box::new(OptBinarySemantic {
                binary: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                charset: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
            }))
        }
        DdlRule::OptCharsetAlt01 | DdlRule::OptCollateAlt01 => out.ident.clear(),
        DdlRule::OptCharsetAlt02 | DdlRule::OptCollateAlt02 => {
            out.ident = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .ident
                .clone()
        }
        DdlRule::StringListAlt01 => {
            out.item = Some(Box::new(vec![
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
            ]))
        }
        DdlRule::StringListAlt02 => {
            let mut values = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<String>>())
                .cloned()
                .unwrap_or_default();
            values.push(
                rhs.borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
            );
            out.item = Some(Box::new(values));
        }
        DdlRule::TextStringAlt01 => {
            out.item = Some(Box::new(TextStringSemantic {
                value: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .ident
                    .clone(),
                binary: false,
            }))
        }
        DdlRule::TextStringAlt02 | DdlRule::TextStringAlt03 => {
            out.item = Some(Box::new(TextStringSemantic {
                value: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .map(semantic_value_text)
                    .unwrap_or_default(),
                binary: true,
            }))
        }
        DdlRule::TextStringListAlt01 => {
            let values = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<TextStringSemantic>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        DdlRule::TextStringListAlt02 => {
            let mut values = rhs
                .borrow(rhs_len - (2))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<TextStringSemantic>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs
                .borrow(rhs_len - (0))
                .expect("DDL RHS position")
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<TextStringSemantic>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DdlRule::CreatePolicyStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::CreatePlacementPolicyStmt {
                node_text: Default::default(),
                OrReplace: rhs
                    .borrow(rhs_len - (5))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                IfNotExists: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                PolicyName: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (1)).expect("DDL RHS position").ident,
                ),
                PlacementOptions: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::PlacementOption>>())
                    .cloned()
                    .unwrap_or_default(),
            }))
        }
        DdlRule::AlterPolicyStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::AlterPlacementPolicyStmt {
                node_text: Default::default(),
                IfExists: rhs
                    .borrow(rhs_len - (2))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                PolicyName: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (1)).expect("DDL RHS position").ident,
                ),
                PlacementOptions: rhs
                    .borrow(rhs_len - (0))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::PlacementOption>>())
                    .cloned()
                    .unwrap_or_default(),
            }))
        }
        DdlRule::DropPolicyStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::DropPlacementPolicyStmt {
                node_text: Default::default(),
                IfExists: rhs
                    .borrow(rhs_len - (1))
                    .expect("DDL RHS position")
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                PolicyName: parser_ast::NewCIStr(
                    &rhs.borrow(rhs_len - (0)).expect("DDL RHS position").ident,
                ),
            }))
        }
    }
    Ok(true)
}
