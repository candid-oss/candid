#!/usr/bin/env python3
"""Isolated HTTP/WebSocket fixture. Synthetic content only; no device imports."""
import argparse
import base64
import hashlib
import io
import json
import math
import pathlib
import struct
import threading
import time
import wave
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = pathlib.Path(__file__).resolve().parents[1]
TOKEN = 'isolated-fixture-only'

class Fixture:
    def __init__(self):
        self.lock = threading.RLock()
        self.state = json.loads((ROOT / 'Sources/ConsoleCore/Resources/MockState.json').read_text())
        self.offline = False
        self.reads = self.writes = self.sockets = 0
        self.started = None

    def snapshot(self):
        with self.lock:
            recording = self.state['serial_device']['status']['recording']
            if self.started is not None:
                elapsed = time.monotonic() - self.started
                recording['duration_ms'] = int(elapsed * 1000)
                if elapsed >= recording.get('max_seconds', 15):
                    recording.update(state='ready', stop_reason='max_duration')
                    self.started = None
            return json.loads(json.dumps(self.state))

    def frame(self):
        with self.lock:
            snapshot = self.snapshot()
            now = datetime.now(timezone.utc).isoformat()
            monotonic_ms = time.monotonic() * 1000
            frame = dict(device_id='MOCK-ESP32', boot_id='fixture-boot', start_sample=int(monotonic_ms / 20) * 320,
                         buckets=[[-1200, 1800]] * 5, rms_dbfs=-26.5, clipped_samples=0,
                         monotonic_ms=monotonic_ms, recording=self.started is not None,
                         vad_state='speech' if int(time.monotonic()) % 2 else 'silence', received_at=now)
            frames = self.state['live_audio']['frames']
            frames.append(frame)
            self.state['live_audio']['frames'] = frames[-150:]
            return dict(type='audio.levels', frame=frame, gap_samples=0, serial_device=snapshot['serial_device'])

fixture = Fixture()

class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *_): pass
    def send_json(self, value, code=200):
        body = json.dumps(value, ensure_ascii=False).encode()
        self.send_response(code); self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)
    def do_GET(self):
        if self.path == '/__test/stats':
            return self.send_json(dict(reads=fixture.reads, writes=fixture.writes, sockets=fixture.sockets))
        if fixture.offline: return self.send_json({'detail': '隔离替身暂时离线'}, 503)
        if self.path == '/api/live': return self.websocket()
        with fixture.lock: fixture.reads += 1
        if self.path == '/api/config':
            return self.send_json(dict(session_token=TOKEN, protocol_version=1, schema_version=1, max_record_seconds=15))
        if self.path == '/api/state': return self.send_json(fixture.snapshot())
        if self.path.startswith('/api/assets/') and self.path.endswith('/content'):
            buffer = io.BytesIO()
            with wave.open(buffer, 'wb') as wav:
                wav.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
                wav.writeframes(b''.join(struct.pack('<h', int(math.sin(i * 2 * math.pi * 440 / 16000) * 6000)) for i in range(32000)))
            body = buffer.getvalue()
            self.send_response(200); self.send_header('Content-Type', 'audio/wav')
            self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)
            return
        self.send_json({'detail': 'unknown fixture route'}, 404)
    def do_POST(self):
        data = json.loads(self.rfile.read(int(self.headers.get('Content-Length', 0))) or '{}')
        if self.path == '/__test/offline':
            fixture.offline = bool(data['offline']); return self.send_json({'ok': True})
        if fixture.offline: return self.send_json({'detail': '隔离替身暂时离线'}, 503)
        if self.headers.get('X-Debug-Session') != TOKEN: return self.send_json({'detail': 'session expired'}, 401)
        with fixture.lock:
            fixture.writes += 1
            status = fixture.state['serial_device']['status']
            if self.path == '/api/device/commands':
                command = data['command']
                if command == 'record.start':
                    fixture.started = time.monotonic()
                    status['recording'] = dict(state='recording', capture_id='fixture-capture', started_monotonic_ms=fixture.started * 1000, max_seconds=data['max_seconds'])
                elif command == 'record.stop':
                    fixture.started = None; status['recording']['state'] = 'ready'
                elif command == 'vad.start': status['vad'].update(enabled=True, state='silence')
                elif command == 'vad.stop': status['vad'].update(enabled=False, state='disabled')
                else: return self.send_json(dict(ok=False, state='failed', error='unsupported'))
                reply = dict(ok=True, state='verified' if command == 'record.stop' else 'recording')
                if command == 'record.stop': reply['asset'] = fixture.state['assets'][0]
                return self.send_json(reply)
            if self.path == '/api/serial/vad/pause':
                status['vad'].update(enabled=False, state='disabled'); return self.send_json({'state': 'stopped'})
            if self.path == '/api/asr/probe': return self.send_json(fixture.state['asr'])
            if self.path == '/api/asr':
                fixture.state['transcripts'].insert(0, dict(job_id='fixture-job', asset_id=data['asset_id'], status='completed', error=None, result=dict(text='隔离替身合成文字，无模型调用。', provider='mock', model='Mock', timestamps_available=False)))
                return self.send_json({'state': 'completed'})
        self.send_json({'detail': 'unknown fixture route'}, 404)
    def websocket(self):
        key = self.headers.get('Sec-WebSocket-Key')
        if not key: return self.send_json({'detail': 'upgrade required'}, 400)
        accept = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
        self.send_response(101); self.send_header('Upgrade', 'websocket'); self.send_header('Connection', 'Upgrade')
        self.send_header('Sec-WebSocket-Accept', accept); self.end_headers()
        with fixture.lock: fixture.sockets += 1
        self.connection.settimeout(2)
        def emit(value):
            body = json.dumps(value).encode()
            header = bytes([0x81, len(body)]) if len(body) < 126 else bytes([0x81, 126]) + struct.pack('!H', len(body))
            self.wfile.write(header + body); self.wfile.flush()
        try:
            emit(dict(type='snapshot', state=fixture.snapshot()))
            while not fixture.offline:
                if fixture.state['serial_device']['status']['vad']['enabled']:
                    emit(fixture.frame())
                else: emit(dict(type='state', state=fixture.snapshot()))
                time.sleep(0.25)
        except (OSError, ConnectionError): pass
        finally: self.close_connection = True

def server(port):
    return ThreadingHTTPServer(('127.0.0.1', port), Handler)

if __name__ == '__main__':
    parser = argparse.ArgumentParser(); parser.add_argument('--port', type=int, default=18776)
    args = parser.parse_args()
    service = server(args.port)
    print(f'Isolated synthetic fixture: http://127.0.0.1:{service.server_port}', flush=True)
    try: service.serve_forever()
    except KeyboardInterrupt: pass
    finally: service.server_close()
