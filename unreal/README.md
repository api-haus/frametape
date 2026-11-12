# Unreal and C++ integration

Requires Rust/Cargo, an Unreal C++ toolchain, and FFmpeg at runtime. Linux source build of Unreal 5.8.3 was verified. Build Rust ABI and install plugin sources into a project:

```bash
./unreal/install /absolute/path/to/Project
```

Enable `Frametape` in the project's `.uproject` and add `Frametape` to its module dependencies.
Plugin defaults to disabled. Each `FFrametapeRecorder` owns one session; callers may create
independent recorders. The installer builds before copying artifacts. If `processqueue` is installed, builds use its CPU admission queue. It supports Linux and
macOS; Windows users must build `frametape-ffi` for their target and place its static library
under `Plugins/Frametape/Source/ThirdParty/lib/Win64/frametape_ffi.lib`.

```cpp
#include "FrametapeRecorder.h"

FFrametapeRecorder Recorder;
Recorder.Start(TEXT("/recordings/issue.ftape"), TEXT("{\"host\":\"MyProject\"}"));
// Once per rendered frame, beginning at index zero; durations use nanoseconds.
Recorder.Frame(0, 16666667);
Recorder.Event(TEXT("profile.cpu"), TEXT("{\"scope\":\"physics\",\"duration_ns\":1250000}"));
Recorder.Event(TEXT("physics.box3d.step"), TEXT("{\"physics_step\":0,\"render_frame\":0}"));
Recorder.Attach(TEXT("physics.box3d"), TEXT("/recordings/simulation.b3rec"), TEXT("application/x-box3d-recording"));
if (!Recorder.Stop()) UE_LOG(LogTemp, Error, TEXT("%s"), *Recorder.Error());
```

Check every return value in production. `Stop` drains readbacks and encoding before closing
archive. Successful encoder close emits final `captured` mappings; queued pictures stay unverified after encoding failure. Engine pre-exit closes active recordings before Slate/render teardown. Errors retain journal/original evidence; an existing failed clip is attached as `video.failed` without marking any picture captured. Video queues never block a frame; events
record captured, repeated, busy and unavailable pictures explicitly. Video uses shared Rust
encoder selection. Backbuffer RGB10 is converted to RGB8 before encoding. Video uses fixed
60 fps unless configured; logical frame timing remains separate in nanoseconds.

`FFrametapeOptions` selects input/video independently, frame rate and window. Supply `Window`
for an explicit viewport; otherwise recorder selects game window or active editor window.
Input captures Slate keyboard, analog, mouse, wheel, touch, gesture and motion events delivered through those
paths. It does not claim OS-global input capture; use desktop recorder for that. Explicit
custom events and attachments work with both video and input disabled.

`Example/FrametapeExample.uproject` demonstrates registration in another host. Install plugin
into that directory, build its editor target, then use `Frametape.Start [archive.ftape]` and
`Frametape.Stop` console commands. Example owns no game assets. Its bounded video smoke
mode waits for a game viewport, captures eight pictures, finalizes them, and exits with a
success/failure process status. `-FrametapeVideoSmoke=OUTPUT.ftape` selects it;
`-FrametapeVideoFps=30` selects the encoded frame rate. Input is explicitly disabled for
this capture mode; normal recordings retain their independent input option.

## Box3D

Box3D's recording API owns simulation snapshots, geometry and mutation streams. Call
`b3World_StartRecording` at a step boundary and `b3World_StopRecording` before attaching
`b3Recording_GetData` / `b3Recording_GetSize` bytes or a `.b3rec` file. Record each physics-step
and render-frame relationship separately; multiple steps can share one rendered frame.

`examples/box3d.cpp` records two physics steps per render frame through public Box3D and
Frametape C APIs. Supply Box3D headers/library from your project; none are vendored here.
`include/frametape.h` is the engine-independent C interface usable without Unreal.

Rust hosts can attach their native recording and timeline directly:

```bash
cargo run --manifest-path unreal/examples/Cargo.toml -- simulation.b3rec timeline.json issue.ftape
```

`timeline.json` contains `frames` and `events` arrays following public `Frame` and `Event`
contracts. Each physics event uses channel `physics.box3d.step`, a render frame index,
monotonic timestamp in nanoseconds and data such as `{"physics_step":42,"render_frame":21}`.
Original Box3D bytes remain unchanged inside archive.
## Foreground recording control

`FFrametapeControl` registers an opt-in Linux process endpoint for `frametape record` and the
global shortcut. Supply start, stop and busy callbacks on the game thread. Start receives
the archive path and selected window title; reject a different capture target. Stop must drain
and publish that exact archive before returning success. The example module demonstrates
registration with `FFrametapeRecorder`; game adapters can retain their existing state/profiler
producers. No HTTP listener or engine-global recorder is installed automatically.
