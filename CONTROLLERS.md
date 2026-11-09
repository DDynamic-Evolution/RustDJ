# Controller profiles in RustDJ 0.23

Select a model in **Controller / MIDI**. These are independent USB MIDI presets
for the listed models; brand names do not imply support for all products from
that manufacturer. Automated MIDI message tests passed. Physical testing of
each additional controller is still required.

| Model | Deck banks | Included controls | Limitations |
| --- | --- | --- | --- |
| Pioneer DDJ-400 | 1/2 | Transport, jog/seek, 14-bit tempo/gain/EQ/faders, filter, master/headphones/mix, browser/load, eight hotcues and reported beat-loop pad messages | Shared Beat FX is not assigned; proprietary features are not emulated |
| Pioneer DDJ-FLX4 | 1/2 | Same common MIDI control subset, including master/headphone levels | USB MIDI only; Smart CFX/Fader, Bluetooth, proprietary features and shared Beat FX are not implemented |
| Pioneer DDJ-SX | 1/3 and 2/4, four native channels | Transport/Cue/Sync, jog/seek, 14-bit tempo/trim/EQ/faders/filters/crossfader, four LOAD buttons, browser, eight hotcues and loop pads, banked FX, transport and pad LEDs | Original DDJ-SX only; use other-software MIDI mode. ROLL pads trigger normal beat loops; hardware output controls, FX assignment switches, slip/scratch, slicer and sampler are not mapped |
| Roland DJ-202 | 1/3 and 2/4 | Existing transport, mixer, filter/FX, browser, eight hotcue/loop pads and LED feedback | Hardware master level is not a software CC; TR-S is not emulated |
| Roland DJ-505 | 1/3 and 2/4 | Transport, 14-bit mixer/tempo, filter/FX, browser/load, mode-dependent hotcue/loop pads and basic LEDs | Select HOT CUE or LOOP first; TR-S, external line/phono mixer paths and hardware effects are not implemented; hardware output level controls remain hardware controls |
| Numark Mixtrack (Pro) 3 | 1/2 | Transport, 14-bit tempo with inverted direction, jog, 7-bit EQ/filter/faders, master/headphones/mix, browser/load, FX buttons, four hotcues and four direct loops | No physical trim knob; four loop pads use 1/2/4/8 beats; use SHIFT+hotcue to clear; touch strip/sampler are not assigned |
| Numark Mixtrack Platinum FX | Four native deck channels | Transport, inverted 14-bit tempo, 7-bit mixer/filter/faders, headphones/mix, browser/load, mode-dependent eight hotcue/loop pads, loop controls and basic LEDs | Select HOT CUE or AUTO LOOP first; jog displays, fader cuts and effect paddles are not implemented |
| Generic MIDI / Learn | Explicitly learned targets | Any supported Note/CC bindings assigned with MIDI Learn or RustDJ JSON | No guessed addresses, initialization, manufacturer bank routing or LEDs |

Two-deck presets operate decks 1/2. Decks 3/4 remain available through the GUI,
keyboard and additional learned bindings. Jog mappings provide relative
seek/nudge behavior; they do not claim vinyl-style scratch emulation.

## Pioneer DDJ-SX setup

