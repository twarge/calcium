import SwiftUI
import UniformTypeIdentifiers

extension UTType {
    static let calciumDocument = UTType(exportedAs: "com.twarge.calcium.document")
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

    static var readableContentTypes: [UTType] { [.calciumDocument, .plainText] }
    static var writableContentTypes: [UTType] { [.calciumDocument] }

    init(text: String = CalciumDocument.starter) {
        self.text = Engine.strippingAnswers(from: text)
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
        text = Engine.strippingAnswers(from: contents)
    }

    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper {
        var onDisk = Engine.materializingAnswers(in: text)
        // A text file ends with a newline. Without this, appending at the end
        // of a document quietly drops the one it was opened with.
        if !onDisk.isEmpty && !onDisk.hasSuffix("\n") {
            onDisk.append("\n")
        }
        return FileWrapper(regularFileWithContents: Data(onDisk.utf8))
    }

    static let starter = """
        # Untitled

        Write anything here. Indent a line to make it a calculation, and put
        `=>` where you want the answer.

            2 + 2 =>

            radius = 3
            area   = pi * radius^2 =>

            100 ft in m =>

        """
}
