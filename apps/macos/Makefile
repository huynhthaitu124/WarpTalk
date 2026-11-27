# WarpTalk macOS app. Requires macOS 14.2+, Xcode 16.3+ (Swift 6.1 toolchain, needed by LiveKit 2.19.0), `brew install xcodegen`.
PROJECT := WarpTalk.xcodeproj
SCHEME  := WarpTalk
BUILD   := build
APP     := $(BUILD)/Build/Products/Debug/WarpTalk.app
WEB_DIR := ../web
XCB     := xcodebuild -project $(PROJECT) -scheme $(SCHEME) -configuration Debug -destination 'platform=macOS' -derivedDataPath $(BUILD) MACOSX_DEPLOYMENT_TARGET=14.2  # packages (LiveKit) declare 10.15; Xcode 27 SDK needs 13+

.PHONY: core gen build run test clean open web-export bundle run-bundled release

# Rust copilot-core -> CopilotCore/CopilotCore.xcframework + CopilotCore.swift (UniFFI). Run once before `make gen`/`build`, and after Rust changes.
core:
	../../crates/pipeline/copilot-core/scripts/build-xcframework.sh

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

# Distributable: Release, universal (Apple silicon + Intel), web embedded, ad-hoc re-signed, zipped into ../../dist.
REL_APP := $(BUILD)/Build/Products/Release/WarpTalk.app
VERSION := $(shell grep -m1 'MARKETING_VERSION' project.yml | sed 's/.*"\(.*\)"/\1/')
release: gen web-export
	REQUIRE_WEB_BUNDLE=YES xcodebuild -project $(PROJECT) -scheme $(SCHEME) -configuration Release \
	  -destination 'generic/platform=macOS' -derivedDataPath $(BUILD) MACOSX_DEPLOYMENT_TARGET=14.2 \
	  ARCHS="arm64 x86_64" ONLY_ACTIVE_ARCH=NO build
	rm -rf $(BUILD)/dist && mkdir -p $(BUILD)/dist ../../dist
	ditto --norsrc $(REL_APP) $(BUILD)/dist/WarpTalk.app
	codesign --force --deep --sign - --preserve-metadata=entitlements,requirements,flags,runtime $(BUILD)/dist/WarpTalk.app
	codesign -v --strict --deep $(BUILD)/dist/WarpTalk.app
	lipo -info $(BUILD)/dist/WarpTalk.app/Contents/MacOS/WarpTalk
	COPYFILE_DISABLE=1 ditto -c -k --norsrc --keepParent $(BUILD)/dist/WarpTalk.app ../../dist/WarpTalk-$(VERSION)-macos.zip

clean:
	rm -rf $(BUILD) $(PROJECT)
