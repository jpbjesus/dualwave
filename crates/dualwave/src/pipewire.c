/* DualWave's native PipeWire endpoints. No allocations or locks in process callbacks. */
#include <pipewire/pipewire.h>
#include <spa/param/audio/format-utils.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

/* Bounded SPSC rings. Each side alone owns its read/write index. */
#define CAPACITY 16384u
struct ring {
    _Atomic unsigned read, write;
    float samples[CAPACITY][2];
};
struct endpoint {
    struct pw_stream *stream;
    struct ring ring;
    _Atomic int state;
    _Atomic int flush;
};
struct bridge {
    struct pw_thread_loop *loop;
    struct endpoint playback, capture;
};

static unsigned available(struct ring *r) {
    return atomic_load_explicit(&r->write, memory_order_acquire)
        - atomic_load_explicit(&r->read, memory_order_acquire);
}

static unsigned push(struct ring *r, const float *samples, unsigned frames) {
    unsigned w = atomic_load_explicit(&r->write, memory_order_relaxed);
    unsigned rd = atomic_load_explicit(&r->read, memory_order_acquire);
    frames = SPA_MIN(frames, CAPACITY - (w - rd));
    for (unsigned i = 0; i < frames; i++) {
        unsigned index = (w + i) % CAPACITY;
        r->samples[index][0] = samples[i * 2];
        r->samples[index][1] = samples[i * 2 + 1];
    }
    atomic_store_explicit(&r->write, w + frames, memory_order_release);
    return frames;
}

static unsigned pop(struct ring *r, float *samples, unsigned frames) {
    unsigned rd = atomic_load_explicit(&r->read, memory_order_relaxed);
    unsigned w = atomic_load_explicit(&r->write, memory_order_acquire);
    frames = SPA_MIN(frames, w - rd);
    for (unsigned i = 0; i < frames; i++)
        memcpy(samples + i * 2, r->samples[(rd + i) % CAPACITY], 2 * sizeof(float));
    atomic_store_explicit(&r->read, rd + frames, memory_order_release);
    return frames;
}

static void state_changed(void *data, enum pw_stream_state old,
        enum pw_stream_state state, const char *error) {
    (void)old;
    (void)error;
    struct endpoint *e = data;
    atomic_store(&e->state, state);
}

static void playback_process(void *data) {
    struct endpoint *e = data;
    struct pw_buffer *b = pw_stream_dequeue_buffer(e->stream);
    if (!b) return;
    struct spa_buffer *buf = b->buffer;
    if (buf->n_datas > 0 && buf->datas[0].data && buf->datas[0].chunk) {
        struct spa_data *d = &buf->datas[0];
        unsigned offset = SPA_MIN(d->chunk->offset, d->maxsize);
        unsigned bytes = SPA_MIN(d->chunk->size, d->maxsize - offset);
        if (offset % (sizeof(float) * 2) == 0)
            push(&e->ring, (const float *)((char *)d->data + offset), bytes / (sizeof(float) * 2));
    }
    pw_stream_queue_buffer(e->stream, b);
}

static void capture_process(void *data) {
    struct endpoint *e = data;
    if (atomic_exchange(&e->flush, 0)) {
        atomic_store_explicit(&e->ring.read,
            atomic_load_explicit(&e->ring.write, memory_order_acquire), memory_order_release);
    }
    struct pw_buffer *b = pw_stream_dequeue_buffer(e->stream);
    if (!b) return;
    struct spa_buffer *buf = b->buffer;
    if (buf->n_datas > 0 && buf->datas[0].data && buf->datas[0].chunk) {
        struct spa_data *d = &buf->datas[0];
        unsigned frames = d->maxsize / (sizeof(float) * 2);
        if (b->requested) frames = SPA_MIN(frames, b->requested);
        unsigned got = pop(&e->ring, d->data, frames);
        memset((float *)d->data + got * 2, 0, (frames - got) * 2 * sizeof(float));
        d->chunk->offset = 0;
        d->chunk->stride = sizeof(float) * 2;
        d->chunk->size = frames * sizeof(float) * 2;
    }
    pw_stream_queue_buffer(e->stream, b);
}

