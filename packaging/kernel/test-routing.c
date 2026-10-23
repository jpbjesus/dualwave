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
    puts("512 routing cases passed: media never reaches physical input/raw readers in isolated mode");
    return 0;
}
