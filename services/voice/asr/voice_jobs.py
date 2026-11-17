"""Bounded, cancellable inference tasks owned by the independently deployed service."""
import asyncio
import hashlib
import json
import logging
import os
from pathlib import Path
import signal
import shutil
import subprocess
import sys
import tempfile
import time
import uuid

from fastapi import File, Form, HTTPException, Request, UploadFile
from .runtime import MAX_BYTES


class VoiceJobs:
    def __init__(self, gate, command=None):
        self.gate, self.command, self.jobs = gate, command, {}

    def public(self, job):
        value = {k: job[k] for k in ('id', 'state', 'stage', 'created_ms', 'updated_ms', 'error')}
        try:
            progress = json.loads((job['root'] / 'progress.json').read_text())
            if progress.get('stage') in {'decoding', 'loading_model', 'transcribing', 'voiceprints', 'finalizing'}:
                value.update(progress)
        except (OSError, ValueError):
            pass
        if job['state'] not in ('running',):
            value['stage'] = job['stage']
        if job['state'] == 'done':
            value['result'] = job['result']
        value['cancellation_supported'] = True
        return value

    async def submit(self, data, mode, request_id):
        if mode not in ('quick', 'conversation') or not request_id or len(request_id) > 200:
            raise HTTPException(400, 'Invalid mode or request ID')
        digest = hashlib.sha256(data).hexdigest()
        for job in self.jobs.values():
            if job['request_id'] == request_id:
                if (job['digest'], job['mode']) != (digest, mode):
                    raise HTTPException(409, 'Request ID input conflict')
                return self.public(job)
        for key, job in list(self.jobs.items()):
            if job['state'] in ('done', 'failed', 'cancelled') and time.time() * 1000 - job['updated_ms'] > 600_000:
                job['temp'].cleanup(); del self.jobs[key]
        if len(self.jobs) >= 32:
            terminal = next((key for key,j in self.jobs.items() if j['state'] in ('done','failed','cancelled')), None)
            if terminal is not None:
                self.jobs.pop(terminal)['temp'].cleanup()
        if len(self.jobs) >= 32 or self.gate.locked():
            raise HTTPException(429, 'Voice service busy', headers={'Retry-After': '5'})
        await self.gate.acquire()
        temp = tempfile.TemporaryDirectory(prefix='candid-voice-job-')
        root = Path(temp.name)
        try:
            (root / 'input').write_bytes(data)
            now = int(time.time() * 1000)
            job = dict(id=str(uuid.uuid4()), state='running', stage='starting', created_ms=now, updated_ms=now,
                       error=None, root=root, temp=temp, digest=digest, mode=mode, request_id=request_id)
            env = dict(os.environ, PYTHONPATH=str(Path(__file__).resolve().parents[1]) + os.pathsep + os.environ.get('PYTHONPATH', ''))
            command = self.command(root, mode) if self.command else [sys.executable, '-m', 'asr.voice_job_worker', str(root), mode]
            with (root / 'worker.log').open('wb') as log:
                job['process'] = subprocess.Popen(command, env=env, stdout=log, stderr=log, start_new_session=True)
            self.jobs[job['id']] = job
            job['watcher'] = asyncio.create_task(self.watch(job))
            return self.public(job)
        except Exception:
            temp.cleanup(); self.gate.release(); raise

    async def terminate(self, job):
        process = job['process']
        if process.poll() is None:
            try: os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError: pass
            try: await asyncio.wait_for(asyncio.to_thread(process.wait), 2)
            except asyncio.TimeoutError:
                try: os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError: pass
                await asyncio.to_thread(process.wait)

    async def watch(self, job):
        from .media_api import checked_voice_result
        try:
            deadline = time.monotonic() + 600
            while job['process'].poll() is None:
                if time.monotonic() > deadline:
                    job.update(state='failed', stage='failed', error='timeout')
                    await self.terminate(job)
                    break
                await asyncio.sleep(.1)
            if job['state'] == 'running':
                path = job['root'] / 'result.json'
                if job['process'].returncode != 0 or not path.exists() or path.stat().st_size > 8 * 1024 * 1024:
                    raise ValueError('Inference worker failed')
                result = json.loads(path.read_text())
                result = checked_voice_result(result, (job['root'] / 'input').read_bytes())
                result['cancellation_supported'] = True
                job.update(state='done', stage='done', result=result)
        except Exception:
            logging.getLogger(__name__).exception('Voice task failed: %s (worker exit %s)', job['id'], job['process'].poll())
            job.update(state='failed', stage='failed', error='inference_failed')
        finally:
            shutil.rmtree(job['root'] / 'work', ignore_errors=True)
            job['updated_ms'] = int(time.time() * 1000)
            for name in ('input', 'result.json'):
                (job['root'] / name).unlink(missing_ok=True)
            self.gate.release()

    async def cancel(self, job):
        if job['state'] == 'running':
            job.update(state='cancelling', stage='cancelling')
            await self.terminate(job)
            await job['watcher']
            job.update(state='cancelled', stage='cancelled', updated_ms=int(time.time() * 1000))
        elif job['state'] == 'cancelling':
            await job['watcher']
            job.update(state='cancelled', stage='cancelled')
        return self.public(job)

    async def close(self):
        for job in self.jobs.values():
            await self.cancel(job)
            job['temp'].cleanup()


def install_voice_jobs(app, gate, *, command=None):
    manager = VoiceJobs(gate, command)
    def allowed(request):
        if request.headers.get('origin'):
            raise HTTPException(403, 'Browser origins are not accepted')
    def find(key):
        if key not in manager.jobs:
            raise HTTPException(404, 'Voice task not found')
        return manager.jobs[key]

    @app.get('/v1/voice/capabilities')
    async def capabilities(request: Request):
        allowed(request)
        return {'format': 'candid-voice-jobs/1', 'progress': True, 'cancellation': 'terminate_worker'}

    @app.post('/v1/voice/jobs')
    async def submit(request: Request, file: UploadFile = File(...), mode: str = Form('quick'), request_id: str = Form(...)):
        try:
            allowed(request)
            data = await file.read(MAX_BYTES + 1)
            if not data or len(data) > MAX_BYTES:
                raise HTTPException(413, 'Audio must be nonempty and at most 64 MiB')
            return await manager.submit(data, mode, request_id)
        finally:
            await file.close()

    @app.get('/v1/voice/jobs/{key}')
    async def status(key: str, request: Request):
        allowed(request); return manager.public(find(key))

    @app.delete('/v1/voice/jobs/{key}')
    async def cancel(key: str, request: Request):
        allowed(request); return await manager.cancel(find(key))
    return manager
