# Copyright 2026 AsterSQL.
"""Independent signed ABS differential; services belong to the caller.

Uses the import-safe task23 native client bindings, never its test entrypoint.
PG18_REFERENCE_DSN/PG_LIBPQ_LIBRARY and MYSQL8_REFERENCE_HOST/PORT/USER/PASSWORD/
MYSQL8_CLIENT_LIBRARY have the same contracts as task23. No secrets are logged.
--reference-only EVIDENCE records the actual version, without equivalence claims.
Default arguments: PORT DATABASE EVIDENCE MYSQL_PORT. Exact18.6 remains required.
"""
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import signal
import struct
import subprocess
import sys

from pg_function_23_libpq import (
    Libpq, Wire, Mysql as NativeMysql, MysqlBind, compare, compare_wire, compare_mysql, matrix_self_test, payload,
)


class Mysql(NativeMysql):
    """Extend the verified C ABI for ABS errors deferred until result buffering."""
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
        obs['code'] = self.mysql_errno(self.conn)
        obs['state'] = self.mysql_sqlstate(self.conn).decode()
        obs['warning_count'] = self.mysql_warning_count(self.conn)
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
                                     parameters=self.mysql_stmt_param_count(stmt)))
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
                    stored = self.mysql_stmt_store_result(stmt)
                    if stored:
                        # Native execute may receive metadata before evaluation fails
                        # during result buffering. Keep the actual descriptor and stage.
                        obs['code'] = self.mysql_stmt_errno(stmt)
                        obs['state'] = self.mysql_stmt_sqlstate(stmt).decode()
                        obs['warning_count'] = self.mysql_warning_count(self.conn)
                        self.mysql_stmt_free_result(stmt)
                        obs['warnings'] = self.query('SHOW WARNINGS', warnings=False)['rows']
                        observations.append(obs)
                        continue
                    while True:
                        rc = self.mysql_stmt_fetch(stmt)
                        if rc == 100:  # MYSQL_NO_DATA
                            break
                        assert rc == 0 and not any(e.value for e in errors), 'native result truncation/error'
                        obs['rows'].append([None if nulls[i].value else buffers[i].raw[:lengths[i].value].hex()
                                            for i in range(len(buffers))])
                    assert not self.mysql_stmt_free_result(stmt)
                obs['warnings'] = self.query('SHOW WARNINGS', warnings=False)['rows']
                observations.append(obs)
            return observations
        finally:
            assert not self.mysql_stmt_close(stmt)


DOMAINS = ((23, 4, 'int4', (-17, 0, 17, -2147483647, 2147483647, None, -2147483648, 2)),
           (20, 8, 'int8', (-17, 0, 17, -9007199254740993, -9223372036854775807,
                           9223372036854775807, None, -9223372036854775808, 2)))
MYSQL_VALUES = (-17, 0, 17, -9007199254740993, -9223372036854775807,
                9223372036854775807, None, -9223372036854775808, 2)


