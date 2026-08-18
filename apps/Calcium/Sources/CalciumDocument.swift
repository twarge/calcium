import SwiftUI
import UniformTypeIdentifiers

extension UTType {
    static let calciumDocument = UTType(exportedAs: "com.twarge.calcium.document")
    /// The same text format under another extension, owned by another app.
    static let calcaDocument = UTType(importedAs: "io.calca.document")
}

/// A Calcium document.
///
/// On disk a document carries its answers, so the file is self-contained and
/// readable anywhere. In the editor it does not: answers are stripped on open
/// and shown alongside the text instead, then written back on save.
///
/// That split is the point. If answers lived in the buffer they would have to
/// be rewritten on every keystroke, which fights the user's typing, undo and
/// selection. Keeping the buffer answer-free means the editor never edits the
/// text behind the user's back.
struct CalciumDocument: FileDocument {
    /// The text as edited: `=>` markers with nothing after them.
    var text: String

    // The two calculation extensions only, deliberately not plain text at
    // large: claiming public.plain-text put every .txt and .md on the
    // system into the document browser and its recents. A stray text file
    // can still be renamed to .calcium — the format *is* plain text.
    static var readableContentTypes: [UTType] {
        [.calciumDocument, .calcaDocument]
    }
    // Writable as well as readable, so opening one of these and saving writes
    // back in place rather than forcing a conversion. It is the same plain
    // text either way; only the extension differs.
    static var writableContentTypes: [UTType] {
        [.calciumDocument, .calcaDocument]
    }

    init(text: String = CalciumDocument.defaultText) {
        self.text = text
    }

    /// What a new document opens with: the sample text, or nothing, per the
    /// preference.
    static var defaultText: String {
        (UserDefaults.standard.object(forKey: "starterText") as? Bool ?? true)
            ? starter : ""
    }

    init(configuration: ReadConfiguration) throws {
        guard let data = configuration.file.regularFileContents else {
            throw CocoaError(.fileReadCorruptFile)
        }
        // Anything that is not valid UTF-8 is not a document we can edit
        // without destroying it, so refuse rather than lossily converting.
        guard let contents = String(data: data, encoding: .utf8) else {
            throw CocoaError(.fileReadInapplicableStringEncoding)
        }
        text = contents
    }

    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper {
        FileWrapper(regularFileWithContents: Self.fileContents(for: text))
    }

    /// The exact bytes a save writes. Shared with the iOS share sheet, so a
    /// shared document can never differ from a saved one.
    static func fileContents(for text: String) -> Data {
        // The buffer already carries fresh answers; recomputing here costs
        // little and guarantees the file is right even if a pass was still
        // pending when the save arrived.
        var onDisk = Engine.materializingAnswers(in: text)
        // A text file ends with a newline. Without this, appending at the end
        // of a document quietly drops the one it was opened with.
        if !onDisk.isEmpty && !onDisk.hasSuffix("\n") {
            onDisk.append("\n")
        }
        return Data(onDisk.utf8)
    }

    static let starter = """
        # Calcium

        Write math expressions and use `=>` to see the answer.

            1 + 2 => 3

        Odd units:

            walking speed = 1 mph
            walking speed in furlongs/fortnight
                => 2,688 furlongs/fortnight

        Compute with uncertainty:

            current = 2±0.1 mA
            resistance = 10±2 Ω
            voltage = current * resistance in mV
                => (20 ± 4.1) mV

        See the reference guide for more!

        """
}
