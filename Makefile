# vixen — common tasks.  `make help` lists them.
ARCH     := $(shell uname -m | sed s/arm64/aarch64/)
TRIPLE   := $(ARCH)-apple-darwin
APP      := target/release/bundle/macos/Vixen.app
DMG      := dist/Vixen.dmg

.PHONY: help build test lint fmt cli app dmg install install-cli install-app run watch clean icons tray-icon dist

help:            ## Show this help
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-12s %s\n", $$1, $$2}'

build:           ## Build CLI + library (release)
	cargo build --release

test:            ## Run all tests
	cargo test --workspace

lint:            ## Clippy + format check
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:             ## Format code
	cargo fmt --all

cli:             ## Build the `vixen` CLI
	cargo build --release -p vixen-cli

app:             ## Build the macOS app bundle (needs `cargo install tauri-cli --version ^2`)
	cd app/src-tauri && cargo tauri build

icons:           ## Regenerate app icons from app/assets/*.svg
	cd app/src-tauri && cargo tauri icon ../assets/icon.svg -o icons
	cd app/src-tauri/icons && rm -rf android ios Square*Logo.png StoreLogo.png 64x64.png
	$(MAKE) tray-icon

tray-icon:       ## Render the menu-bar template icon (needs rsvg-convert)
	rsvg-convert -h 36 app/assets/fox.svg -o app/src-tauri/icons/tray.png

dist: cli app   ## Package release archives for this Mac into dist/ (as CI does)
	@mkdir -p dist
	tar -czf dist/vixen-$(TRIPLE).tar.gz -C target/release vixen
	tar -czf dist/Vixen-$(ARCH).app.tar.gz -C target/release/bundle/macos Vixen.app
	cd dist && shasum -a 256 *.tar.gz > SHA256SUMS && cat SHA256SUMS

dmg: app         ## Package the app as a .dmg in dist/
	@mkdir -p dist
	hdiutil create -volname "Vixen" -srcfolder "$(APP)" -ov -format UDZO "$(DMG)"

install: install-cli install-app ## Install CLI to ~/.cargo/bin and app to /Applications

install-cli:     ## Install `vixen` to ~/.cargo/bin
	cargo install --path crates/vixen-cli --locked

install-app: app ## Copy the app to /Applications
	rm -rf "/Applications/Vixen.app"
	cp -R "$(APP)" /Applications/

run:             ## Run the app in dev mode
	cd app/src-tauri && cargo tauri dev

watch: cli       ## Live terminal view
	./target/release/vixen watch

clean:           ## Remove build output
	cargo clean
	rm -rf dist
