// SPDX-License-Identifier: MIT
// Link against the host's Box3D and libframetape_ffi; no engine sources are bundled here.
#include "frametape.h"
#include <box3d/box3d.h>
#include <cstdio>

int main(int argc, char** argv)
{
    if (argc != 3) { std::fprintf(stderr, "usage: box3d-record OUTPUT.ftape OUTPUT.b3rec\n"); return 2; }
    ft_session* tape = ft_session_open(argv[1], "{\"host\":\"Box3D C++ example\",\"clock\":\"simulation\"}");
    if (!tape) { std::fprintf(stderr, "%s\n", ft_last_error()); return 1; }
    const b3WorldDef def = b3DefaultWorldDef();
    const b3WorldId world = b3CreateWorld(&def);
    b3BodyDef bodyDef = b3DefaultBodyDef();
    bodyDef.type = b3_dynamicBody;
    bodyDef.position = {0, 3, 0};
    const b3BodyId body = b3CreateBody(world, &bodyDef);
    const b3ShapeDef shapeDef = b3DefaultShapeDef();
    const b3Sphere sphere = {{0, 0, 0}, 0.5f};
    b3CreateSphereShape(body, &shapeDef, &sphere);
    b3Recording* recording = b3CreateRecording(0);
    b3World_StartRecording(world, recording);
    bool ok = true;
    for (uint64_t frame = 0; ok && frame < 60; ++frame)
    {
        ok = ft_session_frame(tape, frame, frame * 1000000000 / 60, 1000000000 / 60) == 0;
        for (uint64_t sub = 0; ok && sub < 2; ++sub)
        {
            const uint64_t step = frame * 2 + sub;
            b3World_Step(world, 1.0f / 120.0f, 4);
            char json[160];
            std::snprintf(json, sizeof(json), "{\"physics_step\":%llu,\"render_frame\":%llu}", static_cast<unsigned long long>(step), static_cast<unsigned long long>(frame));
            ok = ft_session_event(tape, frame, (step + 1) * 1000000000 / 120, "physics.box3d.step", json) == 0;
        }
    }
    b3World_StopRecording(world);
    ok = ok && b3SaveRecordingToFile(recording, argv[2]);
    if (ok) ok = ft_session_attach(tape, "physics.box3d", argv[2], "application/x-box3d-recording") == 0;
    b3DestroyRecording(recording);
    b3DestroyWorld(world);
    if (ok) ok = ft_session_close(tape) == 0;
    else { std::fprintf(stderr, "%s\n", ft_last_error()); ft_session_abort(tape); }
    if (!ok) std::fprintf(stderr, "Box3D recording failed; journal and original payload remain available\n");
    return ok ? 0 : 1;
}
