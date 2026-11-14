# Makefile — Rust workspace build, Windows interoperability components, install.
#
# The Linux-side runtime (TAP bridge, service, firewall/routes, lifecycle) is
# Rust. The Windows-side components that talk to Wine/Radmin remain C shims
# compiled with MinGW; they are interoperability shims, not Linux code.

PREFIX ?= /usr
SYSTEMDDIR ?= /usr/lib/systemd/user
BINDIR ?= $(PREFIX)/bin
LIBDIR ?= $(PREFIX)/lib
ASSETDIR ?= $(LIBDIR)/radmin-vpn

CARGO ?= cargo
CC       = gcc
MINGW32  = i686-w64-mingw32-gcc
MINGW64  = x86_64-w64-mingw32-gcc
CFLAGS   = -Wall -O2
BUILD    = build
STRIP    = strip
STRIP32  = i686-w64-mingw32-strip
STRIP64  = x86_64-w64-mingw32-strip
STRIP_FLAGS = -s -S --strip-debug --strip-unneeded

# DDK header path differs across distros:
#   Debian/Ubuntu:  /usr/x86_64-w64-mingw32/include/ddk
#   Fedora/RHEL:    /usr/x86_64-w64-mingw32/sys-root/mingw/include/ddk
DDK_INC := $(firstword $(wildcard \
    /usr/x86_64-w64-mingw32/include/ddk \
    /usr/x86_64-w64-mingw32/sys-root/mingw/include/ddk \
    /usr/x86_64-w64-mingw32/sys-root/mingw/include \
    /usr/x86_64-w64-mingw32/include \
))
ifneq ($(DDK_INC),)
DDKFLAGS = -I$(DDK_INC)
else
DDKFLAGS =
$(warning DDK headers not found in standard paths; rvpnnetmp.sys compile may fail)
endif

MINGW64_LIB := $(firstword $(wildcard \
    /usr/x86_64-w64-mingw32/lib \
    /usr/x86_64-w64-mingw32/sys-root/mingw/lib \
))
ifneq ($(MINGW64_LIB),)
MINGW64_LIBFLAGS = -L$(MINGW64_LIB)
else
MINGW64_LIBFLAGS =
endif

WINBINS = $(BUILD)/rvpnnetmp.sys $(BUILD)/adapter_hook.dll $(BUILD)/rvpn_launcher.exe \
          $(BUILD)/netsh.exe $(BUILD)/netsh64.exe $(BUILD)/drvinst.exe

.PHONY: all
all: target/release/radmin-vpn $(WINBINS)

# Always let cargo decide what needs rebuilding: a directory-level dependency
# (`src crates Cargo.toml`) does not notice edits to files inside src/, which
# let stale release binaries slip into installs.
.PHONY: force
force:

target/release/radmin-vpn: force
	$(CARGO) build --release --workspace

$(BUILD):
	mkdir -p $(BUILD)

$(BUILD)/rvpnnetmp.sys: src/rvpnnetmp.c | $(BUILD)
	$(MINGW64) -shared -o $@ $< \
		$(DDKFLAGS) $(MINGW64_LIBFLAGS) \
		-lntoskrnl -lhal -nostdlib -lgcc \
		-Wl,--subsystem,native -Wl,--entry,DriverEntry
	$(STRIP64) $(STRIP_FLAGS) $@ || true

$(BUILD)/adapter_hook.dll: src/adapter_hook.c | $(BUILD)
	$(MINGW32) -shared -o $@ $< \
		-liphlpapi -lws2_32 -lole32 -Wl,--enable-stdcall-fixup
	$(STRIP32) $(STRIP_FLAGS) $@ || true

$(BUILD)/rvpn_launcher.exe: src/rvpn_launcher.c | $(BUILD)
	$(MINGW32) $(CFLAGS) -o $@ $<
	$(STRIP32) $(STRIP_FLAGS) $@ || true

$(BUILD)/netsh.exe: src/netsh_wrapper.c | $(BUILD)
	$(MINGW32) $(CFLAGS) -o $@ $< -municode
	$(STRIP32) $(STRIP_FLAGS) $@ || true

$(BUILD)/netsh64.exe: src/netsh_wrapper.c | $(BUILD)
	$(MINGW64) $(CFLAGS) -o $@ $< -municode
	$(STRIP64) $(STRIP_FLAGS) $@ || true

$(BUILD)/drvinst.exe: src/drvinst.c | $(BUILD)
	$(MINGW32) $(CFLAGS) -o $@ $<
	$(STRIP32) $(STRIP_FLAGS) $@ || true

.PHONY: install
install: target/release/radmin-vpn $(WINBINS)
	install -d -m 0755 "$(DESTDIR)$(BINDIR)"
	install -d -m 0755 "$(DESTDIR)$(LIBDIR)"
	install -Dm0755 target/release/radmin-vpn "$(DESTDIR)$(BINDIR)/radmin-vpn"
	if [ -z "$(DESTDIR)" ]; then \
		setcap 'cap_net_admin,cap_net_raw+ep' "$(BINDIR)/radmin-vpn"; \
	else \
		setcap 'cap_net_admin,cap_net_raw+ep' "$(DESTDIR)$(BINDIR)/radmin-vpn" 2>/dev/null || echo "warning: could not set file capabilities in staging tree (package postinst will apply them)"; \
	fi
	install -Dm0644 packaging/systemd/radmin-vpn.service "$(DESTDIR)$(SYSTEMDDIR)/radmin-vpn.service"
	install -d -m 0755 "$(DESTDIR)$(ASSETDIR)"
	for f in $(WINBINS); do install -Dm0644 "$$f" "$(DESTDIR)$(ASSETDIR)/$$(basename $$f)"; done
	if [ -f target/release/librvpn_dnsfix.so ]; then \
		install -Dm0755 target/release/librvpn_dnsfix.so "$(DESTDIR)$(ASSETDIR)/rvpn_dnsfix.so"; \
	fi
	if [ -f target/release/librvpn_reuseport.so ]; then \
		install -Dm0755 target/release/librvpn_reuseport.so "$(DESTDIR)$(ASSETDIR)/rvpn_reuseport.so"; \
	fi

.PHONY: clean
clean:
	$(CARGO) clean

.PHONY: distclean
distclean: clean
	rm -rf $(BUILD) target

.PHONY: check
check:
	cargo fmt --check
	cargo clippy --all-targets --all-features
	cargo test --all-features
