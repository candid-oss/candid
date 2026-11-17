#!/usr/bin/env python3
"""Compare two core binaries on identical synthetic sync input, without changing the installed App.

Creates isolated roots with originals, transcript consent and disabled model features of their own.
Power domains cover the whole machine; per-process counters cover only each benchmark core.
"""
import argparse
import hashlib
import http.client
import json
import pathlib
import shutil
import socket
import sqlite3
import subprocess
import time
import tempfile
import uuid
import wave

SCRIPT = pathlib.Path(__file__).resolve().parent


class UnixHTTP(http.client.HTTPConnection):
    def __init__(self, path):
        super().__init__('localhost', timeout=90)
        self.path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(str(self.path))


def request(sock, path, value=None):
    c = UnixHTTP(sock)
    try:
        c.request('GET' if value is None else 'POST', path,
                  None if value is None else json.dumps(value).encode(), {'Content-Type': 'application/json'})
        r = c.getresponse()
        data = r.read()
        if r.status not in (200, 202):
            raise RuntimeError(f'{path}: {r.status}: {data[:300]!r}')
        return json.loads(data)
    finally:
        c.close()


def start(binary, root, run):
    run.mkdir()
    short_run = pathlib.Path(tempfile.mkdtemp(prefix='p-sync-'))
    sock = short_run / 'core.sock'
    log = (run / 'core.log').open('w')
    p = subprocess.Popen([str(binary), 'serve', '--root', str(root), '--run-dir', str(run),
                          '--socket', str(sock), '--sync-interval', '3600'], stdout=log, stderr=log)
    p.benchmark_socket_dir = short_run
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        if p.poll() is not None:
            raise RuntimeError(f'core exited: {run / "core.log"}')
        if sock.exists():
            s = request(sock, '/local/status')
            if s.get('last_sync_ms') and not s['syncing']:
                return p, sock, log
        time.sleep(.1)
    raise RuntimeError('core did not become ready')


def stop(p, log):
    p.terminate()
    p.wait(timeout=30)
    log.close()
    shutil.rmtree(p.benchmark_socket_dir)


def sync(sock):
    before = request(sock, '/local/status')['last_sync_ms']
    request(sock, '/local/sync', {})
    until = time.monotonic() + 90
    while time.monotonic() < until:
        s = request(sock, '/local/status')
        if not s['syncing'] and s['last_sync_ms'] > before:
            return s
        time.sleep(.05)
    raise RuntimeError('sync did not finish')


def line(typ, payload, n=0):
    return {'timestamp': f'2026-11-17T00:{n // 60:02}:{n % 60:02}Z', 'type': typ, 'payload': payload}


def turn(n, chars=1024):
    return [line('event_msg', {'type': 'user_message', 'message': f'SYNTHETIC request {n} ' + 'x' * chars}, n),
            line('response_item', {'type': 'message', 'id': f'SYNTHETIC-reply-{n}', 'role': 'assistant',
                                 'content': [{'type': 'output_text', 'text': 'SYNTHETIC reply, never persisted'}]}, n),
            line('event_msg', {'type': 'task_complete'}, n)]


def write_lines(path, lines, mode='w'):
    with path.open(mode) as f:
        for v in lines:
            f.write(json.dumps(v, separators=(',', ':')) + '\n')


