# Releasing SolidDraft

## Version rule and first-release gate

SolidDraft uses `0.x.y` versions. Every `0.x` release is labelled **alpha** and marked as a GitHub prerelease. Numbers describe completed work, never dates. Increase the minor number for a completed capability milestone and the patch number for fixes to that milestone. Set `[workspace.package].version` in `Cargo.toml`; packaging reads that value rather than maintaining a second release counter. Keep `Cargo.lock` in sync when changing the workspace version.

**Do not cut `0.1.0` until all work currently in flight has landed and been pushed to `main`.** Preparing or merging the release machinery is not permission to publish the first release. There are no scheduled releases or nightlies.

## What the workflow builds

`.github/workflows/release.yml` uses cargo-packager 0.11.8 for platform packages and Trunk 0.21.14 for the browser build. Archive names include the workspace version.

| Platform | Downloads |
|---|---|
| Windows x64 | `.msi`, portable `.zip` |
| macOS Intel and Apple Silicon | universal `.dmg`, containing both architectures |
| Linux x86_64 | `.AppImage`, `.deb`, portable `.tar.gz` |
| Browser | `.zip`, with relative asset URLs suitable for a GitHub Pages project path |
| Release metadata | `SHA256SUMS`, SPDX JSON SBOM, generated `release-notes.md` |

Windows builds link the C runtime statically. Assembly inspects the portable executable's imports and rejects a dependency on the separately installed Visual C++ runtime.

Linux packages build on Ubuntu 22.04. The portable tar needs the host's graphics drivers and desktop libraries; the AppImage bundles dependencies through cargo-packager. The archive packages include LICENSE and NOTICE. The Debian package and installers also include them as resources.

Winit loads XKB libraries at runtime rather than linking them into the executable. `packaging/Packager.json` explicitly includes those libraries and the XCB XKB dependency in the AppImage, and declares desktop/graphics runtime dependencies for Debian. Keep these declarations aligned with the packaged startup smoke, not just the binary's `ldd` output.

The SBOM comes from Syft through `anchore/sbom-action`. It inventories the checked-out source, locked dependencies and staged packages; it is not a claim that every workspace dependency is linked into every executable. The tag workflow creates GitHub build-provenance attestations for every downloadable file, including the notes, SBOM and checksum manifest. Dry runs do not publish attestations.

Verify a download with `sha256sum -c SHA256SUMS` after downloading the complete set, or check its individual entry. Verify provenance with:

```sh
gh attestation verify soliddraft-0.1.0-linux-x86_64.AppImage --repo BohnBawerick/soliddraft
```

## Dry run, without publishing

On `main`, select **Release packages > Run workflow** in GitHub Actions, or run:

```sh
gh workflow run release.yml --ref main
```

`workflow_dispatch` always builds without creating a release, deploying Pages, signing with repository credentials or publishing attestations. It takes no publish switch. A pull request that changes release configuration also runs the same complete packaging path. This lets a new workflow be tested before GitHub permits manual dispatch from the default branch.

Branch dry runs cancel their superseded runs. Each run on `main` or a version tag has its own concurrency group and cannot cancel or replace another protected run. A manual check against a release tag must not interrupt publishing.

Download these run artifacts:

- `release-dry-run`, the exact seven packages, SBOM, notes and SHA256SUMS.
- `github-pages`, the deploy-ready Pages tar payload. Uploading this artifact does not enable a site.
- `appimage-smoke` and `browser-smoke`, screenshots from the packaged builds.

The Linux smoke starts the AppImage under Xvfb with a software Vulkan adapter, opens the demo and requires the application to save a screenshot and exit. The browser smoke extracts the release zip and serves it under `/soliddraft/` in headless Chromium. It requires a successful WASM response, completion of application startup, a sized canvas and no JavaScript exceptions or application panic messages.

Before release, also download the AppImage and browser zip from the successful run and exercise them on the validation machine. The browser can use a local static HTTP server, not a public self-hosted service. Use `/usr/bin/chromium` for the local headless check. Keep screenshots and logs attached to the pull request or workflow run, not in the repository.

## Release checklist

