/* Native PulseAudio/PipeWire-Pulse control; never changes a sink's volume. */
#include <pulse/pulseaudio.h>
#include <stdint.h>
#include <stdlib.h>
#include <time.h>
#include <errno.h>
#include <unistd.h>
#include <string.h>
#include <stdio.h>

typedef int (*select_volume)(uint32_t, const char *, uint32_t *, unsigned, void *);
struct request {
    select_volume select;
    void *userdata;
    unsigned pending;
    int changed;
    int matched;
    int failed;
};

static int64_t now_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (int64_t)t.tv_sec * 1000 + t.tv_nsec / 1000000;
}

static void changed(pa_context *c, int success, void *userdata) {
    (void)c;
    struct request *r = userdata;
    r->pending--;
    if (success) r->changed++;
    else r->failed = 1;
}

static void stream(pa_context *c, const pa_sink_input_info *info, int eol, void *userdata) {
    struct request *r = userdata;
    if (eol) { if (eol < 0) r->failed = 1; return; }
    if (!info || !info->has_volume || !info->volume_writable || !pa_cvolume_valid(&info->volume)) return;
    const char *text = pa_proplist_gets(info->proplist, PA_PROP_APPLICATION_PROCESS_ID);
    if (!text || !*text) return;
    char *end = NULL;
    errno = 0;
    unsigned long pid = strtoul(text, &end, 10);
    if (errno || *end || !pid || pid > UINT32_MAX) return;
    const char *binary = pa_proplist_gets(info->proplist, PA_PROP_APPLICATION_PROCESS_BINARY);
    const char *host = pa_proplist_gets(info->proplist, PA_PROP_APPLICATION_PROCESS_HOST);
    char local_host[256] = {0};
    if (!binary || !*binary || !host || gethostname(local_host, sizeof(local_host) - 1) < 0 || strcmp(host, local_host)) return;
    pa_cvolume volume = info->volume;
    int selected = r->select((uint32_t)pid, binary, volume.values, volume.channels, r->userdata);
    if (!selected) return;
    r->matched++;
    if (selected < 0) return;
    pa_operation *op = pa_context_set_sink_input_volume(c, info->index, &volume, changed, r);
    if (!op) { r->failed = 1; return; }
    r->pending++;
    pa_operation_unref(op);
}

/* Poll with a deadline so a stopped or disconnected server cannot stall shutdown. */
static int pump(pa_mainloop *loop, pa_context *context, int64_t deadline) {
    if (!PA_CONTEXT_IS_GOOD(pa_context_get_state(context)) || now_ms() >= deadline) return -1;
    if (pa_mainloop_prepare(loop, 20) < 0 || pa_mainloop_poll(loop) < 0 || pa_mainloop_dispatch(loop) < 0) return -1;
    return 0;
}

int dw_game_volume(select_volume select, void *userdata) {
    int result = -1;
    int64_t deadline = now_ms() + 750;
    struct request r = { .select = select, .userdata = userdata };
    pa_mainloop *loop = pa_mainloop_new();
    if (!loop) return -1;
    pa_context *context = pa_context_new(pa_mainloop_get_api(loop), "DualWave game volume");
    pa_operation *op = NULL;
    if (!context) goto done;
    if (pa_context_connect(context, NULL, PA_CONTEXT_NOAUTOSPAWN, NULL) < 0) goto done;
    while (pa_context_get_state(context) != PA_CONTEXT_READY)
        if (pump(loop, context, deadline) < 0) goto done;
    if (pa_context_is_local(context) != 1) goto done;
    op = pa_context_get_sink_input_info_list(context, stream, &r);
    if (!op) goto done;
    while (pa_operation_get_state(op) == PA_OPERATION_RUNNING || r.pending)
        if (pump(loop, context, deadline) < 0) goto done;
    result = r.failed ? -1 : (r.matched ? r.changed : -2);
done:
    if (op) { pa_operation_cancel(op); pa_operation_unref(op); }
    if (context) { pa_context_disconnect(context); pa_context_unref(context); }
    pa_mainloop_free(loop);
    return result;
}
