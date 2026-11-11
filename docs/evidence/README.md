# Recording and annotation checks

Synthetic recordings in this folder are safe to inspect and redistribute under the
project's MIT license. Actual desktop footage and raw desktop input stay private.

## Observed checks

| Surface | Observed result | Evidence |
| --- | --- | --- |
| Desktop | HEVC NVENC, 3840×2160 px, 4:4:4; 79 source frames, 72 encoded pictures over 3.010 s; 85 input events from 34 evdev devices; 0 lost input packets | Aggregate results |
| Native Unreal | 8 source frames and 8 matching encoded pictures at configured 30 fps; HEVC Rext/yuv444p; captions checked on frames 0, 3 and 7; process exit 0 | Unreal take |
| Browser | Real keyboard/pointer/text input, confirmed WebCodecs pictures and one ZIP bundle | Browser take |
| Box3D | Real C++ producer: 60 render frames, 120 physics steps, 10,301 original recording bytes; independent Rust consumer retained bytes | Physics take |
| Annotation app | Real trusted browser input: box, ellipse, arrow, brush, text, move, undo/redo, range 0–6, save, step and source-paced playback | App take and screenshot |
| Missing image | Repeated source frame remains blank with explicit notice; prior picture hidden | Missing-frame screenshot |
| Concurrent save | Stale editor receives conflict; current archive preserved and authored draft retained | Stale-save screenshot |
| Catalog | Name sorting, no-match search and narrow 640×900 px controls exercised; malformed annotations disable only their row while two valid recordings remain available | Narrow view and HTTP check |
| Storage | Original attachment roundtrip; corrupt extraction never publishes partial output; checked edit rejects stale base; recovery retains pending channels without claiming complete video | Workspace tests |
| RGB lossless | Actual FFmpeg encode/decode preserves RGB samples, including odd-size padding | Codec integration test |

Desktop requested 30 fps; the observed capture did not deliver every requested picture.
Null mappings preserve the gaps. This is a functional capture receipt, not a throughput
benchmark or a claim of constant-rate delivery.

Native video input was explicitly disabled in the eight-frame check. The desktop and
browser checks exercise their separate input producers. AMD, Windows and macOS runtime
capture remain unverified; Linux/NVIDIA is the exercised hardware path.

## Reproduce inspection

```bash
frametape inspect docs/evidence/unreal-video.ftape --frames
frametape inspect docs/evidence/browser-input.ftape --events
frametape inspect docs/evidence/box3d.ftape
frametape export docs/evidence/app-qa.ftape app-frames --frames 0,5,15
```

## Related Documentation

- [Aggregate results](results.json)
- [Unreal take](unreal-video.ftape)
- [Browser take](browser-input.ftape)
- [Physics take](box3d.ftape)
- [App take](app-qa.ftape)
- [Annotated app screenshot](app-annotated.png)
- [Missing-frame screenshot](missing-frame.png)
- [Stale-save screenshot](stale-save.png)
- [Narrow view](narrow-view.png)
- [Recording format](../format.md)
