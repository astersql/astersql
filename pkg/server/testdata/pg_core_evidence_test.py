# Copyright 2026 AsterSQL.
"""Independent standard-library tests for first-phase input registration."""
import ast
import copy
import json
from collections import Counter
from pathlib import Path
import unittest

from pg_core_evidence import CaseRegistry, load_cases, validate_cases


class CaseRegistryTests(unittest.TestCase):
    def setUp(self):
        self.manifest = load_cases()
        self.table = 'public.pg_core_diff_12345'

    def test_all_first_phase_ids(self):
        expected = set()
        for typ in ['int4', 'int8']:
            for syntax in ['suffix', 'cast']:
                expected.update(f'{typ}_{syntax}_{label}' for label in
                                ['min', 'max', 'text', 'integer', 'null', 'over', 'under', 'invalid'])
                expected.update(f'recover_{typ}_{syntax}_{label}' for label in ['over', 'under', 'invalid'])
        expected.update(['int_alias', 'nested_narrow', 'nested_overflow', 'nested_recovery'])
        for name, labels in [('inferred', ['initial', 'boundary', 'null']),
                             ('zero', ['initial', 'null']), ('explicit', ['initial', 'null']),
                             ('text_source', ['initial', 'null'])]:
            expected.update([name + '_prepare', name + '_describe'])
            expected.update(name + '_run_' + label for label in labels)
            expected.update(name + '_' + label for label in
                            ['invalid', 'invalid_recovery', 'overflow', 'overflow_recovery'])
        expected.update(['write', 'readback', 'empty_prepare', 'empty_describe', 'empty_execute',
                         'begin', 'transaction_error', 'transaction_failed', 'rollback', 'rollback_recovery'])
        business = [x['case_id'] for x in self.manifest['cases'] if x['kind'] == 'business']
        checks = [x['case_id'] for x in self.manifest['cases'] if x['kind'] == 'self_check']
        self.assertEqual(set(business), expected)
        self.assertEqual(len(business), 91)
        self.assertEqual(set(checks), {'negative_self_check_' + key for key in ['rows', 'fields', 'state']})
        registry = CaseRegistry(self.manifest, self.table)
        for case in self.manifest['cases']:
            actual = copy.deepcopy(case['input'])
            if 'sql' in actual:
                actual['sql'] = actual['sql'].replace('{run_table}', self.table)
            self.assertEqual(registry.claim(case['case_id'], case['family'], actual), actual)
        self.assertEqual(registry.finish(), 94)

    def test_real_check_rejects_unregistered_before_send(self):
        # Execute the actual fixture gate with no connections: this tests rejection,
        # and cannot provide or simulate a successful database observation.
        tree = ast.parse(Path(__file__).with_name('pg_core_libpq.py').read_text())
        differential = next(x for x in tree.body if isinstance(x, ast.FunctionDef)
                            and x.name == 'integer_differential')
        check = next(x for x in differential.body if isinstance(x, ast.FunctionDef)
                     and x.name == 'check')
        namespace = {'registry': CaseRegistry(self.manifest, self.table),
                     'connections': {}, 'observations': {'aster': {'unregistered': {}},
                                                         'reference': {'unregistered': {}}},
                     'equal': lambda *args: None, 'counts': Counter()}
        exec(compile(ast.Module(body=[check], type_ignores=[]), '<actual fixture gate>', 'exec'), namespace)
        with self.assertRaises(ValueError):
            namespace['check']('unregistered', 'value', (self.manifest['cases'][0]['input'], None))

    def test_duplicate_id(self):
        self.manifest['cases'].append(copy.deepcopy(self.manifest['cases'][0]))
        with self.assertRaises(ValueError):
            validate_cases(self.manifest)

    def test_unknown_capability(self):
        self.manifest['cases'][0]['capability_id'] = 'not.registered'
        with self.assertRaises(ValueError):
            validate_cases(self.manifest)

    def test_missing_registration(self):
        registry = CaseRegistry(self.manifest, self.table)
        with self.assertRaises(ValueError):
            registry.claim('unregistered', 'value', self.manifest['cases'][0]['input'])

    def test_sql_and_phase_and_parameter_mismatch(self):
        case = self.manifest['cases'][0]
        for key, wrong in [('sql', 'SELECT 2::int4'), ('phase', 'execute'),
                           ('arguments', [None]), ('parameter_oids', [0]),
                           ('parameter_formats', [1]), ('result_format', 1)]:
            with self.subTest(key=key):
                actual = copy.deepcopy(case['input'])
                actual[key] = wrong
                registry = CaseRegistry(self.manifest, self.table)
                with self.assertRaises(ValueError):
                    registry.claim(case['case_id'], case['family'], actual)

    def test_null_is_not_uncollected(self):
        case = next(x for x in self.manifest['cases'] if x['case_id'] == 'zero_run_null')
        self.assertEqual(case['input']['arguments'], [None])
        for replacement in [None, [], ['NULL'], [{'uncollected': True}]]:
            registry = CaseRegistry(self.manifest, self.table)
            for preceding in self.manifest['cases']:
                if preceding['case_id'] == case['case_id']:
                    break
                registry.claim(preceding['case_id'], preceding['family'], preceding['input'])
            actual = copy.deepcopy(case['input'])
            actual['arguments'] = replacement
            with self.assertRaises(ValueError):
                registry.claim(case['case_id'], case['family'], actual)

    def test_missing_input_field(self):
        del self.manifest['cases'][0]['input']['arguments']
        with self.assertRaises(ValueError):
            validate_cases(self.manifest)

    def test_repeated_execution(self):
        case = self.manifest['cases'][0]
        registry = CaseRegistry(self.manifest, self.table)
        registry.claim(case['case_id'], case['family'], case['input'])
        with self.assertRaises(ValueError):
            registry.claim(case['case_id'], case['family'], case['input'])

    def test_missing_execution(self):
        with self.assertRaises(ValueError):
            CaseRegistry(self.manifest, self.table).finish()

    def test_order_and_family(self):
        first, second = self.manifest['cases'][:2]
        for case, family in [(second, second['family']), (first, 'wrong')]:
            with self.assertRaises(ValueError):
                CaseRegistry(self.manifest, self.table).claim(case['case_id'], family, case['input'])

    def test_unsafe_table_binding(self):
        for table in ['public.other_table', 'public.pg_core_diff_1; DROP TABLE users', 'pg_core_diff_1']:
            with self.assertRaises(ValueError):
                CaseRegistry(self.manifest, table)

    def test_undeclared_binding(self):
        self.manifest['cases'][0]['input']['sql'] = 'SELECT {other_binding}'
        with self.assertRaises(ValueError):
            validate_cases(self.manifest)


