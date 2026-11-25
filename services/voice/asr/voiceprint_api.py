"""Explicit local enrollment; vectors never leave the independent model service."""
from __future__ import annotations

import os
import sqlite3
import tempfile
from pathlib import Path
from typing import Literal

from fastapi import File, Form, HTTPException, Request, UploadFile
from pydantic import BaseModel, StrictBool
from starlette.concurrency import run_in_threadpool

from .runtime import MAX_BYTES, MAX_SECONDS


def library_path() -> Path:
    configured = os.environ.get("CANDID_VOICE_LIBRARY")
    return (Path(configured).expanduser() if configured else
            Path(__file__).resolve().parents[3] / "data/voice-library/speakers.sqlite")


class LibraryCommand(BaseModel):
    action: Literal["enroll", "rename", "merge", "delete", "forget", "withdraw", "rematch", "forget_refs"]
    analysis_id: str | None = None
    speaker_id: str | None = None
    person_id: str | None = None
    target_id: str | None = None
    name: str | None = None
    confirmed: StrictBool = False
    receipt: dict | None = None
    refs: list[str] | None = None


def install_voiceprint_api(app, gate, pool, *, path=None, analyzer=None):
    """`pool` is the resident `ModelPool`; window and analysis requests load the speaker models on first use."""
    from .models import measured

    def store():
        from .speaker_store import SpeakerStore
        location = Path(path) if path else library_path()
        location.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        if location.name != "speakers.sqlite":
            raise ValueError("Voice library must be named speakers.sqlite")
        library = SpeakerStore(location.parent)
        os.chmod(library.path, 0o600)
        return library

    def permitted(request):
        if request.headers.get("origin"):
            raise HTTPException(403, "声纹库不接受浏览器来源请求")

    def analyze(data):
        from .media_worker import decode_original
        from .speaker_pipeline import SpeakerPipeline, public_result
        library = store()
        with tempfile.TemporaryDirectory(prefix="candid-voiceprint-") as directory:
            root = Path(directory)
            source, decoded = root / "upload", root / "decoded.wav"
            source.write_bytes(data)
            decode_original(source, decoded, max_seconds=MAX_SECONDS)
            with pool.use():
                pipeline = SpeakerPipeline(library, pool.lazy("speaker"))
                result = measured(pipeline.analyze, decoded)
            # The model's source is the processing WAV, not the compressed original.
            # Retain the original digest separately for an unambiguous UI reference.
            import hashlib
            result["upload_hash"] = hashlib.sha256(data).hexdigest()
            return public_result(result)

    @app.get("/v1/voiceprints")
    async def people(request: Request):
        permitted(request)
        try:
            return {"format": "candid-voiceprints/1",
                    "capabilities": ["template_withdrawal", "recording_reconciliation", "recording_windows",
                                     "recording_windows_quick"],
                    "people": await run_in_threadpool(lambda: store().people())}
        except Exception as exc:
            raise HTTPException(503, "本机声纹库暂时不可用") from exc

    @app.post("/v1/voiceprints/analyze")
    async def analysis(request: Request, file: UploadFile = File(...)):
        try:
            permitted(request)
            data = await file.read(MAX_BYTES + 1)
            if not data or len(data) > MAX_BYTES:
                raise HTTPException(413, "音频必须非空且不超过64MiB")
            if gate.locked():
                raise HTTPException(429, "语音服务正在处理其他请求", headers={"Retry-After": "5"})
            async with gate:
                result = await run_in_threadpool(analyzer or analyze, data)
                return {"format": "candid-voiceprint-analysis/1", "analysis": result}
        except ValueError as exc:
            raise HTTPException(400, "音频或声纹分析无效，请检查后重试") from exc
        except HTTPException:
            raise
        except Exception as exc:
            raise HTTPException(503, "声纹分析暂时不可用，可重试") from exc
        finally:
            await file.close()

    @app.post("/v1/voiceprints/reconcile")
    async def reconcile_recording(request: Request):
        permitted(request)
        if gate.locked():
            raise HTTPException(429, "语音服务正在处理其他请求", headers={"Retry-After": "5"})
        body = await request.body()
        if len(body) > 48 * 1024 * 1024:
            raise HTTPException(413, "说话人样本过大")
        try:
            import json
            value = json.loads(body)
            from .speaker_reconciliation import reconcile
            from .speaker_store import VoiceRefs
            async with gate:
                library = store()
                return await run_in_threadpool(reconcile, value, library, VoiceRefs(library.path.parent))
        except (ValueError, TypeError, KeyError) as exc:
            raise HTTPException(400, "说话人整理输入无效") from exc
        except HTTPException:
            raise
        except Exception as exc:
            raise HTTPException(503, "说话人整理暂时不可用，可重试") from exc

    @app.post("/v1/voiceprints/window")
    async def recording_window(request: Request, file: UploadFile = File(...), meta: str = Form(...)):
        """One window of a recording: transcript with local speaker labels and opaque voice references."""
        try:
            permitted(request)
            data = await file.read(64 * 1024 * 1024 + 1)
            if not data or len(data) > 64 * 1024 * 1024:
                raise HTTPException(413, "窗口音频必须非空且不超过64MiB")
            if gate.locked():
                raise HTTPException(429, "语音服务正在处理其他请求", headers={"Retry-After": "5"})
            import json
            from .recording_windows import read_window, recognize_window, transcribe_window
            from .speaker_models import TokenLimit
            from .speaker_store import VoiceRefs
            info = json.loads(meta)
            if not isinstance(info, dict) or info.get("mode", "conversation") not in ("quick", "conversation"):
                raise ValueError("Invalid window metadata")
            audio = read_window(data)

            def run(refs):
                # One window per call; the caller saves it before asking for the next, so stopping between
                # windows is the cancellation point and nothing here needs to be killed.
                with pool.use():
                    if info.get("mode") == "quick":
                        return measured(transcribe_window, audio, pool.get("asr"), info.get("scope"), info.get("index"))
                    return measured(recognize_window, audio, pool.get("speaker"), refs, info.get("scope"), info.get("index"))

            async with gate:
                library = store()
                try:
                    return await run_in_threadpool(run, VoiceRefs(library.path.parent))
                except TokenLimit as exc:
                    raise HTTPException(422, "window_too_dense") from exc
        except HTTPException:
            raise
        except (ValueError, TypeError, KeyError) as exc:
            raise HTTPException(400, "窗口音频或参数无效") from exc
        except Exception as exc:
            raise HTTPException(503, "录音识别暂时不可用，可重试") from exc
        finally:
            await file.close()

    @app.post("/v1/voiceprints/stitch")
    async def recording_stitch(request: Request):
        """Join all windows of a recording into one set of speakers and name them from the library."""
        permitted(request)
        if gate.locked():
            raise HTTPException(429, "语音服务正在处理其他请求", headers={"Retry-After": "5"})
        body = await request.body()
        if len(body) > 32 * 1024 * 1024:
            raise HTTPException(413, "录音结果过大")
        try:
            import json
            from .recording_windows import stitch
            from .speaker_store import VoiceRefs
            value = json.loads(body)
            async with gate:
                library = store()
                return await run_in_threadpool(measured, stitch, value, library, VoiceRefs(library.path.parent))
        except (ValueError, TypeError, KeyError) as exc:
            raise HTTPException(400, "录音拼接输入无效") from exc
        except HTTPException:
            raise
        except Exception as exc:
            raise HTTPException(503, "录音拼接暂时不可用，可重试") from exc

    @app.post("/v1/voiceprints")
    async def command(request: Request, value: LibraryCommand):
        permitted(request)
        if value.action in ("enroll", "merge", "delete", "forget", "withdraw") and not value.confirmed:
            raise HTTPException(400, "请先确认登记授权、人物身份或此次删除操作")

        def perform():
            library = store()
            if value.action == "enroll":
                return library.enroll(value.analysis_id, value.speaker_id, name=value.name,
                                      person_id=value.person_id, confirmed=value.confirmed, with_receipt=True)
            if value.action == "rename":
                library.rename(value.person_id, value.name or "")
            elif value.action == "merge":
                return library.merge(value.person_id, value.target_id)
            elif value.action == "delete":
                if any(p["id"] == value.person_id for p in library.people()):
                    library.delete(value.person_id)
            elif value.action == "withdraw":
                return library.withdraw(value.receipt)
            elif value.action in ("rematch", "forget_refs"):
                from .media_worker import match_of
                from .speaker_store import VoiceRefs
                refs = [r for r in (value.refs or []) if isinstance(r, str) and 1 <= len(r) <= 128][:2000]
                saved = VoiceRefs(library.path.parent)
                if value.action == "forget_refs":
                    return saved.forget(refs)
                # The library's own matcher, so names, thresholds and margins agree with new transcripts.
                return {ref: match_of(library.match(vector, seconds=seconds))
                        for ref, (vector, seconds) in saved.entries(refs).items()}
            else:
                if library.cached(value.analysis_id):
                    library.forget(value.analysis_id)
            return None

        try:
            result = await run_in_threadpool(perform)
            out = {"format": "candid-voiceprints/1",
                   "people": await run_in_threadpool(lambda: store().people())}
            if value.action == "enroll":
                out.update(result)
            elif value.action == "withdraw":
                out["removed_templates"] = result
            elif value.action == "rematch":
                out["matches"] = result
            elif value.action == "forget_refs":
                out["forgotten"] = result
            else:
                out["person"] = result
            return out
        except (ValueError, TypeError) as exc:
            raise HTTPException(400, "声纹库操作无效，请核对人物、分析与名称") from exc
        except sqlite3.IntegrityError as exc:
            raise HTTPException(409, "人物名称已存在，请改名或明确选择合并") from exc
        except Exception as exc:
            raise HTTPException(503, "声纹库操作未完成，可重试") from exc
