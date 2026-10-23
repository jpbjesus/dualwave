/* SPDX-License-Identifier: GPL-2.0-or-later */
#include "dualwave-routing.h"
#include <assert.h>
#include <stdio.h>

int main(void)
{
    for (unsigned seq = 0; seq < 16; seq++) {
        for (unsigned type = 0; type < 16; type++) {
            unsigned char tag = (seq << 4) | type;
            enum dualwave_route expected = type <= 1 ? DUALWAVE_STATE
                : type == 2 ? DUALWAVE_AUDIO : DUALWAVE_DISCARD;
            assert(dualwave_report_route(tag, 1) == expected);
            assert(dualwave_report_route(tag, 0) ==
                (type <= 1 ? DUALWAVE_STATE : DUALWAVE_LEGACY_MEDIA));
        }
    }
    unsigned char request[DUALWAVE_CONTROL_SIZE] = { DUALWAVE_CONTROL_REPORT, DUALWAVE_CONTROL_VERSION };
    assert(dualwave_control_valid(request, sizeof(request)));
    request[2] = 1;
    assert(dualwave_control_valid(request, sizeof(request)));
    for (unsigned command = 2; command < 256; command++) {
        request[2] = command;
        assert(!dualwave_control_valid(request, sizeof(request)));
    }
    request[2] = 1;
    for (unsigned index = 3; index < sizeof(request); index++) {
        request[index] = 1;
        assert(!dualwave_control_valid(request, sizeof(request)));
        request[index] = 0;
    }
    assert(!dualwave_control_valid(request, sizeof(request) - 1));
    assert(!dualwave_control_valid(request, sizeof(request) + 1));
    request[0] = 0x31;
    assert(!dualwave_control_valid(request, sizeof(request)));
    request[0] = DUALWAVE_CONTROL_REPORT;
    request[1]++;
    assert(!dualwave_control_valid(request, sizeof(request)));
    puts("512 media routing cases and strict local exclusive-control ABI validation passed");
    return 0;
}
