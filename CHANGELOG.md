# Changelog

## 0.23.8

This entry covers the changes on `main` since the [0.23.1 tag](https://github.com/DDynamic-Evolution/RustDJ/releases/tag/v0.23.1).

### Platform and interface

- Added Linux support with X11 and Wayland, a desktop launcher and a Debian package build script. Linux stores preferences and library data under XDG directories and can reuse the earlier library location.
- Added saved two- and four-deck layouts. The waveform lanes, mixer channels and track-loading buttons follow the selected layout.
- Made the main console more compact and added a **Library** view that expands the track list during playback.

### Mixing and track preparation

- Added two configurable external audio inputs and a separate microphone channel. The mixer includes a mic toggle, level control and meter.
- Added **Talkover**, four file-backed **Jingles** pads with MIDI Learn, and a stereo master limiter with a −1 dBFS sample-peak ceiling and gain-reduction readout.
- Improved tempo and beat alignment with separate bass, midrange and treble analysis. Older analysis entries refresh when tracks are loaded or analyzed.
- Added a **Beatgrid editor** that saves the tempo and first beat per file. Users can restore the detected grid from the editor. Grids use a fixed tempo per track.
- Added persistent **Set history** with local-time display and CSV export.

### Streaming and recording

- Added direct start and stop controls for SHOUTcast and MP3 recording, with independent status and elapsed-time displays.
- Added named stream profiles and per-profile password storage through the Linux system keyring or Windows DPAPI. The previous stream configuration becomes the **Default** profile.
- Added optional reconnect for dropped live streams. The application reports retry status and keeps local recording independent.

## 0.23.1

- Smoothed jogwheel motion during playback and paused cueing.
- Improved scrolling waveform rendering and synchronization with playback.

## 0.23.0

- Added a native four-deck Pioneer DDJ-SX profile with mixer, transport, hotcue, loop and effects mappings.

## 0.22.0

- Added RustDJ branding, a window icon and embedded Windows executable resources.

## 0.21.0

- Added model-specific Pioneer, Roland and Numark MIDI controller profiles.

## 0.20.0

- Added certificate-verified TLS for SHOUTcast source connections and metadata updates. Legacy unencrypted transport requires explicit selection.

## 0.19.0

- Switched the interface and README to English.
