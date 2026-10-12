# 0015: Config in ~/.config/hangtime; release as a zip with an installer

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

ADR-0008 put the identity (and later `hangtime.toml`) in the platform
config folder, `%APPDATA%\hangtime` on Windows. The user prefers the
same `~/.config/hangtime` path on every platform: easy to find, and the
same in instructions for everyone. Hangtime is also ready to hand to
users on the same network, which needs a release build they can
install without Rust.

## Options considered

- **Keep the platform folder:** Windows-conventional, but hidden in
  `AppData\Roaming` and different per platform.
- **`~/.config/hangtime` everywhere** (or `$XDG_CONFIG_HOME/hangtime`):
  one path in all docs; common for command-line tools on Windows too.
- **MSI or MSIX installer:** Start menu, uninstall entry, and upgrades
  handled by Windows, but needs WiX or signing tooling.
- **Zip with a PowerShell installer:** the exe already runs standalone
  (static C runtime); a script adds PATH, Start menu, and a firewall
  rule for the current user with no admin rights.

## Decision

Identity, settings, and the mic-test recording live in
`$XDG_CONFIG_HOME/hangtime` if set, otherwise `~/.config/hangtime`, on
every platform. On start, files found in the old platform folder are
moved across (never overwriting), so an install keeps its radio ID.

The starter `hangtime.toml` lists the whole group layout in comments
(All Call at 0, the configured groups, Parrot last) and sets up three
open example groups, plus a commented keyed example.

Releases are `hangtime-<version>-windows-x64.zip`, built by
`scripts/package.ps1`, holding the exe, `install.cmd`/`install.ps1`, a
user README, and the changelog. The installer is per user
(`%LOCALAPPDATA%\Programs\Hangtime`); run as administrator it also adds
an inbound UDP firewall rule for private and domain networks.

## Consequences

- Upgrading is "run the new zip's installer"; settings stay put.
- No uninstall entry in Windows Settings; `install.cmd -Uninstall`
  removes it.
- The exe isn't signed, so SmartScreen may warn on a downloaded zip.
- Linux and macOS packages are not built yet.
