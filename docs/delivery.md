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

- [x] Bundle contracts and corruption/path validation; interrupted recording journal retained.
- [x] Hardware video encoder with explicit actual settings, bounded queue and failure feedback.
- [x] Desktop capture and all selected evdev events sharing monotonic frame clock.
- [x] CLI inspect/extract/import and annotation packet export.
- [x] Folder catalog, preview, sorting, frame stepping, marks/ranges, persistence.
- [x] Reusable Rust/C ABI/Unreal/browser producers and example consumers.
- [x] Raycat migration and real applicable gate.
- [x] BallRPG migration, native/browser verification.
- [x] Real capture/input/UI proof; codec readability checked, limitations recorded.
- [x] Public source audit, MIT attribution, GitHub publication and remote verdict.

## Verification scope

Workspace compiler/tests, actual NVIDIA desktop capture, Unreal RHI recording, browser input,
Box3D composition and app GUI checks passed. Public synthetic artifacts carry their own receipts.
GitHub CI passed source `38a36b0cbbcfd9417539ea0a24cc3fd343246ef6`, run `43101404695`.

Private host receipts remain in their repositories. BallRPG's actual web host produced 40 source
frames, 40 encoded pictures and 40 native replay frames with no identity mismatch. Its native build,
commandlet and actual editor capture passed. One earlier editor startup SIGSEGV remains
unreproduced; no causal fix is claimed. Raycat's compiler gates and actual native review retained
canonical annotation containers through move/undo/redo, resolved missing source images and opened
the completed take's profiler. Its background palette selection was not verified; BRP screenshots
timed out during recording after one success and succeeded after recording stopped.

AMD runtime needs AMD hardware. Desktop capture currently targets Linux wlroots/evdev; hardware
YUV conversion is not advertised as RGB lossless. Requested frame rate is not a delivery promise.

## QA plan

Build exact workspace commit. In unattended workspace 8, open app against synthetic and
locally imported takes; record QA. Through real keyboard/mouse: sort/filter catalog, open
take, scrub/step/play, draw box/ellipse/brush/arrow/text, move/erase, adjust range, undo/redo,
save/reopen. Inspect screenshots for clipping and correct alignment. Verify an input event
falls within recorded frame interval, dropped images are marked, original attachments survive,
and agent export includes matching marked images/channel data. Do not publish desktop footage
or imported private captures.

## Related documentation

- [Public recording and annotation evidence](evidence/README.md)
- [Format contract](format.md)
