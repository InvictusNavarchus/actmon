# System service deployment

These are reviewable service setup steps, not a service installer. Run them only
when ready to deploy. They change system files, device permissions, and running
services.
The units target Linux with systemd; offline validation used Fedora's systemd
259. Offline checks validate packaging; they do not prove runtime access or useful
collection. Follow the deployed-service validation after installation or updates.

## Execution and installation contract

| Concern | Behavior |
| --- | --- |
| Availability | Boot-managed collection before login and after logout, once storage and input sources are ready |
| Manager and identity | System manager; existing unprivileged `invictus` account |
| Access | `actmon-input` assigned to the collector service only; read access to selected input devices; no input injection |
| Installation authority | Build as the invoking user; elevate only system file/account/device configuration |
| First activation | Install files first, then explicitly enable/start the collector and backup timer |
| Updates | Binary replacement does not restart collection or alter enablement; explicitly restart an active collector when ready |
| Customization | Preserve the environment file, device rules, drop-ins, timer opt-outs, and existing history |
| Removal | Explicitly stop/disable owned units; retain recorded data and shared accounts/groups |

A lingering user service could provide boot/logout availability, but would need a
new device-access arrangement. The system service supplies the collector's group
without adding it to the login account globally. This deployment does not use
lingering. The backup job receives no input group; its system scope keeps snapshot
management with this deployment, rather than satisfying a device requirement.

## Account, paths, and device access

Use the existing `invictus` account via the system manager, not `systemctl --user`.
`actmon.service` starts at boot independently of KDE, login, or lingering.
These are plain units for this single-account deployment; no instance suffix is needed.
The account choice keeps the owner-only database and snapshots readable by your
existing Homekeep job without ACL export machinery. It is not a security boundary
against other processes running as your own user.

| Item | Default |
| --- | --- |
| Binary | `/usr/local/bin/actmon` |
| Configuration | `/etc/actmon/invictus.env` |
| Live database | `/home/invictus/.local/share/actmon/activity.db` |
| Published snapshots (Homekeep source) | `/home/invictus/.local/share/actmon/backups/` |
| Private backup state/staging | `/home/invictus/.local/share/actmon/.backups.actmon-backup/` |
| Collector | `actmon.service` |
| Daily timer | `actmon-backup.timer` |
| Backup job | `actmon-backup.service` |

Installation creates the data directory as its user with mode 0700; both jobs
use umask 0077. The units target this laptop's `invictus` account and explicitly use
`/home/invictus`, waiting for its backing mount at boot. They do not discover
alternate home paths or inherit shell XDG settings. `%h` cannot be used here: in the system manager it
resolves to `/root` regardless of `User=`. See path overrides below for other homes.
`ProtectHome=tmpfs` hides `/home`, `/root`, and `/run/user`; `BindPaths=` exposes
only the Actmon directory within those trees as writable inside each service.

For Homekeep, select `.local/share/actmon/backups` only, after the first successful
snapshot creates it. Do not select the Actmon parent. If a broader selection already
includes it, exclude `/.local/share/actmon/activity.db*` and
`/.local/share/actmon/.backups.actmon-backup/**` so live SQLite files and private
staging are not uploaded. Homekeep configuration remains a separate operation.

The collector receives `actmon-input` through `SupplementaryGroups=`. The user
is not added to that group globally, and the backup service does not receive it.
The example udev rules assign group `actmon-input` and mode 0640 only to event
nodes named `keyboard-debouncer` and `USB Optical Mouse ` (trailing space).
Rules follow device recreation rather than hard-coding event numbers. The grant
covers every node with a matching name, not a uniquely identified physical unit.
If duplicate names appear, all matching nodes receive group access but the
collector refuses ambiguous selection. Neither of the current devices provides a
unique serial identifier. Port matching could narrow the physical mouse grant,
but would bind access to that port and would not resolve the collector's name
ambiguity. Richer identity selection needs coordinated collector and rule support.
These examples do change matching nodes' existing group permissions: other tools relying on membership
in `input` to read these specific nodes may need their own access arrangement.
The current physical keyboard names and `/dev/uinput` do not match these rules;
a device deliberately using a configured name would match. The existing debouncer
service and its input/uinput permissions are left unchanged.

The collector still refuses ambiguous names and never grabs a device. No hard
dependency on the debouncer service is needed: missing sources are recorded as
unavailable and rediscovered every two seconds. The service's device policy permits
read access to the input character-device class; filesystem permissions still
apply. The backup job has `PrivateDevices=yes` and no need for real input nodes.

## Prepare and install

From the repository root, build and inspect the exact device names:

