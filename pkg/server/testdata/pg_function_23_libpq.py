# Copyright 2026 AsterSQL.
"""Bounded registration/overload observations. No import-time client execution.

--self-test requires no database or libpq. --reference-only records a real PG
endpoint without claiming AsterSQL equivalence. Default: PORT DATABASE EVIDENCE.
PG18_REFERENCE_DSN and optional PG_LIBPQ_LIBRARY are never printed.
"""
import copy
import ctypes as c
import ctypes.util
import json
import os
from pathlib import Path
import sys

# Fixed order and IDs are shared by reference-only and differential runs.
CASES = (
    ('abs-int4', 'SELECT pg_catalog.abs(-17::int4) AS v', 23, '17'),
    ('abs-int8', 'SELECT pg_catalog.abs(-17::int8) AS v', 20, '17'),
    ('round-implicit', 'SELECT round(42::int4, 2::int4) AS v', 1700, '42.00'),
    ('substr-unknown', "SELECT substr('abcdef', 2, 3) AS v", 25, 'bcd'),
    ('date-trunc', "SELECT date_trunc('day', timestamp '2026-10-29 12:34:56') AS v", 1114, '2026-10-29 00:00:00'),
    ('abs-text', "SELECT abs('1'::text) AS v", None, '42883'),
    ('abs-empty', 'SELECT abs() AS v', None, '42883'),
)


def compare(left, right):
    """Messages/positions are archived, not treated as portable equality fields."""
    for key in ('status', 'metadata', 'parameters', 'rows', 'state'):
        assert left[key] == right[key], f'differential mismatch: {key}'


def self_test():
    original = dict(status=2, metadata=[[23, 4, -1, 0, 'v', 0, 0]],
                    parameters=[], rows=[['3137']], state='')
    compare(original, copy.deepcopy(original))
    for key in ('value', 'oid', 'sqlstate'):
        altered = copy.deepcopy(original)
        if key == 'value':
            altered['rows'][0][0] = '3138'
        elif key == 'oid':
            altered['metadata'][0][0] = 20
        else:
            altered['state'] = '42883'
        try:
            compare(original, altered)
        except AssertionError:
            continue
        raise AssertionError(f'comparator accepted corrupted {key}')
    assert original['rows'] == [['3137']] and original['metadata'][0][0] == 23
    return 3


class Libpq:
    def __init__(self):
        library = os.environ.get('PG_LIBPQ_LIBRARY') or (
            '/opt/homebrew/opt/libpq@18/lib/libpq.dylib'
            if sys.platform == 'darwin' else ctypes.util.find_library('pq'))
        self.lib = c.CDLL(library)
        p, t, i, u = c.c_void_p, c.c_char_p, c.c_int, c.c_uint
        signatures = {
            'PQlibVersion': (i,), 'PQconnectdb': (p, t), 'PQstatus': (i, p),
            'PQfinish': (None, p), 'PQexec': (p, p, t), 'PQclear': (None, p),
            'PQresultStatus': (i, p), 'PQntuples': (i, p), 'PQnfields': (i, p),
            'PQgetisnull': (i, p, i, i), 'PQgetvalue': (p, p, i, i),
            'PQgetlength': (i, p, i, i), 'PQftype': (u, p, i),
            'PQfsize': (i, p, i), 'PQfmod': (i, p, i), 'PQfformat': (i, p, i),
            'PQfname': (t, p, i), 'PQftable': (u, p, i), 'PQftablecol': (i, p, i),
            'PQnparams': (i, p), 'PQparamtype': (u, p, i),
            'PQresultErrorField': (t, p, i),
            'PQprepare': (p, p, t, t, i, c.POINTER(u)),
            'PQdescribePrepared': (p, p, t),
            'PQexecPrepared': (p, p, t, i, c.POINTER(t), c.POINTER(i), c.POINTER(i), i),
        }
        for name, signature in signatures.items():
            fn = getattr(self.lib, name)
            fn.restype, fn.argtypes = signature[0], signature[1:]
            setattr(self, name, fn)
        assert self.PQlibVersion() >= 180000, 'libpq>=18 required'

    def observe(self, result):
        assert result, 'libpq returned no result'
        try:
            fields = {f: (self.PQresultErrorField(result, ord(f)) or b'').decode()
                      for f in 'SVCMDHP'}
            return dict(status=self.PQresultStatus(result), state=fields['C'],
                        errors=fields,
                        metadata=[[self.PQftype(result, i), self.PQfsize(result, i),
                                   self.PQfmod(result, i), self.PQfformat(result, i),
                                   self.PQfname(result, i).decode(),
                                   self.PQftable(result, i), self.PQftablecol(result, i)]
                                  for i in range(self.PQnfields(result))],
                        parameters=[self.PQparamtype(result, i)
                                    for i in range(self.PQnparams(result))],
                        rows=[[None if self.PQgetisnull(result, r, i) else c.string_at(
                            self.PQgetvalue(result, r, i), self.PQgetlength(result, r, i)).hex()
                               for i in range(self.PQnfields(result))]
                              for r in range(self.PQntuples(result))])
        finally:
            self.PQclear(result)


