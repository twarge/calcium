import Foundation

/// Per-document view state: zoom and cursor position.
///
/// Kept in an extended attribute on the file, because it belongs to the
/// document but not in it — a `.calcium` file is plain text, readable
/// anywhere, and must not grow app-private furniture. This is the same
/// mechanism TextEdit uses to remember the cursor. It travels with the file
/// on Apple filesystems and is silently dropped by transports that cannot
/// carry it (git, mail), which is the right behaviour for view state.
struct DocumentViewState: Codable, Equatable {
    /// Zoom relative to the Preferences font size. 1 is unzoomed.
    var scale: Double = 1
    /// Insertion point, as a UTF-16 offset. Clamped on restore, so a file
    /// edited elsewhere cannot put the cursor out of bounds.
    var cursor: Int = 0

    private static let name = "com.twarge.calcium.viewstate"

    static func load(from url: URL) -> DocumentViewState? {
        url.withUnsafeFileSystemRepresentation { path -> DocumentViewState? in
            guard let path else { return nil }
            let size = getxattr(path, name, nil, 0, 0, 0)
            guard size > 0 else { return nil }
            var data = Data(count: size)
            let read = data.withUnsafeMutableBytes {
                getxattr(path, name, $0.baseAddress, size, 0, 0)
            }
            guard read == size else { return nil }
            return try? JSONDecoder().decode(DocumentViewState.self, from: data)
        }
    }

    func save(to url: URL) {
        guard let data = try? JSONEncoder().encode(self) else { return }
        url.withUnsafeFileSystemRepresentation { path in
            guard let path else { return }
            _ = data.withUnsafeBytes {
                setxattr(path, Self.name, $0.baseAddress, data.count, 0, 0)
            }
        }
    }
}