class ObservationArchiveTests(unittest.TestCase):
    def setUp(self):
        import tempfile
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)

    def gate(self, fail_reference=False, wrong_expected=False, attempt=1, fail_observe=False):
        # Execute the real consumption gate with controlled fixture-owned failure.
        from pg_core_evidence import append_observation
        tree = ast.parse(Path(__file__).with_name('pg_core_libpq.py').read_text())
        differential = next(x for x in tree.body if isinstance(x, ast.FunctionDef)
                            and x.name == 'integer_differential')
        check = next(x for x in differential.body if isinstance(x, ast.FunctionDef)
                     and x.name == 'check')
        case = load_cases()['cases'][0]
        def send(side):
            if fail_reference and side == 'reference':
                raise RuntimeError('controlled collection failure password=secret')
            return {'status': 2, 'state': '', 'rows': [[side]], 'params': []}
        def observe(result, partial):
            if fail_observe:
                partial['status'] = result['status']
                raise RuntimeError('controlled extraction failure')
            return result
        namespace = dict(json=json, registry=CaseRegistry(load_cases(), 'public.pg_core_diff_123'),
                         connections={'aster': 'aster', 'reference': 'reference'},
                         observations={'aster': {}, 'reference': {}}, observe=observe,
                         equal=lambda *args: None, counts=Counter(),
                         append_observation=append_observation, evidence_dir=self.directory,
                         run_id='run', attempt=attempt)
        exec(compile(ast.Module(body=[check], type_ignores=[]), '<actual archive gate>', 'exec'), namespace)
        namespace['check'](case['case_id'], case['family'], (case['input'], send),
                           expected_rows=[['wrong']] if wrong_expected else None)

    def records(self):
        from pg_core_evidence import read_observations
        return read_observations(self.directory)

    def test_both_sides_are_independent_before_comparison(self):
        self.gate()
        self.assertEqual([r['observation']['rows'] for r in self.records()],
                         [[['aster']], [['reference']]])

    def test_partial_failure_retains_collected_side(self):
        with self.assertRaises(RuntimeError):
            self.gate(fail_reference=True)
        records = self.records()
        self.assertEqual(len(records), 2)
        self.assertEqual(records[0]['observation']['rows'], [['aster']])
        self.assertEqual(records[1]['outcome'], 'collection_failed')
        self.assertIsNone(records[1]['observation'])
        self.assertNotIn('secret', (self.directory / 'reference.jsonl').read_text())

    def test_assertion_failure_retains_observation(self):
        with self.assertRaises(AssertionError):
            self.gate(wrong_expected=True)
        self.assertTrue(self.records(), 'assertion must retain the collected side')
        self.assertEqual(self.records()[0]['observation']['rows'], [['aster']])

    def test_extraction_failure_keeps_partial_result(self):
        with self.assertRaises(RuntimeError):
            self.gate(fail_observe=True)
        self.assertEqual(self.records()[0]['observation'], {'status': 2})
        self.assertEqual(self.records()[0]['outcome'], 'collection_failed')

    def test_empty_result_and_uncollected_remain_distinct(self):
        from pg_core_evidence import append_observation
        for i, observation in enumerate([{'rows': [], 'fields': [['n', 23, 4, -1]]}, None]):
            append_observation(self.directory, dict(run='run', case_id='empty', side='aster',
                               attempt=i, observation=observation))
        self.assertEqual(self.records()[0]['observation']['rows'], [])
        self.assertIsNone(self.records()[1]['observation'])

    def test_retry_preserves_prior_records(self):
        self.gate(attempt=1)
        self.gate(attempt=2)
        self.assertEqual(len(self.records()), 4)
        self.assertEqual({r['attempt'] for r in self.records()}, {1, 2})

    def test_cleanup_failure_preserves_business_failure(self):
        from pg_core_evidence import append_observation
        tree = ast.parse(Path(__file__).with_name('pg_core_libpq.py').read_text())
        differential = next(x for x in tree.body if isinstance(x, ast.FunctionDef)
                            and x.name == 'integer_differential')
        cleanup = next(x for x in differential.body if isinstance(x, ast.Try)).finalbody
        def execute(*args):
            raise ValueError('controlled cleanup failure')
        def archive(case, side, actual_input, observation, outcome, error_type=None):
            append_observation(self.directory, dict(run='run', case_id=case, side=side,
                               attempt=1, observation=observation, outcome=outcome,
                               error_type=error_type))
        import sys
        namespace = dict(sys=sys, created={'aster'}, connections={'aster': object()},
                         status=lambda conn: 0, execute=execute, observe=lambda x: x,
                         finish=lambda conn: None, archive=archive)
        body = ast.parse("raise RuntimeError('business failure')").body
        code = ast.fix_missing_locations(ast.Module(body=[ast.Try(
            body=body, handlers=[], orelse=[], finalbody=cleanup)], type_ignores=[]))
        with self.assertRaisesRegex(RuntimeError, 'business failure'):
            exec(compile(code, '<actual cleanup gate>', 'exec'), namespace)
        self.assertEqual(self.records()[0]['outcome'], 'cleanup_failed')

    def test_dsn_rejected_without_leak(self):
        from pg_core_evidence import append_observation
        with self.assertRaises(ValueError) as caught:
            append_observation(self.directory, dict(run='run', case_id='x', side='aster',
                               attempt=1, dsn='password=secret'))
        self.assertNotIn('secret', str(caught.exception))
        self.assertEqual(self.records(), [])


if __name__ == '__main__':
    unittest.main()
