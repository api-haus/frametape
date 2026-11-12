#ifndef FRAMETAPE_H
#define FRAMETAPE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ft_session ft_session;
typedef struct ft_video ft_video;

/* Serialize calls on each handle. Different handles are independent.
 * All strings are UTF-8 and borrowed only for the duration of the call.
 * Errors return NULL or -1; ft_last_error lives until the next ABI call on this thread. */
const char *ft_last_error(void);

/* Refuses existing output or its .recording journal. Metadata must be a JSON object. */
ft_session *ft_session_open(const char *path, const char *metadata_json);
/* Frame indices start at zero and are contiguous; timestamps share one monotonic epoch in nanoseconds. */
int ft_session_frame(ft_session *, uint64_t frame, uint64_t timestamp_ns, uint64_t duration_ns);
/* UINT64_MAX leaves independently sampled events unbound; resolve them by timestamp. */
int ft_session_event(ft_session *, uint64_t frame, uint64_t timestamp_ns, const char *channel, const char *json);
int ft_session_attach(ft_session *, const char *channel, const char *path, const char *media_type);
/* Close always consumes the handle. Failure preserves the journal for recovery. */
int ft_session_close(ft_session *);
/* Abort consumes the handle and retains its recoverable journal. */
void ft_session_abort(ft_session *);

/* Packed rgba/bgra/rgb0/bgr0 or x2bgr10le pixels; width * height * 4 bytes.
 * x2bgr10le is converted to RGBA8 before encoding (10-bit source precision is reduced).
 * Pixel bytes are copied before ft_video_frame returns; caller can then reuse/free its buffer. */
ft_video *ft_video_open(const char *path, uint32_t width, uint32_t height, uint32_t fps, const char *pixel_format);
/* 1 queued, 0 busy, -1 failed. Record busy frames explicitly; frame identity is never inferred. */
int ft_video_frame(ft_video *, uint64_t frame, const uint8_t *pixels, size_t length);
/* Close always consumes the handle and waits for the encoder. */
int ft_video_close(ft_video *);
/* Abort consumes the handle; original encoder output is retained. */
void ft_video_abort(ft_video *);

#if defined(__unix__)
typedef struct ft_control ft_control;
/* One opt-in native control host per process. Poll and reply on the app's owning thread.
 * Start also carries window_title; reject a mismatch with the window actually captured.
 * Requests: {"op":"start"|"stop","path":"/absolute/take.ftape"} or {"op":"status"}.
 * Replies: {"recording":bool,"path":string|null,"error":string|null}.
 * A stop reply acknowledges a fully published archive, not queued work. */
ft_control *ft_control_open(void);
/* NULL means no request; inspect ft_last_error for a failure. JSON lives until next poll. */
const char *ft_control_poll(ft_control *);
int ft_control_reply(ft_control *, const char *json);
void ft_control_close(ft_control *);
#endif

#ifdef __cplusplus
}
#endif
#endif