This preset targets the original **DDJ-SX**, not the SX2 or SX3. Set the
controller's utilities to use **DJ software other than Serato**, as described
in [Pioneer's setup instructions](https://pioneer-support.zendesk.com/hc/en-us/articles/23923285968537-DDJ-SX-MIDI-compatible-software).
Then select **Automatic (known models)** or **Pioneer DDJ-SX** in **Controller / MIDI**.
The preset does not change the hardware utility setting automatically.

- The deck selectors address decks 1/3 on the left and 2/4 on the right.
  Each of the four mixer channels always controls its matching deck number.
- **HOT CUE** provides eight hotcues; **SHIFT + pad** clears the cue.
- **ROLL** pads trigger RustDJ's normal, latching beat loops, in the same
  order as the GUI: **1/4, 1/2, 1, 2, 4, 8, 16, 32 beats**. These are not
  momentary slip rolls. GROUP2 pad messages use the same loop assignments.
- **AUTO LOOP** toggles the current beat-loop length. **SHIFT + AUTO LOOP**
  recalls a loop; IN/OUT, half/double and **SHIFT + OUT** (exit) are supported.
- The three buttons in each FX section toggle RustDJ's **Flanger, Reverb,
  Echo**. Its **first FX knob** controls the common FX level for the selected
  left/right deck. Another channel's EQ, fader or headphone CUE does not change
  this FX target. FX assignment buttons and the other FX knobs are unassigned.
- Master/headphone level and cue/master mix have no default software bindings
  in this preset. Use RustDJ's visible output controls and configure the audio
  device/channel pairs separately. The profile does not add ASIO support.

Transport and hotcue/loop pad LED messages are included. Jog displays, meters,
advanced pad animations and physical hardware operation have not been verified.
Existing learned mappings for other controllers remain separate.

## Profiles and persistence

- **Automatic (known models)** detects model names in MIDI port names. A saved
  preferred port is used by exact name; an unavailable preferred port is not
  replaced with an unrelated device. Multiple matching devices require selection.
- A model's current bindings are saved before switching. Old settings seed the
  selected/previously preferred model bank so existing custom assignments are
  preserved. Without model information, the existing Roland DJ-202 bank is used.
- Mode changes and bank handling are model-specific. Pioneer pad channels are
  not interpreted as Roland mode messages. Unselected/unsupported DJ-505 and
  Platinum FX pad modes do not trigger hotcues accidentally.
- Held transport Cue is released on reset, profile changes and disconnection.
- LED output uses the matching controller output port, or one unambiguous
  model-matching output. DJ-202 retains its existing pad/bank feedback; other
  presets provide basic Play/Sync/channel-Cue LEDs. Pad colors and jog displays
  are not promised for the new models.
- The new absolute inverted input modes support controllers with reversed
  faders. Existing MIDI-learn choices remain available.

## Import and export

**Export JSON …** writes schema version 1, a model identifier and its current
bindings. **Import JSON …** validates the entire file before changing anything.
Files are limited to 1 MiB / 4096 bindings. Note and CC addresses, 14-bit pairs,
deck/effect/pad ranges and address overlap are checked. Importing a model profile
selects that model; use Generic for a custom controller without device-specific
behavior. Controller-profile settings and exported mappings contain no SHOUTcast credentials.

## Mapping references

The Rust tables are independently implemented from published MIDI addresses;
no upstream JavaScript or XML mapping implementation is included.

- [Pioneer DDJ-400 MIDI message list](https://www.pioneerdj.com/-/media/pioneerdj/software-info/controller/ddj-400/ddj-400_midi_message_list_e1.pdf)
- [Pioneer DDJ-FLX4 MIDI message list](https://www.pioneerdj.com/-/media/pioneerdj/software-info/controller/ddj-flx4/ddj-flx4_midi_message_list_e1.pdf)
- [Pioneer DDJ-SX MIDI message list](https://www.pioneerdj.com/-/media/pioneerdj/software-info/controller/ddj-sx/ddj-sx_list_of_midi_messages_e.pdf)
- [DDJ-SX address cross-check in the Mixxx controller project](https://github.com/mixxxdj/mixxx/blob/main/res/controllers/Pioneer%20DDJ-SX.midi.xml)
- [Roland DJ-202 mapping reference](https://github.com/Lykos153/mixxx/tree/Mapping-DJ-202/res/controllers)
- [Roland DJ-505 MIDI reference](https://github.com/mixxxdj/mixxx/blob/main/res/controllers/Roland_DJ-505.midi.xml)
- [Numark Mixtrack (Pro) 3 MIDI reference](https://github.com/mixxxdj/mixxx/blob/main/res/controllers/Numark-Mixtrack-3.midi.xml)
- [Numark Mixtrack Platinum FX MIDI reference](https://github.com/mixxxdj/mixxx/blob/main/res/controllers/Numark%20Mixtrack%20Platinum%20FX.midi.xml)

The Pioneer references distinguish MSB/LSB pairs from jog/browse relative CCs.
The Roland/Numark references distinguish native bank channels and pad modes.
Hardware-only controls and unverified proprietary protocols are left out rather
than assigned invented addresses. Configure the interface's master/headphone
channels separately; a MIDI profile does not install an audio driver.
