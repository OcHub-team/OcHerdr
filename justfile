# OcHerdr local development and acceptance commands.

set shell := ["zsh", "-cu"]

# GPUI's macOS/Metal build requires a complete Xcode installation.
export DEVELOPER_DIR := env_var_or_default("DEVELOPER_DIR", "/Applications/Xcode.app/Contents/Developer")

app_bundle := "target/qa/OcHerdr.app"
app_binary := app_bundle / "Contents/MacOS/ocherdr"

# Show the available workflow instead of starting a GUI unexpectedly.
default:
    @just --list

# Check the local toolchain before a development or acceptance run.
doctor:
    @command -v cargo >/dev/null || { echo "error: cargo is not installed" >&2; exit 1; }
    @command -v rustc >/dev/null || { echo "error: rustc is not installed" >&2; exit 1; }
    @if [[ -n "${ZIG:-}" ]]; then zig_bin="$ZIG"; elif [[ -x /opt/homebrew/opt/zig@0.15/bin/zig ]]; then zig_bin=/opt/homebrew/opt/zig@0.15/bin/zig; elif [[ -x /usr/local/opt/zig@0.15/bin/zig ]]; then zig_bin=/usr/local/opt/zig@0.15/bin/zig; else zig_bin="$(command -v zig || true)"; fi; [[ -n "$zig_bin" ]] || { echo "error: Zig 0.15.2 is required (brew install zig@0.15)" >&2; exit 1; }; version="$($zig_bin version)"; [[ "$version" == 0.15.2* ]] || { echo "error: Zig 0.15.2 is required, found $version at $zig_bin" >&2; exit 1; }; echo "Rust $(rustc --version | awk '{print $2}') · Zig $version · macOS $(sw_vers -productVersion)"
    @if command -v herdr >/dev/null; then echo "Herdr $(herdr --version 2>/dev/null || echo installed)"; else echo "warning: herdr is not on PATH; the GUI will open without a local session"; fi

# Start the debug build for day-to-day development.
run: doctor
    cargo run -p ocherdr --locked

# Start the optimized binary without packaging an app bundle.
run-release: doctor
    cargo run --release -p ocherdr --locked

# Fast workspace type-check.
check:
    cargo check --workspace --all-targets --locked

# Apply Rust formatting.
fmt:
    cargo fmt --all

# Run the warning-free lint gate used by CI.
lint:
    cargo clippy --workspace --all-targets --locked -- -D warnings

# Run all unit and documentation tests.
test:
    cargo test --workspace --locked

# Run the same quality gate as GitHub Actions without changing source files.
ci:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked

# Build the optimized OcHerdr executable.
build-release: doctor
    cargo build --release -p ocherdr --locked

# Create an ad-hoc signed macOS app bundle at target/qa/OcHerdr.app.
qa-app: build-release
    install -d "{{ app_bundle }}/Contents/MacOS"
    install -m 755 target/release/ocherdr "{{ app_binary }}"
    install -m 644 packaging/macos/Info.plist "{{ app_bundle }}/Contents/Info.plist"
    codesign --force --deep --sign - "{{ app_bundle }}"
    @echo "Built {{ app_bundle }}"

# Full local acceptance: CI gate, package the app, then launch a fresh instance.
accept: ci qa-app
    open -n "{{ app_bundle }}"

# Open the last packaged QA app without rebuilding it.
open:
    @test -d "{{ app_bundle }}" || { echo "error: run 'just qa-app' first" >&2; exit 1; }
    open -n "{{ app_bundle }}"

# Remove Cargo and packaged QA build artifacts.
clean:
    cargo clean
