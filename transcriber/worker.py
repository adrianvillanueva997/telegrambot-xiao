# Copyright (c) 2026 Adrian Villanueva
"""Persistent Spanish voice transcription worker; JSON lines on standard IO."""

import json
import os
import subprocess
import sys
from pathlib import Path
from threading import Timer
from typing import TYPE_CHECKING

from vosk import KaldiRecognizer, Model, SetLogLevel

if TYPE_CHECKING:
    from collections.abc import Callable


def transcribe(model: Model, path: str, progress: Callable[[str], None]) -> str:
    """Convert a voice file to PCM and transcribe it with the shared model."""
    process = subprocess.Popen(  # noqa: S603 - arguments are passed without a shell
        [  # noqa: S607 - ffmpeg is installed in the runtime image
            "ffmpeg",
            "-nostdin",
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            "ogg",
            "-i",
            path,
            "-t",
            "601",
            "-f",
            "s16le",
            "-ac",
            "1",
            "-ar",
            "16000",
            "pipe:1",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    watchdog = Timer(840, process.kill)
    watchdog.start()
    try:
        recognizer = KaldiRecognizer(model, 16000)
        parts = []
        total = 0
        next_progress = 0
        if process.stdout is None:
            msg = "FFmpeg stdout unavailable"
            raise ValueError(msg)
        while chunk := process.stdout.read(4000):
            total += len(chunk)
            if total > 600 * 16000 * 2:
                msg = "audio exceeds duration limit"
                raise ValueError(msg)
            if recognizer.AcceptWaveform(chunk):
                parts.append(json.loads(recognizer.Result()).get("text", ""))
            if total >= next_progress:
                partial = json.loads(recognizer.PartialResult()).get("partial", "")
                progress("\n".join(part for part in [*parts, partial] if part))
                next_progress = total + 64000
        if process.wait() != 0:
            raise subprocess.CalledProcessError(process.returncode, process.args)
        parts.append(json.loads(recognizer.FinalResult()).get("text", ""))
        return "\n".join(part for part in parts if part)
    finally:
        watchdog.cancel()
        if process.poll() is None:
            process.kill()
        process.wait()
        if process.stdout is not None:
            process.stdout.close()


def main() -> None:
    """Load the model once and process one request at a time."""
    SetLogLevel(-1)
    model = Model(str(Path(os.environ["VOSK_MODEL_PATH"])))
    sys.stdout.write(json.dumps({"ready": True}) + "\n")
    sys.stdout.flush()
    for line in sys.stdin:
        request = json.loads(line)
        try:

            def progress(text: str, request_id: int = request["id"]) -> None:
                sys.stdout.write(
                    json.dumps(
                        {"id": request_id, "partial": text},
                        ensure_ascii=False,
                    )
                    + "\n",
                )
                sys.stdout.flush()

            text = transcribe(model, request["path"], progress)
            response = {"id": request["id"], "text": text}
        except (OSError, subprocess.SubprocessError, ValueError, KeyError) as error:
            sys.stderr.write(f"transcription failed: {error}\n")
            sys.stderr.flush()
            response = {"id": request.get("id"), "error": "transcription failed"}
        sys.stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
