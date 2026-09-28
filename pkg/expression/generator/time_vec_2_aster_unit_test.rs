// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// time_vec 生成器迁移回归测试。
//
// 校验区间单位顺序、各函数签名矩阵规模、模板渲染产物与双文件写出行为，
// 确保与 Go `expression/generator/time_vec.go` 保持一致。

#[cfg(test)]
mod tests {
    use crate::time_vec::*;
    use std::fs;

    /// Duration 组 11 项、Datetime 组 9 项，拼接后即完整区间单位列表。
    #[test]
    fn interval_units_match_go_order_and_partition() {
        let duration = interval_units_for_duration_as_duration();
        let datetime = interval_units_for_duration_as_datetime();
        assert_eq!(duration.len(), 11);
        assert_eq!(datetime.len(), 9);
        assert_eq!(duration.first(), Some(&"MICROSECOND"));
        assert_eq!(duration.last(), Some(&"DAY_MICROSECOND"));
        assert_eq!(datetime.first(), Some(&"DAY"));
        assert_eq!(datetime.last(), Some(&"YEAR_MONTH"));

        let all = interval_units();
        assert_eq!(&all[..duration.len()], duration.as_slice());
        assert_eq!(&all[duration.len()..], datetime.as_slice());
    }

    /// 各签名表长度与恒 NULL 签名个数、FUNCTIONS 函数名顺序对齐 Go。
    #[test]
    fn signature_matrices_match_go_counts_and_null_cases() {
        assert_eq!(ADD_TIME_SIGS.len(), 11);
        assert_eq!(SUB_TIME_SIGS.len(), 11);
        assert_eq!(TIME_DIFF_SIGS.len(), 8);
        assert_eq!(ADD_DATE_SIGS.len(), 32);
        assert_eq!(SUB_DATE_SIGS.len(), 32);
        assert_eq!(ADD_TIME_SIGS.iter().filter(|sig| sig.all_null).count(), 3);
        assert_eq!(SUB_TIME_SIGS.iter().filter(|sig| sig.all_null).count(), 3);
        assert_eq!(
            FUNCTIONS
                .iter()
                .map(|function| function.func_name)
                .collect::<Vec<_>>(),
            ["AddTime", "SubTime", "TimeDiff", "AddDate", "SubDate"]
        );
    }

    /// 生产模板应展开出具体签名方法体与关键计算/Resize 调用。
    #[test]
    fn production_templates_render_go_behavior_branches() {
        let add = render_template(
            ADD_OR_SUB_TIME_TEMPLATE,
            &Function {
                func_name: "AddTime",
                sigs: ADD_TIME_SIGS,
            },
        )
        .expect("render ADDTIME template");
        assert!(add.contains("func (b *builtinAddDatetimeAndDurationSig) vecEvalTime"));
        assert!(add.contains("types.AddDuration(arg0, arg1)"));
        assert!(add.contains("result.ResizeTime(n, true)"));

        let diff =
            render_template(TIME_DIFF_TEMPLATE, TIME_DIFF_SIGS).expect("render TIMEDIFF template");
        assert!(diff.contains("func (b *builtinDurationDurationTimeDiffSig) vecEvalDuration"));
        assert!(diff.contains("calculateDurationTimeDiff(ctx, lhs, rhs)"));
    }

    /// 测试模板含区间单位常量，且 generate_one_file 写出实现与 `_test.go` 对。
    #[test]
    fn test_template_expands_interval_functions_and_generation_writes_pair() {
        let rendered =
            render_test_template(TEST_FILE_TEMPLATE, &TMPL_VAL).expect("render generated Go tests");
        assert!(rendered.contains("types.NewStringDatum(\"MICROSECOND\")"));
        assert!(rendered.contains("types.NewStringDatum(\"YEAR_MONTH\")"));
        assert!(rendered.contains("TestVectorizedBuiltinTimeFuncGenerated"));

        let temp = tempfile::tempdir().expect("create temporary output directory");
        let prefix = temp.path().join("builtin_time_vec_generated");
        generate_one_file(&prefix).expect("generate implementation and test files");
        let implementation =
            fs::read_to_string(prefix.with_extension("go")).expect("read generated implementation");
        let tests = fs::read_to_string(format!("{}_test.go", prefix.display()))
            .expect("read generated tests");
        assert!(implementation.contains("builtinAddDatetimeAndDurationSig"));
        assert!(tests.contains("vecBuiltinTimeGeneratedCases"));
    }
}