static const struct pw_stream_events playback_events = {
    PW_VERSION_STREAM_EVENTS, .state_changed = state_changed, .process = playback_process,
};
static const struct pw_stream_events capture_events = {
    PW_VERSION_STREAM_EVENTS, .state_changed = state_changed, .process = capture_process,
};

static int connect_endpoint(struct bridge *b, struct endpoint *e, int capture) {
    uint8_t storage[1024];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    struct spa_audio_info_raw info = {
        .format = SPA_AUDIO_FORMAT_F32, .rate = 48000, .channels = 2,
        .position = { SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR },
    };
    struct pw_properties *props = pw_properties_new(
        PW_KEY_APP_NAME, "DualWave",
        PW_KEY_NODE_NAME, capture ? "dualwave.microphone" : "dualwave.headphones",
        PW_KEY_NODE_DESCRIPTION, capture ? "DualWave Microphone" : "DualWave Headphones",
        PW_KEY_MEDIA_CLASS, capture ? "Audio/Source" : "Audio/Sink",
        PW_KEY_NODE_VIRTUAL, "true",
        PW_KEY_NODE_PAUSE_ON_IDLE, "true",
        PW_KEY_NODE_WANT_DRIVER, "true",
        PW_KEY_NODE_LATENCY, "512/48000",
        /* Connecting DualWave never steals the user's default audio device. */
        "priority.session", "0",
        NULL);
    e->stream = pw_stream_new_simple(pw_thread_loop_get_loop(b->loop),
        capture ? "DualWave Microphone" : "DualWave Headphones", props,
        capture ? &capture_events : &playback_events, e);
    if (!e->stream) return -1;
    const struct spa_pod *params[] = {
        spa_format_audio_raw_build(&builder, SPA_PARAM_EnumFormat, &info),
    };
    return pw_stream_connect(e->stream, capture ? PW_DIRECTION_OUTPUT : PW_DIRECTION_INPUT,
        PW_ID_ANY, PW_STREAM_FLAG_MAP_BUFFERS | PW_STREAM_FLAG_RT_PROCESS, params, 1);
}

void dw_pw_free(struct bridge *b) {
    if (!b) return;
    if (b->loop) {
        pw_thread_loop_stop(b->loop);
        if (b->playback.stream) pw_stream_destroy(b->playback.stream);
        if (b->capture.stream) pw_stream_destroy(b->capture.stream);
        pw_thread_loop_destroy(b->loop);
    }
    free(b);
}

struct bridge *dw_pw_new(void) {
    pw_init(NULL, NULL);
    struct bridge *b = calloc(1, sizeof(*b));
    if (!b) return NULL;
    atomic_init(&b->playback.ring.read, 0);
    atomic_init(&b->playback.ring.write, 0);
    atomic_init(&b->capture.ring.read, 0);
    atomic_init(&b->capture.ring.write, 0);
    atomic_init(&b->playback.flush, 0);
    atomic_init(&b->capture.flush, 0);
    atomic_init(&b->playback.state, PW_STREAM_STATE_UNCONNECTED);
    atomic_init(&b->capture.state, PW_STREAM_STATE_UNCONNECTED);
    b->loop = pw_thread_loop_new("dualwave-pipewire", NULL);
    if (!b->loop || connect_endpoint(b, &b->playback, 0) < 0
            || connect_endpoint(b, &b->capture, 1) < 0 || pw_thread_loop_start(b->loop) < 0) {
        dw_pw_free(b);
        return NULL;
    }
    return b;
}

int dw_pw_state(struct bridge *b, int capture) {
    return atomic_load(capture ? &b->capture.state : &b->playback.state);
}
unsigned dw_pw_available(struct bridge *b) { return available(&b->playback.ring); }
unsigned dw_pw_read(struct bridge *b, float *out, unsigned frames) {
    return pop(&b->playback.ring, out, frames);
}
unsigned dw_pw_write(struct bridge *b, const float *in, unsigned frames) {
    return push(&b->capture.ring, in, frames);
}
/* Only the ring's consumer changes read. RT callbacks run outside the main-loop lock. */
void dw_pw_clear(struct bridge *b, int capture) {
    if (capture) {
        atomic_store(&b->capture.flush, 1);
        return;
    }
    struct ring *r = &b->playback.ring;
    atomic_store_explicit(&r->read, atomic_load_explicit(&r->write, memory_order_acquire), memory_order_release);
}
