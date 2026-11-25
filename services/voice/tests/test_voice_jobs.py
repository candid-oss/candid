"""In-process voice jobs: progress, long polling, cancellation between chunks, and idle model unloading."""
import asyncio
import plistlib
import threading
import time
from pathlib import Path

import httpx
from fastapi import FastAPI

from asr.job_progress import report
from asr.models import ModelPool
from asr.voice_jobs import install_voice_jobs


def result():
    return {"duration_seconds": 1, "segments": [],
            "producer": {"model_id": "SYNTHETIC", "model_revision": "test", "timestamp_kind": "audio_chunk",
                         "processor_version": "1"}}


class Runner:
    """Synthetic inference: quick finishes at once; conversation reports chunk after chunk until released."""

    def __init__(self):
        self.release = threading.Event()
        self.chunks = 0

    def __call__(self, data, mode):
        report("transcribing", 0, 10)
        if mode == "conversation":
            while not self.release.wait(0.01):
                self.chunks += 1
                report("transcribing", self.chunks, 10)
        return result()


def test_jobs_progress_idempotency_rejection_and_cancel_between_chunks():
    async def run():
        gate = asyncio.Lock(); app = FastAPI(); runner = Runner()
        manager = install_voice_jobs(app, gate, runner)
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="http://test") as client:
            caps = await client.get("/v1/voice/capabilities")
            assert caps.json()["format"] == "candid-voice-jobs/1" and caps.json()["long_poll"] is True
            assert (await client.get("/v1/voice/capabilities", headers={"origin": "http://browser"})).status_code == 403
            assert (await client.get("/v1/voice/jobs/missing")).status_code == 404

            async def submit(key, mode="quick", data=b"SYNTHETIC"):
                return await client.post("/v1/voice/jobs", files={"file": ("test.wav", data)},
                                         data={"mode": mode, "request_id": key})
            assert (await submit("empty", data=b"")).status_code == 413
            slow = await submit("slow", "conversation"); assert slow.status_code == 200
            key = slow.json()["id"]
            assert (await submit("busy")).status_code == 429
            assert (await submit("slow", "conversation")).json()["id"] == key
            assert (await submit("slow", "quick")).status_code == 409
            status = (await client.get(f"/v1/voice/jobs/{key}?wait=5&since=0")).json()
            assert status["stage"] == "transcribing" and status["seq"] > 0
            started = time.monotonic()
            stopped = await client.delete("/v1/voice/jobs/" + key)
            # Cancellation takes effect at the next chunk boundary, well within the wait.
            assert stopped.json()["state"] == "cancelled" and time.monotonic() - started < 2
            assert not gate.locked()
            assert (await client.delete("/v1/voice/jobs/" + key)).json()["state"] == "cancelled"
            done = await submit("after-stop"); key = done.json()["id"]
            value = done.json()
            while value["state"] == "running":
                value = (await client.get(f"/v1/voice/jobs/{key}?wait=5&since={value['seq']}")).json()
            assert value["state"] == "done" and value["result"]["cancellation_supported"]
            assert value["result"]["source_hash"] and value["result"]["cpu_seconds"] >= 0
            assert not gate.locked()
        await manager.close()
    asyncio.run(run())


