# Delivery

Owner scope: desktop recorder with native NVIDIA/AMD compression and synchronized input;
Rust library, C++ ABI and Unreal plugin/example; browser recording; profiling, Box3D and
custom event channels; one inspectable file; annotation/catalog app; Raycat and BallRPG
migration preserving existing features; public MIT repository under api-haus.

## Reuse

| Capability | Existing source | Action |
| --- | --- | --- |
| Annotation model | Raycat `raycat_record/src/annotations.rs` | Extract; re-export in engine |
| Async video encoding | Raycat `raycat_movie/src/encode.rs` | Extract; add AMD/quality controls |
| Reflected state/replay | Raycat `raycat_record` | Retain engine adapter and original attachment |
| Profiler scopes | Raycat `.aprof` | Attach original ZIP; preserve frame coverage |
| Unreal readbacks | BallRPG `BallTakeVideo.cpp` | Extract plugin; shared encoder via C ABI |
| WebGPU/WebCodecs | BallRPG `CoreHost.cpp` | Extract browser producer |
| Annotation editor | BallRPG `tools/takes/annotator.html` | Extract app; preserve range and mark tools |
| Game replay | BallRPG `.brtake` | Retain game schema; attach bytes and exported frames |
| Box3D | Existing native recorder | Opaque attachment plus physics/render mapping |
| Desktop capture/input | No reusable implementation in audited harnesses | Wayland screencopy + evdev monotonic timestamps |

## Acceptance

- [ ] Bundle contracts and corruption/path validation; interrupted recording journal retained.
- [ ] Hardware video encoder with explicit actual settings, bounded queue and failure feedback.
- [ ] Desktop capture and all selected evdev events sharing monotonic frame clock.
- [ ] CLI inspect/extract/import and annotation packet export.
- [ ] Folder catalog, preview, sorting, frame stepping, marks/ranges, persistence.
- [ ] Reusable Rust/C ABI/Unreal/browser producers and example consumers.
- [ ] Raycat migration and real applicable gate.
- [ ] BallRPG migration, native/browser verification.
- [ ] Real capture/input/UI proof; codec readability checked, limitations recorded.
- [ ] Public source audit, MIT attribution, GitHub publication and remote verdict.

## QA plan

Build exact workspace commit. In unattended workspace 8, open app against synthetic and
locally imported takes; record QA. Through real keyboard/mouse: sort/filter catalog, open
take, scrub/step/play, draw box/ellipse/brush/arrow/text, move/erase, adjust range, undo/redo,
save/reopen. Inspect screenshots for clipping and correct alignment. Verify an input event
falls within recorded frame interval, dropped images are marked, original attachments survive,
and agent export includes matching marked images/channel data. Do not publish desktop footage
or imported private captures.
