# Collector architecture

This document describes the implemented collector. See the [data model](data-model.md)
for measurement definitions and query examples, and the [design](design.md) for
intended direction and implementation milestones.

## Device discovery and access

Device discovery reads `/sys/class/input` metadata and prints JSON with the exact
name, event node, and sysfs identity. It does not open input devices or collect
input. Device names can contain trailing spaces; preserve them in selectors.

The collector needs read access to the selected `/dev/input/event*` nodes. It
opens them read-only, never grabs them, and never falls back to the physical
keyboard. The [systemd deployment guide](systemd.md) provides installation
steps and device-permission rules. The CLI itself does not install those rules
or change permissions. Running as root is not required by the design.

## Source selection and recovery

Selectors use exact names and refuse ambiguous matches. A missing or inaccessible
device is recorded as unavailable while the other source can keep collecting.
Hotplug discovery scans sysfs every two seconds and retries unavailable devices;
this first milestone uses periodic discovery rather than a udev monitor. Discovery
failures are reported separately and retried while existing readers continue;
reader errors still close the affected source. Recreated virtual devices are
reopened. There may be an unobserved period before discovery.

## Collector locking

The collector holds an advisory lock next to the database to reject a second
Actmon collector targeting the same resolved path. The lock file is persistent;
the kernel releases its lock when a process exits. SQLite itself does not enforce
this application-level single-collector policy against unrelated writers.

## Diagnostics

Use `RUST_LOG=actmon=debug` to include transaction latency diagnostics on stderr.
Structured error messages contain source metadata, not input contents.

## Aggregation and coverage loss

Input is captured independently of the SQLite writer. A one-second grace period
allows delayed boundary events to arrive before an interval is finalized.
Normally roughly one to two seconds remain in aggregation before being queued;
writer delay can add to that. Already-finalized intervals receiving late input
are conservatively marked incomplete rather than silently moving old input into
the current second.

Loss ranges also apply to intervals created or inserted in later batches. Source
coverage changes at exact interval boundaries do not create artificial partial
intervals. Loss and disconnect events only affect the source's supported
categories. Shutdown invalidates the unread tail since each source's last
successful drain, including any full interval crossed by that tail.

Raw evdev mode exposes SYN_DROPPED directly. The decoder ignores events through
the next SYN_REPORT, does not synthesize replacement keypresses, and marks the
potentially affected range incomplete. It conservatively includes the period
since the previous drained read. Clock changes, detected suspension through
CLOCK_BOOTTIME comparison, and scheduling gaps over two seconds reset interval
continuity and leave explicit gaps. These clock observations do not identify a
specific suspend cause or replace system journal evidence.

Direct suspend notifications and optional session metadata remain later work.

## Writer queue and durability

WAL with `synchronous=FULL` synchronizes completed transactions. Measurements
still in memory are not durable. The capture-to-writer queue holds 120 batches;
it is not a guaranteed number of seconds. Persistent write failures retry every
250 ms with a 250 ms SQLite busy timeout. When the queue fills, dropped batches
become an explicit loss range on the next accepted batch. Shutdown ends failed
write retries; storage failure at exit can leave only a journal explanation and
an unexplained database gap.

## Commit timing and freshness

The latest commit timestamp is sampled before its successful transaction, so it
is an approximate freshness indicator. `latest_capture_to_commit_us` retains
the newest interval's measured age at the start of the transaction that inserted
it, excluding the commit itself. Event-only commits preserve this measurement
and its association with `latest_interval_utc_us`; it is not a continuously
updated age or a measurement of the event-only commit.
`previous_commit_latency_us` retains the preceding transaction's measured cost
without adding a second synchronization per batch. If a retry establishes that
an uncertain commit persisted, the next batch records that latency as NULL rather
than reusing an older transaction's timing. The recovery is logged separately.
No heartbeat or timestamp proves uninterrupted collection or power-loss recovery.
