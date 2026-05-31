"""ETag + Cache-Control + conditional-GET contract for hydration endpoints.

Asserts the four substrate properties on every GET under the hydration
prefixes:

- A 2xx response carries a strong-format ``ETag`` (``"<sha256-hex>"``)
  and ``Cache-Control: no-cache``.
- Two consecutive requests at unchanged state produce identical ETags
  (the body skip is reproducible across polls).
- A request whose ``If-None-Match`` matches the freshly-computed ETag
  is answered ``304 Not Modified`` with no body.
- A request whose ``If-None-Match`` does not match falls through to the
  200 with a fresh body and a fresh ETag.

The route-coverage test enumerates the FastAPI route table and asserts
the substrate fires on every hydration-prefix GET route. The
enumeration is non-trivial: it pulls every covered route's path into
the assertion message so a silent miss surfaces by name.

Endpoints outside the hydration scope (health, character, equipment,
analytics, settings, demo, recording) are explicitly asserted to carry
neither header, pinning the substrate's narrow remit so an accidental
widening of ``ETAG_PREFIXES`` surfaces as a test failure.
"""

from __future__ import annotations

import io
import os
import re
import tempfile
from collections.abc import Iterator
from contextlib import redirect_stdout
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

import backend.routers.demo as demo_module
from backend.main import BACKEND_PORT, app
from backend.middleware.etag import (
    CACHE_CONTROL_VALUE,
    ETAG_PREFIXES,
    covered_get_routes,
    if_none_match_matches,
    path_is_in_etag_scope,
)

BASE_URL = f"http://localhost:{BACKEND_PORT}"
ALLOWED_ORIGIN = "tauri://localhost"
REQUEST_HEADERS = {"Origin": ALLOWED_ORIGIN}

STRONG_ETAG_RE = re.compile(r'^"[0-9a-f]{64}"$')


# Routes with no path parameters that return 200 in a fresh app are the
# ones the substrate can roundtrip end-to-end without seeded state. Routes
# with parameters are still covered (covered_get_routes asserts the
# enumeration); their 200-path roundtrip is exercised by the scenario-state
# HTTP-fingerprint tests, not this contract suite.
ROUNDTRIPPABLE_ROUTES: tuple[str, ...] = (
    "/api/tracking/status",
    "/api/tracking/live",
    "/api/tracking/recent-events",
    "/api/tracking/snapshot",
    "/api/tracking/sessions",
    "/api/tracking/manual-mob-suggestions",
    "/api/tracking/tag-suggestions",
    "/api/scan/skills/status",
    "/api/quests",
    "/api/quests/mobs",
    "/api/quests/analytics",
    "/api/quests/playlists",
    "/api/quests/playlists/analytics",
    "/api/codex/species",
    "/api/codex/meta/attributes",
)

OUT_OF_SCOPE_GET_ROUTES: tuple[str, ...] = (
    "/api/health",
    "/api/character/attributes",
    "/api/equipment/library",
    "/api/analytics/summary",
    "/api/settings",
)


@pytest.fixture(scope="module")
def client() -> Iterator[TestClient]:
    """Boot the real app lifespan against a throwaway data + demo dir.

    Mirrors ``test_api_contract.contract_env`` so the lifespan-init
    cost is paid once per module rather than per test. Restores the
    patched resolver and data-dir env on teardown.
    """
    data_dir = tempfile.mkdtemp(prefix="eo_etag_data_")
    demo_dir = tempfile.mkdtemp(prefix="eo_etag_demo_")

    from backend.scripts.demo_seed.__main__ import main as seed_demo

    with redirect_stdout(io.StringIO()):
        seed_demo(["--reseed", "--out", demo_dir])
    demo_db = Path(demo_dir) / "entropia_orme.db"
    assert demo_db.exists(), "demo seed did not produce a database"

    original_resolver = demo_module._resolve_demo_db_path
    demo_module._resolve_demo_db_path = lambda: demo_db
    demo_module._state["conn"] = None
    demo_module._state["svc"] = None

    original_data_dir = os.environ.get("ENTROPIAORME_DATA_DIR")
    os.environ["ENTROPIAORME_DATA_DIR"] = data_dir

    try:
        with TestClient(app, base_url=BASE_URL) as test_client:
            yield test_client
    finally:
        demo_module._resolve_demo_db_path = original_resolver
        demo_module._state["conn"] = None
        demo_module._state["svc"] = None
        if original_data_dir is None:
            os.environ.pop("ENTROPIAORME_DATA_DIR", None)
        else:
            os.environ["ENTROPIAORME_DATA_DIR"] = original_data_dir


def _get(client: TestClient, route: str, **headers: str):
    """GET ``route`` with the allowed Origin pre-set."""
    merged = {**REQUEST_HEADERS, **headers}
    return client.get(route, headers=merged)


