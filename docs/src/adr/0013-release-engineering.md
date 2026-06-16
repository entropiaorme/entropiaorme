# ADR-0013: Bespoke installer, signed auto-update, and a provenance-bearing release pipeline

- Status: Accepted
- Context: the decision is locked; the client-side updater plumbing, the tag-to-release pipeline, and the repo-wide action pinning have landed, while the bespoke installer chrome and code-signing activation are in progress

## Context and problem statement

EntropiaOrme needed an industry-grade way to ship. The release was a manual local build that produced an unsigned NSIS installer and a portable ZIP, with no automatic updates and no supply-chain artefacts. Three gaps followed from that. The installer was the stock `tauri-bundler` NSIS chrome, which does not read as a curated product. Updates were notify-only: a user downloaded and ran each new installer by hand. And the distribution layer carried no software bill of materials, no build-provenance attestation, and no commit-pinned CI actions, so nothing attested how a release was built.

Two constraints shaped the response. The application is Windows-only because Entropia Universe is, so the effort leans into Windows-native distribution rather than a portability story. And Authenticode code signing is gated on an external certificate that no code change can advance, so every artefact has to remain installable through the unsigned window (an NSIS or WiX installer runs after a SmartScreen click-through; an MSIX, by contrast, is effectively uninstallable unsigned).

## Decision

The distribution layer graduates to a maximalist, signing-ready shape in three parts.

**Installer.** A hand-authored WiX Burn bootstrapper is the centrepiece: a branded, themed install experience rather than stock chrome. The portable ZIP stays for the no-install audience. MSIX is added as a secondary, modern track (clean uninstall, OS-channel updates) that becomes the primary installer once a certificate exists; NSIS is retired once the Burn installer supersedes it. The curated experience deliberately rides WiX Burn rather than MSIX, because MSIX chrome is standardised (not bespoke) and is uninstallable while unsigned, whereas the Burn vehicle is both the bespoke one and the unsigned-installable one. Product onboarding stays in the application's first-run experience, not the installer; conflating the two is the stock-installer mistake.

**Updates.** The Tauri updater plugin checks a per-channel signed manifest (stable and beta), served from `entropiaorme.com` (already allowed by the application content-security policy). Integrity rests on the manifest signature, verified against a public key embedded in the bundle, and on the updater's newer-only version rule, which refuses a replayed older manifest (downgrade defence). The update-signing key is independent of the Authenticode certificate, so the signed-manifest path is wired now and does not wait on the certificate. MSIX updates flow through the operating system's own channel and are not driven by this updater.

**Release pipeline.** A tag-driven workflow builds the artefacts, generates a CycloneDX SBOM for the shipped shell, computes per-asset checksums, records a SLSA-style build-provenance attestation, and drafts a pre-release for a human to publish. Every GitHub Action across the workflows is pinned to a commit SHA with a version comment and kept current by Dependabot. The release workflow is standalone and tag-only, so it never gates pull requests.

## Consequences

This change lands the client and infrastructure halves and leaves the certificate-dependent and taste-dependent halves for a focused follow-up. Landed: the updater plugins, channel resolution, commands, capability grants, and configuration; the tag-to-release pipeline with its SBOM, checksums, and provenance; the repo-wide action SHA-pinning with Dependabot; and a lock-step version-bump helper for the three version stamps the parity guard governs. In progress: the WiX Burn installer and the MSIX target, the production update-signing key (provisioned out of band as a CI secret) and the Authenticode certificate, the WinGet and Scoop manifests, and the first live signed release. Until those land the pipeline produces unsigned installers, installable with the SmartScreen click-through the marketing site already documents, and the updater is wired but not yet emitting signed artefacts.

This record supersedes the earlier conservative posture (a single NSIS target, notify-only updates, shipping unsigned for the 0.x window). That posture was the right starting point; this is the graduated, opt-in target shape, taken on deliberately rather than by drift. See [ADR-0006](0006-tauri-svelte-frontend.md) for the Tauri shell this plugs into, [ADR-0001](0001-strangler-fig-port.md) for the native backend the shell now hosts, the [service and crate map](../architecture/service-map.md), and the [ADR index](index.md).

## Evidence

- `.github/workflows/release.yml`
- `.github/workflows/ci.yml`, `.github/workflows/nightly.yml`, `.github/dependabot.yml`
- `frontend/src-tauri/entropia-orme/src/updater.rs`
- `frontend/src-tauri/entropia-orme/src/lib.rs`
- `frontend/src-tauri/entropia-orme/tauri.conf.json`
- `frontend/src-tauri/entropia-orme/capabilities/default.json`
- `backend/scripts/bump_version.py`
