# Frametape

Record a bug once. Send one `.ftape` file containing video, input, profiling,
physics, custom events and annotations, connected by explicit frame IDs and timestamps.

MIT libraries and local tools for Rust, C/C++, Unreal and browser hosts.

```bash
cargo install --git https://github.com/api-haus/frametape --locked frametape-cli
frametape record issue.ftape --output DP-1
frametape app recordings/
frametape inspect issue.ftape --frame 42
frametape export issue.ftape issue-frames/
```

Install FFmpeg/FFprobe separately. Desktop capture currently requires Linux with a
wlroots compositor supporting screencopy v3 and read access to selected `/dev/input/event*`
devices. `--input /dev/input/eventN` selects devices; the default follows all devices,
including hotplug. `--no-input` selects video alone. No device is grabbed exclusively.

`--encoder hevc_nvenc`, `--encoder h264_amf` or `--encoder hevc_vaapi
--vaapi-device /dev/dri/renderD128` pins an encoder. Automatic selection actually encodes
a probe frame; an explicit encoder failure never silently selects another. Readable mode
prefers NVIDIA 4:4:4 HEVC/H.264, then AV1 and AMD AMF/VAAPI. Actual codec, chroma and hardware
status are recorded. CPU encoding remains available when hardware cannot encode.
`--lossless` preserves RGB samples through software `libx264rgb`; hardware RGB-to-YUV
conversion is not advertised as pixel-lossless. Alpha is excluded.

The capture backend currently transfers compositor SHM buffers through a bounded encoder
queue. Requested frame rate is a limit, not a delivery guarantee. Missing pictures have
explicit null mappings; input-overrun events and encoder failures remain visible.
NVIDIA has runtime verification; AMD requires verification on an AMD device.

## Libraries

| Package | Responsibility |
| --- | --- |
| `frametape` | Versioned bundle, channels, source frames, annotations, legacy import |
| `frametape-video` | Bounded asynchronous FFmpeg encoder, codec negotiation, resize concatenation |
| `frametape-desktop` | Independently usable Linux Wayland and evdev producers |
| `frametape-ffi` | Per-instance C ABI, static/shared libraries |
| `frametape-cli` | Recorder, catalog/editor, extraction and agent packets |

```toml
[dependencies]
frametape = { git = "https://github.com/api-haus/frametape" }
```

```rust
use frametape::{Channel, Event, Frame, TapeWriter};
use serde_json::json;

fn main() -> std::io::Result<()> {
let mut take = TapeWriter::create("issue.recording", json!({"host":"my-app"}))?;
take.register(Channel {
    id: "physics.step".into(), kind: "physics".into(), schema: "my-app:physics:v1".into(),
})?;
take.frame(&Frame { index: 0, timestamp_ns: 0, duration_ns: 16_666_667, video_frame: None })?;
take.event(&Event {
    frame: Some(0), timestamp_ns: 0, channel: "physics.step".into(),
    data: json!({"step":42,"contacts":7}),
})?;
take.attach("physics.step", "simulation.b3rec", "simulation.b3rec", "application/octet-stream")?;
take.finish("issue.ftape")?;
Ok(())
}
```

Producers own payload schemas and sampling. Register only the channels needed; no engine,
UI or encoder dependency is required by the bundle library. Independent samples can leave
`Event.frame` null and resolve it through `frame_at` using timestamps.

[Unreal installation and Box3D examples](unreal/README.md), [browser integration](web/README.md)
and the [C header](include/frametape.h) cover other hosts.

## Review and agent inspection

The local app catalogs a folder with previews, search and sorting. Open a take, step or
scrub source frames, draw boxes/ellipses/arrows/brush strokes/text, move or erase marks,
edit inclusive ranges, undo/redo and save. Paused views decode original pixels;
playback uses a labeled compatible preview paced by the source clock. Annotations live
inside the take. Concurrent editor changes are rejected rather than overwritten.

```bash
frametape members issue.ftape
frametape inspect issue.ftape --frames
frametape inspect issue.ftape --channel input.evdev
frametape inspect issue.ftape --annotations
frametape extract issue.ftape attachments/simulation.b3rec simulation.b3rec
frametape export issue.ftape packet/ --frames 10,42,80
```

Export selects annotated first/middle/last frames by default, or explicit frames/a cadence
with `--every N`. PNGs contain burned-in annotations; `manifest.json` binds them to channel
events and source-frame timing. `--list` exports data without decoding images.

## Existing recordings

```bash
frametape pack take.jsonl
frametape pack take.brtake --data take.data.json
frametape recover interrupted.ftape.recording recovered.ftape
```

Raycat imports original entity JSONL, video/mapping, profiler scopes and annotations; video
annotation ranges are converted to source-frame ranges. BallRPG imports its original
game take and producer-exported frame JSON. Its private game codec stays in its project.
Box3D simulation recordings remain original attachments with explicit physics-step/render
mapping events. Game-state replay and physics resimulation remain host responsibilities.

Completed journals are removed only after atomic, non-overwriting publication. Failures
retain journals; recovery keeps damaged trailing records as evidence and rejects interior
corruption. Readers check member paths, duplicates, sizes, schema version and timeline
references. Standard ZIP/JSON/JSONL and original media remain readable without this tool.

## Development

```bash
cargo test --workspace --locked
cargo build --release --workspace --locked
```

On the development workstation, prefix heavy commands with `processqueue cpu`; render
checks use `processqueue render processqueue gpu`. No hosted service, account or telemetry
is required by the tools. See [format](docs/format.md) and [delivery evidence](docs/delivery.md).

Annotation and bounded encoder code originated in Raycat Engine; Unreal/WebGPU capture and
review UI originated in the owner's BallRPG project. Extracted first-party code is MIT;
no private engine, game assets or third-party Box3D implementation is vendored.