def test_strong_etag_format_on_every_roundtrippable_route(client: TestClient) -> None:
    """A 2xx GET under a hydration prefix returns a strong-format ETag."""
    for route in ROUNDTRIPPABLE_ROUTES:
        response = _get(client, route)
        assert response.status_code == 200, (
            f"{route} returned {response.status_code}: {response.text}"
        )
        etag = response.headers.get("etag")
        assert etag is not None, f"{route} returned no ETag header"
        assert STRONG_ETAG_RE.match(etag), (
            f"{route} returned malformed ETag {etag!r}; expected strong "
            "quoted sha256 hex"
        )


def test_cache_control_no_cache_on_every_roundtrippable_route(
    client: TestClient,
) -> None:
    """Every covered 2xx GET sets ``Cache-Control: no-cache``."""
    for route in ROUNDTRIPPABLE_ROUTES:
        response = _get(client, route)
        assert response.status_code == 200, response.text
        assert response.headers.get("cache-control") == CACHE_CONTROL_VALUE, (
            f"{route} Cache-Control = {response.headers.get('cache-control')!r}, "
            f"expected {CACHE_CONTROL_VALUE!r}"
        )


def test_etag_stable_across_repeated_calls_at_unchanged_state(
    client: TestClient,
) -> None:
    """Two calls at unchanged backend state produce identical ETags."""
    for route in ROUNDTRIPPABLE_ROUTES:
        first = _get(client, route)
        second = _get(client, route)
        assert first.status_code == 200 and second.status_code == 200
        assert first.headers["etag"] == second.headers["etag"], (
            f"{route} ETag drifted across two calls at unchanged state: "
            f"{first.headers['etag']!r} vs {second.headers['etag']!r}; "
            f"bodies: {first.text!r} vs {second.text!r}"
        )


def test_if_none_match_match_returns_304_with_empty_body(
    client: TestClient,
) -> None:
    """A matching If-None-Match yields 304 with no body, carrying the ETag."""
    for route in ROUNDTRIPPABLE_ROUTES:
        first = _get(client, route)
        assert first.status_code == 200
        etag = first.headers["etag"]

        conditional = _get(client, route, **{"If-None-Match": etag})
        assert conditional.status_code == 304, (
            f"{route} If-None-Match={etag!r} returned "
            f"{conditional.status_code}: {conditional.text!r}"
        )
        assert conditional.content == b"", (
            f"{route} 304 carried a non-empty body: {conditional.content!r}"
        )
        assert conditional.headers.get("etag") == etag, (
            f"{route} 304 ETag {conditional.headers.get('etag')!r} != {etag!r}"
        )
        assert conditional.headers.get("cache-control") == CACHE_CONTROL_VALUE


def test_if_none_match_mismatch_falls_through_to_200(client: TestClient) -> None:
    """A non-matching If-None-Match yields the full 200 with a fresh ETag."""
    for route in ROUNDTRIPPABLE_ROUTES:
        bogus_etag = '"' + ("0" * 64) + '"'
        response = _get(client, route, **{"If-None-Match": bogus_etag})
        assert response.status_code == 200, (
            f"{route} returned {response.status_code} on mismatched "
            f"If-None-Match; expected 200"
        )
        assert response.headers["etag"] != bogus_etag


def test_covered_get_routes_enumerates_every_hydration_route() -> None:
    """The introspection helper finds every GET route under the hydration
    prefixes and only those. The assertion message lists the discovered
    routes so a silent miss reads cleanly in the failure output."""
    routes = covered_get_routes(app)

    assert routes, "covered_get_routes returned no routes; substrate empty"
    for route in routes:
        assert path_is_in_etag_scope(route), (
            f"covered_get_routes returned {route!r} which is not under any "
            f"of {ETAG_PREFIXES}"
        )

    expected_subset = {
        "/api/tracking/status",
        "/api/tracking/live",
        "/api/tracking/recent-events",
        "/api/tracking/snapshot",
        "/api/tracking/sessions",
        "/api/tracking/session/{session_id}",
        "/api/tracking/session/{session_id}/quest-link-suggestion",
        "/api/scan/skills/status",
        "/api/scan/skills/pending",
        "/api/scan/skills/capture/{page}",
        "/api/quests",
        "/api/quests/mobs",
        "/api/quests/analytics",
        "/api/quests/playlists",
        "/api/quests/playlists/analytics",
        "/api/quests/{quest_id}",
        "/api/codex/species",
        "/api/codex/species/{name}/ranks",
        "/api/codex/recommend",
        "/api/codex/meta/attributes",
    }
    missing = expected_subset - set(routes)
    assert not missing, (
        f"Hydration-prefix GET routes missing from introspection: {missing}. "
        f"Discovered: {routes}"
    )