def summarize(root):
    db = sqlite3.connect(root / 'timeline.sqlite')
    # Stable evidence contract: keys, original times, titles and admitted text, independent of generated IDs.
    rows = db.execute("SELECT e.source_key,e.kind,e.start_ms,e.end_ms,e.title,t.field,t.ord,c.text "
                      "FROM events e LEFT JOIN event_text t USING(event_id) LEFT JOIN text_chunks c USING(chunk_id) "
                      "ORDER BY e.source_key,e.kind,t.field,t.ord").fetchall()
    result = {'evidence_sha256': hashlib.sha256(json.dumps(rows, ensure_ascii=False).encode()).hexdigest(),
              'events': db.execute('SELECT count(*) FROM events').fetchone()[0],
              'prompt_count': db.execute("SELECT count(*) FROM events WHERE kind='ai.prompt'").fetchone()[0],
              'integrity': db.execute('PRAGMA integrity_check').fetchone()[0],
              'cursor_bytes': db.execute('SELECT sum(length(cursor)) FROM source_cursors').fetchone()[0],
              'model_calls': db.execute('SELECT count(*) FROM ai_calls').fetchone()[0]}
    db.close()
    return result


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--before', type=pathlib.Path, required=True)
    ap.add_argument('--after', type=pathlib.Path, required=True)
    ap.add_argument('--power-tool', type=pathlib.Path, required=True)
    ap.add_argument('--output-dir', type=pathlib.Path, required=True)
    args = ap.parse_args()
    out = args.output_dir.resolve()
    out.mkdir(parents=True, exist_ok=False)
    before, after = args.before.resolve(), args.after.resolve()
    transcript_dir = out / 'SYNTHETIC-transcripts'
    transcript_dir.mkdir()
    transcript = transcript_dir / 'active.jsonl'
    base_lines = [line('session_meta', {'id': 'SYNTHETIC-active', 'cwd': '/SYNTHETIC/project'})]
    for i in range(1000):
        base_lines.extend(turn(i))
    write_lines(transcript, base_lines)
    for i in range(300):
        write_lines(transcript_dir / f'history-{i}.jsonl', [line('session_meta', {'id': f'SYNTHETIC-history-{i}'})])
    (transcript_dir / 'invalid.jsonl').write_text('{SYNTHETIC incomplete invalid JSON}\n')
    seed = out / 'seed'
    subprocess.run([str(before), 'serve', '--root', str(seed), '--run-dir', str(out / 'seed-once'), '--once'],
                   check=True, capture_output=True)
    db = sqlite3.connect(seed / 'timeline.sqlite')
    catalog = json.loads((SCRIPT.parent.parent / 'core/workflows/catalog.json').read_text())
    settings = {'version': 1, 'items': {f['id']: {'enabled': False, 'sources': f['sources'],
                'allow_remote': False, 'daily_calls': 0} for f in catalog['features']
                if f['implemented'] and f['mode'] != 'agent'}}
    db.execute("INSERT OR REPLACE INTO store_meta VALUES('features.settings/1',?)", (json.dumps(settings),))
    db.commit()
    db.close()
    p, sock, log = start(before, seed, out / 'seed-run')
    try:
        source = request(sock, '/local/sources', {'kind': 'manual', 'display_name': 'SYNTHETIC filler'})
        sid = source.get('source_id', source.get('source', {}).get('source_id'))
        if not sid:
            raise RuntimeError(f'Unexpected source shape: {source}')
        for batch in range(10):
            events = [{'source_key': f'SYNTHETIC-filler-{batch}-{i}', 'kind': 'speech.segment' if i < 100 else 'note',
                       'start_ms': 1794873600000, 'payload': {'recording_event_id': 'SYNTHETIC-reference'},
                       'title': 'SYNTHETIC unrelated evidence'} for i in range(500)]
            request(sock, '/local/ingest', {'source_id': sid, 'events': events})
        audio = out / 'SYNTHETIC.wav'
        with wave.open(str(audio), 'wb') as w:
            w.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
            w.writeframes(b'\0' * 32000)
        for i in range(24):
            request(sock, '/local/media', {'action': 'import', 'id': str(uuid.UUID(int=i + 1)),
                    'filename': 'SYNTHETIC.wav', 'kind': 'audio', 'source': 'import', 'path': str(audio),
                    'occurred_at': '2026-11-17T00:00:00Z'})
        request(sock, '/local/agents/consent', {'consent': {'version': 1, 'agent': 'codex', 'read': True,
                'search': True, 'activity': True, 'paths': [str(transcript_dir)]}})
        sync(sock)
    finally:
        stop(p, log)
    reports = {'binary_sha256': {label: hashlib.sha256(binary.read_bytes()).hexdigest() for label, binary in [('before', before), ('after', after)]}}
    for label, binary in [('before', before), ('after', after)]:
        write_lines(transcript, base_lines)
        root = out / label
        shutil.copytree(seed, root)
        p, sock, log = start(binary, root, out / (label + '-run'))
        try:
            sync(sock)  # Warm caches and convert any inline legacy checkpoints before steady measurement.
            sampler = subprocess.Popen(['python3', str(SCRIPT / 'measure_pipeline.py'), '--label', f'SYNTHETIC-sync-{label}',
                        '--seconds', '60', '--pid', str(p.pid), '--power-tool', str(args.power_tool.resolve()),
                        '--output', str(out / (label + '.json'))], stdout=(out / (label + '-sampler.log')).open('w'))
            time.sleep(3.5)
            timings = []
            begun = time.monotonic()
            for i in range(8):
                delay = begun + 6 * i - time.monotonic()
                if delay > 0:
                    time.sleep(delay)
                write_lines(transcript, turn(1000 + i, 4096), 'a')
                t = time.monotonic()
                status = sync(sock)
                timings.append({'step': i, 'sync_seconds': time.monotonic() - t, 'problems': status['problems']})
            if sampler.wait(timeout=90):
                raise RuntimeError('power sampler failed')
            reports[label] = {'core_pid': p.pid, 'steps': timings}
        finally:
            stop(p, log)
        reports[label].update(summarize(root))
    reports['same_evidence'] = reports['before']['evidence_sha256'] == reports['after']['evidence_sha256']
    (out / 'comparison.json').write_text(json.dumps(reports, indent=2))
    print(json.dumps(reports, indent=2))
    if not reports['same_evidence'] or reports['before']['model_calls'] or reports['after']['model_calls']:
        raise RuntimeError('evidence differs or unexpected model work')


if __name__ == '__main__':
    main()
