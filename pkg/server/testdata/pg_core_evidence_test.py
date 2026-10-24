# Copyright 2026 AsterSQL.
"""Independent standard-library tests for first-phase input registration."""
import ast
import copy
import json
from collections import Counter
from pathlib import Path
import unittest

from pg_core_evidence import CaseRegistry, load_cases, validate_cases, compare_scalar_output, compare_rows, compare_metadata, compare_rejection, validate_provenance


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
                         'scalar_null', 'scalar_empty', 'scalar_literal_null', 'scalar_zero_rows',
                         'multiset_ordered', 'multiset_unordered',
                         'invalid_write_prepare', 'invalid_write_describe', 'invalid_int4_write', 'invalid_int4_write_recovery', 'begin', 'transaction_error', 'transaction_failed', 'rollback', 'rollback_recovery'])
        business = [x['case_id'] for x in self.manifest['cases'] if x['kind'] == 'business']
        checks = [x['case_id'] for x in self.manifest['cases'] if x['kind'] == 'self_check']
        self.assertEqual(set(business), expected)
        self.assertEqual(len(business), 101)
        self.assertEqual(set(checks), {'negative_self_check_' + key for key in ['rows', 'fields', 'state']})
        registry = CaseRegistry(self.manifest, self.table)
        for case in self.manifest['cases']:
            actual = copy.deepcopy(case['input'])
            if 'sql' in actual:
                actual['sql'] = actual['sql'].replace('{run_table}', self.table)
            self.assertEqual(registry.claim(case['case_id'], case['family'], actual), actual)
        self.assertEqual(registry.finish(), 104)

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
            return {'status': 2, 'state': '', 'rows': [[side]], 'params': [], 'fields': []}
        def observe(result, partial):
            if fail_observe:
                partial['status'] = result['status']
                raise RuntimeError('controlled extraction failure')
            return result
        namespace = dict(json=json, registry=CaseRegistry(load_cases(), 'public.pg_core_diff_123'),
                         connections={'aster': 'aster', 'reference': 'reference'},
                         observations={'aster': {}, 'reference': {}}, observe=observe,
                         equal=lambda *args: None, counts=Counter(),
                         provenance={key: 'archive-only-controlled-test' for key in
                             ['client', 'os', 'storage', 'authentication', 'endpoints',
                              'target_version', 'source', 'reference_build']},
                         append_observation=append_observation, evidence_dir=self.directory,
                         run_id='run', attempt=attempt, attempts=[], save_attempts=lambda: None)
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
                         finish=lambda conn: None, archive=archive, attempts=[], save_attempts=lambda: None,
                         completed=False, run_id='run', registry=CaseRegistry(load_cases(), 'public.pg_core_diff_123'),
                         summarize_run=__import__('pg_core_evidence').summarize_run,
                         evidence_dir=self.directory, json=json)
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


class ScalarComparisonTests(unittest.TestCase):
    """The scalar contract distinguishes collected output from absent evidence."""

    def output(self, value):
        cell = ({'kind': 'sql_null', 'length': 0} if value is None else
                {'kind': 'value', 'length': len(value.encode()), 'hex': value.encode().hex()})
        return {'rows': [[value]], 'cells': [[cell]]}

    def compare(self, a, b):
        return compare_scalar_output(a, b, {'result_format': 0})

    def test_equal_values(self):
        for value in ['9007199254740993', '', 'NULL', None]:
            self.assertTrue(self.compare(self.output(value), self.output(value)))

    def test_six_outcomes_are_distinct(self):
        outcomes = [self.output('1'), self.output(None), self.output('NULL'),
                    self.output(''), {'rows': [], 'cells': []}, {'kind': 'uncollected'}]
        for i, a in enumerate(outcomes):
            for j, b in enumerate(outcomes):
                if i != j:
                    with self.subTest(i=i, j=j):
                        self.assertFalse(self.compare(a, b))

    def test_both_uncollected_fail(self):
        self.assertFalse(self.compare({'kind': 'uncollected'}, {'kind': 'uncollected'}))

    def test_both_not_applicable_fail(self):
        self.assertFalse(self.compare({'kind': 'not_applicable'}, {'kind': 'not_applicable'}))

    def test_both_missing_rows_fail(self):
        self.assertFalse(self.compare({'cells': []}, {'cells': []}))

    def test_both_missing_cells_fail(self):
        self.assertFalse(self.compare({'rows': []}, {'rows': []}))

    def test_unknown_cell_tag_fails(self):
        a = self.output('1')
        a['cells'][0][0]['kind'] = 'unknown'
        self.assertFalse(self.compare(a, copy.deepcopy(a)))

    def test_missing_cell_tag_fails(self):
        a = self.output('1')
        del a['cells'][0][0]['kind']
        self.assertFalse(self.compare(a, copy.deepcopy(a)))

    def test_inconsistent_null_fails(self):
        a = self.output(None)
        a['rows'] = [['NULL']]
        self.assertFalse(self.compare(a, copy.deepcopy(a)))

    def test_cell_shape_and_payload_fail(self):
        for key, wrong in [('cells', []), ('rows', [[1]])]:
            a = self.output('1')
            a[key] = wrong
            self.assertFalse(self.compare(a, copy.deepcopy(a)))
        for key, wrong in [('length', 7), ('hex', '00')]:
            a = self.output('1')
            a['cells'][0][0][key] = wrong
            self.assertFalse(self.compare(a, copy.deepcopy(a)))

    def test_zero_rows_are_collected(self):
        self.assertTrue(self.compare({'rows': [], 'cells': []}, {'rows': [], 'cells': []}))

    def test_order_and_precision_preserved(self):
        a = self.output('9007199254740993')
        self.assertFalse(self.compare(a, self.output('9007199254740992')))
        b = self.output('2')
        multi = {'rows': a['rows'] + b['rows'], 'cells': a['cells'] + b['cells']}
        reverse = {'rows': b['rows'] + a['rows'], 'cells': b['cells'] + a['cells']}
        self.assertFalse(self.compare(multi, reverse))
        before = json.dumps(multi, sort_keys=True)
        self.assertTrue(self.compare(multi, copy.deepcopy(multi)))
        self.assertEqual(json.dumps(multi, sort_keys=True), before)

    def test_unknown_contract_fails(self):
        a = self.output('1')
        self.assertFalse(compare_scalar_output(a, a, {'result_format': 1}))

    def test_unknown_scalar_field_fails(self):
        a = self.output('1')
        a['cells'][0][0]['unknown'] = True
        self.assertFalse(self.compare(a, copy.deepcopy(a)))
        a = self.output('1')
        a['kind'] = 'uncollected'
        self.assertFalse(self.compare(a, copy.deepcopy(a)))

    def test_real_equal_rejects_both_uncollected(self):
        tree = ast.parse(Path(__file__).with_name('pg_core_libpq.py').read_text())
        differential = next(x for x in tree.body if isinstance(x, ast.FunctionDef)
                            and x.name == 'integer_differential')
        equal = next(x for x in differential.body if isinstance(x, ast.FunctionDef)
                     and x.name == 'equal')
        namespace = {'compare_rows': compare_rows}
        exec(compile(ast.Module(body=[equal], type_ignores=[]), '<actual scalar gate>', 'exec'), namespace)
        a = {'status': 2, 'state': '', 'fields': [], 'params': [],
             'rows': {'kind': 'uncollected'}, 'cells': {'kind': 'uncollected'}}
        with self.assertRaises(AssertionError):
            namespace['equal']('both_uncollected', a, copy.deepcopy(a))


