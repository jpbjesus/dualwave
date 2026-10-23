/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Included after struct dualsense. The child HID never presents a gamepad. */
#include "dualwave-routing.h"

static bool dualwave_audio;
module_param(dualwave_audio, bool, 0444);
MODULE_PARM_DESC(dualwave_audio, "Isolate Bluetooth microphone packets on a vendor HID child (opt-in)");

static const u8 dualwave_audio_descriptor[] = {
	0x06, 0x00, 0xff, /* Vendor-defined usage page, not Generic Desktop/Gamepad */
	0x09, 0x01, 0xa1, 0x01,
	0x85, 0x31,       /* Preserve the original packet and CRC, including report ID */
	0x15, 0x00, 0x26, 0xff, 0x00,
	0x75, 0x08, 0x95, 0x4d, 0x09, 0x01, 0x81, 0x02, 0xc0,
};

static int dualwave_audio_parse(struct hid_device *hdev)
{
	return hid_parse_report(hdev, dualwave_audio_descriptor,
				sizeof(dualwave_audio_descriptor));
}

static int dualwave_audio_start(struct hid_device *hdev)
{
	return 0;
}

static void dualwave_audio_stop(struct hid_device *hdev)
{
}

static int dualwave_audio_raw_request(struct hid_device *hdev, unsigned char reportnum,
		u8 *buf, size_t len, unsigned char rtype, int reqtype)
{
	/* The child is receive-only. Control reports belong to the physical HID. */
	return -EOPNOTSUPP;
}

static const struct hid_ll_driver dualwave_audio_ll = {
	.parse = dualwave_audio_parse,
	.start = dualwave_audio_start,
	.stop = dualwave_audio_stop,
	.open = dualwave_audio_start,
	.close = dualwave_audio_stop,
	.raw_request = dualwave_audio_raw_request,
	.max_buffer_size = 128,
};

static ssize_t dualwave_mic_muted_show(struct device *dev,
		struct device_attribute *attr, char *buf)
{
	struct dualsense *ds = hid_get_drvdata(to_hid_device(dev));
	bool muted;

	scoped_guard(spinlock_irqsave, &ds->base.lock)
		muted = ds->mic_muted;
	return sysfs_emit(buf, "%u\n", muted);
}
static DEVICE_ATTR_RO(dualwave_mic_muted);
static struct attribute *dualwave_audio_attrs[] = {
	&dev_attr_dualwave_mic_muted.attr,
	NULL,
};
static const struct attribute_group dualwave_audio_group = {
	.attrs = dualwave_audio_attrs,
};

static void dualwave_audio_destroy(void *data)
{
	struct dualsense *ds = data;
	struct hid_device *audio = rcu_replace_pointer(ds->audio_hdev, NULL, true);

	/* Parent reports can be in flight during a failed probe or disconnect. */
	synchronize_rcu();
	if (audio)
		hid_destroy_device(audio);
}

static int dualwave_audio_create(struct dualsense *ds)
{
	struct hid_device *parent = ds->base.hdev;
	struct hid_device *audio;
	int ret;

	ret = devm_device_add_group(&parent->dev, &dualwave_audio_group);
	if (ret)
		return ret;
	audio = hid_allocate_device();
	if (IS_ERR(audio))
		return PTR_ERR(audio);
	audio->ll_driver = &dualwave_audio_ll;
	audio->bus = BUS_VIRTUAL;
	audio->vendor = 0;
	audio->product = 0;
	audio->version = 1;
	audio->group = HID_GROUP_GENERIC;
	audio->dev.parent = &parent->dev;
	strscpy(audio->name, "DualWave Microphone Transport", sizeof(audio->name));
	strscpy(audio->uniq, parent->uniq, sizeof(audio->uniq));
	snprintf(audio->phys, sizeof(audio->phys), "%s/dualwave-audio", parent->uniq);
	ret = hid_add_device(audio);
	if (ret) {
		hid_destroy_device(audio);
		return ret;
	}
	rcu_assign_pointer(ds->audio_hdev, audio);
	return devm_add_action_or_reset(&parent->dev, dualwave_audio_destroy, ds);
}

static void dualwave_audio_receive(struct dualsense *ds, u8 *data, int size)
{
	struct hid_device *audio;

	rcu_read_lock();
	audio = rcu_dereference(ds->audio_hdev);
	if (audio)
		hid_safe_input_report(audio, HID_INPUT_REPORT, data, size, size, 1);
	rcu_read_unlock();
}
