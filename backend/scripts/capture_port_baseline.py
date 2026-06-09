"""Capture the Python backend's performance and coverage baseline.

The numbers captured here are the old-implementation reference the
native-backend port is graded against in flight: process cold-start,
idle memory, graceful-shutdown time, per-endpoint hydration latency,
leaf hot-path micro-benchmarks, OCR latency and memory, artefact sizes,
and the per-module branch-coverage table. `backend/architecture/
PORT-BASELINE.md` carries the prose (what each band means, the variance
tolerances, the reproduction commands); this script owns measurement
and fills the document's generated blocks.

Canonical invocation (from the repo root, venv python; the coverage
JSON comes from a pinned-order suite run first):

    python -m pytest -m "fast or standard" -p no:randomly --cov=backend \
        --cov-branch --cov-report= -n auto --dist=loadfile
    python -m coverage json
    python -m backend.scripts.capture_port_baseline --coverage-json coverage.json

Skip flags (`--skip-process`, `--skip-http`, `--skip-hot-paths`,
`--skip-ocr`, `--skip-freeze`) allow partial reruns: a skipped leg
keeps its previous value from the committed JSON, so a partial rerun
never silently blanks a measurement. `--render-only` re-renders the
document from the JSON without measuring anything.
"""

from __future__ import annotations

import argparse
import json
import platform
import statistics
import subprocess
import sys
import tempfile
import time
import timeit
import tomllib
from collections.abc import Callable, Iterable
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT))

from backend.testing.external_process import (  # noqa: E402
    ExternalBackendLeg,
    free_ports,
)
from backend.testing.http_fingerprint import HYDRATION_ENDPOINTS  # noqa: E402

SCENARIO = (
    REPO_ROOT
    / "backend"
    / "tests"
    / "e2e"
    / "corpus"
    / "scripted"
    / "basic_hunt_10_events"
)
OCR_CAPTURES = (
    REPO_ROOT
    / "backend"
    / "tests"
    / "e2e"
    / "corpus"
    / "recorded"
    / "hunt_with_skill_scan"
    / "scan_captures"
)
JSON_OUT = REPO_ROOT / "backend" / "architecture" / "port_baseline.json"
DOC_OUT = REPO_ROOT / "backend" / "architecture" / "PORT-BASELINE.md"

SETTLE_SECONDS = 2.0


# ── helpers ──────────────────────────────────────────────────────────────────


def _stats(samples: list[float]) -> dict[str, float]:
    ordered = sorted(samples)
    p95_index = max(0, min(len(ordered) - 1, round(0.95 * (len(ordered) - 1))))
    return {
        "median": round(statistics.median(ordered), 4),
        "p95": round(ordered[p95_index], 4),
        "min": round(ordered[0], 4),
        "max": round(ordered[-1], 4),
        "samples": len(ordered),
    }


def _vm_rss_mib(pid: int) -> float | None:
    """Resident set size in MiB from /proc; None where /proc is absent."""
    try:
        status = Path(f"/proc/{pid}/status").read_text(encoding="utf-8")
    except OSError:
        return None
    for line in status.splitlines():
        if line.startswith("VmRSS:"):
            return round(int(line.split()[1]) / 1024.0, 1)
    return None


def _cpu_model() -> str:
    try:
        cpuinfo = Path("/proc/cpuinfo").read_text(encoding="utf-8")
        for line in cpuinfo.splitlines():
            if line.lower().startswith("model name"):
                return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return platform.processor() or "unknown"


def _git_commit() -> str:
    try:
        return subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


# ── measurement legs ─────────────────────────────────────────────────────────


def host_leg() -> dict[str, Any]:
    return {
        "captured_at": datetime.now(UTC).strftime("%Y-%m-%d %H:%M UTC"),
        "commit": _git_commit(),
        "platform": platform.platform(),
        "cpu": _cpu_model(),
        "python": platform.python_version(),
    }