class MultisetComparisonTests(unittest.TestCase):
    def output(self, rows):
        return {'rows': rows, 'cells': [[
            {'kind': 'sql_null', 'length': 0} if v is None else
            {'kind': 'value', 'length': len(v.encode()), 'hex': v.encode().hex()}
            for v in row] for row in rows]}

    def test_permutations_and_raw_preservation(self):
        a = self.output([['1', None], ['2', ''], ['1', None]])
        b = self.output([['2', ''], ['1', None], ['1', None]])
        before = copy.deepcopy((a, b))
        self.assertTrue(compare_rows(a, b, 'multiset'))
        self.assertEqual((a, b), before)

    def test_missing_duplicate(self):
        self.assertFalse(compare_rows(self.output([['1'], ['1']]), self.output([['1']]), 'multiset'))

    def test_extra_duplicate(self):
        self.assertFalse(compare_rows(self.output([['1']]), self.output([['1'], ['1']]), 'multiset'))

    def test_null_empty_and_literal_distinct(self):
        for value in ['', 'NULL']:
            self.assertFalse(compare_rows(self.output([[None]]), self.output([[value]]), 'multiset'))

    def test_column_order(self):
        self.assertFalse(compare_rows(self.output([['1', '2']]), self.output([['2', '1']]), 'multiset'))

    def test_strict_order(self):
        self.assertFalse(compare_rows(self.output([['1'], ['2']]), self.output([['2'], ['1']]), 'strict'))

    def test_invalid_type_and_contract(self):
        a = self.output([['1']])
        b = copy.deepcopy(a)
        b['rows'] = [[1]]
        self.assertFalse(compare_rows(b, b, 'multiset'))
        self.assertFalse(compare_rows(a, a, 'unknown'))

    def test_inconsistent_cells(self):
        a = self.output([['1']])
        a['cells'][0][0]['hex'] = '32'
        self.assertFalse(compare_rows(a, a, 'multiset'))


class MetadataComparisonTests(unittest.TestCase):
    def setUp(self):
        # Use only a deep copy of an actual retained libpq observation.
        # task4 inferred_describe, PostgreSQL 18.4/libpq 180000.
        self.sample = {
            'status': 1, 'state': '', 'rows': [], 'cells': [],
            'fields': [['b', 23, 4, -1], ['a', 20, 8, -1], ['c', 23, 4, -1]],
            'formats': [0, 0, 0], 'params': [20, 23], 'param_count': 2,
            'command_tag': '', 'affected_rows': '', 'table_origins': None,
        }

    def test_complete_description(self):
        self.assertTrue(compare_metadata(self.sample, copy.deepcopy(self.sample), 'describe'))

    def mutate(self, key, value):
        changed = copy.deepcopy(self.sample)
        changed[key] = value
        self.assertFalse(compare_metadata(changed, self.sample, 'describe'))

    def test_column_name(self):
        fields = copy.deepcopy(self.sample['fields'])
        fields[0][0] = 'wrong'
        self.mutate('fields', fields)

    def test_column_oid(self):
        fields = copy.deepcopy(self.sample['fields'])
        fields[0][1] = 20
        self.mutate('fields', fields)

    def test_typlen(self):
        fields = copy.deepcopy(self.sample['fields'])
        fields[0][2] = 8
        self.mutate('fields', fields)

    def test_typmod(self):
        fields = copy.deepcopy(self.sample['fields'])
        fields[0][3] = 4
        self.mutate('fields', fields)

    def test_parameter_oid(self):
        self.mutate('params', [23, 23])

    def test_parameter_count(self):
        self.mutate('param_count', 1)

    def test_format(self):
        self.mutate('formats', [1, 0, 0])

    def test_both_missing_required_fields(self):
        for key in ['fields', 'params', 'formats', 'param_count']:
            with self.subTest(key=key):
                changed = copy.deepcopy(self.sample)
                del changed[key]
                self.assertFalse(compare_metadata(changed, copy.deepcopy(changed), 'describe'))

    def test_both_malformed_descriptions(self):
        for key, value in [('fields', [['b', 23]]), ('params', [None]),
                           ('param_count', 3), ('formats', []), ('formats', [2, 0, 0])]:
            with self.subTest(key=key):
                changed = copy.deepcopy(self.sample)
                changed[key] = value
                self.assertFalse(compare_metadata(changed, copy.deepcopy(changed), 'describe'))

    def test_unknown_phase(self):
        self.assertFalse(compare_metadata(self.sample, self.sample, 'unknown'))

    def test_real_equal_rejects_both_missing_metadata(self):
        tree = ast.parse(Path(__file__).with_name('pg_core_libpq.py').read_text())
        differential = next(x for x in tree.body if isinstance(x, ast.FunctionDef)
                            and x.name == 'integer_differential')
        equal = next(x for x in differential.body if isinstance(x, ast.FunctionDef)
                     and x.name == 'equal')
        namespace = {'compare_rows': compare_rows, 'compare_metadata': compare_metadata}
        exec(compile(ast.Module(body=[equal], type_ignores=[]), '<actual metadata gate>', 'exec'), namespace)
        for key in ['fields', 'params', 'formats', 'param_count']:
            with self.subTest(key=key):
                changed = copy.deepcopy(self.sample)
                del changed[key]
                with self.assertRaises(AssertionError):
                    namespace['equal']('missing_' + key, changed, copy.deepcopy(changed), phase='describe')

    def test_raw_unchanged(self):
        before = copy.deepcopy(self.sample)
        self.assertTrue(compare_metadata(self.sample, self.sample, 'describe'))
        self.assertEqual(self.sample, before)


if __name__ == '__main__':
    unittest.main()


class RejectionComparisonTests(unittest.TestCase):
    """Comparator fixtures describe evidence; only the real entry proves client success."""

    def setUp(self):
        self.contract = {'classification': 'expected_rejection', 'phase': 'PQexec',
                         'sqlstate': '22P02', 'transaction_after_error': 'E'}
        # Deep copy of task6 real AsterSQL transaction_error_rejection_contract;
        # PostgreSQL18.4/libpq180000. No connection is made by this test.
        snapshot = {'affected_rows': '6',
         'cells': [[{'hex': '31', 'kind': 'value', 'length': 1},
                    {'hex': '39303037313939323534373430393933', 'kind': 'value', 'length': 16},
                    {'kind': 'sql_null', 'length': 0}],
                   [{'hex': '32', 'kind': 'value', 'length': 1},
                    {'kind': 'sql_null', 'length': 0},
                    {'kind': 'sql_null', 'length': 0}],
                   [{'hex': '33', 'kind': 'value', 'length': 1},
                    {'hex': '30', 'kind': 'value', 'length': 1},
                    {'hex': '', 'kind': 'value', 'length': 0}],
                   [{'hex': '34', 'kind': 'value', 'length': 1},
                    {'hex': '30', 'kind': 'value', 'length': 1},
                    {'hex': '4e554c4c', 'kind': 'value', 'length': 4}],
                   [{'hex': '35', 'kind': 'value', 'length': 1},
                    {'hex': '30', 'kind': 'value', 'length': 1},
                    {'hex': '', 'kind': 'value', 'length': 0}],
                   [{'hex': '36', 'kind': 'value', 'length': 1},
                    {'kind': 'sql_null', 'length': 0},
                    {'kind': 'sql_null', 'length': 0}]],
         'command_tag': 'SELECT 6',
         'fields': [['id', 23, 4, -1], ['n', 20, 8, -1], ['note', 25, -1, -1]],
         'formats': [0, 0, 0],
         'param_count': 0,
         'params': [],
         'rows': [['1', '9007199254740993', None],
                  ['2', None, None],
                  ['3', '0', ''],
                  ['4', '0', 'NULL'],
                  ['5', '0', ''],
                  ['6', None, None]],
         'state': '',
         'status': 2,
         'table_origins': None}
        self.sample = {'backend_phase': None,
         'classification': 'expected_rejection',
         'cleanup': {'connection_closed': True, 'drop': True, 'rollback': True},
         'connection_ok': True,
         'error': {'affected_rows': '',
                   'cells': [],
                   'command_tag': '',
                   'fields': [],
                   'formats': [],
                   'param_count': 0,
                   'params': [],
                   'rows': [],
                   'state': '22P02',
                   'status': 7,
                   'table_origins': None},
         'phase': 'PQexec',
         'recovery': {'affected_rows': '1',
                      'cells': [[{'hex': '31', 'kind': 'value', 'length': 1}]],
                      'command_tag': 'SELECT 1',
                      'fields': [['n', 23, 4, -1]],
                      'formats': [0],
                      'param_count': 0,
                      'params': [],
                      'rows': [['1']],
                      'state': '',
                      'status': 2,
                      'table_origins': None},
         'transaction_after_error': 'E',
         'transaction_after_recovery': 'I'}
        self.sample.update(before=copy.deepcopy(snapshot), after=copy.deepcopy(snapshot))

    def reject(self, key, value):
        changed = copy.deepcopy(self.sample)
        changed[key] = value
        self.assertFalse(compare_rejection(changed, self.sample, self.contract))
        self.assertFalse(compare_rejection(changed, changed, self.contract))

    def test_complete(self):
        self.assertTrue(compare_rejection(self.sample, copy.deepcopy(self.sample), self.contract))

    def test_wrong_sqlstate(self):
        self.reject('error', {'status': 7, 'state': '22003'})

    def test_missing_phase(self):
        self.reject('phase', None)

    def test_network_error(self):
        self.reject('connection_ok', False)

    def test_recovery_failed(self):
        self.reject('recovery', {'status': 7, 'state': '25P02', 'rows': []})

    def test_side_effect(self):
        after = copy.deepcopy(self.sample['after'])
        after['rows'].pop()
        after['cells'].pop()
        self.reject('after', after)

    def test_cleanup_failed(self):
        self.reject('cleanup', {'rollback': True, 'drop': False, 'connection_closed': True})

    def test_unimplemented_reclassified(self):
        self.reject('classification', 'unimplemented')
        contract = dict(self.contract, classification='required_success')
        self.assertFalse(compare_rejection(self.sample, self.sample, contract))

    def test_missing_evidence(self):
        for key in self.sample:
            changed = copy.deepcopy(self.sample)
            del changed[key]
            self.assertFalse(compare_rejection(changed, changed, self.contract), key)

    def test_failed_transaction_not_recovered_by_sync(self):
        self.reject('transaction_after_recovery', 'E')

    def test_prepare_failure_is_not_execute_rejection(self):
        self.reject('phase', 'PQprepare')

    def test_raw_records_unchanged(self):
        before = copy.deepcopy(self.sample)
        self.assertTrue(compare_rejection(self.sample, self.sample, self.contract))
        self.assertEqual(self.sample, before)

    def test_unknown_internal_phase_is_not_inferred(self):
        self.reject('backend_phase', 'parse')

    def test_manifest_cannot_reclassify_success(self):
        manifest = load_cases()
        case = next(x for x in manifest['cases'] if x['case_id'] == 'invalid_int4_write')
        case['expected']['status'] = 2
        with self.assertRaises(ValueError):
            validate_cases(manifest)

    def test_equal_sql_error_alone_is_insufficient(self):
        error = copy.deepcopy(self.sample['error'])
        self.assertTrue(compare_rows(error, error, 'strict'))
        self.assertTrue(compare_metadata(error, error))
        self.assertFalse(compare_rejection({'error': error}, {'error': error}, self.contract))


