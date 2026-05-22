"""Backend-wide pytest configuration.

Registers the ``--update-fingerprints`` CLI option so the e2e harness
tests can flip the golden-file workflow into write mode without
pytest complaining about an unrecognised flag when the e2e suite is
not the only thing being collected. Individual fixtures live in the
nearest conftest (e.g. ``e2e/conftest.py``).
"""

from __future__ import annotations


def pytest_addoption(parser):
    """Register backend-wide pytest CLI options.

    Currently exposes ``--update-fingerprints`` so the e2e harness can
    rewrite scenario goldens. Hoisting the registration to this
    backend-root conftest (rather than the e2e subdir conftest) keeps
    the flag recognised regardless of which subset of tests is being
    collected, so ``pytest backend/tests/test_fingerprint.py
    --update-fingerprints`` does not error on argument parsing.
    """
    parser.addoption(
        "--update-fingerprints",
        action="store_true",
        default=False,
        help=(
            "E2E harness: rewrite scenario goldens with the current run's "
            "output. Surfaces the diff vs the prior golden for review "
            "before writing; default behaviour without the flag asserts "
            "against goldens and fails on divergence."
        ),
    )
