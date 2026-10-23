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
#endif
