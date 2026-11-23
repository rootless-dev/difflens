"""JSON line logs on stderr, as every DiffLens analyzer writes them.

Each line is one object with `time`, `level`, `msg` and `service`, then the
fields given. The level comes from DIFFLENS_LOG_LEVEL; an unknown name falls
back to info, because losing logs is worse than logging too much.
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from typing import Any, TextIO

_LEVELS = {"debug": 10, "trace": 10, "info": 20, "warn": 30, "warning": 30, "error": 40}


class Logger:
    def __init__(self, stream: TextIO, service: str, level: str) -> None:
        self._stream = stream
        self._service = service
        self._threshold = _LEVELS.get(level.strip().lower(), _LEVELS["info"])

    def debug(self, msg: str, **fields: Any) -> None:
        self._write("debug", msg, fields)

    def info(self, msg: str, **fields: Any) -> None:
        self._write("info", msg, fields)

    def warn(self, msg: str, **fields: Any) -> None:
        self._write("warn", msg, fields)

    def error(self, msg: str, **fields: Any) -> None:
        self._write("error", msg, fields)

    def _write(self, level: str, msg: str, fields: dict[str, Any]) -> None:
        if _LEVELS[level] < self._threshold:
            return
        record = {
            "time": datetime.now(timezone.utc).isoformat(timespec="milliseconds"),
            "level": level,
            "service": self._service,
            "msg": msg,
            **fields,
        }
        self._stream.write(json.dumps(record, ensure_ascii=False, default=str) + "\n")
        self._stream.flush()