```sh
cargo build --locked --release
./target/release/actmon devices
bash scripts/verify-packaging.sh
```

The verification script uses a disposable filesystem root. It validates the
packaged calendar expression and checks that enabling the collector also enables
the backup timer, while a deliberately masked timer stays masked on later enables.
It does not install or start host services.

Review `packaging/actmon.env.example` and
`packaging/udev/99-actmon-input.rules.example` together. Both must select the same
keyboard and mouse. Preserve the mouse name's trailing space inside quotes.
Udev names are match patterns; escape pattern characters if using different devices.
Use a local copy for host-specific edits instead of changing tracked examples.
The environment file follows systemd syntax, not shell syntax; no `export`, tilde,
or shell substitutions. `${VARIABLE}` in ExecStart preserves each expanded value
as one argument, including whitespace in device names and paths.

For a first installation using the reviewed example names:

```sh
sudo install -d -o invictus -g invictus -m 0700 /home/invictus/.local/share/actmon
bash install.sh
sudo install -d -m 0755 /etc/actmon /etc/sysusers.d /etc/udev/rules.d /usr/local/lib/systemd/system
sudo install -m 0644 packaging/sysusers/actmon.conf /etc/sysusers.d/actmon.conf
sudo systemd-sysusers /etc/sysusers.d/actmon.conf
sudo install -m 0644 packaging/systemd/actmon.service /usr/local/lib/systemd/system/actmon.service
sudo install -m 0644 packaging/systemd/actmon-backup.service /usr/local/lib/systemd/system/actmon-backup.service
sudo install -m 0644 packaging/systemd/actmon-backup.timer /usr/local/lib/systemd/system/actmon-backup.timer
sudo install -m 0644 packaging/actmon.env.example /etc/actmon/invictus.env
sudo install -m 0644 packaging/udev/99-actmon-input.rules.example /etc/udev/rules.d/99-actmon-input.rules
sudo udevadm verify /etc/udev/rules.d/99-actmon-input.rules
sudo systemctl daemon-reload
sudo udevadm control --reload-rules
```

Do not overwrite an existing configuration or rule file blindly during upgrades.
On SELinux systems, use the distribution's normal labels for installed binaries,
units, and state. Investigate actual denials if encountered; do not disable SELinux.

Packaged units live in `/usr/local/lib/systemd/system`; put local overrides in
`/etc/systemd/system/<unit>.d/`. Keeping the packaged timer outside `/etc` allows
`systemctl mask` to create its `/etc/systemd/system/actmon-backup.timer` symlink
without colliding with the unit file.

Reloading rules affects future device events. Apply them to **only the two selected
current nodes** after confirming their names (event numbers can change). For
example, if discovery still reports event22 and event5:

```sh
cat /sys/class/input/event22/device/name /sys/class/input/event5/device/name
sudo udevadm trigger --action=change /sys/class/input/event22 /sys/class/input/event5
sudo udevadm settle
getfacl /dev/input/event22 /dev/input/event5
```

Confirm `root:actmon-input` and group read access. Do not broadly trigger all input
devices. If temporary per-user ACLs from the earlier smoke test remain, remove only
those known test grants so validation exercises the service group:

```sh
sudo setfacl -x u:invictus /dev/input/event22 /dev/input/event5
```

Skip removal if that explicit entry is absent. Do not remove unrelated desktop ACLs.

Enable the collector and daily backups together:

```sh
sudo systemctl enable --now actmon.service actmon-backup.timer
systemctl status actmon.service actmon-backup.timer
journalctl -u actmon.service -n 50 --no-pager
```

`Also=` makes enabling the collector include the timer; the explicit command above
also starts both immediately. Daily backup scheduling is enabled by default in
this installation flow. To opt out, including across later collector enables:

```sh
sudo systemctl mask --now actmon-backup.timer
```

To opt back in, unmask it and run `systemctl enable --now` for the timer.
Masking the timer does not prevent an explicitly requested manual backup job.

## Validate the deployed services

After several seconds of normal input:

```sh
actmon status --database /home/invictus/.local/share/actmon/activity.db
systemctl show actmon.service -p User -p SupplementaryGroups -p MainPID -p NRestarts
sudo systemctl start actmon-backup.service
actmon backup-status --destination /home/invictus/.local/share/actmon/backups
systemctl list-timers actmon-backup.timer
journalctl -u actmon-backup.service -n 50 --no-pager
```

