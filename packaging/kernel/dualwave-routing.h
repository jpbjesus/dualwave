/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef DUALWAVE_ROUTING_H
#define DUALWAVE_ROUTING_H

enum dualwave_route {
    DUALWAVE_STATE,
    DUALWAVE_LEGACY_MEDIA,
    DUALWAVE_AUDIO,
    DUALWAVE_DISCARD,
};

static inline enum dualwave_route dualwave_report_route(unsigned char tag, int isolated)
{
    unsigned char type = tag & 0x0f;
    if (type <= 1)
        return DUALWAVE_STATE;
    if (!isolated)
        return DUALWAVE_LEGACY_MEDIA;
    return type == 2 ? DUALWAVE_AUDIO : DUALWAVE_DISCARD;
}
/* Local child feature report ABI; these bytes are never sent to the controller. */
#define DUALWAVE_CONTROL_REPORT 0xf0
#define DUALWAVE_CONTROL_SIZE 8
#define DUALWAVE_CONTROL_VERSION 1
#define DUALWAVE_CONTROL_ACTIVE 1
#define DUALWAVE_CONTROL_OWNED 2

static inline int dualwave_control_valid(const unsigned char *data, unsigned int len)
{
    unsigned int i;
    if (len != DUALWAVE_CONTROL_SIZE || data[0] != DUALWAVE_CONTROL_REPORT ||
        data[1] != DUALWAVE_CONTROL_VERSION || data[2] > 1)
        return 0;
    for (i = 3; i < len; i++)
        if (data[i])
            return 0;
    return 1;
}
#endif