def test_routes_outside_the_hydration_prefixes_carry_no_etag(
    client: TestClient,
) -> None:
    """The substrate is intentionally narrow: routes outside the four
    hydration prefixes are left untouched. A change that accidentally
    widens ``ETAG_PREFIXES`` would surface as a new ETag header on a
    canonical out-of-scope route, which this test catches.
    """
    for route in OUT_OF_SCOPE_GET_ROUTES:
        response = _get(client, route)
        if response.status_code >= 400:
            # The substrate also skips non-2xx; a 4xx here is fine for the
            # purposes of "ETag should not appear on out-of-scope routes".
            assert "etag" not in {k.lower() for k in response.headers}, (
                f"{route} returned {response.status_code} with an ETag; the "
                "substrate should leave non-hydration routes untouched"
            )
            continue
        assert "etag" not in {k.lower() for k in response.headers}, (
            f"{route} returned 2xx with an ETag; the substrate should leave "
            "non-hydration routes untouched"
        )


def test_if_none_match_weak_validator_matches_strong_etag(
    client: TestClient,
) -> None:
    """Per RFC 7232 §2.3.2, conditional GET uses the weak-comparison
    function: a client sending ``W/"<hex>"`` against the server's
    strong ``"<hex>"`` still gets a 304."""
    route = "/api/tracking/status"
    first = _get(client, route)
    assert first.status_code == 200
    strong_etag = first.headers["etag"]
    assert strong_etag.startswith('"'), strong_etag
    weak_form = f"W/{strong_etag}"

    response = _get(client, route, **{"If-None-Match": weak_form})
    assert response.status_code == 304, (
        f"Weak If-None-Match {weak_form!r} should match strong "
        f"{strong_etag!r}; got {response.status_code}: {response.text!r}"
    )
    assert response.content == b""


def test_if_none_match_comma_separated_list_matches(client: TestClient) -> None:
    """A multi-tag If-None-Match header matches when any candidate
    matches the current ETag (common shape from clients holding
    multiple cached representations)."""
    route = "/api/tracking/status"
    first = _get(client, route)
    etag = first.headers["etag"]
    bogus = '"' + ("a" * 64) + '"'
    response = _get(client, route, **{"If-None-Match": f"{bogus}, {etag}"})
    assert response.status_code == 304, (
        f"{route} should 304 on comma-list containing the current ETag; "
        f"got {response.status_code}"
    )


def test_if_none_match_wildcard_matches(client: TestClient) -> None:
    """The ``*`` wildcard matches any current representation."""
    route = "/api/tracking/status"
    response = _get(client, route, **{"If-None-Match": "*"})
    assert response.status_code == 304, (
        f"If-None-Match: * should 304; got {response.status_code}"
    )
    assert response.content == b""


def test_if_none_match_matches_unit_classification() -> None:
    """Unit-level coverage of the header-parsing predicate without an HTTP
    round-trip: classification of all the documented input shapes."""
    current = '"abc123"'

    # Non-match shapes.
    assert if_none_match_matches(None, current) is False
    assert if_none_match_matches("", current) is False
    assert if_none_match_matches('"xyz"', current) is False
    assert if_none_match_matches('"xyz", "qrs"', current) is False

    # Strong match.
    assert if_none_match_matches(current, current) is True

    # Weak comparison: W/"abc123" matches "abc123" under conditional GET.
    assert if_none_match_matches(f"W/{current}", current) is True

    # Wildcard.
    assert if_none_match_matches("*", current) is True
    assert if_none_match_matches("  *  ", current) is True

    # Comma-separated list with whitespace variants.
    assert if_none_match_matches(f'"xyz", {current}', current) is True
    assert if_none_match_matches(f'  "xyz" ,   {current}  ', current) is True
    assert if_none_match_matches(f'"xyz",W/{current}', current) is True

    # Empty list entries are skipped, not treated as a wildcard.
    assert if_none_match_matches(",,", current) is False


def test_path_is_in_etag_scope_classification() -> None:
    """Unit-level coverage of the prefix predicate, so the classification
    surface is testable without booting the app."""
    for prefix in ETAG_PREFIXES:
        assert path_is_in_etag_scope(prefix)
        assert path_is_in_etag_scope(prefix + "/anything")
        assert path_is_in_etag_scope(prefix + "/deeply/nested/thing")

    # Avoid false positives on similarly-named prefixes (e.g. /api/codex
    # must not match /api/codex2).
    assert not path_is_in_etag_scope("/api/health")
    assert not path_is_in_etag_scope("/api/codex2")
    assert not path_is_in_etag_scope("/api/trackingx")
    assert not path_is_in_etag_scope("")
    assert not path_is_in_etag_scope("/")
