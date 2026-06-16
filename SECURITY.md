# Security Policy

## Supported Versions

EntropiaOrme is in active pre-1.0 development. Security fixes target the latest released version on the [Releases page](https://github.com/entropiaorme/entropiaorme/releases). Older versions are not supported.

## Reporting a Vulnerability

Report security issues privately to **MikelWL@protonmail.com**.

Please include:
- A description of the vulnerability and its impact.
- Steps to reproduce or proof-of-concept code.
- The version of EntropiaOrme you tested against (Settings, About panel).

I aim to acknowledge reports within 7 days and ship a fix or mitigation within 30 days for confirmed issues. Once a fix is released I will publicly credit reporters who consent.

Please do not file public GitHub issues for security reports.

## Supply chain security

Dependencies and bundled build inputs are held under automated review on every change:

- **Python dependencies** are version-constrained in `backend/requirements.txt` and `backend/requirements-dev.txt`, with known-vulnerable releases excluded explicitly. The continuous-integration dependency-audit job runs `pip-audit --strict` against both files, so a newly disclosed advisory fails the build.
- **Rust dependencies** are pinned by the committed `Cargo.lock`. The Rust policy job runs `cargo audit -D warnings` against the RustSec advisory database and `cargo deny check` against the policy in `frontend/src-tauri/deny.toml` (advisories, licences, and source allow-lists), so an advisory or a disallowed licence fails the build.
- **Frontend dependencies** are pinned by the committed `package-lock.json` and installed with `npm ci`, which refuses to deviate from the lockfile.
- **Bundled binary assets** (the optical-character-recognition model and its character dictionary) are recorded in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) with their SHA-256 hashes, so a shipped asset can be verified against the published notice.

Release artefacts do not yet carry a signed software bill of materials or build-provenance attestation. Both are intended additions, and this section will be extended to describe them once the release pipeline publishes them.