Confirm available coverage for all supported sources, increasing timestamps, a
verified local snapshot, and a scheduled next timer run. A running collector with
unavailable devices is not proof of healthy input collection. The oneshot job
normally becomes inactive after success; use its exit result and backup status.
Follow the [restore rehearsal](backups.md#restore-rehearsal) on a separate copy of
the published artifact.

The timer runs around local midnight, with up to 15 minutes randomized delay.
`Persistent=yes` provides one catch-up activation when a scheduled run was missed
while the timer was inactive; it does not reconstruct one snapshot for every
missed day. It does not wake a suspended laptop. An already-running backup unit
is not started again, and the CLI also locks its destination. There is no automatic
retry after a failed job; inspect the journal and retry manually, or await the next
daily run. A catch-up attempt on first installation can race initial database
creation and fail; the explicit first backup above verifies the setup afterward.
Backup failure does not stop or restart the collector.

The collector restarts after failure with a five-second delay, capped at five
starts per minute. After fixing repeated failures, use `systemctl reset-failed`
and start it again. Normal `systemctl stop` does not trigger a restart. SIGTERM
allows a clean flush; systemd may kill a collector that cannot stop within 30 seconds.
The backup command has a 300-second cooperative deadline and the unit has a
10-minute startup ceiling plus 30 seconds for termination. A supervisor kill can
leave an unfinished attempt/staging; it does not mean a partial artifact was published.
All operational output goes to the system journal; no parallel log files are added.

During a later coordinated check, unplug/reconnect the mouse, recreate the virtual
keyboard through the debouncer's normal lifecycle, and suspend/resume. Check both
journal events and recorded coverage. Those are disruptive actions and are not
part of offline packaging validation. Leave a longer recording running afterward
to measure resource cost before considering durability or cadence changes.

## Change paths or schedule

Edit `/etc/actmon/invictus.env` to set `ACTMON_DATABASE` or
`ACTMON_BACKUP_DIRECTORY`. Both must be absolute paths. Paths inside
`/home/invictus/.local/share/actmon` are already writable. A destination's parent must exist;
the backup command creates its final private directory and sibling control path.
The same environment file is read by both jobs; changes take effect on their next
start (restart the collector after editing). Configuration changes do not migrate
existing history or snapshots. Keep the existing data until a separate migration
has been verified.

For storage outside the default data directory, pre-create the required parent
owned by `invictus` with mode 0700. In the appropriate service drop-ins, reset
`BindPaths=` in `[Service]` and list each required parent with
`BindPaths=/absolute/parent`. Reset `RequiresMountsFor=` in `[Unit]` and list the
same parents there. Update the environment paths too. The backup job needs the
database parent for SQLite WAL/SHM handling and the backup parent for sibling
staging. Keep `ProtectHome=tmpfs`, but its hiding applies only to `/home`, `/root`,
and `/run/user`. A home outside those trees is not hidden by that setting; such
layouts need a separately reviewed sandbox configuration if equivalent isolation
is required. Resetting the old paths matters for alternate home layouts where
`/home/<user>` does not exist.
Do not choose a mount point for the published directory if its sibling staging
would land on another filesystem.

Change the timer through `systemctl edit actmon-backup.timer`, for example:

```ini
[Timer]
OnCalendar=
OnCalendar=*-*-* 12:00:00
```

Reload the manager and restart the timer after editing. This does not change rclone.
Actmon still reports excess over seven snapshots without deleting anything; pruning
requires coordination with external readers.

## Upgrade or remove

### Replace the old instance names

Existing deployments using `actmon@invictus.service` must switch to the plain
names before enabling the new collector. The account, environment file, binary,
database, and backup paths are unchanged; no data migration or rebuild is needed.
The old and new collector must not run together against the same database.

Before switching, inspect the old units with `systemctl cat` and record their
enabled, active, and masked states. Install the three new unit files into
`/usr/local/lib/systemd/system` using the commands above, preserving the existing
environment file and udev rules. Copy any template or instance drop-ins to the
corresponding plain unit's `.d` directory, reviewing conflicts instead of blindly
overwriting files. For this laptop's existing 06:00 schedule, with no new drop-in
already present:

```sh
sudo install -d -m 0755 /etc/systemd/system/actmon-backup.timer.d
sudo cp -a /etc/systemd/system/actmon-backup@invictus.timer.d/schedule.conf /etc/systemd/system/actmon-backup.timer.d/schedule.conf
```

For the currently enabled collector and timer, stop and disable the old names:

```sh
sudo systemctl disable --now actmon-backup@invictus.timer actmon@invictus.service
sudo systemctl stop actmon-backup@invictus.service
```

If `/var/lib/systemd/timers/stamp-actmon-backup@invictus.timer` exists, copy it with
`sudo cp -a` to `/var/lib/systemd/timers/stamp-actmon-backup.timer` while both timers
are stopped. Preserving its timestamp avoids an extra persistent catch-up caused
solely by the rename. Preserve an existing backup opt-out by masking the new timer
before enabling the collector; do not start the timer in that case.

For the currently enabled backup schedule:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now actmon.service actmon-backup.timer
systemctl status actmon.service actmon-backup.timer
systemctl list-timers actmon-backup.timer
```

Confirm the effective schedule, historical coverage, and fresh collection using
the deployment validation above. After verification, archive the old template
files and their old drop-in directories outside systemd's unit search paths, then
reload the manager. Keep them for rollback. Do not leave the old instance enabled.

### Update the binary or storage

For an executable-only update, run `bash install.sh` as your normal user.
It builds and validates the new binary before requesting installation privileges,
then atomically replaces `/usr/local/bin/actmon`. Running jobs retain their old
executable until they exit; new commands use the replacement immediately. The
script does not restart services, change units or local configuration, or move data.
Restart the collector separately when it needs to use the new executable. Record
its prior state first: leave an inactive collector inactive, retain enablement and
masks, and do not re-enable an opted-out timer. For an active collector whose new
binary is ready, `sudo systemctl try-restart actmon.service` activates the update
without starting a collector that has since become inactive. Let a running backup
job finish using its existing executable.

For service configuration or storage changes, stop the timer and both services
first. Preserve local configuration and data. **The default data directory changed
from `/var/lib/actmon/<user>` to `/home/<user>/.local/share/actmon`.** Existing
installations must choose their paths before installing the new units and restarting:

- **Keep existing storage:** explicitly set `ACTMON_DATABASE` and
  `ACTMON_BACKUP_DIRECTORY` to the old locations in the environment file. Apply
  the path overrides above to both services, replacing the default bind and mount
  requirements with the existing parents. Environment overrides alone do not
  change the sandbox. Homekeep still cannot select snapshots outside home.
- **Move to home:** create the new bind source with explicit ownership before
  starting either service:

  ```sh
  sudo install -d -o invictus -g invictus -m 0700 /home/invictus/.local/share/actmon
  ```

  With collection stopped, create and verify a final snapshot of the old database
  using the [backup procedure](backups.md#create-a-snapshot). Follow the
  [restore rehearsal](backups.md#restore-rehearsal) in a separate working directory.
  Install the verified database into the new private directory as `invictus`, without
  overwriting any existing database or pairing it with old WAL/SHM/journal files.
  Preserve the old dataset and published snapshots for rollback. Update any
  explicit environment overrides to the new paths and
  verify historical coverage against the old database before restarting.

Without the bind source, the new units cannot start. Creating an empty directory
alone is not a history migration: the collector can initialize a fresh database,
and new backups would omit the old history. These steps do not automatically move
or delete any data; a first installation has no old dataset to migrate.

Install the new binary/units, reload systemd, and restart the collector and timer
(unless intentionally opted out) only after the chosen paths and data are ready.
Confirm historical coverage and new samples, then run a manual backup and restore
verification. Keep the old dataset until the transition has been verified. No
schema migration is added by this packaging milestone.

To stop deployment while preserving all recorded data:

```sh
sudo systemctl disable --now actmon-backup.timer actmon.service
sudo systemctl stop actmon-backup.service
```

For complete removal, remove only the installed Actmon units, drop-ins, environment,
sysusers definition, binary, and Actmon udev rule, then reload the managers. Restore
normal device permissions through a reconnect or targeted change event after rule
removal. Keep the group until it has no remaining uses; removing its sysusers
file does not delete the group. Do not delete history or backups as part of uninstall.

## Offline verification

`bash scripts/verify-packaging.sh` checks the units in a temporary root,
validates sysusers in dry-run mode, validates udev rule syntax without resolving
an uninstalled group, and checks the daily calendar expression. It installs no host
files and starts no service. It requires systemd analysis/sysusers tools and udevadm
with `verify` support (systemd 254+). Runtime permissions, SELinux policy, actual
udev matching, hotplug behavior, and sandbox access require the deployment checks
above; parser success alone does not establish those properties.

References: [systemd execution settings](https://www.freedesktop.org/software/systemd/man/259/systemd.exec.html),
[service lifecycle](https://www.freedesktop.org/software/systemd/man/259/systemd.service.html),
[timer behavior](https://www.freedesktop.org/software/systemd/man/259/systemd.timer.html),
[udev rules](https://www.freedesktop.org/software/systemd/man/259/udev.html).
