# Deploying with Intune

The Hangtime MSI installs per machine and needs no input, so it can be
deployed as a Windows line-of-business (LOB) app.

## Add the app

1. In the Intune admin center, go to **Apps → Windows → Add**, and pick
   **Line-of-business app**.
1. Upload `hangtime-<version>-x64.msi`. Intune reads the name,
   publisher, version, and product code from it.
1. **App install context: Device.** Hangtime installs to
   `C:\Program Files\Hangtime` for everyone on the machine.
1. Leave **Ignore app version** off, so a newer MSI upgrades the old
   one.
1. Assign it to a device or user group as **Required** (pushed) or
   **Available** (in Company Portal).

What the install does: copies the exe and README, adds Start menu
shortcuts and `PATH`, and adds inbound UDP firewall rules for
Hangtime on the **Private** and **Domain** profiles. It doesn't touch
anyone's settings.

For a Win32 app (`.intunewin`) instead, wrap the MSI with the Microsoft
Win32 Content Prep Tool and use:

```text
Install:    msiexec /i hangtime-<version>-x64.msi /qn
Uninstall:  msiexec /x {product code} /qn
Detection:  MSI product code (Intune fills it in from the MSI)
```

## First start for each user

The installer runs as SYSTEM, so it can't ask anyone for their name or
headset. Instead, the first time each person starts Hangtime, it
suggests their first name from their Windows account, asks them to
connect their headset, and writes
`%USERPROFILE%\.config\hangtime\hangtime.toml` with the default
talkgroups (Group A, B, and C between All Call and Parrot).

## Upgrades and removal

Upload the new MSI to the same app (or a new app that supersedes it).
Windows Installer replaces the old version in place; users' settings
and identities are kept. Uninstalling removes the program, shortcuts,
`PATH` entry, and firewall rules, and also keeps settings.

## Network

Peers find each other with mDNS (UDP 5353) and talk on UDP 4747 within
one network or VLAN. Make sure your network policy allows both between
the machines that should talk. Across subnets or VPNs, users start
Hangtime with `--peer <address>:4747`.
