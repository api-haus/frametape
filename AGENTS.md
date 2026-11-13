# FramETape

MIT recording libraries and local tools. Preserve original channel payloads and explicit
frame mappings. A missing/dropped/repeated image must never masquerade as a captured frame.
No recordings, private project code, credentials or machine-specific paths in public commits.
Run `processqueue cpu cargo test --workspace` and review the diff before committing.
Use independent capture/channel producers; core bundle/annotation crate has no engine dependency.
Scratch: `.agents/scratch/` (excluded via `.git/info/exclude`). Builds: `target/` (same).
Native playback checks must verify drawable pixels, not only clocks/events. WebKit can report
readiness while a detached video decoder yields transparent canvas snapshots; attach auxiliary
decoders to the document. Serialize native seeks: canceling a JS wait does not cancel GStreamer's seek.
Player instrument: `tools/bench-player/run.py` drives `--example player_bench`; `--play` records continuous
presentation, `--step` paused stepping. A hidden Hyprland workspace delivers no frame callbacks and Xvfb
skips the GPU path: measure in a private headless Wayland compositor, compare a reference player there.
On NVIDIA Wayland WebKit's SHM renderer collapses with window pixels and its GL video sink paints green.