class ProvenanceTests(unittest.TestCase):
    def setUp(self):
        # Version/protocol strings retained from the real task6 libpq observation.
        # Digests are validator test inputs, not claims of a new database run.
        self.sample = {
            'run': 'provenance-unit', 'target_version': '18.4',
            'source': {'head': 'a' * 40, 'workspace_sha256': 'b' * 64},
            'build': {'path': '/test/server', 'sha256': 'c' * 64,
                      'profile': 'cargo-test-default-features'},
            'client': {'python': '3.12.11', 'library': '/test/libpq.dylib',
                       'sha256': 'd' * 64, 'PQlibVersion': 180000},
            'os': 'Darwin arm64', 'storage': 'CreateAnalyzeSession test storage',
            'authentication': 'InsecureRootOnly',
            'endpoints': {side: {'protocol': 30000, 'server_version': version,
                                'encoding': 'UTF8', 'timezone': 'UTC',
                                'search_path': 'public', 'transaction': 'I'}
                          for side, version in [('aster', '18.0 (AsterSQL)'),
                                                ('reference', '18.4 (Homebrew)')]},
            'reference': {'server_version_num': '180004',
                          'version': 'PostgreSQL 18.4 (Homebrew) on aarch64-apple-darwin25.4.0, compiled by Apple clang version 21.0.0 (clang-2100.0.123.102), 64-bit'},
            'reference_build': {'version': '18.4', 'server_version_num': 180004,
                                'source': 'Homebrew receipt', 'artifact_sha256': 'e' * 64,
                                'configuration': 'pg_config --configure output'}}
        self.expected = copy.deepcopy(self.sample)

    def reject(self, mutate):
        changed = copy.deepcopy(self.sample)
        mutate(changed)
        self.assertFalse(validate_provenance(changed, self.expected))

    def test_collected_exact_baseline(self):
        self.assertTrue(validate_provenance(self.sample, self.expected))

    def test_18_4_cannot_claim_18_6(self):
        self.sample['target_version'] = self.expected['target_version'] = '18.6'
        self.assertFalse(validate_provenance(self.sample, self.expected))

    def test_handshake_alone(self):
        self.reject(lambda x: x['reference'].pop('version'))

    def test_missing_build_source(self):
        self.reject(lambda x: x['reference_build'].pop('source'))

    def test_library_changed(self):
        self.reject(lambda x: x['client'].update(sha256='f' * 64))

    def test_workspace_changed(self):
        self.reject(lambda x: x['source'].update(workspace_sha256='f' * 64))

    def test_environment_missing(self):
        self.reject(lambda x: x['endpoints']['aster'].pop('search_path'))

    def test_build_version_mismatch(self):
        self.reject(lambda x: x['reference_build'].update(server_version_num=180006))

    def test_missing_build_file(self):
        self.reject(lambda x: x.update(reference_build=None))

    def test_binary_changed(self):
        self.reject(lambda x: x['build'].update(sha256='f' * 64))

    def test_raw_unchanged(self):
        before = copy.deepcopy(self.sample)
        self.assertTrue(validate_provenance(self.sample, self.expected))
        self.assertEqual(self.sample, before)

    def test_both_records_missing_evidence(self):
        for key in ['reference', 'reference_build', 'endpoints', 'client', 'source', 'build']:
            changed = copy.deepcopy(self.sample)
            changed[key] = None
            self.assertFalse(validate_provenance(changed, changed), key)
        for key in ['encoding', 'timezone', 'search_path', 'transaction', 'protocol']:
            changed = copy.deepcopy(self.sample)
            del changed['endpoints']['aster'][key]
            self.assertFalse(validate_provenance(changed, changed), key)

    def test_exact_18_6_structure(self):
        # Tests the schema only; this does not supply real PostgreSQL18.6 evidence.
        changed = copy.deepcopy(self.sample)
        changed['target_version'] = '18.6'
        changed['reference'] = {'version': 'PostgreSQL 18.6 (controlled build)',
                                'server_version_num': '180006'}
        changed['reference_build'].update(version='18.6', server_version_num=180006)
        changed['endpoints']['reference']['server_version'] = '18.6'
        self.assertTrue(validate_provenance(changed, changed))


