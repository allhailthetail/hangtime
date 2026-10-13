# 0016: MSI installer, and setup in the app on first start

- **Status:** Accepted
- **Date:** 2026-10-13
- **Supersedes:** the zip-and-script installer in
  [0015](0015-config-folder-and-release-zip.md); 0015's config folder
  still stands.

## Context

Hangtime is going to more people, some on machines managed with
Intune. The user wants a real Windows installer, setup that fills in
each person's name, microphone, and speaker, default talkgroups, the
app icon, and a README with release notes at the end of the install.

An installer deployed by Intune runs silently as SYSTEM. It can't ask
anyone a question, and it can't write into a user's profile, where
`hangtime.toml` lives.

## Options considered

- **MSI with setup dialogs and custom actions:** asks for name and
  headset during install. Only works for interactive installs, writes
  settings for whoever ran the installer, and needs custom-action code
  in the MSI.
- **Per-machine MSI, setup in the app on first start:** the MSI only
  installs files; the first time each user starts Hangtime, the app
  asks for their name and headset and writes their settings. Works the
  same for a double-clicked MSI, Intune, or the portable zip.
- **MSIX:** modern packaging, but needs signing for every install and
  doesn't suit a console app that opens firewall ports.

## Decision

A per-machine MSI built with WiX v5 (`installer/hangtime.wxs`,
`scripts/package.ps1`): Program Files, Start menu shortcuts for
Hangtime and its README, system `PATH`, and inbound UDP firewall rules
for the Private and Domain profiles. It has a fixed UpgradeCode and
major upgrades, so new versions replace old ones. The interactive UI
shows the license and ends with a checked "Open the README" box.

The app runs setup (`hangtime setup`, and automatically when there is
no settings file): it suggests the first name from the Windows
account's display name (falling back to the login name), has the user
connect their headset and picks the microphone and speaker that
appeared (or offers a list), and writes `hangtime.toml` with `vox =
false`, `vox_sensitivity = 3`, and Group A and B (open) and Group C
(keyed with the placeholder `testing-only-change-me`) between All Call
and Parrot. Without a terminal it writes the defaults without asking.

The exe embeds `icons/windows/hangtime.ico` and version details. The
README is HTML generated on every release from `docs/QUICKSTART.md`
and `CHANGELOG.md`.

WiX v5 is used rather than v6, whose license adds a maintenance fee
for commercial users. It runs from its NuGet package on the .NET
runtime, downloaded with pinned checksums, so building needs no .NET
SDK or admin rights.

## Consequences

- Installing needs admin rights (per machine); the portable zip covers
  anyone without them.
- Everyone gets the same Group C key until they change it; the
  settings file and README say so.
- No code signing yet: SmartScreen may warn on downloaded files.
- Admins can't yet preset talkgroups or keys for a whole organisation;
  a machine-wide default settings file would be the next step.
