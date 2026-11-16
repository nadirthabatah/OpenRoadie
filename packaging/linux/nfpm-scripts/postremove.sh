#!/bin/sh
set -eu

# Reload udev rules and wait for the uaccess revocation to take effect.
# Best-effort, as in postinstall.sh: no udev daemon must not fail a removal.
if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules 2>/dev/null || true
  udevadm trigger --subsystem-match=hidraw 2>/dev/null || true
  udevadm trigger --subsystem-match=tty 2>/dev/null || true
  udevadm trigger --subsystem-match=misc --attr-match=name=uinput 2>/dev/null || true
  udevadm settle 2>/dev/null || true
fi
