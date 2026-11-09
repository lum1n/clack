import Foundation

/// Thin wrapper over the C ABI in `clack.h`. The speech engine is the Rust core.
/// This file is not compiled in this environment: there is no Swift toolchain.
public final class ClackSession {
    private let pointer: OpaquePointer

    public init(model: String, language: String, token: String? = nil) throws {
        var error: UnsafeMutablePointer<CChar>?
        let created = model.withCString { model in
            language.withCString { language in
                if let token {
                    return token.withCString { token in
                        clack_create(model, language, token, &error)
                    }
                }
                return clack_create(model, language, nil, &error)
            }
        }
        if let created {
            pointer = created
            return
        }
        throw ClackError(message: take(error) ?? "could not create Clack")
    }

    deinit {
        clack_free(pointer)
    }

    public static func normalize(model: String) throws -> String {
        var error: UnsafeMutablePointer<CChar>?
        let result = model.withCString { clack_normalize_model_ref($0, &error) }
        if let result {
            return takeOwned(result)
        }
        throw ClackError(message: take(error) ?? "invalid model link")
    }

    public func startListening() throws {
        try call(clack_start_listening)
    }

    public func stopListening() throws {
        try call(clack_stop_listening)
    }

    public func cancelListening() throws {
        try call(clack_cancel_listening)
    }

    public func pushAudio(_ samples: [Float], sampleRate: UInt32) throws {
        var error: UnsafeMutablePointer<CChar>?
        let ok = samples.withUnsafeBufferPointer { buffer in
            clack_push_audio(pointer, buffer.baseAddress, buffer.count, sampleRate, &error)
        }
        if ok == 0 {
            throw ClackError(message: take(error) ?? "pushAudio failed")
        }
    }

    public func poll() -> TranscriptUpdate? {
        var isFinal: Int32 = 0
        var cancelled: Int32 = 0
        var text: UnsafeMutablePointer<CChar>?
        if clack_poll(pointer, &isFinal, &cancelled, &text) == 0 {
            return nil
        }
        return TranscriptUpdate(
            text: text.map(takeOwned) ?? "",
            isFinal: isFinal != 0,
            cancelled: cancelled != 0
        )
    }

    public var transcript: String {
        guard let text = clack_transcript(pointer) else { return "" }
        return takeOwned(text)
    }

    public var isListening: Bool {
        clack_is_listening(pointer) != 0
    }

    public func speechActivity() -> SpeechActivity {
        var activity = ClackActivity()
        clack_speech_activity(pointer, &activity)
        let levels = withUnsafeBytes(of: activity.levels) { raw -> [Float] in
            let buffer = raw.bindMemory(to: Float.self)
            return Array(buffer)
        }
        return SpeechActivity(speaking: activity.speaking != 0, levels: levels)
    }

    private func call(_ function: (OpaquePointer?, UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32) throws {
        var error: UnsafeMutablePointer<CChar>?
        if function(pointer, &error) == 0 {
            throw ClackError(message: take(error) ?? "Clack call failed")
        }
    }
}

public struct TranscriptUpdate {
    public var text: String
    public var isFinal: Bool
    public var cancelled: Bool
}

public struct SpeechActivity {
    public var speaking: Bool
    public var levels: [Float]
}

public struct ClackError: Error, CustomStringConvertible {
    public var message: String
    public var description: String { message }
}

private func take(_ pointer: UnsafeMutablePointer<CChar>?) -> String? {
    guard let pointer else { return nil }
    return takeOwned(pointer)
}

private func takeOwned(_ pointer: UnsafeMutablePointer<CChar>) -> String {
    let text = String(cString: pointer)
    clack_string_free(pointer)
    return text
}
