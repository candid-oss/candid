"""One resident process: models load on first use and are released after ten quiet minutes.

There are no worker processes. Every inference runs inside `use()`, which keeps the pool busy; models are loaded
by `get()` the first time a request needs them. When the last user leaves, one timer is armed; it unloads only if
nothing used the pool since. Unloading drops the references, collects garbage and empties MLX's buffer cache so the
GPU memory is given back. Nothing polls: the timer is the only wake-up, and only while a model is loaded.
"""
from __future__ import annotations

import gc
import logging
import sys
import threading
import time
from contextlib import contextmanager

IDLE_SECONDS = 10 * 60
log = logging.getLogger(__name__)


def clear_mlx_cache():
    """Return MLX's cached GPU buffers to the system; a no-op where MLX was never imported."""
    if "mlx.core" not in sys.modules:
        return
    try:
        import mlx.core as mx
        clear = getattr(mx, "clear_cache", None) or mx.metal.clear_cache
        clear()
    except Exception:  # pragma: no cover - depends on the installed MLX
        log.exception("Could not clear the MLX cache")


class _Slot:
    def __init__(self, factory):
        self.factory, self.value, self.loaded = factory, None, False


class Lazy:
    """Stands in for a model object; the model loads on the first `load()` or attribute access."""

    def __init__(self, pool, name):
        self._pool, self._name = pool, name

    def load(self):
        self._pool.get(self._name)

    def __getattr__(self, key):
        return getattr(self._pool.get(self._name), key)


class ModelPool:
    """`asr` (Qwen3-ASR) and `speaker` (MOSS plus CAM++) slots; each object has `load()` and optionally `unload()`."""

    def __init__(self, asr=None, speaker=None, *, idle_seconds=IDLE_SECONDS, clock=time.monotonic):
        from .runtime import Runtime
        from .speaker_models import Models
        self.slots = {
            "asr": _Slot((lambda: asr) if asr is not None else Runtime),
            "speaker": _Slot((lambda: speaker) if speaker is not None else Models),
        }
        self.idle_seconds, self.clock = idle_seconds, clock
        self.lock = threading.RLock()
        self.busy, self.last_used, self.timer = 0, clock(), None

    def get(self, name):
        """The loaded model for `name`; loads it now if needed. Call inside `use()`."""
        slot = self.slots[name]
        with self.lock:
            if not slot.loaded:
                if slot.value is None:
                    slot.value = slot.factory()
                started = time.monotonic()
                load = getattr(slot.value, "load", None)
                if load is not None:
                    load()
                slot.loaded = True
                log.info("Loaded %s model in %.1fs", name, time.monotonic() - started)
            return slot.value

    def lazy(self, name):
        return Lazy(self, name)

    @contextmanager
    def use(self):
        with self.lock:
            self.busy += 1
            self.last_used = self.clock()
            if self.timer is not None:
                self.timer.cancel()
                self.timer = None
        try:
            yield self
        finally:
            with self.lock:
                self.busy -= 1
                self.last_used = self.clock()
                if self.busy == 0:
                    self._arm(self.idle_seconds)

    def _arm(self, delay):
        if not any(s.loaded for s in self.slots.values()):
            return
        if self.timer is not None:
            self.timer.cancel()
        self.timer = threading.Timer(max(delay, 0.0), self._expire)
        self.timer.daemon = True
        self.timer.start()

    def _expire(self):
        with self.lock:
            self.timer = None
            if self.busy:
                return
            remaining = self.idle_seconds - (self.clock() - self.last_used)
            if remaining > 1e-3:
                self._arm(remaining)
                return
            self.unload()

    def unload(self):
        with self.lock:
            names = [name for name, slot in self.slots.items() if slot.loaded]
            for name in names:
                slot = self.slots[name]
                release = getattr(slot.value, "unload", None)
                if release is not None:
                    release()
                slot.loaded = False
            if names:
                gc.collect()
                clear_mlx_cache()
                log.info("Unloaded idle models: %s", ", ".join(names))

    def state(self):
        return {name: slot.loaded for name, slot in self.slots.items()}

    def close(self):
        with self.lock:
            if self.timer is not None:
                self.timer.cancel()
                self.timer = None
            self.unload()


def measured(fn, *args, **kwargs):
    """Run `fn` and add the process CPU seconds it took as `cpu_seconds` to its dict result."""
    started = time.process_time()
    result = fn(*args, **kwargs)
    if isinstance(result, dict):
        result["cpu_seconds"] = round(time.process_time() - started, 3)
    return result
