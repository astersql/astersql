# Copyright 2026 AsterSQL.
"""Bounded registration/overload observations. No import-time client execution.

--self-test requires no database or libpq. --reference-only records a real PG
endpoint without claiming AsterSQL equivalence. Default: PORT DATABASE EVIDENCE MYSQL_PORT.
PG18_REFERENCE_DSN and optional PG_LIBPQ_LIBRARY are never printed.
MySQL8 reference connection uses MYSQL8_REFERENCE_HOST/PORT/USER/PASSWORD
(all required; PASSWORD may be empty for an owned local instance).
MYSQL8_CLIENT_LIBRARY selects the MySQL8 public C ABI. Reference services are
owned by the caller; this script only creates/drops its uniquely named database.
Raw PG messages supplement libpq only for per-phase/portal/NULL-length fields.
"""
import copy
import ctypes as c
import ctypes.util
import json
import os
import socket
import struct
import subprocess
import hashlib
import signal
from pathlib import Path
import sys

# Fixed order and IDs are shared by reference-only and differential runs.
CASES = (
    ('abs-int4', 'SELECT pg_catalog.abs(-17::int4) AS v', 23, '17'),
    ('abs-int8', 'SELECT pg_catalog.abs(-17::int8) AS v', 20, '17'),
    ('round-implicit', 'SELECT round(42::int4, 2::int4) AS v', 1700, '42.00'),
    ('substr-unknown', "SELECT substr('abcdef', 2, 3) AS v", 25, 'bcd'),
    ('date-trunc', "SELECT date_trunc('day', timestamp '2026-10-29 12:34:56') AS v", 1114, '2026-10-29 00:00:00'),
    ('abs-null', 'SELECT pg_catalog.abs(NULL::int4) AS v', 23, None),
    ('abs-boundary', "SELECT pg_catalog.abs('-2147483647'::int4) AS v", 23, '2147483647'),
    ('abs-overflow', "SELECT pg_catalog.abs('-2147483648'::int4) AS v", None, '22003'),
    ('abs-text', "SELECT abs('1'::text) AS v", None, '42883'),
    ('abs-empty', 'SELECT abs() AS v', None, '42883'),
    ('date-trunc-unit', "SELECT date_trunc('nonsense', timestamp '2026-10-29 12:34:56') AS v", None, '22023'),
    ('date-trunc-text', "SELECT date_trunc('day', '2026-10-29 12:34:56'::text) AS v", None, '42883'),
    ('abs-text-utf8', "SELECT 'é', abs('1'::text) AS v", None, '42883'),
)


def compare(left, right):
    """Messages/positions are archived, not treated as portable equality fields."""
    for key in ('status', 'metadata', 'parameters', 'rows', 'state'):
        assert left[key] == right[key], f'differential mismatch: {key}'
    if left['state']:
        for field in 'SVCDHP':
            assert left.get('errors', {}).get(field, '') == right.get('errors', {}).get(field, ''), f'PG error field mismatch: {field}'


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
            'PQsocket': (i, p), 'PQlibVersion': (i,), 'PQconnectdb': (p, t), 'PQstatus': (i, p),
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


class Wire:
    """Supplement libpq authentication with explicit Parse/portal Describe phases."""
    def __init__(self, pq, dsn):
        self.pq = pq
        self.conn = pq.PQconnectdb(dsn.encode())
        if not self.conn or pq.PQstatus(self.conn) != 0:
            if self.conn:
                pq.PQfinish(self.conn)
            raise AssertionError('wire login failed (DSN redacted)')
        self.fd = pq.PQsocket(self.conn)
        self.blocking = os.get_blocking(self.fd)
        self.sock = socket.socket(fileno=os.dup(self.fd))
        self.sock.settimeout(10)

    def close(self):
        self.sock.close()
        os.set_blocking(self.fd, self.blocking)
        self.pq.PQfinish(self.conn)

    def send(self, tag, body=b''):
        self.sock.sendall(tag + struct.pack('!I', len(body) + 4) + body)

    def exact(self, size):
        data = b''
        while len(data) < size:
            part = self.sock.recv(size - len(data))
            assert part, 'wire EOF'
            data += part
        return data

    def read(self, until):
        obs = dict(tags='', metadata=[], parameters=[], rows=[], lengths=[],
                   errors=[], ready='', commands=[])
        while True:
            tag = self.exact(1).decode()
            size = struct.unpack('!I', self.exact(4))[0]
            assert 4 <= size < 1048576
            body = self.exact(size - 4)
            if tag not in ('N', 'S'):
                obs['tags'] += tag
            if tag == 'T':
                count = struct.unpack('!H', body[:2])[0]
                offset = 2
                for _ in range(count):
                    end = body.index(b'\0', offset)
                    name = body[offset:end].decode()
                    table, attr, oid, width, mod, fmt = struct.unpack('!IhIhih', body[end+1:end+19])
                    obs['metadata'].append([oid, width, mod, fmt, name, table, attr])
                    offset = end + 19
                assert offset == len(body)
            elif tag == 't':
                count = struct.unpack('!H', body[:2])[0]
                obs['parameters'] = list(struct.unpack('!' + 'I' * count, body[2:]))
            elif tag == 'D':
                count = struct.unpack('!H', body[:2])[0]
                offset, row, lengths = 2, [], []
                for _ in range(count):
                    length = struct.unpack('!i', body[offset:offset+4])[0]
                    offset += 4
                    assert length >= -1
                    lengths.append(length)
                    row.append(None if length == -1 else body[offset:offset+length].hex())
                    if length >= 0:
                        offset += length
                assert offset == len(body)
                obs['rows'].append(row)
                obs['lengths'].append(lengths)
            elif tag == 'E':
                obs['errors'].append({chr(v[0]): v[1:].decode() for v in body.split(b'\0') if v})
            elif tag == 'C':
                obs['commands'].append(body[:-1].decode())
            elif tag == 'Z':
                obs['ready'] = body.decode()
            if tag in until or tag == 'E':
                return obs

    def phase(self, tag, body, until):
        self.send(tag, body)
        self.send(b'H')
        return self.read(until)

    def sync(self):
        self.send(b'S')
        return self.read('Z')

    def query(self, sql):
        self.send(b'Q', sql.encode() + b'\0')
        result = self.read('Z')
        if result['errors']:
            result['tail'] = self.read('Z')
        return result


