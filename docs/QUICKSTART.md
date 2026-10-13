# Hangtime

Push-to-talk voice between computers on the same network, like a
two-way radio. One person talks at a time on each talkgroup.

## Install

- **Installer:** run `hangtime-<version>-x64.msi`. It installs Hangtime
  for everyone on the computer, adds it to the Start menu and to `PATH`,
  and lets it through Windows Firewall on private and work networks.
- **Managed computers:** your IT team can push the same MSI with
  Intune or any tool that installs MSIs silently.
- **Without installing:** unzip `hangtime-<version>-windows-x64.zip`
  anywhere and run `hangtime.exe`. Windows asks about the firewall the
  first time; allow **Private networks**.

## First start

Start **Hangtime** from the Start menu (or run `hangtime` in a
terminal). The first time, it asks two things:

1. **Your name**, as others see it. It suggests your first name from
   Windows; press Enter to keep it.
1. **Your headset.** Connect it when asked (plug it in, or turn it on
   so it pairs), then press Enter. Hangtime picks the microphone and
   speaker that just appeared. If nothing new shows up, pick from the
   list, or press Enter for the Windows default.

It then saves your settings and opens the radio. To answer the
questions again later, run `hangtime setup`.

## Using it

- **Hold SPACE** to talk. Let go to listen.
- **TAB** or a **number key** picks the talkgroup you talk and listen
  on. Everyone hears **All Call** (0).
- The last talkgroup, **Parrot**, plays you back: pick it, say
  something, and let go to check your microphone and speaker.
- **V** turns on VOX: talking keys the radio, no SPACE needed. **+**
  and **-** make it more or less sensitive. Use a headset with VOX.
- **M** mutes the selected talkgroup. You still see who is talking.
- **Q** quits.

Other people running Hangtime on the same network appear under Peers
by themselves.

## Adjusting Hangtime

Your settings are in `%USERPROFILE%\.config\hangtime\hangtime.toml`.
Open it in Notepad, change it, and restart Hangtime. Every setting has
a comment explaining it.

| Setting | What it does | Default |
| --- | --- | --- |
| `name` | Name others see | Your first name |
| `microphone`, `speaker` | Device, by name or part of it | From setup |
| `vox` | Start with VOX on | `false` |
| `vox_sensitivity` | 1 (least) to 5 (most) | `3` |
| `muted` | Talkgroups not to play, e.g. `["All Call"]` | none |

Run `hangtime devices` to see microphone and speaker names, and
`hangtime mic-test` to find the right VOX sensitivity for your
microphone and room.

### Talkgroups

Talkgroups are the `[[group]]` entries at the end of the file. You
start with:

| # | Talkgroup | Who can join |
| --- | --- | --- |
| 0 | All Call | Everyone (built in) |
| 1 | Group A | Anyone (open) |
| 2 | Group B | Anyone (open) |
| 3 | Group C | People with its key |
| last | Parrot | Only you: plays you back (built in) |

People share a talkgroup when they use the same `id`. Add a `key` (a
shared passphrase) to keep others out. **Group C's key,
`testing-only-change-me`, is the same for everyone**: agree a new one
with your group, give it out privately, and put it in each person's
file. To add a talkgroup, copy a `[[group]]` block and give it a new
name and `id`.

## Commands

```text
hangtime                  start the radio
hangtime setup            choose your name and headset again
hangtime devices          list microphones and speakers
hangtime mic-test         record 10 s and suggest a VOX sensitivity
hangtime --peer HOST:4747 also connect to someone the LAN can't find
hangtime --version
```

## Network

Hangtime uses UDP port 4747 and finds others with mDNS (UDP 5353), so
it works within one network or VLAN. Across subnets or VPNs, start it
with `--peer` and the other computer's address.

## Uninstall

Remove **Hangtime** in Settings → Apps. Your settings and identity
stay in `%USERPROFILE%\.config\hangtime`; delete that folder to remove
them too.
