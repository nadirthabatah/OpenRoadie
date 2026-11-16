#!/bin/sh
set -eu

# Reload udev rules and wait for the new uaccess tags to be applied.
# udevadm trigger is asynchronous — settle ensures the tags are in place
# before the script exits so the agent can open /dev/hidraw* and the mouse's
# /dev/input/event* node immediately, even for a device connected before the
# install.
#
# Best-effort, like the rest: with no udev daemon running (a container, WSL)
# these fail, and failing here left the whole package half-installed.
if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules 2>/dev/null || true
  udevadm trigger --subsystem-match=hidraw 2>/dev/null || true
  udevadm trigger --subsystem-match=input 2>/dev/null || true
  udevadm trigger --subsystem-match=tty 2>/dev/null || true
  udevadm trigger --subsystem-match=misc --attr-match=name=uinput 2>/dev/null || true
  udevadm settle 2>/dev/null || true
fi

# Refresh icon and desktop caches (best-effort).
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -qtf /usr/share/icons/hicolor || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || true
fi

echo "OpenRoadie installed. Enable the background agent for your user with:"
echo "  systemctl --user enable --now roadie-agent.service"
