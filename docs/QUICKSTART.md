# Hangtime

Push-to-talk voice between computers on the same network, like a
two-way radio. One person talks at a time on each talkgroup.

## Install

1. Unzip this folder anywhere.
1. Double-click `install.cmd`. (Right-click it and choose "Run as
   administrator" to also set up the Windows Firewall rule; otherwise
   Windows asks on first run, and you should allow **Private
   networks**.)
1. Start **Hangtime** from the Start menu, or run `hangtime` in a new
   terminal.

Nothing else is needed: no runtime, no server, no account.

## First run

Hangtime opens full screen in the terminal:

- **Hold SPACE** to talk. Let go to listen.
- **TAB** or a **number key** picks the talkgroup you talk and listen
  on. The bottom one, **Parrot**, plays you back: pick it and say
  something to check your microphone and speaker.
- **V** turns on VOX (talking keys the radio, no SPACE needed). **+**
  and **-** make it more or less sensitive. Use a headset with VOX.
- **M** mutes the selected talkgroup. You still see who is talking.
- **Q** quits.

Other Hangtime users on the same network appear under Peers on their
own. Everyone hears **All Call**.

## Settings

Settings are in `%USERPROFILE%\.config\hangtime\hangtime.toml`, written
with comments and example talkgroups on first run. Edit it, then
restart Hangtime. Things you can set there:

- your name, microphone, and speaker
- VOX, and muted groups
- your talkgroups

People share a talkgroup when they use the same `id` in their
settings. Give a group a `key` (a shared passphrase) to keep others
out of it.

Useful commands:

```text
hangtime devices          list microphones and speakers
hangtime mic-test         record 10 s and suggest a VOX sensitivity
hangtime --peer HOST:4747 connect to someone the LAN can't find
hangtime --version
```

## Network

Hangtime uses UDP port 4747 and finds others with mDNS (UDP 5353), so
it works within one network or VLAN. Across subnets or VPNs, use
`--peer` with the other computer's address.

## Uninstall

Run `install.cmd -Uninstall` from this folder. Your settings and
identity stay in `%USERPROFILE%\.config\hangtime`; delete that folder
to remove them too.
