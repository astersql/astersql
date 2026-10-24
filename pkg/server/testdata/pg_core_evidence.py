# Copyright 2026 AsterSQL.
"""Input registration for the existing first-phase libpq differential fixture.

Importing this module never loads libpq, reads argv, or connects to a database.
None inside arguments is SQL NULL; absent/uncollected input is rejected.
"""
from collections import Counter
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
        if case.get('row_order', 'strict') not in {'strict', 'multiset'}:
            raise ValueError(f'{case_id}: invalid row_order')
        if not isinstance(case.get('expected'), dict):
            raise ValueError(f'{case_id}: expected basis required')
        contract = case.get('rejection_contract')
        if contract is not None:
            if (not isinstance(contract, dict) or set(contract) != {
                    'classification', 'phase', 'sqlstate', 'transaction_after_error'}
                    or contract['classification'] != 'expected_rejection'
                    or case['kind'] != 'business' or case['expected'].get('status') != 7
                    or contract['sqlstate'] != case['expected'].get('state')
                    or contract['sqlstate'] not in {'22P02', '22003', '25P02'}
                    or contract['transaction_after_error'] not in {'I', 'E'}
                    or contract['phase'] != {'query': 'PQexec', 'prepare': 'PQprepare',
                                             'describe': 'PQdescribePrepared',
                                             'execute': 'PQexecPrepared'}.get(data['phase'])):
                raise ValueError(f'{case_id}: invalid rejection contract')
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


def compare_scalar_output(actual, reference, contract):
    """Compare collected scalar output without changing either observation."""
    if contract != {'result_format': 0}:
        return False

    def collected(output):
        allowed = {'rows', 'cells', 'status', 'state', 'fields', 'params', 'formats',
                   'command_tag', 'affected_rows', 'table_origins', 'param_count'}
        if not isinstance(output, dict) or set(output) - allowed:
            return False
        rows, cells = output.get('rows'), output.get('cells')
        if not isinstance(rows, list) or not isinstance(cells, list) or len(rows) != len(cells):
            return False
        for row, tagged in zip(rows, cells):
            if not isinstance(row, list) or not isinstance(tagged, list) or len(row) != len(tagged):
                return False
            for value, cell in zip(row, tagged):
                if not isinstance(cell, dict) or type(cell.get('length')) is not int:
                    return False
                if value is None:
                    if cell != {'kind': 'sql_null', 'length': 0}:
                        return False
                elif isinstance(value, str):
                    payload = value.encode('utf-8')
                    if cell != {'kind': 'value', 'length': len(payload), 'hex': payload.hex()}:
                        return False
                else:
                    return False
        return True

    return (collected(actual) and collected(reference)
            and actual['rows'] == reference['rows'] and actual['cells'] == reference['cells'])


def compare_rows(actual, reference, row_order):
    """Compare collected rows under an explicit ordering contract."""
    if row_order not in {'strict', 'multiset'}:
        return False
    # Validate both complete tagged observations before constructing hashable rows.
    if not all(compare_scalar_output(x, x, {'result_format': 0}) for x in (actual, reference)):
        return False
    if row_order == 'strict':
        return compare_scalar_output(actual, reference, {'result_format': 0})

    def frequencies(output):
        return Counter(json.dumps(row, sort_keys=True, separators=(',', ':'))
                       for row in output['cells'])

    return frequencies(actual) == frequencies(reference)


def compare_metadata(actual, reference, phase='query'):
    """Compare result descriptions collected by libpq for this operation phase."""
    if phase not in {'query', 'prepare', 'describe', 'execute'}:
        return False

    def collected(output):
        if not isinstance(output, dict):
            return False
        fields, formats = output.get('fields'), output.get('formats')
        params, count = output.get('params'), output.get('param_count')
        if not isinstance(fields, list) or not isinstance(formats, list):
            return False
        if len(formats) != len(fields) or any(type(x) is not int or x not in (0, 1) for x in formats):
            return False
        for field in fields:
            if not isinstance(field, (tuple, list)) or len(field) != 4:
                return False
            name, oid, length, modifier = field
            if not isinstance(name, str) or any(type(x) is not int for x in (oid, length, modifier)):
                return False
            if oid <= 0 or length < -2 or modifier < -1:
                return False
        if not isinstance(params, list) or type(count) is not int or count != len(params):
            return False
        if any(type(oid) is not int or oid <= 0 for oid in params):
            return False
        # PQnparams describes parameters only on PQdescribePrepared results.
        return phase == 'describe' or count == 0

    keys = ('fields', 'formats', 'params', 'param_count')
    return (collected(actual) and collected(reference)
            and all(actual[k] == reference[k] for k in keys))


