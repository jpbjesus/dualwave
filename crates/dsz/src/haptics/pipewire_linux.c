/* Native GUI PipeWire streams. Audio callbacks use bounded SPSC rings only. */
#include <pipewire/pipewire.h>
#include <spa/param/audio/format-utils.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <pthread.h>

static pthread_once_t pipewire_once = PTHREAD_ONCE_INIT;
static void init_pipewire(void) { pw_init(NULL, NULL); }

#define CAPACITY 16384u
struct ring { _Atomic unsigned read, write; float samples[CAPACITY][2]; };
struct endpoint;
struct endpoint_link {
    struct endpoint *endpoint;
    struct endpoint_link *next;
    struct pw_link *proxy;
    struct spa_hook listener;
    uint32_t id;
    int active;
};
struct endpoint {
    struct pw_thread_loop *loop;
    struct pw_stream *stream;
    struct ring ring;
    _Atomic int state, flush, enabled;
    _Atomic unsigned demand;
    struct pw_registry *registry;
    struct spa_hook registry_listener;
    struct endpoint_link *links;
    int mode; /* 0: headphones sink, 1: microphone source, 2: output monitor */
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
static void discard(struct ring *r) {
    atomic_store_explicit(&r->read, atomic_load_explicit(&r->write, memory_order_acquire), memory_order_release);
}
static void state_changed(void *data, enum pw_stream_state old,
        enum pw_stream_state state, const char *error) {
    (void)old; (void)error;
    struct endpoint *e = data;
    atomic_store(&e->state, state);
    pw_thread_loop_signal(e->loop, false);
}
static void source_fill(struct endpoint *e, float *output, unsigned frames) {
    int enabled = atomic_load_explicit(&e->enabled, memory_order_acquire)
        && atomic_load_explicit(&e->demand, memory_order_acquire) > 0;
    if (atomic_exchange(&e->flush, 0) || !enabled) discard(&e->ring);
    unsigned got = enabled ? pop(&e->ring, output, frames) : 0;
    memset(output + got * 2, 0, (frames - got) * 2 * sizeof(float));
}
static void process(void *data) {
    struct endpoint *e = data;
    struct pw_buffer *b = pw_stream_dequeue_buffer(e->stream);
    if (!b) return;
    struct spa_buffer *buf = b->buffer;
    if (buf->n_datas > 0 && buf->datas[0].data && buf->datas[0].chunk) {
        struct spa_data *d = &buf->datas[0];
        if (e->mode == 1) {
            unsigned frames = d->maxsize / (sizeof(float) * 2);
            if (b->requested) frames = SPA_MIN(frames, b->requested);
            source_fill(e, d->data, frames);
            d->chunk->offset = 0;
            d->chunk->stride = sizeof(float) * 2;
            d->chunk->size = frames * sizeof(float) * 2;
        } else {
            unsigned offset = SPA_MIN(d->chunk->offset, d->maxsize);
            unsigned bytes = SPA_MIN(d->chunk->size, d->maxsize - offset);
            if (offset % (sizeof(float) * 2) == 0)
                push(&e->ring, (const float *)((char *)d->data + offset), bytes / (sizeof(float) * 2));
        }
    }
    pw_stream_queue_buffer(e->stream, b);
}
static const struct pw_stream_events events = {
    PW_VERSION_STREAM_EVENTS, .state_changed = state_changed, .process = process,
};
static void endpoint_link_info(void *data, const struct pw_link_info *info) {
    struct endpoint_link *link = data;
    int active = info->state == PW_LINK_STATE_ACTIVE;
    if (active == link->active) return;
    link->active = active;
    if (active) atomic_fetch_add(&link->endpoint->demand, 1);
    else atomic_fetch_sub(&link->endpoint->demand, 1);
}
static const struct pw_link_events endpoint_link_events = {
    PW_VERSION_LINK_EVENTS, .info = endpoint_link_info,
};
static void endpoint_global(void *data, uint32_t id, uint32_t permissions,
        const char *type, uint32_t version, const struct spa_dict *props) {
    (void)permissions;
    struct endpoint *e = data;
    if (strcmp(type, PW_TYPE_INTERFACE_Link) || !props) return;
    const char *node = spa_dict_lookup(props, e->mode == 1 ? PW_KEY_LINK_OUTPUT_NODE : PW_KEY_LINK_INPUT_NODE);
    if (!node || strtoul(node, NULL, 10) != pw_stream_get_node_id(e->stream)) return;
    struct endpoint_link *link = calloc(1, sizeof(*link));
    if (!link) return;
    link->endpoint = e;
    link->id = id;
    link->proxy = pw_registry_bind(e->registry, id, type, SPA_MIN(version, (uint32_t)PW_VERSION_LINK), 0);
    if (!link->proxy) { free(link); return; }
    link->next = e->links;
    e->links = link;
    pw_link_add_listener(link->proxy, &link->listener, &endpoint_link_events, link);
}
static void endpoint_global_remove(void *data, uint32_t id) {
    struct endpoint *e = data;
    struct endpoint_link **at = &e->links;
    while (*at) {
        struct endpoint_link *link = *at;
        if (link->id != id) { at = &link->next; continue; }
        *at = link->next;
        if (link->active) atomic_fetch_sub(&e->demand, 1);
        spa_hook_remove(&link->listener);
        pw_proxy_destroy((struct pw_proxy *)link->proxy);
        free(link);
        return;
    }
}
static const struct pw_registry_events endpoint_registry_events = {
    PW_VERSION_REGISTRY_EVENTS, .global = endpoint_global, .global_remove = endpoint_global_remove,
};
void dwg_pw_free(struct endpoint *e) {
    if (!e) return;
    if (e->loop) {
        pw_thread_loop_stop(e->loop);
        while (e->links) endpoint_global_remove(e, e->links->id);
        if (e->registry) {
            spa_hook_remove(&e->registry_listener);
            pw_proxy_destroy((struct pw_proxy *)e->registry);
        }
        if (e->stream) pw_stream_destroy(e->stream);
        pw_thread_loop_destroy(e->loop);
    }
    free(e);
}
struct endpoint *dwg_pw_new(int mode, const char *name, const char *description, const char *target) {
    pthread_once(&pipewire_once, init_pipewire);
    struct endpoint *e = calloc(1, sizeof(*e));
    if (!e) return NULL;
    e->mode = mode;
    atomic_init(&e->ring.read, 0);
    atomic_init(&e->ring.write, 0);
    atomic_init(&e->flush, 0);
    atomic_init(&e->enabled, 0);
    atomic_init(&e->demand, 0);
    atomic_init(&e->state, PW_STREAM_STATE_UNCONNECTED);
    e->loop = pw_thread_loop_new("dualwave-gui-audio", NULL);
    if (!e->loop) goto fail;
    struct pw_properties *props = pw_properties_new(
        PW_KEY_APP_NAME, "DualWave", PW_KEY_NODE_NAME, name,
        PW_KEY_NODE_DESCRIPTION, description,
        PW_KEY_NODE_LATENCY, "512/48000", NULL);
    if (mode == 2) {
        pw_properties_set(props, PW_KEY_STREAM_CAPTURE_SINK, "true");
        pw_properties_set(props, PW_KEY_STREAM_MONITOR, "true");
        pw_properties_set(props, PW_KEY_MEDIA_CATEGORY, "Capture");
        if (target && *target) pw_properties_set(props, PW_KEY_TARGET_OBJECT, target);
    } else {
        pw_properties_set(props, PW_KEY_MEDIA_CLASS, mode == 1 ? "Audio/Source" : "Audio/Sink");
        pw_properties_set(props, PW_KEY_NODE_VIRTUAL, "true");
        pw_properties_set(props, PW_KEY_NODE_PAUSE_ON_IDLE, "true");
        pw_properties_set(props, PW_KEY_NODE_WANT_DRIVER, "true");
        /* Merely connecting a controller must not change desktop defaults. */
        pw_properties_set(props, "priority.session", "0");
    }
    e->stream = pw_stream_new_simple(pw_thread_loop_get_loop(e->loop), description, props, &events, e);
    if (!e->stream) goto fail;
    uint8_t storage[1024];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    struct spa_audio_info_raw info = {
        .format = SPA_AUDIO_FORMAT_F32, .rate = 48000, .channels = 2,
        .position = { SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR },
    };
    const struct spa_pod *params[] = { spa_format_audio_raw_build(&builder, SPA_PARAM_EnumFormat, &info) };
    enum pw_stream_flags flags = PW_STREAM_FLAG_MAP_BUFFERS | PW_STREAM_FLAG_RT_PROCESS;
    if (mode == 2) flags |= PW_STREAM_FLAG_AUTOCONNECT;
    if (pw_stream_connect(e->stream, mode == 1 ? PW_DIRECTION_OUTPUT : PW_DIRECTION_INPUT,
            PW_ID_ANY, flags, params, 1) < 0) goto fail;
    if (mode != 2) {
        struct pw_core *core = pw_stream_get_core(e->stream);
        if (!core) goto fail;
        e->registry = pw_core_get_registry(core, PW_VERSION_REGISTRY, 0);
        if (!e->registry) goto fail;
        pw_registry_add_listener(e->registry, &e->registry_listener, &endpoint_registry_events, e);
    }
    if (pw_thread_loop_start(e->loop) < 0) goto fail;
    /* A successful allocation is not evidence of a daemon connection. */
    pw_thread_loop_lock(e->loop);
    struct timespec deadline;
    pw_thread_loop_get_time(e->loop, &deadline, 3 * SPA_NSEC_PER_SEC);
    int state;
    while ((state = atomic_load(&e->state)) == PW_STREAM_STATE_UNCONNECTED || state == PW_STREAM_STATE_CONNECTING) {
        if (pw_thread_loop_timed_wait_full(e->loop, &deadline) < 0) break;
    }
    state = atomic_load(&e->state);
    pw_thread_loop_unlock(e->loop);
    if (state < PW_STREAM_STATE_PAUSED) goto fail;
    return e;
fail:
    dwg_pw_free(e);
    return NULL;
}
int dwg_pw_state(struct endpoint *e) { return atomic_load(&e->state); }
int dwg_pw_demand(struct endpoint *e) {
    return atomic_load(&e->state) == PW_STREAM_STATE_STREAMING && atomic_load(&e->demand) > 0;
}
unsigned dwg_pw_available(struct endpoint *e) { return available(&e->ring); }
unsigned dwg_pw_read(struct endpoint *e, float *out, unsigned frames) { return pop(&e->ring, out, frames); }
unsigned dwg_pw_write(struct endpoint *e, const float *in, unsigned frames) {
    if (!atomic_load_explicit(&e->enabled, memory_order_acquire)
            || atomic_load_explicit(&e->demand, memory_order_acquire) == 0) return 0;
    return push(&e->ring, in, frames);
}
void dwg_pw_enable(struct endpoint *e, int enabled) {
    atomic_store_explicit(&e->enabled, enabled, memory_order_release);
    if (!enabled) atomic_store(&e->flush, 1);
}
void dwg_pw_clear(struct endpoint *e) {
    if (e->mode == 1) atomic_store(&e->flush, 1);
    else discard(&e->ring);
}

/* Registry enumeration avoids a dependency on pw-dump or a PulseAudio server. */
struct outputs {
    struct pw_main_loop *loop;
    struct pw_core *core;
    int seq, failed;
    void (*emit)(void *, const char *);
    void *data;
};
static void global(void *data, uint32_t id, uint32_t permissions, const char *type,
        uint32_t version, const struct spa_dict *props) {
    (void)id; (void)permissions; (void)version;
    struct outputs *o = data;
    if (strcmp(type, PW_TYPE_INTERFACE_Node) || !props) return;
    const char *class = spa_dict_lookup(props, PW_KEY_MEDIA_CLASS);
    const char *name = spa_dict_lookup(props, PW_KEY_NODE_NAME);
    if (class && !strcmp(class, "Audio/Sink") && name) o->emit(o->data, name);
}
static void done(void *data, uint32_t id, int seq) {
    (void)id;
    struct outputs *o = data;
    if (seq == o->seq) pw_main_loop_quit(o->loop);
}
static void core_error(void *data, uint32_t id, int seq, int res, const char *message) {
    (void)id; (void)seq; (void)res; (void)message;
    struct outputs *o = data;
    o->failed = 1;
    pw_main_loop_quit(o->loop);
}
static void timeout(void *data, uint64_t expirations) {
    (void)expirations;
    struct outputs *o = data;
    o->failed = 1;
    pw_main_loop_quit(o->loop);
}
int dwg_pw_outputs(void (*emit)(void *, const char *), void *data) {
    pthread_once(&pipewire_once, init_pipewire);
    struct outputs o = { .emit = emit, .data = data };
    o.loop = pw_main_loop_new(NULL);
    if (!o.loop) return -1;
    struct pw_loop *loop = pw_main_loop_get_loop(o.loop);
    struct pw_context *context = pw_context_new(loop, NULL, 0);
    o.core = context ? pw_context_connect(context, NULL, 0) : NULL;
    if (!o.core) { if (context) pw_context_destroy(context); pw_main_loop_destroy(o.loop); return -1; }
    static const struct pw_registry_events re = { PW_VERSION_REGISTRY_EVENTS, .global = global };
    static const struct pw_core_events ce = { PW_VERSION_CORE_EVENTS, .done = done, .error = core_error };
    struct spa_hook rh, ch;
    struct pw_registry *registry = pw_core_get_registry(o.core, PW_VERSION_REGISTRY, 0);
    pw_registry_add_listener(registry, &rh, &re, &o);
    pw_core_add_listener(o.core, &ch, &ce, &o);
    struct spa_source *timer = pw_loop_add_timer(loop, timeout, &o);
    struct timespec limit = { .tv_sec = 3 };
    pw_loop_update_timer(loop, timer, &limit, NULL, false);
    o.seq = pw_core_sync(o.core, PW_ID_CORE, 0);
    pw_main_loop_run(o.loop);
    pw_loop_destroy_source(loop, timer);
    spa_hook_remove(&rh);
    spa_hook_remove(&ch);
    pw_proxy_destroy((struct pw_proxy *)registry);
    pw_core_disconnect(o.core);
    pw_context_destroy(context);
    pw_main_loop_destroy(o.loop);
    return o.failed ? -1 : 0;
}

/* Exercises the same producer/consumer functions without a daemon or audio device. */
int dwg_pw_buffer_test(void) {
    struct endpoint *e = calloc(1, sizeof(*e));
    float *input = calloc(CAPACITY * 2, sizeof(float));
    float *output = calloc(CAPACITY * 2, sizeof(float));
    if (!e || !input || !output) { free(e); free(input); free(output); return 0; }
    atomic_init(&e->ring.read, UINT32_MAX - 5);
    atomic_init(&e->ring.write, UINT32_MAX - 5);
    atomic_init(&e->enabled, 0);
    atomic_init(&e->flush, 0);
    atomic_init(&e->demand, 1);
    for (unsigned i = 0; i < CAPACITY; i++) { input[i * 2] = (float)i; input[i * 2 + 1] = -(float)i; }
    int ok = dwg_pw_write(e, input, 100) == 0 && available(&e->ring) == 0;
    dwg_pw_enable(e, 1);
    ok &= dwg_pw_write(e, input, CAPACITY) == CAPACITY;
    ok &= dwg_pw_write(e, input, 1) == 0;
    ok &= pop(&e->ring, output, CAPACITY) == CAPACITY;
    ok &= memcmp(input, output, CAPACITY * 2 * sizeof(float)) == 0;
    ok &= available(&e->ring) == 0;
    ok &= dwg_pw_write(e, input, 100) == 100;
    dwg_pw_enable(e, 0);
    source_fill(e, output, 100);
    for (unsigned i = 0; i < 200; i++) ok &= output[i] == 0;
    ok &= available(&e->ring) == 0 && dwg_pw_write(e, input, 100) == 0;
    dwg_pw_enable(e, 1);
    ok &= dwg_pw_write(e, input, 100) == 100;
    source_fill(e, output, 200);
    ok &= memcmp(input, output, 200 * sizeof(float)) == 0;
    for (unsigned i = 200; i < 400; i++) ok &= output[i] == 0;
    atomic_store(&e->demand, 0);
    ok &= dwg_pw_write(e, input, 100) == 0;
    source_fill(e, output, 100);
    for (unsigned i = 0; i < 200; i++) ok &= output[i] == 0;
    free(e); free(input); free(output);
    return ok;
}