# MYSQL_FIELD / MYSQL_BIND ABI from MySQL 8.0 public mysql.h. The selected
# library must implement that ABI (override MYSQL8_CLIENT_LIBRARY explicitly).
class MysqlField(c.Structure):
    _fields_ = [(n, c.c_char_p) for n in ('name', 'org_name', 'table', 'org_table', 'db', 'catalog', 'def_')] + [
        ('length', c.c_ulong), ('max_length', c.c_ulong)] + [
        (n, c.c_uint) for n in ('name_length', 'org_name_length', 'table_length', 'org_table_length',
                               'db_length', 'catalog_length', 'def_length', 'flags', 'decimals', 'charsetnr')] + [
        ('type', c.c_int), ('extension', c.c_void_p)]


class MysqlBind(c.Structure):
    _fields_ = [(n, c.c_void_p) for n in ('length', 'is_null', 'buffer', 'error', 'row_ptr',
                                       'store_param_func', 'fetch_result', 'skip_result')] + [
        (n, c.c_ulong) for n in ('buffer_length', 'offset', 'length_value')] + [
        (n, c.c_uint) for n in ('param_number', 'pack_length')] + [('buffer_type', c.c_int)] + [
        (n, c.c_bool) for n in ('error_value', 'is_unsigned', 'long_data_used', 'is_null_value')] + [
        ('extension', c.c_void_p)]