def compare_rejection(actual, reference, contract):
    """A SQL error is acceptable only with collected recovery/effect/cleanup evidence.

    The phase is the libpq call, not a guessed backend parse/execute stage.
    Required-success and unimplemented capabilities cannot become rejections.
    """
    if (not isinstance(contract, dict) or set(contract) != {
            'classification', 'phase', 'sqlstate', 'transaction_after_error'}
            or contract['classification'] != 'expected_rejection'
            or contract['phase'] not in {'PQexec', 'PQprepare', 'PQdescribePrepared', 'PQexecPrepared'}
            or contract['sqlstate'] not in {'22P02', '22003', '25P02'}
            or contract['transaction_after_error'] not in {'I', 'E'}):
        return False

    def successful_probe(probe):
        return (isinstance(probe, dict) and probe.get('status') == 2
                and probe.get('state') == ''
                and compare_scalar_output(probe, probe, {'result_format': 0})
                and compare_metadata(probe, probe) and isinstance(probe.get('rows'), list)
                and all(isinstance(row, list) and all(v is None or isinstance(v, str)
                                                    for v in row) for row in probe['rows']))

    def collected(record):
        if not isinstance(record, dict) or not {
                'classification', 'phase', 'backend_phase', 'connection_ok', 'error',
                'transaction_after_error', 'recovery', 'transaction_after_recovery',
                'before', 'after', 'cleanup'} <= set(record):
            return False
        return (record['classification'] == contract['classification']
                and record['phase'] == contract['phase'] and record['backend_phase'] is None
                and record['connection_ok'] is True
                and isinstance(record['error'], dict)
                and record['error'].get('status') == 7
                and record['error'].get('state') == contract['sqlstate']
                and record['transaction_after_error'] == contract['transaction_after_error']
                and record['transaction_after_recovery'] == 'I'
                and successful_probe(record['recovery']) and record['recovery']['rows'] == [['1']]
                and successful_probe(record['before']) and successful_probe(record['after'])
                and record['before']['rows'] == record['after']['rows']
                and isinstance(record['cleanup'], dict)
                and set(record['cleanup']) == {'rollback', 'drop', 'connection_closed'}
                and all(value is True for value in record['cleanup'].values()))

    return (collected(actual) and collected(reference)
            and actual['before']['rows'] == reference['before']['rows']
            and actual['after']['rows'] == reference['after']['rows'])


def source_snapshot(root):
    """Hash tracked and nonignored Rust build/fixture inputs, including deletions."""
    import hashlib
    import subprocess
    root = Path(root)
    names = subprocess.check_output([
        'git', '-C', str(root), 'ls-files', '-z', '--cached', '--others',
        '--exclude-standard', '--', '*.rs', '*.toml', 'Cargo.lock', '*.py', '*.json',
        '.cargo/*']).split(b'\0')
    files = {}
    for name in sorted(set(names) - {b''}):
        relative = name.decode()
        path = root / relative
        files[relative] = hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None
    return {'head': subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'],
                                           text=True).strip(),
            'workspace_sha256': hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest(),
            'files': files}


