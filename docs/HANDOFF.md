# Handoff — where this fork stands

`AGENTS.md` is the standing contract: architecture, gate commands, subsystem
rules. This file is the other half — the state of *this fork's* work, the
things that cost real time to learn the first time, and what comes next.
Read it before picking anything up.

Last updated after PR #13 merged on 2026-09-03, which brought monitors,
Elgato lights including the Key Light Neo over USB, TourBox controllers,
VIA protocol 12, and the Focusrite audio stack onto `master`. Everything
described below is merged.

---

## 1. Who this is for

Nadir Thabatah is blind. He works by dictation and screen reader, often from
an iPhone. That one fact decides more design here than any technical
constraint, and it is why several things that look like style rules are
actually correctness rules:

- **Every output is read aloud.** Box-drawing characters, bare symbols, and
  `(s)` plurals are unlistenable, so `roadie-cli/src/spoken.rs` enforces their
  absence and integration tests check the exact shipping strings. Counts and
  verbs have to agree — "the 1 device you cannot open **are** not among them"
  shipped once and is exactly the class `spoken::assert_agrees` now catches.
- **Colour is never the only signal.** Tested, not merely intended.
- **Padding is a bug, not cosmetics.** Monitors pad text with NUL, and
  `str::trim` does not remove it. A padded model name reaches a screen reader
  as a name followed by silence, with nothing on screen to explain it. That
  was a real defect found in `roadie-ddc`, and the same trim now guards both
  the capability parser and the EDID parser.
- **Never say a word twice.** `Edid::describe` resolves `GSM` to "LG" and then
  declines to prepend it to a name that already starts with it, because
  "LG LG ULTRAFINE" is worse aloud than on screen.
- **An unrecoverable action needs a name.** `PowerMode::Off` can stop a
  monitor answering DDC, leaving the bezel button as the only way back. For
  someone who cannot see the bezel that is not an inconvenience, so
  `is_recoverable` exists to be checked and anything unrecognised is assumed
  unrecoverable.

- **Never present a decision as a picker.** An interactive multiple-choice
  list is inaccessible: it has to be navigated rather than read, and the
  options do not simply arrive in the ear the way prose does. When an agent
  needs Nadir to choose something, the options go in plain sentences and he
  types the answer back. This applies to Claude Code's `AskUserQuestion` tool
  specifically — do not call it. Write the options out instead.

When in doubt about wording, read the sentence out loud.

## 2. Where the work is

- **This fork:** `github.com/nadirthabatah/OpenRoadie`, branch `master`.
- **Upstream:** AprilNEA's OpenLogi. Still the source of most of the HID++
  code. Its brand assets were deleted rather than renamed.
- **The project is OpenRoadie; the command is `roadie`.** Every subcommand
  follows: `roadie doctor`, `roadie devices`, `roadie via`, `roadie mcp`.

**A trap worth naming.** `nadirthabatah/Open-switch` is a *different*
repository — the original planning notes, docs only, no code. A session can be
started with that as its working directory while all the real work is in
OpenRoadie. Check `git remote -v` before believing a clean working tree.

## 3. What is built

Merged, in order:

| PR | What |
| --- | --- |
| 1–2 | MCP server over stdio, and the Elgato Stream Deck driver |
| 3 | Cross-vendor device survey — `roadie devices` says what it can configure on each |
| 4 | VIA/QMK keyboards and macro pads |
| 5 | Portable setup bundles — export a profile, import it on another machine |
| 6 | `roadie doctor` diagnostics |
| 7 | MCP parity with the CLI |
| 8 | The rename: 720 files, OpenLogi to OpenRoadie, `openlogi` to `roadie` |
| 9 | The logo, and the retirement of upstream's artwork |
| 10 | `roadie-ddc` — DDC/CI, MCCS and EDID as a pure crate |
| 11 | Repository URLs pointed at the post-rename name |
| 12 | This handoff |
| 13 | Monitors over DDC/CI, Elgato lights (network and Neo USB), TourBox, VIA 12, and Focusrite audio |

### Device categories that work today

Logitech HID++ mice and keyboards, Elgato Stream Decks, QMK/VIA keyboards and
macro pads, UVC webcams, Logitech standalone lights, monitors over DDC/CI,
Elgato Key Lights over the network and the Neo over USB, TourBox controllers
over their serial port, and Focusrite audio interfaces over their vendor USB
interface.

### TourBox, as of the branch

`roadie-tourbox` is the fourth device family on this branch and the first that
is neither HID nor network. A TourBox presents a **USB CDC serial port** and
streams **one byte per event**: the low six bits name the control, the high two
say what happened to it. For a button, bit 7 is release. For a wheel, bit 6 is
direction and bit 7 is "the turn has ended" — so a wheel uses all four
combinations and a button uses two. No framing, no length, no checksum, no
sequence number. A byte read is an event and a byte lost is an event lost.

That shape decides the crate. There is nothing to resynchronise to, so a byte
the build cannot explain is either a control from a model it has never met or a
corrupt byte, and *nothing in the encoding tells those apart* — hence a typed
error rather than a nearest-match, because a wrong guess here is a keystroke
nobody asked for. Buttons and wheels are separate types (`Button` and `Wheel`)
so that the combinations which really are impossible — a button that claims to
have turned — cannot be constructed and have to be rejected only at the one
place bytes come in.

It is one crate rather than two, like `roadie-keylight` and unlike
`roadie-ddc`: the host half is a serial port with no platform FFI in it, so a
`serial` feature says the same thing as a sibling crate would, at far less
ceremony. The protocol half holds the wasm portability claim.

**The whole surface is now hardware-verified.** On 2026-08-31, against a
TourBox Elite on this desk: enumeration finds it by USB identity
(`c251:2005`, serial `00000001`), `roadie devices` files it as configurable,
and the port opens with nothing else holding it. On 2026-09-02, the rest: a
450-event pass with every button pressed, every wheel turned both ways and
every wheel pressed, decoded without a single protocol error. All fourteen
buttons and all three wheels answered to their transcribed codes — but only
after the unlock discovery below, which is the finding of the sitting.

**Cross-checking three drivers was worth more than the mutation sweep.** The
codes were first transcribed from one project, then compared against two more
written independently, for different models, in different languages. That is
not hardware, but it is three witnesses, and it settled one open question and
found one outright defect that no test could have.

The open question was the knob's press byte, and hardware has now settled it.
One source records `0x77`, which would make the knob the only control setting
a turn bit while being pressed. Two others record `0x37` — and the first
source's own *release* byte is `0xb7`, which is `0x37` with the release bit
set and therefore inconsistent with its own press. The build implements
`0x37` and **rejects `0x77` by name** rather than quietly accepting both, so
hardware could overturn it; on 2026-09-02 the Elite's knob was pressed with
this build listening and decoded cleanly as a knob press, which is only
possible if it sent `0x37`. The refusal test,
`the_disputed_knob_byte_is_refused_rather_than_guessed_at`, stays: `0x77`
remains a byte no known device sends.

The defect was worse and is the reason this exercise earned its keep. **A wheel
reports the end of a turn**, and this build rejected those bytes as impossible.
A wheel sends a run of detents and then one more byte carrying the same control
code and direction with the high bit set — the same bit that means "released" on
a button, which is consistent rather than coincidental, since both mark the end
of something held. One driver names them the `_STOP` family; another prints one
from live hardware. Every turn of every wheel would have ended in a spurious
error at the moment the hand stopped. The type model was wrong too, and said so
out loud: it claimed "a wheel turns one way or the other and is never released",
which is exactly the kind of confident sentence that should be checked against a
second source. `TurnPhase` now carries it, and a wheel has no impossible
action at all — both of its high bits are meaningful and independent.

The general lesson, which is the one to carry: **the mutation sweep and the
cross-check find different things.** The sweep proves the tests bite on the
behaviour the code claims. It cannot see a claim that is wrong in the code and
the tests alike, because both were written from the same reading of the same
source. Only a second reading finds that, and here it took a third to break the
tie.

**Two findings worth keeping, and one confident sentence that hardware
demolished.** macOS publishes every serial device twice, as `/dev/cu.NAME`
and `/dev/tty.NAME`; listing both reported one controller as two, and the
`tty.` half is also the wrong one to hand anybody, because opening it blocks
waiting for a carrier a controller never asserts. The demolished sentence is
this one, which stood here until 2026-09-02: "a TourBox streams whether or
not anything has ever talked to it, which is why reading one needs no
handshake and no write access." That was transcribed from NEO drivers, and
**an Elite says nothing at all until it is sent an 8-byte unlock command** —
`event::UNLOCK_MESSAGE`, carried byte-identically by two independent Elite
drivers, one of which recovered it from a Bluetooth capture. It cost three
sessions of silent listeners, each explained away by a plausible neighbour
(a cable, then TourBox Console — which turned out not to be installed at
all). The Elite answers the unlock with 26 bytes beginning `0x07`, and only
then streams. `TourBox::open_path` now performs the whole exchange — unlock,
reply, 94-byte haptics setup — and a device that answers nothing is
configured anyway, because the models without the requirement never answer.

