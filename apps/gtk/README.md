# Calcium for Linux

GTK 4 over the same engine, linked as a crate — no C ABI, no JSON, the
document layer's own types. A standalone workspace so the main workspace's
tests never need GTK installed.

```bash
cd apps/gtk
cargo build --release
./target/release/calcium-gtk
```

On Linux, `libgtk-4-dev` (Debian/Ubuntu) or `gtk4-devel` (Fedora) is the
only prerequisite. On macOS it also builds and runs, for development:

```bash
brew install gtk4
# libffi is keg-only, and the sandbox SDK libraries lack .pc files;
# point pkg-config at libffi and provide stubs for zlib/expat/bzip2 if
# your environment does not already.
export PKG_CONFIG_PATH="$(brew --prefix libffi)/lib/pkgconfig"
cargo build
```

## What is ported

Answers in the text after each `=>`, evaluated and spliced on every edit
with the caret held still by the same three-case adjustment as the Mac
editor; deleting an arrow takes its answer with it. Styling from the
engine's own reports: prose grey, headings scaled by depth, comments,
token colours, the redefinition underline, inline Markdown (bold, italic,
code spans, links, list markers). Return steps over answers and continues
list markers; Ctrl+/ toggles comments on indented lines, Ctrl+] and
Ctrl+[ indent and outdent; Ctrl+O and Ctrl+S open and save, answers
materialised on disk and stripped on open. Splices stay off the undo
stack (irreversible actions), so Ctrl+Z steps over the user's own typing
only.

## Not yet

Completions, value scrubbing, prose spelling, per-document view state,
pinch zoom, and the distraction-free chrome. The evaluation is always
inline — the measured fallback the Apple editors use is not ported.

## Flatpak

`com.twarge.Calcium.json` packages the editor for Flathub; `data/` holds
the desktop file, AppStream metainfo, and icon. Publishing, from Linux:

1. Push the repository and tag a release (the manifest names `v0.1.0`).
2. `python3 flatpak-cargo-generator.py Cargo.lock -o cargo-sources.json`
   (from flathub/flatpak-builder-tools) so the sandboxed build works
   offline.
3. Test: `flatpak-builder --user --install build-dir com.twarge.Calcium.json`
4. Submit: fork `flathub/flathub`, add the manifest plus
   `cargo-sources.json` on a new branch, open the PR.