def test_long_poll_returns_on_progress_and_otherwise_waits():
    async def run():
        gate = asyncio.Lock(); app = FastAPI(); runner = Runner()
        manager = install_voice_jobs(app, gate, runner)
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="http://test") as client:
            job = (await client.post("/v1/voice/jobs", files={"file": ("t.wav", b"SYNTHETIC")},
                                     data={"mode": "conversation", "request_id": "poll"})).json()
            # Progress arrives every ~10 ms; a 30-second long poll returns at the first change.
            first = (await client.get(f"/v1/voice/jobs/{job['id']}?wait=30&since=0")).json()
            started = time.monotonic()
            second = (await client.get(f"/v1/voice/jobs/{job['id']}?wait=30&since={first['seq']}")).json()
            assert second["seq"] > first["seq"] and time.monotonic() - started < 1
            # Without changes the request waits for the timeout and returns the unchanged state.
            runner.release.set()
            final = (await client.get(f"/v1/voice/jobs/{job['id']}?wait=30&since={second['seq']}")).json()
            while final["state"] != "done":
                final = (await client.get(f"/v1/voice/jobs/{job['id']}?wait=30&since={final['seq']}")).json()
            started = time.monotonic()
            idle = (await client.get(f"/v1/voice/jobs/{job['id']}?wait=0.3&since={final['seq'] + 5}")).json()
            assert idle["state"] == "done" and time.monotonic() - started < 0.2, "a finished job never waits"
        await manager.close()

        gate = asyncio.Lock(); app = FastAPI(); runner = Runner()
        manager = install_voice_jobs(app, gate, runner)
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="http://test") as client:
            job = (await client.post("/v1/voice/jobs", files={"file": ("t.wav", b"SYNTHETIC")},
                                     data={"mode": "conversation", "request_id": "stall"})).json()
            runner_chunks = runner.chunks
            current = (await client.get(f"/v1/voice/jobs/{job['id']}")).json()
            started = time.monotonic()
            stalled = (await client.get(f"/v1/voice/jobs/{job['id']}?wait=0.3&since={current['seq'] + 10_000}")).json()
            assert time.monotonic() - started >= 0.25 and stalled["state"] == "running" and runner.chunks > runner_chunks
            runner.release.set()
        await manager.close()
    asyncio.run(run())


class Model:
    def __init__(self):
        self.loads = self.unloads = 0

    def load(self):
        self.loads += 1

    def unload(self):
        self.unloads += 1


def test_models_load_on_first_use_and_unload_after_idle():
    asr, speaker = Model(), Model()
    pool = ModelPool(asr=asr, speaker=speaker, idle_seconds=0.2)
    assert pool.state() == {"asr": False, "speaker": False} and asr.loads == 0
    with pool.use():
        pool.get("asr"); pool.get("asr")
    assert asr.loads == 1 and speaker.loads == 0 and pool.state()["asr"]
    time.sleep(0.1)
    with pool.use():  # use within the idle time keeps the model and restarts the clock
        pool.get("asr")
    time.sleep(0.12)
    assert pool.state()["asr"] and asr.unloads == 0
    deadline = time.monotonic() + 3
    while pool.state()["asr"] and time.monotonic() < deadline:
        time.sleep(0.02)
    assert not pool.state()["asr"] and asr.unloads == 1
    with pool.use():
        pool.get("asr")
    assert asr.loads == 2
    pool.close()
    assert pool.timer is None and not pool.state()["asr"]


def test_busy_pool_never_unloads():
    asr = Model()
    pool = ModelPool(asr=asr, speaker=Model(), idle_seconds=0.05)
    with pool.use():
        pool.get("asr")
        time.sleep(0.2)
        assert pool.state()["asr"]
    pool.close()


def test_health_reports_loaded_models_and_service_starts_without_loading():
    from fastapi.testclient import TestClient
    from asr.app import create_app

    class Engine(Model):
        device = "synthetic"

        def transcribe(self, data, language=None):
            return {"text": "SYNTHETIC"}

    engine = Engine()
    with TestClient(create_app(engine, model_idle_seconds=0.2)) as client:
        health = client.get("/health").json()
        assert health["status"] == "ready" and health["models_loaded"] == {"asr": False, "speaker": False}
        assert engine.loads == 0
        response = client.post("/v1/audio/transcriptions", files={"file": ("x.wav", b"SYNTHETIC")})
        assert response.status_code == 200 and "cpu_seconds" in response.json()
        assert client.get("/health").json()["models_loaded"]["asr"] is True
        deadline = time.monotonic() + 3
        while client.get("/health").json()["models_loaded"]["asr"] and time.monotonic() < deadline:
            time.sleep(0.05)
        assert client.get("/health").json()["models_loaded"]["asr"] is False and engine.unloads == 1


def test_launchd_runs_the_service_as_background_work(tmp_path):
    import importlib.util
    path = Path(__file__).resolve().parents[1] / "scripts/local_asr.py"
    spec = importlib.util.spec_from_file_location("local_asr", path)
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    config = plistlib.loads(plistlib.dumps(module.launchd_config("io.github.candid-oss.voice", tmp_path)))
    assert config["ProcessType"] == "Background" and config["LowPriorityBackgroundIO"] is True
