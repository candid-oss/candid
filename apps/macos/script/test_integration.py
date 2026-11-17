#!/usr/bin/env python3
"""Owns an ephemeral fixture server, cleans it up, never uses 8765/8766/8767."""
import os
import pathlib
import subprocess
import threading
from mock_server import server

root = pathlib.Path(__file__).resolve().parents[1]
service = server(0)
thread = threading.Thread(target=service.serve_forever, daemon=True)
thread.start()
try:
    env = dict(os.environ, MAC_CONSOLE_FIXTURE_URL=f'http://127.0.0.1:{service.server_port}')
    result = subprocess.run([str(root / 'script/test.sh'), '--filter', 'SocketIntegrationTests'], env=env)
finally:
    service.shutdown(); service.server_close(); thread.join(timeout=2)
raise SystemExit(result.returncode)
