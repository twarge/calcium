# Builds everything Calcium is: the engine, its tests, the Mac and iOS
# apps, the web demo, and the Linux (GTK) editor.
#
#   make            everything below
#   make test       engine unit tests + the corpus
#   make mac        debug app        (make mac-release for optimised)
#   make ios        signed device build
#   make web        wasm engine into web/
#   make gtk        the GTK editor (needs gtk4; see apps/gtk/README.md)
#   make run-gtk    build and launch it
#   make clean

.PHONY: all engine test mac mac-release ios web gtk run-gtk clean

all: test mac ios web gtk

# The engine, always optimised: unoptimised it is six times slower and
# every editor feels it. The app builds run this themselves; standalone
# it is here for CI and the curious.
engine:
	cargo build --release -p calcium-ffi

test:
	cargo test --workspace

# Exactly what Xcode's own Build does — same scheme, same default
# DerivedData, the project's pre-build phase compiling the Rust engine —
# so a make build and a ⌘B are the same build.
mac:
	xcodebuild -project apps/Calcium.xcodeproj -scheme Calcium \
	  -destination "platform=macOS" -quiet build
	@echo "==> Built for macOS"

mac-release:
	xcodebuild -project apps/Calcium.xcodeproj -scheme Calcium \
	  -destination "platform=macOS" -configuration Release -quiet build
	@echo "==> Built for macOS (Release)"

ios:
	xcodebuild -project apps/Calcium.xcodeproj -scheme Calcium \
	  -destination "generic/platform=iOS" -allowProvisioningUpdates -quiet build
	@echo "==> Built for iOS device"

web:
	./apps/build-web.sh

# On Linux, gtk4 development files are the only prerequisite. On macOS,
# Homebrew's libffi is keg-only and the SDK ships zlib/expat/bzip2 as
# libraries without .pc metadata — point pkg-config at the keg and stub
# the metadata, so the target works without touching the environment.
gtk:
ifeq ($(shell uname),Darwin)
	@mkdir -p apps/gtk/target/pkgconfig
	@for lib in zlib:z expat:expat bzip2:bz2; do \
	  name=$${lib%%:*}; link=$${lib##*:}; \
	  printf 'Name: %s\nDescription: %s (macOS SDK)\nVersion: 1\nLibs: -l%s\nCflags:\n' \
	    "$$name" "$$name" "$$link" > "apps/gtk/target/pkgconfig/$$name.pc"; \
	done
	cd apps/gtk && \
	  PKG_CONFIG_PATH="$$(brew --prefix libffi)/lib/pkgconfig:$$PWD/target/pkgconfig:$$PKG_CONFIG_PATH" \
	  cargo build --release
else
	cd apps/gtk && cargo build --release
endif

run-gtk: gtk
	./apps/gtk/target/release/calcium-gtk

clean:
	cargo clean
	cd apps/gtk && cargo clean
