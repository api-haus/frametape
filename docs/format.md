# Recording format v1

`.ftape` is a ZIP archive. JSON streams use Deflate; already-compressed media and native
profiler/physics artifacts are stored without recompression. ZIP CRCs protect member reads.

| Member | Contract |
| --- | --- |
| `manifest.json` | `format:"frametape"`, `version:1`, recording ID, creation UTC ns, host metadata, channel/attachment descriptors, counts, duration ns |
| `frames.jsonl` | Contiguous zero-based `index`, relative `timestamp_ns`, `duration_ns`, nullable `video_frame` |
| `events.jsonl` | Channel ID, relative `timestamp_ns`, nullable source `frame`, producer-owned `data` |
| `annotations.json` | Optional version-2 inclusive source-frame containers and normalized marks |
| `attachments/*` | Producer-native bytes, declared channel, media type and byte size |

Source frames and encoded pictures have different identities. Null means no fresh picture
for that source frame. Repeated pictures use producer `video.frame` events with
`source_frame`, `video_frame`, and explicit `status`; consumers may display them as repeats.
Never derive source frame from video seconds. Source timing may be variable while video
is CFR; the review playhead follows source timestamps.

The shared reader resolves final `video.frame` records into its returned frames. Only
`captured` confirms a fresh picture; `queued`, `repeated`, busy and unavailable states do
not. Original records remain available in the event stream. `visit_events` streams long
channels without retaining every payload in memory.

Desktop evdev event timestamps use `EVIOCSCLOCKID(CLOCK_MONOTONIC)`. Compositor ready
timestamps are checked against monotonic request/receive bounds; incompatible epochs fail.
The capture channel records the absolute epoch ns and actual encoding settings. No claimed
sub-frame precision exceeds the capture/input APIs. Input SYN_DROPPED is retained and counted.

Each channel has a unique nonempty ID, kind and schema URI/media type. Each producer owns
its payload, clock conversion and lifetime. Independent producers merge by timestamp;
explicit source frame wins association when supplied. A source may start before the first
sampled image or outlive the last; such events remain unbound rather than gaining invented
pictures. Multiple physics steps may share one source frame.

Writer publication refuses existing output and uses a sibling temporary file plus an atomic
hard link. Successful publication removes known journal members only. Annotation writes
copy original members without re-encoding and atomically replace the archive; failed writes
preserve original evidence. Local editor requests include a revision and reject stale edits.
`edit_annotations` acquires the stable sibling `.annotations.lock` before reading the
canonical document and invoking the caller's checked edit. Native and standalone editors
share this transaction. Existing file permissions survive replacement.

Recovery leaves the original journal intact. Pending desktop input is merged into the
recovered event stream; unfinished video and accepted mappings remain original attachments
marked unverified. Out-of-coverage event bindings become unbound, with their original
records attached. A recovered take explicitly reports incomplete coverage.

Annotations preserve the Raycat version-2 geometry: rectangle/ellipse corners, arrow ends,
stroke polylines and text anchors; colors are sRGB triples, stroke/text sizes are fractions
of the short image dimension. Legacy version-1 annotations migrate on load. Engine-native
sidecars remain host-owned; Raycat synchronizes authored changes into its delivered bundle.

Native `.aprof`, `.b3rec`, entity JSONL and `.brtake` files stay attached unchanged. Their
own schema versions, profile retention limits and replay guarantees remain authoritative.
Bundle availability does not prove physics determinism or reconstruct unrecorded channels.