def validate_provenance(record, expected):
    """Fail closed: exact target, independent build source and stable run inputs.

    expected is a fresh end-of-run collection, never a caller-declared version.
    The reference build document is controlled external metadata, not SQL output.
    """
    def digest(value):
        return isinstance(value, str) and re.fullmatch(r'[a-f0-9]{64}', value) is not None

    try:
        target = record['target_version']
        if target not in {'18.4', '18.6'} or expected['target_version'] != target:
            return False
        for key in ['run', 'source', 'build', 'client', 'os', 'storage', 'authentication',
                    'endpoints', 'reference', 'reference_build']:
            if not record[key] or record[key] != expected[key]:
                return False
        source, build, client = (record[key] for key in ['source', 'build', 'client'])
        if (not re.fullmatch(r'[a-f0-9]{40}', source['head'])
                or not digest(source['workspace_sha256'])
                or not Path(build['path']).is_absolute() or not digest(build['sha256'])
                or build['profile'] != 'cargo-test-default-features'
                or not Path(client['library']).is_absolute() or not digest(client['sha256'])
                or not isinstance(client['PQlibVersion'], int) or client['PQlibVersion'] < 180000
                or not client['python']):
            return False
        for side in ['aster', 'reference']:
            endpoint = record['endpoints'][side]
            if (endpoint['protocol'] not in {30000, 30002} or not endpoint['server_version']
                    or not all(isinstance(endpoint[k], str) and endpoint[k]
                               for k in ['encoding', 'timezone', 'search_path'])
                    or endpoint['transaction'] != 'I'):
                return False
        reference, metadata = record['reference'], record['reference_build']
        number = 180000 + int(target.split('.')[1])
        return (str(reference['server_version_num']) == str(number)
                and reference['version'].startswith('PostgreSQL ' + target + ' ')
                and record['endpoints']['reference']['server_version'].split()[0] == target
                and metadata['version'] == target and metadata['server_version_num'] == number
                and isinstance(metadata['source'], str) and bool(metadata['source'].strip())
                and digest(metadata['artifact_sha256'])
                and isinstance(metadata['configuration'], str) and bool(metadata['configuration'].strip())
                and record['storage'] == 'CreateAnalyzeSession test storage'
                and record['authentication'] == 'InsecureRootOnly')
    except (KeyError, TypeError, ValueError, AttributeError):
        return False


def summarize_run(run, cases, attempts, cleanup_ok, completed, run_error=None):
    """Count unique business identities only after all applicable gates and cleanup.

    Attempts are immutable history: a later pass cannot erase a failure. Assertion
    totals cover completed check() contracts only, excluding comparator controls.
    """
    ids = [case['case_id'] for case in cases]
    entries = {case['case_id']: case for case in cases}
    current = [item for item in attempts if item['run'] == run]
    grouped = {key: [item for item in current if item['case_id'] == key] for key in ids}
    invalid = len(ids) != len(entries) or any(item['case_id'] not in entries for item in current)
    invalid |= any(len({item['attempt'] for item in group}) != len(group) for group in grouped.values())
    counts = Counter(item['outcome'] for item in current)
    if not completed and cleanup_ok and not any(
            counts[key] for key in ['failed', 'pending', 'exception', 'timeout']):
        counts[run_error or 'failed'] += 1
    failed = counts['failed'] + int(not cleanup_ok) + int(invalid)
    skipped = sum(not group for group in grouped.values()) + counts['skipped']
    errors = counts['exception'] + counts['pending']
    timeouts = counts['timeout']
    passed = any(case['kind'] == 'business' for case in cases) and completed and cleanup_ok and not (
        failed or skipped or errors or timeouts or counts['ignored'])
    valid = {}
    for key, group in grouped.items():
        good = bool(group) and all(item['outcome'] == 'passed' and
                                   type(item['assertions']) is int and item['assertions'] > 0
                                   for item in group)
        if good:
            valid[key] = group[0]['assertions']
        else:
            passed = False
    effective = [key for key in valid if entries[key]['kind'] == 'business'] if completed and cleanup_ok and not invalid else []
    return {'run': run, 'passed': passed, 'discovered': len(entries),
            'executed': len(grouped) - sum(not group for group in grouped.values()),
            'attempts': len(current), 'unique_effective': len(effective),
            'failed': failed, 'exceptions': errors, 'timeouts': timeouts,
            'skipped': skipped, 'ignored': counts['ignored'],
            'self_checks': sum(entries[key]['kind'] == 'self_check' for key in valid),
            'assertions': sum(valid[key] for key in effective),
            'assertion_scope': 'completed business check contracts; excludes self checks and run guards',
            'cleanup_ok': cleanup_ok, 'completed': completed,
            'retries': [{'case_id': key, 'attempts': [item['attempt'] for item in group]}
                        for key, group in grouped.items() if len(group) > 1]}


