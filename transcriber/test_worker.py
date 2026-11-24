# Copyright (c) 2026 Adrian Villanueva
"""Tests for the transcription worker."""

import json
import subprocess
import unittest
from unittest.mock import MagicMock, patch

import worker


class TranscriptionTest(unittest.TestCase):
    """Check segmented Vosk results and FFmpeg errors."""

    @patch("worker.KaldiRecognizer")
    @patch("worker.subprocess.run")
    def test_joins_results(self, run: MagicMock, recognizer_class: MagicMock) -> None:
        """Include complete segments and the final segment."""
        run.return_value.stdout = bytes(5000)
        recognizer = recognizer_class.return_value
        recognizer.AcceptWaveform.side_effect = [True, False]
        recognizer.Result.return_value = json.dumps({"text": "hola"})
        recognizer.FinalResult.return_value = json.dumps({"text": "mundo"})
        assert worker.transcribe(MagicMock(), "voice.ogg") == "hola\nmundo"  # noqa: S101 - test assertion

    @patch("worker.subprocess.run")
    def test_ffmpeg_failure_propagates(self, run: MagicMock) -> None:
        """Let the worker report decoding errors to the supervisor."""
        run.side_effect = subprocess.TimeoutExpired("ffmpeg", 60)
        with self.assertRaises(subprocess.TimeoutExpired):  # noqa: PT027 - standard-library test suite
            worker.transcribe(MagicMock(), "voice.ogg")


if __name__ == "__main__":
    unittest.main()
