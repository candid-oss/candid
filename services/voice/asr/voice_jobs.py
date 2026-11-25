"""Bounded, cancellable inference jobs, run in this process on the already loaded models.

A job runs on one thread while holding the service's inference gate. Cancelling sets a flag that takes effect at
the next chunk boundary (`job_progress.report`), so no process is ever killed. Status supports long polling:
`GET /v1/voice/jobs/{id}?wait=30&since=<seq>` returns as soon as the job's `seq` passes `since` (or it ends), or
after `wait` seconds.
"""
import asyncio
import hashlib
import logging
import threading
import time
import uuid

from fastapi import File, Form, HTTPException, Request, UploadFile

from . import job_progress
from .runtime import MAX_BYTES

TIMEOUT_SECONDS = 600
MAX_WAIT_SECONDS = 60
TERMINAL = ("done", "failed", "cancelled")


class JobTimeout(Exception):
    pass


def now_ms():
    return int(time.time() * 1000)


class VoiceJobs:
    def __init__(self, gate, runner, *, timeout=TIMEOUT_SECONDS):
        self.gate, self.runner, self.timeout, self.jobs = gate, runner, timeout, {}

    def public(self, job):
        value = {k: job[k] for k in ("id", "state", "stage", "created_ms", "updated_ms", "error", "seq")}
        if job["state"] in ("running", "cancelling"):
            value.update(job["progress"])
            value["stage"] = job["stage"]
        if job["state"] == "done":
            value["result"] = job["result"]
        value["cancellation_supported"] = True
        return value

    def changed(self, job, **fields):
        job.update(fields)
        job["seq"] += 1
        job["updated_ms"] = now_ms()
        event, job["event"] = job["event"], asyncio.Event()
        event.set()

    async def submit(self, data, mode, request_id):
        if mode not in ("quick", "conversation") or not request_id or len(request_id) > 200:
            raise HTTPException(400, "Invalid mode or request ID")
        digest = hashlib.sha256(data).hexdigest()
        for job in self.jobs.values():
            if job["request_id"] == request_id:
                if (job["digest"], job["mode"]) != (digest, mode):
                    raise HTTPException(409, "Request ID input conflict")
                return self.public(job)
        for key, job in list(self.jobs.items()):
            if job["state"] in TERMINAL and now_ms() - job["updated_ms"] > 600_000:
                del self.jobs[key]
        if len(self.jobs) >= 32:
            terminal = next((key for key, j in self.jobs.items() if j["state"] in TERMINAL), None)
            if terminal is not None:
                del self.jobs[terminal]
        if len(self.jobs) >= 32 or self.gate.locked():
            raise HTTPException(429, "Voice service busy", headers={"Retry-After": "5"})
        await self.gate.acquire()
        stamp = now_ms()
        job = dict(id=str(uuid.uuid4()), state="running", stage="starting", created_ms=stamp, updated_ms=stamp,
                   error=None, seq=0, progress={}, event=asyncio.Event(), cancel=threading.Event(),
                   digest=digest, mode=mode, request_id=request_id, result=None)
        self.jobs[job["id"]] = job
        job["task"] = asyncio.create_task(self.run(job, data))
        return self.public(job)

    async def run(self, job, data):
        from .media_api import checked_voice_result
        loop = asyncio.get_running_loop()
        deadline = time.monotonic() + self.timeout

        def sink(stage, completed=None, total=None):
            if job["cancel"].is_set():
                raise job_progress.Cancelled()
            if time.monotonic() > deadline:
                raise JobTimeout()
            progress = {"stage": stage, "updated_ms": now_ms()}
            if completed is not None:
                progress["completed_seconds"] = completed
            if total is not None:
                progress["total_seconds"] = total
            loop.call_soon_threadsafe(lambda: self.changed(job, stage=stage, progress=progress))

        try:
            job_progress.SINK.set(sink)  # this task's own context; the worker thread inherits a copy
            started = time.process_time()
            result = await asyncio.to_thread(self.runner, data, job["mode"])
            if job["cancel"].is_set():
                raise job_progress.Cancelled()
            result = checked_voice_result(result, data)
            result["cancellation_supported"] = True
            result["cpu_seconds"] = round(time.process_time() - started, 3)
            self.changed(job, state="done", stage="done", result=result)
        except job_progress.Cancelled:
            self.changed(job, state="cancelled", stage="cancelled")
        except JobTimeout:
            self.changed(job, state="failed", stage="failed", error="timeout")
        except Exception:
            logging.getLogger(__name__).exception("Voice task failed: %s", job["id"])
            self.changed(job, state="failed", stage="failed", error="inference_failed")
        finally:
            self.gate.release()

    async def wait(self, job, since=None, seconds=0.0):
        """The job once its `seq` passes `since` (default: the current one) or it ends, at most after `seconds`."""
        since = job["seq"] if since is None else since
        deadline = time.monotonic() + max(0.0, min(float(seconds), MAX_WAIT_SECONDS))
        while job["seq"] <= since and job["state"] not in TERMINAL:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            try:
                await asyncio.wait_for(job["event"].wait(), remaining)
            except asyncio.TimeoutError:
                break
        return self.public(job)

    async def cancel(self, job, seconds=5.0):
        """Stops the job at its next chunk boundary; waits briefly so a confirmed stop can be reported."""
        if job["state"] == "running":
            job["cancel"].set()
            self.changed(job, state="cancelling", stage="cancelling")
        if job["state"] == "cancelling":
            try:
                await asyncio.wait_for(asyncio.shield(job["task"]), seconds)
            except asyncio.TimeoutError:
                pass
        return self.public(job)

    async def close(self):
        for job in list(self.jobs.values()):
            if job["state"] in ("running", "cancelling"):
                job["cancel"].set()
                await asyncio.gather(job["task"], return_exceptions=True)


def install_voice_jobs(app, gate, runner, **options):
    manager = VoiceJobs(gate, runner, **options)

    def allowed(request):
        if request.headers.get("origin"):
            raise HTTPException(403, "Browser origins are not accepted")

    def find(key):
        if key not in manager.jobs:
            raise HTTPException(404, "Voice task not found")
        return manager.jobs[key]

    @app.get("/v1/voice/capabilities")
    async def capabilities(request: Request):
        allowed(request)
        return {"format": "candid-voice-jobs/1", "progress": True, "cancellation": "between_chunks",
                "long_poll": True, "cpu_seconds": True}

    @app.post("/v1/voice/jobs")
    async def submit(request: Request, file: UploadFile = File(...), mode: str = Form("quick"),
                     request_id: str = Form(...)):
        try:
            allowed(request)
            data = await file.read(MAX_BYTES + 1)
            if not data or len(data) > MAX_BYTES:
                raise HTTPException(413, "Audio must be nonempty and at most 64 MiB")
            return await manager.submit(data, mode, request_id)
        finally:
            await file.close()

    @app.get("/v1/voice/jobs/{key}")
    async def status(key: str, request: Request, wait: float = 0, since: int | None = None):
        allowed(request)
        return await manager.wait(find(key), since, wait)

    @app.delete("/v1/voice/jobs/{key}")
    async def cancel(key: str, request: Request):
        allowed(request)
        return await manager.cancel(find(key))

    return manager