class EffectiveCountTests(unittest.TestCase):
    def setUp(self):
        from pg_core_evidence import summarize_run
        self.summarize = summarize_run
        self.cases = [{'case_id': 'a', 'kind': 'business'},
                      {'case_id': 'self', 'kind': 'self_check'}]
        self.attempts = [dict(run='r', case_id='a', attempt=1, outcome='passed', assertions=7),
                         dict(run='r', case_id='self', attempt=1, outcome='passed', assertions=1)]

    def report(self, cleanup=True, completed=True):
        return self.summarize('r', self.cases, self.attempts, cleanup, completed)

    def test_zero_tests(self):
        self.cases, self.attempts = [], []
        self.assertFalse(self.report()['passed'])

    def test_double_alias(self):
        self.cases.append(dict(self.cases[0]))
        self.assertFalse(self.report()['passed'])

    def test_duplicate_retry(self):
        self.attempts.append(dict(self.attempts[0], attempt=2))
        report = self.report()
        self.assertEqual(report['unique_effective'], 1)
        self.assertEqual(report['retries'], [{'case_id': 'a', 'attempts': [1, 2]}])

    def test_self_checks_excluded(self):
        report = self.report()
        self.assertEqual(report['unique_effective'], 1)
        self.assertEqual(report['self_checks'], 1)
        self.assertEqual(report['assertions'], 7)

    def test_cleanup_failure(self):
        report = self.report(cleanup=False)
        self.assertFalse(report['passed'])
        self.assertEqual(report['unique_effective'], 0)
        self.assertEqual(report['failed'], 1)

    def test_incomplete_execution(self):
        self.attempts.pop(0)
        self.assertFalse(self.report()['passed'])
        self.assertEqual(self.report()['skipped'], 1)

    def test_health_cannot_cover_failure(self):
        self.attempts[0].update(outcome='failed')
        self.attempts.append(dict(self.attempts[0], attempt=2, outcome='passed'))
        report = self.report()
        self.assertFalse(report['passed'])
        self.assertEqual(report['unique_effective'], 0)
        self.assertEqual(report['failed'], 1)

    def test_late_assertion_failure(self):
        self.assertFalse(self.report(completed=False)['passed'])
        self.assertEqual(self.report(completed=False)['unique_effective'], 0)

    def test_missing_assertions(self):
        self.attempts[0]['assertions'] = 0
        self.assertFalse(self.report()['passed'])

    def test_other_run_does_not_cover(self):
        self.attempts[0]['run'] = 'old'
        self.assertFalse(self.report()['passed'])


    def test_failure_categories(self):
        for outcome, field in [('exception', 'exceptions'), ('timeout', 'timeouts'),
                               ('skipped', 'skipped'), ('ignored', 'ignored')]:
            self.attempts[0]['outcome'] = outcome
            report = self.report()
            self.assertFalse(report['passed'])
            self.assertEqual(report[field], 1)
            self.assertEqual(report['unique_effective'], 0)

    def test_self_checks_alone_do_not_pass(self):
        self.cases.pop(0)
        self.attempts.pop(0)
        self.assertFalse(self.report()['passed'])

    def test_duplicate_attempt_is_invalid(self):
        self.attempts.append(dict(self.attempts[0]))
        self.assertFalse(self.report()['passed'])


    def test_late_exception_and_timeout(self):
        for outcome, field in [('exception', 'exceptions'), ('timeout', 'timeouts')]:
            report = self.summarize('r', self.cases, self.attempts, True, False, outcome)
            self.assertFalse(report['passed'])
            self.assertEqual(report[field], 1)
            self.assertEqual(report['failed'], 0)

    def test_cleanup_failure_is_one_failure(self):
        report = self.report(cleanup=False, completed=False)
        self.assertEqual(report['failed'], 1)
        self.assertFalse(report['passed'])


class WhitelistTests(unittest.TestCase):
    def setUp(self):
        from pg_core_evidence import validate_whitelist, apply_whitelist
        self.validate = validate_whitelist
        self.apply = apply_whitelist
        self.context = dict(capability='integer', case_id='int_alias', sql='SELECT 1::int',
                            type_oids=[23], client='libpq18-sha', environment='test-env',
                            version='18.4', implementation='a' * 40, configuration='default')
        self.rule = dict(id='test-only-name', scope=copy.deepcopy(self.context),
                         field='fields.name', column=0, actual='aster', reference='postgres',
                         transform='replace_exact_name',
                         basis=dict(version='18.4', section='test section', link='https://example.org/spec',
                                    evidence='test-only evidence'), approval='test-only review',
                         review={k: self.context[k] for k in
                                 ['version', 'implementation', 'configuration', 'client']})
        self.actual = dict(status=2, state='', rows=[['1']],
                           cells=[[dict(kind='value', length=1, hex='31')]],
                           fields=[['aster', 23, 4, -1]], formats=[0], params=[], param_count=0)
        self.reference = copy.deepcopy(self.actual)
        self.reference['fields'][0][0] = 'postgres'

    def test_exact_rule_transforms_copy(self):
        before = copy.deepcopy(self.actual)
        a, b, used = self.apply([self.rule], self.context, self.actual, self.reference)
        self.assertEqual(a, b)
        self.assertEqual(used, ['test-only-name'])
        self.assertEqual(self.actual, before)

    def test_empty_official_list(self):
        self.assertEqual(json.loads(Path(__file__).with_name('pg_core_whitelist.json').read_text()), [])
        a, b, used = self.apply([], self.context, self.actual, self.reference)
        self.assertEqual(a, self.actual)
        self.assertEqual(used, [])
        self.assertNotEqual(a, b)

    def test_missing_reason(self):
        for key in ['basis', 'approval']:
            rule = copy.deepcopy(self.rule); rule[key] = None
            with self.assertRaises(ValueError): self.validate([rule])

    def test_wildcard_scope(self):
        for key in self.context:
            rule = copy.deepcopy(self.rule); rule['scope'][key] = '*'
            with self.assertRaises(ValueError): self.validate([rule])

    def test_unknown_field(self):
        for field in ['state', 'status', 'rows', 'fields.oid', 'recovery', 'cleanup', 'side_effects']:
            rule = copy.deepcopy(self.rule); rule['field'] = field
            with self.assertRaises(ValueError): self.validate([rule])

    def test_missing_review(self):
        for key in self.rule['review']:
            rule = copy.deepcopy(self.rule); del rule['review'][key]
            with self.assertRaises(ValueError): self.validate([rule])

    def test_stale_conditions_do_not_apply(self):
        for key in self.context:
            context = copy.deepcopy(self.context)
            context[key] = [20] if key == 'type_oids' else 'changed'
            a, b, used = self.apply([self.rule], context, self.actual, self.reference)
            self.assertNotEqual(a, b)
            self.assertEqual(used, [])

    def test_review_must_bind_scope(self):
        rule = copy.deepcopy(self.rule); rule['review']['version'] = '18.6'
        with self.assertRaises(ValueError): self.validate([rule])

    def test_missing_observation_cannot_be_filled(self):
        for key in self.actual:
            actual = copy.deepcopy(self.actual); del actual[key]
            with self.assertRaises(ValueError): self.apply([self.rule], self.context, actual, self.reference)

    def test_errors_cannot_be_waived(self):
        for key, value in [('status', 7), ('state', '22003')]:
            actual = copy.deepcopy(self.actual); actual[key] = value
            with self.assertRaises(ValueError): self.apply([self.rule], self.context, actual, self.reference)

    def test_unknown_rule_key_and_duplicate_id(self):
        rule = copy.deepcopy(self.rule); rule['ignore'] = True
        with self.assertRaises(ValueError): self.validate([rule])
        with self.assertRaises(ValueError): self.validate([self.rule, self.rule])

    def test_nonmatching_column_name_is_not_replaced(self):
        actual = copy.deepcopy(self.actual); actual['fields'][0][0] = 'other'
        a, b, used = self.apply([self.rule], self.context, actual, self.reference)
        self.assertNotEqual(a, b)
        self.assertEqual(used, [])


class CapabilityGateTests(unittest.TestCase):
    """Copies of archived real observations test the gate, not database success."""
    def setUp(self):
        self.data = json.loads(SAMPLE_CAPABILITY_EVIDENCE)

    def evaluate(self):
        from pg_core_evidence import evaluate_capabilities
        return evaluate_capabilities(**self.data)

    def supported(self):
        return self.evaluate()['capabilities']['integer.value']['status'] == 'supported'

    def test_complete_current(self):
        self.assertTrue(self.supported())
        self.assertEqual(self.evaluate()['capabilities']['mysql.bypass_health']['status'], 'unverified')

    def test_historical_stdout(self):
        self.data['summary'] = {'passed': True, 'unique_effective': 94}
        self.assertFalse(self.supported())

    def test_wrong_exact_version(self):
        self.data['provenance']['record']['target_version'] = '18.6'
        self.assertFalse(self.supported())

    def test_missing_endpoint(self):
        del self.data['observations']['reference']
        self.assertFalse(self.supported())

    def test_missing_metadata(self):
        del self.data['observations']['aster'][0]['observation']['fields']
        self.assertFalse(self.supported())

    def test_missing_recovery(self):
        for record in self.data['observations']['aster']:
            if record['case_id'].endswith('_rejection_contract'):
                del record['observation']['recovery']
        self.assertEqual(self.evaluate()['capabilities']['integer.transaction']['status'], 'unverified')

    def test_missing_cleanup(self):
        del self.data['observations']['rust_cleanup']['database_dropped']
        self.assertFalse(self.supported())

    def test_zero_business(self):
        self.data['summary']['unique_effective'] = 0
        self.assertFalse(self.supported())

    def test_wrong_capability_association(self):
        self.data['cases']['cases'][0]['capability_id'] = 'integer.invalid'
        self.assertFalse(self.supported())

    def test_old_run_observation(self):
        self.data['observations']['aster'][0]['run'] = 'historical'
        self.assertFalse(self.supported())

    def test_missing_sql_input(self):
        self.data['observations']['reference'][0]['input']['sql'] = 'SELECT 0'
        self.assertFalse(self.supported())

    def test_missing_client_provenance(self):
        del self.data['provenance']['record']['client']
        self.assertFalse(self.supported())

    def test_cleanup_failure_and_exception(self):
        self.data['summary']['exceptions'] = 1
        self.assertFalse(self.supported())

    def test_input_unchanged(self):
        before = copy.deepcopy(self.data)
        self.evaluate()
        self.assertEqual(before, self.data)


