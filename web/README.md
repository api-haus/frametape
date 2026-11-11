# Browser producers

Load `recorder.js`, `input.js` and `bundle.js` as classic scripts, or import them for their
global APIs. They have no package dependencies. Each producer creates independent instances;
input, video and bundle assembly can be selected separately.

## Video

Host supplies the actual WebGPU canvas texture after submitting the commands that draw it.
Logical frame indices must increase. Nanosecond timestamps/durations belong to the host's
frame stream; encoded video uses 60 fps and returns an explicit logical-frame/picture map.

```javascript
const video = FrametapeVideo.create({
  canvas: { context },
  note(text) { console.log(text); },
}, { maxWidth: 0, bitrate: 12_000_000 });
video.begin();
video.frame(0);
device.queue.submit([commands]);
video.drawn({ context, texture });
const encoded = await video.end();
// encoded.bytes is Annex B H.264; encoded.mapping binds source_frame to video_frame.
```

Call `frame(index)` once per logical frame, before its matching `drawn`. A frame without
a new picture repeats its previous picture when possible. Readbacks and encoder queues are
bounded. Mapping statuses distinguish missing/repeated pictures and encoder pressure. Returned
picture indices correspond to confirmed encoder output, including failure cases.

Default width cap is 1280 px, preserving the original onboard browser recorder. `maxWidth: 0`
keeps full source resolution. Browser H.264 is lossy. Hardware selection defaults to browser
choice; `hardwareAcceleration` can request a specific preference, which may be unsupported.
The tested Linux NVIDIA browser supports H.264 with `no-preference` and `prefer-software`,
while `prefer-hardware` reports unsupported. Metadata must not claim hardware encoding merely
because a preference was requested.

## Input

```javascript
const input = FrametapeInput.attach({
  target: window,
  enabled: () => recording,
  publish(data, sourceTimestampMs) {
    events.push({ frame: currentFrame, timestamp_ns: clockNs(), channel: 'input', data });
  },
});
input.pollGamepads(); // Host calls once per frame while recording.
input.dispose();     // Removes every owned listener.
```

Keyboard, pointer/mouse/wheel, touch, text/composition, focus and gamepad families can each
be disabled with their named boolean options. DOM events retain raw fields; gamepads publish
when their state changes. `publish` receives the browser event's timestamp separately so the
host can map it to its own monotonic epoch. Input passes through unchanged to the application.

## One recording file

```javascript
for (const map of encoded.mapping) {
  const frame = frames[map.source_frame];
  frame.video_frame = map.status === 'captured' ? map.video_frame : null;
  events.push({ frame: frame.index, timestamp_ns: frame.timestamp_ns,
    channel: 'video.frame', data: map });
}
const tape = await FrametapeBundle.create({
  metadata: { host: 'My WebGPU app', video_fps: 60 },
  frames,
  events,
  attachments: [
    { channel: 'video', name: 'video.h264', media_type: 'video/h264', data: encoded.bytes },
    { channel: 'physics', name: 'simulation.b3rec', media_type: 'application/x-box3d-recording', data: physicsBytes },
  ],
});
FrametapeBundle.download(tape, 'issue.ftape');
```

Frames start at index zero, are contiguous, and carry `timestamp_ns`, `duration_ns` and
`video_frame` (`null` when no image exists). Events carry `frame` (`null` for independent
sampling), `timestamp_ns`, `channel` and producer-owned JSON `data`. Attachments preserve the
original bytes. Bundle writer emits the same manifest/streams/ZIP format as Rust; it rejects
unsafe paths, duplicate members, invalid clocks and recordings over its 4 GiB browser ZIP limit.
Caller retains supplied frames/events/attachments if assembly fails.

Real Chrome QA recorded button clicks, typed text and a color change through WebGPU/WebCodecs:
16 logical frames, 16 confirmed video pictures, 159 events through shared input producer. Native FFmpeg decoded first/last
pictures; full-resolution text and explicit frame mappings were inspected. This verifies the
producer and container, without claiming engine/gameplay integration from that fixture alone.