One more Elite fact from the same pass: it sends **no wheel-stop bytes**.
The `_STOP` family that `TurnPhase::Ended` decodes came from other drivers
and never once appeared in 450 events; the decoder keeps accepting them,
because a model that sends them would otherwise fail on every turn, but on
an Elite their absence is the expected shape of a turn.

Still unverified: every model other than the Elite.

### Monitors, as of the branch

`roadie-ddc` is the pure protocol: packet framing, VCP features, capability
strings, EDID. 91 unit tests, no host I/O, holds the wasm portability claim.

`roadie-display` is the host half: the `VcpBackend` seam, the `Ddc` adapter
carrying framing and timing and retries, backends for all three platforms, and
`mock::Panel`, a monitor made of software that answers *packets* so everything
above it can be driven with nothing plugged in. `roadie display` is the CLI,
`list_displays` / `read_display_settings` / `set_display_setting` are the MCP
tools, and `roadie devices` includes the screen.

**One monitor has now been reached, and it half-answered.** On 2026-08-30 the
macOS backend ran against Nadir's desk for the first time: the registry walk
found the panel, the `dlopen`ed private calls behaved, and the EDID read at
I²C `0x50` named it — an LG TV, which then never acknowledged DDC at `0x37`
(`IOAVServiceWriteI2C` 0xe0114000, LG TVs speak CEC instead). So the transport
and identification were verified and the reply checksum seed was not; that
needed a desktop monitor.

**The desktop monitor arrived, and the seed is verified.** On 2026-09-02 an
RTK HG560T34 on USB-C answered everything: brightness and contrast reads, a
full capability string (MCCS 2.2, seven switchable inputs), and a brightness
write that read back correctly and was restored. Replies came back, parsed,
and passed the checksum — which closes the one gap no mock could. A detail
worth keeping: macOS listed only the LG TV as an online display at the time,
and the registry walk still found and drove the HG560T34 — the DDC transport
does not care whether the window server is drawing to a panel. The backends differ per platform,
and the table below is what each one had to be written against:

| Platform | API | Notes |
| --- | --- | --- |
| macOS | `IOAVService` on Apple silicon | Private framework, resolved by `dlopen`. What MonitorControl and BetterDisplay use. Never works on the built-in display. Intel's `IOI2CSendRequest` path is deliberately **not** implemented — an Intel Mac is told so plainly rather than handed a second untestable path. |
| Windows | `dxva2.dll` | Higher level: it does the framing itself, so `roadie-ddc`'s packet layer is unused there |
| Linux | `/dev/i2c-*`, discovered through `/sys/class/drm/*/ddc/i2c-dev/` | The kernel already publishes each display's EDID at `/sys/class/drm/*/edid`, so displays can be named with no I2C access at all |

That table is why the backend trait belongs at the *VCP* level (`get`, `set`,
`capabilities`) rather than at the packet level — Windows never sees a packet.

The EDID is read differently on each: Linux from `/sys/class/drm/*/edid`, which
needs no permission at all; macOS off the EEPROM at I²C address `0x50`, on the
transport that already exists, so identification needs no second private
dependency; Windows not at all, which is a real gap — `dxva2` describes most
panels as "Generic PnP Monitor", so displays there are numbered rather than
named.

## 4. Working agreements that cost time to learn

**`cargo run -p xtask -- ci` is the only gate that counts.** A hand-rolled
gate built from remembered commands once passed while missing MSRV,
cargo-deny and shell entirely, and excluded a crate CI does not exclude. The
repo has its own runner; use it. On Linux it reports 11 passed and 1 skipped;
the skip is the macOS test job and it is **not** a pass — say so in the PR.

Its prerequisites on a fresh container: `libxkbcommon-dev`,
`libxkbcommon-x11-dev`, `shellcheck`, `shfmt`, `cargo-deny`,
`gcc-mingw-w64-x86-64`, and the `x86_64-pc-windows-gnu` and
`wasm32-unknown-unknown` targets.

**A stale `target/release/roadie` impersonates a cross-surface bug.** On
2026-09-03 the binary on disk was four days old, predating the Neo USB
driver that had merged that morning. The symptoms were exactly the shape
section 4 teaches you to read as drift between the three surfaces:
`roadie light list` reported no lights at all, while `roadie devices` saw
the Neo, named it, and filed it under "detected, not configurable by this
build" — one surface confidently contradicting another about the same
attached device, with the handoff insisting the path was verified. Nothing
was wrong with the code. `ls -la target/release/roadie` against
`git log -1` settles it in one command, and it is worth spending that
command *before* reading any driver source, because a stale binary and a
genuine drift defect present identically. Verify hardware with a binary you
watched compile.

**Never push while a run is in progress on the same branch.** CI sets
`concurrency: cancel-in-progress: true`, keyed on the ref. Fourteen commits
once went unverified this way while the last completed run was fourteen
commits stale, and "the gate is green" was reported on the strength of it.
Batch locally, push once, wait. Different branches are safe — the key includes
the ref.

**macOS compiles here.** `cargo check` does not link, so
`--target aarch64-apple-darwin` type-checks a macOS-only file — signatures,
lifetimes, borrows and all — on a Linux container with no Mac anywhere. It
works for any crate whose macOS dependencies are pure Rust, which the `objc2`
family is, and it caught every mistake in `roadie-display`'s `IOAVService`
backend. `rustup target add aarch64-apple-darwin` and then clippy that target
alongside the Windows one. What it does not prove is anything about *running*:
no linking, no frameworks resolved, no private symbol confirmed to exist, and
no `dlsym`'d signature checked against the one Apple ships — a wrong one
type-checks perfectly and is undefined behaviour when called. The full note is
in `.claude/rules/cross-platform.md`.

**A new dependency can move the `gpui` git pin, and a plain `cargo check` is
enough to do it.** The manifest entry has no rev, so a re-resolve follows the
branch's HEAD. It does not look like a dependency problem: it is a compile
error deep inside `gpui_linux` about `ImageFormat::iter`, with a note about two
versions of `strum`. `git checkout Cargo.lock` does not fix it, because the
next `cargo check` moves it again. Let the lock pick up the genuinely new
packages, then `cargo update -p gpui --precise cc053a4a...`, then check that
`git diff Cargo.lock` shows only the new crates.

**Read the three surfaces side by side; the odd one out is usually a bug.**
Every device family is reachable three ways — the command line, the MCP
server, and the GUI through the agent — and they are written at different
times, so they drift. Four defects this session were found that way and none
of them by a failing test. A Key Light that stopped answering was listed by
the CLI and by MCP, both with a comment explaining why, and silently dropped
by the agent, so it vanished from the GUI. `roadie doctor` claimed "no
peripheral of any kind" while `roadie devices` had grown two families it did
not check. The technique is cheap: pick one question — what happens when the
device does not answer? — and ask it of all three.

**A mutation can survive because the data happens to be sorted.** Swapping
"take the newest applicable firmware table" for "take the last one listed"
changed nothing in `roadie-scarlett`, because the tables are listed oldest
first. That is a fragility rather than an equivalence: a table added out of
order would have selected the wrong layout silently. The fix is a test with
deliberately jumbled data, which is the only thing that can tell the two apart
— and separately an assertion that the real data *is* ordered, which the code
does not rely on but which catches the mistake.

**Clippy reports as warnings; `-D warnings` makes them failures.** Grepping
output for `^error` hides every one of them. Check the exit code — and not
after a pipe, where `$?` is the last command's status, not clippy's.

**A test that agrees with the code proves nothing.** Wire-format tests spell
byte sequences out literally rather than computing them from the function
under test. `51 82 01 10 AC` is the documented DDC read-brightness sequence
and checks the framing from outside; the packed EDID codes `0x10AC` and
`0x1E6D` are the published PNP IDs for Dell and LG.

**Check the echo.** Neither HID++ VIA nor DDC has sequence numbers. A reply
read too early is the *previous* answer, and every reply after it is shifted
by one — brightness set from a contrast reading, with nothing reporting an
error. Both protocols echo what they are answering. Both are checked.

