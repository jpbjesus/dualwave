#include "../src/haptics/pipewire_linux.c"
#include <assert.h>
int main(void) {
    assert(dwg_pw_buffer_test());
    return 0;
}