def process_leg(boots: int) -> dict[str, Any]:
    """Cold-start, settled idle RSS, and graceful-shutdown time over N boots."""
    cold: list[float] = []
    rss: list[float] = []
    shutdown: list[float] = []
    for _ in range(boots):
        with tempfile.TemporaryDirectory(prefix="baseline-boot-") as work:
            (port,) = free_ports()
            leg = ExternalBackendLeg(SCENARIO, Path(work), port=port)
            started = time.perf_counter()
            leg.start()
            try:
                leg.wait_ready()
                cold.append(time.perf_counter() - started)
                time.sleep(SETTLE_SECONDS)
                sample = _vm_rss_mib(leg.pid)
                if sample is not None:
                    rss.append(sample)
            finally:
                stopping = time.perf_counter()
                leg.shutdown()
                shutdown.append(time.perf_counter() - stopping)
    return {
        "scenario": SCENARIO.name,
        "settle_seconds": SETTLE_SECONDS,
        "cold_start_s": _stats(cold),
        "idle_rss_mib": _stats(rss) if rss else {"unavailable": "no /proc on host"},
        "graceful_shutdown_s": _stats(shutdown),
    }


def http_leg(requests_per_endpoint: int) -> dict[str, Any]:
    """Per-endpoint latency against a freshly replayed scenario state."""
    import httpx

    results: dict[str, dict[str, float]] = {}
    with tempfile.TemporaryDirectory(prefix="baseline-http-") as work:
        (port,) = free_ports()
        leg = ExternalBackendLeg(SCENARIO, Path(work), port=port)
        leg.start()
        try:
            leg.wait_ready()
            summary = leg.replay()
            with httpx.Client(
                base_url=f"http://127.0.0.1:{port}", timeout=30.0
            ) as client:
                endpoints = [("GET_health", "GET", "/api/health")] + [
                    (
                        endpoint_id,
                        method,
                        template.format(session_id=summary["session_id"]),
                    )
                    for endpoint_id, method, template in HYDRATION_ENDPOINTS
                ]
                for endpoint_id, _method, path in endpoints:
                    for _ in range(3):  # warm-up
                        client.get(path)
                    timings: list[float] = []
                    for _ in range(requests_per_endpoint):
                        started = time.perf_counter()
                        response = client.get(path)
                        timings.append((time.perf_counter() - started) * 1000.0)
                        if response.status_code != 200:
                            raise RuntimeError(
                                f"{endpoint_id} returned {response.status_code}"
                            )
                    results[endpoint_id] = _stats(timings)
        finally:
            leg.shutdown()
    return {
        "scenario": SCENARIO.name,
        "requests_per_endpoint": requests_per_endpoint,
        "latency_ms": results,
    }


def hot_paths_leg() -> dict[str, Any]:
    """timeit micro-benchmarks of the named leaf hot paths (best of 5)."""
    from backend.data.tt_value_curve import levels_for_tt_value, tt_value_at
    from backend.services.chatlog_parser import parse_line
    from backend.services.cost_engine import cost_per_shot_from_props

    damage_line = "2026-05-19 10:00:00 [System] [] You inflicted 10.5 points of damage"
    props = {
        "weapon_entity": {
            "name": "Baseline Rifle",
            "economy": {"decay": 2.0, "ammo_burn": 24},
        },
    }
    benches: dict[str, Callable[[], object]] = {
        "cost_engine.cost_per_shot_from_props": lambda: cost_per_shot_from_props(props),
        "chatlog_parser.parse_line (damage line)": lambda: parse_line(damage_line),
        "tt_value_curve.tt_value_at": lambda: tt_value_at(50.5),
        "tt_value_curve.levels_for_tt_value": lambda: levels_for_tt_value(50.0, 10.0),
    }
    number, repeat = 10_000, 5
    per_call_us = {
        name: round(
            min(timeit.repeat(fn, number=number, repeat=repeat)) / number * 1e6, 3
        )
        for name, fn in benches.items()
    }
    return {
        "method": f"timeit, best of {repeat} runs of {number} calls",
        "per_call_us": per_call_us,
    }


