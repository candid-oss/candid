"""Progress and cancellation points for the inference running in this thread.

A job installs a sink; `report()` at each chunk or window boundary forwards progress to it, and the sink raises
`Cancelled` once the job was cancelled, so cancellation takes effect at the next boundary. Without a sink (plain
requests) `report()` does nothing.
"""
import contextvars

SINK = contextvars.ContextVar("candid_voice_progress", default=None)


class Cancelled(Exception):
    """The job was cancelled; raised at the next progress point."""


def report(stage, completed_seconds=None, total_seconds=None):
    sink = SINK.get()
    if sink is not None:
        sink(stage, completed_seconds, total_seconds)
