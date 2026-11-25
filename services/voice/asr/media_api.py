"""External voice-service API for Rust workflows; no product database access."""
from __future__ import annotations

import hashlib
import json
import math
import tempfile
import time
import wave
from pathlib import Path
from typing import Literal

from fastapi import File, Form, HTTPException, Request, UploadFile
from starlette.concurrency import run_in_threadpool

from .runtime import MAX_BYTES, MAX_SECONDS


def recognize_upload(pool, data: bytes, mode: str) -> dict:
    """One short upload; models come from the resident pool and load only when a chunk has speech."""
    from . import media_worker
    from .job_progress import report

    class PooledRuntime:
        def load(self):
            pool.get("asr")

        def transcribe(self, audio):
            return pool.get("asr").transcribe(audio)

    with tempfile.TemporaryDirectory(prefix="candid-voice-api-") as directory:
        root = Path(directory)
        source, decoded = root / "upload", root / "decoded.wav"
        source.write_bytes(data)
        report("decoding")
        media_worker.decode_original(source, decoded, max_seconds=MAX_SECONDS)
        with wave.open(str(decoded), "rb") as audio:
            duration = audio.getnframes() / audio.getframerate()
        if duration > MAX_SECONDS:
            raise ValueError("单次服务输入超过五分钟，请按来源分段提交")
        result = (media_worker.quick_result(decoded, root, PooledRuntime()) if mode == "quick"
                  else media_worker.conversation_result(decoded, root, pool.lazy("speaker")))
        return dict(result, duration_seconds=duration)


def checked_voice_result(result: dict, data: bytes) -> dict:
    if not isinstance(result, dict) or not isinstance(result.get("producer"), dict):
        raise ValueError("语音服务缺少模型来源")
    producer = result["producer"]
    if not all(isinstance(producer.get(key), str) and producer[key]
               for key in ("model_id", "model_revision", "timestamp_kind")):
        raise ValueError("语音服务模型来源不完整")
    duration = result.get("duration_seconds")
    if type(duration) not in (int, float) or not math.isfinite(duration) or not 0 < duration <= MAX_SECONDS:
        raise ValueError("语音服务时长无效")
    segments = result.get("segments")
    if not isinstance(segments, list) or len(segments) > 10000:
        raise ValueError("语音服务分段无效")
    for segment in segments:
        if not isinstance(segment, dict):
            raise ValueError("语音服务分段格式无效")
        start, end = segment.get("start_seconds"), segment.get("end_seconds")
        if (type(start) not in (int, float) or type(end) not in (int, float)
                or not math.isfinite(start) or not math.isfinite(end)
                or not 0 <= start < end <= duration
                or not isinstance(segment.get("text"), str) or not segment["text"].strip()):
            raise ValueError("语音服务来源区间或文字无效")
        for key in ("speaker", "identity"):
            if segment.get(key) is not None and not isinstance(segment[key], str):
                raise ValueError("语音服务人物候选无效")
        if segment.get("voice_ref") is not None and (not isinstance(segment["voice_ref"], str) or len(segment["voice_ref"]) > 128):
            raise ValueError("语音服务人物候选无效")
        match = segment.get("match")
        if match is not None and (not isinstance(match, dict) or not isinstance(match.get("name"), str)
                                  or type(match.get("score")) not in (int, float) or not math.isfinite(match["score"])):
            raise ValueError("语音服务人物候选无效")
    value = dict(result, format="candid-voice-result/1",
                 source_hash=hashlib.sha256(data).hexdigest(), cancellation_supported=False)
    if len(json.dumps(value, ensure_ascii=False, allow_nan=False).encode()) > 8 * 1024 * 1024:
        raise ValueError("语音服务结果过大")
    return value


def install_voice_api(app, gate, runner):
    @app.post("/v1/voice")
    async def voice(request: Request, file: UploadFile = File(...), mode: Literal["quick", "conversation"] = Form("quick")):
        try:
            if request.headers.get("origin"):
                raise HTTPException(403, "语音服务不接受浏览器来源请求")
            data = await file.read(MAX_BYTES + 1)
            if not data or len(data) > MAX_BYTES:
                raise HTTPException(413, "音频必须非空且不超过64MiB")
            if gate.locked():
                raise HTTPException(429, "语音服务正在处理其他请求", headers={"Retry-After": "5"})
            async with gate:
                started = time.process_time()
                result = await run_in_threadpool(runner, data, mode)
                result = checked_voice_result(result, data)
                result["cpu_seconds"] = round(time.process_time() - started, 3)
                return result
        except (ValueError, wave.Error, EOFError) as exc:
            raise HTTPException(400, "音频或语音结果无效，请检查输入后重试") from exc
        except HTTPException:
            raise
        except Exception as exc:
            raise HTTPException(503, "语音服务暂时不可用，可重试") from exc
        finally:
            await file.close()
