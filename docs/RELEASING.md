# Releasing

Releases are a zip for Windows x64 holding `hangtime.exe` (static C
runtime, no installer needed to run), `install.cmd` and `install.ps1`,
a user README (from [QUICKSTART.md](QUICKSTART.md)), and the changelog.

## Steps

1. Pick the version. Bump `version` in the root `Cargo.toml` (the
   crates inherit it) and run `cargo build` to update `Cargo.lock`.
1. In [CHANGELOG.md](../CHANGELOG.md), change the top heading from
   "unreleased" to today's date and check the list is complete.
1. Build the package from a clean tree on `main`:

   ```powershell
   .\scripts\package.ps1
   ```

   It runs format, clippy, and tests, builds the release exe, checks
   the exe reports the version, and writes
   `dist\hangtime-<version>-windows-x64.zip` and a `.sha256` file.
1. Smoke test on a second machine: unzip, run `install.cmd`, start
   Hangtime, talk on Parrot, then talk to the build machine.
1. Commit, then tag and push:

   ```powershell
   git commit -am "Release <version>"
   git tag -a v<version> -m "Hangtime <version>"
   git push origin main v<version>
   ```

1. Publish the zip and its `.sha256` (for example as a GitHub release
   attached to the tag) with the changelog entry as the notes.

## Notes

- The exe isn't code-signed, so SmartScreen may warn on first run from
  a download. The installer unblocks the copy it installs.
- A wire protocol change bumps the ALPN version and is called out in
  the changelog: old and new versions can't talk to each other.
