# WarpTalk macOS app. Requires macOS 14.2+, Xcode 15.3+, `brew install xcodegen`.
PROJECT := WarpTalk.xcodeproj
SCHEME  := WarpTalk
BUILD   := build
APP     := $(BUILD)/Build/Products/Debug/WarpTalk.app

.PHONY: gen build run test clean open

gen:
	xcodegen generate

open: gen
	open $(PROJECT)

build: gen
	xcodebuild -project $(PROJECT) -scheme $(SCHEME) -configuration Debug \
		-destination 'platform=macOS' -derivedDataPath $(BUILD) build

run: build
	open $(APP)

test: gen
	xcodebuild -project $(PROJECT) -scheme $(SCHEME) -configuration Debug \
		-destination 'platform=macOS' -derivedDataPath $(BUILD) test

clean:
	rm -rf $(BUILD) $(PROJECT)
