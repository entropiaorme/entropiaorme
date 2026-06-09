"""Reduction of published payloads to their JSON wire form.

Bus payloads are live Python objects (Pydantic models, raw ``datetime``
instants, plain dicts); their JSON wire form is what crosses a language
boundary. The equivalence apparatus captures streams in wire form so a
Rust leg fed the captured stream normalises identically to the Python
leg fed the live objects: the raw-capture fixtures and the test-mode
event sink both serialise through this reduction.
"""

from __future__ import annotations

from datetime import datetime
from typing import Any

from pydantic import BaseModel


def wire(payload: Any) -> Any:
    """Reduce a published payload to its JSON wire form.

    Mirrors the Normalizer's pre-walk reductions so normalising the wire
    form yields the same bytes as normalising the live object:

    - a ``BaseModel`` reduces via ``model_dump(mode="json")`` (the
      Normalizer's BaseModel branch);
    - a raw ``datetime`` reduces to ``isoformat()`` (the Normalizer's
      datetime branch keys its symbol table on exactly
      ``value.isoformat()``, so the string form lands on the same
      ``<TS_N>`` symbol);
    - dicts and lists recurse so a nested datetime in a plain dict
      payload is reduced too.
    """
    if isinstance(payload, BaseModel):
        return wire(payload.model_dump(mode="json"))
    if isinstance(payload, datetime):
        return payload.isoformat()
    if isinstance(payload, dict):
        return {key: wire(value) for key, value in payload.items()}
    if isinstance(payload, (list, tuple)):
        return [wire(item) for item in payload]
    return payload
