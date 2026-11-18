# Security Policy

## Supported Versions

EntropiaOrme is a personal project published as source. Security fixes land in the next source snapshot on `main`; the installers on the [Releases page](https://github.com/entropiaorme/entropiaorme/releases) are historical and are not maintained.

## Reporting a Vulnerability

Report security issues privately to **MikelWL@protonmail.com**.

Please include:
- A description of the vulnerability and its impact.
- Steps to reproduce or proof-of-concept code.
- The commit of EntropiaOrme you built and tested against.

I aim to acknowledge reports within 7 days and publish a fix or mitigation within 30 days for confirmed issues. Once a fix is published I will publicly credit reporters who consent.

Please do not file public GitHub issues for security reports.

## Supply chain security

The shipped application is a single native-Rust binary; it bundles no interpreter or scripting runtime. Its dependencies and bundled build inputs are held under automated review:

- **Rust dependencies** are pinned by the committed `Cargo.lock`. The continuous-integration policy job runs `cargo audit -D warnings` against the RustSec advisory database and `cargo deny check` against the policy in `app/src-tauri/deny.toml` (advisories, licences, and source allow-lists) on every change, so an advisory or a disallowed licence fails the build. A small number of advisories are suppressed with a documented, per-advisory rationale in `app/src-tauri/.cargo/audit.toml`: each is an unmaintained or unsound warning, not a vulnerability, and all but one sit on transitive crates. The exception is `chrono`, a direct dependency declared unmaintained with no patched release; it stays until the date handling is migrated to a maintained crate. Should a vulnerability-class suppression ever become necessary, the standing policy is that it must be proven unreachable in the shipped artefacts, pinned structurally (for example a `cargo deny` wrapper ban restricting the flagged crate to its sole legitimate parent), and re-checked by a scheduled job that fails the moment an upstream fix becomes available, so no suppression outlives its justification.
- **Frontend dependencies** are pinned by the committed `package-lock.json` and installed with `npm ci`, which refuses to deviate from the lockfile.
- **Bundled binary assets** are recorded in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md): the optical-character-recognition model and its character dictionary with their SHA-256 hashes, so a shipped model can be verified against the published notice, and the ONNX Runtime libraries with their upstream source and licence.

Every GitHub Action in the continuous-integration workflow is pinned to a commit SHA, so the build steps themselves are fixed inputs. No release artefacts are produced: the tag-driven release pipeline, its bill of materials, checksums and provenance attestations, and installer signing were retired with installer distribution ([ADR-0040](docs/src/adr/0040-periodic-source-snapshots.md)); [ADR-0014](docs/src/adr/0014-release-engineering.md) records how they worked.