class Mysql:
    def __init__(self, host, port, user, password, database=None):
        library = os.environ.get('MYSQL8_CLIENT_LIBRARY', '/opt/homebrew/opt/mysql@8.0/lib/libmysqlclient.dylib')
        self.lib = c.CDLL(library)
        p, t, i, u, ul = c.c_void_p, c.c_char_p, c.c_int, c.c_uint, c.c_ulong
        signatures = {
            'mysql_get_client_version': (ul,), 'mysql_init': (p, p), 'mysql_options': (i, p, i, p),
            'mysql_real_connect': (p, p, t, t, t, t, u, t, ul), 'mysql_close': (None, p),
            'mysql_query': (i, p, t), 'mysql_store_result': (p, p), 'mysql_free_result': (None, p),
            'mysql_num_fields': (u, p), 'mysql_fetch_fields': (c.POINTER(MysqlField), p),
            'mysql_fetch_row': (c.POINTER(p), p), 'mysql_fetch_lengths': (c.POINTER(ul), p),
            'mysql_errno': (u, p), 'mysql_sqlstate': (t, p), 'mysql_warning_count': (u, p),
            'mysql_stmt_init': (p, p), 'mysql_stmt_prepare': (i, p, t, ul),
            'mysql_stmt_param_count': (ul, p), 'mysql_stmt_execute': (i, p),
            'mysql_stmt_result_metadata': (p, p), 'mysql_stmt_bind_param': (c.c_bool, p, c.POINTER(MysqlBind)),
            'mysql_stmt_bind_result': (c.c_bool, p, c.POINTER(MysqlBind)), 'mysql_stmt_store_result': (i, p),
            'mysql_stmt_fetch': (i, p), 'mysql_stmt_free_result': (c.c_bool, p),
            'mysql_stmt_close': (c.c_bool, p), 'mysql_stmt_errno': (u, p), 'mysql_stmt_sqlstate': (t, p),
        }
        for name, sig in signatures.items():
            fn = getattr(self.lib, name)
            fn.restype, fn.argtypes = sig[0], sig[1:]
            setattr(self, name, fn)
        assert 80000 <= self.mysql_get_client_version() < 90000, 'MySQL8 client ABI required'
        self.conn = self.mysql_init(None)
        assert self.conn
        try:
            disabled, timeout = c.c_int(1), c.c_uint(10)
            assert self.mysql_options(self.conn, 35, c.byref(disabled)) == 0  # SSL_MODE_DISABLED
            for option in (0, 11, 12):  # connect/read/write timeout
                assert self.mysql_options(self.conn, option, c.byref(timeout)) == 0
            assert self.mysql_options(self.conn, 7, c.c_char_p(b'utf8mb4')) == 0
            if not self.mysql_real_connect(self.conn, host.encode(), user.encode(), password.encode(),
                                           database.encode() if database else None, port, None, 0):
                raise AssertionError('MySQL connection failed (identity redacted)')
        except BaseException:
            self.close()
            raise

    def close(self):
        if self.conn:
            self.mysql_close(self.conn)
            self.conn = None

    def metadata(self, result):
        if not result:
            return []
        fields = self.mysql_fetch_fields(result)
        return [[fields[i].name.decode(), fields[i].type, fields[i].flags,
                 fields[i].charsetnr, fields[i].length, fields[i].decimals]
                for i in range(self.mysql_num_fields(result))]

    def query(self, sql, warnings=True):
        rc = self.mysql_query(self.conn, sql.encode())
        obs = dict(code=self.mysql_errno(self.conn), state=self.mysql_sqlstate(self.conn).decode(),
                   metadata=[], rows=[], warning_count=self.mysql_warning_count(self.conn))
        if rc == 0:
            result = self.mysql_store_result(self.conn)
            if result:
                try:
                    obs['metadata'] = self.metadata(result)
                    while True:
                        row = self.mysql_fetch_row(result)
                        if not row:
                            break
                        lengths = self.mysql_fetch_lengths(result)
                        obs['rows'].append([None if not row[i] else c.string_at(row[i], lengths[i]).hex()
                                            for i in range(len(obs['metadata']))])
                finally:
                    self.mysql_free_result(result)
        if warnings:
            obs['warnings'] = self.query('SHOW WARNINGS', warnings=False)['rows']
        return obs

    def prepared(self, sql, values):
        """Real COM_STMT_PREPARE/BIND/EXECUTE using the independent native client."""
        stmt = self.mysql_stmt_init(self.conn)
        assert stmt
        observations = []
        try:
            rc = self.mysql_stmt_prepare(stmt, sql.encode(), len(sql.encode()))
            result = self.mysql_stmt_result_metadata(stmt) if rc == 0 else None
            try:
                meta = self.metadata(result)
            finally:
                if result:
                    self.mysql_free_result(result)
            observations.append(dict(phase='prepare', code=self.mysql_stmt_errno(stmt),
                                     state=self.mysql_stmt_sqlstate(stmt).decode(), metadata=meta,
                                     parameters=self.mysql_stmt_param_count(stmt),
                                     warning_count=self.mysql_warning_count(self.conn),
                                     warnings=self.query('SHOW WARNINGS', warnings=False)['rows']))
            if rc:
                return observations
            for value in values:
                length, null = c.c_ulong(0 if value is None else len(value)), c.c_bool(value is None)
                buffer = c.create_string_buffer(value or b'')
                param = MysqlBind()
                param.buffer_type = 253  # VAR_STRING; native client encodes binary protocol params
                param.buffer = c.cast(buffer, c.c_void_p)
                param.buffer_length = length.value
                param.length, param.is_null = c.addressof(length), c.addressof(null)
                assert not self.mysql_stmt_bind_param(stmt, c.byref(param))
                rc = self.mysql_stmt_execute(stmt)
                obs = dict(phase='execute', input=None if value is None else value.hex(),
                           code=self.mysql_stmt_errno(stmt), state=self.mysql_stmt_sqlstate(stmt).decode(),
                           rows=[], metadata=[], warning_count=self.mysql_warning_count(self.conn))
                if rc == 0:
                    result = self.mysql_stmt_result_metadata(stmt)
                    try:
                        obs['metadata'] = self.metadata(result)
                    finally:
                        if result:
                            self.mysql_free_result(result)
                    buffers = [c.create_string_buffer(4096) for _ in obs['metadata']]
                    lengths = [c.c_ulong() for _ in buffers]
                    nulls = [c.c_bool() for _ in buffers]
                    errors = [c.c_bool() for _ in buffers]
                    binds = (MysqlBind * len(buffers))()
                    for i in range(len(buffers)):
                        binds[i].buffer_type = 253
                        binds[i].buffer = c.cast(buffers[i], c.c_void_p)
                        binds[i].buffer_length = len(buffers[i])
                        binds[i].length, binds[i].is_null = c.addressof(lengths[i]), c.addressof(nulls[i])
                        binds[i].error = c.addressof(errors[i])
                    assert not self.mysql_stmt_bind_result(stmt, binds)
                    assert self.mysql_stmt_store_result(stmt) == 0
                    while True:
                        rc = self.mysql_stmt_fetch(stmt)
                        if rc == 100:  # MYSQL_NO_DATA
                            break
                        assert rc == 0 and not any(e.value for e in errors), 'native result truncation/error'
                        obs['rows'].append([None if nulls[i].value else buffers[i].raw[:lengths[i].value].hex()
                                            for i in range(len(buffers))])
                    assert not self.mysql_stmt_free_result(stmt)
                obs['execute_header_warning_count'] = obs['warning_count']
                obs['warning_count'] = self.mysql_warning_count(self.conn)
                obs['warnings'] = self.query('SHOW WARNINGS', warnings=False)['rows']
                observations.append(obs)
            return observations
        finally:
            assert not self.mysql_stmt_close(stmt)