def run(dsns, evidence, aster_mysql=None):
    pq = Libpq()
    evidence.mkdir(parents=True, exist_ok=False)
    connections, wires, mysqls, records, failures = {}, {}, {}, {}, []
    counts = dict(assertions=0, result_scenarios=0, interleaved_probes=0)
    reference_database = None
    cleanup = {}

    def check(ok, label):
        counts['assertions'] += 1
        if not ok:
            failures.append(label)

    def archive(domain, side, case, phase, obs):
        peers = records.setdefault((domain, case, phase), {})
        assert side not in peers, 'duplicate observation'
        peers[side] = obs
        with (evidence / f'{domain}-{side}.jsonl').open('a') as stream:
            stream.write(json.dumps(dict(case_id=case, phase=phase, observation=obs)) + '\n')
        return obs

    def query(conn, sql):
        return pq.observe(pq.PQexec(conn, sql.encode()))

    def probe(label):
        for side, conn in connections.items():
            obs = archive('pg-probe', side, label, 'query', query(conn, 'SELECT 1::int4 AS v'))
            check(obs['status'] == 2 and obs['rows'] == [['31']], f'{side}/{label}: PG recovery')
            counts['interleaved_probes'] += 1
        for side, mysql in mysqls.items():
            obs = archive('mysql-probe', side, label, 'query', mysql.query('SELECT abs(CAST(-17 AS SIGNED)) AS v'))
            check(obs['code'] == 0 and obs['rows'] == [['3137']], f'{side}/{label}: MySQL recovery')
            counts['interleaved_probes'] += 1

    def pg_result(obs, side, label, oid, width, value, fmt, empty):
        counts['result_scenarios'] += 1
        overflow = value == -(1 << (width * 8 - 1)) and not empty
        if overflow:
            check(obs['status'] == 7 and obs['state'] == '22003', f'{side}/{label}: overflow SQLSTATE')
            check(not obs['metadata'] and not obs['rows'], f'{side}/{label}: overflow no payload')
            check(all(obs['errors'][k] for k in 'SVCM'), f'{side}/{label}: diagnostics')
        else:
            expected = None if value is None or empty else payload(abs(value), oid, fmt).hex()
            check(obs['status'] == 2 and obs['rows'] == ([] if empty else [[expected]]), f'{side}/{label}: exact value/NULL/empty')
            check(obs['metadata'] == [[oid, width, -1, fmt, 'v', 0, 0]], f'{side}/{label}: metadata')

    try:
        repo = Path(__file__).resolve().parents[3]
        binary = os.environ.get('PG_FUNCTION24_RUST_BINARY')
        (evidence / 'build.json').write_text(json.dumps(dict(
            head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
            fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            bindings_sha256=hashlib.sha256(Path(__file__).with_name('pg_function_23_libpq.py').read_bytes()).hexdigest(),
            cargo_lock_sha256=hashlib.sha256((repo / 'Cargo.lock').read_bytes()).hexdigest(),
            source_diff_sha256=hashlib.sha256(subprocess.check_output(['git', 'diff', 'HEAD', '--', 'pkg/server'], cwd=repo)).hexdigest(),
            rust_binary=binary, rust_binary_sha256=hashlib.sha256(Path(binary).read_bytes()).hexdigest() if binary else None)))
        for side, dsn in dsns:
            conn = pq.PQconnectdb(dsn.encode())
            assert conn
            connections[side] = conn
            assert pq.PQstatus(conn) == 0, f'{side}: connection failed (DSN redacted)'
            identity = {sql: query(conn, sql) for sql in (
                'SELECT version()', 'SHOW server_version_num', 'SHOW client_encoding',
                'SHOW server_encoding', 'SHOW standard_conforming_strings')}
            (evidence / f'pg-{side}-identity.json').write_text(json.dumps(dict(libpq=pq.PQlibVersion(), observations=identity)))
            if side == 'reference':
                if len(dsns) == 2:
                    check(identity['SHOW server_version_num']['rows'] == [['313830303036']], 'PG18.6 unavailable; actual version archived separately')
                obs = query(conn, "SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE p.proname='abs' AND n.nspname NOT IN ('pg_catalog','information_schema')")
                check(obs['rows'] == [['30']], 'reference user ABS overloads')
            wires[side] = Wire(pq, dsn)
        if aster_mysql:
            mysqls['aster'] = Mysql('127.0.0.1', aster_mysql[0], 'root', '', aster_mysql[1])
        if os.environ.get('MYSQL8_REFERENCE_PORT'):
            mysqls['reference'] = Mysql(os.environ['MYSQL8_REFERENCE_HOST'], int(os.environ['MYSQL8_REFERENCE_PORT']),
                                       os.environ['MYSQL8_REFERENCE_USER'], os.environ['MYSQL8_REFERENCE_PASSWORD'])
            name = aster_mysql[1] if aster_mysql else f'function24_ref_{os.getpid()}'
            assert name.replace('_', '').isalnum()
            assert mysqls['reference'].query(f'CREATE DATABASE {name}')['code'] == 0
            reference_database = name
            assert mysqls['reference'].query(f'USE {name}')['code'] == 0
        else:
            check(False, 'MySQL8 reference required')
        for side, mysql in mysqls.items():
            identity = {sql: mysql.query(sql) for sql in ('SELECT version()', 'SELECT @@sql_mode', 'SELECT @@character_set_connection', 'SELECT @@collation_connection')}
            (evidence / f'mysql-{side}-identity.json').write_text(json.dumps(dict(client_version=mysql.mysql_get_client_version(), observations=identity)))
            check(mysql.query('SET NAMES utf8mb4 COLLATE utf8mb4_bin')['code'] == 0, f'{side}: charset setup')
        probe('before')
        for oid, width, typename, values in DOMAINS:
            for index, value in enumerate(values):
                label = f'{typename}-q-{index}'
                literal = 'NULL' if value is None else f"'{value}'"
                sql = f'SELECT pg_catalog.abs({literal}::{typename}) AS v'
                for side, conn in connections.items():
                    obs = archive('pg', side, label, 'query', query(conn, sql))
                    pg_result(obs, side, label, oid, width, value, 0, False)
                probe(label)
            for empty in (False, True):
                name = f'function24_{typename}_{int(empty)}'.encode()
                sql = f'SELECT pg_catalog.abs($1::{typename}) AS v' + (' WHERE false' if empty else '')
                for side, conn in connections.items():
                    obs = archive('pg', side, name.decode(), 'parse', pq.observe(pq.PQprepare(conn, name, sql.encode(), 0, None)))
                    assert obs['status'] == 1, 'ABS prepare required'
                    desc = archive('pg', side, name.decode(), 'statement-describe', pq.observe(pq.PQdescribePrepared(conn, name)))
                    check(desc['parameters'] == [oid] and desc['metadata'] == [[oid, width, -1, 0, 'v', 0, 0]], f'{side}: fixed signature')
                    wire = wires[side]
                    parsed = archive('wire', side, name.decode(), 'parse', wire.phase(b'P', name+b'\0'+sql.encode()+b'\0\0\0', '1'))
                    assert parsed['tags'] == '1', 'raw ABS prepare required'
                    desc = archive('wire', side, name.decode(), 'statement-describe', wire.phase(b'D', b'S'+name+b'\0', 'Tn'))
                    check(desc['parameters'] == [oid] and desc['metadata'] == [[oid, width, -1, 0, 'v', 0, 0]], f'{side}: raw fixed signature')
                for infmt in (0, 1):
                    for outfmt in (0, 1):
                        for index, value in enumerate(values):
                            label = f'{name.decode()}-{infmt}-{outfmt}-{index}'
                            encoded = payload(value, oid, infmt)
                            overflow = value == -(1 << (width * 8 - 1)) and not empty
                            for side, conn in connections.items():
                                obs = archive('pg', side, label, 'execute', pq.observe(pq.PQexecPrepared(conn, name, 1,
                                    (c.c_char_p*1)(encoded), (c.c_int*1)(0 if encoded is None else len(encoded)),
                                    (c.c_int*1)(infmt), outfmt)))
                                pg_result(obs, side, label, oid, width, value, outfmt, empty)
                                wire = wires[side]
                                body = b'\0'+name+b'\0'+struct.pack('!HHHi', 1, infmt, 1, -1 if encoded is None else len(encoded))+(encoded or b'')+struct.pack('!HH', 1, outfmt)
                                bind = archive('wire', side, label, 'bind', wire.phase(b'B', body, '2'))
                                check(bind['tags'] == '2', f'{side}/{label}: BindComplete')
                                if bind['errors']:
                                    archive('wire', side, label, 'bind-sync', wire.sync())
                                    continue
                                desc = archive('wire', side, label, 'portal-describe', wire.phase(b'D', b'P\0', 'Tn'))
                                check(desc['metadata'] == [[oid, width, -1, outfmt, 'v', 0, 0]], f'{side}/{label}: portal identity')
                                wire.send(b'E', b'\0\0\0\0\0'); wire.send(b'S')
                                result = wire.read('Z')
                                if result['errors']:
                                    result['tail'] = wire.read('Z')
                                archive('wire', side, label, 'execute', result)
                                counts['result_scenarios'] += 1
                                check(not result['metadata'], f'{side}/{label}: Execute has no descriptor')
                                if overflow:
                                    check(len(result['errors']) == 1 and result['errors'][0].get('C') == '22003' and not result['rows'], f'{side}/{label}: raw overflow')
                                    check(all(result['errors'][0].get(k) for k in 'SVCM') and result['tail']['ready'] == 'I', f'{side}/{label}: error diagnostics/Sync')
                                else:
                                    expected = None if value is None or empty else payload(abs(value), oid, outfmt).hex()
                                    check(result['tags'] == ('CZ' if empty else 'DCZ') and result['rows'] == ([] if empty else [[expected]]), f'{side}/{label}: raw exact result')
                                    check(result['lengths'] == ([] if empty else [[-1 if value is None else len(bytes.fromhex(expected))]]), f'{side}/{label}: raw value lengths')
                                recovery = archive('wire', side, label, 'recovery', wire.query('SELECT 1::int4 AS v'))
                                check(recovery['rows'] == [['31']] and recovery['ready'] == 'I', f'{side}/{label}: raw recovery')
                            probe(label)
                for side, wire in wires.items():
                    archive('wire', side, name.decode(), 'close', wire.phase(b'C', b'S'+name+b'\0', '3'))
                    archive('wire', side, name.decode(), 'close-sync', wire.sync())
        for index, value in enumerate(MYSQL_VALUES):
            label = f'mysql-q-{index}'
            literal = 'NULL' if value is None else str(value)
            for side, mysql in mysqls.items():
                obs = archive('mysql', side, label, 'query', mysql.query(f'SELECT abs(CAST({literal} AS SIGNED)) AS v'))
                counts['result_scenarios'] += 1
                overflow = value == -9223372036854775808
                check(obs['code'] == (1690 if overflow else 0), f'{side}/{label}: code')
                check(obs['state'] == ('22003' if overflow else '00000'), f'{side}/{label}: SQLSTATE')
                check(obs['rows'] == ([] if overflow else [[None if value is None else str(abs(value)).encode().hex()]]), f'{side}/{label}: exact signed value')
                if overflow:
                    check(not obs['metadata'], f'{side}/{label}: no false error description')
            probe(label)
        for empty in (False, True):
            label = f'mysql-prepared-{int(empty)}'
            sql = 'SELECT abs(CAST(? AS SIGNED)) AS v' + (' WHERE false' if empty else '')
            values = [None if v is None else str(v).encode() for v in MYSQL_VALUES]
            for side, mysql in mysqls.items():
                observations = mysql.prepared(sql, values)
                check(len(observations) == len(values)+1, f'{side}/{label}: all rebindings executed')
                for index, obs in enumerate(observations):
                    archive('mysql', side, label, str(index), obs)
                    if index == 0:
                        check(obs['code'] == 0 and obs['parameters'] == 1, f'{side}/{label}: Prepare')
                        continue
                    counts['result_scenarios'] += 1
                    value = MYSQL_VALUES[index-1]
                    overflow = value == -9223372036854775808 and not empty
                    check(obs['code'] == (1690 if overflow else 0) and obs['state'] == ('22003' if overflow else '00000'), f'{side}/{label}/{index}: execution state')
                    expected = [] if empty or overflow else [[None if value is None else str(abs(value)).encode().hex()]]
                    check(obs['rows'] == expected, f'{side}/{label}/{index}: exact value/empty')
                    if overflow:
                        check(not obs['rows'], f'{side}/{label}/{index}: no success row after error')
            probe(label)
        probe('after')
        if len(dsns) == 2:
            for (domain, case, phase), peers in records.items():
                if set(peers) != {'reference', 'aster'}:
                    check(False, f'{domain}/{case}/{phase}: missing peer')
                    continue
                comparator = compare_wire if domain == 'wire' else compare_mysql if domain.startswith('mysql') else compare
                counts['assertions'] += 1
                try:
                    comparator(peers['reference'], peers['aster'])
                except AssertionError as error:
                    failures.append(f'{domain}/{case}/{phase}: {error}')
    except BaseException as error:
        failures.append('client aborted: '+type(error).__name__)
        raise
    finally:
        for wire in wires.values():
            wire.close()
        for conn in connections.values():
            pq.PQfinish(conn)
        if reference_database:
            cleanup['reference_database_dropped'] = mysqls['reference'].query(f'DROP DATABASE {reference_database}')['code'] == 0
            check(cleanup['reference_database_dropped'], 'reference database cleanup')
        for mysql in mysqls.values():
            mysql.close()
        summary = dict(**counts, observation_keys=len(records), negative_controls=matrix_self_test(),
                       failures=failures, pg_connections_closed=len(connections)+len(wires),
                       mysql_connections_closed=len(mysqls), cleanup=cleanup,
                       scope='signed ABS int4/int8 only; no shared alias or pushdown approval')
        (evidence / 'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary), flush=True)
    assert records and counts['result_scenarios'] and not failures, 'signed ABS differential failed'


if __name__ == '__main__':
    def terminate(signum, frame):
        raise SystemExit(128+signum)
    signal.signal(signal.SIGTERM, terminate)
    if sys.argv[1:] == ['--self-test']:
        print(json.dumps(dict(negative_controls=matrix_self_test(), pg_query_cases=sum(len(d[3]) for d in DOMAINS),
                              signatures=len(DOMAINS), mysql_query_cases=len(MYSQL_VALUES))))
    else:
        reference = os.environ['PG18_REFERENCE_DSN']
        if sys.argv[1] == '--reference-only':
            run([('reference', reference)], Path(sys.argv[2]))
        else:
            port, database, evidence, mysql_port = sys.argv[1:]
            run([('reference', reference), ('aster', f'host=127.0.0.1 port={int(port)} user=root dbname={database} connect_timeout=10')],
                Path(evidence), (int(mysql_port), database))
