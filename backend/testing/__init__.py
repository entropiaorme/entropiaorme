"""E2E replay harness test-mode infrastructure.

This package holds the apparatus the backend leans on at test time:

- ``config``: ``TestModeConfig``, the process-wide flag set at startup
  to swap real-world dependencies for test-controlled ones.
- ``clock``: ``Clock`` interface plus ``RealClock`` and ``MockClock``
  implementations for time injection.
- ``keystroke_source``: ``KeystrokeSource`` interface plus
  ``MockKeystrokeSource`` for input-listener tests.
- ``capturer``: ``FixtureCapturer``, the ``ScreenCapturer`` stand-in
  that serves a recorded panel PNG to the OCR pipeline in tests.
- ``replay``: helpers for streaming a scenario's chat replay into a
  watcher-tailed file and draining the watcher's tick buffer.

The harness is built incrementally: the initial work lands the
scaffolding plus the chatlog-redirection scenario, and later work
wires each seam into its production callers.
"""