def validate_whitelist(rules):
    """Accept only reviewed exact column-name substitutions; no error exemptions."""
    scope_keys = {'capability', 'case_id', 'sql', 'type_oids', 'client', 'environment',
                  'version', 'implementation', 'configuration'}
    review_keys = {'version', 'implementation', 'configuration', 'client'}

    def text(value):
        return isinstance(value, str) and bool(value.strip()) and not any(c in value for c in '*?')

    if not isinstance(rules, list):
        raise ValueError('whitelist must be an array')
    seen = set()
    for rule in rules:
        if not isinstance(rule, dict) or set(rule) != {
                'id', 'scope', 'field', 'column', 'actual', 'reference', 'transform',
                'basis', 'approval', 'review'}:
            raise ValueError('unknown or missing whitelist fields')
        scope, basis, review = rule['scope'], rule['basis'], rule['review']
        if (not text(rule['id']) or rule['id'] in seen
                or not isinstance(scope, dict) or set(scope) != scope_keys
                or not all(text(scope[k]) for k in scope_keys - {'type_oids'})
                or not isinstance(scope['type_oids'], list) or not scope['type_oids']
                or any(type(x) is not int or x <= 0 for x in scope['type_oids'])
                or rule['field'] != 'fields.name' or rule['transform'] != 'replace_exact_name'
                or type(rule['column']) is not int or rule['column'] < 0
                or not text(rule['actual']) or not text(rule['reference'])
                or rule['actual'] == rule['reference']
                or not isinstance(basis, dict)
                or set(basis) != {'version', 'section', 'link', 'evidence'}
                or not all(text(v) for v in basis.values())
                or basis['version'] != scope['version']
                or not basis['link'].startswith('https://') or not text(rule['approval'])
                or not isinstance(review, dict) or set(review) != review_keys
                or any(review[k] != scope[k] for k in review_keys)):
            raise ValueError('whitelist requires exact scope, source and bound review')
        seen.add(rule['id'])
    return copy.deepcopy(rules)


def apply_whitelist(rules, context, actual, reference, phase='query'):
    """Transform comparison copies only, after complete successful collection.

    SQL errors, recovery and side effects remain under their existing contracts.
    A stale rule simply does not apply; it never fills absent evidence.
    """
    rules = validate_whitelist(rules)
    a, b, used = copy.deepcopy(actual), copy.deepcopy(reference), []
    if rules:
        for observation in (a, b):
            if (not compare_rows(observation, observation, 'strict')
                    or not compare_metadata(observation, observation, phase)
                    or observation.get('status') != 2 or observation.get('state') != ''):
                raise ValueError('whitelist cannot waive errors or missing evidence')
    for rule in rules:
        col = rule['column']
        if (context == rule['scope'] and col < len(a['fields']) and col < len(b['fields'])
                and a['fields'][col][0] == rule['actual']
                and b['fields'][col][0] == rule['reference']):
            a['fields'][col] = list(a['fields'][col])
            a['fields'][col][0] = rule['reference']
            used.append(rule['id'])
    return a, b, used