**Forgive malformed data from hardware, and record what you forgave.**
Monitors ship unbalanced capability strings and miscomputed EDID checksums. A
strict parser rejects working hardware, which is the wrong trade for a tool
whose job is the hardware someone already owns. Recover, and put what you
recovered from in a `warnings` field so `doctor` can print a specific sentence
instead of a vague one.

### Mutation testing is what actually finds the defects here

The method: change one thing so the code is deliberately wrong, run the tests,
and require a failure. A mutation that survives is a claim nothing checks.
Script it — apply, test, revert — and expect roughly a fifth to survive the
first pass.

Track record across the fork: six sweeps, and **every gap was in code that
had prose about it but no test.** The comments were consistently ahead of the
tests. The driver logic, where the consequences are worst and the care was
highest, came back clean each time.

The sixth sweep, over `roadie-display`, held to that exactly. Twenty-nine
mutations, twenty-seven killed on the first pass. One survivor was an
equivalent mutant — a redundant emptiness check in front of `Edid::parse`,
which rejects a short block anyway, so the guard was a branch that could not
fire; it was deleted rather than given a test that would not have bitten. The
other was real, and was the sort this method exists to find: the Linux
backend's central promise, that a display is never dropped for being
unreachable, had a paragraph of prose and no test, because enumeration read
`/sys/class/drm` directly and `/sys` cannot be written to. Making the root a
parameter turned five claims into tested ones, and the test written for it
immediately exposed a message that nested one error's text inside another's.

The seventh, over `roadie-keylight` and its front ends, found four survivors in
twenty-two and **not one of them was in the code** — all four were tests that
looked like they proved something and did not. The IPv4-preference test
compared against an address the filter already excluded, so any rule at all
would have passed it. The "too wide for the wire" test used 70000, which
truncates to 4464 and clamps to the same answer as the correct code; 65536 is
the value that tells them apart, because it truncates to zero and clamps to the
opposite end. The refusal of a call that changes nothing was reachable only
through the network, so it was extracted into a function that could be tested.
And `full_description` had never been checked for a network device at all. That
is a different failure mode from the earlier sweeps — prose ahead of tests —
and worth naming: **a test can be present, passing, and evidence of nothing.**

The eighth, over `roadie-tourbox`, ran eighteen and killed eighteen. That is
the first clean first pass on the project, and it is worth being suspicious of
rather than pleased about: the crate is small, its logic is one mask and two
lookups, and the tests were written from the same transcription the code was.
A sweep cannot find a shared misreading of the source material — only hardware
can, which is what section 5's outstanding button pass is for.

Three cautions learned the hard way. `rustfmt` reflows source, so a
pattern-match mutation script silently finds nothing — always report
"pattern not found" separately from "survived".

