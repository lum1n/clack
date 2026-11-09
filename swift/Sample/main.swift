// Tiny embedder. It was not compiled here: `swift` is not installed.
// Point the compiler at crates/clack-ffi/include/clack.h and libclack_ffi.
//
//   swiftc -import-objc-header clack-bridging.h -L ../target/debug -lclack_ffi \
//       Clack.swift Sample/main.swift -o clack-sample

import Foundation

do {
    let repo = try ClackSession.normalize(model: "https://huggingface.co/openai/whisper-tiny/tree/main?not-for-local-files=1")
    print(repo)
    print("Create a session with ClackSession(model:language:token:) when you want weights downloaded.")
    print("Hold-to-talk, the HUD, and keystrokes belong to the GPUI client, not this sample.")
} catch {
    fputs("\(error)\n", stderr)
    exit(1)
}
