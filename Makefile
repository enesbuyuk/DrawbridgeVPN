APP_NAME := DrawbridgeVPN
BIN_NAME := drawbridgevpn
BUNDLE := target/release/$(APP_NAME).app
ICONSET := target/release/$(APP_NAME).iconset
ICNS := target/release/AppIcon.icns

.PHONY: build start bundle icon

build:
	cargo build --release

# Runs the raw binary (fast iteration). Dock/Finder icon only shows up
# correctly from the .app bundle produced by `make bundle` / `make start`.
run:
	cargo run --release

start: bundle
	open "$(BUNDLE)"

icon:
	rm -rf "$(ICONSET)" "$(ICNS)"
	mkdir -p "$(ICONSET)"
	sips -z 16 16   assets/logo_icon.png --out "$(ICONSET)/icon_16x16.png"
	sips -z 32 32   assets/logo_icon.png --out "$(ICONSET)/icon_16x16@2x.png"
	sips -z 32 32   assets/logo_icon.png --out "$(ICONSET)/icon_32x32.png"
	sips -z 64 64   assets/logo_icon.png --out "$(ICONSET)/icon_32x32@2x.png"
	sips -z 128 128 assets/logo_icon.png --out "$(ICONSET)/icon_128x128.png"
	sips -z 256 256 assets/logo_icon.png --out "$(ICONSET)/icon_128x128@2x.png"
	sips -z 256 256 assets/logo_icon.png --out "$(ICONSET)/icon_256x256.png"
	sips -z 512 512 assets/logo_icon.png --out "$(ICONSET)/icon_256x256@2x.png"
	sips -z 512 512 assets/logo_icon.png --out "$(ICONSET)/icon_512x512.png"
	cp assets/logo_icon.png "$(ICONSET)/icon_512x512@2x.png"
	iconutil -c icns "$(ICONSET)" -o "$(ICNS)"

bundle: build icon
	rm -rf "$(BUNDLE)"
	mkdir -p "$(BUNDLE)/Contents/MacOS" "$(BUNDLE)/Contents/Resources"
	cp target/release/$(BIN_NAME) "$(BUNDLE)/Contents/MacOS/$(APP_NAME)"
	cp "$(ICNS)" "$(BUNDLE)/Contents/Resources/AppIcon.icns"
	cp packaging/Info.plist "$(BUNDLE)/Contents/Info.plist"
	touch "$(BUNDLE)"
