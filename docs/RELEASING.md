# Releasing

Each release is built by `scripts\package.ps1` and has two downloads
for Windows x64, plus checksums:

| File | What it is |
| --- | --- |
| `hangtime-<version>-x64.msi` | Installer, per machine. Start menu, `PATH`, firewall rule, README. Silent installs work, so Intune can deploy it ([INTUNE.md](INTUNE.md)). |
| `hangtime-<version>-windows-x64.zip` | Portable: exe and docs |
| `SHA256SUMS-<version>.txt` | Checksums for both. |

Both carry `README.html`, the user guide the installer offers when it
finishes. **It is regenerated on every release** from
[QUICKSTART.md](QUICKSTART.md) (how to use and adjust Hangtime) and
[CHANGELOG.md](../CHANGELOG.md) (release notes), so keep both current;
the package script refuses to build a version with no changelog
section. The bundle also has the licenses and `THIRD-PARTY-NOTICES.txt`
(generated from `cargo metadata`; it carries the libopus and RNNoise
BSD notices that static linking requires).

## Tools

- Rust (stable, MSVC) and the Visual Studio C++ build tools, as for any
  build. The icon is compiled in with the Windows SDK's `rc.exe`.
- PowerShell 7 (`pwsh`).
- The .NET runtime (6 or later). The script downloads WiX v5 from
  nuget.org on first use, checks it against pinned SHA-256 sums, and
  keeps it in `%LOCALAPPDATA%\hangtime-build`. No .NET SDK or admin
  rights needed.

## Steps

1. Pick the version (semantic versioning: new features bump the minor
   version, fixes the patch). Set `version` in the root `Cargo.toml`
   (the crates inherit it) and run `cargo build` to update
   `Cargo.lock`.
1. In [CHANGELOG.md](../CHANGELOG.md), write the release notes under
   `## <version> (unreleased)`, in words a user understands. Update
   [QUICKSTART.md](QUICKSTART.md) if using or adjusting Hangtime
   changed.
1. Build from a clean tree:

   ```powershell
   pwsh .\scripts\package.ps1
   ```

   It runs format, clippy, and tests, builds the release exe, checks it
   reports the version, writes the README, notices, MSI (validated with
   Windows Installer's ICE checks), and zip, and prints the checksums.
1. Open `target\package\docs\README.html` and read it through.
1. Install the MSI on a test machine: Start menu, first-start setup,
   Parrot, and the README at the end. Install over the previous
   version to check the upgrade.
1. Change the changelog heading from "unreleased" to today's date,
   rebuild, commit, and tag:

   ```powershell
   git commit -am "Release <version>"
   git tag -a v<version> -m "Hangtime <version>"
   git push origin main v<version>
   ```

1. Publish a GitHub release on the tag with the MSI, zip, and
   `SHA256SUMS` file attached and the changelog entry as the notes.

## Notes

- Never change the MSI's `UpgradeCode` (in `installer\hangtime.wxs`):
  it is how a new version replaces the old one.
- The exe and MSI aren't code-signed, so SmartScreen may warn on a
  downloaded copy. Intune doesn't require signing for MSI apps.
- A wire protocol change bumps the ALPN version and is called out in
  the changelog: old and new versions can't talk to each other.
