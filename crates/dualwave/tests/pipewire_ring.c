/* Exercise the C boundary under concurrent callbacks and wrapping indices.
 * Build/run recipe is in README. No PipeWire server or hardware needed. */
#include "../src/pipewire.c"
#include <assert.h>
#include <limits.h>
#include <pthread.h>
#include <sched.h>
#include <stdio.h>

#define FRAMES 500000u
static struct ring test_ring;

static void *producer(void *unused) {
    (void)unused;
    unsigned sent = 0;
    float data[257][2];
    while (sent < FRAMES) {
        unsigned n = SPA_MIN(257u, FRAMES - sent);
        for (unsigned i = 0; i < n; i++) {
            data[i][0] = sent + i;
            data[i][1] = -(float)(sent + i);
        }
        unsigned wrote = push(&test_ring, &data[0][0], n);
        sent += wrote;
        if (!wrote) sched_yield();
    }
    return NULL;
}

int main(void) {
    atomic_init(&test_ring.read, UINT_MAX - 32u);
    atomic_init(&test_ring.write, UINT_MAX - 32u);
    pthread_t thread;
    assert(pthread_create(&thread, NULL, producer, NULL) == 0);
    unsigned received = 0;
    float data[31][2];
    while (received < FRAMES) {
        unsigned n = pop(&test_ring, &data[0][0], 31);
        for (unsigned i = 0; i < n; i++) {
            assert(data[i][0] == (float)(received + i));
            assert(data[i][1] == -(float)(received + i));
        }
        received += n;
        if (!n) sched_yield();
    }
    assert(pthread_join(thread, NULL) == 0);
    assert(available(&test_ring) == 0);
    puts("PipeWire ring: 500000 stereo frames passed concurrently across index wrap");
    return 0;
}
