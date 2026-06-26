"""HTTP-level validation and handler-translation tests for the codex endpoints.

The first group drives the FastAPI app through its host/origin guard to assert
the request-validation contract at the API boundary. The second group calls the
claim/calibrate/meta handlers directly through the service-locator seam to cover
their result-passthrough, the tracking-suppression side effect, and the
``ValueError`` -> 400 translation. Both complement the service-level coverage in
``test_codex_service.py`` and stay free of any DB or lifespan.
"""

from types import SimpleNamespace

import pytest
from fastapi import HTTPException
from fastapi.testclient import TestClient

from backend.main import BACKEND_PORT, app
from backend.routers import codex

# The host guard rejects requests whose Host is not in the allowed set (derived
# from the backend port) before routing, so the client base URL must match it,
# and the request carries an allowed Origin.
_BASE_URL = f"http://localhost:{BACKEND_PORT}"
_HEADERS = {"Origin": "tauri://localhost"}

# No lifespan context is entered: an out-of-domain rank is rejected at request
# validation, before the handler (and its service lookups) runs, so the app
# need not be booted for these cases.
client = TestClient(app, base_url=_BASE_URL)


@pytest.mark.parametrize("rank", [-25, -1, 0, 26, 100])
def test_recommend_rejects_out_of_domain_rank(rank):
    """Codex ranks run 1..25; anything outside returns 422, never a 500.

    Out-of-range ranks previously reached the reward and cost lookups and raised
    an unhandled ``IndexError`` (a negative rank wrapped to the wrong row, zero
    and over-range values overflowed), surfacing as a 500. The boundary
    constraint now rejects them at request validation.
    """
    response = client.get(
        "/api/codex/recommend",
        params={"species_name": "Atrox", "rank": rank},
        headers=_HEADERS,
    )
    assert response.status_code == 422


# ── Handler-translation tests (direct call through the service-locator seam) ──


def _codex_services(*, claim=None, calibrate=None, meta=None, unclaim=None, is_tracking=False):
    suppressed: list[str] = []
    svc = SimpleNamespace(
        codex_service=SimpleNamespace(
            claim_rank=claim or (lambda *a: {"ok": True}),
            calibrate=calibrate or (lambda *a: {"ok": True}),
            meta_claim=meta or (lambda *a: {"ok": True}),
            unclaim_rank=unclaim or (lambda *a: {"ok": True}),
        ),
        tracker=SimpleNamespace(is_tracking=is_tracking),
        skill_tracker=SimpleNamespace(suppress_next=suppressed.append),
    )
    return svc, suppressed


def test_claim_rank_returns_service_result(monkeypatch):
    svc, _ = _codex_services(claim=lambda s, r, sk: {"rank": r, "reward": 0.25})
    monkeypatch.setattr(codex, "get_services", lambda: svc)

    result = codex.claim_rank(
        codex.ClaimRequest(species_name="Atrox", rank=5, skill_name="Handgun")
    )

    assert result == {"rank": 5, "reward": 0.25}


def test_claim_rank_suppresses_skill_gain_while_tracking(monkeypatch):
    svc, suppressed = _codex_services(is_tracking=True)
    monkeypatch.setattr(codex, "get_services", lambda: svc)

    codex.claim_rank(
        codex.ClaimRequest(species_name="Atrox", rank=5, skill_name="Handgun")
    )

    assert suppressed == ["Handgun"]


def test_claim_rank_maps_value_error_to_400(monkeypatch):
    def boom(*_a):
        raise ValueError("unknown species")

    svc, _ = _codex_services(claim=boom)
    monkeypatch.setattr(codex, "get_services", lambda: svc)

    with pytest.raises(HTTPException) as exc:
        codex.claim_rank(
            codex.ClaimRequest(species_name="Nope", rank=5, skill_name="Handgun")
        )

    assert exc.value.status_code == 400
    assert exc.value.detail == "unknown species"


def test_calibrate_returns_service_result(monkeypatch):
    svc, _ = _codex_services(calibrate=lambda s, r: {"species": s, "rank": r})
    monkeypatch.setattr(codex, "get_services", lambda: svc)

    result = codex.calibrate(codex.CalibrateRequest(species_name="Atrox", rank=10))

    assert result == {"species": "Atrox", "rank": 10}


def test_calibrate_maps_value_error_to_400(monkeypatch):
    def boom(*_a):
        raise ValueError("bad rank")

    svc, _ = _codex_services(calibrate=boom)
    monkeypatch.setattr(codex, "get_services", lambda: svc)

    with pytest.raises(HTTPException) as exc:
        codex.calibrate(codex.CalibrateRequest(species_name="Atrox", rank=10))

    assert exc.value.status_code == 400


def test_unclaim_rank_returns_service_result(monkeypatch):
    svc, _ = _codex_services(
        unclaim=lambda s: {
            "speciesName": s,
            "rank": 3,
            "skillName": "Handgun",
            "pedValue": 0.25,
        }
    )
    monkeypatch.setattr(codex, "get_services", lambda: svc)

    result = codex.unclaim_rank(codex.UnclaimRequest(species_name="Atrox"))

    assert result == {
        "speciesName": "Atrox",
        "rank": 3,
        "skillName": "Handgun",
        "pedValue": 0.25,
    }


def test_unclaim_rank_maps_value_error_to_400(monkeypatch):
    def boom(*_a):
        raise ValueError("No claimed rank to unclaim for 'Atrox'")

    svc, _ = _codex_services(unclaim=boom)
    monkeypatch.setattr(codex, "get_services", lambda: svc)

    with pytest.raises(HTTPException) as exc:
        codex.unclaim_rank(codex.UnclaimRequest(species_name="Atrox"))

    assert exc.value.status_code == 400
    assert exc.value.detail == "No claimed rank to unclaim for 'Atrox'"


def test_meta_claim_suppresses_attribute_gain_while_tracking(monkeypatch):
    svc, suppressed = _codex_services(
        meta=lambda attr: {"attribute": attr}, is_tracking=True
    )
    monkeypatch.setattr(codex, "get_services", lambda: svc)

    result = codex.meta_claim(codex.MetaClaimRequest(attribute_name="Agility"))

    assert result == {"attribute": "Agility"}
    assert suppressed == ["Agility"]


def test_meta_claim_maps_value_error_to_400(monkeypatch):
    def boom(*_a):
        raise ValueError("no meta reward")

    svc, _ = _codex_services(meta=boom)
    monkeypatch.setattr(codex, "get_services", lambda: svc)

    with pytest.raises(HTTPException) as exc:
        codex.meta_claim(codex.MetaClaimRequest(attribute_name="Agility"))

    assert exc.value.status_code == 400