1. Confirm the owner's first-release gate above. For `0.1.0`, all work in flight must be merged and pushed to `main`.
2. Confirm `main` is green in Hosted CI, including the application scenarios, click budgets and survival checks. Packaging success does not replace these checks. The tag workflow requires a successful Hosted CI push run at the tagged commit.
3. Test the supported desktop part and drawing flows. Open older supported `.dwprt` and `.dwdrw` documents, save and reopen them, and check document identities and references.
   Separately, run the reserved `.dwasm` assembly-format and schema checks. Desktop assembly opening and saving are not supported, so installers must not register an assembly file association. Keep the reserved format and extension unchanged.
4. Land the workspace version and lockfile update on `main`. Run the full release dry run at that commit. Inspect all seven packages, the SBOM, SHA256SUMS, generated notes and both smoke screenshots. After installation, double-click a `.dwprt` part and a `.dwdrw` drawing and confirm that each opens in the application.
5. Review the generated document-format lines. The generator reads `DocKind::format` at the release commit and the nearest reachable previous released tag. It reports each current format and explicitly names a changed previous version. The first release prints all current format versions. Migration details belong in the merged change's pull request and `docs/documents.md`, not a hand-edited changelog.
6. Review signing status. Until credentials exist, macOS and Windows builds are unsigned. Warn users about Gatekeeper and SmartScreen; do not claim that an attestation is an operating-system signing certificate.
7. Only after approval, push the version tag from that green commit. For example, `git tag v0.1.0 <commit>` followed by `git push origin v0.1.0`. The workflow requires the tag to match the workspace version exactly and its commit to be reachable from `main`.
8. Inspect the created GitHub prerelease, asset inventory, checksums and attestations. Do not move an existing release tag. Fix a failed or incorrect published build with a new patch release after completing validation.

Release notes are generated from merged GitHub pull requests whose merge commits fall between the nearest previous released tag and the current commit. The generator lists titles, authors and links, then adds the document-format versions and alpha/file-backup warning. It uses read-only GitHub APIs and includes prereleases when finding the previous release. Do not maintain a hand-edited CHANGELOG.

## Optional signing credentials

Set repository secrets only when the matching certificates and accounts exist. Secrets never belong in configuration files, artifacts or documentation examples. Pull requests and manual dry runs never receive signing credentials. Signing failures on the tag path fail the package job.

| Repository secret | Purpose |
|---|---|
| `WINDOWS_CERTIFICATE` | Base64-encoded PFX code-signing certificate, imported into the ephemeral runner's current-user certificate store |
| `WINDOWS_CERTIFICATE_PASSWORD` | PFX password |
| `APPLE_CERTIFICATE` | Base64-encoded Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Certificate export password |
| `APPLE_SIGNING_IDENTITY` | Full Developer ID Application signing identity |
| `APPLE_ID` | Apple Developer account used by notarytool |
| `APPLE_PASSWORD` | App-specific password for that account |
| `APPLE_TEAM_ID` | Developer team ID |

The Windows package job imports the PFX, passes its thumbprint to cargo-packager and enables SHA-256 signing with RFC 3161 timestamping. cargo-packager signs the executable and MSI before the portable archive and checksums are created. On macOS, setting `APPLE_CERTIFICATE` requires the complete signing and notarization secret set. cargo-packager imports the certificate into a temporary keychain, signs with hardened runtime, notarizes and staples the application before creating the DMG. With no certificate, packaging remains unsigned. Certificate-backed signing cannot be validated until the owner supplies credentials.

## Pages stays off until explicitly approved

No Pages site is enabled by this change. Every successful browser build uploads `github-pages` as a workflow artifact, which is the dry run of the deploy payload.

After the owner approves public browser hosting, configure the repository's Pages source as **GitHub Actions**, protect the `github-pages` environment as appropriate, and set repository variable `SOLIDDRAFT_PAGES_ENABLED=true`. The existing deploy job then deploys only after a successful tag release. Do not set the variable or enable Pages merely to test packaging. Manual dispatch and pull requests cannot deploy, even when the variable is set.

GitHub Pages is static hosting. The modelling kernel runs in the browser; there is no server assistant relay and no bundled API key. A public assistant service is not part of this release machinery.

## Interface work outside this change

This workflow adds no application interface. The design lane owns final package icon artwork, download-page layout, and any in-app update or unsigned-build messaging. The packaging icon is a minimal cube so installers can be exercised now. Those interface choices must not add timing or release instrumentation to modelling or user-edit code paths.
