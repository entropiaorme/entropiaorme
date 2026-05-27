"""HTTP-level validation tests for the codex endpoints.

These drive the FastAPI app through its host/origin guard to assert the
request-validation contract at the API boundary, complementing the
service-level coverage in ``test_codex_service.py``.
"""

import pytest
from fastapi.testclient import TestClient

from backend.main import BACKEND_PORT, app

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