# Each tuple fixes the inferred parameter identities, result type and repeated
# values. The DATE case is still exercised and compared; no skip/allowlist.
EXTENDED = (
    ('abs-int4', 'pg_catalog.abs($1::int4)', [23], 23, 4,
     [(-1,), (2,), (None,)], ['1', '2', None]),
    ('abs-int8', 'pg_catalog.abs($1::int8)', [20], 20, 8,
     [(-1,), (2,), (None,)], ['1', '2', None]),
    ('round-implicit', 'round($1::int4, $2::int4)', [23, 23], 1700, -1,
     [(42, 2), (0, 0), (None, 2)], ['42.00', '0', None]),
    ('substr-unknown', 'substr($1, $2::int4, $3::int4)', [25, 23, 23], 25, -1,
     [('abcdef', 2, 3), ('', 1, 1), (None, 1, 1)], ['bcd', '', None]),
    ('date-trunc', 'date_trunc($1, $2::timestamp)', [25, 1114], 1114, 8,
     [('day', '2026-10-29 12:34:56'), ('day', '2000-01-01 00:00:00'), (None, None)],
     ['2026-10-29 00:00:00', '2000-01-01 00:00:00', None]),
)

MYSQL_CASES = (
    ('abs-signed', 'SELECT abs(CAST(-17 AS SIGNED)) AS v'),
    ('abs-null', 'SELECT abs(CAST(NULL AS SIGNED)) AS v'),
    ('round-implicit', 'SELECT round(CAST(42 AS SIGNED), 2) AS v'),
    ('substr-text', "SELECT substr('abcdef', 2, 3) AS v"),
    ('abs-typed-text', "SELECT abs(CAST('1' AS CHAR)) AS v"),
    ('abs-empty', 'SELECT abs() AS v'),
    ('date-hidden', "SELECT date_trunc('day', CAST('2026-10-29 12:34:56' AS DATETIME)) AS v"),
)


def payload(value, oid, binary):
    if value is None:
        return None
    if not binary:
        return str(value).encode()
    if oid in (23, 20):
        return int(value).to_bytes(4 if oid == 23 else 8, 'big', signed=True)
    if oid == 1114:
        import datetime
        delta = datetime.datetime.fromisoformat(value) - datetime.datetime(2000, 1, 1)
        return struct.pack('!q', delta.days * 86400000000 + delta.seconds * 1000000 + delta.microseconds)
    assert oid == 25
    return str(value).encode()


def compare_wire(left, right):
    for key in ('tags', 'metadata', 'parameters', 'rows', 'lengths', 'ready', 'commands'):
        assert left[key] == right[key], f'wire mismatch: {key}'
    def diagnostics(obs):
        return [[e.get('S'), e.get('V'), e.get('C'), [bool(e.get(k)) for k in 'MD'], e.get('H'), e.get('P')]
                for e in obs['errors']]
    assert diagnostics(left) == diagnostics(right), 'wire mismatch: error fields/presence'
    if 'tail' in left or 'tail' in right:
        compare_wire(left['tail'], right['tail'])


def compare_mysql(left, right):
    # MySQL native column codes/flags/etc remain MySQL fields throughout.
    assert left == right, 'MySQL value/metadata/error/warning difference'


def matrix_self_test():
    count = self_test()
    wire = dict(tags='DCZ', metadata=[], parameters=[], rows=[['31']], lengths=[[1]],
                errors=[], ready='I', commands=['SELECT 1'])
    native = dict(rows=[['31']], metadata=[['v', 8, 32769, 63, 20, 0]],
                  code=0, state='00000', warnings=[], warning_count=0)
    for comparator, original, mutations in (
        (compare_wire, wire, ('rows', 'lengths', 'errors')),
        (compare_mysql, native, ('rows', 'metadata', 'state')),
    ):
        comparator(original, copy.deepcopy(original))
        for key in mutations:
            altered = copy.deepcopy(original)
            if key == 'rows': altered[key] = [['32']]
            elif key == 'lengths': altered[key] = [[-1]]
            elif key == 'errors': altered[key] = [{'S':'ERROR','V':'ERROR','C':'42883','M':'bad'}]
            elif key == 'metadata': altered[key][0][1] = 3
            elif key == 'state': altered[key] = '42000'
            try:
                comparator(original, altered)
            except AssertionError:
                count += 1
                continue
            raise AssertionError(f'comparator accepted {key} corruption')
    error = dict(wire, tags='E', rows=[], lengths=[], commands=[], errors=[dict(S='ERROR', V='ERROR', C='42883', M='missing', H='cast required', P='13')])
    altered = copy.deepcopy(error)
    altered['errors'][0]['P'] = '14'
    try:
        compare_wire(error, altered)
    except AssertionError:
        count += 1
    else:
        raise AssertionError('comparator accepted corrupted function position')
    return count


