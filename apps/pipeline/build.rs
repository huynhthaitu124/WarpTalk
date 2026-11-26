//! libwebrtc's static library carries Objective-C categories (e.g. `NSString+StdString`);
//! on macOS the linker drops them unless `-ObjC` is passed, which crashes the pipeline at
//! peer-connection setup with "unrecognized selector stringForStdString:".
fn main() {
    let macos = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "macos");
    if macos && std::env::var_os("CARGO_FEATURE_LIVEKIT_RTC").is_some() {
        println!("cargo:rustc-link-arg-bins=-ObjC");
    }
}
