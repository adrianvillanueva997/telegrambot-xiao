# Copyright (c) 2026 Adrian Villanueva
"""Persistent Spanish voice transcription worker; JSON lines on standard IO."""

import json
import os
import subprocess
import sys
from collections.abc import Callable
from pathlib import Path

from vosk import KaldiRecognizer, Model, SetLogLevel


def transcribe(model: Model, path: str, progress: Callable[[str], None]) -> str:
    """Convert a voice file to PCM and transcribe it with the shared model."""
    audio = subprocess.run(  # noqa: S603 - arguments are passed without a shell
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
        check=True,
        capture_output=True,
        timeout=120,
    ).stdout
    if len(audio) > 600 * 16000 * 2:
        msg = "audio exceeds duration limit"
        raise ValueError(msg)
    recognizer = KaldiRecognizer(model, 16000)
    parts = []
    for start in range(0, len(audio), 4000):
        if recognizer.AcceptWaveform(audio[start : start + 4000]):
            parts.append(json.loads(recognizer.Result()).get("text", ""))
        if start % 64000 == 0:
            partial = json.loads(recognizer.PartialResult()).get("partial", "")
            progress("\n".join(part for part in [*parts, partial] if part))
    parts.append(json.loads(recognizer.FinalResult()).get("text", ""))
    return "\n".join(part for part in parts if part)


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
