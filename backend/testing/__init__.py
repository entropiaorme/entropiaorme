"""E2E replay harness — test-mode infrastructure.

This package holds the apparatus the backend leans on at test time:

- ``config``: ``TestModeConfig``, the process-wide flag set at startup
  to swap real-world dependencies for test-controlled ones.
- ``clock``: ``Clock`` interface plus ``RealClock`` and ``MockClock``
  implementations for time injection.
- ``keystroke_source``: ``KeystrokeSource`` interface plus
  ``MockKeystrokeSource`` for input-listener tests.
- ``capturer``: ``FixtureCapturer`` scaffolding for serving pre-recorded
  panel screenshots in OCR tests.
- ``replay``: helpers for streaming a scenario's chat replay into a
  watcher-tailed file and draining the watcher's tick buffer.

The harness is built round-by-round per the lane working doc:
R1 lands the scaffolding plus the chatlog-redirection scenario; later
rounds wire each seam into production callers.
"""