def ocr_leg(max_pages: int) -> dict[str, Any]:
    """Engine load, first-page and warm per-page latency, memory delta.

    Runs over the locally seeded recorded skill panels (the corpus is
    local-by-default and never committed); records an honest skip when
    the corpus or the engine is unavailable on this host.
    """
    pages = sorted(OCR_CAPTURES.glob("*-skill.png"))[:max_pages]
    if not pages:
        return {
            "skipped": "recorded OCR corpus not present on this host (local-by-default)"
        }

    import os

    from backend.services import local_ocr
    from backend.services.skill_scan_core import SkillScanCore

    rss_before = _vm_rss_mib(os.getpid())

    started = time.perf_counter()
    engine = local_ocr.get_engine()
    engine_load_s = time.perf_counter() - started
    if engine is None:
        return {"skipped": "local OCR engine unavailable on this host"}

    # The saved capture IS the panel region; feed its bytes through the
    # production per-page path, exactly as the scan service does.
    timings: list[float] = []
    with tempfile.TemporaryDirectory(prefix="baseline-ocr-") as scratch:
        core = SkillScanCore(None, Path(scratch))
        for page in pages:
            page_bytes = page.read_bytes()
            page_started = time.perf_counter()
            levels = core.extract_page_levels(page_bytes)
            timings.append(time.perf_counter() - page_started)
            if not levels:
                raise RuntimeError(f"page yielded no skills: {page}")

    rss_after = _vm_rss_mib(os.getpid())
    return {
        "pages": len(pages),
        "engine_load_s": round(engine_load_s, 3),
        "first_page_s": round(timings[0], 3),
        "warm_page_s": _stats(timings[1:]) if len(timings) > 1 else {},
        "process_rss_delta_mib": (
            round(rss_after - rss_before, 1)
            if rss_before is not None and rss_after is not None
            else None
        ),
    }


def artefacts_leg() -> dict[str, Any]:
    """Freeze the sidecar with PyInstaller on this host and weigh the output.

    The shipped Windows installer and its installed footprint are
    platform artefacts; they are recorded as pending until captured on
    the application's Windows target. The same-host freeze below is the
    comparand a native-backend artefact is weighed against on this host.
    """
    with tempfile.TemporaryDirectory(prefix="baseline-freeze-") as scratch:
        dist = Path(scratch) / "dist"
        started = time.perf_counter()
        subprocess.run(
            [
                sys.executable,
                "-m",
                "PyInstaller",
                str(REPO_ROOT / "backend" / "build_sidecar.spec"),
                "--noconfirm",
                "--distpath",
                str(dist),
                "--workpath",
                str(Path(scratch) / "build"),
            ],
            cwd=REPO_ROOT,
            check=True,
            capture_output=True,
        )
        freeze_s = time.perf_counter() - started
        outputs = {
            str(path.relative_to(dist)): round(path.stat().st_size / (1024 * 1024), 1)
            for path in sorted(dist.rglob("*"))
            if path.is_file()
        }
    return {
        "freeze_host": platform.system(),
        "freeze_s": round(freeze_s, 1),
        "sidecar_outputs_mib": outputs,
        "windows_artefacts": "pending capture on the application's Windows target",
    }