# Captured PostgreSQL18.4/libpq18 observations; this is comparator data only.
SAMPLE_CAPABILITY_EVIDENCE = r'''{
  "cases": {
    "schema_version": 1,
    "capabilities": {
      "integer.value": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "integer.typed_null": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "integer.overflow": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "integer.recovery": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "integer.invalid": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "integer.prepare": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "integer.describe": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "integer.reexecute": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "integer.write": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "integer.readback": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "integer.zero_rows": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "integer.transaction": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "integer.comparator_self_check": {
        "scope": "First-phase explicit int4/int8 inputs only."
      },
      "scalar.output": {
        "scope": "Task 3 bounded text readback: SQL NULL, empty string, literal NULL and zero rows only."
      }
    },
    "cases": [
      {
        "case_id": "int4_suffix_min",
        "kind": "business",
        "capability_id": "integer.value",
        "family": "value",
        "input": {
          "phase": "query",
          "sql": "SELECT '-2147483648'::int4 AS n",
          "statement": null,
          "parameter_oids": null,
          "arguments": null,
          "parameter_formats": null,
          "result_format": 0
        },
        "expected": {
          "status": 2,
          "state": "",
          "rows": [
            [
              "-2147483648"
            ]
          ]
        },
        "dialects": {
          "postgresql": "Reference integer casts, typed parameters and error/recovery contract.",
          "astersql": "Same explicit integer contract via PostgreSQL listener; MySQL health checked separately."
        },
        "bindings": {
          "run_table": "unique table owned by this run"
        },
        "initial_state": "Independent fresh AsterSQL/reference connections; execute preceding registered steps in order. Immediately before write, each endpoint executes CREATE TABLE {run_table} (id INT PRIMARY KEY, n BIGINT, note TEXT). Prepared statement identities and source OIDs come from preceding prepare steps.",
        "recovery": "Registered recovery/rollback steps; finally rollback and drop only run_table.",
        "cleanup": "On each endpoint: PQclear every result; if run_table was created, ROLLBACK then DROP TABLE {run_table}; finally PQfinish.",
        "expected_basis": "Original pg_core_libpq.py integer_differential at c84583fd558f6232fb400e19cfeb18aa0fe99a5c.",
        "comparison_fields": [
          "status",
          "state",
          "fields",
          "rows",
          "params",
          "cells"
        ]
      },
      {
        "case_id": "invalid_int4_write",
        "kind": "business",
        "capability_id": "integer.transaction",
        "family": "rejection_write",
        "input": {
          "phase": "execute",
          "sql": "INSERT INTO {run_table} VALUES ($1, 0, 'rejected')",
          "statement": "invalid_write",
          "parameter_oids": null,
          "arguments": [
            "12x"
          ],
          "parameter_formats": null,
          "result_format": 0
        },
        "expected": {
          "status": 7,
          "state": "22P02"
        },
        "dialects": {
          "postgresql": "Reference integer casts, typed parameters and error/recovery contract.",
          "astersql": "Same explicit integer contract via PostgreSQL listener; MySQL health checked separately."
        },
        "bindings": {
          "run_table": "unique table owned by this run"
        },
        "initial_state": "Independent fresh AsterSQL/reference connections; execute preceding registered steps in order. Immediately before write, each endpoint executes CREATE TABLE {run_table} (id INT PRIMARY KEY, n BIGINT, note TEXT). Prepared statement identities and source OIDs come from preceding prepare steps.",
        "recovery": "Registered recovery/rollback steps; finally rollback and drop only run_table.",
        "cleanup": "On each endpoint: PQclear every result; if run_table was created, ROLLBACK then DROP TABLE {run_table}; finally PQfinish.",
        "expected_basis": "Task6: invalid int4 write must preserve same-connection rows, recover and clean up.",
        "comparison_fields": [
          "status",
          "state",
          "fields",
          "rows",
          "params",
          "cells"
        ],
        "rejection_contract": {
          "classification": "expected_rejection",
          "phase": "PQexecPrepared",
          "sqlstate": "22P02",
          "transaction_after_error": "I"
        }
      }
    ],
    "capability_matrix": {
      "schema_version": 1,
      "capabilities": {
        "integer.value": {
          "classification": "postgresql_exclusive",
          "workflow": [
            "1",
            "8"
          ],
          "dependencies": [
            "current_provenance",
            "two_endpoints",
            "metadata",
            "cleanup"
          ],
          "acceptance": "pg_client_integration_test::pg_core_libpq_integer_differential",
          "scope": "First-phase explicit int4/int8 inputs only.",
          "case_ids": [
            "int4_suffix_min"
          ]
        },
        "mysql.bypass_health": {
          "classification": "shared",
          "workflow": [
            "1",
            "8"
          ],
          "dependencies": [
            "mysql_before_during_after",
            "cleanup"
          ],
          "acceptance": "pg_client_integration_test::pg_core_libpq_integer_differential",
          "scope": "Same server CAST AS SIGNED and table read health only; no MySQL reference equivalence.",
          "case_ids": []
        },
        "integer.transaction": {
          "classification": "postgresql_exclusive",
          "workflow": [
            "1",
            "8"
          ],
          "dependencies": [
            "current_provenance",
            "two_endpoints",
            "metadata",
            "cleanup"
          ],
          "acceptance": "pg_client_integration_test::pg_core_libpq_integer_differential",
          "scope": "First-phase explicit int4/int8 inputs only.",
          "case_ids": [
            "invalid_int4_write"
          ]
        }
      }
    }
  },
  "observations": {
    "aster": [
      {
        "attempt": 1,
        "case_id": "int4_suffix_min",
        "input": {
          "arguments": null,
          "parameter_formats": null,
          "parameter_oids": null,
          "phase": "query",
          "result_format": 0,
          "sql": "SELECT '-2147483648'::int4 AS n",
          "statement": null
        },
        "observation": {
          "affected_rows": "1",
          "cells": [
            [
              {
                "hex": "2d32313437343833363438",
                "kind": "value",
                "length": 11
              }
            ]
          ],
          "command_tag": "SELECT 1",
          "fields": [
            [
              "n",
              23,
              4,
              -1
            ]
          ],
          "formats": [
            0
          ],
          "param_count": 0,
          "params": [],
          "rows": [
            [
              "-2147483648"
            ]
          ],
          "state": "",
          "status": 2,
          "table_origins": null
        },
        "outcome": "collected",
        "run": "86401-1792850299470758000",
        "side": "aster"
      },
      {
        "attempt": 1,
        "case_id": "invalid_int4_write",
        "input": {
          "arguments": [
            "12x"
          ],
          "parameter_formats": null,
          "parameter_oids": null,
          "phase": "execute",
          "result_format": 0,
          "sql": "INSERT INTO public.pg_core_diff_86436 VALUES ($1, 0, 'rejected')",
          "statement": "invalid_write"
        },
        "observation": {
          "affected_rows": "",
          "cells": [],
          "command_tag": "",
          "fields": [],
          "formats": [],
          "param_count": 0,
          "params": [],
          "rows": [],
          "state": "22P02",
          "status": 7,
          "table_origins": null
        },
        "outcome": "collected",
        "run": "86401-1792850299470758000",
        "side": "aster"
      },
      {
        "attempt": 1,
        "case_id": "cleanup_rollback",
        "error_type": null,
        "input": {
          "command": "ROLLBACK"
        },
        "observation": {
          "affected_rows": "",
          "cells": [],
          "command_tag": "ROLLBACK",
          "fields": [],
          "formats": [],
          "param_count": 0,
          "params": [],
          "rows": [],
          "state": "",
          "status": 1,
          "table_origins": null
        },
        "outcome": "collected",
        "run": "86401-1792850299470758000",
        "side": "aster"
      },
      {
        "attempt": 1,
        "case_id": "cleanup_drop",
        "error_type": null,
        "input": {
          "command": "DROP TABLE public.pg_core_diff_86436"
        },
        "observation": {
          "affected_rows": "",
          "cells": [],
          "command_tag": "DROP TABLE",
          "fields": [],
          "formats": [],
          "param_count": 0,
          "params": [],
          "rows": [],
          "state": "",
          "status": 1,
          "table_origins": null
        },
        "outcome": "collected",
        "run": "86401-1792850299470758000",
        "side": "aster"
      },
      {
        "attempt": 1,
        "case_id": "connection_closed",
        "error_type": null,
        "input": null,
        "observation": null,
        "outcome": "closed",
        "run": "86401-1792850299470758000",
        "side": "aster"
      },
      {
        "attempt": 1,
        "case_id": "invalid_int4_write_rejection_contract",
        "error_type": null,
        "input": null,
        "observation": {
          "after": {
            "affected_rows": "6",
            "cells": [
              [
                {
                  "hex": "31",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "39303037313939323534373430393933",
                  "kind": "value",
                  "length": 16
                },
                {
                  "kind": "sql_null",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "32",
                  "kind": "value",
                  "length": 1
                },
                {
                  "kind": "sql_null",
                  "length": 0
                },
                {
                  "kind": "sql_null",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "33",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "30",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "",
                  "kind": "value",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "34",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "30",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "4e554c4c",
                  "kind": "value",
                  "length": 4
                }
              ],
              [
                {
                  "hex": "35",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "30",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "",
                  "kind": "value",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "36",
                  "kind": "value",
                  "length": 1
                },
                {
                  "kind": "sql_null",
                  "length": 0
                },
                {
                  "kind": "sql_null",
                  "length": 0
                }
              ]
            ],
            "command_tag": "SELECT 6",
            "fields": [
              [
                "id",
                23,
                4,
                -1
              ],
              [
                "n",
                20,
                8,
                -1
              ],
              [
                "note",
                25,
                -1,
                -1
              ]
            ],
            "formats": [
              0,
              0,
              0
            ],
            "param_count": 0,
            "params": [],
            "rows": [
              [
                "1",
                "9007199254740993",
                null
              ],
              [
                "2",
                null,
                null
              ],
              [
                "3",
                "0",
                ""
              ],
              [
                "4",
                "0",
                "NULL"
              ],
              [
                "5",
                "0",
                ""
              ],
              [
                "6",
                null,
                null
              ]
            ],
            "state": "",
            "status": 2,
            "table_origins": null
          },
          "backend_phase": null,
          "before": {
            "affected_rows": "6",
            "cells": [
              [
                {
                  "hex": "31",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "39303037313939323534373430393933",
                  "kind": "value",
                  "length": 16
                },
                {
                  "kind": "sql_null",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "32",
                  "kind": "value",
                  "length": 1
                },
                {
                  "kind": "sql_null",
                  "length": 0
                },
                {
                  "kind": "sql_null",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "33",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "30",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "",
                  "kind": "value",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "34",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "30",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "4e554c4c",
                  "kind": "value",
                  "length": 4
                }
              ],
              [
                {
                  "hex": "35",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "30",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "",
                  "kind": "value",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "36",
                  "kind": "value",
                  "length": 1
                },
                {
                  "kind": "sql_null",
                  "length": 0
                },
                {
                  "kind": "sql_null",
                  "length": 0
                }
              ]
            ],
            "command_tag": "SELECT 6",
            "fields": [
              [
                "id",
                23,
                4,
                -1
              ],
              [
                "n",
                20,
                8,
                -1
              ],
              [
                "note",
                25,
                -1,
                -1
              ]
            ],
            "formats": [
              0,
              0,
              0
            ],
            "param_count": 0,
            "params": [],
            "rows": [
              [
                "1",
                "9007199254740993",
                null
              ],
              [
                "2",
                null,
                null
              ],
              [
                "3",
                "0",
                ""
              ],
              [
                "4",
                "0",
                "NULL"
              ],
              [
                "5",
                "0",
                ""
              ],
              [
                "6",
                null,
                null
              ]
            ],
            "state": "",
            "status": 2,
            "table_origins": null
          },
          "classification": "expected_rejection",
          "cleanup": {
            "connection_closed": true,
            "drop": true,
            "rollback": true
          },
          "connection_ok": true,
          "error": {
            "affected_rows": "",
            "cells": [],
            "command_tag": "",
            "fields": [],
            "formats": [],
            "param_count": 0,
            "params": [],
            "rows": [],
            "state": "22P02",
            "status": 7,
            "table_origins": null
          },
          "phase": "PQexecPrepared",
          "recovery": {
            "affected_rows": "1",
            "cells": [
              [
                {
                  "hex": "31",
                  "kind": "value",
                  "length": 1
                }
              ]
            ],
            "command_tag": "SELECT 1",
            "fields": [
              [
                "n",
                23,
                4,
                -1
              ]
            ],
            "formats": [
              0
            ],
            "param_count": 0,
            "params": [],
            "rows": [
              [
                "1"
              ]
            ],
            "state": "",
            "status": 2,
            "table_origins": null
          },
          "transaction_after_error": "I",
          "transaction_after_recovery": "I"
        },
        "outcome": "collected",
        "run": "86401-1792850299470758000",
        "side": "aster"
      }
    ],
    "reference": [
      {
        "attempt": 1,
        "case_id": "int4_suffix_min",
        "input": {
          "arguments": null,
          "parameter_formats": null,
          "parameter_oids": null,
          "phase": "query",
          "result_format": 0,
          "sql": "SELECT '-2147483648'::int4 AS n",
          "statement": null
        },
        "observation": {
          "affected_rows": "1",
          "cells": [
            [
              {
                "hex": "2d32313437343833363438",
                "kind": "value",
                "length": 11
              }
            ]
          ],
          "command_tag": "SELECT 1",
          "fields": [
            [
              "n",
              23,
              4,
              -1
            ]
          ],
          "formats": [
            0
          ],
          "param_count": 0,
          "params": [],
          "rows": [
            [
              "-2147483648"
            ]
          ],
          "state": "",
          "status": 2,
          "table_origins": null
        },
        "outcome": "collected",
        "run": "86401-1792850299470758000",
        "side": "reference"
      },
      {
        "attempt": 1,
        "case_id": "invalid_int4_write",
        "input": {
          "arguments": [
            "12x"
          ],
          "parameter_formats": null,
          "parameter_oids": null,
          "phase": "execute",
          "result_format": 0,
          "sql": "INSERT INTO public.pg_core_diff_86436 VALUES ($1, 0, 'rejected')",
          "statement": "invalid_write"
        },
        "observation": {
          "affected_rows": "",
          "cells": [],
          "command_tag": "",
          "fields": [],
          "formats": [],
          "param_count": 0,
          "params": [],
          "rows": [],
          "state": "22P02",
          "status": 7,
          "table_origins": null
        },
        "outcome": "collected",
        "run": "86401-1792850299470758000",
        "side": "reference"
      },
      {
        "attempt": 1,
        "case_id": "cleanup_rollback",
        "error_type": null,
        "input": {
          "command": "ROLLBACK"
        },
        "observation": {
          "affected_rows": "",
          "cells": [],
          "command_tag": "ROLLBACK",
          "fields": [],
          "formats": [],
          "param_count": 0,
          "params": [],
          "rows": [],
          "state": "",
          "status": 1,
          "table_origins": null
        },
        "outcome": "collected",
        "run": "86401-1792850299470758000",
        "side": "reference"
      },
      {
        "attempt": 1,
        "case_id": "cleanup_drop",
        "error_type": null,
        "input": {
          "command": "DROP TABLE public.pg_core_diff_86436"
        },
        "observation": {
          "affected_rows": "",
          "cells": [],
          "command_tag": "DROP TABLE",
          "fields": [],
          "formats": [],
          "param_count": 0,
          "params": [],
          "rows": [],
          "state": "",
          "status": 1,
          "table_origins": null
        },
        "outcome": "collected",
        "run": "86401-1792850299470758000",
        "side": "reference"
      },
      {
        "attempt": 1,
        "case_id": "connection_closed",
        "error_type": null,
        "input": null,
        "observation": null,
        "outcome": "closed",
        "run": "86401-1792850299470758000",
        "side": "reference"
      },
      {
        "attempt": 1,
        "case_id": "invalid_int4_write_rejection_contract",
        "error_type": null,
        "input": null,
        "observation": {
          "after": {
            "affected_rows": "6",
            "cells": [
              [
                {
                  "hex": "31",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "39303037313939323534373430393933",
                  "kind": "value",
                  "length": 16
                },
                {
                  "kind": "sql_null",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "32",
                  "kind": "value",
                  "length": 1
                },
                {
                  "kind": "sql_null",
                  "length": 0
                },
                {
                  "kind": "sql_null",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "33",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "30",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "",
                  "kind": "value",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "34",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "30",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "4e554c4c",
                  "kind": "value",
                  "length": 4
                }
              ],
              [
                {
                  "hex": "35",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "30",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "",
                  "kind": "value",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "36",
                  "kind": "value",
                  "length": 1
                },
                {
                  "kind": "sql_null",
                  "length": 0
                },
                {
                  "kind": "sql_null",
                  "length": 0
                }
              ]
            ],
            "command_tag": "SELECT 6",
            "fields": [
              [
                "id",
                23,
                4,
                -1
              ],
              [
                "n",
                20,
                8,
                -1
              ],
              [
                "note",
                25,
                -1,
                -1
              ]
            ],
            "formats": [
              0,
              0,
              0
            ],
            "param_count": 0,
            "params": [],
            "rows": [
              [
                "1",
                "9007199254740993",
                null
              ],
              [
                "2",
                null,
                null
              ],
              [
                "3",
                "0",
                ""
              ],
              [
                "4",
                "0",
                "NULL"
              ],
              [
                "5",
                "0",
                ""
              ],
              [
                "6",
                null,
                null
              ]
            ],
            "state": "",
            "status": 2,
            "table_origins": null
          },
          "backend_phase": null,
          "before": {
            "affected_rows": "6",
            "cells": [
              [
                {
                  "hex": "31",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "39303037313939323534373430393933",
                  "kind": "value",
                  "length": 16
                },
                {
                  "kind": "sql_null",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "32",
                  "kind": "value",
                  "length": 1
                },
                {
                  "kind": "sql_null",
                  "length": 0
                },
                {
                  "kind": "sql_null",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "33",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "30",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "",
                  "kind": "value",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "34",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "30",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "4e554c4c",
                  "kind": "value",
                  "length": 4
                }
              ],
              [
                {
                  "hex": "35",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "30",
                  "kind": "value",
                  "length": 1
                },
                {
                  "hex": "",
                  "kind": "value",
                  "length": 0
                }
              ],
              [
                {
                  "hex": "36",
                  "kind": "value",
                  "length": 1
                },
                {
                  "kind": "sql_null",
                  "length": 0
                },
                {
                  "kind": "sql_null",
                  "length": 0
                }
              ]
            ],
            "command_tag": "SELECT 6",
            "fields": [
              [
                "id",
                23,
                4,
                -1
              ],
              [
                "n",
                20,
                8,
                -1
              ],
              [
                "note",
                25,
                -1,
                -1
              ]
            ],
            "formats": [
              0,
              0,
              0
            ],
            "param_count": 0,
            "params": [],
            "rows": [
              [
                "1",
                "9007199254740993",
                null
              ],
              [
                "2",
                null,
                null
              ],
              [
                "3",
                "0",
                ""
              ],
              [
                "4",
                "0",
                "NULL"
              ],
              [
                "5",
                "0",
                ""
              ],
              [
                "6",
                null,
                null
              ]
            ],
            "state": "",
            "status": 2,
            "table_origins": null
          },
          "classification": "expected_rejection",
          "cleanup": {
            "connection_closed": true,
            "drop": true,
            "rollback": true
          },
          "connection_ok": true,
          "error": {
            "affected_rows": "",
            "cells": [],
            "command_tag": "",
            "fields": [],
            "formats": [],
            "param_count": 0,
            "params": [],
            "rows": [],
            "state": "22P02",
            "status": 7,
            "table_origins": null
          },
          "phase": "PQexecPrepared",
          "recovery": {
            "affected_rows": "1",
            "cells": [
              [
                {
                  "hex": "31",
                  "kind": "value",
                  "length": 1
                }
              ]
            ],
            "command_tag": "SELECT 1",
            "fields": [
              [
                "n",
                23,
                4,
                -1
              ]
            ],
            "formats": [
              0
            ],
            "param_count": 0,
            "params": [],
            "rows": [
              [
                "1"
              ]
            ],
            "state": "",
            "status": 2,
            "table_origins": null
          },
          "transaction_after_error": "I",
          "transaction_after_recovery": "I"
        },
        "outcome": "collected",
        "run": "86401-1792850299470758000",
        "side": "reference"
      }
    ],
    "mysql": [
      {
        "run": "86401-1792850299470758000",
        "case_id": "before",
        "side": "mysql",
        "attempt": 1,
        "input": {
          "command": "SELECT id, CAST('-17' AS SIGNED) FROM public.client_isolation"
        },
        "observation": {
          "rows": [
            [
              [
                51,
                49
              ],
              [
                45,
                49,
                55
              ]
            ]
          ]
        },
        "outcome": "bypass_health"
      },
      {
        "run": "86401-1792850299470758000",
        "case_id": "during",
        "side": "mysql",
        "attempt": 1,
        "input": {
          "command": "SELECT id, CAST('-17' AS SIGNED) FROM public.client_isolation"
        },
        "observation": {
          "rows": [
            [
              [
                51,
                49
              ],
              [
                45,
                49,
                55
              ]
            ]
          ]
        },
        "outcome": "bypass_health"
      },
      {
        "run": "86401-1792850299470758000",
        "case_id": "after",
        "side": "mysql",
        "attempt": 1,
        "input": {
          "command": "SELECT id, CAST('-17' AS SIGNED) FROM public.client_isolation"
        },
        "observation": {
          "rows": [
            [
              [
                51,
                49
              ],
              [
                45,
                49,
                55
              ]
            ]
          ]
        },
        "outcome": "bypass_health"
      }
    ],
    "rust_cleanup": {
      "database_dropped": true,
      "postgres_port_closed": true,
      "mysql_port_closed": true
    },
    "child_exit": {
      "success": true,
      "code": 0,
      "status": "exit status: 0"
    },
    "attempts": [
      {
        "assertions": 9,
        "attempt": 1,
        "case_id": "int4_suffix_min",
        "outcome": "passed",
        "run": "86401-1792850299470758000"
      },
      {
        "assertions": 9,
        "attempt": 1,
        "case_id": "invalid_int4_write",
        "outcome": "passed",
        "run": "86401-1792850299470758000"
      }
    ]
  },
  "summary": {
    "run": "86401-1792850299470758000",
    "passed": true,
    "discovered": 2,
    "executed": 2,
    "attempts": 2,
    "unique_effective": 2,
    "failed": 0,
    "exceptions": 0,
    "timeouts": 0,
    "skipped": 0,
    "ignored": 0,
    "self_checks": 0,
    "assertions": 18,
    "assertion_scope": "completed business check contracts; excludes self checks and run guards",
    "cleanup_ok": true,
    "completed": true,
    "retries": []
  },
  "provenance": {
    "record": {
      "authentication": "InsecureRootOnly",
      "build": {
        "path": "/captured/rust-slot/astersql-server",
        "profile": "cargo-test-default-features",
        "sha256": "6e879802a6c832a9b76526548d6e1adc0e44a879ad51453647a1355fb2c005b4"
      },
      "client": {
        "PQlibVersion": 180000,
        "library": "/captured/libpq.5.dylib",
        "python": "3.12.11 (main, Jun  3 2025, 15:41:47) [Clang 17.0.0 (clang-1700.0.13.3)]",
        "sha256": "f5e2768d1403094d2cbcec6127e8290cb8f848a5ea08bfbb9a0268c386a205ab"
      },
      "endpoints": {
        "aster": {
          "encoding": "UTF8",
          "protocol": 30000,
          "search_path": "public",
          "server_version": "18.0 (AsterSQL)",
          "timezone": "UTC",
          "transaction": "I"
        },
        "reference": {
          "encoding": "UTF8",
          "protocol": 30000,
          "search_path": "\"$user\", public",
          "server_version": "18.4 (Homebrew)",
          "timezone": "Asia/Shanghai",
          "transaction": "I"
        }
      },
      "os": "macOS-26.1-arm64-arm-64bit",
      "reference": {
        "server_version_num": "180004",
        "version": "PostgreSQL 18.4 (Homebrew) on aarch64-apple-darwin25.4.0, compiled by Apple clang version 21.0.0 (clang-2100.0.123.102), 64-bit"
      },
      "reference_build": {
        "artifact_sha256": "9e120e78126eff196e43ac2c9485b7f0162c07d7902d04ee7add95bf073e10f4",
        "configuration": "'--datadir=/opt/homebrew/share/postgresql@18' '--includedir=/opt/homebrew/include/postgresql@18' '--sysconfdir=/opt/homebrew/etc' '--docdir=/opt/homebrew/Cellar/postgresql@18/18.4/share/doc/postgresql@18' '--enable-nls' '--enable-thread-safety' '--with-gssapi' '--with-icu' '--with-ldap' '--with-libxml' '--with-libxslt' '--with-lz4' '--with-zstd' '--with-openssl' '--with-libcurl' '--with-pam' '--with-perl' '--with-uuid=e2fs' '--with-extra-version= (Homebrew)' '--with-bonjour' '--with-tcl' 'PG_SYSROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.sdk' '--disable-debug' '--disable-dependency-tracking' '--prefix=/opt/homebrew/Cellar/postgresql@18/18.4' '--libdir=/opt/homebrew/lib/postgresql@18' 'CC=clang' 'LDFLAGS=-L/opt/homebrew/opt/gettext/lib -L/opt/homebrew/opt/krb5/lib -L/opt/homebrew/opt/openssl@3/lib -L/opt/homebrew/opt/readline/lib' 'CPPFLAGS=-I/opt/homebrew/opt/gettext/include -I/opt/homebrew/opt/krb5/include -I/opt/homebrew/opt/openssl@3/include -I/opt/homebrew/opt/readline/include' 'CXX=clang++' 'PKG_CONFIG_PATH=/opt/homebrew/opt/pkgconf/lib/pkgconfig:/opt/homebrew/opt/icu4c@78/lib/pkgconfig:/opt/homebrew/opt/openssl@3/lib/pkgconfig:/opt/homebrew/opt/krb5/lib/pkgconfig:/opt/homebrew/opt/lz4/lib/pkgconfig:/opt/homebrew/opt/readline/lib/pkgconfig:/opt/homebrew/opt/xz/lib/pkgconfig:/opt/homebrew/opt/zstd/lib/pkgconfig'",
        "server_version_num": 180004,
        "source": "Homebrew INSTALL_RECEIPT.json SHA256=79c8ef9459cdc04bb1ded05efb03ae015fd6f7150de250c18e03ce32b7140aa1",
        "version": "18.4"
      },
      "run": "86401-1792850299470758000",
      "source": {
        "head": "e37c137c4dbc6f9d77066798f794503743d65701",
        "workspace_sha256": "5ea3581f213322cdd71df0abddd9c92d7c513cf11c8b731454eefa1db0f1a813"
      },
      "storage": "CreateAnalyzeSession test storage",
      "target_version": "18.4"
    },
    "expected": {
      "authentication": "InsecureRootOnly",
      "build": {
        "path": "/captured/rust-slot/astersql-server",
        "profile": "cargo-test-default-features",
        "sha256": "6e879802a6c832a9b76526548d6e1adc0e44a879ad51453647a1355fb2c005b4"
      },
      "client": {
        "PQlibVersion": 180000,
        "library": "/captured/libpq.5.dylib",
        "python": "3.12.11 (main, Jun  3 2025, 15:41:47) [Clang 17.0.0 (clang-1700.0.13.3)]",
        "sha256": "f5e2768d1403094d2cbcec6127e8290cb8f848a5ea08bfbb9a0268c386a205ab"
      },
      "endpoints": {
        "aster": {
          "encoding": "UTF8",
          "protocol": 30000,
          "search_path": "public",
          "server_version": "18.0 (AsterSQL)",
          "timezone": "UTC",
          "transaction": "I"
        },
        "reference": {
          "encoding": "UTF8",
          "protocol": 30000,
          "search_path": "\"$user\", public",
          "server_version": "18.4 (Homebrew)",
          "timezone": "Asia/Shanghai",
          "transaction": "I"
        }
      },
      "os": "macOS-26.1-arm64-arm-64bit",
      "reference": {
        "server_version_num": "180004",
        "version": "PostgreSQL 18.4 (Homebrew) on aarch64-apple-darwin25.4.0, compiled by Apple clang version 21.0.0 (clang-2100.0.123.102), 64-bit"
      },
      "reference_build": {
        "artifact_sha256": "9e120e78126eff196e43ac2c9485b7f0162c07d7902d04ee7add95bf073e10f4",
        "configuration": "'--datadir=/opt/homebrew/share/postgresql@18' '--includedir=/opt/homebrew/include/postgresql@18' '--sysconfdir=/opt/homebrew/etc' '--docdir=/opt/homebrew/Cellar/postgresql@18/18.4/share/doc/postgresql@18' '--enable-nls' '--enable-thread-safety' '--with-gssapi' '--with-icu' '--with-ldap' '--with-libxml' '--with-libxslt' '--with-lz4' '--with-zstd' '--with-openssl' '--with-libcurl' '--with-pam' '--with-perl' '--with-uuid=e2fs' '--with-extra-version= (Homebrew)' '--with-bonjour' '--with-tcl' 'PG_SYSROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.sdk' '--disable-debug' '--disable-dependency-tracking' '--prefix=/opt/homebrew/Cellar/postgresql@18/18.4' '--libdir=/opt/homebrew/lib/postgresql@18' 'CC=clang' 'LDFLAGS=-L/opt/homebrew/opt/gettext/lib -L/opt/homebrew/opt/krb5/lib -L/opt/homebrew/opt/openssl@3/lib -L/opt/homebrew/opt/readline/lib' 'CPPFLAGS=-I/opt/homebrew/opt/gettext/include -I/opt/homebrew/opt/krb5/include -I/opt/homebrew/opt/openssl@3/include -I/opt/homebrew/opt/readline/include' 'CXX=clang++' 'PKG_CONFIG_PATH=/opt/homebrew/opt/pkgconf/lib/pkgconfig:/opt/homebrew/opt/icu4c@78/lib/pkgconfig:/opt/homebrew/opt/openssl@3/lib/pkgconfig:/opt/homebrew/opt/krb5/lib/pkgconfig:/opt/homebrew/opt/lz4/lib/pkgconfig:/opt/homebrew/opt/readline/lib/pkgconfig:/opt/homebrew/opt/xz/lib/pkgconfig:/opt/homebrew/opt/zstd/lib/pkgconfig'",
        "server_version_num": 180004,
        "source": "Homebrew INSTALL_RECEIPT.json SHA256=79c8ef9459cdc04bb1ded05efb03ae015fd6f7150de250c18e03ce32b7140aa1",
        "version": "18.4"
      },
      "run": "86401-1792850299470758000",
      "source": {
        "head": "e37c137c4dbc6f9d77066798f794503743d65701",
        "workspace_sha256": "5ea3581f213322cdd71df0abddd9c92d7c513cf11c8b731454eefa1db0f1a813"
      },
      "storage": "CreateAnalyzeSession test storage",
      "target_version": "18.4"
    }
  }
}'''