def evaluate_capabilities(cases, observations, summary, provenance):
    """Derive bounded support from current raw evidence after both cleanup layers.

    Classification describes ownership, never proof. Shared dialect equivalence
    remains unverified: this fixture has only a MySQL health connection.
    """
    matrix = cases.get('capability_matrix') or json.loads(
        Path(__file__).with_name('pg_core_capabilities.json').read_text())
    report = {'schema_version': 1, 'run': summary.get('run'),
              'target_version': provenance.get('record', {}).get('target_version'),
              'capabilities': {}}
    entries = matrix['capabilities']
    for key, spec in entries.items():
        report['capabilities'][key] = dict(spec, status='unverified',
                                          reason='incomplete current evidence')
    try:
        validate_cases(cases)
        record, expected = provenance['record'], provenance['expected']
        run = summary['run']
        business = [x for x in cases['cases'] if x['kind'] == 'business']
        ids = {x['case_id'] for x in business}
        registered = {x['capability_id'] for x in business}
        associations = [i for spec in entries.values() for i in spec['case_ids']]
        if (matrix['schema_version'] != 1 or set(associations) != ids
                or len(associations) != len(ids) or not registered <= set(entries)
                or any(set(spec['case_ids']) != {x['case_id'] for x in business
                                                 if x['capability_id'] == key}
                       for key, spec in entries.items())
                or not validate_provenance(record, expected) or record['run'] != run
                or not summary['passed'] or not summary['completed'] or not summary['cleanup_ok']
                or summary['unique_effective'] != len(business) or not business
                or summary['discovered'] != len(cases['cases'])
                or summary['executed'] != len(cases['cases']) or summary['assertions'] <= 0
                or any(summary[k] != 0 for k in ['failed', 'exceptions', 'timeouts', 'skipped', 'ignored'])
                or observations['rust_cleanup'] != {'database_dropped': True,
                    'postgres_port_closed': True, 'mysql_port_closed': True}
                or observations['child_exit']['success'] is not True
                or observations['child_exit']['code'] != 0):
            return report
        recount = summarize_run(run, cases['cases'], observations['attempts'], True, True)
        if any(summary[k] != recount[k] for k in recount):
            return report
        streams = {}
        for side in ['aster', 'reference']:
            rows = observations[side]
            if any(r['run'] != run or r['side'] != side for r in rows):
                return report
            # Duplicate records must never overwrite an earlier failure.
            if len({(r['case_id'], r['attempt']) for r in rows}) != len(rows):
                return report
            streams[side] = {r['case_id']: r for r in rows}
            for key in ['cleanup_rollback', 'cleanup_drop']:
                if streams[side][key]['observation']['status'] != 1:
                    return report
            if streams[side]['connection_closed']['outcome'] != 'closed':
                return report
        mysql = observations['mysql']
        health = (len(mysql) == 3 and [x['case_id'] for x in mysql] == ['before','during','after']
                  and all(x['run'] == run and x['side'] == 'mysql'
                          and x['outcome'] == 'bypass_health'
                          and x['observation']['rows'] == [[[51,49],[45,49,55]]]
                          for x in mysql))
        if not health:
            return report
        passed = set()
        for case in business:
            key, phase = case['case_id'], case['input']['phase']
            pair = [streams[side][key] for side in ['aster', 'reference']]
            valid = True
            for item in pair:
                data = copy.deepcopy(case['input'])
                sql = item['input']['sql']
                tables = re.findall(r'public\.pg_core_diff_[0-9]+', sql)
                data['sql'] = data['sql'].replace('{run_table}', tables[0] if tables else '{run_table}')
                valid &= (item['input'] == data and item['outcome'] == 'collected'
                          and item['attempt'] == 1)
                obs = item['observation']
                valid &= all(obs.get(k) == v for k, v in case['expected'].items())
            a, b = [x['observation'] for x in pair]
            valid &= (compare_rows(a, b, case.get('row_order', 'strict'))
                      and compare_metadata(a, b, phase)
                      and a.get('status') == b.get('status') and a.get('state') == b.get('state'))
            if case.get('rejection_contract'):
                rejections = [streams[side][key + '_rejection_contract']['observation']
                              for side in ['aster', 'reference']]
                valid &= compare_rejection(*rejections, case['rejection_contract'])
            if valid:
                passed.add(key)
        # Recovery and transaction cases are global prerequisites for support;
        # they cannot be dropped to upgrade a value-only result.
        complete = passed == ids
        for key, spec in entries.items():
            result = report['capabilities'][key]
            result['evidence'] = {'run': run, 'case_ids': spec['case_ids'],
                                  'source': {k: record['source'][k] for k in ['head', 'workspace_sha256']},
                                  'version': record['reference'],
                                  'raw': ['aster.jsonl','reference.jsonl'],
                                  'summary': 'summary.json', 'cleanup': 'rust-cleanup.json'}
            if spec['classification'] == 'shared':
                result.update(reason='MySQL bypass health only; missing MySQL reference/type evidence',
                              bypass_health=health)
            elif spec['classification'] == 'postgresql_exclusive' and spec['case_ids'] and complete:
                result.update(status='supported', reason='current exact ' + record['target_version']
                              + ' bounded PostgreSQL input domain')
            # Formal unsupported declarations also stay unverified unless a
            # separate complete refusal contract is explicitly implemented.
    except (KeyError, TypeError, ValueError, AttributeError):
        return report
    return report


def finalize_capabilities(directory):
    """Called by Rust only after child exit, MySQL after-check and server cleanup."""
    directory = Path(directory)
    load = lambda name: json.loads((directory / name).read_text())
    observations = {side: [json.loads(line) for line in
                          (directory / (side + '.jsonl')).read_text().splitlines()]
                    for side in ['aster', 'reference', 'mysql']}
    observations.update(rust_cleanup=load('rust-cleanup.json'), child_exit=load('child-exit.json'),
                        attempts=load('attempts.json'))
    record = load('environment.json')
    # Recollect source/build/client identity at the original end-of-run gate;
    # finalization cannot substitute record for independently collected expected.
    report = evaluate_capabilities(load_cases(), observations, load('summary.json'),
                                   {'record': record, 'expected': load('provenance-expected.json')})
    (directory / 'capabilities.json').write_text(json.dumps(report, sort_keys=True))
    return report
