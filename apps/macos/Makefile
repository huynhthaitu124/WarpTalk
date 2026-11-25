# WarpTalk macOS app. Requires macOS 14.2+, Xcode 16.3+ (Swift 6.1 toolchain, needed by LiveKit 2.19.0), `brew install xcodegen`.
PROJECT := WarpTalk.xcodeproj
SCHEME  := WarpTalk
BUILD   := build
APP     := $(BUILD)/Build/Products/Debug/WarpTalk.app
WEB_DIR := ../web
XCB     := xcodebuild -project $(PROJECT) -scheme $(SCHEME) -configuration Debug -destination 'platform=macOS' -derivedDataPath $(BUILD) MACOSX_DEPLOYMENT_TARGET=14.2  # packages (LiveKit) declare 10.15; Xcode 27 SDK needs 13+

.PHONY: gen build run test clean open web-export bundle run-bundled

gen:
	xcodegen generate

open: gen
	open $(PROJECT)

# Loads the web app from the URL in Settings (default http://localhost:3000, `cd apps/web && npm run dev`).
build: gen
	$(XCB) build

run: build
	open $(APP)

test: gen
	$(XCB) test

# Static export of the Next.js app into apps/web/out (needs `output: 'export'` in apps/web/next.config).
web-export:
	cd $(WEB_DIR) && npm run build

# App with the web app embedded (warptalk://app/, API proxied to the backend URL from Settings). Fails if no export exists.
bundle: gen web-export
	REQUIRE_WEB_BUNDLE=YES $(XCB) build

run-bundled: bundle
	open $(APP)

clean:
	rm -rf $(BUILD) $(PROJECT)
