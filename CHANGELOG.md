# Changelog

All notable changes to Hangtime. Versions follow
[semantic versioning](https://semver.org); the wire protocol has its
own version (ALPN `hangtime/2`, see `docs/PROTOCOL.md`).

## 0.1.0 (unreleased)

First release, for Windows x64 on a shared network.

- Push-to-talk voice between computers over QUIC: Opus in datagrams,
  jitter buffer with FEC and loss concealment, and a latency estimate
  after each received transmission.
- Finds peers on the LAN with mDNS; `--peer HOST:PORT` for anyone it
  can't find. Dropped connections redial on their own.
- Talkgroups with floor control: one talker at a time, talk-permit and
  busy tones, 1.5 s hang time, 120 s time-out. All Call reaches
  everyone. Keyed groups need a shared passphrase to join.
- Scan list: every group shows who is talking and a level bar; you
  hear the selected group and All Call. M mutes a group.
- Built-in Parrot talkgroup plays you back, to check your own audio.
- VOX with noise suppression and five sensitivity levels;
  `hangtime mic-test` suggests one for your mic and room.
- Pick microphone and speaker by name, with fallback to the default.
- Full-screen terminal UI.
- Settings and identity in `~/.config/hangtime`, moved there from
  `%APPDATA%\hangtime` automatically.
- Installer (`install.cmd`) for the current user: PATH, Start menu,
  optional firewall rule.
