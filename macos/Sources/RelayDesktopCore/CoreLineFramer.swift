import Foundation

/// Incremental NDJSON framing. Only bytes from the new read are examined;
/// an incomplete frame is retained without rescanning or copying its prefix.
struct CoreLineFramer {
    let maximumLine: Int
    private var buffer = Data()
    private(set) var bytesScanned = 0

    init(maximumLine: Int) { self.maximumLine = maximumLine }

    mutating func append(_ bytes: UnsafeBufferPointer<UInt8>, frame: (Data) -> Void) -> Bool {
        guard !bytes.isEmpty else { return true }
        let previousCount = buffer.count
        buffer.append(bytes.baseAddress!, count: bytes.count)
        var frameStart = buffer.startIndex
        var length = previousCount
        for offset in bytes.indices {
            bytesScanned += 1
            if bytes[offset] == 0x0A {
                let end = buffer.startIndex + previousCount + offset
                frame(Data(buffer[frameStart..<end]))
                frameStart = end + 1
                length = 0
            } else {
                length += 1
                if length > maximumLine { return false }
            }
        }
        // Compact once per read, only if at least one frame was consumed.
        if frameStart != buffer.startIndex { buffer = Data(buffer[frameStart...]) }
        return true
    }
}
