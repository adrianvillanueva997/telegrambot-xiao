# Copyright (c) 2026 Adrian Villanueva
"""Tests for the transcription worker."""

import io
import json
import subprocess
import unittest
from unittest.mock import MagicMock, patch

import worker


class TranscriptionTest(unittest.TestCase):
    """Check streamed Vosk results and FFmpeg errors."""

    @patch("worker.KaldiRecognizer")
    @patch("worker.subprocess.Popen")
    def test_joins_results(self, popen: MagicMock, recognizer_class: MagicMock) -> None:
        """Include complete segments and the final segment."""
        process = popen.return_value
        process.stdout = io.BytesIO(bytes(5000))
        process.wait.return_value = 0
        recognizer = recognizer_class.return_value
        recognizer.AcceptWaveform.side_effect = [True, False]
        recognizer.Result.return_value = json.dumps({"text": "hola"})
        recognizer.FinalResult.return_value = json.dumps({"text": "mundo"})
        recognizer.PartialResult.return_value = json.dumps({"partial": ""})
        progress = MagicMock()
        assert worker.transcribe(MagicMock(), "voice.ogg", progress) == "hola\nmundo"  # noqa: S101 - test assertion
        progress.assert_called_once_with("hola")
        process.stdout.close()

    @patch("worker.KaldiRecognizer")
    @patch("worker.subprocess.Popen")
    def test_rejects_audio_over_ten_minutes(
        self,
        popen: MagicMock,
        recognizer_class: MagicMock,
    ) -> None:
        """Reject audio over the duration cap even if metadata was incorrect."""
        process = popen.return_value
        process.stdout = io.BytesIO(bytes(600 * 16000 * 2 + 2))
        recognizer_class.return_value.PartialResult.return_value = '{"partial":""}'
        recognizer_class.return_value.AcceptWaveform.return_value = False
        process.poll.return_value = None
        with self.assertRaises(ValueError):  # noqa: PT027 - standard-library test suite
            worker.transcribe(MagicMock(), "voice.ogg", MagicMock())
        process.kill.assert_called_once()
        process.wait.assert_called_once()

    @patch("worker.KaldiRecognizer")
    @patch("worker.subprocess.Popen")
    def test_ffmpeg_failure_propagates(
        self,
        popen: MagicMock,
        recognizer_class: MagicMock,
    ) -> None:
        """Let the worker report decoding errors to the supervisor."""
        process = popen.return_value
        process.stdout = io.BytesIO()
        process.wait.return_value = 1
        process.returncode = 1
        recognizer_class.return_value.FinalResult.return_value = "{}"
        with self.assertRaises(subprocess.CalledProcessError):  # noqa: PT027 - standard-library test suite
            worker.transcribe(MagicMock(), "voice.ogg", MagicMock())


if __name__ == "__main__":
    unittest.main()