def run(dsns, evidence, aster_mysql=None):
    pq = Libpq()
    evidence.mkdir(parents=True, exist_ok=False)
    connections, wires, mysqls, records, failures = {}, {}, {}, {}, []
    reference_database = None
    cleanup = {}
    assertions, probes, result_scenarios = 0, 0, 0
    def check(condition, label):
        nonlocal assertions
        assertions += 1
        if not condition:
            failures.append(label)
    def archive(domain, side, case, phase, observation):
        key = (domain, case, phase)
        assert side not in records.setdefault(key, {}), f'duplicate observation: {key}/{side}'
        records[key][side] = observation
        with (evidence / f'{domain}-{side}.jsonl').open('a') as stream:
            stream.write(json.dumps(dict(case_id=case, phase=phase, observation=observation)) + '\n')
        return observation
    def query(conn, sql):
        return pq.observe(pq.PQexec(conn, sql.encode()))
    def interleave(label):
        nonlocal probes
        # All ordinary PG connections remain open while both raw-wire sessions
        # and the same-server MySQL session execute the actual target operations.
        for side, conn in connections.items():
            obs = query(conn, 'SELECT 1::int4 AS v')
            archive('pg-probe', side, label, 'query', obs)
            check(obs['status'] == 2 and obs['rows'] == [['31']], f'{side}/{label}: PG recovery/isolation')
            probes += 1
        for side, mysql in mysqls.items():
            obs = mysql.query('SELECT abs(CAST(-17 AS SIGNED)) AS v')
            archive('mysql-probe', side, label, 'query', obs)
            check(obs['code'] == 0 and obs['rows'] == [['3137']], f'{side}/{label}: MySQL isolation')
            probes += 1
    def wire_keep(side, case, phase, obs):
        return archive('wire', side, case, phase, obs)
    def finish_error(wire, side, case, phase, obs):
        if not obs['errors']:
            return False
        check(all(e.get(k) for e in obs['errors'] for k in 'SVCM'), f'{side}/{case}: complete error fields')
        check(not obs['metadata'] and not obs['rows'], f'{side}/{case}: no false success payload')
        wire_keep(side, case, phase + '-sync', wire.sync())
        recovery = wire_keep(side, case, phase + '-recovery', wire.query('SELECT 1::int4 AS v'))
        check(recovery['rows'] == [['31']] and recovery['ready'] == 'I', f'{side}/{case}: wire recovery')
        return True
    try:
        repository = Path(__file__).resolve().parents[3]
        (evidence / 'build.json').write_text(json.dumps(dict(
            head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repository, text=True).strip(),
            fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            cargo_lock_sha256=hashlib.sha256((repository / 'Cargo.lock').read_bytes()).hexdigest(),
            source_diff_sha256=hashlib.sha256(subprocess.check_output(
                ['git', 'diff', 'HEAD', '--', 'pkg/server'], cwd=repository)).hexdigest(),
            rust_binary=os.environ.get('PG_FUNCTION23_RUST_BINARY'),
            rust_binary_sha256=(hashlib.sha256(Path(os.environ['PG_FUNCTION23_RUST_BINARY']).read_bytes()).hexdigest()
                                if os.environ.get('PG_FUNCTION23_RUST_BINARY') else None))))
        for side, dsn in dsns:
            conn = pq.PQconnectdb(dsn.encode())
            assert conn
            connections[side] = conn
            assert pq.PQstatus(conn) == 0, f'{side} connection failed (DSN redacted)'
            identity = {sql: query(conn, sql) for sql in (
                'SELECT version()', 'SHOW server_version_num', 'SHOW client_encoding',
                'SHOW server_encoding', 'SHOW standard_conforming_strings')}
            (evidence / f'pg-{side}-identity.json').write_text(json.dumps(
                dict(libpq=pq.PQlibVersion(), observations=identity)))
            if side == 'reference' and len(dsns) == 2:
                check(identity['SHOW server_version_num']['rows'] == [['313830303036']],
                      'PG18.6 target unavailable; other versions are observations only')
            if side == 'reference':
                owned = query(conn, "SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE p.proname IN ('abs','round','substr','date_trunc') AND n.nspname NOT IN ('pg_catalog','information_schema')")
                check(owned['rows'] == [['30']], 'reference has user overloads')
            wires[side] = Wire(pq, dsn)
        if os.environ.get('MYSQL8_REFERENCE_PORT'):
            mysqls['reference'] = Mysql(os.environ['MYSQL8_REFERENCE_HOST'], int(os.environ['MYSQL8_REFERENCE_PORT']),
                                        os.environ['MYSQL8_REFERENCE_USER'], os.environ['MYSQL8_REFERENCE_PASSWORD'])
        elif len(dsns) == 2:
            check(False, 'MySQL8 reference connection required')
        if aster_mysql:
            mysqls['aster'] = Mysql('127.0.0.1', aster_mysql[0], 'root', '', aster_mysql[1])
        if 'reference' in mysqls:
            # CREATE without IF NOT EXISTS ensures no existing object is adopted
            # or removed. Name is supplied by our unique same-server test DB.
            name = aster_mysql[1] if aster_mysql else f'function23_ref_{os.getpid()}'
            assert name.replace('_', '').isalnum()
            created = mysqls['reference'].query(f'CREATE DATABASE {name}')
            assert created['code'] == 0, 'cannot create owned reference database'
            reference_database = name
            assert mysqls['reference'].query(f'USE {name}')['code'] == 0
        for side, mysql in mysqls.items():
            identity = {sql: mysql.query(sql) for sql in ('SELECT version()', 'SELECT @@sql_mode', 'SELECT @@character_set_connection', 'SELECT @@collation_connection')}
            (evidence / f'mysql-{side}-identity.json').write_text(json.dumps(dict(
                client_version=mysql.mysql_get_client_version(), observations=identity)))
            setup = mysql.query('SET NAMES utf8mb4 COLLATE utf8mb4_bin')
            check(setup['code'] == 0, f'{side}: MySQL session charset')
        interleave('before')
        for case, sql, oid, expected in CASES:
            for side, conn in connections.items():
                obs = archive('pg', side, case, 'query', query(conn, sql))
                result_scenarios += 1
                if oid is None:
                    check(obs['status'] == 7 and obs['state'] == expected, f'{side}/{case}: rejection')
                    check(not obs['metadata'] and not obs['rows'], f'{side}/{case}: false payload')
                    check(all(obs['errors'][f] for f in 'SVCM'), f'{side}/{case}: error fields')
                else:
                    check(obs['status'] == 2 and obs['rows'] == [[None if expected is None else expected.encode().hex()]], f'{side}/{case}: value')
                    check(bool(obs['metadata']) and obs['metadata'][0][0] == oid, f'{side}/{case}: oid')
            interleave('query-' + case)
        for case, expression, oids, result_oid, width, bindings, expected_text in EXTENDED:
            for empty in (False, True):
                scenario = case + ('-empty' if empty else '-rows')
                sql = f'SELECT {expression} AS v' + (' WHERE false' if empty else '')
                name = ('function23_' + case.replace('-', '_') + str(int(empty))).encode()
                ready_sides = []
                for side, conn in connections.items():
                    prep = archive('pg', side, scenario, 'parse', pq.observe(pq.PQprepare(conn, name, sql.encode(), 0, None)))
                    check(prep['status'] == 1, f'{side}/{scenario}: libpq prepare')
                    if prep['status'] != 1:
                        # Actual rejection remains archived and fails comparison;
                        # do not manufacture downstream observations for a missing statement.
                        continue
                    desc = archive('pg', side, scenario, 'statement-describe', pq.observe(pq.PQdescribePrepared(conn, name)))
                    check(desc['parameters'] == oids and desc['metadata'] == [[result_oid, width, -1, 0, 'v', 0, 0]],
                          f'{side}/{scenario}: fixed libpq describe')
                    ready_sides.append(side)
                for input_fmt in (0, 1):
                    for output_fmt in (0, 1):
                        for index, values in enumerate(bindings):
                            label = f'{scenario}-{input_fmt}-{output_fmt}-{index}'
                            encoded = [payload(v, oid, input_fmt) for v, oid in zip(values, oids)]
                            n = len(encoded)
                            for side in ready_sides:
                                obs = archive('pg', side, label, 'execute', pq.observe(pq.PQexecPrepared(
                                    connections[side], name, n, (c.c_char_p * n)(*encoded),
                                    (c.c_int * n)(*[0 if v is None else len(v) for v in encoded]),
                                    (c.c_int * n)(*[input_fmt] * n), output_fmt)))
                                result_scenarios += 1
                                check(obs['status'] == 2, f'{side}/{label}: libpq execute')
                                check(obs['metadata'] == [[result_oid, width, -1, output_fmt, 'v', 0, 0]], f'{side}/{label}: libpq descriptor')
                                if empty:
                                    check(obs['rows'] == [], f'{side}/{label}: zero rows')
                                elif output_fmt == 0:
                                    expected = expected_text[index]
                                    check(obs['rows'] == [[None if expected is None else expected.encode().hex()]], f'{side}/{label}: libpq text value/NULL')
                                else:
                                    check(len(obs['rows']) == 1, f'{side}/{label}: binary row count')
                            interleave('libpq-' + label)
                # Raw supplemental statements have the same SQL and no invented
                # inferred OIDs: Parse supplies none; real ParameterDescription fixes them.
                wire_ready = []
                for side, wire in wires.items():
                    obs = wire_keep(side, scenario, 'parse', wire.phase(b'P', name + b'\0' + sql.encode() + b'\0\0\0', '1'))
                    check(obs['tags'] == '1', f'{side}/{scenario}: wire ParseComplete')
                    if finish_error(wire, side, scenario, 'parse', obs):
                        continue
                    desc = wire_keep(side, scenario, 'statement-describe', wire.phase(b'D', b'S' + name + b'\0', 'Tn'))
                    check(desc['parameters'] == oids and desc['metadata'] == [[result_oid, width, -1, 0, 'v', 0, 0]], f'{side}/{scenario}: raw statement describe')
                    if finish_error(wire, side, scenario, 'statement-describe', desc):
                        continue
                    wire_ready.append(side)
                for input_fmt in (0, 1):
                    for output_fmt in (0, 1):
                        for index, values in enumerate(bindings):
                            label = f'{scenario}-{input_fmt}-{output_fmt}-{index}'
                            encoded = [payload(v, oid, input_fmt) for v, oid in zip(values, oids)]
                            for side in wire_ready:
                                wire = wires[side]
                                body = b'\0' + name + b'\0' + struct.pack('!HHH', 1, input_fmt, len(encoded))
                                body += b''.join(struct.pack('!i', -1 if v is None else len(v)) + (v or b'') for v in encoded)
                                body += struct.pack('!HH', 1, output_fmt)
                                obs = wire_keep(side, label, 'bind', wire.phase(b'B', body, '2'))
                                check(obs['tags'] == '2', f'{side}/{label}: BindComplete')
                                if finish_error(wire, side, label, 'bind', obs):
                                    continue
                                desc = wire_keep(side, label, 'portal-describe', wire.phase(b'D', b'P\0', 'Tn'))
                                check(desc['metadata'] == [[result_oid, width, -1, output_fmt, 'v', 0, 0]], f'{side}/{label}: portal describe')
                                if finish_error(wire, side, label, 'portal-describe', desc):
                                    continue
                                wire.send(b'E', b'\0\0\0\0\0')
                                wire.send(b'S')
                                result = wire.read('Z')
                                if result['errors']:
                                    result['tail'] = wire.read('Z')
                                obs = wire_keep(side, label, 'execute', result)
                                result_scenarios += 1
                                check(obs['tags'] == ('CZ' if empty else 'DCZ') and not obs['metadata'], f'{side}/{label}: Execute no extra RowDescription')
                                check(obs['ready'] == 'I' and not obs['errors'], f'{side}/{label}: Execute ready/error')
                                if empty:
                                    check(obs['rows'] == [], f'{side}/{label}: raw zero rows')
                                elif output_fmt == 0:
                                    expected = expected_text[index]
                                    check(obs['rows'] == [[None if expected is None else expected.encode().hex()]], f'{side}/{label}: raw value')
                                elif values[0] is None:
                                    check(obs['rows'] == [[None]] and obs['lengths'] == [[-1]], f'{side}/{label}: raw NULL length')
                            interleave('wire-' + label)
                for side in wire_ready:
                    obs = wire_keep(side, scenario, 'close', wires[side].phase(b'C', b'S' + name + b'\0', '3'))
                    check(obs['tags'] == '3', f'{side}/{scenario}: close statement')
                    wire_keep(side, scenario, 'close-sync', wires[side].sync())
        # Both Q and Parse must reject bad overloads, with recovery before the
        # other PG and same-server MySQL sessions resume their probes.
        for case, sql, oid, expected in CASES:
            if case not in ('abs-text', 'abs-empty', 'abs-text-utf8'):
                continue
            for side, wire in wires.items():
                obs = wire_keep(side, case, 'reject-parse', wire.phase(b'P', b'function23_reject\0' + sql.encode() + b'\0\0\0', '1'))
                check(obs['errors'] and obs['errors'][0].get('C') == expected, f'{side}/{case}: parse SQLSTATE')
                finish_error(wire, side, case, 'reject-parse', obs)
            interleave('parse-reject-' + case)
        for case, sql in MYSQL_CASES:
            for side, mysql in mysqls.items():
                obs = archive('mysql', side, case, 'query', mysql.query(sql))
                result_scenarios += 1
                if case in ('abs-empty', 'date-hidden'):
                    expected_code = 1582 if case == 'abs-empty' else 1305
                    check(obs['code'] == expected_code and not obs['metadata'] and not obs['rows'], f'{side}/{case}: MySQL rejection')
                else:
                    expected = {'abs-signed': '17', 'abs-null': None, 'round-implicit': '42',
                                'substr-text': 'bcd', 'abs-typed-text': '1'}[case]
                    check(obs['code'] == 0 and obs['rows'] == [[None if expected is None else expected.encode().hex()]],
                          f'{side}/{case}: MySQL value/NULL')
            interleave('mysql-' + case)
        for case, expression in (
            ('abs', 'abs(CAST(? AS SIGNED))'),
            ('round', 'round(CAST(? AS SIGNED), 2)'),
            ('substr', 'substr(?)'),
        ):
            # SUBSTR uses unknown/string parameter while positions are fixed.
            if case == 'substr':
                expression = 'substr(?, 2, 3)'
            for empty, null_first in ((False, False), (True, False), (False, True), (True, True)):
                sql = f'SELECT {expression} AS v' + (' WHERE false' if empty else '')
                scenario = case + ('-prepared-empty' if empty else '-prepared-rows') + ('-null-first' if null_first else '')
                for side, mysql in mysqls.items():
                    values = [b'-17', b'2', None] if case != 'substr' else [b'abcdef', b'', None]
                    if null_first:
                        values = values[-1:] + values[:-1]
                    observations = mysql.prepared(sql, values)
                    for index, obs in enumerate(observations):
                        archive('mysql', side, scenario, str(index), obs)
                        check(obs['code'] == 0, f'{side}/{scenario}: MySQL prepared success')
                        if index:
                            result_scenarios += 1
                            expected = [b'17', b'2', None] if case == 'abs' else [b'-17', b'2', None] if case == 'round' else [b'bcd', b'', None]
                            if null_first:
                                expected = expected[-1:] + expected[:-1]
                            check(obs['rows'] == ([] if empty else [[None if expected[index-1] is None else expected[index-1].hex()]]),
                                  f'{side}/{scenario}/{index}: MySQL prepared value/NULL/zero rows')
                interleave('mysql-' + scenario)
        for case, sql in MYSQL_CASES:
            if case not in ('abs-empty', 'date-hidden'):
                continue
            for side, mysql in mysqls.items():
                obs = mysql.prepared(sql, [])[0]
                archive('mysql', side, case, 'reject-prepare', obs)
                check(obs['code'] == (1582 if case == 'abs-empty' else 1305) and not obs['metadata'],
                      f'{side}/{case}: MySQL prepare rejection')
            interleave('mysql-reject-prepare-' + case)
        interleave('after')
        if len(dsns) == 2:
            for (domain, case, phase), observations in records.items():
                if set(observations) != {'reference', 'aster'}:
                    check(False, f'{domain}/{case}/{phase}: missing real peer observation')
                    continue
                comparator = compare_wire if domain == 'wire' else compare_mysql if domain.startswith('mysql') else compare
                assertions += 1
                try:
                    comparator(observations['reference'], observations['aster'])
                except AssertionError as error:
                    failures.append(f'{domain}/{case}/{phase}: {error}')
    except BaseException as error:
        failures.append('client aborted: ' + type(error).__name__)
        raise
    finally:
        for wire in wires.values():
            wire.close()
        for conn in connections.values():
            pq.PQfinish(conn)
        if reference_database:
            dropped = mysqls['reference'].query(f'DROP DATABASE {reference_database}')
            cleanup['mysql_reference_database_dropped'] = dropped['code'] == 0
            check(dropped['code'] == 0, 'owned MySQL reference database cleanup')
        for mysql in mysqls.values():
            mysql.close()
        summary = dict(observation_keys=len(records), result_scenarios=result_scenarios,
                       interleaved_probes=probes, assertions=assertions, negative_controls=matrix_self_test(),
                       failures=failures, pg_connections_closed=len(connections) + len(wires),
                       mysql_connections_closed=len(mysqls), cleanup=cleanup,
                       scope='bounded signatures only; exact18.6 and each signature require all comparisons')
        (evidence / 'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary), flush=True)
    assert records and assertions and not failures, 'registration differential failed'


if __name__ == '__main__':
    def terminate(signum, frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, terminate)
    if sys.argv[1:] == ['--self-test']:
        print(json.dumps(dict(negative_controls=matrix_self_test(), query_scenarios=len(CASES),
                              extended_signatures=len(EXTENDED), mysql_query_scenarios=len(MYSQL_CASES))))
    else:
        reference = os.environ.get('PG18_REFERENCE_DSN')
        assert reference, 'PG18_REFERENCE_DSN required (no skip)'
        if sys.argv[1] == '--reference-only':
            run([('reference', reference)], Path(sys.argv[2]))
        else:
            port, database, evidence, mysql_port = sys.argv[1:]
            run([('reference', reference), ('aster',
                 f'host=127.0.0.1 port={int(port)} user=root dbname={database} connect_timeout=10')],
                Path(evidence), (int(mysql_port), database))
