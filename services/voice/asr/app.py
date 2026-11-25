"""Loopback-only voice service; does not save uploaded audio.

One resident process. Models load on first use and are unloaded after ten idle minutes (`models.ModelPool`), so a
started service costs no memory or GPU until something is transcribed.
"""
import logging
import asyncio
from contextlib import asynccontextmanager

from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from starlette.concurrency import run_in_threadpool

from .models import IDLE_SECONDS, ModelPool, measured
from .runtime import AudioError, MAX_BYTES, MODEL_ID, MODEL_REVISION


def create_app(runtime=None, voice_runner=None, *, voiceprint_path=None, voiceprint_analyzer=None,
               voiceprint_models=None, model_idle_seconds=IDLE_SECONDS):
    pool = ModelPool(asr=runtime, speaker=voiceprint_models, idle_seconds=model_idle_seconds)
    gate = asyncio.Lock()

    @asynccontextmanager
    async def lifespan(app):
        yield
        await app.state.voice_jobs.close()
        pool.close()

    app = FastAPI(title="Local Qwen3 ASR", lifespan=lifespan)
    app.state.models = pool

    @app.get("/health")
    def health():
        asr = pool.slots["asr"].value
        return {"status": "ready", "model_id": MODEL_ID, "model_revision": MODEL_REVISION,
                "device": getattr(asr, "device", None), "backend": "mlx-audio", "quantization": "8bit",
                "timestamps_available": False, "models_loaded": pool.state(),
                "idle_unload_seconds": pool.idle_seconds}

    def transcribe_upload(data, language):
        with pool.use():
            return measured(lambda: pool.get("asr").transcribe(data, language))

    @app.post("/v1/audio/transcriptions")
    async def transcribe(file: UploadFile = File(...), model: str = Form(MODEL_ID),
                         language: str | None = Form(None), response_format: str = Form("json")):
        try:
            if model != MODEL_ID or response_format != "json":
                raise HTTPException(400, f"Use model {MODEL_ID} and response_format=json")
            data = await file.read(MAX_BYTES + 1)
            if len(data) > MAX_BYTES:
                raise HTTPException(413, "Audio exceeds 64 MiB")
            if gate.locked():
                raise BlockingIOError("ASR busy; retry after the current request completes")
            async with gate:
                return await run_in_threadpool(transcribe_upload, data, language)
        except (AudioError, ValueError) as exc:
            raise HTTPException(400, str(exc)) from exc
        except BlockingIOError as exc:
            raise HTTPException(429, str(exc), headers={"Retry-After": "5"}) from exc
        except HTTPException:
            raise
        except Exception as exc:
            logging.getLogger(__name__).exception("Local ASR inference failed")
            raise HTTPException(500, "Local inference failed; check data/asr/server.log and retry") from exc
        finally:
            await file.close()

    from .media_api import install_voice_api, recognize_upload

    def recognize(data, mode):
        with pool.use():
            return recognize_upload(pool, data, mode)

    runner = voice_runner or recognize
    from .voice_jobs import install_voice_jobs
    app.state.voice_jobs = install_voice_jobs(app, gate, runner)
    install_voice_api(app, gate, runner)
    from .voiceprint_api import install_voiceprint_api
    install_voiceprint_api(app, gate, pool, path=voiceprint_path, analyzer=voiceprint_analyzer)
    return app