**Restore the file's timestamp too, not just its contents.** `mv backup.bak
file` puts the bytes back and takes the backup's *older* mtime with them, which
is older than the artifact cargo built from the mutated source. Cargo then
judges the build fresh and the next `cargo test` runs **the previous mutant's
binary against clean source**. This bit exactly once, on the `roadie-tourbox`
sweep: a test that had passed all day failed immediately after the sweep, with
the code visibly correct on disk. Read the other way round, the same trap turns
a stale pass into a reported survivor, which is the more expensive direction.
`touch` the file after restoring it. And distinguish a real gap
from an *equivalent* mutant: `roadie-ddc` has one where truncating an
over-long input name cannot produce a match, because no valid name is as long
as the buffer. That one is documented in the code rather than papered over
with a test that would not really bite.

## 5. The gap that only Nadir can close — and the first sitting

The first hardware sitting happened on 2026-08-30, on macOS at Nadir's desk.
What it verified, for the first time anywhere:

- **HID++ over a Bolt receiver**: an MX Mechanical and an MX Master 3S
  enumerate with battery, serials and feature tables, by direct enumeration.
- **The Stream Deck XL, end to end**: open, brightness, image writes, key
  decode, and the catalogue's key numbering — the physical top-left key is
  key 0, horizontal neighbours differ by 1 and vertical by 8, and press and
  release pair correctly. The visual half was closed on 2026-09-03 by an
  iPhone photograph of a painted test layout, read by a session with working
  eyes: red, green, blue and white landed on the four physical corners
  matching keys 0, 7, 24 and 31, and a label on key 19 sat in the third row,
  fourth column — upright, unclipped, not mirrored. A second photo at 20
  percent brightness closed the text-on-colour caveat the first one left:
  "TOP LEFT" and "BOT LEFT" read in full on their red and blue fills. The
  right-edge keys appeared to read "IGHT", and that scare is worth its
  paragraph: the renderer centres each line and refuses any scale whose
  widest line overflows the key, and dumping the actual rendered buffer
  showed the full word with margins — the missing R was the curved keycap
  refracting its own edge away at the camera's oblique angle. The widest
  line of a label gets the thinnest margin, so it is always the first
  casualty of a photo taken at an angle; check the buffer before blaming
  the renderer. A single ten-character word wraps mid-word across two
  lines, which is the only wrap a spaceless word allows.
- **The macOS display transport and EDID identification** — and not the reply
  checksum seed; see section 3.
- **Discovery honesty**: with no Elgato light on the network, `roadie light
  list` and the OS's own multicast browser agree there is nothing, rather
  than inventing something.

The sitting also found what no test could: **two other programs eat this
desk's Stream Deck.** Elgato's own app holds it exclusively — opens fail,
which the open error now explains. Worse, Logitech's device manager
(`com.logi.cp-dev-mgr`, installed with Logi Options+) holds the deck's
*input* in seize mode: opens succeed, writes land, and every key report goes
to Logitech alone, so the deck looks write-only to anything else. No error
anywhere says so; an independent IOKit listener hearing silence is what gave
it away, and `launchctl bootout gui/<uid>/com.logi.cp-dev-mgr` is what frees
it. `streamdeck verify` now names both causes when it sees no key press.

**A second sitting, 2026-08-31, on the TourBox.** The controller that the
previous session recorded as "unenumerated at USB level, likely a cable issue"
was exactly that: it now enumerates, and it is a TourBox Elite. The cable
diagnosis was right and is worth remembering, because a charge-only USB-C cable
presents as a controller that does not exist rather than as a cable that does
not work — which is why `roadie tourbox` names the cable before anything else
when it finds nothing.

What that sitting verified is in section 3. What it has **not** verified is the
control codes: the outstanding step is one pass of `roadie tourbox listen` with
every button pressed and every wheel turned, which is the only thing that can
confirm the transcribed bytes and settle the knob dispute. The hardware for it
is on the desk, so this is the cheapest open verification on the project.

**A third sitting, 2026-09-02.** It began with nobody at the desk and ended
with Nadir lending his hands three times — a switch flip and two button
passes. Three of the four open hardware gaps closed, and the fourth gained
its hardware:

- **The DDC monitor arrived and the checksum seed is verified** — the RTK
  HG560T34 details are in section 3. Input switching was deliberately not
  tested: changing the input of a monitor at an unattended desk takes the
  screen away from whoever comes back to it, and that choice belongs to the
  person sitting there.
- **The VIA board arrived, spoke protocol 12, and exposed a gap this build
  then closed.** A Kiiboom Cybrix 16 (`5343:0080`) answered the handshake and
  was refused: it speaks VIA protocol 12 and the build implemented only 9.
  The four commands this build sends are byte-identical from 9 through 12 —
  what changed in between was the lighting commands (unused here) and the
  quantum keycode numbering (deliberately not in the name table, which names
  only the era-stable HID-standard codes) — so 12 was added to the accepted
  set, with the transitional 10 and 11 still refused by name. After that, the
  first VIA hardware verification anywhere: handshake (protocol 12, six
  layers), a full keymap read, and a write of F24 confirmed by read-back and
  then undone. Protocol 9 remains transcription no board has confirmed.
- **The TourBox went from three sessions of silence to fully verified in one
  afternoon.** Nadir pressed everything with a listener running and nothing
  arrived — which finally put evidence against the "streams without a
  handshake" claim instead of against its neighbours. Two independent Elite
  drivers supplied the same 8-byte unlock command, the Elite answered it on
  the first try, and the button pass that followed decoded 450 events with
  zero errors: all fourteen buttons, all three wheels in both directions,
  all three wheel presses, and the knob dispute settled as `0x37`. Section 3
  carries the full story, including the wheel-stop bytes the Elite turns out
  never to send.
- **The "Scarlett Solo" resolved into two different devices, and the real
  Focusrite is a Vocaster Two.** For most of the sitting no Focusrite vendor
  id (`0x1235`) appeared anywhere in the IO registry; the box Nadir plugged
  and replugged proved to be a generic "USB AUDIO DEVICE" (`2f6e:4e02`, two
  in, two out, both descriptor strings just "USB AUDIO DEVICE") — no genuine
  Focusrite has ever presented that way, so it is a lookalike or a mix-up,
  and `roadie-scarlett` has nothing to say to it. Then a genuine **Focusrite
  Vocaster Two** enumerated (`1235:8217`, serial `V2VD42B2703F98`) — a
  device `roadie-scarlett` already carries by name, product id and config
  tables. Sections 6.7 through 6.9 finally have hardware to verify against;
  what stands between them and verification is the macOS host layer of
  section 6.10, which is now the next build.

**A fourth sitting, 2026-09-03.** The Key Light Neo and the Vocaster Two both
went from "seen but undrivable" to fully driven, and both needed hardware to
finish. The Neo's story is in section 6.6 and the Vocaster's in 6.10. Two
lessons generalise beyond either:

- **This desk has a USB socket that carries power and no data.** It made a
  healthy Key Light Neo invisible for a day, and the same trap with a
  charge-only *cable* hid the TourBox on 2026-08-30. When a device is missing,
  move it to a port where a known-good device just worked before believing
  anything else; the symptom is identical to a dead device.
- **A vendor app being present is not the same as the hardware being
  reachable.** Elgato's Control Center drives the Neo over USB and exposed
  nothing to the accessibility system when probed, which is precisely why the
  USB driver in 6.6 was worth building rather than deferring to the vendor.

**A fifth sitting, later on 2026-09-03, closed the Stream Deck's visual
half.** A test layout was painted — a distinct colour on each physical corner
key and a label on key 19 — and Nadir photographed the deck from overhead and
sent the picture. The photo confirmed the key numbering visually (each colour
on its intended corner) and the label rendering (upright, unclipped, correctly
placed). Details are in section 5's Stream Deck entry. The sitting had one
scare, and it resolved exactly as section 5 predicts: `streamdeck verify`
heard no key presses, the session's first guess blamed the HID usage page,
and the actual cause was the documented one — `com.logi.cp-dev-mgr` holding
the deck's input in seize mode. One `launchctl bootout` later, a full pass
arrived clean: all four corners decoded as keys 0, 7, 24 and 31 on the
correct rows and columns, five middle keys decoded in sequence, and every
press paired with its release. The Stream Deck XL is now verified end to
end — display, numbering, labels, and input. The bootout does not survive a
reboot: Logitech's manager returns with Logi Options+, and freeing the deck
permanently is a decision about that software, not about this driver.

**The Key Light's mired direction is verified, later on 2026-09-03.** It was
the last gap that needed an observer rather than a test. The pass was an A/B
at fixed brightness with temperature as the only variable: 2907 K, the
warmest the Neo accepts, then 6993 K, the coolest. Nadir reported the light
warm at the first and cold at the second, in that order — which is the
observation that rules out an inverted conversion, since inverting it would
have made the warmest *request* produce the coldest light. Endpoints alone
could still pass over a non-monotonic middle, so a sweep of eight midrange
values followed: every one round-tripped within 12 K of what was asked and
the sequence rose monotonically, against a 24 K worst case the
round-to-nearest is designed for. Direction and magnitude both hold.

Still needing hardware this desk has not shown: nothing on the current list.
The one gap that needed only hands, not purchases — an audible confirmation
that a Vocaster gain change is heard rather than merely stored — closed on
2026-09-04. The pass is recorded at the end of section 6.10.

`docs/VERIFYING.md` remains the ordered pass for whatever hardware appears
next, and this sitting held its shape: every failure it met was either a
permission, another program holding the device, or a genuine finding.

## 6. Monitors: what was built, and why it is shaped this way

**Sections 6.1 through 6.5 are done and on this branch.** They are kept in
full rather than reduced to a changelog line, because what they carry is the
reasoning, and the reasoning is what the next person needs when they change
one of these decisions. What comes after them is 6.6.

The last handoff left "which platform first" open. It was the wrong question
to block on. Everything except a panel
actually answering can be built and proven with no hardware attached and no
macOS host in the room, so the platform choice does not gate the work — it only
decides which backend gets *verified* first, and that is a hardware question
Nadir settles at his desk, not a design question a session settles for him.

So: the trait, the Linux backend and a mock landed first, because those are
the parts a Linux container can prove. macOS went in behind the same trait —
and, as it turned out, could be type-checked here after all; see section 4.
Windows is cross-compiled. Verification starts wherever Nadir happens to be
sitting, and `docs/VERIFYING.md` step 10 is the ordered pass for it.

### 6.1 The seam is a new crate, `roadie-display`

`roadie-ddc` stays pure. It is on the wasm portability list in
`xtask/src/commands/ci/jobs/steps.rs`, and that list is deliberately a claim a
crate earns and then has to keep — adding an ioctl to it would quietly retire
the claim. Host I/O goes in a sibling crate, exactly as `roadie-hid` is the
host-facing sibling of `roadie-device`.

What gets created:

| File | What it carries |
| --- | --- |
| `crates/roadie-display/src/lib.rs` | The `VcpBackend` trait — `get`, `set`, `capabilities` — a `Display` handle carrying the EDID identity, and `enumerate` |
| `crates/roadie-display/src/linux.rs` | `/sys/class/drm` walk, then `/dev/i2c-*` through the `ddc/i2c-dev` symlink, `I2C_RDWR` |
| `crates/roadie-display/src/macos.rs` | `IOAVService` on Apple silicon, `IOI2CSendRequest` on Intel |
| `crates/roadie-display/src/windows.rs` | `dxva2`, which does its own framing |
| `crates/roadie-display/src/mock.rs` | A scripted display behind the same trait, so the CLI and the MCP tools have something to answer them with nothing plugged in |

The trait sits at the VCP level rather than the packet level because Windows
never sees a packet: `dxva2` takes a feature code and a value. On Linux and
macOS the same call goes through `roadie-ddc`'s framing. Putting the seam one
layer lower would give Windows a packet layer to route around.

Two properties the Linux backend gets for free and should not give up. The
kernel publishes each display's EDID at `/sys/class/drm/*/edid`, so a machine
where the I2C nodes are not readable still produces a named list rather than an
empty one — the failure says *which* screen it could not reach. And enumeration
that needs no I2C access means `roadie devices` can include monitors without
asking for a permission first.

Three obligations belong to the backend rather than to `roadie-ddc`, because
they are transport facts:

- **Timing floors.** Roughly 40 ms between requests and 50 ms after a write.
  They are floors, not targets, and the failure mode of rushing a panel is a
  garbled reply rather than a clean error.
- **Retries.** A garbled reply is a transport event, so the transport decides
  whether to ask again.
- **Nothing on Windows.** `dxva2` does framing, timing and retry itself. The
  backend there is thin on purpose, and that asymmetry is the whole reason the
  trait is where it is.

The echo check stays in `roadie-ddc`, where it already is. Neither DDC nor VIA
has sequence numbers, so a reply read too early is the previous answer and
every reply after it is shifted by one.

### 6.2 The safety gate is part of the design, not a later polish

`PowerMode::Off` can leave a monitor not answering DDC, with the bezel button
as the only way back, and `Save Current Settings` writes to memory with a
finite number of erase cycles. So `is_recoverable` is checked before any write
crosses it, the CLI refuses an unrecoverable one without an explicit flag and
names the physical button that will be needed, and saving to the monitor's own
memory is its own subcommand that is never appended to a set.

### 6.3 `roadie display`, not `roadie monitor`

The previous handoff called it `roadie monitor`. Reconsidered, because
`crates/roadie-cli/src/cmd/mcp/tools/monitor.rs` already exists and watches
physical input — "monitor" there is the verb. Two unrelated things under one
word is bad on screen and worse aloud, so the screen is `roadie display` and
the MCP module is `displays.rs`.

`crates/roadie-cli/src/cmd/display.rs` plus a `Display` arm in `Command`.
Subcommands: `list`, `capabilities`, `get`, `set`, `input`, and `brightness` as
the shorthand for the one control people actually reach for. Output goes
through `spoken.rs` like every other subcommand, and the integration tests
spell the shipping strings out rather than computing them.

### 6.4 MCP parity, because that is the standing rule

`crates/roadie-cli/src/cmd/mcp/tools/displays.rs`, registered in `tools.rs`:
`list_displays`, `get_display_setting`, `set_display_setting`. Every category
since PR 7 reaches the MCP server as well as the CLI, and a monitor is the
category most worth asking an assistant to adjust, since the alternative is an
on-screen menu driven by bezel buttons.

### 6.5 The survey has to know about screens

`roadie-catalog` gains a driver for DDC displays, so `roadie devices` includes
the monitor in the one list it promises. The crate is pure, so this is ordinary
tested work.

### 6.6 Key Lights, which are done

Taken next rather than the audio interfaces, because they are HTTP over the
local network and well documented, so a session with no hardware could build
and prove them. `roadie-keylight` is one crate rather than two: unlike
`roadie-ddc`, whose host half is three platform APIs carrying unsafe FFI, a Key
Light's host half is an HTTP client with no platform code at all, so a feature
gate says the same thing at a fraction of the ceremony — the way `roadie-core`
already gates its filesystem reads. The protocol keeps the wasm portability
claim with `net` and `discovery` off.

Three decisions in it are worth keeping:

**They live under `roadie light`, not a command of their own.** The question
someone asks is "what lights do I have", and an answer covering only the ones
on USB would be worse for being confidently incomplete. So a Litra on USB and a
Key Light on Wi-Fi are one list, one selector and one set of verbs.

**Colour temperature is the part to get right.** The light does not take
Kelvin. It takes mireds — reciprocal megakelvin — and they run *backwards*: a
larger number is a warmer light. Everything else on this desk speaks Kelvin, so
the conversion sits at the boundary, rounds to nearest rather than truncating
(which halves the worst-case round-trip drift, 48 K to 24 K), and is tested
against the endpoints Elgato publishes rather than trusted.

**Discovery is not a convenience.** A light's address comes from a DHCP lease
and changes on its own, so an address in a config file is one that will be
wrong one morning. Multicast is how the light stays findable without anyone
maintaining that — and it is also why a guest network or a VPN can hide a light
that is working perfectly.

**The Key Light Neo also speaks over USB, and that path is now built and
hardware-verified (2026-09-03).** It was taken because the desk's Neo could
not be reached any other way: a brand-new light will not join Wi-Fi without
Elgato's app, that app exposes nothing to the accessibility system, and the
whole point of this fork is that the person using it works by ear. USB
control needs no app, no network, and no setup — plug in a data cable and it
answers.

Three things about that path are worth keeping:

- **It is the *same* application protocol as Wi-Fi, in a HID envelope.** The
  Neo answers `GET /elgato/lights` and `PUT /elgato/lights <json>` over USB
  exactly as the network lights do over HTTP; the only new thing is the
  512-byte frame that carries those lines (`roadie-keylight::usb`). So the
  state types, the mired conversion, the clamp logic — all of it — are shared
  unchanged, and the USB half is only framing plus a session. The framing was
  transcribed from the one published reverse engineering of the transport and
  then confirmed byte-for-byte against the light.
- **Two firmware quirks only hardware could have shown.** The Neo answers with
  `0x00` where requests carry a `0x03` "message type" marker, so inbound
  frames are not judged by that byte — validating it refused the actual
  device on the first try. And a write answers with just the `lights` list,
  omitting the `numberOfLights` the same firmware includes on a read, so that
  field now defaults on the way in.
- **On USB power the Neo *refuses* an over-budget brightness rather than
  clamping it.** Asked for 90 percent while capped at 40, it answers
  `{"errors":[{"message":"Invalid parameters"}]}` and changes nothing. The
  network lights clamp; this one rejects. So the driver reads the light's
  `power-info.maximumBrightness` after a refusal and puts the actual ceiling
  in the error — "it allows at most 40 percent; ask for that or less, or move
  it to a stronger power supply" — because a bare "invalid parameters" gives
  someone working by ear nothing to act on. The ceiling depends on the power
  source: about 40 percent on a computer's USB port, full range on a mains
  supply.

`roadie light` lists and drives it alongside the network lights and the USB
Litra — one list, one selector, transport said out loud so the same light on
two paths is two distinguishable entries — and `set_network_light` in the MCP
server does the same. `roadie devices` files it as configurable. The whole
pass ran on the desk's Neo on 2026-09-03: on and off, brightness to its
ceiling, colour temperature across its full 2907–6993 K range, the
over-budget refusal, and the same set again through the MCP server.

### 6.7 Scarlett, the protocol half

`roadie-scarlett` is the wire format and the device tables, with no I/O — the
shape `roadie-ddc` landed in before `roadie-display` followed it. A Scarlett's
audio is standard USB audio class and needs nothing from us; what needs a
protocol is everything around it.

**The licensing position, because it will be asked about.** Focusrite publishes
no specification. The authoritative description is the Linux kernel's
`mixer_scarlett2.c` by Geoffrey D. Bennett, which is GPL-2.0 while this project
is MIT/Apache-2.0. What was taken is facts — opcodes, byte offsets, field
widths, and which model uses which table. A register offset is a fact about a
piece of hardware in the way a pinout is; it is not authored expression. No
code, comment, structure or naming was reproduced. This is the same footing
`roadie-ddc` stands on with `ddcutil`, which is also GPL. Nadir approved this
explicitly. The crate documentation states it in full, deliberately, so that an
auditor finds it in the source rather than in a commit message.

Three things in it are worth keeping:

**The start-up exception nearly ate the sequence check.** Fetching a reply is a
separate transfer from sending the request, so a stale reply is a real, intact
answer to an older question and only the echoed sequence number tells them
apart. Start-up is the exception: the request carrying sequence 1 is answered
with 0. Phrased as a rule about the number, that exception collides *exactly*
with the failure the check exists to catch — every session's second request
carries 1, and the leftover reply from its first carries 0. The first version
had that bug and its own test caught it. Tied to the two commands that only
ever run at start-up, it cannot happen.

**Phantom power is one bit in a shared byte.** Writing the byte outright
switches 48 V off on every other input pair, silently, while the panel shows
only the pair that was asked for. So a bit-sized setting is planned as a
read-modify-write, and `apply_bit` exists as its own function to be tested
without a device.

**The gate is as narrow as the monitor one.** Only switching phantom power
*on* is risky: off is how somebody makes the interface safe again, and a
confirmation in front of the safe direction is an obstacle in the wrong place.
The acknowledgement names the pair it was given for, so agreeing once cannot be
spent on every input.

### 6.8 The Scarlett host layer forks by platform, and Linux is the odd one

Found while planning that layer, before writing any of it, and it changes what
it is. **On Linux the protocol crate is not the way in and should not be used.**

The kernel's `snd-usb-audio` claims the interface, and `mixer_scarlett2.c` is
part of it, so the vendor control endpoint already has an owner. Reaching it
from userspace over raw USB would mean detaching that driver — which stops the
audio, on an audio interface. Worse, the kernel is already doing the work: it
publishes every one of these settings as an ordinary ALSA mixer control, with
names built per model, such as `Line In 1 Phantom Power Switch` and
`Line In 1-2 Phantom Power Switch` where one phantom switch covers a pair.

So the honest shape is:

- **Linux** — read and write ALSA mixer controls. No USB, no protocol crate,
  nothing to reverse. The work is the name mapping per model, which is pure and
  testable, plus an ALSA binding.
- **macOS and Windows** — no kernel driver claims the control interface, so raw
  USB control transfers, which is exactly what `roadie-scarlett` is for.

That is not a loss for the protocol crate: it earns its keep on two of the three
platforms, and it is the only option there. But it does mean "write a USB
backend" is the wrong description of the next step, and that the Linux path is
much smaller and much safer than it looked.

This was **not verified against hardware or against a running kernel** — it is
read from the driver source and from how USB interface claiming works. Worth
confirming with `amixer -c` on a machine with a Scarlett attached before
building on it, which is a one-command check.

### 6.9 The Linux half of Scarlett is names, and they are built

`roadie-scarlett::alsa` turns a model plus an input number into the ALSA
control name the kernel publishes. That is the whole Linux mechanism: no USB,
no packets, no protocol crate — a host reads and writes names.

They look regular and are not, which is why they are generated from per-model
facts rather than formatted from a guess. The number in a name is the input as
a person counts it, from one, but *which* input a control belongs to is a
property of the model. A **Scarlett Solo 4th Gen has one phantom switch and it
is on input two.** A 2i2 3rd Gen has one that covers both inputs, so it is
named for a range — `Line In 1-2` — rather than for an input. An 18i20 3rd Gen
groups four inputs per switch. And the 4th generation turned "air" from a
switch into a choice, which changes the last word of the name.

Getting one wrong fails silently: the name matches no control, and the setting
simply appears not to exist. So there are tests pinning each of those four
shapes by name, plus two that hold the whole table together — every model names
as many controls as it claims to have, and no model names two controls the same
thing. The mutation sweep ran fifteen and killed fifteen.

Still unverified against hardware, and one `amixer -c` with a Scarlett attached
would settle both this and the fork above.

### 6.10 The USB host layer, which is built and hardware-verified

`roadie-focusrite` is the host half — the same split as `roadie-ddc` and
`roadie-display`, and for the same reason: `roadie-scarlett` is on the
*unconditional* wasm portability list, so anything that opens a handle has to
live in a sibling crate or the claim quietly dies. It is `nusb` rather than
hand-rolled IOKit: pure Rust, no libusb, one implementation for all three
hosts, and no `unsafe` anywhere in this crate.

**Section 6.8's central guess is now confirmed on hardware.** A Focusrite
presents several USB interfaces at once, and on 2026-09-03 a Vocaster Two on
this desk showed exactly the predicted shape: interfaces 0 to 2 are USB audio
class and belong to `usbaudiod`; **interface 3 is vendor-specific, class 255,
and has no exclusive owner at all.** Claiming it needs no privileges, nothing
is force-detached, and the audio side never notices — so reading and writing
settings while recording is safe by construction rather than by luck. The
interface number is discovered from the descriptors rather than assumed,
because nothing promises the next model puts it at 3.

Four things the hardware settled that no amount of reading could:

- **The whole protocol layer is right.** The start-up handshake completes, the
  sequence check holds, and the documented start-up quirk is real — the
  request carrying sequence 1 really is answered with sequence 0. Reads return
  sane values, a write lands, reads back, and restores, and the neighbouring
  input is untouched across all of it.
- **The firmware version is at payload offset 8 of the second start-up
  answer.** The Vocaster reported 1749 there while its USB descriptor reported
  `bcdDevice` 1749 — two independent statements of the same number, which is
  what makes the offset believable rather than merely plausible. That number
  chooses the table, so a host that skipped it would silently address the
  oldest layout.
- **This unit's firmware is *older* than its table's threshold** — 1749 against
  the 1769 in `device.rs` — and `table_for` falls back to the oldest table,
  which is the only one the Vocaster has. Everything reads and writes
  correctly on it. Worth naming because the source those thresholds came from
  may mean "minimum supported" rather than "table applies from", and this
  device is evidence the distinction has not bitten yet rather than evidence
  it does not exist.
- **Mass Storage mode does not gate the control channel.** The interface ships
  presenting a small FAT disk of registration files, and this one still is —
  yet every read and every write worked with it on. That is worth recording
  because the Linux driver's behaviour suggests otherwise, and it means
  nobody has to be talked through turning it off before anything works.
  `roadie audio status` says it is on and says plainly that nothing needs
  doing about it.

**The gate in front of 48 volts survived contact with a real front end.** The
CLI refuses `phantom N on`, speaks the sentence, and names the exact command
with `--yes` — options in prose to type back, never a picker. The MCP surface
goes further and **refuses to switch phantom power on at all**, answering with
the sentence to read aloud and the command for a person to type: what it can
damage is at the end of a cable no software can see, and an assistant passing
a confirmation flag is not a person deciding, it is the flag being passed.
Switching it *off* is ungated everywhere, because that is how somebody makes
the interface safe again.

The mock is worth its length. It answers *addresses* rather than scripted
replies and applies a buffered write the way the firmware does — value and
index into the scratch area, then the activation moves it — so a read after a
write returns what the write actually did. Writing it that way immediately
paid: the first version stored the scratch bytes and stopped, every read-back
returned the old value, and the failing test was the mock's shortcut rather
than the code's bug. A fixture that had simply asserted the scratch contents
would have passed and proved nothing.

**The gain is heard, not merely stored — verified 2026-09-04.** Every check
before this one ended at the device's own read-back: write 20, read 20, and
trust that the analog path followed. The pass that closes it was the Key
Light's A/B transposed to ears: with Nadir speaking into the microphone and
monitoring on the Vocaster itself, input 1 dropped from its resting gain of
70 to 20, held twelve seconds, then returned to 70 — each write confirmed by
read-back at the moment it landed. Nadir reported the voice quieter and then
back to full loudness, in that order, which rules out the one failure a
read-back cannot see: a value that lands in the settings store and never
reaches the preamp. The interface stayed in mass storage mode throughout,
consistent with everything above, and ended the test exactly in its
discovered state.

### 6.11 The panel caught up with the desk, 2026-09-04

Until this change the desk panel drew two of the six families the app can
drive. Monitors and Key Lights had wire types, agent handlers and cards;
Stream Decks, Focusrite interfaces, TourBox controllers and VIA boards were
reachable only from the command line and the MCP tools. That is a violation
of the standing three-surface rule rather than a missing feature: a device the
GUI cannot see is a device whose GUI path has never been exercised, and the
project's own record is that the surfaces disagree silently when one of them
is never run.

Protocol v32 appends six on-demand methods for the four families, and the
panel grew a card apiece. Four decisions in it are worth keeping:

- **Gain is two buttons, not a slider.** A slider needs a maximum and these
  interfaces do not report one. A monitor answers every read with its own
  ceiling beside the value, which is exactly what makes the monitor sliders
  honest; a Focusrite answers with a byte. This desk's Vocaster was probed to
  settle it and it stores and reads back **every value to 255** without
  complaint, so any ceiling drawn here would have been invented. Two named
  steps and a number also read aloud unambiguously, where a dragged thumb
  does not.
- **Stream Deck brightness is three actions, not a position.** The protocol
  has a brightness write and no matching read, so a slider would open at a
  number nobody measured. Full, Dim and Off are things you *did*; none of them
  claims to be a thing the deck *is*. `StreamDeckChange` carries the same
  argument, because the wire is where a future caller will look for it.
- **The phantom-power gate crosses the socket as a question, not a token.**
  The GUI sends the change unacknowledged, the agent refuses and returns the
  sentence, and only an explicit second press carries `phantom_acknowledged`.
  The `Acknowledged` proof is still built at the call site from the risk that
  write actually carries — it never rides the wire — so a confirmation for one
  input still cannot authorise the next. Verified on hardware: the refusal
  came back with its words and the interface stayed off.
- **The panel scrolls now.** Six families do not fit any window worth opening,
  and the previous layout had no scroll container — the cards below the fold
  simply could not be reached. Found by screenshotting the running app, which
  is the only thing that would have found it.

TourBox and VIA cards are identity only, and say so. A TourBox stores no
settings; enumerating it is also all the agent does, because *opening* one
writes an unlock and a haptics setup to a device somebody may be using — far
too much to do because a panel was opened. VIA keymap read and write were
deliberately left off the wire: VIA reports no matrix size, so a keymap read
is a blind scan costing a USB round trip per position, which is a different
shape of call and worth designing against a board rather than ahead of one.

**Verified on this desk, 2026-09-04.** All four families answered through the
real agent: Stream Deck XL (32 keys, opened), Vocaster Two (firmware 1749,
inputs at gain 70 and 9), TourBox Elite (14 buttons, 3 wheels), and the
Kiiboom Cybrix 16 speaking VIA protocol 12 with 6 layers. Writes were driven
end to end — gain 70 to 65 and back with read-back, phantom power refused with
the device unchanged, a 101 percent brightness refused before it reached the
deck, and the deck dimmed and restored.

**The press itself is covered by `#[gpui::test]`, not by hand.** Driving the
running window from outside turned out to be a dead end worth writing down:
synthetic `CGEvent` clicks reach the window as *hover* — the button highlights
— and never fire the handler, and `AXPress` reports success and does nothing
either. Neither is a defect, and the proof is that the Refresh button, which
predates all of this, ignores them exactly the same way. What does work is
GPUI's own test harness, which is what `.claude/rules/gui.md` asks for anyway:
two tests drive the real panel through focus and Enter and assert on the
command the agent would have received. Both were mutation-checked rather than
trusted — severing the click handler, and making the gain step a no-op, each
fail their test. The second test pins the safety property no agent-side test
can see: the panel's first press on phantom power carries **no**
acknowledgement, so the refusal is always reached and the warning always gets
read. A toggle that acknowledged on somebody's behalf would switch 48 volts on
with nobody having seen a word about it, and every test on the agent side
would still pass.

Worth knowing for the next sitting: the desk panel's own integration test
serves the mock on the `dev` profile, so a dev agent left running from a
hardware session **takes that socket** and the test then asserts scripted
values against real hardware. It fails loudly rather than silently, but the
failure names the wrong thing — kill the dev agent before reading it as a
regression.

### 6.12 After that — this is the part still to do

The Linux half: an ALSA binding over the names `roadie_scarlett::alsa`
already generates. The USB path here compiles and runs on Linux too and its
failure mode is an error rather than a disturbance, so it is a fallback
rather than a trap — but `snd-usb-audio` publishing these same settings as
ordinary mixer controls remains the better road there, and 6.8's reasoning
about it is still unverified against a running kernel.

Then headsets, MIDI pads, RGB on other brands, and other vendors' mice and
keyboards.

### 6.13 The first Windows sitting, 2026-11-03

The desk moved to a Windows 11 PC (RTX 5090). First native run, not a
cross-compile. What answered with no vendor software involved: both Stream
Decks (XL brightness and key painting verified; presses not yet, see below),
the Key Light Neo on USB (on, off), the TourBox Elite on COM3 (port opens,
handshake runs; no presses yet), the Kiiboom Cybrix 16 over VIA (protocol 12,
keymap reads — only after the report-ID fix in `roadie-hid/src/via.rs`, since
Windows needs byte zero on every output report), and the Vocaster Two (gain
written, read back, restored on input 2).

Four findings worth keeping:

- **The Vocaster needed a GUID, not a driver.** Windows binds Microsoft's
  own WinUSB to interface 3, but the device names no interface GUID, so no
  device interface is ever published and `nusb` cannot open it. Writing
  `DeviceInterfaceGUIDs` into that node's `Device Parameters` and restarting
  only that node fixes it. `roadie audio prepare` does exactly that, behind
  one UAC prompt; the claim error now names that command. Verified end to
  end on this desk: GUID removed, the error named the command, `prepare`
  fixed it, `status` read both inputs.
- **Rust 1.99 broke the gate, and it is fixed.** `double_must_use` on every
  `#[async_trait]` method (async-trait 0.1.92 stops emitting it), a deprecated
  `fetch_update` (`try_update` exists on the 1.98 floor), and the new
  `assert_is_empty` lint across 51 tests. A fresh advisory database also
  failed cargo-deny on two yanked crates and on rustls, webpki-roots and
  tokio advisories, all patched within their semver lines with the gpui pin
  held. The zbus 5 subcrates are ignored with a reason in `deny.toml`:
  gpui_linux keeps zbus 5 in the app either way. Two Windows-only gate
  failures were fixed too: a rustdoc link to a type Windows lacks, and the
  `ci.yml` drift test choking on CRLF checkouts. Full gate on this PC: 8 CI
  jobs passed (shell and the OS-bound test and MSRV jobs skipped by the
  runner; MSRV and `cargo test --workspace` were run by hand and pass).
  Running the runner itself on Windows needs a copy of `xtask.exe` outside
  `target`, or its publish-closure step cannot rebuild itself.
- **Smart App Control blocks every freshly built binary.** It was on here, so
  build scripts, proc-macro DLLs and a rebuilt `roadie.exe` were all refused
  (os error 4551, Code Integrity event 3077). Nadir had it switched off on
  2026-11-03 (`VerifiedAndReputablePolicyState` = 0), after which everything
  built and ran. It is still a release problem: an unsigned
  Windows build will not start for any user who has it on, so Windows
  releases need code signing (SignPath Foundation signs open-source projects
  free).
- **Logi Options+ is installed and running on this PC.** On macOS it is what
  swallowed Stream Deck presses; expect the same here until it is quit.
- **The monitors.** Windows draws only to the LG TV over HDMI, which speaks
  CEC rather than DDC, as on the Mac. The USB-C monitor shows up only as two
  "USB Billboard" devices — alt-mode never negotiated, so no video at all on
  that port: the board's USB4 port carries only the Intel iGPU's video,
  which is disabled, and the 5090 does not feed it. On HDMI to the 5090 the
  HG560T34 answered, and two Windows backend defects showed and are fixed:
  every read-back straight after a write failed with an I²C error although
  the write had landed (the backend now owes the specified 50 ms after a
  write, through `Pacer`), and the monitor was named `display-1` (the EDID
  is now read from the registry via `EnumDisplayDevicesW`). Verified: named
  "RTK HG560T34", brightness 90 and back to 100, each read back.
- **Presses, with Nadir at the desk.** With Logi Options+ gone, Stream Deck
  presses arrive on Windows: the Mini's keys 0, 1 and 5 decoded to the right
  rows and columns, and the XL's top-left key read as row 1, column 1 under
  `streamdeck verify`. The TourBox Elite decoded every control without a
  protocol error: knob, dial and scroll wheel both ways, scroll wheel and
  dial presses, top, tall, short, C1 and C2.
- **Two of a kind now has to be chosen, never guessed.** `open_preferred`
  took the first collection Windows listed, so with a Mini and an XL
  attached `watch` opened the Mini while `verify` in the same minute opened
  the XL. Nadir's rule, stated at the desk: whenever several attached
  devices share a backend, every surface lists them all and lets you
  choose. `roadie-cli/src/choose.rs` is that rule for the command line,
  taken from `roadie audio`, which had it first: `--device` matches part of
  a name, model or serial; one device in reach is used; several are
  refused with each named and a short word to type back ("Mini", not a
  fourteen-character serial read letter by letter). `roadie streamdeck` and
  `roadie via` use it, verified on this desk with `--device xl`. The GUI
  already drew one card per device and MCP already required a `deck`, so
  neither needed changing. Audio, display, light and the Logitech `diag`
  commands each still carry their own matcher; moving them onto `choose`
  is the obvious next step.
- **The Mini shows pictures now, confirmed by photograph.** Gen 1 images
  were refused outright before. Two drivers (python-elgato-streamdeck and
  the Rust elgato-streamdeck crate) agree on its framing and on its panel
  being a *transpose*, where the catalogue said a quarter turn, which would
  have mirrored every key. The first photo, at 80 percent, looked garbled;
  it was glare, and the way to tell was a set of test patterns at 40
  percent — solid red, green and blue, a top half, a left half and a corner
  square — every one exactly where it belonged. The real labels then showed
  two renderer defects the XL had hidden: short words split to be drawn
  bigger ("MU" over "TE"), and text drawn to the edge, where the keycap's
  curve took the tops of letters. A word is now split only when it fits
  whole at no scale, and labels keep a margin of a tenth of the key. Test
  patterns before labels is the order to keep: a pattern says which of
  transfer, orientation and drawing is wrong, and a label cannot.
- **The XL briefly disappears from enumeration** right after another
  process releases it — twice this sitting, each time gone on a retry a
  few seconds later, with no PnP disconnect logged. Not chased yet.
- **Logi Options+ was uninstalled** at Nadir's request, with its plugin
  service. After it was gone, the MX Mechanical and MX Master 3S both
  answered on HID++: battery, DPI 1000 to 1050 and back, SmartShift ratchet
  to free and back, and the backlight read.

### 6.14 The first Linux sitting, 2026-11-03

A headless Ubuntu 26.04 box (`ubuntu-3060`, kernel 7.0), reached from the
Windows PC over Tailscale SSH with a key authorised for it. Rust and the
build tools were installed there; the repository is public, so it clones
with no sign-in. The first native Linux run of anything in this fork.

Attached there: a Logitech Nano receiver `c534` from a cheap combo, with a
keyboard (wpid `4023`, firmware RQK49) and a mouse (wpid `4091`, RQM70).

- **`c534` was missing from the receiver table.** Solaar files it beside
  `c537`, which was present, differing only in holding two devices. With
  the entry, both slots enumerate.
- **The keyboard's features:** battery (`0x1000`, read as 30 percent and
  discharging), special keys (`0x1b00`), Fn inversion (`0x40a0`), layout
  (`0x4520`), no backlight.
- **The mouse's features:** reprogrammable controls (`0x1b04` v4),
  adjustable DPI (`0x2201`), low-res wheel (`0x2130`), and no battery
  feature at all, which is why its battery reads as unknown. DPI is not yet
  exercised: it sleeps within seconds of stopping, and a sleeping device
  cannot be addressed by name.
- **A dozing device is reported as lacking features.** One feature dump in
  four succeeded; the others said the mouse "does not expose" `0x0001`,
  which it had just listed, or decoded an error as `InvalidFunctionId`. A
  sleep, a busy reply, and a missing feature must not share one sentence.
  Not fixed yet.
- **`list` says "Unknown device" where `diag` reads "Wireless Mouse".**
  Not fixed yet.
- Fixed: `doctor` advised installed rules again, a Logitech line the rules
  already carry, and opening an ASUS RGB controller; it now counts only
  drivable nodes and explains remote sessions. `list` printed macOS notes,
  a stale Bolt-only claim, and a dozen retry warnings. `devices` claimed
  the family's full feature range for every Logitech device and monitor.
- **The TourBox had no udev rule** for its serial node; it has one now,
  scoped to `c251:2005`.
- **The whole desk on Linux, 2026-11-04.** Moved over by a USB switch:
  both Stream Decks (brightness, a key painted), the Cybrix 16 (VIA 12,
  keymap read), the TourBox (found at `/dev/ttyACM0`), the Key Light Neo
  (on, off), and the MX Mechanical and MX Master 3S over Bolt (battery,
  DPI 1000 to 1050 and back, SmartShift round trip, backlight read).
- **The Vocaster works on Linux through the USB path.** Section 6.8 expected
  `snd-usb-audio` to own the vendor interface; on kernel 7.0 interface 3 had
  no driver at all, so it claimed cleanly and input 2's gain went 9, 10, 9
  with read-back while the audio interfaces stayed with `snd-usb-audio`. The
  ALSA route may still be the better one; it is no longer the only one.
- **NVIDIA's driver links no connector to its DDC bus.** It creates an I²C
  adapter per output (`NVIDIA i2c adapter N`) and publishes no `ddc` link,
  so every monitor was named and none reachable. The backend now falls back
  to reading the EDID off each adapter whose parent is the connector's own
  graphics card, never an SMBus, and keeps the one matching the kernel's
  block. The HG560T34 over HDMI to an RTX 3060: found on `/dev/i2c-8`,
  brightness 90 and back, capabilities read. Without the `i2c` group it now
  says that, rather than that no line exists.
- **`uaccess` grants nothing over SSH**, which is the normal way into a
  headless box. Device checks there ran under `sudo`; `doctor` says so.
- Native on Ubuntu: fmt, the CI Linux test set, non-GUI clippy and the
  shell job all pass.

### 6.15 The screen-reader pass, 2026-11-04

The GUI read through the accessibility tree the way NVDA, Narrator and Orca
do: UI Automation on Windows, AT-SPI on Linux (WSL2 Ubuntu with WSLg, an
AT-SPI bus started by hand, and the mock agent under `ROADIE_PROFILE=dev`).
What was learned, so nobody relearns it:

- **GPUI exposes an element only with both an id and a role.** A plain
  string child is drawn and never read; `gpui::text!` or `Text::new` makes it
  readable, `Text::new_inaccessible` draws it without announcing it twice.
  `ui/a11y.rs` holds the helpers: headings with levels, labels, named
  groups, a zero-size spoken name for icon-only buttons, and two sliders
  that take focus, keys and assistive-technology steps.
- **`aria_selected` on a button removes its Invoke pattern** in AccessKit,
  so a selected card could not be pressed through UI Automation. Selection
  on a button is `.toggled()`; the library button's `.selected()` is visual
  only and says nothing to a screen reader unless `.toggled()` goes with it.
- **GPUI has no disabled state for assistive technology.** A disabled
  control simply drops out of the Tab order, so "unavailable" travels in its
  name ("Invert scroll direction, unavailable").
- **gpui-component's own pieces are partly unreachable**, and are worked
  around here rather than patched upstream. The settings sidebar's entries
  have no role and take no focus, so the Settings window draws its own page
  tabs and hides the library sidebar with `display: none` (zero width alone
  left its search field in the Tab order; the search is gone with it). Its
  `SettingField::switch` has no name, so every settings switch is
  `settings::switch_item`. Setting titles and descriptions are drawn as
  text nothing reads, so each control carries its own name and description.
  The library `Select` cannot be named, so each sits in a named group.
- **The update-consent dialog** is a dialog with its question as its name
  and its terms as its description.
- **A fresh install locked its whole window behind a configuration error.**
  The GUI writes its first config before its first connect, and a reload
  with no agent was reported as refused. It is now owed to the next
  connection (`services/ipc.rs`, `undeliverable`); an agent that never comes
  is still reported by the unreachable frame.
- **High contrast.** `themes/high-contrast.json` is a black-and-white pair
  with yellow (dark) or deep blue (light) for selection. It replaces the
  chosen themes when Settings, Appearance, High contrast is On, or when the
  OS asks for it: Windows contrast themes (`SPI_GETHIGHCONTRAST`), GNOME's
  high-contrast switch or a `HighContrast` GTK theme, macOS Increase
  Contrast. `theme::accent` follows it, so selection and focus outlines
  take the pair's highlight instead of the brand blue.
- **Text size.** The interface scale multiplies by Windows' "Make text
  bigger" and GNOME's text scaling factor, read once at launch.
- **Settings search** is the app's own, above the page tabs: it narrows the
  tabs to pages whose title or settings match. Each page's setting titles
  are listed in `settings_pages`, beside the pages; keep them in step.

**Orca, driven from the keyboard.** Orca runs in the WSL Ubuntu with its
debug log on, the app under X11 (`WAYLAND_DISPLAY` unset) so `xdotool` can
type into it, and the mock agent. Everything Orca would have said is in the
log as `SPEECH OUTPUT`. That found what tree audits could not:

- **Focus was lost on every page change.** Opening a device, or leaving one,
  removed the focused control; Orca went silent and Tab had nowhere to start.
  The main window now moves focus to the new page's title (`PageFocus`), and
  the desk focuses its heading when it opens.
- **A GPUI focus handle is not a Tab stop unless it says so.** Every slider
  in `ui/a11y.rs` was focusable and never reached by Tab.
- **Remapping a button** now moves focus from the button to its current
  action, and back there when the action list closes.
- **Still open, in gpui-component:** its text input names a frame node but
  focuses an inner element with no node, so Tab into any search or text
  field announces the window around it. Its `Select` exposes no name and no
  value; each sits in a group named for its setting and current choice.

The T3 Code app on this PC brings no Linux of its own: its `wsl-runtime` is
its server, installed into WSL. WSL with WSLg is the Linux to test in.

Windows audits and the Orca run both came back clean on the home window,
the desk, the mouse tabs, Settings and Add Device. Not yet tried with a
person on NVDA or Narrator, nor watched with Magnifier following focus.

## 7. What a session can do with nobody at the desk

Nadir travels, and a session that stalls waiting for a monitor to be plugged in
wastes the time. The split is sharper than it looks, so it is written down.

Buildable, testable and gate-green with no hardware and no computer in front of
anyone:

- Everything in 6.1 through 6.5, all of which is now done. The mock backend is
  what made it true: the CLI, the MCP tools and the survey were all driven
  against a scripted display.
- The macOS FFI, which turned out to be type-checkable here — see section 4.
  CI's two macOS jobs still compile it for real on a PR, and neither is a
  substitute for running it.
- The Windows backend, cross-compiled locally against `x86_64-pc-windows-gnu`
  and by the `clippy (windows)` job.
- A mutation sweep over whatever lands. The fork's record is that every gap
  found this way was in code that had prose about it and no test.
- The `docs/VERIFYING.md` steps for monitors, written ahead of the hardware so
  the first sitting is one ordered pass rather than an exploration.

Needed a panel answering, and the 2026-09-02 sitting settled most of it:

- The reply checksum seed: verified against the RTK HG560T34 — replies come
  back, parse, and pass the checksum.
- The timing floors held on one real panel with the default spacing; one
  panel is a data point, not a distribution.
- Which input-source values a given monitor accepts. Above `0x12` is vendor
  territory and USB-C has no standard number at all — and switching inputs
  on an unattended desk was deliberately not attempted.
- Whether each screen on the desk speaks DDC: answered for this desk. The LG
  TV does not (CEC instead), the HG560T34 does.

## 8. Brand

The logo is Nadir's own. `design/LICENSE` reserves it to him and states
explicitly that none of it derives from OpenLogi — upstream's icon set is
AprilNEA's and was deleted rather than renamed.

Three assets are cut from the single supplied file: the knob (app icon), the
letter R (alternate icon), and the full lockup (README headers, six
languages). Both marks are cut with circular masks rather than rectangles,
because the knob and the R touch in the source. The menu-bar glyphs are the
exception — drawn as geometry, since a photographic knob turns to porridge at
18 points and macOS flattens a template icon to one colour anyway.

`AppIcon::Prism` is now `AppIcon::Letter`, with a serde alias so configs
written before the change still parse. That alias is a schema contract, the
same as the `openlogi` alias on the default variant. Do not remove either.
