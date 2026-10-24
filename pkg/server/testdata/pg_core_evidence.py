# Copyright 2026 AsterSQL.
"""Input registration for the existing first-phase libpq differential fixture.

Importing this module never loads libpq, reads argv, or connects to a database.
None inside arguments is SQL NULL; absent/uncollected input is rejected.
"""
import copy
import json
from pathlib import Path
import re


_INPUT_FIELDS = {'phase', 'sql', 'statement', 'parameter_oids', 'arguments',
                 'parameter_formats', 'result_format'}


def load_cases(path=None):
    path = Path(path) if path is not None else Path(__file__).with_name('pg_core_cases.json')
    return validate_cases(json.loads(path.read_text()))


def validate_cases(cases):
    if not isinstance(cases, dict) or cases.get('schema_version') != 1:
        raise ValueError('unsupported case registry schema')
    capabilities = cases.get('capabilities')
    entries = cases.get('cases')
    if not isinstance(capabilities, dict) or not capabilities or not isinstance(entries, list) or not entries:
        raise ValueError('capability definitions and cases are required')
    seen = {}
    for case in entries:
        case_id = case.get('case_id')
        if not isinstance(case_id, str) or not re.fullmatch(r'[a-z][a-z0-9_]*', case_id) or case_id in seen:
            raise ValueError('invalid or duplicate case_id')
        if case.get('capability_id') not in capabilities:
            raise ValueError(f'{case_id}: undefined capability')
        for key in ['family', 'initial_state', 'recovery', 'cleanup', 'expected_basis', 'comparison_fields']:
            if not case.get(key):
                raise ValueError(f'{case_id}: missing {key}')
        if set(case.get('dialects', {})) != {'postgresql', 'astersql'} or not all(case['dialects'].values()):
            raise ValueError(f'{case_id}: dialect applicability reasons required')
        if set(case.get('bindings', {})) != {'run_table'}:
            raise ValueError(f'{case_id}: run_table binding declaration required')
        data = case.get('input')
        if not isinstance(data, dict):
            raise ValueError(f'{case_id}: input required')
        if case.get('kind') == 'self_check':
            if set(data) != {'phase', 'source_case_id', 'mutation_field'} or data['phase'] != 'self_check':
                raise ValueError(f'{case_id}: invalid self-check input')
            source = seen.get(data['source_case_id'])
            if source is None or source['kind'] != 'business' or data['mutation_field'] not in ['rows', 'fields', 'state']:
                raise ValueError(f'{case_id}: invalid self-check source')
        elif case.get('kind') == 'business':
            if set(data) != _INPUT_FIELDS or data['phase'] not in ['query', 'prepare', 'describe', 'execute']:
                raise ValueError(f'{case_id}: incomplete input or invalid phase')
            if not isinstance(data['sql'], str) or not data['sql']:
                raise ValueError(f'{case_id}: SQL required')
            if '{' in data['sql'].replace('{run_table}', '') or '}' in data['sql'].replace('{run_table}', ''):
                raise ValueError(f'{case_id}: undeclared SQL binding')
            if data['phase'] == 'query':
                if data['statement'] is not None:
                    raise ValueError(f'{case_id}: query cannot name a prepared statement')
            elif not isinstance(data['statement'], str) or not data['statement']:
                raise ValueError(f'{case_id}: statement identity required')
            oids = data['parameter_oids']
            if oids is not None and (not isinstance(oids, list) or any(type(x) is not int or x < 0 for x in oids)):
                raise ValueError(f'{case_id}: invalid source OIDs')
            args = data['arguments']
            if data['phase'] == 'execute':
                if not isinstance(args, list) or any(x is not None and not isinstance(x, str) for x in args):
                    raise ValueError(f'{case_id}: collected arguments required; SQL NULL is a list element')
            elif args is not None:
                raise ValueError(f'{case_id}: unexpected bind arguments')
            if data['phase'] != 'prepare' and oids is not None:
                raise ValueError(f'{case_id}: source OIDs belong to prepare')
            if data['parameter_formats'] is not None or type(data['result_format']) is not int or data['result_format'] != 0:
                raise ValueError(f'{case_id}: first-phase fixture uses default text formats')
        else:
            raise ValueError(f'{case_id}: invalid kind')
        if not isinstance(case.get('expected'), dict):
            raise ValueError(f'{case_id}: expected basis required')
        seen[case_id] = case
    return copy.deepcopy(cases)


class CaseRegistry:
    """Claim the exact registered input before sending it to either endpoint."""

    def __init__(self, cases, run_table):
        self.cases = validate_cases(cases)
        if not isinstance(run_table, str) or not re.fullmatch(r'public\.pg_core_diff_[0-9]+', run_table):
            raise ValueError('unsafe run_table identifier')
        self.run_table = run_table
        self.seen = []

    def claim(self, case_id, family, actual):
        index = len(self.seen)
        if index >= len(self.cases['cases']):
            raise ValueError('execution exceeds registered cases')
        case = self.cases['cases'][index]
        if case_id != case['case_id'] or family != case['family']:
            raise ValueError(f'{case_id}: unregistered, repeated, out-of-order or wrong family')
        expected = copy.deepcopy(case['input'])
        if 'sql' in expected:
            expected['sql'] = expected['sql'].replace('{run_table}', self.run_table)
        if expected != actual:
            raise ValueError(f'{case_id}: actual input differs from registration')
        self.seen.append(case_id)
        return expected

    def finish(self):
        if len(self.seen) != len(self.cases['cases']):
            raise ValueError('registered cases were not executed')
        return len(self.seen)


def append_observation(directory, record):
    """Persist one endpoint before assertions; never accept connection material."""
    allowed = {'run', 'case_id', 'side', 'attempt', 'input', 'observation',
               'outcome', 'error_type'}
    if set(record) - allowed or not {'run', 'case_id', 'side', 'attempt'} <= set(record):
        raise ValueError('invalid observation envelope')
    if record['side'] not in {'aster', 'reference', 'mysql'}:
        raise ValueError('invalid observation side')
    data = record.get('input')
    if data is not None and set(data) - (_INPUT_FIELDS | {'command'}):
        raise ValueError('invalid observation input')
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / (record['side'] + '.jsonl')).open('a', encoding='utf-8') as stream:
        stream.write(json.dumps(record, sort_keys=True) + '\n')
        stream.flush()
        import os
        os.fsync(stream.fileno())


def read_observations(directory):
    """Read separate endpoint streams without comparing or filling missing data."""
    return [json.loads(line) for path in sorted(Path(directory).glob('*.jsonl'))
            for line in path.read_text().splitlines()]
