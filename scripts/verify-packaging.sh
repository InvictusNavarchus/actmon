#!/usr/bin/env bash
# Offline validation only: never reload udev/systemd or install/start host units.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/etc/systemd/system" "$fixture/etc/sysusers.d" \
    "$fixture/etc/udev/rules.d" "$fixture/usr/local/bin"
cp "$repo"/packaging/systemd/* "$fixture/etc/systemd/system/"
cp "$repo/packaging/sysusers/actmon.conf" "$fixture/etc/sysusers.d/"
cp "$repo/packaging/udev/99-actmon-input.rules.example" "$fixture/etc/udev/rules.d/99-actmon-input.rules"
# Verification checks executability but must not run the application.
printf '#!/bin/sh\nexit 99\n' > "$fixture/usr/local/bin/actmon"
chmod 755 "$fixture/usr/local/bin/actmon"
for target in sysinit basic shutdown local-fs multi-user timers; do
    printf '[Unit]\nDescription=Validation fixture\nDefaultDependencies=no\n' > "$fixture/etc/systemd/system/$target.target"
done
systemd-analyze verify --root="$fixture" --man=no --generators=no \
    actmon@invictus.service actmon-backup@invictus.service actmon-backup@invictus.timer
# Explicit configuration arguments are host paths; --root redirects account files.
systemd-sysusers --root="$fixture" --dry-run "$fixture/etc/sysusers.d/actmon.conf"
udevadm verify --resolve-names=never "$fixture/etc/udev/rules.d/99-actmon-input.rules"
mapfile -t calendars < <(sed -n 's/^OnCalendar=//p' "$fixture/etc/systemd/system/actmon-backup@.timer")
if ((${#calendars[@]} == 0)); then
    echo 'Packaged backup timer has no OnCalendar expression' >&2
    exit 1
fi
for calendar in "${calendars[@]}"; do
    systemd-analyze calendar "$calendar" --iterations=2
done

# Exercise the documented opt-out without contacting the host service manager.
systemctl --root="$fixture" enable actmon@invictus.service
systemctl --root="$fixture" is-enabled actmon@invictus.service
systemctl --root="$fixture" is-enabled actmon-backup@invictus.timer
systemctl --root="$fixture" mask actmon-backup@invictus.timer
systemctl --root="$fixture" enable actmon@invictus.service
systemctl --root="$fixture" is-enabled actmon@invictus.service
test "$(readlink "$fixture/etc/systemd/system/actmon-backup@invictus.timer")" = /dev/null
