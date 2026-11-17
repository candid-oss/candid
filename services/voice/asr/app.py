"""Loopback-only multipart transcription API; does not save uploaded audio."""
import logging
import asyncio
from contextlib import asynccontextmanager

from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from starlette.concurrency import run_in_threadpool

from .runtime import AudioError, MAX_BYTES, MODEL_ID, MODEL_REVISION, Runtime


def create_app(runtime=None, voice_runner=None, *, voiceprint_path=None, voiceprint_analyzer=None):
    engine = runtime or Runtime()
    gate = asyncio.Lock()

    @asynccontextmanager
    async def lifespan(app):
        await run_in_threadpool(engine.load)
        yield
        await app.state.voice_jobs.close()

    app = FastAPI(title="Local Qwen3 ASR", lifespan=lifespan)

    @app.get("/health")
    def health():
        return {"status": "ready", "model_id": MODEL_ID, "model_revision": MODEL_REVISION,
                "device": engine.device, "backend": "mlx-audio", "quantization": "8bit",
                "timestamps_available": False}

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
                return await run_in_threadpool(engine.transcribe, data, language)
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

    from .voice_jobs import install_voice_jobs
    app.state.voice_jobs = install_voice_jobs(app, gate)
    from .media_api import install_voice_api
    install_voice_api(app, engine, gate, voice_runner)
    from .voiceprint_api import install_voiceprint_api
    install_voiceprint_api(app, gate, path=voiceprint_path, analyzer=voiceprint_analyzer)
    return app