def run(dsns, evidence):
    pq = Libpq()
    evidence.mkdir(parents=True, exist_ok=False)
    connections, records, failures = [], {}, []
    assertions = 0
    def check(condition, label):
        nonlocal assertions
        assertions += 1
        if not condition:
            failures.append(label)
    def archive(side, case, phase, observation):
        records.setdefault((case, phase), {})[side] = observation
        with (evidence / f'{side}.jsonl').open('a') as stream:
            stream.write(json.dumps(dict(case_id=case, phase=phase,
                                         observation=observation)) + '\n')
        return observation
    def query(conn, sql):
        return pq.observe(pq.PQexec(conn, sql.encode()))
    try:
        for side, dsn in dsns:
            conn = pq.PQconnectdb(dsn.encode())
            assert conn, 'connection allocation failed'
            connections.append(conn)
            assert pq.PQstatus(conn) == 0, f'{side} connection failed (DSN redacted)'
            identity = {sql: query(conn, sql) for sql in (
                'SELECT version()', 'SHOW server_version_num', 'SHOW client_encoding',
                'SHOW server_encoding', 'SHOW standard_conforming_strings')}
            (evidence / f'{side}-identity.json').write_text(json.dumps(
                dict(libpq=pq.PQlibVersion(), observations=identity)))
            if side == 'reference' and len(dsns) == 2:
                check(identity['SHOW server_version_num']['rows'] == [['313830303036']],
                      'PG18.6 target unavailable; other versions are observations only')
            # Public catalog only: no user-created overloads may influence this run.
            if side == 'reference':
                owned = query(conn, "SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE p.proname IN ('abs','round','substr','date_trunc') AND n.nspname NOT IN ('pg_catalog','information_schema')")
                check(owned['rows'] == [['30']], 'reference has user overloads')
            for case, sql, oid, expected in CASES:
                obs = archive(side, case, 'query', query(conn, sql))
                if oid is None:
                    check(obs['status'] == 7 and obs['state'] == expected,
                          f'{side}/{case}: rejection')
                    check(not obs['metadata'] and not obs['rows'], f'{side}/{case}: false payload')
                    check(all(obs['errors'][f] for f in 'SVCM'), f'{side}/{case}: error fields')
                else:
                    check(obs['status'] == 2 and obs['rows'] == [[expected.encode().hex()]],
                          f'{side}/{case}: value')
                    check(bool(obs['metadata']) and obs['metadata'][0][0] == oid,
                          f'{side}/{case}: oid')
                recovery = archive(side, case, 'recovery', query(conn, 'SELECT 1::int4 AS v'))
                check(recovery['rows'] == [['31']] and recovery['state'] == '',
                      f'{side}/{case}: recovery')
            name = b'function23_abs'
            prep = archive(side, 'prepared-abs', 'parse', pq.observe(pq.PQprepare(
                conn, name, b'SELECT pg_catalog.abs($1::int4) AS v', 0, None)))
            check(prep['status'] == 1, f'{side}: prepare')
            desc = archive(side, 'prepared-abs', 'statement-describe',
                           pq.observe(pq.PQdescribePrepared(conn, name)))
            check(desc['parameters'] == [23] and desc['metadata'] == [[23, 4, -1, 0, 'v', 0, 0]],
                  f'{side}: fixed parameter/result description')
            for result_format in (0, 1):
                for value in (b'-1', b'2', None):
                    values = (c.c_char_p * 1)(value)
                    obs = archive(side, f'prepared-{result_format}-{value!r}', 'execute',
                                  pq.observe(pq.PQexecPrepared(conn, name, 1, values,
                                                              None, None, result_format)))
                    expected = None if value is None else (abs(int(value)).to_bytes(4, 'big').hex()
                               if result_format else str(abs(int(value))).encode().hex())
                    check(obs['rows'] == [[expected]] and obs['status'] == 2,
                          f'{side}: prepared value/NULL')
                    check(obs['metadata'] == [[23, 4, -1, result_format, 'v', 0, 0]],
                          f'{side}: prepared descriptor')
            # PQfinish releases this connection-owned statement even on failure.
        if len(dsns) == 2:
            for (case, phase), observations in records.items():
                try:
                    compare(observations['reference'], observations['aster'])
                    assertions += 1
                except AssertionError as error:
                    failures.append(f'{case}/{phase}: {error}')
    finally:
        for conn in connections:
            pq.PQfinish(conn)
        summary = dict(cases=len(records), assertions=assertions,
                       negative_controls=self_test(), failures=failures,
                       connections_closed=len(connections),
                       scope='PG observations only; portal Describe/raw wire and MySQL reference pending')
        (evidence / 'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary), flush=True)
    assert records and assertions and not failures, 'registration differential failed'


if __name__ == '__main__':
    if sys.argv[1:] == ['--self-test']:
        print(json.dumps(dict(negative_controls=self_test(), scenarios=len(CASES))))
    else:
        reference = os.environ.get('PG18_REFERENCE_DSN')
        assert reference, 'PG18_REFERENCE_DSN required (no skip)'
        if sys.argv[1] == '--reference-only':
            run([('reference', reference)], Path(sys.argv[2]))
        else:
            port, database, evidence = sys.argv[1:]
            run([('reference', reference), ('aster',
                 f'host=127.0.0.1 port={int(port)} user=root dbname={database} connect_timeout=10')],
                Path(evidence))
