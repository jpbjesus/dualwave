obj-m += hid-playstation.o

KVER ?= $(shell uname -r)
KERNEL_DIR ?= /usr/lib/modules/$(KVER)/build
KBUILD_OPTIONS := $(shell sed -n 's/^CONFIG_CC_IS_CLANG=y/LLVM=1/p' $(KERNEL_DIR)/include/config/auto.conf)

.PHONY: all clean
all:
	$(MAKE) -C $(KERNEL_DIR) M=$(CURDIR) $(KBUILD_OPTIONS) modules

clean:
	$(MAKE) -C $(KERNEL_DIR) M=$(CURDIR) $(KBUILD_OPTIONS) clean
