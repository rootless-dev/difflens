"""Running tools in their own process group, within a deadline."""

from __future__ import annotations

import os
import signal
import subprocess
import threading
import time
from dataclasses import dataclass
from typing import IO, Optional

MAX_CAPTURE = 4 << 20


@dataclass
class Result:
    code: int
    stdout: str
    stderr: str
    timed_out: bool = False
    missing: bool = False  # the program could not be started

    def first_line(self) -> str:
        for text in (self.stderr, self.stdout):
            for line in text.splitlines():
                if line.strip():
                    return line.strip()
        return f"exit status {self.code}"


class Tail:
    """Keeps the last limit characters written to it."""

    def __init__(self, limit: int = MAX_CAPTURE) -> None:
        self.limit = limit
        self.parts: list[str] = []
        self.size = 0
        self.lock = threading.Lock()

    def write(self, text: str) -> None:
        with self.lock:
            self.parts.append(text)
            self.size += len(text)
            if self.size > 2 * self.limit:
                joined = "".join(self.parts)[-self.limit :]
                self.parts, self.size = [joined], len(joined)

    def text(self) -> str:
        with self.lock:
            return "".join(self.parts)[-self.limit :]


def pump(stream: IO[bytes], into: Tail) -> threading.Thread:
    def run() -> None:
        for chunk in iter(lambda: stream.read1(65536) if hasattr(stream, "read1") else stream.read(65536), b""):
            into.write(chunk.decode("utf-8", "replace"))
        stream.close()

    thread = threading.Thread(target=run, daemon=True)
    thread.start()
    return thread


def start(cmd: list[str], cwd: str, env: Optional[dict[str, str]]) -> subprocess.Popen:
    return subprocess.Popen(
        cmd,
        cwd=cwd,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )


def stop(process: subprocess.Popen) -> None:
    """Terminates the process group, then kills it if it lingers."""
    for sig, wait in ((signal.SIGTERM, 3.0), (signal.SIGKILL, 5.0)):
        try:
            os.killpg(process.pid, sig)
        except (ProcessLookupError, PermissionError):
            return
        try:
            process.wait(wait)
            return
        except subprocess.TimeoutExpired:
            continue


def run(cmd: list[str], cwd: str, env: Optional[dict[str, str]] = None, timeout: float = 60.0) -> Result:
    try:
        process = start(cmd, cwd, env)
    except (FileNotFoundError, PermissionError, OSError) as error:
        return Result(code=127, stdout="", stderr=str(error), missing=True)
    out, err = Tail(), Tail()
    readers = [pump(process.stdout, out), pump(process.stderr, err)]  # type: ignore[arg-type]
    timed_out = False
    try:
        process.wait(max(timeout, 0.1))
    except subprocess.TimeoutExpired:
        timed_out = True
        stop(process)
    for reader in readers:
        reader.join(5)
    return Result(code=process.returncode if process.returncode is not None else -1,
                  stdout=out.text(), stderr=err.text(), timed_out=timed_out)


def remaining(deadline: float, floor: float = 1.0) -> float:
    return max(deadline - time.monotonic(), floor)