def coverage_table(coverage_json: Path) -> dict[str, Any]:
    """The per-module branch-coverage table from a pinned-order suite run."""
    data = json.loads(coverage_json.read_text(encoding="utf-8"))
    measured: list[dict[str, Any]] = []
    for module, entry in sorted(data["files"].items()):
        summary = entry["summary"]
        if not module.startswith("backend/") or summary["num_statements"] == 0:
            continue
        measured.append(
            {
                "module": module,
                "statements": summary["num_statements"],
                "branches": summary["num_branches"],
                "branch_pct": (
                    round(summary["percent_branches_covered"], 1)
                    if summary["num_branches"]
                    else None
                ),
                "overall_pct": round(summary["percent_covered"], 1),
            }
        )

    pyproject = tomllib.loads(
        (REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8")
    )
    omit: Iterable[str] = pyproject["tool"]["coverage"]["run"]["omit"]
    excluded = [
        entry
        for entry in omit
        if entry.startswith("backend/")
        and not entry.startswith(("backend/tests", "backend/scripts"))
        and entry != "backend/build_app.py"
    ]
    return {
        "coverage_meta": {
            "format": "coverage.py json, branch=true, pinned-order fast+standard run",
        },
        "measured": measured,
        "excluded_device_io": sorted(excluded),
    }


# ── rendering ────────────────────────────────────────────────────────────────


def _render_host(host: dict[str, Any]) -> str:
    rows = [
        ("Captured", host["captured_at"]),
        ("Commit", f"`{host['commit']}`"),
        ("Platform", host["platform"]),
        ("CPU", host["cpu"]),
        ("Python", host["python"]),
    ]
    return "\n".join(["| | |", "| --- | --- |"] + [f"| {k} | {v} |" for k, v in rows])


def _render_stat_table(rows: list[tuple[str, dict[str, Any] | str]], unit: str) -> str:
    lines = [
        "| Measurement | median | p95 | min | max | samples | unit |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    for name, stat in rows:
        if isinstance(stat, str) or "median" not in stat:
            lines.append(f"| {name} | {stat} | | | | | |")
            continue
        lines.append(
            f"| {name} | {stat['median']} | {stat['p95']} | {stat['min']} "
            f"| {stat['max']} | {stat['samples']} | {unit} |"
        )
    return "\n".join(lines)


def _render_process(process: dict[str, Any]) -> str:
    return _render_stat_table(
        [
            ("Cold start to healthy", process["cold_start_s"]),
            (
                f"Idle RSS after {process['settle_seconds']:g}s settle (MiB)",
                process["idle_rss_mib"],
            ),
            ("Graceful shutdown", process["graceful_shutdown_s"]),
        ],
        "s (RSS: MiB)",
    )


def _render_http(http: dict[str, Any]) -> str:
    lines = [
        "| Endpoint | p50 ms | p95 ms | min ms |",
        "| --- | --- | --- | --- |",
    ]
    for endpoint_id, stat in http["latency_ms"].items():
        lines.append(
            f"| `{endpoint_id}` | {stat['median']} | {stat['p95']} | {stat['min']} |"
        )
    lines.append("")
    lines.append(
        f"{http['requests_per_endpoint']} requests per endpoint after 3 warm-ups, "
        f"against a freshly replayed `{http['scenario']}` state."
    )
    return "\n".join(lines)


def _render_hot_paths(hot: dict[str, Any]) -> str:
    lines = ["| Hot path | per-call µs |", "| --- | --- |"]
    for name, value in hot["per_call_us"].items():
        lines.append(f"| `{name}` | {value} |")
    lines.append("")
    lines.append(f"Method: {hot['method']}.")
    return "\n".join(lines)


def _render_ocr(ocr: dict[str, Any]) -> str:
    if "skipped" in ocr:
        return f"Not captured: {ocr['skipped']}."
    lines = [
        "| Measurement | value |",
        "| --- | --- |",
        f"| Pages read | {ocr['pages']} |",
        f"| Engine load (s) | {ocr['engine_load_s']} |",
        f"| First page (s) | {ocr['first_page_s']} |",
    ]
    if ocr["warm_page_s"]:
        warm = ocr["warm_page_s"]
        lines.append(f"| Warm page median (s) | {warm['median']} (p95 {warm['p95']}) |")
    if ocr.get("process_rss_delta_mib") is not None:
        lines.append(f"| Process RSS delta (MiB) | {ocr['process_rss_delta_mib']} |")
    return "\n".join(lines)


def _render_artefacts(artefacts: dict[str, Any]) -> str:
    lines = [
        "| Artefact | size MiB |",
        "| --- | --- |",
    ]
    for name, size in artefacts["sidecar_outputs_mib"].items():
        lines.append(f"| `{name}` ({artefacts['freeze_host']} freeze) | {size} |")
    lines.append(
        f"| Windows installer / installed footprint | {artefacts['windows_artefacts']} |"
    )
    lines.append("")
    lines.append(f"Freeze duration on this host: {artefacts['freeze_s']:g}s.")
    return "\n".join(lines)


def _render_coverage(coverage: dict[str, Any]) -> str:
    lines = [
        "| Module | statements | branches | branch % | overall % |",
        "| --- | --- | --- | --- | --- |",
    ]
    for row in coverage["measured"]:
        branch = row["branch_pct"] if row["branch_pct"] is not None else "no branches"
        lines.append(
            f"| `{row['module']}` | {row['statements']} | {row['branches']} "
            f"| {branch} | {row['overall_pct']} |"
        )
    lines.append("")
    lines.append(
        "Excluded from measurement (device / IO glue that cannot run headless; "
        "see the `omit` list in `pyproject.toml`): "
        + ", ".join(f"`{entry}`" for entry in coverage["excluded_device_io"])
        + ". Their equivalence rests on the recorded OCR / scan corpus and the "
        "input-seam tests rather than a coverage figure."
    )
    return "\n".join(lines)


_RENDERERS: dict[str, Callable[[dict[str, Any]], str]] = {
    "host": _render_host,
    "process": _render_process,
    "http": _render_http,
    "hot-paths": _render_hot_paths,
    "ocr": _render_ocr,
    "artefacts": _render_artefacts,
    "coverage": _render_coverage,
}


def render_document(data: dict[str, Any], doc_path: Path) -> None:
    """Fill every generated block in the document from the captured data."""
    text = doc_path.read_text(encoding="utf-8")
    for block, renderer in _RENDERERS.items():
        begin = f"<!-- BEGIN GENERATED: {block} -->"
        end = f"<!-- END GENERATED: {block} -->"
        if begin not in text or end not in text:
            raise SystemExit(
                f"{doc_path}: missing generated block markers for {block!r}"
            )
        head, rest = text.split(begin, 1)
        _, tail = rest.split(end, 1)
        text = f"{head}{begin}\n{renderer(data[block])}\n{end}{tail}"
    doc_path.write_text(text, encoding="utf-8")


# ── entry point ──────────────────────────────────────────────────────────────


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--coverage-json", type=Path, default=REPO_ROOT / "coverage.json"
    )
    parser.add_argument("--boots", type=int, default=5)
    parser.add_argument("--latency-requests", type=int, default=30)
    parser.add_argument("--ocr-pages", type=int, default=12)
    parser.add_argument("--skip-process", action="store_true")
    parser.add_argument("--skip-http", action="store_true")
    parser.add_argument("--skip-hot-paths", action="store_true")
    parser.add_argument("--skip-ocr", action="store_true")
    parser.add_argument("--skip-freeze", action="store_true")
    parser.add_argument("--render-only", action="store_true")
    args = parser.parse_args(argv)

    data: dict[str, Any] = {}
    if JSON_OUT.exists():
        data = json.loads(JSON_OUT.read_text(encoding="utf-8"))

    if not args.render_only:
        data["host"] = host_leg()
        if not args.skip_process:
            print("[baseline] process leg ...", flush=True)
            data["process"] = process_leg(args.boots)
        if not args.skip_http:
            print("[baseline] http leg ...", flush=True)
            data["http"] = http_leg(args.latency_requests)
        if not args.skip_hot_paths:
            print("[baseline] hot-path leg ...", flush=True)
            data["hot-paths"] = hot_paths_leg()
        if not args.skip_ocr:
            print("[baseline] ocr leg ...", flush=True)
            data["ocr"] = ocr_leg(args.ocr_pages)
        if not args.skip_freeze:
            print("[baseline] freeze leg (PyInstaller, takes minutes) ...", flush=True)
            data["artefacts"] = artefacts_leg()
        if args.coverage_json.exists():
            print("[baseline] coverage table ...", flush=True)
            data["coverage"] = coverage_table(args.coverage_json)
        elif "coverage" not in data:
            raise SystemExit(
                f"coverage JSON not found at {args.coverage_json}; run the "
                "pinned-order coverage suite and `python -m coverage json` first"
            )
        JSON_OUT.write_text(
            json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(f"[baseline] wrote {JSON_OUT.relative_to(REPO_ROOT)}")

    missing = [block for block in _RENDERERS if block not in data]
    if missing:
        raise SystemExit(f"baseline JSON is missing blocks: {', '.join(missing)}")
    render_document(data, DOC_OUT)
    print(f"[baseline] rendered {DOC_OUT.relative_to(REPO_ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
